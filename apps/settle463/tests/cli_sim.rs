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
        assert_eq!(state["held"], "0");
        if scenario == "pass" {
            assert!(String::from_utf8_lossy(&out.stderr).contains("buyer_download length=52 sha256=37ba5c2e570e4e256af7049d0d9c27f70b50f5de4210101d7c0bd2b497db7587"));
            let mut broken = value.clone();
            let sig = broken["receipt"]["signature"].as_str().unwrap();
            let prefix = if sig.starts_with('0') { "1" } else { "0" };
            broken["receipt"]["signature"] = format!("{prefix}{}", &sig[1..]).into();
            let damaged = session.0.join("damaged.json");
            fs::write(&damaged, serde_json::to_vec(&broken).unwrap()).unwrap();
            assert_eq!(session.verify(&damaged, &[]).status.code(), Some(2));
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
