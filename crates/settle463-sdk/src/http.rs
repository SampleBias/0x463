//! Blocking loopback HTTP transport and role-bound simulator adapter.

use crate::{Error, Identity, MessageTransport, Result};
use ed25519_dalek::Signer;
use serde_json::{json, Value};
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
    collections::{HashMap, HashSet},
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
    private_ids: Mutex<HashMap<String, String>>,
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
            private_ids: Mutex::default(),
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
        image.extend(canonical(&json!({"challenge_id":id,"expires_at":expiry,"method":method,"nonce":nonce,"path":path,"public_key":key,"query_hash":query_hash,"role":role}))?);
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
        let key = if kind == "submit" {
            hex_string(&message_id(&[path.as_bytes(), b"\0", &bytes].concat()))
        } else {
            hex_string(&crate::random_seed()?)
        };
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
        Ok(serde_json::from_slice(
            &self.private_get(&join_order(order, "")?, MAX_JSON)?,
        )?)
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
            .set(
                "Idempotency-Key",
                &hex_string(&message_id(&[path.as_bytes(), b"\0", bytes].concat())),
            )
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
        if hex_string(msg.id()) != id {
            return Err(Error::Linkage);
        }
        Ok(msg)
    }
    pub(crate) fn post_signed(&self, msg: &SignedMessage, direct: bool) -> Result<SignedMessage> {
        if msg.unsigned().environment != self.env {
            return Err(Error::Environment);
        }
        let (route, order) = match &msg.unsigned().payload {
            Body::ServiceOffer(_) => ("offers", None),
            Body::Request(r) => ("requests", Some(&r.order_id)),
            Body::Quote(q) => ("quotes", Some(&q.order_id)),
            Body::Accept(a) => ("acceptances", Some(&a.order_id)),
            Body::Delivery(d) => ("deliveries", Some(&d.order_id)),
            Body::Verdict(v) => ("verdicts", Some(&v.order_id)),
            _ if direct => ("messages", None),
            _ => return Err(Error::Linkage),
        };
        let route = if direct { "messages" } else { route };
        let req = self
            .agent
            .post(&format!("{}/v1/{route}", self.base))
            .set("Content-Type", "application/json")
            .set("Idempotency-Key", &hex_string(msg.id()));
        let returned = parse_signed(
            &read_response(req.send_bytes(&msg.to_wire()?), MAX_JSON)?,
            &self.env,
        )?;
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
        if let Some(order) = order {
            self.private_ids
                .lock()
                .map_err(|_| Error::Unavailable)?
                .insert(hex_string(returned.id()), order.clone());
        }
        Ok(returned)
    }
    fn submission(&self, order: &str, v: &Value) -> Result<Submission> {
        let id = text(v, "intent_id")?.to_owned();
        hex::<32>(&id)?;
        let outcome = outcome(v)?;
        self.intents
            .lock()
            .map_err(|_| Error::Unavailable)?
            .insert(id.clone(), order.to_owned());
        Ok(Submission {
            intent_id: id,
            outcome,
        })
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
        let order = self
            .private_ids
            .lock()
            .map_err(|_| Error::Unavailable)?
            .get(id)
            .cloned();
        if let Some(order) = order {
            return self.fetch_for_order(id, &order);
        }
        let v = self.public_json("GET", &format!("/v1/offers/{id}"), None)?;
        let msg = parse_signed(&serde_json::to_vec(&v)?, &self.env)?;
        if !matches!(msg.unsigned().payload, Body::ServiceOffer(_)) || hex_string(msg.id()) != id {
            return Err(Error::Linkage);
        }
        Ok(msg)
    }
    fn offers(&self) -> Result<Vec<SignedMessage>> {
        let mut items = Vec::new();
        let mut cursor = String::new();
        let mut seen = HashSet::new();
        loop {
            let page = self.public_json(
                "GET",
                &format!("/v1/offers?environment={}&limit=100{cursor}", self.env),
                None,
            )?;
            for item in page["items"].as_array().ok_or(Error::Linkage)? {
                let msg = parse_signed(&serde_json::to_vec(item)?, &self.env)?;
                if !matches!(msg.unsigned().payload, Body::ServiceOffer(_)) {
                    return Err(Error::Linkage);
                }
                items.push(msg);
            }
            match &page["next_cursor"] {
                Value::Null => break,
                Value::String(s) if decimal(s).is_ok() && seen.insert(s.clone()) => {
                    cursor = format!("&cursor={s}")
                }
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
        if !matches!(&a.unsigned().payload,Body::Accept(a) if a.request_id==hex_string(request.id()) && a.order_id==r.order_id)
        {
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
        Error::Http { code, .. } if code == "ALREADY_COMPLETE" => FundingError::AlreadyFunded,
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
        s.verify(&f.review)?;
        let order = &f.review.order_id;
        let v=self.json_action(order,"submit",json!({"action":"fund","buyer_signature":hex_string(&s.buyer),"seller_signature":hex_string(&s.seller)})).map_err(funding)?;
        self.submission(order, &v).map_err(funding)
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
        let path = format!("/v1/orders/{order}/intents/{id}");
        let raw = self.private_get(&path, 8192).map_err(funding)?;
        let v = serde_json::from_slice(&raw).map_err(settle463_core::Error::from)?;
        outcome(&v).map_err(funding)
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
        let v = self
            .json_action(&r.terms.order_id, "submit", b)
            .map_err(funding)?;
        self.submission(&r.terms.order_id, &v).map_err(funding)
    }
}
