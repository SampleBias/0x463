use crate::ident::{base58, decimal, environment, hex, hex_string, timestamp};
use crate::wire::*;
use crate::{order_id, policy_hash, Error, Result, SignedMessage, UnsignedMessage};
use std::collections::HashSet;

const PROFILE: &str = "sha256-json-object-v1";
const MIB: u64 = 1_048_576;
const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

fn require(test: bool, field: &'static str) -> Result<()> {
    if test {
        Ok(())
    } else {
        Err(Error::Invalid(field))
    }
}

fn identifiers(values: &[&str]) -> Result<()> {
    for value in values {
        hex::<32>(value)?;
    }
    Ok(())
}

fn distinct(values: &[&str]) -> bool {
    values.iter().copied().collect::<HashSet<_>>().len() == values.len()
}

fn list(values: &[String], max: usize) -> Result<()> {
    require(
        !values.is_empty()
            && values.len() <= max
            && values.iter().collect::<HashSet<_>>().len() == values.len(),
        "bounded unique list",
    )
}

fn name(s: &str) -> Result<()> {
    require(
        (1..=64).contains(&s.len())
            && s.as_bytes()[0].is_ascii_lowercase()
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
        "service or store id",
    )
}

fn asset(a: &Asset, env: &str) -> Result<()> {
    require(a.decimals == "6", "asset decimals")?;
    match a.kind.as_str() {
        "sim" if env.starts_with("sim:") => require(a.id == "test-six-decimal", "sim asset"),
        "spl-token" if env.starts_with("solana:") => base58::<32>(&a.id).map(|_| ()),
        _ => Err(Error::Invalid("asset kind")),
    }
}

pub(crate) fn policy(p: &Policy) -> Result<()> {
    hex::<32>(&p.expected_digest)?;
    let bytes = decimal(&p.limits.max_bytes)?;
    let names = decimal(&p.limits.max_field_name_bytes)?;
    let fields = decimal(&p.limits.max_fields)?;
    let strings = decimal(&p.limits.max_string_bytes)?;
    require(
        p.profile == PROFILE
            && bytes <= MIB
            && decimal(&p.expected_length)? <= bytes
            && names <= 64
            && (1..=64).contains(&fields)
            && strings <= 4096
            && !p.schema.is_empty()
            && p.schema.len() as u64 <= fields,
        "policy limits",
    )?;
    for (key, value) in &p.schema {
        require(
            key.len() as u64 <= names
                && matches!(value.as_str(), "string" | "bool" | "u64_decimal"),
            "policy schema",
        )?;
    }
    Ok(())
}

impl UnsignedMessage {
    pub fn validate(&self, expected: &str) -> Result<()> {
        require(
            self.protocol == "0x463" && self.version == "0.1",
            "protocol or version",
        )?;
        environment(&self.environment)?;
        require(self.environment == expected, "pinned environment")?;
        hex::<32>(&self.issuer)?;
        let env = &self.environment;
        match &self.payload {
            Body::ServiceOffer(o) => {
                name(&o.service_id)?;
                list(&o.environments, 8)?;
                list(&o.verification_profiles, 8)?;
                for e in &o.environments {
                    environment(e)?;
                }
                timestamp(&o.expires_at)?;
                require(
                    o.descriptor_version == "1"
                        && o.input_profile.content_type == "application/json"
                        && o.output_profile.content_type == "application/json"
                        && o.input_profile.schema_profile == PROFILE
                        && o.output_profile.schema_profile == PROFILE
                        && (1..=MIB).contains(&decimal(&o.input_profile.max_bytes)?)
                        && o.verification_profiles.iter().all(|s| !s.is_empty()),
                    "offer profile",
                )
            }
            Body::Request(r) => {
                identifiers(&[&r.offer_id, &r.seller])?;
                require(
                    self.issuer == r.buyer && r.buyer != r.seller,
                    "request roles",
                )?;
                require(
                    hex::<32>(&r.order_id)? == order_id(env, &r.buyer, &r.nonce)?,
                    "derived order id",
                )?;
                list(&r.allowed_verifiers, 8)?;
                for key in &r.allowed_verifiers {
                    hex::<32>(key)?;
                    require(key != &r.buyer && key != &r.seller, "allowed verifier")?;
                }
                name(&r.service_id)?;
                asset(&r.asset, env)?;
                timestamp(&r.expires_at)?;
                require(
                    r.artifact_constraints.verification_profile == PROFILE
                        && decimal(&r.artifact_constraints.max_bytes)? <= MIB
                        && decimal(&r.max_principal)? > 0,
                    "request constraints",
                )
            }
            Body::Quote(q) => quote(q, env, &self.issuer),
            Body::Accept(a) => {
                identifiers(&[&a.request_id, &a.quote_id, &a.order_id])?;
                asset(&a.asset, env)?;
                require(a.environment == *env && decimal(&a.amount)? > 0, "accept")
            }
            Body::Delivery(d) => {
                identifiers(&[
                    &d.order_id,
                    &d.quote_id,
                    &d.policy_hash,
                    &d.digest,
                    &d.content_id,
                ])?;
                decimal(&d.length).map(|_| ())
            }
            Body::Verdict(v) => verdict(v),
            Body::Receipt(r) => receipt(r, env, &self.issuer),
            Body::Observation(o) => {
                identifiers(&[&o.order_id, &o.event_digest])?;
                require(
                    decimal(&o.event_seq)? > 0
                        && o.refs.len() <= 16
                        && matches!(
                            o.event_type.as_str(),
                            "offer_published"
                                | "request_stored"
                                | "quote_stored"
                                | "accepted"
                                | "artifact_stored"
                                | "intent_recorded"
                                | "submitted"
                                | "finalized"
                                | "reconciliation_gap"
                                | "receipt_issued"
                        ),
                    "observation",
                )?;
                for reference in &o.refs {
                    if hex::<32>(reference).is_err() {
                        locator(reference, env, &o.order_id)?;
                    }
                }
                Ok(())
            }
        }
    }
}

