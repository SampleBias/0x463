//! Signed buyer and seller negotiation over a replaceable message transport.

mod fulfillment;
mod http;
mod ledger;
pub use ledger::{LedgerAction, LedgerSigning};
mod transport;
pub use fulfillment::{StoredArtifact, VerifierClient};
pub use http::{DirectPeer, HttpCoordinator, Role};
pub use settle463_core::settlement::SettlementBackend;
pub use transport::{MemoryTransport, MessageTransport};

use ed25519_dalek::{Signer, SigningKey};
use settle463_core::{
    check_quote_request, check_quote_terms,
    ident::{decimal, hex_string, timestamp},
    order_id, policy_hash,
    settlement::{FundingError, OrderView, PreparedFund},
    sign,
    wire::*,
    SignedMessage, UnsignedMessage,
};
use std::{collections::BTreeSet, sync::Arc};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("POLICY_REJECTED")]
    PolicyRejected,
    #[error("ACCEPTANCE_EXISTS")]
    AcceptanceExists,
    #[error("ENVIRONMENT_MISMATCH")]
    Environment,
    #[error("LINKAGE_FAILED")]
    Linkage,
    #[error("order already settled as {0:?}")]
    AlreadyComplete(settle463_core::settlement::OrderState),
    #[error("HTTP {status}: {code}")]
    Http { status: u16, code: String },
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("message not found")]
    NotFound,
    #[error("message store or randomness unavailable")]
    Unavailable,
    #[error(transparent)]
    Wire(#[from] settle463_core::Error),
    #[error(transparent)]
    Funding(#[from] FundingError),
}
pub type Result<T> = std::result::Result<T, Error>;

fn policy(allowed: bool) -> Result<()> {
    if allowed {
        Ok(())
    } else {
        Err(Error::PolicyRejected)
    }
}

fn random_seed() -> Result<[u8; 32]> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| Error::Unavailable)?;
    Ok(bytes)
}
#[derive(Clone)]
pub struct Identity(SigningKey);
impl Identity {
    pub fn generate() -> Result<Self> {
        Ok(Self::from_seed(&random_seed()?))
    }
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self(SigningKey::from_bytes(seed))
    }
    pub fn public_hex(&self) -> String {
        hex_string(self.0.verifying_key().as_bytes())
    }
    pub fn public_base58(&self) -> String {
        const DIGITS: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
        let mut bytes = self.0.verifying_key().to_bytes();
        let zeros = bytes.iter().take_while(|&&b| b == 0).count();
        let mut digits = Vec::new();
        let mut start = zeros;
        while start < bytes.len() {
            let mut rem = 0u16;
            for byte in &mut bytes[start..] {
                let value = rem * 256 + u16::from(*byte);
                *byte = (value / 58) as u8;
                rem = value % 58;
            }
            digits.push(DIGITS[usize::from(rem)] as char);
            while start < bytes.len() && bytes[start] == 0 {
                start += 1;
            }
        }
        let mut out = "1".repeat(zeros);
        out.extend(digits.into_iter().rev());
        out
    }
}

