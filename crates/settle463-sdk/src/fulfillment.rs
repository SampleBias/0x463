use crate::http::text;
use crate::{
    Actor, BuyerClient, BuyerPolicy, Error, HttpCoordinator, Result, Role, SellerClient,
    SellerPolicy,
};
use ed25519_dalek::Signer;
use settle463_core::{
    artifact::MAX_BYTES,
    ident::{decimal, hex, hex_string, timestamp},
    message_id, parse_signed,
    settlement::{Action, Agreement, Review, Submission},
    sign,
    wire::{Body, Delivery, Verdict},
    SignedMessage,
};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct StoredArtifact {
    pub content_id: String,
    pub digest: String,
    pub length: String,
}

impl Actor {
    fn connected(http: Arc<HttpCoordinator>, role: Role) -> Result<Self> {
        if http.role != role {
            return Err(Error::PolicyRejected);
        }
        Ok(Self {
            identity: http.identity.clone(),
            transport: http.clone(),
            backend: http.clone(),
            http: Some(http),
        })
    }
    fn coordinator(&self) -> Result<&HttpCoordinator> {
        self.http.as_deref().ok_or(Error::Unavailable)
    }
    fn authorize_action(
        &self,
        a: &Agreement,
        action: Action,
        m: Option<&SignedMessage>,
    ) -> Result<Submission> {
        let expected = Review::sim_fund(a, self.env())?;
        let review = self.backend.prepare_action(&expected.order_id, action, m)?;
        if review.action != action || review.terms != expected {
            return Err(Error::Linkage);
        }
        let id = m.map(|v| hex_string(v.id()));
        let key = self.identity.public_hex();
        let valid = match (action, m.map(|msg| &msg.unsigned().payload)) {
            (Action::CommitDelivery, Some(Body::Delivery(_))) => {
                key == expected.seller && review.delivery_id == id && review.verdict_id.is_none()
            }
            (Action::Release, Some(Body::Verdict(v))) => {
                key == expected.verifier
                    && v.decision == "PASS"
                    && review.verdict_id == id
                    && review.delivery_id.as_ref() == Some(&v.delivery_id)
            }
            (Action::Refund, None) => key == expected.buyer && review.verdict_id.is_none(),
            _ => false,
        };
        if !valid {
            return Err(Error::PolicyRejected);
        }
        let sig = if action == Action::Refund {
            None
        } else {
            Some(self.identity.0.sign(&review.signing_bytes()?).to_bytes())
        };
        Ok(self.backend.submit_action(&review, m, sig.as_ref())?)
    }
}