fn quote(q: &Quote, env: &str, issuer: &str) -> Result<()> {
    identifiers(&[
        &q.request_id,
        &q.order_id,
        &q.offer_id,
        &q.buyer,
        &q.seller,
        &q.verifier,
        &q.receipt_issuer,
    ])?;
    require(
        q.seller == issuer && distinct(&[&q.buyer, &q.seller, &q.verifier, &q.receipt_issuer]),
        "quote roles",
    )?;
    name(&q.service_id)?;
    name(&q.artifact_store.store_id)?;
    asset(&q.asset, env)?;
    let fund = timestamp(&q.deadlines.fund_by)?;
    let deliver = timestamp(&q.deadlines.deliver_by)?;
    require(
        fund < deliver
            && deliver < timestamp(&q.deadlines.resolve_by)?
            && fund <= timestamp(&q.expires_at)?,
        "quote deadlines",
    )?;
    let p = &q.acceptance_policy;
    require(
        hex::<32>(&q.policy_hash)? == policy_hash(p)?
            && q.artifact.digest == p.expected_digest
            && q.artifact.length == p.expected_length
            && q.artifact.verification_profile == p.profile
            && decimal(&q.artifact.max_bytes)? <= MIB
            && decimal(&q.artifact.length)? <= decimal(&q.artifact.max_bytes)?,
        "quote artifact",
    )?;
    decimal(&q.artifact_store.retention_seconds)?;
    require(
        decimal(&q.amount)? > 0
            && q.fees.protocol_atomic == "0"
            && matches!(
                q.fees.network_fee_payer.as_str(),
                "buyer" | "seller" | "verifier" | "external"
            )
            && q.artifact_store.read_access == "order-parties",
        "quote terms",
    )?;
    match &q.settlement {
        Settlement::Sim {
            asset_id,
            buyer_destination,
            seller_destination,
        } => require(
            q.asset.kind == "sim"
                && asset_id == &q.asset.id
                && buyer_destination.strip_prefix("sim:") == Some(&q.buyer)
                && seller_destination.strip_prefix("sim:") == Some(&q.seller),
            "sim settlement",
        ),
        Settlement::Solana {
            mint,
            token_program,
            buyer_wallet,
            seller_wallet,
            buyer_token_account,
            seller_token_account,
            escrow_pda,
            vault_token_account,
            config,
            program_id,
        } => {
            for key in [
                buyer_token_account,
                seller_token_account,
                escrow_pda,
                vault_token_account,
                config,
                program_id,
            ] {
                base58::<32>(key)?;
            }
            let mut ids = env.split(':').skip(2);
            require(
                q.asset.kind == "spl-token"
                    && mint == &q.asset.id
                    && token_program == TOKEN_PROGRAM
                    && base58::<32>(buyer_wallet)? == hex::<32>(&q.buyer)?
                    && base58::<32>(seller_wallet)? == hex::<32>(&q.seller)?
                    && ids.next() == Some(program_id)
                    && ids.next() == Some(config),
                "solana settlement",
            )
        }
    }
}