pub struct BuyerPolicy {
    pub trusted_publishers: BTreeSet<String>,
    pub max_principal: u64,
    pub allowed_verifiers: BTreeSet<String>,
    pub allowed_services: BTreeSet<String>,
    pub allowed_assets: BTreeSet<String>,
    pub allowed_environments: BTreeSet<String>,
    pub max_deliver_window_seconds: u64,
    pub max_resolve_window_seconds: u64,
}
pub struct SellerPolicy {
    pub min_principal: u64,
    pub allowed_verifiers: BTreeSet<String>,
    pub allowed_environments: BTreeSet<String>,
}
pub struct RequestTerms {
    pub asset: Asset,
    pub max_principal: u64,
    pub allowed_verifiers: Vec<String>,
    pub max_bytes: u64,
    pub expires_at: i64,
}
pub struct QuoteTerms {
    pub amount: u64,
    pub verifier: String,
    pub receipt_issuer: String,
    pub policy: Policy,
    pub deadlines: Deadlines,
    pub expires_at: i64,
    pub fees: Fees,
    pub artifact_store: ArtifactStore,
}
struct Actor {
    identity: Identity,
    transport: Arc<dyn MessageTransport>,
    backend: Arc<dyn SettlementBackend>,
    http: Option<Arc<HttpCoordinator>>,
}
impl Actor {
    fn env(&self) -> &str {
        self.backend.environment()
    }
    fn unsigned(&self, payload: Body) -> UnsignedMessage {
        UnsignedMessage {
            protocol: "0x463".into(),
            version: "0.1".into(),
            environment: self.env().into(),
            issuer: self.identity.public_hex(),
            payload,
        }
    }
    fn publish(&self, msg: UnsignedMessage) -> Result<SignedMessage> {
        msg.validate(self.env())
            .map_err(|_| Error::PolicyRejected)?;
        let signed = sign(msg, &self.identity.0)?;
        self.transport.publish(signed.clone())?;
        Ok(signed)
    }
    fn check_fund(&self, fund: &PreparedFund) -> Result<()> {
        fund.check(self.env())?;
        let acceptance = self
            .transport
            .accepted(&fund.agreement.request)?
            .ok_or(Error::Linkage)?;
        if acceptance.id() != fund.agreement.acceptance.id() {
            return Err(Error::Linkage);
        }
        policy(self.backend.clock()? < timestamp(&fund.review.deadlines.fund_by)?)?;
        Ok(())
    }
    fn sign_review(&self, fund: &PreparedFund) -> Result<[u8; 64]> {
        Ok(self
            .identity
            .0
            .sign(&fund.review.signing_bytes()?)
            .to_bytes())
    }
}