impl BuyerClient {
    pub fn connect(http: Arc<HttpCoordinator>, policy: BuyerPolicy) -> Result<Self> {
        Ok(Self {
            actor: Actor::connected(http, Role::Buyer)?,
            policy,
        })
    }
    pub fn read_artifact(&self, order: &str, content: &str) -> Result<Vec<u8>> {
        self.actor.coordinator()?.read_artifact(order, content)
    }
    pub fn export_receipt(&self, order: &str) -> Result<Vec<u8>> {
        self.actor.coordinator()?.receipt(order)
    }
    pub fn refund_if_eligible(&self, agreement: &Agreement) -> Result<Submission> {
        self.actor.authorize_action(agreement, Action::Refund, None)
    }
}
impl SellerClient {
    pub fn connect(http: Arc<HttpCoordinator>, policy: SellerPolicy) -> Result<Self> {
        Ok(Self {
            actor: Actor::connected(http, Role::Seller)?,
            policy,
        })
    }
    pub fn upload_artifact(&self, order: &str, bytes: &[u8]) -> Result<StoredArtifact> {
        let result = self.actor.coordinator()?.upload(order, bytes)?;
        let content_id = text(&result, "content_id")?.to_owned();
        hex::<32>(&content_id)?;
        let digest = hex_string(&message_id(bytes));
        let length = bytes.len().to_string();
        if result["digest"] != digest || result["length"] != length {
            return Err(Error::Linkage);
        }
        Ok(StoredArtifact {
            content_id,
            digest,
            length,
        })
    }
    pub fn commit_delivery(
        &self,
        a: &Agreement,
        artifact: &StoredArtifact,
    ) -> Result<SignedMessage> {
        let terms = Review::sim_fund(a, self.actor.env())?;
        if self.actor.identity.public_hex() != terms.seller {
            return Err(Error::PolicyRejected);
        }
        let draft = Delivery {
            order_id: terms.order_id,
            quote_id: terms.quote_id,
            policy_hash: terms.policy_hash,
            digest: artifact.digest.clone(),
            length: artifact.length.clone(),
            content_id: artifact.content_id.clone(),
        };
        let msg = self
            .actor
            .publish(self.actor.unsigned(Body::Delivery(draft)))?;
        self.actor
            .authorize_action(a, Action::CommitDelivery, Some(&msg))?;
        Ok(msg)
    }
}
pub struct VerifierClient {
    actor: Actor,
    judge_lock: Mutex<()>,
}
impl VerifierClient {
    pub fn connect(http: Arc<HttpCoordinator>) -> Result<Self> {
        Ok(Self {
            actor: Actor::connected(http, Role::Verifier)?,
            judge_lock: Mutex::new(()),
        })
    }
    pub fn fetch_artifact(&self, order: &str, content: &str) -> Result<Vec<u8>> {
        self.actor.coordinator()?.read_artifact(order, content)
    }
    fn delivery<'a>(&self, terms: &Review, msg: &'a SignedMessage) -> Result<&'a Delivery> {
        let Body::Delivery(d) = &msg.unsigned().payload else {
            return Err(Error::Linkage);
        };
        if self.actor.identity.public_hex() != terms.verifier
            || msg.unsigned().environment != terms.environment
            || msg.unsigned().issuer != terms.seller
            || d.order_id != terms.order_id
            || d.quote_id != terms.quote_id
            || d.policy_hash != terms.policy_hash
        {
            return Err(Error::Linkage);
        }
        Ok(d)
    }
    fn check_verdict<'a>(
        &self,
        terms: &Review,
        delivery_id: &str,
        msg: &'a SignedMessage,
    ) -> Result<&'a Verdict> {
        let Body::Verdict(v) = &msg.unsigned().payload else {
            return Err(Error::Linkage);
        };
        if msg.unsigned().environment != terms.environment
            || msg.unsigned().issuer != terms.verifier
            || v.order_id != terms.order_id
            || v.quote_id != terms.quote_id
            || v.policy_hash != terms.policy_hash
            || v.delivery_id != delivery_id
        {
            return Err(Error::Linkage);
        }
        Ok(v)
    }
    /// Reuses the coordinator's first definitive signed decision. An unavailable artifact can be retried.
    pub fn judge(&self, a: &Agreement, msg: &SignedMessage) -> Result<SignedMessage> {
        let _guard = self.judge_lock.lock().map_err(|_| Error::Unavailable)?;
        let terms = Review::sim_fund(a, self.actor.env())?;
        let (_, q) = a.terms(self.actor.env())?;
        let d = self.delivery(&terms, msg)?;
        let delivery_id = hex_string(msg.id());
        let order = self.actor.coordinator()?.order(&terms.order_id)?;
        if order["delivery_id"] != delivery_id {
            return Err(Error::Linkage);
        }
        if !order["definitive_verdict"].is_null() {
            let existing = parse_signed(
                &serde_json::to_vec(&order["definitive_verdict"])?,
                self.actor.env(),
            )?;
            let v = self.check_verdict(&terms, &delivery_id, &existing)?;
            if !matches!(v.decision.as_str(), "PASS" | "FAIL") {
                return Err(Error::Linkage);
            }
            return Ok(existing);
        }
        let resolve = timestamp(&terms.deadlines.resolve_by)?;
        let now = self.actor.backend.clock()?;
        let declared = decimal(&d.length)?;
        let draft = if declared > MAX_BYTES as u64 {
            settle463_verify::declared_oversized(now, resolve)
        } else {
            let bytes = if now >= resolve {
                None
            } else {
                match self.fetch_artifact(&terms.order_id, &d.content_id) {
                    Ok(bytes) => Some(bytes),
                    Err(
                        Error::Unavailable
                        | Error::Http {
                            status: 503 | 404, ..
                        },
                    ) => None,
                    Err(e) => return Err(e),
                }
            };
            if let Some(b) = &bytes {
                if hex_string(&message_id(b)) != d.digest || declared != b.len() as u64 {
                    return Err(Error::Linkage);
                }
            }
            settle463_verify::judge(
                bytes.as_deref(),
                &q.acceptance_policy,
                self.actor.backend.clock()?,
                resolve,
            )
        };
        let v = Verdict {
            order_id: terms.order_id,
            quote_id: terms.quote_id,
            delivery_id,
            policy_hash: terms.policy_hash,
            algorithm: q.acceptance_policy.profile.clone(),
            algorithm_version: "1".into(),
            decision: draft.decision,
            reason_codes: draft.reason_codes,
            observed_digest: draft.observed_digest,
            observed_length: draft.observed_length,
        };
        let signed = sign(
            self.actor.unsigned(Body::Verdict(v)),
            &self.actor.identity.0,
        )?;
        self.actor.coordinator()?.post_signed(&signed, false)
    }
    pub fn release_if_pass(
        &self,
        a: &Agreement,
        delivery: &SignedMessage,
        verdict: &SignedMessage,
    ) -> Result<Submission> {
        let terms = Review::sim_fund(a, self.actor.env())?;
        let (_, q) = a.terms(self.actor.env())?;
        let d = self.delivery(&terms, delivery)?;
        let v = self.check_verdict(&terms, &hex_string(delivery.id()), verdict)?;
        if v.decision != "PASS"
            || v.observed_digest.as_deref() != Some(&q.acceptance_policy.expected_digest)
            || v.observed_length.as_deref() != Some(&q.acceptance_policy.expected_length)
            || d.digest != q.acceptance_policy.expected_digest
            || d.length != q.acceptance_policy.expected_length
            || self.actor.backend.clock()? >= timestamp(&terms.deadlines.resolve_by)?
        {
            return Err(Error::PolicyRejected);
        }
        let existing = self.judge(a, delivery)?;
        if existing.id() != verdict.id() {
            return Err(Error::Linkage);
        }
        self.actor
            .authorize_action(a, Action::Release, Some(verdict))
    }
}
