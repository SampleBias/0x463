//! Deterministic verification of raw bytes against a signed acceptance policy.

use settle463_core::{
    artifact::{digest, object, FieldValue, MAX_BYTES},
    ident::decimal,
    policy_hash,
    wire::Policy,
};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerdictDraft {
    pub decision: String,
    pub reason_codes: Vec<String>,
    pub observed_digest: Option<String>,
    pub observed_length: Option<String>,
}

fn without_bytes(decision: &str, reason: &str) -> VerdictDraft {
    VerdictDraft {
        decision: decision.into(),
        reason_codes: vec![reason.into()],
        observed_digest: None,
        observed_length: None,
    }
}

/// A transport that refuses an oversized declared length can report it without buffering the body.
pub fn declared_oversized(ledger_time: i64, resolve_by: i64) -> VerdictDraft {
    if ledger_time >= resolve_by {
        return without_bytes("INDETERMINATE", "DEADLINE_EXPIRED");
    }
    without_bytes("FAIL", "OVERSIZED")
}

pub fn judge(
    bytes: Option<&[u8]>,
    policy: &Policy,
    ledger_time: i64,
    resolve_by: i64,
) -> VerdictDraft {
    if ledger_time >= resolve_by {
        return without_bytes("INDETERMINATE", "DEADLINE_EXPIRED");
    }
    let Some(bytes) = bytes else {
        return without_bytes("INDETERMINATE", "UNAVAILABLE");
    };
    let hash = digest(bytes);
    let length = bytes.len().to_string();
    let mut reasons = BTreeSet::new();
    if policy.expected_digest != hash {
        reasons.insert("DIGEST_MISMATCH");
    }
    if policy.expected_length != length {
        reasons.insert("SIZE_MISMATCH");
    }
    if policy_hash(policy).is_err() {
        reasons.insert("UNSUPPORTED_PROFILE");
    } else {
        let limit = |s: &str| decimal(s).expect("limits checked by policy_hash");
        if bytes.len() as u64 > limit(&policy.limits.max_bytes) {
            reasons.insert("OVERSIZED");
        }
        if bytes.len() <= MAX_BYTES {
            match object(bytes) {
                Err(_) => {
                    reasons.insert("MALFORMED_JSON");
                }
                Ok(obj) => {
                    if obj.duplicate {
                        reasons.insert("DUPLICATE_FIELD");
                    }
                    if obj.fields.len() as u64 > limit(&policy.limits.max_fields) {
                        reasons.insert("TOO_MANY_FIELDS");
                    }
                    let mut present = BTreeSet::new();
                    for (name, value) in &obj.fields {
                        present.insert(name);
                        if name.len() as u64 > limit(&policy.limits.max_field_name_bytes)
                            || matches!(value,FieldValue::String(s) if s.len() as u64>limit(&policy.limits.max_string_bytes))
                        {
                            reasons.insert("FIELD_LENGTH");
                        }
                        if matches!(value, FieldValue::Nested) {
                            reasons.insert("SCHEMA_MISMATCH");
                        }
                        if let Some(field_type) = policy.schema.get(name) {
                            let valid_type = match (field_type.as_str(), value) {
                                ("bool", FieldValue::Bool) | ("string", FieldValue::String(_)) => {
                                    true
                                }
                                ("u64_decimal", FieldValue::String(text)) => decimal(text).is_ok(),
                                _ => false,
                            };
                            if !valid_type {
                                reasons.insert("FIELD_TYPE");
                            }
                        } else {
                            reasons.insert("SCHEMA_MISMATCH");
                        }
                    }
                    if policy.schema.keys().any(|name| !present.contains(name)) {
                        reasons.insert("SCHEMA_MISMATCH");
                    }
                }
            }
        }
    }
    let decision = if reasons.is_empty() {
        reasons.insert("MATCH");
        "PASS"
    } else {
        "FAIL"
    };
    VerdictDraft {
        decision: decision.into(),
        reason_codes: reasons.into_iter().map(String::from).collect(),
        observed_digest: Some(hash),
        observed_length: Some(length),
    }
}
