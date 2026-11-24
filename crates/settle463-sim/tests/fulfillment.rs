use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};
use settle463_core::{
    ident::{hex, hex_string},
    parse_signed,
    receipt::{build_sim_bundle, make_sim_receipt, verify_sim_bundle, ReceiptError, ReceiptTrust},
    settlement::{
        Action, ActionReview, Agreement, ChainOutcome, FundSignatures, FundingError, OrderState,
        OrderView, SettlementBackend,
    },
    sign,
    wire::Body,
    SignedMessage, UnsignedMessage,
};
use settle463_sim::{Simulator, ASSET_ID};
use std::sync::Barrier;

const ENV: &str = "sim:fixture";
const START: i64 = 1_700_000_000;
const DELIVER: i64 = START + 7200;
const RESOLVE: i64 = START + 10800;
const GOLDEN: &str = include_str!("../../../fixtures/golden/positive-signing-vector.json");
const WORKED: &[u8] = include_bytes!("../../../spec/examples/worked-transaction/bundle.json");

fn key(role: &str) -> SigningKey {
    let g: Value = serde_json::from_str(GOLDEN).unwrap();
    SigningKey::from_bytes(&hex(g["keys"][role]["seed_hex"].as_str().unwrap()).unwrap())
}

struct Order {
    sim: Simulator,
    agreement: Agreement,
    order: String,
    delivery: SignedMessage,
    verdict: SignedMessage,
}
impl Order {
    fn funded() -> Self {
        let b: Value = serde_json::from_slice(WORKED).unwrap();
        let load = |k: &str| parse_signed(&serde_json::to_vec(&b[k]).unwrap(), ENV).unwrap();
        let agreement = Agreement {
            request: load("request"),
            quote: load("quote"),
            acceptance: load("acceptance"),
        };
        let (_, q) = agreement.terms(ENV).unwrap();
        let order = q.order_id.clone();
        let sim =
            Simulator::with_environment(ENV, START, [(format!("sim:{}", q.buyer), 5_000_000)])
                .unwrap();
        let fund = sim.prepare_fund(agreement.clone()).unwrap();
        let bytes = fund.review.signing_bytes().unwrap();
        let signatures = FundSignatures {
            buyer: key("buyer").sign(&bytes).to_bytes(),
            seller: key("seller").sign(&bytes).to_bytes(),
        };
        sim.submit_fund(&fund, &signatures).unwrap();
        Self {
            sim,
            agreement,
            order,
            delivery: load("delivery"),
            verdict: load("verdict"),
        }
    }
    fn view(&self) -> OrderView {
        self.sim.inspect(&self.order).unwrap().unwrap()
    }
    fn prepared(&self, action: Action) -> (ActionReview, Option<[u8; 64]>) {
        let (evidence, role) = match action {
            Action::CommitDelivery => (Some(&self.delivery), Some("seller")),
            Action::Release => (Some(&self.verdict), Some("verifier")),
            Action::Refund => (None, None),
        };
        let review = self
            .sim
            .prepare_action(&self.order, action, evidence)
            .unwrap();
        let sig = role.map(|s| key(s).sign(&review.signing_bytes().unwrap()).to_bytes());
        (review, sig)
    }
    fn deliver(&self) {
        let (r, s) = self.prepared(Action::CommitDelivery);
        for _ in 0..2 {
            self.sim
                .submit_action(&r, Some(&self.delivery), s.as_ref())
                .unwrap();
        }
        assert_eq!(
            self.view().delivery_id.as_deref(),
            Some(hex_string(self.delivery.id()).as_str())
        );
    }
    fn refund(&self) {
        let (r, s) = self.prepared(Action::Refund);
        for _ in 0..2 {
            self.sim.submit_action(&r, None, s.as_ref()).unwrap();
        }
    }
    fn assert_money(&self, expected: OrderState) {
        let o = self.view();
        assert_eq!(o.state, expected);
        assert_eq!(o.paid + o.held + o.refunded, 2_000_000);
        assert_eq!(
            self.sim
                .balance(&o.review.escrow_account, ASSET_ID)
                .unwrap(),
            o.held
        );
        assert_eq!(
            self.sim
                .balance(&o.review.seller_destination, ASSET_ID)
                .unwrap(),
            o.paid
        );
        assert_eq!(
            self.sim
                .balance(&o.review.buyer_destination, ASSET_ID)
                .unwrap(),
            3_000_000 + o.refunded
        );
        assert_eq!(
            self.sim.transfers().unwrap().len(),
            if o.held == 0 { 2 } else { 1 }
        );
    }
}

