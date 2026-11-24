use ed25519_dalek::SigningKey;
use serde_json::{json, Value};
use settle463_core::{
    ident::hex_string,
    order_id, parse_signed,
    settlement::{Agreement, ChainOutcome, OrderState},
    sign,
    wire::{Accept, Asset, Body, Deadlines},
    SignedMessage, UnsignedMessage,
};
use settle463_solana::{ChainAction, Deployment, Expected, Prepared, Resolution, Rpc, SignedTx};
use std::{
    env, fs,
    io::Read,
    net::TcpListener,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn key(directory: &Path, role: &str) -> Result<SigningKey> {
    let bytes: Vec<u8> =
        serde_json::from_slice(&fs::read(directory.join(format!("{role}.json")))?)?;
    let pair = bytes.as_slice().try_into()?;
    Ok(SigningKey::from_keypair_bytes(pair)?)
}

fn public(key: &SigningKey) -> String {
    hex_string(key.verifying_key().as_bytes())
}

fn envelope(body: Body, signer: &SigningKey, deployment: &Deployment) -> Result<SignedMessage> {
    let unsigned = UnsignedMessage {
        protocol: "0x463".into(),
        version: "0.1".into(),
        environment: deployment.environment(),
        issuer: public(signer),
        payload: body,
    };
    Ok(sign(unsigned, signer)?)
}

fn agreement(deployment: &Deployment, parties: &[SigningKey; 4], now: i64) -> Result<Agreement> {
    let [buyer, seller, verifier, issuer] = parties;
    let env = deployment.environment();
    let mut nonce = [0; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut nonce)?;
    let nonce = hex_string(&nonce);
    let order = hex_string(&order_id(&env, &public(buyer), &nonce)?);
    let asset = Asset {
        id: deployment.mint.clone(),
        decimals: "6".into(),
        kind: "spl-token".into(),
    };

    let template = parse_signed(
        include_bytes!("../../../spec/examples/worked-transaction/service_offer.json"),
        "sim:fixture",
    )?;
    let Body::ServiceOffer(mut offer) = template.unsigned().payload.clone() else {
        unreachable!()
    };
    offer.environments = vec![env.clone()];
    offer.expires_at = (now + 7200).to_string();
    let offer = envelope(Body::ServiceOffer(offer), seller, deployment)?;

    let template = parse_signed(
        include_bytes!("../../../spec/examples/worked-transaction/request.json"),
        "sim:fixture",
    )?;
    let Body::Request(mut r) = template.unsigned().payload.clone() else {
        unreachable!()
    };
    r.offer_id = hex_string(offer.id());
    r.buyer = public(buyer);
    r.seller = public(seller);
    r.nonce = nonce;
    r.order_id = order.clone();
    r.allowed_verifiers = vec![public(verifier)];
    r.expires_at = (now + 3600).to_string();
    r.asset = asset.clone();
    let settlement = deployment.settlement(&r.buyer, &r.seller, &r.nonce)?;
    let request = envelope(Body::Request(r), buyer, deployment)?;

    let template = parse_signed(
        include_bytes!("../../../spec/examples/worked-transaction/quote.json"),
        "sim:fixture",
    )?;
    let Body::Quote(mut q) = template.unsigned().payload.clone() else {
        unreachable!()
    };
    q.request_id = hex_string(request.id());
    q.order_id = order.clone();
    q.offer_id = hex_string(offer.id());
    q.buyer = public(buyer);
    q.seller = public(seller);
    q.verifier = public(verifier);
    q.receipt_issuer = public(issuer);
    q.asset = asset.clone();
    q.settlement = settlement;
    q.expires_at = (now + 3600).to_string();
    q.deadlines = Deadlines {
        fund_by: (now + 3600).to_string(),
        deliver_by: (now + 4200).to_string(),
        resolve_by: (now + 4800).to_string(),
    };
    let amount = q.amount.clone();
    let quote = envelope(Body::Quote(q), seller, deployment)?;
    let acceptance = envelope(
        Body::Accept(Accept {
            order_id: order,
            quote_id: hex_string(quote.id()),
            request_id: hex_string(request.id()),
            amount,
            asset,
            environment: env,
        }),
        buyer,
        deployment,
    )?;
    Ok(Agreement {
        request,
        quote,
        acceptance,
    })
}

fn signatures(
    rpc: &Rpc,
    expected: &Expected,
    message: &Prepared,
    parties: &[SigningKey; 4],
) -> Result<[String; 2]> {
    Ok([
        rpc.sign(expected, message, &parties[0].to_bytes())?,
        rpc.sign(expected, message, &parties[1].to_bytes())?,
    ])
}

fn wait(
    rpc: &Rpc,
    e: &Expected,
    tx: &SignedTx,
    target: ChainOutcome,
    timeout: Duration,
) -> Result<Resolution> {
    let start = Instant::now();
    loop {
        let observed = rpc.reconcile(e, &tx.signature, Some(tx))?;
        if observed.outcome == target {
            return Ok(observed);
        }
        assert!(
            matches!(
                observed.outcome,
                ChainOutcome::Pending | ChainOutcome::Unknown
            ),
            "unexpected {observed:?} for {}",
            tx.signature
        );
        assert!(
            start.elapsed() < timeout,
            "timeout waiting for {target:?}; last observation {observed:?}"
        );
        thread::sleep(Duration::from_secs(1));
    }
}

#[test]
#[ignore = "requires an isolated buyer fixture and a running private validator"]
fn expired_fund_and_rpc_recovery_deposit_once() -> Result<()> {
    let directory = PathBuf::from(env::var("PHASE_D_ADAPTER_FIXTURE")?);
    let deployment: Deployment =
        serde_json::from_slice(&fs::read(directory.join("deployment.json"))?)?;
    let parties = [
        key(&directory, "buyer")?,
        key(&directory, "seller")?,
        key(&directory, "verifier")?,
        key(&directory, "coordinator")?,
    ];
    let payer = bs58::encode(parties[0].verifying_key().as_bytes()).into_string();
    let rpc = Rpc::new(&deployment)?;
    rpc.validate_deployment()?;
    let agreement = agreement(&deployment, &parties, rpc.clock()?)?;
    let expected = |deployment| {
        Expected::new(
            deployment,
            &agreement,
            ChainAction::Fund,
            None,
            None,
            &payer,
        )
    };
    let e = expected(&deployment)?;
    let balances = || -> Result<[u64; 3]> {
        Ok([
            rpc.balance(&e.review().buyer_destination)?,
            rpc.balance(&e.vault())?,
            rpc.balance(&e.review().seller_destination)?,
        ])
    };
    assert!(rpc.inspect(&e)?.is_none());
    assert_eq!(balances()?, [5_000_000, 0, 0]);

    let first = rpc.prepare(&e)?;
    let old_signatures = signatures(&rpc, &e, &first, &parties)?;
    let unused = e.assemble(&first, &old_signatures)?;
    wait(
        &rpc,
        &e,
        &unused,
        ChainOutcome::Expired,
        Duration::from_secs(240),
    )?;
    for signer in &parties[..2] {
        let failure = rpc.sign(&e, &first, &signer.to_bytes()).unwrap_err();
        assert!(
            failure.to_string().contains("blockhash expired"),
            "{failure}"
        );
    }
    assert!(rpc.inspect(&e)?.is_none());
    assert_eq!(balances()?, [5_000_000, 0, 0]);

    let second = rpc.prepare(&e)?;
    assert_ne!(first.message, second.message);
    assert!(second.context_slot > first.context_slot);
    let renewed = signatures(&rpc, &e, &second, &parties)?;
    for i in 0..2 {
        assert_ne!(renewed[i], old_signatures[i]);
        let mut mixed = renewed.clone();
        mixed[i] = old_signatures[i].clone();
        assert!(e.assemble(&second, &mixed).is_err());
    }
    let tx = e.assemble(&second, &renewed)?;

    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let address = listener.local_addr()?;
    drop(listener);
    let disconnected = Deployment {
        rpc_url: format!("http://{address}"),
        ..deployment.clone()
    };
    let unreachable = Rpc::new(&disconnected)?;
    assert!(unreachable.broadcast(&tx).unwrap_err().is_unavailable());
    assert!(unreachable
        .reconcile(&expected(&disconnected)?, &tx.signature, Some(&tx))
        .unwrap_err()
        .is_unavailable());
    thread::sleep(Duration::from_secs(1));
    assert_eq!(balances()?, [5_000_000, 0, 0]);
    rpc.broadcast(&tx)?;
    rpc.broadcast(&tx)?;
    let finalized = wait(
        &rpc,
        &e,
        &tx,
        ChainOutcome::FinalizedSuccess,
        Duration::from_secs(100),
    )?;
    assert_eq!(rpc.reconcile(&e, &tx.signature, Some(&tx))?, finalized);
    assert_eq!(balances()?, [3_000_000, 2_000_000, 0]);
    assert_eq!(
        rpc.inspect(&e)?.ok_or("missing funded order")?.state,
        OrderState::Funded
    );
    let history: Value = ureq::post(&deployment.rpc_url)
        .send_json(json!({
            "jsonrpc":"2.0", "id":1, "method":"getSignaturesForAddress",
            "params":[e.review().escrow_account,{"commitment":"finalized"}]
        }))?
        .into_json()?;
    let rows = history["result"]
        .as_array()
        .ok_or("missing finalized escrow history")?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["err"], Value::Null);
    assert_eq!(rows[0]["signature"], tx.signature);
    assert_eq!(rows[0]["slot"].as_u64(), finalized.slot);
    println!(
        "ADAPTER_RECOVERY {}",
        json!({
            "cases":["T-REC-03","T-REC-04"], "order_id":e.review().order_id, "escrow":e.review().escrow_account,
            "expired_signature":unused.signature, "expired_last_valid_height":first.last_valid_block_height,
            "expired_context_slot":first.context_slot, "renewed_context_slot":second.context_slot,
            "funding_signature":tx.signature, "finalized_slot":finalized.slot, "finalized_escrow_transactions":rows.len(),
            "buyer_delta":-2_000_000, "vault_delta":2_000_000, "seller_delta":0
        })
    );
    Ok(())
}
