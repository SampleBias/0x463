//! Blocking loopback HTTP transport and role-bound simulator adapter.

use crate::{Error, Identity, MessageTransport, Result};
use ed25519_dalek::Signer;
use serde_json::{json, value::RawValue, Value};
use settle463_core::{
    canonical::canonical,
    ident::{decimal, environment, hex, hex_string, timestamp},
    message_id, parse_signed,
    settlement::{
        Action, ActionReview, Agreement, ChainOutcome, FundSignatures, FundingError, OrderState,
        OrderView, PreparedFund, Review, SettlementBackend, Submission,
    },
    wire::Body,
    SignedMessage,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::Read,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Role {
    Buyer,
    Seller,
    Verifier,
    ReceiptIssuer,
}
impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Buyer => "buyer",
            Self::Seller => "seller",
            Self::Verifier => "verifier",
            Self::ReceiptIssuer => "receipt_issuer",
        }
    }
}

pub struct HttpCoordinator {
    base: String,
    env: String,
    pub(crate) identity: Identity,
    pub(crate) role: Role,
    agent: ureq::Agent,
    intents: Mutex<HashMap<String, String>>,
}
pub(crate) fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key).and_then(Value::as_str).ok_or(Error::Linkage)
}
fn read_response(
    response: std::result::Result<ureq::Response, ureq::Error>,
    limit: usize,
) -> Result<Vec<u8>> {
    let resp = match response {
        Ok(resp) => resp,
        Err(ureq::Error::Status(status, resp)) => {
            let error: Value =
                serde_json::from_reader(resp.into_reader().take(8192)).unwrap_or(Value::Null);
            let code = error
                .pointer("/error/code")
                .and_then(Value::as_str)
                .unwrap_or("HTTP_ERROR");
            return Err(Error::Http {
                status,
                code: code.into(),
            });
        }
        Err(ureq::Error::Transport(_)) => return Err(Error::Unavailable),
    };
    if resp
        .header("Content-Length")
        .and_then(|h| h.parse::<u64>().ok())
        .is_some_and(|n| n > limit as u64)
    {
        return Err(Error::Linkage);
    }
    let mut bytes = Vec::new();
    resp.into_reader()
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Unavailable)?;
    if bytes.len() > limit {
        return Err(Error::Linkage);
    }
    Ok(bytes)
}
const MAX_JSON: usize = 16 * 1024 * 1024;
type RawObject<'a> = BTreeMap<String, &'a RawValue>;

// An HTTP page or projection can contain null. Its embedded protocol envelopes must
// retain their original bytes until strict signature and duplicate-name checks finish.
fn offer_page(bytes: &[u8], env: &str) -> Result<(Vec<SignedMessage>, Option<String>)> {
    let page: RawObject<'_> = serde_json::from_slice(bytes)?;
    let raw_items: Vec<&RawValue> =
        serde_json::from_str(page.get("items").ok_or(Error::Linkage)?.get())?;
    let mut items = Vec::with_capacity(raw_items.len());
    for item in raw_items {
        let message = parse_signed(item.get().as_bytes(), env)?;
        if !matches!(message.unsigned().payload, Body::ServiceOffer(_)) {
            return Err(Error::Linkage);
        }
        items.push(message);
    }
    let cursor = serde_json::from_str(page.get("next_cursor").ok_or(Error::Linkage)?.get())?;
    Ok((items, cursor))
}

fn order_response(bytes: &[u8], env: &str) -> Result<Value> {
    let projection: RawObject<'_> = serde_json::from_slice(bytes)?;
    for name in ["request", "quote", "acceptance", "definitive_verdict"] {
        if let Some(raw) = projection.get(name).filter(|v| v.get() != "null") {
            parse_signed(raw.get().as_bytes(), env)?;
        }
    }
    Ok(serde_json::from_slice(bytes)?)
}

fn join_order(order: &str, suffix: &str) -> Result<String> {
    hex::<32>(order)?;
    Ok(format!("/v1/orders/{order}{suffix}"))
}

