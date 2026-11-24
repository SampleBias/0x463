//! Funding agreements, signer reviews and settlement results.

use crate::ident::{hex, hex_string};
use crate::wire::{Asset, Body, Deadlines, Quote, Request, Settlement};
use crate::{check_quote_request, domain_preimage, message_id, SignedMessage};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Serialize;

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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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
        let (_, q) = agreement.terms(environment)?;
        let Settlement::Sim {
            buyer_destination,
            seller_destination,
            ..
        } = &q.settlement
        else {
            return Err(FundingError::Environment);
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
            escrow_account: format!("sim-escrow:{}", q.order_id),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderState {
    Funded,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderView {
    pub state: OrderState,
    pub review: Review,
    pub held: u64,
    pub paid: u64,
    pub refunded: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainOutcome {
    Pending,
    FinalizedSuccess,
    FinalizedFailure,
    Expired,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submission {
    pub intent_id: String,
    pub outcome: ChainOutcome,
}

pub trait SettlementBackend: Send + Sync {
    fn environment(&self) -> &str;
    fn clock(&self) -> Result<i64>;
    fn prepare_fund(&self, agreement: Agreement) -> Result<PreparedFund>;
    fn submit_fund(&self, fund: &PreparedFund, signatures: &FundSignatures) -> Result<Submission>;
    fn inspect(&self, order_id: &str) -> Result<Option<OrderView>>;
    fn reconcile(&self, intent_id: &str) -> Result<ChainOutcome>;
}
