use serde_json::Value;
use settle463_core::ident::hex_string;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Session(PathBuf);
impl Session {
    fn new() -> Self {
        let mut nonce = [0u8; 16];
        getrandom::getrandom(&mut nonce).unwrap();
        Self(std::env::temp_dir().join(format!("settle463-cli-test-{}", hex_string(&nonce))))
    }
    fn demo(&self, scenario: &str) -> Command {
        let mut cmd = cli();
        cmd.args([
            "demo",
            "--backend",
            "sim",
            "--scenario",
            scenario,
            "--output-dir",
        ])
        .arg(&self.0);
        cmd
    }
    fn inspect(&self, order: &str) -> Value {
        let out = cli()
            .args(["order", "inspect", order, "--json", "--trust-file"])
            .arg(self.0.join("trust.json"))
            .output()
            .unwrap();
        serde_json::from_slice(ok(&out)).unwrap()
    }
    fn verify(&self, bundle: &Path, flags: &[&str]) -> Output {
        cli()
            .args(["receipt", "verify"])
            .arg(bundle)
            .args(["--offline", "--trust-file"])
            .arg(self.0.join("trust.json"))
            .args(flags)
            .output()
            .unwrap()
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_settle463"))
}
fn ok(out: &Output) -> &[u8] {
    assert!(
        out.status.success(),
        "CLI failed: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    &out.stdout
}
#[test]
fn scenarios_and_fresh_process_receipts() {
    let cases = [
        ("pass", "paid", "-2000000", "2000000"),
        ("bad-artifact", "refunded_timeout_after_delivery", "0", "0"),
        ("no-delivery", "refunded_timeout_before_delivery", "0", "0"),
        (
            "verifier-offline",
            "refunded_timeout_after_delivery",
            "0",
            "0",
        ),
    ];
    for (scenario, outcome, buyer, seller) in cases {
        let session = Session::new();
        let out = session.demo(scenario).output().unwrap();
        let stdout = std::str::from_utf8(ok(&out)).unwrap();
        let lines: Vec<_> = stdout
            .lines()
            .map(|line| line.split_once('=').unwrap())
            .collect();
        let keys: Vec<_> = lines.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            [
                "order_id",
                "backend",
                "outcome",
                "asset",
                "principal",
                "delta_buyer",
                "delta_seller",
                "delta_escrow",
                "receipt"
            ]
        );
        assert_eq!(
            &lines[1..8],
            &[
                ("backend", "sim"),
                ("outcome", outcome),
                ("asset", "test-six-decimal"),
                ("principal", "2000000"),
                ("delta_buyer", buyer),
                ("delta_seller", seller),
                ("delta_escrow", "0")
            ]
        );
        let bundle = Path::new(lines[8].1);
        let raw = fs::read(bundle).unwrap();
        let value: Value = serde_json::from_slice(&raw).unwrap();
        let terms = &value["quote"]["body"];
        let roles: std::collections::BTreeSet<_> =
            ["buyer", "seller", "verifier", "receipt_issuer"]
                .map(|name| terms[name].as_str().unwrap())
                .into_iter()
                .collect();
        assert_eq!(roles.len(), 4);
        assert_eq!(value.get("delivery").is_some(), scenario != "no-delivery");
        assert_eq!(
            value.get("verdict").is_some(),
            matches!(scenario, "pass" | "bad-artifact")
        );
        assert_eq!(ok(&session.verify(bundle, &[])), b"evidence_signed\n");
        let state = session.inspect(lines[0].1);
        assert_eq!(
            state["state"],
            if scenario == "pass" {
                "Paid"
            } else {
                "Refunded"
            }
        );
        assert_eq!(state["order_id"], lines[0].1);
        assert_eq!(state["held"], "0");
        if scenario == "bad-artifact" {
            let v = &value["verdict"]["body"];
            assert_eq!(v["decision"], "FAIL");
            assert_ne!(
                v["observed_digest"],
                terms["acceptance_policy"]["expected_digest"]
            );
        }
        if scenario == "pass" {
            assert!(String::from_utf8_lossy(&out.stderr).contains("buyer_download length=52 sha256=37ba5c2e570e4e256af7049d0d9c27f70b50f5de4210101d7c0bd2b497db7587"));
            let mut broken = value.clone();
            let sig = broken["receipt"]["signature"].as_str().unwrap();
            let prefix = if sig.starts_with('0') { "1" } else { "0" };
            broken["receipt"]["signature"] = format!("{prefix}{}", &sig[1..]).into();
            let damaged = session.0.join("damaged.json");
            fs::write(&damaged, serde_json::to_vec(&broken).unwrap()).unwrap();
            let rejected = session.verify(&damaged, &[]);
            assert_eq!(rejected.status.code(), Some(2));
            assert_eq!(rejected.stdout, b"invalid_evidence\n");
            assert_eq!(
                session
                    .verify(bundle, &["--trusted-issuer", &"00".repeat(32)])
                    .status
                    .code(),
                Some(3)
            );
            assert_eq!(
                cli()
                    .args(["receipt", "verify"])
                    .arg(bundle)
                    .arg("--offline")
                    .output()
                    .unwrap()
                    .status
                    .code(),
                Some(1)
            );
        }
    }
}

#[test]
#[cfg(debug_assertions)]
fn restart_after_funding_intent_commit_debits_exactly_once() {
    let session = Session::new();
    let failed = session
        .demo("pass")
        .env("SETTLE463_TEST_STOP", "before_broadcast")
        .output()
        .unwrap();
    assert!(!failed.status.success());
    let trust: Value =
        serde_json::from_slice(&fs::read(session.0.join("trust.json")).unwrap()).unwrap();
    let order = trust["trusted_order"].as_str().unwrap();
    for _ in 0..2 {
        let view = session.inspect(order);
        assert_eq!(view["state"], "Funded");
        assert_eq!(view["held"], "2000000");
        assert_eq!(view["paid"], "0");
        assert_eq!(view["refunded"], "0");
        assert_eq!(view["buyer_balance"], "3000000");
        assert_eq!(view["seller_balance"], "0");
    }
}

struct Peer {
    state: Session,
    child: std::process::Child,
    url: String,
}
impl Peer {
    fn start(buyer: &str, issuer_seed: &[u8; 32]) -> Self {
        use std::io::{BufRead, BufReader};
        let state = Session::new();
        fs::create_dir(&state.0).unwrap();
        let key_file = state.0.join("issuer.key");
        fs::write(&key_file, hex_string(issuer_seed)).unwrap();
        let mut admin_token = [0; 32];
        getrandom::getrandom(&mut admin_token).unwrap();
        let config = settle463d::Config {
            database: state.0.join("db.sqlite"),
            artifact_root: state.0.join("artifacts"),
            issuer_seed_file: key_file,
            environment: "sim:fixture".into(),
            buyer: buyer.to_owned(),
            opening_balance: 5_000_000,
            clock: 1_700_000_000,
            admin_token: hex_string(&admin_token),
        };
        let file = state.0.join("coordinator.json");
        fs::write(&file, serde_json::to_vec(&config).unwrap()).unwrap();
        let mut child = cli()
            .args(["__serve", "--config"])
            .arg(file)
            .args(["--listen", "127.0.0.1:0"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        let addr = ready
            .trim()
            .strip_prefix("LISTEN=")
            .expect("daemon should bind");
        Self {
            state,
            child,
            url: format!("http://{addr}"),
        }
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn direct_peer_transfers_signed_quote_to_second_coordinator_without_discovery() {
    use settle463_core::{ident::hex, parse_signed, wire::Body};
    use settle463_sdk::{DirectPeer, HttpCoordinator, Identity, MessageTransport, Role};
    use std::sync::Arc;

    let vector: Value = serde_json::from_slice(include_bytes!(
        "../../../fixtures/golden/positive-signing-vector.json"
    ))
    .unwrap();
    let keys = &vector["keys"];
    let buyer_public = keys["buyer"]["public_key_hex"].as_str().unwrap();
    let source_seed = hex(keys["coordinator"]["seed_hex"].as_str().unwrap()).unwrap();
    let mut destination_seed = [0; 32];
    getrandom::getrandom(&mut destination_seed).unwrap();
    let source = Peer::start(buyer_public, &source_seed);
    let destination = Peer::start(buyer_public, &destination_seed);
    assert_ne!(source.url, destination.url);
    assert_ne!(source.state.0, destination.state.0);
    let connect = |url: &str, role, name: &str| {
        let id = Identity::from_seed(&hex(keys[name]["seed_hex"].as_str().unwrap()).unwrap());
        Arc::new(HttpCoordinator::new(url, "sim:fixture", id, role).unwrap())
    };
    let seller = connect(&source.url, Role::Seller, "seller");
    let direct = DirectPeer(connect(&destination.url, Role::Seller, "seller"));
    let buyer = connect(&destination.url, Role::Buyer, "buyer");
    let offer = parse_signed(
        include_bytes!("../../../spec/examples/worked-transaction/service_offer.json"),
        "sim:fixture",
    )
    .unwrap();
    let request = parse_signed(
        include_bytes!("../../../spec/examples/worked-transaction/request.json"),
        "sim:fixture",
    )
    .unwrap();
    let pretty_quote = include_bytes!("../../../spec/examples/worked-transaction/quote.json");
    let quote = parse_signed(pretty_quote, "sim:fixture").unwrap();
    assert_ne!(pretty_quote.as_slice(), quote.to_wire().unwrap());
    let Body::Quote(q) = &quote.unsigned().payload else {
        panic!("expected fixture quote")
    };
    assert_eq!(
        q.receipt_issuer,
        Identity::from_seed(&source_seed).public_hex()
    );
    assert_ne!(
        q.receipt_issuer,
        Identity::from_seed(&destination_seed).public_hex()
    );
    let id = hex_string(quote.id());
    for m in [&offer, &request, &quote] {
        seller.publish(m.clone()).unwrap();
    }
    for m in [&offer, &request] {
        direct.publish(m.clone()).unwrap();
    }
    assert!(buyer.fetch_for_order(&id, &q.order_id).is_err());
    let signed = seller.fetch_for_order(&id, &q.order_id).unwrap();
    assert_eq!(signed.wire_bytes(), pretty_quote);
    direct.publish(signed).unwrap();
    let received = buyer.fetch(&id).unwrap();
    assert_eq!(received.wire_bytes(), pretty_quote);
    assert_eq!(received.id(), quote.id());
}
