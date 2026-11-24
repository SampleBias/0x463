//! Closed v0.1 body schemas.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub decimals: String,
    pub id: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub max_bytes: String,
    pub max_field_name_bytes: String,
    pub max_fields: String,
    pub max_string_bytes: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub expected_digest: String,
    pub expected_length: String,
    pub limits: Limits,
    pub profile: String,
    pub schema: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputProfile {
    pub content_type: String,
    pub max_bytes: String,
    pub schema_profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputProfile {
    pub content_type: String,
    pub schema_profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Offer {
    pub service_id: String,
    pub descriptor_version: String,
    pub input_profile: InputProfile,
    pub output_profile: OutputProfile,
    pub verification_profiles: Vec<String>,
    pub environments: Vec<String>,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Constraints {
    pub verification_profile: String,
    pub max_bytes: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub order_id: String,
    pub nonce: String,
    pub service_id: String,
    pub offer_id: String,
    pub buyer: String,
    pub seller: String,
    pub artifact_constraints: Constraints,
    pub allowed_verifiers: Vec<String>,
    pub asset: Asset,
    pub max_principal: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub digest: String,
    pub length: String,
    pub verification_profile: String,
    pub max_bytes: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deadlines {
    pub fund_by: String,
    pub deliver_by: String,
    pub resolve_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fees {
    pub protocol_atomic: String,
    pub network_fee_payer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactStore {
    pub store_id: String,
    pub read_access: String,
    pub retention_seconds: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quote {
    pub request_id: String,
    pub order_id: String,
    pub offer_id: String,
    pub buyer: String,
    pub seller: String,
    pub verifier: String,
    pub receipt_issuer: String,
    pub service_id: String,
    pub artifact: Artifact,
    pub acceptance_policy: Policy,
    pub policy_hash: String,
    pub amount: String,
    pub asset: Asset,
    pub settlement: Settlement,
    pub deadlines: Deadlines,
    pub expires_at: String,
    pub fees: Fees,
    pub artifact_store: ArtifactStore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Accept {
    pub request_id: String,
    pub quote_id: String,
    pub order_id: String,
    pub amount: String,
    pub asset: Asset,
    pub environment: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delivery {
    pub order_id: String,
    pub quote_id: String,
    pub policy_hash: String,
    pub digest: String,
    pub length: String,
    pub content_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verdict {
    pub order_id: String,
    pub quote_id: String,
    pub delivery_id: String,
    pub policy_hash: String,
    pub algorithm: String,
    pub algorithm_version: String,
    pub decision: String,
    pub reason_codes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_length: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub order_id: String,
    pub outcome: String,
    pub principal: String,
    pub asset: Asset,
    pub buyer: String,
    pub seller: String,
    pub verifier: String,
    pub receipt_issuer: String,
    pub request_id: String,
    pub quote_id: String,
    pub acceptance_id: String,
    pub environment: String,
    pub destination: String,
    pub ledger_kind: String,
    pub terminal_locator: String,
    pub finality: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub order_id: String,
    pub event_seq: String,
    pub event_digest: String,
    pub event_type: String,
    pub refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "profile", deny_unknown_fields)]
pub enum Settlement {
    #[serde(rename = "sim-ledger-v1")]
    Sim {
        asset_id: String,
        buyer_destination: String,
        seller_destination: String,
    },
    #[serde(rename = "solana-spl-token-v1")]
    Solana {
        mint: String,
        token_program: String,
        buyer_wallet: String,
        seller_wallet: String,
        buyer_token_account: String,
        seller_token_account: String,
        escrow_pda: String,
        vault_token_account: String,
        config: String,
        program_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "body",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Body {
    ServiceOffer(Offer),
    Request(Request),
    Quote(Box<Quote>),
    Accept(Accept),
    Delivery(Delivery),
    Verdict(Verdict),
    Receipt(Box<Receipt>),
    Observation(Observation),
}
