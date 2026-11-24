use crate::{
    session::{self, Server},
    AnyResult,
};
use serde_json::{json, Value};
use settle463_core::{
    ident::{decimal, hex_string},
    parse_signed,
    receipt::{verify_sim_bundle, ReceiptTrust},
    settlement::{Agreement, ChainOutcome, FundSignatures},
    wire::Body,
};
use settle463_sdk::{
    BuyerClient, BuyerPolicy, HttpCoordinator, QuoteTerms, RequestTerms, Role, SellerClient,
    SellerPolicy, SettlementBackend, VerifierClient,
};
use std::{collections::BTreeSet, path::Path, sync::Arc};

const START: i64 = 1_700_000_000;
const REFERENCE: &[u8] = include_bytes!("../../../fixtures/artifacts/reference.json");
const CHANGED: &[u8] = include_bytes!("../../../fixtures/artifacts/one-byte-change.json");

pub fn run(scenario: &str, dir: &Path) -> AnyResult<Value> {
    if !matches!(
        scenario,
        "pass" | "bad-artifact" | "no-delivery" | "verifier-offline"
    ) {
        return Err("unknown scenario".into());
    }
    let buyer_id = session::key(dir, "buyer")?;
    let seller_id = session::key(dir, "seller")?;
    let verifier_id = session::key(dir, "verifier")?;
    let issuer = session::key(dir, "issuer")?;
    let env = format!("sim:{}", hex_string(&session::random()?[..16]));
    let token = hex_string(&session::random()?);
    let opening = 5_000_000u64;
    let config = dir.join("coordinator.json");
    let settings = settle463d::Config {
        database: dir.join("coordinator.sqlite"),
        artifact_root: dir.join("artifacts"),
        environment: env.clone(),
        clock: START,
        buyer: buyer_id.public_hex(),
        opening_balance: opening,
        issuer_seed_file: dir.join("issuer.key"),
        admin_token: token.clone(),
    };
    session::private_write(&config, &serde_json::to_vec_pretty(&settings)?)?;
    let server = Server::start(&config)?;
    let payer = Arc::new(HttpCoordinator::new(
        &server.base,
        &env,
        buyer_id,
        Role::Buyer,
    )?);
    let seller_key = seller_id.public_hex();
    let verifier_key = verifier_id.public_hex();
    let seller_http = Arc::new(HttpCoordinator::new(
        &server.base,
        &env,
        seller_id,
        Role::Seller,
    )?);
    let verifiers = BTreeSet::from([verifier_key.clone()]);
    let environments = BTreeSet::from([env.clone()]);
    let buyer = BuyerClient::connect(
        payer.clone(),
        BuyerPolicy {
            trusted_publishers: BTreeSet::from([seller_key]),
            max_principal: 2_500_000,
            allowed_services: BTreeSet::from(["sample-json-report".into()]),
            allowed_assets: BTreeSet::from(["test-six-decimal".into()]),
            allowed_environments: environments.clone(),
            allowed_verifiers: verifiers.clone(),
            max_deliver_window_seconds: 86400,
            max_resolve_window_seconds: 86400,
        },
    )?;
    let seller = SellerClient::connect(
        seller_http,
        SellerPolicy {
            allowed_verifiers: verifiers,
            min_principal: 1_000_000,
            allowed_environments: environments,
        },
    )?;
    let fixture_offer = parse_signed(
        include_bytes!("../../../spec/examples/worked-transaction/service_offer.json"),
        "sim:fixture",
    )?;
    let Body::ServiceOffer(mut offer) = fixture_offer.unsigned().payload.clone() else {
        return Err("invalid offer fixture".into());
    };
    offer.environments = vec![env.clone()];
    seller.publish_offer(offer)?;
    let trusted = buyer.discover()?.pop().ok_or("no trusted offer")?;
    let fixture_quote = parse_signed(
        include_bytes!("../../../spec/examples/worked-transaction/quote.json"),
        "sim:fixture",
    )?;
    let Body::Quote(q) = fixture_quote.unsigned().payload.clone() else {
        return Err("invalid quote fixture".into());
    };
    let request = buyer.create_request(
        &trusted,
        RequestTerms {
            asset: q.asset,
            max_principal: 2_500_000,
            allowed_verifiers: vec![verifier_key.clone()],
            max_bytes: 1_048_576,
            expires_at: START + 3600,
        },
    )?;
    let quote = seller.quote(
        &request,
        QuoteTerms {
            amount: 2_000_000,
            verifier: verifier_key,
            receipt_issuer: issuer.public_hex(),
            policy: q.acceptance_policy,
            deadlines: q.deadlines,
            expires_at: START + 3600,
            fees: q.fees,
            artifact_store: q.artifact_store,
        },
    )?;
    let acceptance = buyer.accept(&request, &quote)?;
    let agreement = Agreement {
        request,
        quote,
        acceptance,
    };
    let fund = payer.prepare_fund(agreement.clone())?;
    let order = &fund.review.order_id;
    let trust = ReceiptTrust {
        trusted_order: order.clone(),
        trusted_issuer: issuer.public_hex(),
        environment: env.clone(),
    };
    session::write_trust(dir, &trust)?;
    let sig = FundSignatures {
        buyer: buyer.sign_fund(&fund)?,
        seller: seller.sign_fund(&fund)?,
    };
    if payer.submit_fund(&fund, &sig)?.outcome != ChainOutcome::FinalizedSuccess {
        return Err("funding pending".into());
    }

    if scenario == "no-delivery" {
        payer.set_sim_clock(START + 7200, &token)?;
        buyer.refund_if_eligible(&agreement)?;
    } else {
        let bytes = if scenario == "bad-artifact" {
            CHANGED
        } else {
            REFERENCE
        };
        let blob = seller.upload_artifact(order, bytes)?;
        let delivery = seller.commit_delivery(&agreement, &blob)?;
        if scenario == "pass" {
            let downloaded = buyer.read_artifact(order, &blob.content_id)?;
            if downloaded.as_slice() != REFERENCE {
                return Err("buyer download differs from reference".into());
            }
            eprintln!(
                "buyer_download length={} sha256={}",
                downloaded.len(),
                settle463_core::artifact::digest(&downloaded)
            );
        }
        if scenario != "verifier-offline" {
            let verifier = VerifierClient::connect(Arc::new(HttpCoordinator::new(
                &server.base,
                &env,
                verifier_id,
                Role::Verifier,
            )?))?;
            let verdict = verifier.judge(&agreement, &delivery)?;
            let Body::Verdict(v) = &verdict.unsigned().payload else {
                return Err("unexpected verdict kind".into());
            };
            match (scenario, v.decision.as_str()) {
                ("pass", "PASS") => {
                    verifier.release_if_pass(&agreement, &delivery, &verdict)?;
                }
                ("bad-artifact", "FAIL") => (),
                _ => return Err("unexpected artifact decision".into()),
            }
        }
        if scenario != "pass" {
            payer.set_sim_clock(START + 10800, &token)?;
            buyer.refund_if_eligible(&agreement)?;
        }
    }
    let bytes = buyer.export_receipt(order)?;
    let receipt = verify_sim_bundle(&bytes, &trust)?;
    let path = dir.join("receipt.json");
    session::private_write(&path, &bytes)?;
    let projection = payer.order(order)?;
    let value = |k: &str| -> AnyResult<u64> {
        Ok(decimal(projection[k].as_str().ok_or("missing balance")?)?)
    };
    Ok(
        json!({"order_id":order,"backend":"sim","outcome":receipt.outcome,"asset":receipt.asset.id,"principal":receipt.principal,
        "delta_buyer":(i128::from(value("buyer_balance")?)-i128::from(opening)).to_string(),"delta_seller":value("seller_balance")?.to_string(),
        "delta_escrow":value("held")?.to_string(),"receipt":path}),
    )
}
