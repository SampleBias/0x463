use crate::{Error, Result};
use settle463_core::{
    check_quote_request,
    ident::{decimal, environment, hex_string},
    parse_signed,
    settlement::Agreement,
    wire::Body,
    SignedMessage,
};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

pub trait MessageTransport: Send + Sync {
    fn publish(&self, message: SignedMessage) -> Result<()>;
    fn fetch(&self, message_id: &str) -> Result<SignedMessage>;
    fn offers(&self) -> Result<Vec<SignedMessage>>;
    fn accepted(&self, request: &SignedMessage) -> Result<Option<SignedMessage>>;
}

#[derive(Default)]
struct Store {
    messages: HashMap<String, SignedMessage>,
    // Uniqueness spans different request IDs produced with the same buyer nonce.
    acceptances: HashMap<(String, String, String), String>,
}

fn acceptance_key(request: &SignedMessage) -> Result<(String, String, String)> {
    let Body::Request(r) = &request.unsigned().payload else {
        return Err(Error::Linkage);
    };
    Ok((
        request.unsigned().environment.clone(),
        r.buyer.clone(),
        r.nonce.clone(),
    ))
}
pub(crate) fn check_offer(offer: &SignedMessage, request: &SignedMessage) -> Result<()> {
    let (Body::ServiceOffer(o), Body::Request(r)) =
        (&offer.unsigned().payload, &request.unsigned().payload)
    else {
        return Err(Error::Linkage);
    };
    let env = &request.unsigned().environment;
    if offer.unsigned().environment != *env
        || !o.environments.contains(env)
        || offer.unsigned().issuer != r.seller
        || hex_string(offer.id()) != r.offer_id
        || o.service_id != r.service_id
        || !o
            .verification_profiles
            .contains(&r.artifact_constraints.verification_profile)
        || decimal(&r.artifact_constraints.max_bytes)? > decimal(&o.input_profile.max_bytes)?
    {
        return Err(Error::Linkage);
    }
    Ok(())
}

pub struct MemoryTransport {
    environment: String,
    store: Mutex<Store>,
}
impl MemoryTransport {
    pub fn new(env: &str) -> Result<Self> {
        environment(env)?;
        Ok(Self {
            environment: env.to_owned(),
            store: Mutex::default(),
        })
    }
    fn lock(&self) -> Result<MutexGuard<'_, Store>> {
        self.store.lock().map_err(|_| Error::Unavailable)
    }
    pub fn publish_wire(&self, bytes: &[u8]) -> Result<()> {
        self.publish(parse_signed(bytes, &self.environment)?)
    }
}
impl Store {
    fn get(&self, id: &str) -> Result<&SignedMessage> {
        self.messages.get(id).ok_or(Error::NotFound)
    }
}
impl MessageTransport for MemoryTransport {
    fn publish(&self, msg: SignedMessage) -> Result<()> {
        if msg.unsigned().environment != self.environment {
            return Err(Error::Environment);
        }
        let id = hex_string(msg.id());
        let mut store = self.lock()?;
        if store.messages.contains_key(&id) {
            return Ok(());
        }
        match &msg.unsigned().payload {
            Body::ServiceOffer(_) => (),
            Body::Request(r) => check_offer(store.get(&r.offer_id)?, &msg)?,
            Body::Quote(q) => check_quote_request(&msg, store.get(&q.request_id)?)?,
            Body::Accept(a) => {
                let request = store.get(&a.request_id)?.clone();
                let agreement = Agreement {
                    request: request.clone(),
                    quote: store.get(&a.quote_id)?.clone(),
                    acceptance: msg.clone(),
                };
                agreement.terms(&self.environment)?;
                let key = acceptance_key(&request)?;
                if store.acceptances.contains_key(&key) {
                    return Err(Error::AcceptanceExists);
                }
                store.acceptances.insert(key, id.clone());
            }
            _ => return Err(Error::Linkage),
        }
        store.messages.insert(id, msg);
        Ok(())
    }
    fn fetch(&self, id: &str) -> Result<SignedMessage> {
        self.lock()?.get(id).cloned()
    }
    fn offers(&self) -> Result<Vec<SignedMessage>> {
        Ok(self
            .lock()?
            .messages
            .values()
            .filter(|m| matches!(m.unsigned().payload, Body::ServiceOffer(_)))
            .cloned()
            .collect())
    }
    fn accepted(&self, request: &SignedMessage) -> Result<Option<SignedMessage>> {
        let key = acceptance_key(request)?;
        let store = self.lock()?;
        store
            .acceptances
            .get(&key)
            .map(|id| store.get(id).cloned())
            .transpose()
    }
}
