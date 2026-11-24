//! Funding agreements, signer reviews and settlement results.

use crate::ident::{hex, hex_string};
use crate::wire::{Artifact, Asset, Body, Deadlines, Quote, Request, Settlement};
use crate::{check_quote_request, domain_preimage, message_id, SignedMessage};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum FundingError {
    #[error(transparent)]
    Wire(#[from] crate::Error),
    #[error("LINKAGE_FAILED")]
    Linkage,
    #[error("ENVIRONMENT_MISMATCH")]
    Environment,
    #[error("FUND_DEADLINE")]
    Deadline,
    #[error("ALREADY_COMPLETE")]
    AlreadyFunded,
    #[error("SUBMISSION_PENDING")]
    Pending,
    #[error("INSUFFICIENT_FUNDS")]
    InsufficientFunds,
    #[error("order is missing")]
    OrderMissing,
    #[error("action is unavailable in this order state")]
    WrongState,
    #[error("delivery deadline passed")]
    DeliveryDeadline,
    #[error("resolution deadline passed")]
    ResolveDeadline,
    #[error("refund deadline has not been reached")]
    TooEarly,
    #[error("destination balance overflows")]
    BalanceOverflow,
    #[error("ledger unavailable")]
    Unavailable,
    #[error("invalid opening balance or clock")]
    InvalidGenesis,
}
pub type Result<T> = std::result::Result<T, FundingError>;

#[derive(Clone, Debug)]
pub struct Agreement {
    pub request: SignedMessage,
    pub quote: SignedMessage,
    pub acceptance: SignedMessage,
}

impl Agreement {
    pub fn terms(&self, env: &str) -> Result<(&Request, &Quote)> {
        if [&self.request, &self.quote, &self.acceptance]
            .iter()
            .any(|m| m.unsigned().environment != env)
        {
            return Err(FundingError::Environment);
        }
        check_quote_request(&self.quote, &self.request)?;
        let (Body::Request(r), Body::Quote(q), Body::Accept(a)) = (
            &self.request.unsigned().payload,
            &self.quote.unsigned().payload,
            &self.acceptance.unsigned().payload,
        ) else {
            return Err(FundingError::Linkage);
        };
        if self.acceptance.unsigned().issuer != r.buyer
            || a.request_id != q.request_id
            || a.quote_id != hex_string(self.quote.id())
            || a.order_id != q.order_id
            || a.amount != q.amount
            || a.asset != q.asset
            || a.environment != env
        {
            return Err(FundingError::Linkage);
        }
        Ok((r, q))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Review {
    pub environment: String,
    pub order_id: String,
    pub request_id: String,
    pub quote_id: String,
    pub acceptance_id: String,
    pub policy_hash: String,
    pub buyer: String,
    pub seller: String,
    pub verifier: String,
    pub receipt_issuer: String,
    pub principal: String,
    pub asset: Asset,
    pub buyer_destination: String,
    pub seller_destination: String,
    pub escrow_account: String,
    pub deadlines: Deadlines,
}

impl Review {
    pub fn sim_fund(agreement: &Agreement, environment: &str) -> Result<Self> {
        if !environment.starts_with("sim:") {
            return Err(FundingError::Environment);
        }
        Self::from_agreement(agreement, environment)
    }

    /// Signed settlement terms. The Solana adapter independently derives and checks every address.
    pub fn from_agreement(agreement: &Agreement, environment: &str) -> Result<Self> {
        let (_, q) = agreement.terms(environment)?;
        let (buyer_destination, seller_destination, escrow_account) = match &q.settlement {
            Settlement::Sim {
                buyer_destination,
                seller_destination,
                ..
            } => (
                buyer_destination,
                seller_destination,
                format!("sim-escrow:{}", q.order_id),
            ),
            Settlement::Solana {
                buyer_token_account,
                seller_token_account,
                escrow_pda,
                ..
            } => (
                buyer_token_account,
                seller_token_account,
                escrow_pda.clone(),
            ),
        };
        Ok(Self {
            environment: environment.to_owned(),
            order_id: q.order_id.clone(),
            request_id: q.request_id.clone(),
            quote_id: hex_string(agreement.quote.id()),
            acceptance_id: hex_string(agreement.acceptance.id()),
            policy_hash: q.policy_hash.clone(),
            buyer: q.buyer.clone(),
            seller: q.seller.clone(),
            verifier: q.verifier.clone(),
            receipt_issuer: q.receipt_issuer.clone(),
            principal: q.amount.clone(),
            asset: q.asset.clone(),
            buyer_destination: buyer_destination.clone(),
            seller_destination: seller_destination.clone(),
            escrow_account,
            deadlines: q.deadlines.clone(),
        })
    }

    /// Both parties authorize this single simulator debit and escrow credit.
    pub fn signing_bytes(&self) -> Result<Vec<u8>> {
        let value = serde_json::to_value(self).map_err(crate::Error::from)?;
        Ok(domain_preimage(b"0x463/sim-fund/0.1", &value)?)
    }
}

#[derive(Clone, Debug)]
pub struct PreparedFund {
    pub agreement: Agreement,
    pub review: Review,
}

impl PreparedFund {
    pub fn check(&self, environment: &str) -> Result<()> {
        if self.review != Review::sim_fund(&self.agreement, environment)? {
            return Err(FundingError::Linkage);
        }
        Ok(())
    }
}

pub struct FundSignatures {
    pub buyer: [u8; 64],
    pub seller: [u8; 64],
}

impl FundSignatures {
    pub fn verify(&self, review: &Review) -> Result<String> {
        let bytes = review.signing_bytes()?;
        for (id, signature) in [(&review.buyer, &self.buyer), (&review.seller, &self.seller)] {
            let key = VerifyingKey::from_bytes(&hex(id)?).map_err(|_| crate::Error::Signature)?;
            key.verify_strict(&bytes, &Signature::from_bytes(signature))
                .map_err(|_| crate::Error::Signature)?;
        }
        Ok(hex_string(&message_id(&bytes)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderState {
    Funded,
    Delivered,
    Paid,
    Refunded,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderView {
    pub state: OrderState,
    pub review: Review,
    pub held: u64,
    pub paid: u64,
    pub refunded: u64,
    pub delivery_id: Option<String>,
    pub verdict_id: Option<String>,
    pub terminal_clock: Option<i64>,
    pub terminal_locator: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChainOutcome {
    Pending,
    FinalizedSuccess,
    FinalizedFailure,
    Expired,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submission {
    pub intent_id: String,
    pub outcome: ChainOutcome,
}

pub trait SettlementBackend: Send + Sync {
    fn environment(&self) -> &str;
    fn clock(&self) -> Result<i64>;
    fn prepare_fund(&self, agreement: Agreement) -> Result<PreparedFund>;
    fn submit_fund(&self, fund: &PreparedFund, signatures: &FundSignatures) -> Result<Submission>;
    fn prepare_action(
        &self,
        order_id: &str,
        action: Action,
        evidence: Option<&SignedMessage>,
    ) -> Result<ActionReview>;
    fn submit_action(
        &self,
        review: &ActionReview,
        evidence: Option<&SignedMessage>,
        signature: Option<&[u8; 64]>,
    ) -> Result<Submission>;
    fn inspect(&self, order_id: &str) -> Result<Option<OrderView>>;
    fn reconcile(&self, intent_id: &str) -> Result<ChainOutcome>;
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    CommitDelivery,
    Release,
    Refund,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionReview {
    pub action: Action,
    pub terms: Review,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict_id: Option<String>,
}

pub fn check_delivery(terms: &Review, message: &SignedMessage) -> Result<()> {
    let Body::Delivery(d) = &message.unsigned().payload else {
        return Err(FundingError::Linkage);
    };
    if message.unsigned().environment != terms.environment
        || message.unsigned().issuer != terms.seller
        || d.order_id != terms.order_id
        || d.quote_id != terms.quote_id
        || d.policy_hash != terms.policy_hash
    {
        return Err(FundingError::Linkage);
    }
    Ok(())
}

pub fn check_verdict(
    terms: &Review,
    delivery: &SignedMessage,
    message: &SignedMessage,
) -> Result<()> {
    check_delivery(terms, delivery)?;
    let (Body::Delivery(d), Body::Verdict(v)) =
        (&delivery.unsigned().payload, &message.unsigned().payload)
    else {
        return Err(FundingError::Linkage);
    };
    if message.unsigned().environment != terms.environment
        || message.unsigned().issuer != terms.verifier
        || v.order_id != terms.order_id
        || v.quote_id != terms.quote_id
        || v.policy_hash != terms.policy_hash
        || v.delivery_id != hex_string(delivery.id())
        || v.observed_digest
            .as_ref()
            .is_some_and(|hash| hash != &d.digest)
        || v.observed_length
            .as_ref()
            .is_some_and(|size| size != &d.length)
    {
        return Err(FundingError::Linkage);
    }
    Ok(())
}

pub fn check_pass_policy(artifact: &Artifact, verdict: &SignedMessage) -> Result<()> {
    let Body::Verdict(v) = &verdict.unsigned().payload else {
        return Err(FundingError::Linkage);
    };
    if v.decision == "PASS"
        && (v.observed_digest.as_ref() != Some(&artifact.digest)
            || v.observed_length.as_ref() != Some(&artifact.length))
    {
        return Err(FundingError::Linkage);
    }
    Ok(())
}

impl ActionReview {
    /// Reconstructs all settlement terms from a funded order and its signed action evidence.
    pub fn for_order(
        action: Action,
        view: &OrderView,
        evidence: Option<&SignedMessage>,
    ) -> Result<Self> {
        let terms = &view.review;
        let (delivery_id, verdict_id) = match (action, evidence) {
            (Action::CommitDelivery, Some(m)) => {
                check_delivery(terms, m)?;
                (Some(hex_string(m.id())), None)
            }
            (Action::Release, Some(m)) => {
                let Body::Verdict(v) = &m.unsigned().payload else {
                    return Err(FundingError::Linkage);
                };
                if m.unsigned().environment != terms.environment
                    || m.unsigned().issuer != terms.verifier
                    || v.order_id != terms.order_id
                    || v.quote_id != terms.quote_id
                    || v.policy_hash != terms.policy_hash
                    || v.decision != "PASS"
                    || Some(&v.delivery_id) != view.delivery_id.as_ref()
                {
                    return Err(FundingError::Linkage);
                }
                (view.delivery_id.clone(), Some(hex_string(m.id())))
            }
            (Action::Refund, None) => (view.delivery_id.clone(), None),
            _ => return Err(FundingError::Linkage),
        };
        Ok(Self {
            action,
            terms: terms.clone(),
            delivery_id,
            verdict_id,
        })
    }
    pub fn signing_bytes(&self) -> Result<Vec<u8>> {
        let value = serde_json::to_value(self).map_err(crate::Error::from)?;
        Ok(domain_preimage(b"0x463/sim-action/0.1", &value)?)
    }
    pub fn intent_id(&self) -> Result<String> {
        Ok(hex_string(&message_id(&self.signing_bytes()?)))
    }
    pub fn verify_authority(&self, signature: Option<&[u8; 64]>) -> Result<()> {
        let issuer = match self.action {
            Action::CommitDelivery => &self.terms.seller,
            Action::Release => &self.terms.verifier,
            Action::Refund => {
                return if signature.is_none() {
                    Ok(())
                } else {
                    Err(FundingError::Linkage)
                }
            }
        };
        let key = VerifyingKey::from_bytes(&hex(issuer)?).map_err(|_| crate::Error::Signature)?;
        key.verify_strict(
            &self.signing_bytes()?,
            &Signature::from_bytes(signature.ok_or(crate::Error::Signature)?),
        )
        .map_err(|_| crate::Error::Signature.into())
    }
}