fn verdict(v: &Verdict) -> Result<()> {
    identifiers(&[&v.order_id, &v.quote_id, &v.delivery_id, &v.policy_hash])?;
    list(&v.reason_codes, 8)?;
    require(
        v.algorithm == PROFILE
            && v.algorithm_version == "1"
            && v.reason_codes.windows(2).all(|w| w[0] < w[1]),
        "verdict profile or codes",
    )?;
    let observed = match (&v.observed_digest, &v.observed_length) {
        (Some(d), Some(n)) => {
            hex::<32>(d)?;
            decimal(n)?;
            true
        }
        (None, None) => false,
        _ => return Err(Error::Invalid("incomplete observations")),
    };
    for code in &v.reason_codes {
        match code.as_str() {
            "UNAVAILABLE" | "DEADLINE_EXPIRED" => {
                return require(
                    v.decision == "INDETERMINATE" && v.reason_codes.len() == 1,
                    "indeterminate verdict",
                );
            }
            "MATCH" => {
                return require(
                    v.decision == "PASS" && v.reason_codes.len() == 1 && observed,
                    "PASS verdict",
                )
            }
            "DIGEST_MISMATCH" | "SIZE_MISMATCH" | "SCHEMA_MISMATCH" | "FIELD_TYPE"
            | "MALFORMED_JSON" | "DUPLICATE_FIELD" | "TOO_MANY_FIELDS" | "FIELD_LENGTH" => {
                require(observed, "missing artifact observations")?
            }
            "OVERSIZED" | "UNSUPPORTED_PROFILE" => (),
            _ => return Err(Error::Invalid("unknown reason code")),
        }
    }
    require(v.decision == "FAIL", "verdict decision")
}

fn locator(s: &str, env: &str, order: &str) -> Result<()> {
    if env.starts_with("solana:") {
        return base58::<64>(s).map(|_| ());
    }
    let (id, seq) = s
        .strip_prefix("sim:")
        .and_then(|r| r.split_once(':'))
        .ok_or(Error::Invalid("sim locator"))?;
    require(id == order && decimal(seq)? > 0, "sim locator")
}

fn receipt(r: &Receipt, env: &str, issuer: &str) -> Result<()> {
    identifiers(&[
        &r.order_id,
        &r.buyer,
        &r.seller,
        &r.verifier,
        &r.receipt_issuer,
        &r.request_id,
        &r.quote_id,
        &r.acceptance_id,
    ])?;
    require(
        r.receipt_issuer == issuer
            && r.environment == env
            && decimal(&r.principal)? > 0
            && distinct(&[&r.buyer, &r.seller, &r.verifier, &r.receipt_issuer]),
        "receipt roles or terms",
    )?;
    asset(&r.asset, env)?;
    for id in [&r.delivery_id, &r.verdict_id].into_iter().flatten() {
        hex::<32>(id)?;
    }
    let party = match r.outcome.as_str() {
        "paid" if r.delivery_id.is_some() && r.verdict_id.is_some() => &r.seller,
        "refunded_timeout_before_delivery" if r.delivery_id.is_none() && r.verdict_id.is_none() => {
            &r.buyer
        }
        "refunded_timeout_after_delivery" if r.delivery_id.is_some() => &r.buyer,
        _ => return Err(Error::Invalid("receipt outcome")),
    };
    locator(&r.terminal_locator, env, &r.order_id)?;
    if env.starts_with("sim:") {
        require(
            r.ledger_kind == "sim"
                && r.finality == "simulated-final"
                && r.destination.strip_prefix("sim:") == Some(party),
            "sim receipt",
        )
    } else {
        base58::<32>(&r.destination)?;
        require(
            r.ledger_kind == "solana" && r.finality == "finalized",
            "solana receipt",
        )
    }
}

/// Checks a signed quote against the signed request, including limits outside the quote.
pub fn check_quote_request(quote: &SignedMessage, request: &SignedMessage) -> Result<()> {
    let Body::Quote(q) = &quote.unsigned().payload else {
        return Err(Error::Invalid("quote kind"));
    };
    check_quote_terms(q, &quote.unsigned().environment, request)
}

/// Checks contextual terms before the seller signs a quote.
pub fn check_quote_terms(q: &Quote, environment: &str, request: &SignedMessage) -> Result<()> {
    let Body::Request(r) = &request.unsigned().payload else {
        return Err(Error::Invalid("quote/request kinds"));
    };
    require(
        environment == request.unsigned().environment
            && q.request_id == hex_string(request.id())
            && q.order_id == r.order_id
            && q.offer_id == r.offer_id
            && q.service_id == r.service_id
            && q.buyer == r.buyer
            && q.seller == r.seller
            && q.asset == r.asset
            && r.allowed_verifiers.contains(&q.verifier)
            && decimal(&q.amount)? <= decimal(&r.max_principal)?
            && decimal(&q.artifact.max_bytes)? <= decimal(&r.artifact_constraints.max_bytes)?
            && timestamp(&q.deadlines.fund_by)? <= timestamp(&r.expires_at)?,
        "quote/request linkage",
    )
}
