//! Role-bound signing for HTTP coordinators backed by an external ledger.
use crate::{BuyerClient, Error, HttpCoordinator, Result, SellerClient};
use ed25519_dalek::Signer;
use serde_json::{json, Value};
use settle463_core::{
    ident::hex_string,
    message_id,
    settlement::{Agreement, ChainOutcome, Review, Submission},
    wire::{Request, Settlement},
    SignedMessage,
};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub enum LedgerAction<'a> {
    Fund,
    CommitDelivery(&'a SignedMessage),
    Release(&'a SignedMessage, &'a SignedMessage),
    Refund(Option<&'a SignedMessage>),
}
impl LedgerAction<'_> {
    pub fn name(self) -> &'static str {
        match self {
            Self::Fund => "fund",
            Self::CommitDelivery(_) => "commit_delivery",
            Self::Release(..) => "release",
            Self::Refund(_) => "refund",
        }
    }
    fn command(self) -> Value {
        let mut cmd = json!({"action":self.name()});
        match self {
            Self::Release(_, v) => cmd["verdict_id"] = hex_string(v.id()).into(),
            Self::CommitDelivery(d) => cmd["delivery_id"] = hex_string(d.id()).into(),
            _ => {}
        }
        cmd
    }
}

/// Implementations must decode all instructions against the signed agreement before using the key.
pub trait LedgerSigning: Send + Sync {
    fn settlement(&self, request: &Request) -> Result<Settlement>;
    fn sign(
        &self,
        agreement: &Agreement,
        action: LedgerAction<'_>,
        draft: &Value,
        public_hex: &str,
        sign: &dyn Fn(&[u8]) -> [u8; 64],
    ) -> Result<String>;
    fn assemble(
        &self,
        agreement: &Agreement,
        action: LedgerAction<'_>,
        draft: &Value,
        signatures: &[String],
    ) -> Result<Value>;
}

impl HttpCoordinator {
    pub(crate) fn ledger_action(
        &self,
        agreement: &Agreement,
        action: LedgerAction<'_>,
        second: Option<&HttpCoordinator>,
    ) -> Result<Submission> {
        let (_, q) = agreement.terms(self.environment_name())?;
        let signer = self.ledger.as_deref().ok_or(Error::Unavailable)?;
        for _ in 0..2 {
            let mut cmd = action.command();
            let prepared = self.json_action(&q.order_id, "prepare", cmd.clone())?;
            let sign_as = |party: &HttpCoordinator| {
                party.ledger.as_deref().ok_or(Error::Unavailable)?.sign(
                    agreement,
                    action,
                    &prepared,
                    &party.identity.public_hex(),
                    &|message| party.identity.0.sign(message).to_bytes(),
                )
            };
            let mut signatures = vec![sign_as(self)?];
            if let Some(party) = second {
                signatures.push(sign_as(party)?);
            }
            let signed = signer.assemble(agreement, action, &prepared, &signatures)?;
            let signature = signed["signature"].as_str().ok_or(Error::Linkage)?;
            let id = hex_string(&message_id(signature.as_bytes()));
            cmd["transaction"] = signed;
            let mut sub = match self.submit_command(&q.order_id, id.clone(), cmd) {
                Ok(s) => s,
                Err(Error::Unavailable | Error::Http { status: 503, .. }) => Submission {
                    intent_id: id,
                    outcome: ChainOutcome::Unknown,
                },
                Err(e) => return Err(e),
            };
            let until = Instant::now() + Duration::from_secs(100);
            while matches!(sub.outcome, ChainOutcome::Pending | ChainOutcome::Unknown)
                && Instant::now() < until
            {
                std::thread::sleep(Duration::from_millis(300));
                sub.outcome = self.reconcile_for_order(&q.order_id, &sub.intent_id)?;
            }
            match sub.outcome {
                ChainOutcome::Expired => continue,
                ChainOutcome::FinalizedSuccess => return Ok(sub),
                ChainOutcome::FinalizedFailure => return Err(Error::PolicyRejected),
                _ => return Err(Error::Unavailable),
            }
        }
        Err(Error::Unavailable)
    }
}

impl BuyerClient {
    pub fn fund(&self, seller: &SellerClient, agreement: &Agreement) -> Result<Submission> {
        if self.actor.env() != seller.actor.env() {
            return Err(Error::Environment);
        }
        if self.actor.env().starts_with("sim:") {
            let prepared = self.actor.backend.prepare_fund(agreement.clone())?;
            let signatures = settle463_core::settlement::FundSignatures {
                buyer: self.sign_fund(&prepared)?,
                seller: seller.sign_fund(&prepared)?,
            };
            return Ok(self.actor.backend.submit_fund(&prepared, &signatures)?);
        }
        let review = Review::from_agreement(agreement, self.actor.env())?;
        self.quote_policy(&agreement.request, &agreement.quote)?;
        let (_, q) = agreement.terms(self.actor.env())?;
        seller.terms_policy(q)?;
        for actor in [&self.actor, &seller.actor] {
            let accepted = actor
                .transport
                .accepted(&agreement.request)?
                .ok_or(Error::Linkage)?;
            if accepted.id() != agreement.acceptance.id() {
                return Err(Error::Linkage);
            }
        }
        if review.buyer != self.actor.identity.public_hex()
            || review.seller != seller.actor.identity.public_hex()
        {
            return Err(Error::PolicyRejected);
        }
        self.actor.coordinator()?.ledger_action(
            agreement,
            LedgerAction::Fund,
            Some(seller.actor.coordinator()?),
        )
    }
}