#[test]
fn delivery_boundary_and_before_delivery_refund() {
    for now in [DELIVER - 1, DELIVER, DELIVER + 1] {
        let o = Order::funded();
        let (delivery, sig) = o.prepared(Action::CommitDelivery);
        o.sim.set_clock(now).unwrap();
        let submitted = o
            .sim
            .submit_action(&delivery, Some(&o.delivery), sig.as_ref());
        if now < DELIVER {
            assert!(submitted.is_ok());
            assert!(matches!(
                o.sim.prepare_action(&o.order, Action::Refund, None),
                Err(FundingError::TooEarly)
            ));
            o.assert_money(OrderState::Delivered);
        } else {
            assert!(matches!(submitted, Err(FundingError::DeliveryDeadline)));
            o.refund();
            o.assert_money(OrderState::Refunded);
            assert!(o.view().delivery_id.is_none());
        }
    }
}

#[test]
fn timely_pass_and_refund_races_have_one_terminal_transfer() {
    for now in [RESOLVE - 1, RESOLVE, RESOLVE + 1] {
        let o = Order::funded();
        o.deliver();
        let (release, sig) = o.prepared(Action::Release);
        let refund = ActionReview::for_order(Action::Refund, &o.view(), None).unwrap();
        o.sim.set_clock(now).unwrap();
        let gate = Barrier::new(2);
        let (paid, refunded) = std::thread::scope(|s| {
            let a = s.spawn(|| {
                gate.wait();
                o.sim
                    .submit_action(&release, Some(&o.verdict), sig.as_ref())
            });
            let b = s.spawn(|| {
                gate.wait();
                o.sim.submit_action(&refund, None, None)
            });
            (a.join().unwrap(), b.join().unwrap())
        });
        if now < RESOLVE {
            assert_eq!(paid.unwrap().outcome, ChainOutcome::FinalizedSuccess);
            assert!(refunded.is_err());
            o.sim
                .submit_action(&release, Some(&o.verdict), sig.as_ref())
                .unwrap();
            o.assert_money(OrderState::Paid);
        } else {
            assert!(paid.is_err());
            assert!(refunded.is_ok());
            o.refund();
            o.assert_money(OrderState::Refunded);
        }
        let (_, q) = o.agreement.terms(ENV).unwrap();
        assert_eq!(o.view().terminal_clock, Some(now));
        assert_eq!(
            o.view().terminal_locator,
            Some(format!("sim:{}:3", q.order_id))
        );
        assert!(matches!(
            o.sim.prepare_fund(o.agreement.clone()),
            Err(FundingError::AlreadyFunded)
        ));
    }
}

#[test]
fn actions_require_correct_evidence_and_separate_authority() {
    let o = Order::funded();
    let (commit, _) = o.prepared(Action::CommitDelivery);
    let sig = key("buyer")
        .sign(&commit.signing_bytes().unwrap())
        .to_bytes();
    assert!(o
        .sim
        .submit_action(&commit, Some(&o.delivery), Some(&sig))
        .is_err());
    assert!(o
        .sim
        .submit_action(&commit, Some(&o.delivery), Some(o.delivery.signature()))
        .is_err());
    o.assert_money(OrderState::Funded);
    o.deliver();
    let mut unsigned = o.verdict.unsigned().clone();
    if let Body::Verdict(v) = &mut unsigned.payload {
        v.decision = "FAIL".into();
        v.reason_codes = vec!["FIELD_TYPE".into()];
    }
    let negative = sign(unsigned, &key("verifier")).unwrap();
    assert!(o
        .sim
        .prepare_action(&o.order, Action::Release, Some(&negative))
        .is_err());
    assert!(o
        .sim
        .prepare_action(&o.order, Action::Refund, None)
        .is_err());
    let (release, sig) = o.prepared(Action::Release);
    assert!(o
        .sim
        .submit_action(&release, Some(&negative), sig.as_ref())
        .is_err());
    o.assert_money(OrderState::Delivered);
    o.sim.set_clock(RESOLVE).unwrap();
    o.refund();
    o.assert_money(OrderState::Refunded);
}

