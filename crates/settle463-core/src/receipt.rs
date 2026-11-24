//! Simulator receipt construction and offline verification against caller-pinned identities.

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
struct Bundle {
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
    ledger: SimLedger,
}

pub fn make_sim_receipt(
    view: &OrderView,
    failed_verdict: Option<&SignedMessage>,
) -> Result<Receipt, ReceiptError> {
    let r = &view.review;
    let clock = view.terminal_clock.ok_or(InvalidEvidence)?;
    let locator = view.terminal_locator.as_ref().ok_or(InvalidEvidence)?;
    let amount = decimal(&r.principal).map_err(|_| InvalidEvidence)?;
    if !r.environment.starts_with("sim:") || amount == 0 || view.held != 0 || clock < 0 {
        return Err(InvalidEvidence);
    }
    let delivered = view.delivery_id.is_some();
    let resolve = timestamp(&r.deadlines.resolve_by).map_err(|_| InvalidEvidence)?;
    let deliver = timestamp(&r.deadlines.deliver_by).map_err(|_| InvalidEvidence)?;
    let (outcome, destination, verdict_id) = match view.state {
        OrderState::Paid
            if delivered
                && clock < resolve
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
            let outcome = if delivered && clock >= resolve {
                "refunded_timeout_after_delivery"
            } else if !delivered && clock >= deliver {
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
        ledger_kind: "sim".into(),
        terminal_locator: locator.clone(),
        finality: "simulated-final".into(),
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
        ledger: ledger(view, body)?,
    };
    let bytes = serde_json::to_vec(&bundle).map_err(|_| InvalidEvidence)?;
    verify_sim_bundle(
        &bytes,
        &ReceiptTrust {
            trusted_issuer: view.review.receipt_issuer.clone(),
            trusted_order: view.review.order_id.clone(),
            environment: view.review.environment.clone(),
        },
    )?;
    Ok(bytes)
}

/// Checks the signed order and its internal simulator evidence. External trust pins are
/// checked after authentication so a damaged signature is reported as invalid evidence.
pub fn verify_sim_bundle(bytes: &[u8], trust: &ReceiptTrust) -> Result<Receipt, ReceiptError> {
    let parsed = canonical::parse(bytes).map_err(|_| InvalidEvidence)?;
    let b: Bundle = serde_json::from_value(parsed.clone()).map_err(|_| InvalidEvidence)?;
    if b.bundle_version != "0.1"
        || !b.environment.starts_with("sim:")
        || serde_json::to_value(&b).map_err(|_| InvalidEvidence)? != parsed
    {
        return Err(InvalidEvidence);
    }
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
    let review = Review::sim_fund(&agreement, &b.environment).map_err(|_| InvalidEvidence)?;
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
        terminal_clock: Some(timestamp(&b.ledger.clock).map_err(|_| InvalidEvidence)?),
        terminal_locator: Some(b.ledger.terminal_locator.clone()),
    };
    let expected = make_sim_receipt(
        &view,
        verdict.as_ref().filter(|_| state == OrderState::Refunded),
    )?;
    if **receipt != expected || b.ledger != ledger(&view, &expected)? {
        return Err(InvalidEvidence);
    }
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
    if trust.environment != b.environment
        || trust.trusted_order != b.order_id
        || trust.trusted_issuer != view.review.receipt_issuer
    {
        return Err(ReceiptError::Untrusted);
    }
    Ok(expected)
}
