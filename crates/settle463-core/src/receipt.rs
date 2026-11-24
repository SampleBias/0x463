//! Signed receipt bundles and offline verification against caller-pinned identities.

use crate::ident::{decimal, hex_string, timestamp};
use crate::settlement::{
    check_delivery, check_pass_policy, check_verdict, Agreement, OrderState, OrderView, Review,
};
use crate::wire::{Body, Receipt};
use crate::{canonical, parse_signed, SignedMessage};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReceiptTrust {
    pub trusted_issuer: String,
    pub trusted_order: String,
    pub environment: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ReceiptError {
    #[error("invalid_evidence")]
    InvalidEvidence,
    #[error("untrusted")]
    Untrusted,
}
use ReceiptError::InvalidEvidence;

#[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SimLedger {
    ledger_kind: String,
    deployment_id: String,
    clock: String,
    finality: String,
    principal: String,
    decimals: String,
    asset_id: String,
    destination: String,
    terminal_locator: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bundle<L> {
    bundle_version: String,
    order_id: String,
    environment: String,
    request: Value,
    quote: Value,
    acceptance: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    delivery: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    verdict: Option<Value>,
    receipt: Value,
    observations: Vec<Value>,
    ledger: L,
}

pub fn make_sim_receipt(
    view: &OrderView,
    failed_verdict: Option<&SignedMessage>,
) -> Result<Receipt, ReceiptError> {
    terminal_receipt(
        view,
        failed_verdict,
        "sim",
        "simulated-final",
        Some(view.terminal_clock.ok_or(InvalidEvidence)?),
    )
}

pub fn make_solana_receipt(
    view: &OrderView,
    failed_verdict: Option<&SignedMessage>,
) -> Result<Receipt, ReceiptError> {
    terminal_receipt(
        view,
        failed_verdict,
        "solana",
        "finalized",
        Some(view.terminal_clock.ok_or(InvalidEvidence)?),
    )
}

fn terminal_receipt(
    view: &OrderView,
    failed_verdict: Option<&SignedMessage>,
    kind: &str,
    finality: &str,
    clock: Option<i64>,
) -> Result<Receipt, ReceiptError> {
    let r = &view.review;
    let locator = view.terminal_locator.as_ref().ok_or(InvalidEvidence)?;
    let amount = decimal(&r.principal).map_err(|_| InvalidEvidence)?;
    if r.environment.split(':').next() != Some(kind)
        || amount == 0
        || view.held != 0
        || clock.is_some_and(|now| now < 0)
    {
        return Err(InvalidEvidence);
    }
    let delivered = view.delivery_id.is_some();
    let resolve = timestamp(&r.deadlines.resolve_by).map_err(|_| InvalidEvidence)?;
    let deliver = timestamp(&r.deadlines.deliver_by).map_err(|_| InvalidEvidence)?;
    let (outcome, destination, verdict_id) = match view.state {
        OrderState::Paid
            if delivered
                && clock.is_none_or(|now| now < resolve)
                && view.paid == amount
                && view.refunded == 0
                && failed_verdict.is_none() =>
        {
            (
                "paid",
                &r.seller_destination,
                Some(view.verdict_id.clone().ok_or(InvalidEvidence)?),
            )
        }
        OrderState::Refunded
            if view.refunded == amount && view.paid == 0 && view.verdict_id.is_none() =>
        {
            let outcome = if delivered && clock.is_none_or(|now| now >= resolve) {
                "refunded_timeout_after_delivery"
            } else if !delivered && clock.is_none_or(|now| now >= deliver) {
                "refunded_timeout_before_delivery"
            } else {
                return Err(InvalidEvidence);
            };
            let verdict_id = if let Some(m) = failed_verdict {
                let Body::Verdict(v) = &m.unsigned().payload else {
                    return Err(InvalidEvidence);
                };
                if v.decision != "FAIL"
                    || m.unsigned().issuer != r.verifier
                    || m.unsigned().environment != r.environment
                    || v.order_id != r.order_id
                    || v.quote_id != r.quote_id
                    || v.policy_hash != r.policy_hash
                    || view.delivery_id.as_ref() != Some(&v.delivery_id)
                {
                    return Err(InvalidEvidence);
                }
                Some(hex_string(m.id()))
            } else {
                None
            };
            (outcome, &r.buyer_destination, verdict_id)
        }
        _ => return Err(InvalidEvidence),
    };
    Ok(Receipt {
        order_id: r.order_id.clone(),
        outcome: outcome.to_owned(),
        principal: r.principal.clone(),
        asset: r.asset.clone(),
        buyer: r.buyer.clone(),
        seller: r.seller.clone(),
        verifier: r.verifier.clone(),
        receipt_issuer: r.receipt_issuer.clone(),
        request_id: r.request_id.clone(),
        quote_id: r.quote_id.clone(),
        acceptance_id: r.acceptance_id.clone(),
        environment: r.environment.clone(),
        destination: destination.clone(),
        ledger_kind: kind.into(),
        terminal_locator: locator.clone(),
        finality: finality.into(),
        delivery_id: view.delivery_id.clone(),
        verdict_id,
    })
}

fn ledger(view: &OrderView, receipt: &Receipt) -> Result<SimLedger, ReceiptError> {
    Ok(SimLedger {
        ledger_kind: receipt.ledger_kind.clone(),
        deployment_id: receipt
            .environment
            .strip_prefix("sim:")
            .ok_or(InvalidEvidence)?
            .to_owned(),
        clock: view.terminal_clock.ok_or(InvalidEvidence)?.to_string(),
        finality: receipt.finality.clone(),
        principal: receipt.principal.clone(),
        decimals: receipt.asset.decimals.clone(),
        asset_id: receipt.asset.id.clone(),
        destination: receipt.destination.clone(),
        terminal_locator: receipt.terminal_locator.clone(),
    })
}

fn value(m: &SignedMessage) -> Result<Value, ReceiptError> {
    serde_json::from_slice(&m.to_wire().map_err(|_| InvalidEvidence)?).map_err(|_| InvalidEvidence)
}

pub fn build_sim_bundle(
    agreement: &Agreement,
    delivery: Option<&SignedMessage>,
    verdict: Option<&SignedMessage>,
    receipt: &SignedMessage,
    observations: &[SignedMessage],
    view: &OrderView,
) -> Result<Vec<u8>, ReceiptError> {
    let Body::Receipt(body) = &receipt.unsigned().payload else {
        return Err(InvalidEvidence);
    };
    if *body.as_ref()
        != make_sim_receipt(view, verdict.filter(|_| view.state == OrderState::Refunded))?
        || Review::sim_fund(agreement, &view.review.environment).map_err(|_| InvalidEvidence)?
            != view.review
    {
        return Err(InvalidEvidence);
    }
    let bytes = assemble(
        agreement,
        delivery,
        verdict,
        receipt,
        observations,
        ledger(view, body)?,
    )?;
    verify_sim_bundle(&bytes, &issuer_trust(body))?;
    Ok(bytes)
}

fn issuer_trust(receipt: &Receipt) -> ReceiptTrust {
    ReceiptTrust {
        trusted_issuer: receipt.receipt_issuer.clone(),
        trusted_order: receipt.order_id.clone(),
        environment: receipt.environment.clone(),
    }
}

fn assemble<L: Serialize>(
    agreement: &Agreement,
    delivery: Option<&SignedMessage>,
    verdict: Option<&SignedMessage>,
    receipt: &SignedMessage,
    observations: &[SignedMessage],
    ledger: L,
) -> Result<Vec<u8>, ReceiptError> {
    let Body::Receipt(body) = &receipt.unsigned().payload else {
        return Err(InvalidEvidence);
    };
    let bundle = Bundle {
        bundle_version: "0.1".into(),
        order_id: body.order_id.clone(),
        environment: body.environment.clone(),
        request: value(&agreement.request)?,
        quote: value(&agreement.quote)?,
        acceptance: value(&agreement.acceptance)?,
        delivery: delivery.map(value).transpose()?,
        verdict: verdict.map(value).transpose()?,
        receipt: value(receipt)?,
        observations: observations.iter().map(value).collect::<Result<_, _>>()?,
        ledger,
    };
    serde_json::to_vec(&bundle).map_err(|_| InvalidEvidence)
}

fn decode<L: serde::de::DeserializeOwned + Serialize>(
    bytes: &[u8],
    prefix: &str,
) -> Result<Bundle<L>, ReceiptError> {
    let parsed = canonical::parse(bytes).map_err(|_| InvalidEvidence)?;
    let b: Bundle<L> = serde_json::from_value(parsed.clone()).map_err(|_| InvalidEvidence)?;
    if b.bundle_version != "0.1"
        || !b.environment.starts_with(prefix)
        || serde_json::to_value(&b).map_err(|_| InvalidEvidence)? != parsed
    {
        return Err(InvalidEvidence);
    }
    Ok(b)
}

struct Checked {
    agreement: Agreement,
    delivery: Option<SignedMessage>,
    verdict: Option<SignedMessage>,
    receipt: Receipt,
    view: OrderView,
}

fn checked<L>(b: &Bundle<L>, clock: Option<i64>, locator: &str) -> Result<Checked, ReceiptError> {
    let message = |v: &Value| {
        parse_signed(
            &canonical::canonical(v).map_err(|_| InvalidEvidence)?,
            &b.environment,
        )
        .map_err(|_| InvalidEvidence)
    };
    let agreement = Agreement {
        request: message(&b.request)?,
        quote: message(&b.quote)?,
        acceptance: message(&b.acceptance)?,
    };
    let review = Review::from_agreement(&agreement, &b.environment).map_err(|_| InvalidEvidence)?;
    let (_, quote) = agreement
        .terms(&b.environment)
        .map_err(|_| InvalidEvidence)?;
    if b.order_id != review.order_id {
        return Err(InvalidEvidence);
    }
    let signed = message(&b.receipt)?;
    let Body::Receipt(receipt) = &signed.unsigned().payload else {
        return Err(InvalidEvidence);
    };
    let delivery = b.delivery.as_ref().map(message).transpose()?;
    let verdict = b.verdict.as_ref().map(message).transpose()?;
    if let Some(m) = &delivery {
        check_delivery(&review, m).map_err(|_| InvalidEvidence)?;
    }
    if let Some(v) = &verdict {
        check_verdict(&review, delivery.as_ref().ok_or(InvalidEvidence)?, v)
            .map_err(|_| InvalidEvidence)?;
        check_pass_policy(&quote.artifact, v).map_err(|_| InvalidEvidence)?;
    }
    let amount = decimal(&review.principal).map_err(|_| InvalidEvidence)?;
    let (state, paid, refunded) = match receipt.outcome.as_str() {
        "paid"
            if verdict.as_ref().is_some_and(
                |m| matches!(&m.unsigned().payload,Body::Verdict(v) if v.decision=="PASS"),
            ) =>
        {
            (OrderState::Paid, amount, 0)
        }
        "refunded_timeout_before_delivery" | "refunded_timeout_after_delivery" => {
            (OrderState::Refunded, 0, amount)
        }
        _ => return Err(InvalidEvidence),
    };
    let view = OrderView {
        state,
        review,
        held: 0,
        paid,
        refunded,
        delivery_id: delivery.as_ref().map(|m| hex_string(m.id())),
        verdict_id: verdict
            .as_ref()
            .filter(|_| state == OrderState::Paid)
            .map(|m| hex_string(m.id())),
        terminal_clock: clock,
        terminal_locator: Some(locator.to_owned()),
    };
    let mut prev = 0;
    for item in &b.observations {
        let m = message(item)?;
        let Body::Observation(o) = &m.unsigned().payload else {
            return Err(InvalidEvidence);
        };
        let seq = decimal(&o.event_seq).map_err(|_| InvalidEvidence)?;
        if o.order_id != view.review.order_id
            || m.unsigned().issuer != view.review.receipt_issuer
            || seq <= prev
        {
            return Err(InvalidEvidence);
        }
        prev = seq;
    }
    Ok(Checked {
        agreement,
        delivery,
        verdict,
        receipt: *receipt.clone(),
        view,
    })
}

/// Authenticates the signed agreement and its simulator evidence before comparing caller trust.
pub fn verify_sim_bundle(bytes: &[u8], trust: &ReceiptTrust) -> Result<Receipt, ReceiptError> {
    let b: Bundle<SimLedger> = decode(bytes, "sim:")?;
    let c = checked(
        &b,
        Some(timestamp(&b.ledger.clock).map_err(|_| InvalidEvidence)?),
        &b.ledger.terminal_locator,
    )?;
    let expected = make_sim_receipt(
        &c.view,
        c.verdict
            .as_ref()
            .filter(|_| c.view.state == OrderState::Refunded),
    )?;
    if c.receipt != expected || b.ledger != ledger(&c.view, &expected)? {
        return Err(InvalidEvidence);
    }
    if &issuer_trust(&expected) != trust {
        return Err(ReceiptError::Untrusted);
    }
    Ok(expected)
}

mod solana;
pub use solana::{build_solana_bundle, verify_solana_bundle, SolanaLedger, VerifiedSolanaBundle};