#[test]
fn pending_release_expires_at_execution_and_a_refund_can_finalize() {
    let o = Order::funded();
    o.deliver();
    o.sim.set_clock(RESOLVE - 1).unwrap();
    o.sim.set_manual_finalization(true).unwrap();
    let (r, s) = o.prepared(Action::Release);
    let pending = o
        .sim
        .submit_action(&r, Some(&o.verdict), s.as_ref())
        .unwrap();
    assert_eq!(pending.outcome, ChainOutcome::Pending);
    o.sim.set_clock(RESOLVE).unwrap();
    let (r, _) = o.prepared(Action::Refund);
    let refund = o.sim.submit_action(&r, None, None).unwrap();
    assert_eq!(
        o.sim.finalize(&pending.intent_id).unwrap(),
        ChainOutcome::Expired
    );
    assert_eq!(
        o.sim.finalize(&refund.intent_id).unwrap(),
        ChainOutcome::FinalizedSuccess
    );
    assert_eq!(
        o.sim.reconcile(&pending.intent_id).unwrap(),
        ChainOutcome::Expired
    );
    o.assert_money(OrderState::Refunded);
}

#[test]
fn four_terminal_receipts_bind_conditional_messages_and_detached_trust() {
    for scenario in ["pass", "bad-artifact", "no-delivery", "verifier-offline"] {
        let o = Order::funded();
        let delivered = scenario != "no-delivery";
        if delivered {
            o.deliver();
        }
        let verdict = match scenario {
            "pass" => {
                let (r, s) = o.prepared(Action::Release);
                o.sim
                    .submit_action(&r, Some(&o.verdict), s.as_ref())
                    .unwrap();
                Some(o.verdict.clone())
            }
            "bad-artifact" => {
                let mut unsigned = o.verdict.unsigned().clone();
                if let Body::Verdict(v) = &mut unsigned.payload {
                    v.decision = "FAIL".into();
                    v.reason_codes = vec!["SCHEMA_MISMATCH".into()];
                }
                Some(sign(unsigned, &key("verifier")).unwrap())
            }
            _ => None,
        };
        if scenario != "pass" {
            o.sim
                .set_clock(if delivered { RESOLVE } else { DELIVER })
                .unwrap();
            o.refund();
        }
        let view = o.view();
        let receipt = make_sim_receipt(
            &view,
            verdict.as_ref().filter(|_| scenario == "bad-artifact"),
        )
        .unwrap();
        let trust = ReceiptTrust {
            trusted_issuer: receipt.receipt_issuer.clone(),
            trusted_order: receipt.order_id.clone(),
            environment: ENV.into(),
        };
        let signed = sign(
            UnsignedMessage {
                protocol: "0x463".into(),
                version: "0.1".into(),
                environment: ENV.into(),
                issuer: trust.trusted_issuer.clone(),
                payload: Body::Receipt(Box::new(receipt.clone())),
            },
            &key("coordinator"),
        )
        .unwrap();
        let bytes = build_sim_bundle(
            &o.agreement,
            delivered.then_some(&o.delivery),
            verdict.as_ref(),
            &signed,
            &[],
            &view,
        )
        .unwrap();
        assert_eq!(verify_sim_bundle(&bytes, &trust), Ok(receipt));
        let mut damaged: Value = serde_json::from_slice(&bytes).unwrap();
        damaged["ledger"]["principal"] = json!("2000001");
        assert_eq!(
            verify_sim_bundle(&serde_json::to_vec(&damaged).unwrap(), &trust),
            Err(ReceiptError::InvalidEvidence)
        );
        damaged = serde_json::from_slice(&bytes).unwrap();
        damaged["receipt"]["body"]["amount"] = json!("0");
        assert_eq!(
            verify_sim_bundle(&serde_json::to_vec(&damaged).unwrap(), &trust),
            Err(ReceiptError::InvalidEvidence)
        );
        let wrong = ReceiptTrust {
            trusted_issuer: "11".repeat(32),
            ..trust
        };
        assert_eq!(
            verify_sim_bundle(&bytes, &wrong),
            Err(ReceiptError::Untrusted)
        );
    }
}

#[test]
fn published_bundle_verifies_and_injected_fields_fail() {
    let mut doc: Value = serde_json::from_slice(WORKED).unwrap();
    let trust = ReceiptTrust {
        environment: ENV.into(),
        trusted_issuer: doc["receipt"]["issuer"].as_str().unwrap().into(),
        trusted_order: doc["order_id"].as_str().unwrap().into(),
    };
    assert_eq!(verify_sim_bundle(WORKED, &trust).unwrap().outcome, "paid");
    doc["observations"] = json!([doc["verdict"].clone()]);
    assert_eq!(
        verify_sim_bundle(&serde_json::to_vec(&doc).unwrap(), &trust),
        Err(ReceiptError::InvalidEvidence)
    );
    doc["observations"] = json!([]);
    doc["trust_file"] = json!({"trusted_order":trust.trusted_order});
    assert_eq!(
        verify_sim_bundle(&serde_json::to_vec(&doc).unwrap(), &trust),
        Err(ReceiptError::InvalidEvidence)
    );
}
