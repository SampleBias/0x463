use crate::{
    session::{self, Server},
    AnyResult,
};
use serde_json::{json, Value};
use settle463_core::{
    ident::{decimal, hex_string},
    parse_signed,
    receipt::{verify_sim_bundle, verify_solana_bundle, ReceiptTrust},
    settlement::{Agreement, ChainOutcome, Review},
    wire::{Asset, Body, Deadlines, Settlement},
};
use settle463_sdk::{
    BuyerClient, BuyerPolicy, HttpCoordinator, QuoteTerms, RequestTerms, Role, SellerClient,
    SellerPolicy, VerifierClient,
};
use std::{collections::BTreeSet, path::Path, sync::Arc};

const START: i64 = 1_700_000_000;
const REFERENCE: &[u8] = include_bytes!("../../../fixtures/artifacts/reference.json");
const CHANGED: &[u8] = include_bytes!("../../../fixtures/artifacts/one-byte-change.json");

pub fn run(scenario: &str, dir: &Path, deployment: Option<&Path>) -> AnyResult<Value> {
    if !matches!(
        scenario,
        "pass" | "bad-artifact" | "no-delivery" | "verifier-offline"
    ) {
        return Err("unknown scenario".into());
    }
    let validator = deployment
        .map(crate::validator::Validator::load)
        .transpose()?
        .map(Arc::new);
    let key = |name: &str| -> AnyResult<_> {
        if let Some(file) = deployment {
            crate::validator::fixture_key(file, dir, name)
        } else {
            session::key(
                dir,
                if name == "coordinator" {
                    "issuer"
                } else {
                    name
                },
            )
        }
    };
    let buyer_id = key("buyer")?;
    let seller_id = key("seller")?;
    let verifier_id = key("verifier")?;
    let issuer = key("coordinator")?;
    let (env, start, asset, deadlines) = if let Some(v) = &validator {
        let start = v.rpc.clock()?;
        eprintln!(
            "{} mint={}",
            settle463_core::receipt::SolanaLedger::LOCAL_ASSET_LABEL,
            v.deployment.mint
        );
        (
            v.deployment.environment(),
            start,
            Asset {
                kind: "spl-token".into(),
                id: v.deployment.mint.clone(),
                decimals: "6".into(),
            },
            Deadlines {
                fund_by: (start + 45).to_string(),
                deliver_by: (start + 65).to_string(),
                resolve_by: (start + 95).to_string(),
            },
        )
    } else {
        (
            format!("sim:{}", hex_string(&session::random()?[..16])),
            START,
            Asset {
                kind: "sim".into(),
                id: "test-six-decimal".into(),
                decimals: "6".into(),
            },
            Deadlines {
                fund_by: (START + 3600).to_string(),
                deliver_by: (START + 7200).to_string(),
                resolve_by: (START + 10800).to_string(),
            },
        )
    };
    let token = hex_string(&session::random()?);
    let opening = 5_000_000u64;
    let config = dir.join("coordinator.json");
    let settings = settle463d::Config {
        deployment: validator.as_ref().map(|v| v.deployment.clone()),
        database: dir.join("coordinator.sqlite"),
        artifact_root: dir.join("artifacts"),
        environment: env.clone(),
        clock: start,
        buyer: buyer_id.public_hex(),
        opening_balance: opening,
        issuer_seed_file: dir.join("issuer.key"),
        admin_token: token.clone(),
    };
    session::private_write(&config, &serde_json::to_vec_pretty(&settings)?)?;
    let server = Server::start(&config)?;
    let connect = |id, role| -> AnyResult<_> {
        let http = HttpCoordinator::new(&server.base, &env, id, role)?;
        Ok(Arc::new(if let Some(v) = &validator {
            http.with_ledger(v.clone())
        } else {
            http
        }))
    };
    let payer = connect(buyer_id, Role::Buyer)?;
    let seller_key = seller_id.public_hex();
    let verifier_key = verifier_id.public_hex();
    let seller_http = connect(seller_id, Role::Seller)?;
    let verifiers = BTreeSet::from([verifier_key.clone()]);
    let environments = BTreeSet::from([env.clone()]);
    let buyer = BuyerClient::connect(
        payer.clone(),
        BuyerPolicy {
            trusted_publishers: BTreeSet::from([seller_key]),
            max_principal: 2_500_000,
            allowed_services: BTreeSet::from(["sample-json-report".into()]),
            allowed_assets: BTreeSet::from([asset.id.clone()]),
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
    offer.expires_at = (start + 7200).to_string();
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
            asset,
            max_principal: 2_500_000,
            allowed_verifiers: vec![verifier_key.clone()],
            max_bytes: 1_048_576,
            expires_at: start + 3600,
        },
    )?;
    let quote = seller.quote(
        &request,
        QuoteTerms {
            amount: 2_000_000,
            verifier: verifier_key,
            receipt_issuer: issuer.public_hex(),
            policy: q.acceptance_policy,
            deadlines: deadlines.clone(),
            expires_at: start + 3600,
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
    let review = Review::from_agreement(&agreement, &env)?;
    let order = &review.order_id;
    let trust = ReceiptTrust {
        trusted_order: order.clone(),
        trusted_issuer: issuer.public_hex(),
        environment: env.clone(),
    };
    let balances = || -> AnyResult<[u64; 3]> {
        let Some(v) = &validator else {
            return Ok([opening, 0, 0]);
        };
        let (_, q) = agreement.terms(&env)?;
        let Settlement::Solana {
            buyer_token_account,
            seller_token_account,
            vault_token_account,
            ..
        } = &q.settlement
        else {
            return Err("missing token accounts".into());
        };
        Ok([
            v.rpc.balance(buyer_token_account)?,
            v.rpc.balance(seller_token_account)?,
            v.rpc.balance(vault_token_account)?,
        ])
    };
    let before = balances()?;
    if let Some(v) = &validator {
        let mut file = serde_json::to_value(&trust)?;
        file["genesis"] = v.deployment.genesis.clone().into();
        file["program_id"] = v.deployment.program_id.clone().into();
        session::private_write(&dir.join("trust.json"), &serde_json::to_vec_pretty(&file)?)?;
    } else {
        session::write_trust(dir, &trust)?;
    }
    if buyer.fund(&seller, &agreement)?.outcome != ChainOutcome::FinalizedSuccess {
        return Err("funding pending".into());
    }
    let wait = |target: &str| -> AnyResult<()> {
        let target = settle463_core::ident::timestamp(target)?;
        if let Some(v) = &validator {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(120);
            while v.rpc.clock()? < target {
                if std::time::Instant::now() >= end {
                    return Err("validator clock did not reach deadline".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
        } else {
            payer.set_sim_clock(target, &token)?;
        }
        Ok(())
    };

    if scenario == "no-delivery" {
        wait(&deadlines.deliver_by)?;
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
            let verifier = VerifierClient::connect(connect(verifier_id, Role::Verifier)?)?;
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
            wait(&deadlines.resolve_by)?;
            buyer.refund_if_eligible(&agreement)?;
        }
    }
    let bytes = buyer.export_receipt(order)?;
    let receipt = if validator.is_some() {
        verify_solana_bundle(&bytes, &trust)?.receipt
    } else {
        verify_sim_bundle(&bytes, &trust)?
    };
    let path = dir.join("receipt.json");
    session::private_write(&path, &bytes)?;
    let after = if validator.is_some() {
        balances()?
    } else {
        let projection = payer.order(order)?;
        let value = |k: &str| -> AnyResult<u64> {
            Ok(decimal(projection[k].as_str().ok_or("missing balance")?)?)
        };
        [
            value("buyer_balance")?,
            value("seller_balance")?,
            value("held")?,
        ]
    };
    let delta = |index: usize| (i128::from(after[index]) - i128::from(before[index])).to_string();
    Ok(json!({
        "order_id": order,
        "backend": if validator.is_some(){"local-validator"}else{"sim"},
        "outcome": receipt.outcome,
        "asset": receipt.asset.id,
        "principal": receipt.principal,
        "delta_buyer": delta(0),
        "delta_seller": delta(1),
        "delta_escrow": delta(2),
        "receipt": path,
    }))
}