pub struct BuyerClient {
    actor: Actor,
    policy: BuyerPolicy,
}
impl BuyerClient {
    pub fn new(
        identity: Identity,
        policy: BuyerPolicy,
        transport: Arc<dyn MessageTransport>,
        backend: Arc<dyn SettlementBackend>,
    ) -> Self {
        Self {
            actor: Actor {
                identity,
                transport,
                backend,
                http: None,
            },
            policy,
        }
    }
    fn permits_offer(&self, offer: &SignedMessage, now: i64) -> bool {
        let Body::ServiceOffer(o) = &offer.unsigned().payload else {
            return false;
        };
        let env = self.actor.env();
        offer.unsigned().environment == env
            && self.policy.allowed_environments.contains(env)
            && self
                .policy
                .trusted_publishers
                .contains(&offer.unsigned().issuer)
            && self.policy.allowed_services.contains(&o.service_id)
            && o.environments.iter().any(|e| e == env)
            && o.verification_profiles
                .contains(&o.output_profile.schema_profile)
            && timestamp(&o.expires_at).is_ok_and(|expiry| now < expiry)
    }
    pub fn discover(&self) -> Result<Vec<SignedMessage>> {
        let now = self.actor.backend.clock()?;
        Ok(self
            .actor
            .transport
            .offers()?
            .into_iter()
            .filter(|offer| self.permits_offer(offer, now))
            .collect())
    }
    pub fn create_request(
        &self,
        offer: &SignedMessage,
        terms: RequestTerms,
    ) -> Result<SignedMessage> {
        self.create_request_with_nonce(offer, terms, random_seed()?)
    }
    /// Supplies a nonce for a reproducible request or to reconstruct an earlier request.
    pub fn create_request_with_nonce(
        &self,
        offer: &SignedMessage,
        terms: RequestTerms,
        nonce: [u8; 32],
    ) -> Result<SignedMessage> {
        let now = self.actor.backend.clock()?;
        policy(self.permits_offer(offer, now))?;
        let Body::ServiceOffer(o) = &offer.unsigned().payload else {
            return Err(Error::Linkage);
        };
        let buyer = self.actor.identity.public_hex();
        let nonce = hex_string(&nonce);
        let r = Request {
            order_id: hex_string(&order_id(self.actor.env(), &buyer, &nonce)?),
            nonce,
            service_id: o.service_id.clone(),
            offer_id: hex_string(offer.id()),
            buyer,
            seller: offer.unsigned().issuer.clone(),
            artifact_constraints: Constraints {
                verification_profile: o.output_profile.schema_profile.clone(),
                max_bytes: terms.max_bytes.to_string(),
            },
            allowed_verifiers: terms.allowed_verifiers,
            asset: terms.asset,
            max_principal: terms.max_principal.to_string(),
            expires_at: terms.expires_at.to_string(),
        };
        policy(terms.expires_at > now && terms.max_bytes <= decimal(&o.input_profile.max_bytes)?)?;
        self.request_policy(&r)?;
        self.actor.publish(self.actor.unsigned(Body::Request(r)))
    }
    fn request_policy(&self, r: &Request) -> Result<()> {
        policy(
            self.policy.allowed_environments.contains(self.actor.env())
                && self.policy.trusted_publishers.contains(&r.seller)
                && self.policy.allowed_services.contains(&r.service_id)
                && self.policy.allowed_assets.contains(&r.asset.id)
                && decimal(&r.max_principal)? <= self.policy.max_principal
                && r.buyer == self.actor.identity.public_hex()
                && r.allowed_verifiers
                    .iter()
                    .all(|v| self.policy.allowed_verifiers.contains(v)),
        )
    }
    fn quote_policy(&self, request: &SignedMessage, quote: &SignedMessage) -> Result<()> {
        let (Body::Request(r), Body::Quote(q)) =
            (&request.unsigned().payload, &quote.unsigned().payload)
        else {
            return Err(Error::Linkage);
        };
        let offer = self.actor.transport.fetch(&r.offer_id)?;
        transport::check_offer(&offer, request)?;
        self.request_policy(r)?;
        check_quote_request(quote, request).map_err(|_| Error::PolicyRejected)?;
        if self.actor.env().starts_with("solana:") {
            let ledger = self
                .actor
                .coordinator()?
                .ledger
                .as_deref()
                .ok_or(Error::Unavailable)?;
            policy(q.settlement == ledger.settlement(r)?)?;
        }
        let fund = timestamp(&q.deadlines.fund_by)?;
        let deliver = timestamp(&q.deadlines.deliver_by)?;
        let resolve = timestamp(&q.deadlines.resolve_by)?;
        policy(
            quote.unsigned().environment == self.actor.env()
                && self.actor.backend.clock()? < fund
                && deliver.abs_diff(fund) <= self.policy.max_deliver_window_seconds
                && resolve.abs_diff(deliver) <= self.policy.max_resolve_window_seconds,
        )
    }
    pub fn accept(&self, request: &SignedMessage, quote: &SignedMessage) -> Result<SignedMessage> {
        self.quote_policy(request, quote)?;
        let Body::Quote(q) = &quote.unsigned().payload else {
            return Err(Error::Linkage);
        };
        let a = Accept {
            request_id: q.request_id.clone(),
            quote_id: hex_string(quote.id()),
            order_id: q.order_id.clone(),
            amount: q.amount.clone(),
            asset: q.asset.clone(),
            environment: self.actor.env().into(),
        };
        self.actor.publish(self.actor.unsigned(Body::Accept(a)))
    }
    pub fn sign_fund(&self, fund: &PreparedFund) -> Result<[u8; 64]> {
        self.actor.check_fund(fund)?;
        self.quote_policy(&fund.agreement.request, &fund.agreement.quote)?;
        self.actor.sign_review(fund)
    }
    pub fn inspect(&self, order: &str) -> Result<Option<OrderView>> {
        Ok(self.actor.backend.inspect(order)?)
    }
}