impl HttpCoordinator {
    /// HTTP is restricted to a numeric loopback address. Private requests consume one challenge.
    pub fn new(base: &str, env: &str, identity: Identity, role: Role) -> Result<Self> {
        environment(env)?;
        let addr: SocketAddr = base
            .trim_end_matches('/')
            .strip_prefix("http://")
            .ok_or(Error::Environment)?
            .parse()
            .map_err(|_| Error::Environment)?;
        if !addr.ip().is_loopback() || !env.starts_with("sim:") {
            return Err(Error::Environment);
        }
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout(Duration::from_secs(30))
            .build();
        Ok(Self {
            base: format!("http://{addr}"),
            env: env.to_owned(),
            identity,
            role,
            agent,
            intents: Mutex::default(),
        })
    }
    fn public_json(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
        let req = self.agent.request(method, &format!("{}{path}", self.base));
        let resp = match body {
            Some(v) => req
                .set("Content-Type", "application/json")
                .send_bytes(&serde_json::to_vec(v)?),
            None => req.call(),
        };
        Ok(serde_json::from_slice(&read_response(resp, MAX_JSON)?)?)
    }
    fn authorization(&self, method: &str, target: &str) -> Result<String> {
        let key = self.identity.public_hex();
        let role = self.role.as_str();
        let challenge = self.public_json(
            "POST",
            "/v1/auth/challenge",
            Some(&json!({"public_key":key,"role":role})),
        )?;
        let id = text(&challenge, "challenge_id")?;
        hex::<32>(id)?;
        let nonce = text(&challenge, "nonce")?;
        hex::<32>(nonce)?;
        let expiry = text(&challenge, "expires_at")?;
        timestamp(expiry)?;
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let query_hash = if query.is_empty() {
            "00".repeat(32)
        } else {
            hex_string(&message_id(query.as_bytes()))
        };
        let mut image = b"0x463/http/0.1\0".to_vec();
        image.extend(canonical(&json!({
            "challenge_id": id, "expires_at": expiry, "method": method,
            "nonce": nonce, "path": path, "public_key": key,
            "query_hash": query_hash, "role": role,
        }))?);
        let sig = self.identity.0.sign(&image).to_bytes();
        Ok(format!("0x463 {id}.{}", hex_string(&sig)))
    }
    fn private_get(&self, target: &str, limit: usize) -> Result<Vec<u8>> {
        let header = self.authorization("GET", target)?;
        read_response(
            self.agent
                .get(&format!("{}{target}", self.base))
                .set("Authorization", &header)
                .call(),
            limit,
        )
    }
    fn json_action(&self, order: &str, kind: &str, body: Value) -> Result<Value> {
        let path = join_order(order, &format!("/actions/{kind}"))?;
        let auth = self.authorization("POST", &path)?;
        let bytes = serde_json::to_vec(&body)?;
        let key = hex_string(&crate::random_seed()?);
        let req = self
            .agent
            .post(&format!("{}{path}", self.base))
            .set("Content-Type", "application/json")
            .set("Idempotency-Key", &key)
            .set("Authorization", &auth);
        Ok(serde_json::from_slice(&read_response(
            req.send_bytes(&bytes),
            MAX_JSON,
        )?)?)
    }
    pub fn order(&self, order: &str) -> Result<Value> {
        let bytes = self.private_get(&join_order(order, "")?, MAX_JSON)?;
        order_response(&bytes, &self.env)
    }
    pub fn receipt(&self, order: &str) -> Result<Vec<u8>> {
        self.private_get(&join_order(order, "/receipt")?, MAX_JSON)
    }
    pub fn read_artifact(&self, order: &str, content: &str) -> Result<Vec<u8>> {
        hex::<32>(content)?;
        self.private_get(
            &join_order(order, &format!("/artifacts/{content}"))?,
            1_048_576,
        )
    }
    pub(crate) fn upload(&self, order: &str, bytes: &[u8]) -> Result<Value> {
        let path = join_order(order, "/artifacts")?;
        let req = self
            .agent
            .post(&format!("{}{path}", self.base))
            .set("Content-Type", "application/octet-stream")
            .set("X-Artifact-Length", &bytes.len().to_string())
            .set("X-Artifact-Digest", &hex_string(&message_id(bytes)))
            .set("Idempotency-Key", &hex_string(&crate::random_seed()?))
            .set("Authorization", &self.authorization("POST", &path)?);
        Ok(serde_json::from_slice(&read_response(
            req.send_bytes(bytes),
            8192,
        )?)?)
    }
    pub fn set_sim_clock(&self, clock: i64, admin_token: &str) -> Result<()> {
        let req = self
            .agent
            .post(&format!("{}/v1/sim/clock", self.base))
            .set("Content-Type", "application/json")
            .set("X-Sim-Admin", admin_token)
            .set("Idempotency-Key", &hex_string(&crate::random_seed()?));
        read_response(
            req.send_bytes(&serde_json::to_vec(&json!({"clock":clock.to_string()}))?),
            8192,
        )?;
        Ok(())
    }
    pub fn fetch_for_order(&self, id: &str, order: &str) -> Result<SignedMessage> {
        hex::<32>(id)?;
        hex::<32>(order)?;
        let msg = parse_signed(
            &self.private_get(&format!("/v1/messages/{id}?order_id={order}"), MAX_JSON)?,
            &self.env,
        )?;
        let signed_order = match &msg.unsigned().payload {
            Body::Request(v) => &v.order_id,
            Body::Quote(v) => &v.order_id,
            Body::Accept(v) => &v.order_id,
            Body::Delivery(v) => &v.order_id,
            Body::Verdict(v) => &v.order_id,
            Body::Receipt(v) => &v.order_id,
            Body::Observation(v) => &v.order_id,
            Body::ServiceOffer(_) => return Err(Error::Linkage),
        };
        if signed_order != order || hex_string(msg.id()) != id {
            return Err(Error::Linkage);
        }
        Ok(msg)
    }
    pub(crate) fn post_signed(&self, msg: &SignedMessage, direct: bool) -> Result<SignedMessage> {
        if msg.unsigned().environment != self.env {
            return Err(Error::Environment);
        }
        let route = match &msg.unsigned().payload {
            Body::ServiceOffer(_) => "offers",
            Body::Request(_) => "requests",
            Body::Quote(_) => "quotes",
            Body::Accept(_) => "acceptances",
            Body::Delivery(_) => "deliveries",
            Body::Verdict(_) => "verdicts",
            _ if direct => "messages",
            _ => return Err(Error::Linkage),
        };
        let route = if direct { "messages" } else { route };
        let req = self
            .agent
            .post(&format!("{}/v1/{route}", self.base))
            .set("Content-Type", "application/json")
            .set("Idempotency-Key", &hex_string(&crate::random_seed()?));
        let bytes = match read_response(req.send_bytes(&msg.to_wire()?), MAX_JSON) {
            Err(Error::Http { status: 409, code }) if code == "ACCEPTANCE_EXISTS" => {
                return Err(Error::AcceptanceExists)
            }
            other => other?,
        };
        let returned = parse_signed(&bytes, &self.env)?;
        if returned.id() != msg.id() {
            match (&msg.unsigned().payload, &returned.unsigned().payload) {
                (Body::Verdict(v), Body::Verdict(w))
                    if msg.unsigned().issuer == returned.unsigned().issuer
                        && w.order_id == v.order_id
                        && w.delivery_id == v.delivery_id
                        && w.quote_id == v.quote_id
                        && w.policy_hash == v.policy_hash
                        && matches!(w.decision.as_str(), "PASS" | "FAIL") => {}
                _ => return Err(Error::Linkage),
            }
        }
        Ok(returned)
    }
    fn submit_command(&self, order: &str, intent_id: String, command: Value) -> Result<Submission> {
        // Retain the locally computed ID even if the server commits and the response is lost.
        self.intents
            .lock()
            .map_err(|_| Error::Unavailable)?
            .insert(intent_id.clone(), order.to_owned());
        let response = self.json_action(order, "submit", command)?;
        if text(&response, "intent_id")? != intent_id {
            return Err(Error::Linkage);
        }
        Ok(Submission {
            intent_id,
            outcome: outcome(&response)?,
        })
    }
    pub fn reconcile_for_order(&self, order: &str, intent_id: &str) -> Result<ChainOutcome> {
        hex::<32>(intent_id)?;
        let path = join_order(order, &format!("/intents/{intent_id}"))?;
        let response = serde_json::from_slice(&self.private_get(&path, 8192)?)?;
        if text(&response, "intent_id")? != intent_id {
            return Err(Error::Linkage);
        }
        outcome(&response)
    }
}
fn outcome(v: &Value) -> Result<ChainOutcome> {
    Ok(match text(v, "outcome")? {
        "finalized_success" => ChainOutcome::FinalizedSuccess,
        "finalized_failure" => ChainOutcome::FinalizedFailure,
        "pending" => ChainOutcome::Pending,
        "expired" => ChainOutcome::Expired,
        "unknown" => ChainOutcome::Unknown,
        _ => return Err(Error::Linkage),
    })
}
impl MessageTransport for HttpCoordinator {
    fn publish(&self, msg: SignedMessage) -> Result<()> {
        self.post_signed(&msg, false).map(|_| ())
    }
    fn fetch(&self, id: &str) -> Result<SignedMessage> {
        hex::<32>(id)?;
        let public = self
            .agent
            .get(&format!("{}/v1/offers/{id}", self.base))
            .call();
        let (raw, is_offer) = match read_response(public, MAX_JSON) {
            Ok(raw) => (raw, true),
            Err(Error::Http { status: 404, .. }) => (
                self.private_get(&format!("/v1/messages/{id}"), MAX_JSON)?,
                false,
            ),
            Err(e) => return Err(e),
        };
        let msg = parse_signed(&raw, &self.env)?;
        if (is_offer && !matches!(msg.unsigned().payload, Body::ServiceOffer(_)))
            || hex_string(msg.id()) != id
        {
            return Err(Error::Linkage);
        }
        Ok(msg)
    }
    fn offers(&self) -> Result<Vec<SignedMessage>> {
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen = HashSet::new();
        loop {
            let mut request = self
                .agent
                .get(&format!("{}/v1/offers", self.base))
                .query("environment", &self.env)
                .query("limit", "100");
            if let Some(c) = &cursor {
                request = request.query("cursor", c);
            }
            let (page, next) = offer_page(&read_response(request.call(), MAX_JSON)?, &self.env)?;
            items.extend(page);
            match next {
                None => break,
                Some(next) if seen.insert(next.clone()) => cursor = Some(next),
                _ => return Err(Error::Linkage),
            }
        }
        Ok(items)
    }
    fn accepted(&self, request: &SignedMessage) -> Result<Option<SignedMessage>> {
        let Body::Request(r) = &request.unsigned().payload else {
            return Err(Error::Linkage);
        };
        let v = match self.order(&r.order_id) {
            Err(Error::Http { status: 404, .. }) => return Ok(None),
            other => other?,
        };
        if v["acceptance"].is_null() {
            return Ok(None);
        }
        let a = parse_signed(&serde_json::to_vec(&v["acceptance"])?, &self.env)?;
        let Body::Accept(accepted) = &a.unsigned().payload else {
            return Err(Error::Linkage);
        };
        if accepted.request_id != hex_string(request.id()) || accepted.order_id != r.order_id {
            return Err(Error::Linkage);
        }
        Ok(Some(a))
    }
}

