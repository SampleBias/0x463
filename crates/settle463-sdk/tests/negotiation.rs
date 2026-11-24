use ed25519_dalek::SigningKey;
use serde_json::Value;
use settle463_core::{
    ident::{base58, hex, hex_string},
    settlement::{Agreement, ChainOutcome, FundSignatures, FundingError, OrderState, PreparedFund},
    sign,
    wire::{ArtifactStore, Asset, Body, Deadlines, Fees, Offer, Policy},
    SignedMessage,
};
use settle463_sdk::{
    BuyerClient, BuyerPolicy, Error, Identity, MemoryTransport, MessageTransport, QuoteTerms,
    RequestTerms, SellerClient, SellerPolicy, SettlementBackend,
};
use settle463_sim::{Simulator, ASSET_ID};
use std::{
    collections::BTreeSet,
    sync::{Arc, Barrier},
    thread,
};

const START: i64 = 1_700_000_000;
const FUND: i64 = START + 3600;
const AMOUNT: u64 = 2_000_000;
const OPENING: u64 = 5_000_000;
const GOLDEN: &str = include_str!("../../../fixtures/golden/positive-signing-vector.json");

struct Market {
    backend: Arc<Simulator>,
    store: Arc<MemoryTransport>,
    buyer: BuyerClient,
    seller: SellerClient,
    buyer_key: String,
    seller_key: String,
    seller_seed: [u8; 32],
    verifier: String,
    receipt: String,
    offer: SignedMessage,
}
fn only(key: &str) -> BTreeSet<String> {
    BTreeSet::from([key.to_owned()])
}
fn buyer_policy(env: &str, seller: &str, verifier: &str) -> BuyerPolicy {
    BuyerPolicy {
        trusted_publishers: only(seller),
        max_principal: 2_500_000,
        allowed_verifiers: only(verifier),
        allowed_services: only("sample-json-report"),
        allowed_assets: only(ASSET_ID),
        allowed_environments: only(env),
        max_deliver_window_seconds: 86_400,
        max_resolve_window_seconds: 86_400,
    }
}
fn offer(env: &str) -> Offer {
    let g: Value = serde_json::from_str(GOLDEN).unwrap();
    let mut o: Offer =
        serde_json::from_value(g["signing"]["unsigned_object"]["body"].clone()).unwrap();
    o.environments = vec![env.into()];
    o
}
impl Market {
    fn new() -> Self {
        let buyer = Identity::generate().unwrap();
        let mut seller_seed = [0; 32];
        getrandom::getrandom(&mut seller_seed).unwrap();
        let seller = Identity::from_seed(&seller_seed);
        let buyer_key = buyer.public_hex();
        let seller_key = seller.public_hex();
        assert_eq!(
            base58::<32>(&buyer.public_base58()).unwrap(),
            hex::<32>(&buyer_key).unwrap()
        );
        let verifier = Identity::generate().unwrap().public_hex();
        let receipt = Identity::generate().unwrap().public_hex();
        let backend =
            Arc::new(Simulator::new(START, [(format!("sim:{buyer_key}"), OPENING)]).unwrap());
        let env = backend.environment();
        let store = Arc::new(MemoryTransport::new(env).unwrap());
        let buyer = BuyerClient::new(
            buyer,
            buyer_policy(env, &seller_key, &verifier),
            store.clone(),
            backend.clone(),
        );
        let seller = SellerClient::new(
            seller,
            SellerPolicy {
                min_principal: 500_000,
                allowed_verifiers: only(&verifier),
                allowed_environments: only(env),
            },
            store.clone(),
            backend.clone(),
        );
        let offer = seller.publish_offer(offer(env)).unwrap();
        Self {
            backend,
            store,
            buyer,
            seller,
            buyer_key,
            seller_key,
            seller_seed,
            verifier,
            receipt,
            offer,
        }
    }
    fn request_terms(&self) -> RequestTerms {
        RequestTerms {
            asset: Asset {
                kind: "sim".into(),
                id: ASSET_ID.into(),
                decimals: "6".into(),
            },
            max_principal: 2_500_000,
            allowed_verifiers: vec![self.verifier.clone()],
            max_bytes: 1_048_576,
            expires_at: FUND,
        }
    }
    fn quote_terms(&self, amount: u64) -> QuoteTerms {
        let g: Value = serde_json::from_str(GOLDEN).unwrap();
        let policy: Policy = serde_json::from_value(g["policy"]["object"].clone()).unwrap();
        QuoteTerms {
            amount,
            verifier: self.verifier.clone(),
            receipt_issuer: self.receipt.clone(),
            policy,
            deadlines: Deadlines {
                fund_by: FUND.to_string(),
                deliver_by: (START + 7200).to_string(),
                resolve_by: (START + 10800).to_string(),
            },
            expires_at: FUND,
            fees: Fees {
                protocol_atomic: "0".into(),
                network_fee_payer: "buyer".into(),
            },
            artifact_store: ArtifactStore {
                store_id: "test-store".into(),
                read_access: "order-parties".into(),
                retention_seconds: "86400".into(),
            },
        }
    }
    fn request(&self) -> SignedMessage {
        self.buyer
            .create_request(&self.offer, self.request_terms())
            .unwrap()
    }
    fn quote(&self, request: &SignedMessage, amount: u64) -> SignedMessage {
        self.seller
            .quote(request, self.quote_terms(amount))
            .unwrap()
    }
    fn prepare(&self, request: SignedMessage, quote: SignedMessage) -> PreparedFund {
        let acceptance = self.buyer.accept(&request, &quote).unwrap();
        self.backend
            .prepare_fund(Agreement {
                request,
                quote,
                acceptance,
            })
            .unwrap()
    }
    fn fund(&self) -> PreparedFund {
        let r = self.request();
        let q = self.quote(&r, AMOUNT);
        self.prepare(r, q)
    }
    fn signatures(&self, f: &PreparedFund) -> FundSignatures {
        FundSignatures {
            buyer: self.buyer.sign_fund(f).unwrap(),
            seller: self.seller.sign_fund(f).unwrap(),
        }
    }
    fn assert_balances(&self, funded: u64) {
        assert_eq!(
            self.backend
                .balance(&format!("sim:{}", self.buyer_key), ASSET_ID)
                .unwrap(),
            OPENING - funded
        );
        assert_eq!(
            self.backend
                .balance(&format!("sim:{}", self.seller_key), ASSET_ID)
                .unwrap(),
            0
        );
        let transfers = self.backend.transfers().unwrap();
        assert_eq!(transfers.iter().map(|t| t.amount).sum::<u64>(), funded);
        for transfer in transfers {
            let view = self.buyer.inspect(&transfer.order_id).unwrap().unwrap();
            assert_eq!(
                (view.state, view.held, view.paid, view.refunded),
                (OrderState::Funded, transfer.amount, 0, 0)
            );
            assert_eq!(
                self.backend
                    .balance(&view.review.escrow_account, ASSET_ID)
                    .unwrap(),
                view.held
            );
            assert_eq!(view.held + view.paid + view.refunded, transfer.amount);
        }
    }
}