pub struct SellerClient {
    actor: Actor,
    policy: SellerPolicy,
}
impl SellerClient {
    pub fn new(
        identity: Identity,
        policy: SellerPolicy,
        transport: Arc<dyn MessageTransport>,
        backend: Arc<dyn SettlementBackend>,
    ) -> Self {
        Self {
            actor: Actor {
                identity,
                transport,
                backend,
                http: None,
            },
            policy,
        }
    }
    pub fn publish_offer(&self, offer: Offer) -> Result<SignedMessage> {
        policy(
            self.policy.allowed_environments.contains(self.actor.env())
                && offer.environments.iter().any(|e| e == self.actor.env())
                && timestamp(&offer.expires_at)? > self.actor.backend.clock()?,
        )?;
        self.actor
            .publish(self.actor.unsigned(Body::ServiceOffer(offer)))
    }
    fn terms_policy(&self, q: &Quote) -> Result<()> {
        policy(
            self.policy.allowed_environments.contains(self.actor.env())
                && q.seller == self.actor.identity.public_hex()
                && self.policy.allowed_verifiers.contains(&q.verifier)
                && decimal(&q.amount)? >= self.policy.min_principal
                && self.actor.backend.clock()? < timestamp(&q.deadlines.fund_by)?,
        )
    }
    pub fn quote(&self, request: &SignedMessage, terms: QuoteTerms) -> Result<SignedMessage> {
        let Body::Request(r) = &request.unsigned().payload else {
            return Err(Error::Linkage);
        };
        policy(
            request.unsigned().environment == self.actor.env()
                && r.seller == self.actor.identity.public_hex(),
        )?;
        let offer = self.actor.transport.fetch(&r.offer_id)?;
        transport::check_offer(&offer, request)?;
        let q = Quote {
            request_id: hex_string(request.id()),
            order_id: r.order_id.clone(),
            offer_id: r.offer_id.clone(),
            buyer: r.buyer.clone(),
            seller: r.seller.clone(),
            verifier: terms.verifier,
            receipt_issuer: terms.receipt_issuer,
            service_id: r.service_id.clone(),
            policy_hash: hex_string(
                &policy_hash(&terms.policy).map_err(|_| Error::PolicyRejected)?,
            ),
            artifact: Artifact {
                digest: terms.policy.expected_digest.clone(),
                length: terms.policy.expected_length.clone(),
                verification_profile: terms.policy.profile.clone(),
                max_bytes: r.artifact_constraints.max_bytes.clone(),
            },
            acceptance_policy: terms.policy,
            amount: terms.amount.to_string(),
            asset: r.asset.clone(),
            settlement: if self.actor.env().starts_with("sim:") {
                Settlement::Sim {
                    asset_id: r.asset.id.clone(),
                    buyer_destination: format!("sim:{}", r.buyer),
                    seller_destination: format!("sim:{}", r.seller),
                }
            } else {
                self.actor
                    .coordinator()?
                    .ledger
                    .as_deref()
                    .ok_or(Error::Unavailable)?
                    .settlement(r)?
            },
            deadlines: terms.deadlines,
            expires_at: terms.expires_at.to_string(),
            fees: terms.fees,
            artifact_store: terms.artifact_store,
        };
        check_quote_terms(&q, self.actor.env(), request).map_err(|_| Error::PolicyRejected)?;
        self.terms_policy(&q)?;
        self.actor
            .publish(self.actor.unsigned(Body::Quote(Box::new(q))))
    }
    pub fn sign_fund(&self, fund: &PreparedFund) -> Result<[u8; 64]> {
        self.actor.check_fund(fund)?;
        let (_, quote) = fund.agreement.terms(self.actor.env())?;
        self.terms_policy(quote)?;
        self.actor.sign_review(fund)
    }
}