pub struct DirectPeer(pub Arc<HttpCoordinator>);
impl MessageTransport for DirectPeer {
    fn publish(&self, msg: SignedMessage) -> Result<()> {
        self.0.post_signed(&msg, true).map(|_| ())
    }
    fn fetch(&self, id: &str) -> Result<SignedMessage> {
        self.0.fetch(id)
    }
    fn offers(&self) -> Result<Vec<SignedMessage>> {
        self.0.offers()
    }
    fn accepted(&self, r: &SignedMessage) -> Result<Option<SignedMessage>> {
        self.0.accepted(r)
    }
}

fn funding(e: Error) -> FundingError {
    match e {
        Error::Funding(e) => e,
        Error::Environment => FundingError::Environment,
        Error::Linkage => FundingError::Linkage,
        Error::Wire(e) => FundingError::Wire(e),
        Error::Http { code, .. } => match code.as_str() {
            "ALREADY_COMPLETE" => FundingError::AlreadyFunded,
            "STATE_CONFLICT" => FundingError::WrongState,
            "ENVIRONMENT_MISMATCH" => FundingError::Environment,
            "NOT_FOUND" => FundingError::OrderMissing,
            "BAD_SIGNATURE" => FundingError::Wire(settle463_core::Error::Signature),
            "LEDGER_UNAVAILABLE" | "STORE_UNAVAILABLE" => FundingError::Unavailable,
            _ => FundingError::Linkage,
        },
        _ => FundingError::Unavailable,
    }
}
fn action_payload(action: Action, evidence: Option<&SignedMessage>) -> Value {
    let mut b = json!({"action":action});
    if let Some(m) = evidence {
        let k = if action == Action::CommitDelivery {
            "delivery_id"
        } else {
            "verdict_id"
        };
        b[k] = hex_string(m.id()).into();
    }
    b
}
impl SettlementBackend for HttpCoordinator {
    fn environment(&self) -> &str {
        &self.env
    }
    fn clock(&self) -> settle463_core::settlement::Result<i64> {
        let v = self
            .public_json("GET", "/v1/sim/clock", None)
            .map_err(funding)?;
        if v["environment"] != self.env {
            return Err(FundingError::Environment);
        }
        Ok(timestamp(text(&v, "clock").map_err(funding)?)?)
    }
    fn prepare_fund(
        &self,
        agreement: Agreement,
    ) -> settle463_core::settlement::Result<PreparedFund> {
        let expected = Review::sim_fund(&agreement, &self.env)?;
        let v = self
            .json_action(&expected.order_id, "prepare", json!({"action":"fund"}))
            .map_err(funding)?;
        if v != serde_json::to_value(&expected).map_err(settle463_core::Error::from)? {
            return Err(FundingError::Linkage);
        }
        Ok(PreparedFund {
            agreement,
            review: expected,
        })
    }
    fn submit_fund(
        &self,
        f: &PreparedFund,
        s: &FundSignatures,
    ) -> settle463_core::settlement::Result<Submission> {
        f.check(&self.env)?;
        let expected_intent = s.verify(&f.review)?;
        let order = &f.review.order_id;
        let command = json!({
            "action": "fund",
            "buyer_signature": hex_string(&s.buyer),
            "seller_signature": hex_string(&s.seller),
        });
        self.submit_command(order, expected_intent, command)
            .map_err(funding)
    }
    fn inspect(&self, order: &str) -> settle463_core::settlement::Result<Option<OrderView>> {
        let v = match self.order(order) {
            Err(Error::Http { status: 404, .. }) => return Ok(None),
            x => x.map_err(funding)?,
        };
        if v["review"].is_null() || v["state"] == "Accepted" {
            return Ok(None);
        }
        let state: OrderState =
            serde_json::from_value(v["state"].clone()).map_err(settle463_core::Error::from)?;
        Ok(Some(OrderView {
            state,
            review: serde_json::from_value(v["review"].clone())
                .map_err(settle463_core::Error::from)?,
            held: decimal(text(&v, "held").map_err(funding)?)?,
            paid: decimal(text(&v, "paid").map_err(funding)?)?,
            refunded: decimal(text(&v, "refunded").map_err(funding)?)?,
            delivery_id: v["delivery_id"].as_str().map(String::from),
            verdict_id: v["verdict_id"].as_str().map(String::from),
            terminal_locator: v["terminal_locator"].as_str().map(String::from),
            terminal_clock: v["terminal_clock"].as_str().map(timestamp).transpose()?,
        }))
    }
    fn reconcile(&self, id: &str) -> settle463_core::settlement::Result<ChainOutcome> {
        hex::<32>(id)?;
        let Some(order) = self
            .intents
            .lock()
            .map_err(|_| FundingError::Unavailable)?
            .get(id)
            .cloned()
        else {
            return Ok(ChainOutcome::Unknown);
        };
        self.reconcile_for_order(&order, id).map_err(funding)
    }
    fn prepare_action(
        &self,
        order: &str,
        action: Action,
        evidence: Option<&SignedMessage>,
    ) -> settle463_core::settlement::Result<ActionReview> {
        let view = self.inspect(order)?.ok_or(FundingError::Linkage)?;
        let expected = ActionReview::for_order(action, &view, evidence)?;
        let v = self
            .json_action(order, "prepare", action_payload(action, evidence))
            .map_err(funding)?;
        if v != serde_json::to_value(&expected).map_err(settle463_core::Error::from)? {
            return Err(FundingError::Linkage);
        }
        Ok(expected)
    }
    fn submit_action(
        &self,
        r: &ActionReview,
        evidence: Option<&SignedMessage>,
        sig: Option<&[u8; 64]>,
    ) -> settle463_core::settlement::Result<Submission> {
        let mut b = action_payload(r.action, evidence);
        if let Some(sig) = sig {
            b["signature"] = hex_string(sig).into();
        }
        self.submit_command(&r.terms.order_id, r.intent_id()?, b)
            .map_err(funding)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_envelopes_reject_duplicate_keys_before_map_deserialization() {
        let duplicate =
            |bytes: &str| bytes.replacen(r#""protocol":"#, r#""protocol":"other","protocol":"#, 1);
        let offer = include_str!("../../../spec/examples/worked-transaction/service_offer.json");
        let page = format!(r#"{{"items":[{offer}],"next_cursor":null}}"#);
        let (items, cursor) = offer_page(page.as_bytes(), "sim:fixture").unwrap();
        assert_eq!(items.len(), 1);
        assert!(cursor.is_none());
        assert!(offer_page(duplicate(&page).as_bytes(), "sim:fixture").is_err());

        let quote = include_str!("../../../spec/examples/worked-transaction/quote.json");
        let order = format!(r#"{{"quote":{quote},"verdict_id":null}}"#);
        assert!(order_response(order.as_bytes(), "sim:fixture").is_ok());
        assert!(order_response(duplicate(&order).as_bytes(), "sim:fixture").is_err());
    }
}
