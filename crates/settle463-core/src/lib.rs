//! Signed wire messages and identifiers for version 0.1 of the 0x463 protocol.

pub mod canonical;
pub mod ident;
pub mod settlement;
mod validate;
pub mod wire;
pub use validate::{check_quote_request, check_quote_terms};

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use wire::{Body, Policy};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("invalid {0}")]
    Invalid(&'static str),
    #[error("invalid Ed25519 signature or issuer")]
    Signature,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnsignedMessage {
    pub protocol: String,
    pub version: String,
    pub environment: String,
    pub issuer: String,
    #[serde(flatten)]
    pub payload: Body,
}

#[derive(Debug, Clone)]
pub struct SignedMessage {
    unsigned: UnsignedMessage,
    signature: [u8; 64],
    id: [u8; 32],
}

impl SignedMessage {
    pub fn unsigned(&self) -> &UnsignedMessage {
        &self.unsigned
    }
    pub fn signature(&self) -> &[u8; 64] {
        &self.signature
    }
    pub fn id(&self) -> &[u8; 32] {
        &self.id
    }

    pub fn to_wire(&self) -> Result<Vec<u8>> {
        let mut value = serde_json::to_value(&self.unsigned)?;
        value["signature"] = ident::hex_string(&self.signature).into();
        canonical::canonical(&value)
    }
}

fn domain_preimage(domain: &[u8], value: &Value) -> Result<Vec<u8>> {
    let mut data = domain.to_vec();
    data.push(0);
    data.extend(canonical::canonical(value)?);
    Ok(data)
}

pub fn signing_preimage(message: &UnsignedMessage) -> Result<Vec<u8>> {
    domain_preimage(b"0x463/signed/0.1", &serde_json::to_value(message)?)
}

pub fn message_id(preimage: &[u8]) -> [u8; 32] {
    Sha256::digest(preimage).into()
}

pub fn order_id(environment: &str, buyer: &str, nonce: &str) -> Result<[u8; 32]> {
    ident::environment(environment)?;
    ident::hex::<32>(buyer)?;
    ident::hex::<32>(nonce)?;
    Ok(message_id(&domain_preimage(
        b"0x463/order/0.1",
        &json!({ "environment": environment, "buyer": buyer, "nonce": nonce }),
    )?))
}

pub fn policy_hash(policy: &Policy) -> Result<[u8; 32]> {
    validate::policy(policy)?;
    Ok(message_id(&domain_preimage(
        b"0x463/policy/0.1",
        &serde_json::to_value(policy)?,
    )?))
}

pub fn sign(unsigned: UnsignedMessage, key: &SigningKey) -> Result<SignedMessage> {
    unsigned.validate(&unsigned.environment)?;
    if key.verifying_key().as_bytes() != &ident::hex::<32>(&unsigned.issuer)? {
        return Err(Error::Signature);
    }
    let preimage = signing_preimage(&unsigned)?;
    Ok(SignedMessage {
        signature: key.sign(&preimage).to_bytes(),
        id: message_id(&preimage),
        unsigned,
    })
}

/// Verifies one envelope. Previously accepted messages supply role and linkage context separately.
pub fn parse_signed(bytes: &[u8], expected_environment: &str) -> Result<SignedMessage> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Envelope {
        protocol: String,
        version: String,
        environment: String,
        issuer: String,
        kind: String,
        body: Value,
        signature: String,
    }
    let mut original = canonical::parse(bytes)?;
    let env: Envelope = serde_json::from_value(original.clone())?;
    original
        .as_object_mut()
        .ok_or(Error::Invalid("envelope object"))?
        .remove("signature");
    let signature = ident::hex::<64>(&env.signature)?;
    let unsigned = UnsignedMessage {
        protocol: env.protocol,
        version: env.version,
        environment: env.environment,
        issuer: env.issuer,
        payload: serde_json::from_value(json!({"kind":env.kind,"body":env.body}))?,
    };
    // Serde also accepts positional arrays for struct fields. The signed wire format requires objects.
    if serde_json::to_value(&unsigned)? != original {
        return Err(Error::Invalid("wire object shape"));
    }
    unsigned.validate(expected_environment)?;
    let key = VerifyingKey::from_bytes(&ident::hex::<32>(&unsigned.issuer)?)
        .map_err(|_| Error::Signature)?;
    let preimage = signing_preimage(&unsigned)?;
    key.verify_strict(&preimage, &Signature::from_bytes(&signature))
        .map_err(|_| Error::Signature)?;
    Ok(SignedMessage {
        id: message_id(&preimage),
        unsigned,
        signature,
    })
}