#[test]
fn negotiated_fund_and_replay() {
    let m = Market::new();
    let offers = m.buyer.discover().unwrap();
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].id(), m.offer.id());
    let r = m.request();
    let q = m.quote(&r, AMOUNT);
    assert!(m.store.accepted(&r).unwrap().is_none());
    m.assert_balances(0);
    let f = m.prepare(r, q);
    let signatures = m.signatures(&f);
    let result = m.backend.submit_fund(&f, &signatures).unwrap();
    assert_eq!(result.outcome, ChainOutcome::FinalizedSuccess);
    assert_eq!(
        m.backend.reconcile(&result.intent_id).unwrap(),
        result.outcome
    );
    m.assert_balances(AMOUNT);
    let view = m.buyer.inspect(&f.review.order_id).unwrap().unwrap();
    println!("T-NEG-01 environment={} order={} buyer={} seller={} amount={} held={} paid={} refunded={} buyer_balance={} seller_balance={}",
        m.backend.environment(),f.review.order_id,m.buyer_key,m.seller_key,f.review.principal,view.held,view.paid,view.refunded,
        m.backend.balance(&f.review.buyer_destination,ASSET_ID).unwrap(),m.backend.balance(&f.review.seller_destination,ASSET_ID).unwrap());
    assert!(matches!(
        m.backend.submit_fund(&f, &signatures),
        Err(FundingError::AlreadyFunded)
    ));
    m.assert_balances(AMOUNT);
}

