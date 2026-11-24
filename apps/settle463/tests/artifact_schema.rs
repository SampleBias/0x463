use settle463_core::{
    artifact::digest,
    ident::hex_string,
    parse_signed,
    settlement::{Agreement, ChainOutcome, FundSignatures},
    wire::Body,
};
use settle463_sdk::{
    BuyerClient, BuyerPolicy, Error, HttpCoordinator, Identity, QuoteTerms, RequestTerms, Role,
    SellerClient, SellerPolicy, SettlementBackend, VerifierClient,
};
use std::{
    fs,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Arc,
};

const START: i64 = 1_700_000_000;
const EXTRA: &[u8; 64] = include_bytes!("../../../fixtures/artifacts/extra-field.json");
const DIGEST: &str = "a6c4530d161e5de02e365783fe7dbb96d93e6334ba78f0b52257e0b7fa75635a";
const PRINCIPAL: u64 = 2_000_000;

struct Coordinator {
    child: Child,
    dir: PathBuf,
}
impl Drop for Coordinator {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn committed_extra_field_fails_schema_and_keeps_principal_until_timeout(
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(digest(EXTRA), DIGEST);
    let buyer_id = Identity::generate()?;
    let seller_id = Identity::generate()?;
    let verifier_id = Identity::generate()?;
    let buyer_key = buyer_id.public_hex();
    let seller_key = seller_id.public_hex();
    let verifier_key = verifier_id.public_hex();
    let issuer_seed = [4; 32];
    let issuer = Identity::from_seed(&issuer_seed).public_hex();
    let env = format!("sim:{}", &buyer_key[..32]);
    let dir = std::env::temp_dir().join(format!("settle463-schema-{buyer_key}"));
    fs::create_dir(&dir)?;
    let config = settle463d::Config {
        database: dir.join("db.sqlite"),
        artifact_root: dir.join("artifacts"),
        environment: env.clone(),
        buyer: buyer_key,
        opening_balance: 5_000_000,
        clock: START,
        issuer_seed_file: dir.join("issuer.key"),
        admin_token: "a5".repeat(32),
    };
    fs::write(&config.issuer_seed_file, hex_string(&issuer_seed))?;
    let config_file = dir.join("config.json");
    fs::write(&config_file, serde_json::to_vec(&config)?)?;
    let child = Command::new(env!("CARGO_BIN_EXE_settle463"))
        .args(["__serve", "--config"])
        .arg(config_file)
        .args(["--listen", "127.0.0.1:0"])
        .stdout(Stdio::piped())
        .spawn()?;
    let mut server = Coordinator { child, dir };
    let mut ready = String::new();
    BufReader::new(server.child.stdout.take().unwrap()).read_line(&mut ready)?;
    let address = ready
        .trim()
        .strip_prefix("LISTEN=")
        .ok_or("coordinator did not start")?;
    let connect = |identity, role| {
        HttpCoordinator::new(&format!("http://{address}"), &env, identity, role).map(Arc::new)
    };
    let payer = connect(buyer_id, Role::Buyer)?;
    let buyer = BuyerClient::connect(
        payer.clone(),
        BuyerPolicy {
            trusted_publishers: [seller_key].into(),
            max_principal: PRINCIPAL,
            allowed_services: ["sample-json-report".into()].into(),
            allowed_assets: ["test-six-decimal".into()].into(),
            allowed_environments: [env.clone()].into(),
            allowed_verifiers: [verifier_key.clone()].into(),
            max_deliver_window_seconds: 3600,
            max_resolve_window_seconds: 3600,
        },
    )?;
    let seller = SellerClient::connect(
        connect(seller_id, Role::Seller)?,
        SellerPolicy {
            min_principal: PRINCIPAL,
            allowed_verifiers: [verifier_key.clone()].into(),
            allowed_environments: [env.clone()].into(),
        },
    )?;
    let verifier = VerifierClient::connect(connect(verifier_id, Role::Verifier)?)?;
    let offer = parse_signed(
        include_bytes!("../../../spec/examples/worked-transaction/service_offer.json"),
        "sim:fixture",
    )?;
    let Body::ServiceOffer(mut descriptor) = offer.unsigned().payload.clone() else {
        panic!("offer fixture")
    };
    descriptor.environments = vec![env.clone()];
    let offer = seller.publish_offer(descriptor)?;
    let reference = parse_signed(
        include_bytes!("../../../spec/examples/worked-transaction/quote.json"),
        "sim:fixture",
    )?;
    let Body::Quote(q) = reference.unsigned().payload.clone() else {
        panic!("quote fixture")
    };
    let mut policy = q.acceptance_policy;
    assert_eq!(
        policy.schema.keys().map(String::as_str).collect::<Vec<_>>(),
        ["job", "result", "units"]
    );
    policy.expected_digest = DIGEST.into();
    policy.expected_length = EXTRA.len().to_string();
    let request = buyer.create_request(
        &offer,
        RequestTerms {
            asset: q.asset,
            max_principal: PRINCIPAL,
            allowed_verifiers: vec![verifier_key.clone()],
            max_bytes: 1_048_576,
            expires_at: START + 3600,
        },
    )?;
    let quote = seller.quote(
        &request,
        QuoteTerms {
            amount: PRINCIPAL,
            verifier: verifier_key,
            receipt_issuer: issuer,
            policy,
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
    let signatures = FundSignatures {
        buyer: buyer.sign_fund(&fund)?,
        seller: seller.sign_fund(&fund)?,
    };
    assert_eq!(
        payer.submit_fund(&fund, &signatures)?.outcome,
        ChainOutcome::FinalizedSuccess
    );
    let blob = seller.upload_artifact(order, EXTRA)?;
    assert_eq!(blob.digest, DIGEST);
    let delivery = seller.commit_delivery(&agreement, &blob)?;
    let signed = verifier.judge(&agreement, &delivery)?;
    let Body::Verdict(report) = &signed.unsigned().payload else {
        panic!("expected verifier report")
    };
    assert_eq!(report.decision, "FAIL");
    assert_eq!(report.reason_codes, ["SCHEMA_MISMATCH"]);
    assert_eq!(report.observed_digest.as_deref(), Some(DIGEST));
    assert_eq!(report.observed_length.as_deref(), Some("64"));
    assert!(matches!(
        verifier.release_if_pass(&agreement, &delivery, &signed),
        Err(Error::PolicyRejected)
    ));
    let held = payer.order(order)?;
    assert_eq!(held["state"], "Delivered");
    assert_eq!(held["held"], "2000000");
    assert_eq!(held["paid"], "0");
    assert_eq!(held["seller_balance"], "0");

    payer.set_sim_clock(START + 10800, &config.admin_token)?;
    assert_eq!(
        buyer.refund_if_eligible(&agreement)?.outcome,
        ChainOutcome::FinalizedSuccess
    );
    let refunded = payer.order(order)?;
    assert_eq!(refunded["state"], "Refunded");
    assert_eq!(refunded["held"], "0");
    assert_eq!(refunded["refunded"], "2000000");
    assert_eq!(refunded["paid"], "0");
    assert_eq!(refunded["seller_balance"], "0");
    assert_eq!(refunded["buyer_balance"], "5000000");
    Ok(())
}