#[test]
fn only_one_concurrent_acceptance_and_one_deposit_per_nonce() {
    let m = Market::new();
    let nonce = [8; 32];
    let r = m
        .buyer
        .create_request_with_nonce(&m.offer, m.request_terms(), nonce)
        .unwrap();
    let quotes = [m.quote(&r, AMOUNT), m.quote(&r, AMOUNT + 1)];
    let gate = Barrier::new(2);
    let results = thread::scope(|s| {
        let handles: Vec<_> = quotes
            .iter()
            .map(|q| {
                s.spawn(|| {
                    gate.wait();
                    m.buyer.accept(&r, q)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        results
            .iter()
            .filter(|v| matches!(v, Err(Error::AcceptanceExists)))
            .count(),
        1
    );
    let (winner, a) = results
        .into_iter()
        .enumerate()
        .find_map(|(i, r)| r.ok().map(|a| (i, a)))
        .unwrap();
    assert_eq!(m.store.accepted(&r).unwrap().unwrap().id(), a.id());
    m.assert_balances(0);
    let mut different = m.request_terms();
    different.max_principal -= 1;
    let r2 = m
        .buyer
        .create_request_with_nonce(&m.offer, different, nonce)
        .unwrap();
    assert_ne!(r.id(), r2.id());
    let q2 = m.quote(&r2, AMOUNT);
    assert!(matches!(
        m.buyer.accept(&r2, &q2),
        Err(Error::AcceptanceExists)
    ));
    m.assert_balances(0);
    let f = m
        .backend
        .prepare_fund(Agreement {
            request: r,
            quote: quotes[winner].clone(),
            acceptance: a,
        })
        .unwrap();
    let sig = m.signatures(&f);
    m.backend.submit_fund(&f, &sig).unwrap();
    assert!(m.backend.submit_fund(&f, &sig).is_err());
    m.assert_balances(AMOUNT + winner as u64);
}

#[test]
fn fund_deadline_checked_at_accept_sign_submit_and_pending_finalization() {
    let m = Market::new();
    let good = m.fund();
    let late = m.fund();
    let pending = m.fund();
    let r = m.request();
    let q = m.quote(&r, AMOUNT);
    let late_sig = m.signatures(&late);
    let pending_sig = m.signatures(&pending);
    m.backend.set_clock(FUND - 1).unwrap();
    m.backend.submit_fund(&good, &m.signatures(&good)).unwrap();
    m.assert_balances(AMOUNT);
    m.backend.set_manual_finalization(true).unwrap();
    let submission = m.backend.submit_fund(&pending, &pending_sig).unwrap();
    assert_eq!(
        m.backend.reconcile(&submission.intent_id).unwrap(),
        ChainOutcome::Pending
    );
    m.assert_balances(AMOUNT);
    m.backend.set_clock(FUND).unwrap();
    assert!(matches!(m.buyer.accept(&r, &q), Err(Error::PolicyRejected)));
    assert!(m.store.accepted(&r).unwrap().is_none());
    assert!(m.buyer.sign_fund(&late).is_err());
    assert!(m.seller.sign_fund(&late).is_err());
    assert!(matches!(
        m.backend.submit_fund(&late, &late_sig),
        Err(FundingError::Deadline)
    ));
    assert!(m.backend.inspect(&late.review.order_id).unwrap().is_none());
    m.assert_balances(AMOUNT);
    assert_eq!(
        m.backend.finalize(&submission.intent_id).unwrap(),
        ChainOutcome::Expired
    );
    assert_eq!(
        m.backend.reconcile(&submission.intent_id).unwrap(),
        ChainOutcome::Expired
    );
    assert!(m
        .backend
        .inspect(&pending.review.order_id)
        .unwrap()
        .is_none());
    m.assert_balances(AMOUNT);
}

#[test]
fn one_critical_section_preserves_balances_when_pending_orders_compete() {
    let m = Market::new();
    m.backend.set_manual_finalization(true).unwrap();
    let mut pending = Vec::new();
    for _ in 0..3 {
        let f = m.fund();
        let sig = m.signatures(&f);
        let s = m.backend.submit_fund(&f, &sig).unwrap();
        assert!(matches!(
            m.backend.submit_fund(&f, &sig),
            Err(FundingError::Pending)
        ));
        m.assert_balances(0);
        pending.push(s.intent_id);
    }
    let gate = Barrier::new(3);
    let outcomes = thread::scope(|s| {
        let hs: Vec<_> = pending
            .iter()
            .map(|id| {
                s.spawn(|| {
                    gate.wait();
                    m.backend.finalize(id).unwrap()
                })
            })
            .collect();
        hs.into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        outcomes
            .iter()
            .filter(|&&o| o == ChainOutcome::FinalizedSuccess)
            .count(),
        2
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|&&o| o == ChainOutcome::FinalizedFailure)
            .count(),
        1
    );
    for (id, outcome) in pending.iter().zip(outcomes) {
        assert_eq!(m.backend.finalize(id).unwrap(), outcome);
    }
    m.assert_balances(AMOUNT * 2);
}

#[test]
fn changed_reviews_and_wrong_signatures_move_nothing() {
    let m = Market::new();
    let f = m.fund();
    let sig = m.signatures(&f);
    type Edit = fn(&mut settle463_core::settlement::Review);
    let edits: &[Edit] = &[
        |r| r.principal = "0".into(),
        |r| r.principal = "2000001".into(),
        |r| r.asset.id = "other-asset".into(),
        |r| r.deadlines.deliver_by = "1700007201".into(),
        |r| r.buyer = r.seller.clone(),
        |r| r.verifier = r.seller.clone(),
        |r| r.seller_destination = r.buyer_destination.clone(),
        |r| r.escrow_account = r.seller_destination.clone(),
        |r| r.environment = "sim:other".into(),
        |r| r.policy_hash = "00".repeat(32),
        |r| r.acceptance_id = r.quote_id.clone(),
    ];
    for edit in edits {
        let mut changed = f.clone();
        edit(&mut changed.review);
        assert!(m.seller.sign_fund(&changed).is_err());
        assert!(m.buyer.sign_fund(&changed).is_err());
        assert!(m.backend.submit_fund(&changed, &sig).is_err());
        m.assert_balances(0);
    }
    for bad in [
        FundSignatures {
            buyer: [0; 64],
            seller: sig.seller,
        },
        FundSignatures {
            buyer: sig.buyer,
            seller: [0; 64],
        },
        FundSignatures {
            buyer: sig.seller,
            seller: sig.buyer,
        },
    ] {
        assert!(m.backend.submit_fund(&f, &bad).is_err());
        m.assert_balances(0);
    }
}

#[test]
fn invalid_quotes_and_untrusted_or_cross_deployment_messages_are_rejected() {
    let m = Market::new();
    let r = m.request();
    let other = Market::new();
    assert!(matches!(
        other.store.publish(r.clone()),
        Err(Error::Environment)
    ));
    assert!(other
        .store
        .publish_wire(include_bytes!(
            "../../../spec/examples/worked-transaction/request.json"
        ))
        .is_err());
    let outsider = BuyerClient::new(
        Identity::generate().unwrap(),
        buyer_policy(m.backend.environment(), &other.seller_key, &m.verifier),
        m.store.clone(),
        m.backend.clone(),
    );
    assert!(outsider.discover().unwrap().is_empty());
    assert!(matches!(
        outsider.create_request(&m.offer, m.request_terms()),
        Err(Error::PolicyRejected)
    ));
    let mut wrong = m.request_terms();
    wrong.max_principal += 1;
    assert!(matches!(
        m.buyer.create_request(&m.offer, wrong),
        Err(Error::PolicyRejected)
    ));
    let q = m.quote(&r, AMOUNT);
    let mut unsigned = q.unsigned().clone();
    if let Body::Quote(body) = &mut unsigned.payload {
        body.amount = "2500001".into();
    }
    let over = sign(unsigned, &SigningKey::from_bytes(&m.seller_seed)).unwrap();
    assert!(matches!(
        m.buyer.accept(&r, &over),
        Err(Error::PolicyRejected)
    ));
    assert!(m.store.accepted(&r).unwrap().is_none());
    let mut wide = m.quote_terms(AMOUNT);
    wide.deadlines.deliver_by = (FUND + 86401).to_string();
    wide.deadlines.resolve_by = (FUND + 86402).to_string();
    let wide = m.seller.quote(&r, wide).unwrap();
    assert!(matches!(
        m.buyer.accept(&r, &wide),
        Err(Error::PolicyRejected)
    ));
    let mut collision = m.quote_terms(AMOUNT);
    collision.verifier = m.seller_key.clone();
    assert!(matches!(
        m.seller.quote(&r, collision),
        Err(Error::PolicyRejected)
    ));
    let mut body: Value = serde_json::from_slice(&q.to_wire().unwrap()).unwrap();
    body["body"]["acceptance_policy"]["expected_length"] = "51".into();
    assert!(m
        .store
        .publish_wire(&serde_json::to_vec(&body).unwrap())
        .is_err());
    let mut bytes: Value = serde_json::from_slice(&r.to_wire().unwrap()).unwrap();
    bytes["signature"] = hex_string(&[0; 64]).into();
    assert!(m
        .store
        .publish_wire(&serde_json::to_vec(&bytes).unwrap())
        .is_err());
    m.assert_balances(0);
    other.assert_balances(0);
}
