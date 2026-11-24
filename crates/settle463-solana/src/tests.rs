use super::*;
use crate::rpc::Transport;
use crate::transaction::decode_wire;
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};
use settle463_core::wire::{Asset, Deadlines};
use solana_instruction::{AccountMeta, Instruction};
use solana_message::Message;
use std::sync::{Arc, Mutex};

fn actor(byte: u8) -> SigningKey {
    SigningKey::from_bytes(&[byte; 32])
}
fn address(k: &SigningKey) -> Pubkey {
    Pubkey::new_from_array(k.verifying_key().to_bytes())
}
fn example(action: ChainAction) -> Expected {
    let program = Pubkey::new_from_array([83; 32]);
    let mint = Pubkey::new_from_array([99; 32]);
    let buyer = actor(1);
    let seller = actor(2);
    let verifier = actor(3);
    let b = hex_string(buyer.verifying_key().as_bytes());
    let s = hex_string(seller.verifying_key().as_bytes());
    let d = Deployment {
        rpc_url: "http://127.0.0.1:8899".into(),
        genesis: Pubkey::new_from_array([44; 32]).to_string(),
        program_id: program.to_string(),
        config: Deployment::config_address(&program.to_string()).unwrap(),
        mint: mint.to_string(),
        token_program: TOKEN_PROGRAM.into(),
    };
    let nonce = [15; 32];
    let Settlement::Solana {
        escrow_pda,
        buyer_token_account,
        seller_token_account,
        vault_token_account,
        ..
    } = d.settlement(&b, &s, &hex_string(&nonce)).unwrap()
    else {
        unreachable!()
    };
    let terms = Review {
        environment: d.environment(),
        order_id: "0a".repeat(32),
        request_id: "0b".repeat(32),
        quote_id: "0c".repeat(32),
        acceptance_id: "0d".repeat(32),
        policy_hash: "0e".repeat(32),
        buyer: b,
        seller: s,
        verifier: hex_string(verifier.verifying_key().as_bytes()),
        receipt_issuer: hex_string(actor(4).verifying_key().as_bytes()),
        principal: "2000000".into(),
        asset: Asset {
            id: mint.to_string(),
            kind: "spl-token".into(),
            decimals: "6".into(),
        },
        buyer_destination: buyer_token_account,
        seller_destination: seller_token_account,
        escrow_account: escrow_pda,
        deadlines: Deadlines {
            fund_by: "100".into(),
            deliver_by: "200".into(),
            resolve_by: "300".into(),
        },
    };
    Expected {
        deployment: d,
        terms,
        nonce,
        action,
        payer: Some(address(&buyer)),
        delivery: if matches!(action, ChainAction::Release | ChainAction::CommitDelivery) {
            Some([5; 32])
        } else {
            None
        },
        verdict: if action == ChainAction::Release {
            Some([6; 32])
        } else {
            None
        },
        vault: key(&vault_token_account).unwrap(),
    }
}
fn signed(e: &Expected) -> SignedTx {
    let prep = e
        .build(&Pubkey::new_from_array([9; 32]).to_string(), 160, 10)
        .unwrap();
    let roles: Vec<_> = match e.action {
        ChainAction::Fund | ChainAction::CommitDelivery => vec![1, 2],
        ChainAction::Release => vec![1, 3],
        ChainAction::Refund => vec![1],
    };
    let signatures = roles
        .into_iter()
        .map(|i| {
            let k = actor(i);
            e.sign_with(&prep, &hex_string(k.verifying_key().as_bytes()), |b| {
                k.sign(b).to_bytes()
            })
            .unwrap()
        })
        .collect::<Vec<_>>();
    e.assemble(&prep, &signatures).unwrap()
}
#[test]
fn signer_reconstructs_all_legacy_instructions_before_exposing_bytes() {
    let e = example(ChainAction::Fund);
    let tx = signed(&e);
    let (m, _) = decode_wire(&tx.wire).unwrap();
    let buyer = actor(1);
    let payer = address(&buyer);
    let mut ix = vec![e.escrow_ix(payer).unwrap()];
    let extra = Instruction {
        program_id: key(TOKEN_PROGRAM).unwrap(),
        accounts: vec![AccountMeta::new(payer, true)],
        data: vec![3; 9],
    };
    ix.push(extra);
    let altered = Message::new_with_blockhash(&ix, Some(&payer), &m.recent_blockhash);
    let p = Prepared {
        message: STANDARD.encode(altered.serialize()),
        last_valid_block_height: 60,
        context_slot: 10,
    };
    let called = std::cell::Cell::new(false);
    assert!(e
        .sign_with(&p, &hex_string(buyer.verifying_key().as_bytes()), |_| {
            called.set(true);
            [0; 64]
        })
        .is_err());
    assert!(!called.get());
    let mut damaged = tx.clone();
    let mut bytes = STANDARD.decode(&tx.wire).unwrap();
    bytes[66] ^= 1;
    damaged.wire = STANDARD.encode(bytes);
    assert!(e.validate_signed(&damaged).is_err());
    let mut swapped = tx;
    swapped.signature = bs58::encode([0; 64]).into_string();
    assert!(e.validate_signed(&swapped).is_err());
}
#[test]
fn signed_action_pins_and_compute_budget_are_checked() {
    let e = example(ChainAction::Release);
    let payer = e.payer().unwrap();
    let mut ix = e.escrow_ix(payer).unwrap();
    let cb = Instruction {
        program_id: key(COMPUTE).unwrap(),
        accounts: vec![],
        data: vec![2, 0, 1, 0, 0],
    };
    let m = Message::new(&[cb.clone(), ix.clone()], Some(&payer));
    assert_eq!(e.review_message(&m).unwrap(), 1);
    ix.accounts[4].pubkey = payer;
    assert!(e
        .review_message(&Message::new(&[ix], Some(&payer)))
        .is_err());
    let mut bad = cb;
    bad.data[0] = 1;
    assert!(e
        .review_message(&Message::new(
            &[bad, e.escrow_ix(payer).unwrap()],
            Some(&payer)
        ))
        .is_err());
}
#[test]
fn local_signer_authorizes_zero_priority_fee_and_bounded_compute() {
    let e = example(ChainAction::Fund);
    let buyer = actor(1);
    let payer = address(&buyer);
    let limit = |n: u32| [vec![2], n.to_le_bytes().to_vec()].concat();
    let price = |n: u64| [vec![3], n.to_le_bytes().to_vec()].concat();
    for (instructions, permitted) in [
        (vec![limit(1), price(0)], true),
        (vec![limit(1_400_000)], true),
        (vec![price(1)], false),
        (vec![price(u64::MAX)], false),
        (vec![limit(1_400_001)], false),
        (vec![limit(0)], false),
        (vec![price(0), price(0)], false),
        (vec![limit(1), limit(2)], false),
    ] {
        let mut ixs: Vec<_> = instructions
            .into_iter()
            .map(|data| Instruction {
                program_id: key(COMPUTE).unwrap(),
                accounts: vec![],
                data,
            })
            .collect();
        ixs.push(e.escrow_ix(payer).unwrap());
        let msg = Message::new(&ixs, Some(&payer));
        assert!(e.review_message(&msg).is_ok());
        let prepared = Prepared {
            message: STANDARD.encode(msg.serialize()),
            last_valid_block_height: 1,
            context_slot: 0,
        };
        let called = std::cell::Cell::new(false);
        let result = e.sign_with(&prepared, &e.terms.buyer, |bytes| {
            called.set(true);
            buyer.sign(bytes).to_bytes()
        });
        assert_eq!(result.is_ok(), permitted);
        assert_eq!(called.get(), permitted);
    }
}

fn chain_account(owner: &str, data: &[u8], exec: bool) -> Value {
    json!({"owner":owner,"data":[STANDARD.encode(data),"base64"],"executable":exec})
}
fn escrow_bytes(e: &Expected) -> Vec<u8> {
    let r = e.review();
    let mut b = discriminator("account", "Escrow").to_vec();
    for id in [
        &r.order_id,
        &r.request_id,
        &r.quote_id,
        &r.acceptance_id,
        &r.policy_hash,
        &r.buyer,
        &r.seller,
        &r.verifier,
    ] {
        b.extend(hex::<32>(id).unwrap());
    }
    for id in [
        &e.deployment.mint,
        &r.buyer_destination,
        &r.seller_destination,
        &e.vault.to_string(),
        TOKEN_PROGRAM,
        &e.deployment.config,
    ] {
        b.extend(key(id).unwrap().to_bytes());
    }
    b.extend(2_000_000u64.to_le_bytes());
    for t in [100i64, 200, 300] {
        b.extend(t.to_le_bytes());
    }
    b.push(0);
    b.extend([0; 66]);
    b.push(
        Pubkey::find_program_address(
            &[
                b"settle463",
                b"escrow",
                b"v1",
                from_hex(&r.buyer).unwrap().as_ref(),
                &e.nonce,
            ],
            &key(&e.deployment.program_id).unwrap(),
        )
        .1,
    );
    b.push(r.environment.len() as u8);
    b.extend(r.environment.as_bytes());
    b.resize(698, 0);
    b
}
#[derive(Default)]
struct Responses {
    disconnected: bool,
    status: Value,
    transaction: Value,
    escrow: Option<Vec<u8>>,
    height: u64,
    skipped_slot: Option<u64>,
    missing_block: Option<u64>,
    landed: Option<(u64, String)>,
    scanned_blocks: Vec<u64>,
    earliest: u64,
    valid: bool,
    vault_balance: u64,
}
struct Ledger {
    dep: Deployment,
    state: Mutex<Responses>,
}
impl Transport for Ledger {
    fn call(&self, m: &str, p: Value) -> Result<Value> {
        let mut s = self.state.lock().unwrap();
        if s.disconnected {
            return Err(Error::Unavailable("private RPC disconnected".into()));
        }
        let d = &self.dep;
        Ok(match m {
            "getGenesisHash" => json!(d.genesis),
            "getTransaction" => s.transaction.clone(),
            "getSignatureStatuses" => json!({"value":[s.status]}),
            "getBlockHeight" => json!(s.height),
            "isBlockhashValid" => json!({"context":{"slot":s.height},"value":s.valid}),
            "getBlocksWithLimit" => {
                let (start, count) = (p[0].as_u64().unwrap(), p[1].as_u64().unwrap());
                assert_eq!(p[2]["commitment"], "finalized");
                json!((start..=s.height)
                    .filter(|slot| s.skipped_slot != Some(*slot))
                    .take(count as usize)
                    .collect::<Vec<_>>())
            }
            "getBlock" => {
                let slot = p[0].as_u64().unwrap();
                s.scanned_blocks.push(slot);
                if Some(slot) == s.missing_block {
                    return Ok(Value::Null);
                }
                let key_for = |slot: u64| {
                    if slot == 10 {
                        return Pubkey::new_from_array([9; 32]).to_string();
                    }
                    let mut bytes = [0; 32];
                    bytes[..8].copy_from_slice(&slot.to_le_bytes());
                    Pubkey::new_from_array(bytes).to_string()
                };
                let signatures: Vec<_> = s
                    .landed
                    .iter()
                    .filter(|(at, _)| *at == slot)
                    .map(|(_, sig)| sig)
                    .collect();
                let parent = slot.saturating_sub(1);
                let parent = if s.skipped_slot == Some(parent) {
                    parent.saturating_sub(1)
                } else {
                    parent
                };
                let height = slot - u64::from(s.skipped_slot.is_some_and(|skipped| skipped < slot));
                json!({"blockhash":key_for(slot),"previousBlockhash":key_for(parent),"parentSlot":parent,"blockHeight":height,"signatures":signatures})
            }
            "getFirstAvailableBlock" => json!(s.earliest),
            "getMultipleAccounts" => {
                assert_eq!(p[1]["commitment"], "finalized");
                let escrow = s
                    .escrow
                    .as_ref()
                    .map(|b| chain_account(&d.program_id, b, false));
                let mut data = [0; 165];
                data[..32].copy_from_slice(key(&d.mint)?.as_ref());
                data[32..64].copy_from_slice(key(p[0][0].as_str().unwrap())?.as_ref());
                data[64..72].copy_from_slice(&s.vault_balance.to_le_bytes());
                data[108] = 1;
                let vault = chain_account(TOKEN_PROGRAM, &data, false);
                json!({"context":{"slot":90},"value":[escrow,vault]})
            }
            "getAccountInfo" => {
                let addr = p[0].as_str().unwrap();
                let value = if addr == CLOCK {
                    let mut data = [0; 40];
                    data[32..].copy_from_slice(&20i64.to_le_bytes());
                    chain_account("Sysvar1111111111111111111111111111111111111", &data, false)
                } else if addr == d.program_id {
                    let pd = Pubkey::find_program_address(
                        &[key(&d.program_id)?.as_ref()],
                        &key(LOADER)?,
                    )
                    .0;
                    let mut raw = vec![2, 0, 0, 0];
                    raw.extend(pd.to_bytes());
                    chain_account(LOADER, &raw, true)
                } else if addr == d.mint {
                    let mut raw = [0; 82];
                    raw[44] = 6;
                    raw[45] = 1;
                    chain_account(TOKEN_PROGRAM, &raw, false)
                } else if addr == d.config {
                    let mut raw = discriminator("account", "Config").to_vec();
                    raw.push(
                        Pubkey::find_program_address(
                            &[b"settle463", b"config", b"v1"],
                            &key(&d.program_id)?,
                        )
                        .1,
                    );
                    raw.extend(key(&d.mint)?.to_bytes());
                    raw.extend(key(TOKEN_PROGRAM)?.to_bytes());
                    raw.push(6);
                    raw.extend(key(&d.genesis)?.to_bytes());
                    chain_account(&d.program_id, &raw, false)
                } else {
                    s.escrow
                        .as_ref()
                        .map_or(Value::Null, |v| chain_account(&d.program_id, v, false))
                };
                json!({"context":{"slot":90},"value":value})
            }
            _ => return Err(Error::Unavailable(m.to_owned())),
        })
    }
}
#[test]
fn inspect_checks_escrow_and_vault_from_one_finalized_bank() {
    let e = example(ChainAction::Fund);
    let mock = Arc::new(Ledger {
        dep: e.deployment.clone(),
        state: Mutex::new(Responses {
            escrow: Some(escrow_bytes(&e)),
            vault_balance: 2_000_001,
            ..Responses::default()
        }),
    });
    let rpc = Rpc::with_transport(e.deployment.clone(), mock.clone());
    let view = rpc.inspect(&e).unwrap().unwrap();
    assert_eq!(view.held, 2_000_000);
    mock.state.lock().unwrap().vault_balance = 1_999_999;
    assert!(matches!(rpc.inspect(&e), Err(Error::Invalid(_))));
}
#[test]
fn dropped_observations_archival_gaps_and_finalized_failures_are_distinct() {
    let e = example(ChainAction::Fund);
    let tx = signed(&e);
    let mock = Arc::new(Ledger {
        dep: e.deployment.clone(),
        state: Mutex::new(Responses::default()),
    });
    let rpc = Rpc::with_transport(e.deployment.clone(), mock.clone());
    let reconcile = || rpc.reconcile(&e, &tx.signature, Some(&tx)).unwrap().outcome;
    mock.state.lock().unwrap().status = json!({"confirmationStatus":"processed"});
    assert_eq!(reconcile(), ChainOutcome::Pending);
    mock.state.lock().unwrap().status = Value::Null;
    assert_eq!(reconcile(), ChainOutcome::Unknown);
    {
        let mut s = mock.state.lock().unwrap();
        s.height = 180;
        s.earliest = 11;
    }
    assert_eq!(reconcile(), ChainOutcome::Unknown);
    mock.state.lock().unwrap().earliest = 1;
    assert_eq!(reconcile(), ChainOutcome::Expired);
    mock.state.lock().unwrap().escrow = Some(escrow_bytes(&e));
    assert_eq!(reconcile(), ChainOutcome::Expired);
    mock.state.lock().unwrap().transaction = json!({"slot":21,"blockTime":45,"transaction":[tx.wire,"base64"],"meta":{"err":{"InstructionError":[1,"Custom"]}}});
    assert_eq!(reconcile(), ChainOutcome::FinalizedFailure);
    assert!(!rpc
        .reconcile(&e, &bs58::encode([7; 64]).into_string(), None)
        .unwrap_err()
        .is_unavailable());
}
#[test]
fn expiry_after_long_downtime_checks_only_the_real_window_and_binds_its_metadata() {
    let e = example(ChainAction::Fund);
    let tx = signed(&e);
    let mock = Arc::new(Ledger {
        dep: e.deployment.clone(),
        state: Mutex::new(Responses {
            height: 300_000,
            ..Responses::default()
        }),
    });
    let rpc = Rpc::with_transport(e.deployment.clone(), mock.clone());
    let reconcile = |tx: &SignedTx| rpc.reconcile(&e, &tx.signature, Some(tx));
    assert_eq!(reconcile(&tx).unwrap().outcome, ChainOutcome::Expired);
    assert_eq!(
        mock.state.lock().unwrap().scanned_blocks,
        (10..=161).collect::<Vec<_>>()
    );
    for (slot, height) in [(10, 44), (10, u64::MAX), (11, 161)] {
        let bad = SignedTx {
            context_slot: slot,
            last_valid_block_height: height,
            ..tx.clone()
        };
        assert!(matches!(reconcile(&bad), Err(Error::Invalid(_))));
    }
    {
        let mut s = mock.state.lock().unwrap();
        s.skipped_slot = Some(40);
        s.scanned_blocks.clear();
    }
    assert_eq!(reconcile(&tx).unwrap().outcome, ChainOutcome::Expired);
    assert_eq!(
        mock.state.lock().unwrap().scanned_blocks,
        (10..=162).filter(|s| *s != 40).collect::<Vec<_>>()
    );
    mock.state.lock().unwrap().missing_block = Some(82);
    assert_eq!(reconcile(&tx).unwrap().outcome, ChainOutcome::Unknown);
    mock.state.lock().unwrap().missing_block = None;
    mock.state.lock().unwrap().landed = Some((162, tx.signature.clone()));
    assert_eq!(reconcile(&tx).unwrap().outcome, ChainOutcome::Unknown);
}
#[test]
fn finalized_fund_requires_its_historical_delta_and_own_checked_cpi() {
    let e = example(ChainAction::Fund);
    let tx = signed(&e);
    let (msg, _) = decode_wire(&tx.wire).unwrap();
    let idx = |address: Pubkey| msg.account_keys.iter().position(|k| *k == address).unwrap();
    let from = idx(key(&e.terms.buyer_destination).unwrap());
    let to = idx(e.vault);
    let mint = idx(key(&e.deployment.mint).unwrap());
    let auth = idx(address(&actor(1)));
    let t = idx(key(TOKEN_PROGRAM).unwrap());
    let mut transfer = vec![12];
    transfer.extend(2_000_000u64.to_le_bytes());
    transfer.push(6);
    let mut init = vec![18];
    init.extend(key(&e.terms.escrow_account).unwrap().to_bytes());
    let row = |index: usize, n: &str| json!({"accountIndex":index,"mint":e.deployment.mint,"uiTokenAmount":{"amount":n,"decimals":6}});
    let response = json!({"slot":21,"blockTime":45,"transaction":[tx.wire,"base64"],"meta":{"err":null,
        "preTokenBalances":[row(from,"5000000")],"postTokenBalances":[row(from,"3000000"),row(to,"2000000")],
        "innerInstructions":[{"index":0,"instructions":[{"programIdIndex":t,"accounts":[to,mint],"data":bs58::encode(init).into_string()}]},
            {"index":1,"instructions":[{"programIdIndex":t,"accounts":[from,mint,to,auth],"data":bs58::encode(transfer).into_string(),"stackHeight":2}]}]}});
    let mut escrow = escrow_bytes(&e);
    escrow[488] = 3; // A later refunded state still corroborates the earlier funding transaction.
    let mock = Arc::new(Ledger {
        dep: e.deployment.clone(),
        state: Mutex::new(Responses {
            transaction: response.clone(),
            escrow: Some(escrow),
            ..Responses::default()
        }),
    });
    let rpc = Rpc::with_transport(e.deployment.clone(), mock.clone());
    assert_eq!(
        rpc.reconcile(&e, &tx.signature, Some(&tx)).unwrap().outcome,
        ChainOutcome::FinalizedSuccess
    );
    for field in ["remove creation", "change delta", "move CPI"] {
        let mut v = response.clone();
        match field {
            "remove creation" => v["meta"]["innerInstructions"][0]["instructions"] = json!([]),
            "change delta" => {
                v["meta"]["postTokenBalances"][1]["uiTokenAmount"]["amount"] = json!("2000001")
            }
            _ => v["meta"]["innerInstructions"][1]["index"] = json!(2),
        };
        mock.state.lock().unwrap().transaction = v;
        assert!(matches!(
            rpc.reconcile(&e, &tx.signature, Some(&tx)),
            Err(Error::Invalid(_))
        ));
    }
}

#[test]
fn expired_message_requires_new_authorizations_from_both_funding_signers() {
    let e = example(ChainAction::Fund);
    let mock = Arc::new(Ledger {
        dep: e.deployment.clone(),
        state: Mutex::new(Responses::default()),
    });
    let rpc = Rpc::with_transport(e.deployment.clone(), mock.clone());
    let mut prepared = e
        .build(&Pubkey::new_from_array([9; 32]).to_string(), 160, 10)
        .unwrap();
    let called = std::cell::Cell::new(false);
    let buyer = actor(1);
    assert!(rpc
        .sign_with(&e, &prepared, &e.terms.buyer, |b| {
            called.set(true);
            buyer.sign(b).to_bytes()
        })
        .is_err());
    assert!(!called.get());
    mock.state.lock().unwrap().valid = true;
    let old_buyer = rpc.sign(&e, &prepared, &[1; 32]).unwrap();
    prepared = e
        .build(&Pubkey::new_from_array([10; 32]).to_string(), 211, 161)
        .unwrap();
    let buyer = rpc.sign(&e, &prepared, &[1; 32]).unwrap();
    let seller = rpc.sign(&e, &prepared, &[2; 32]).unwrap();
    assert_ne!(buyer, old_buyer);
    assert!(e.assemble(&prepared, &[old_buyer, seller.clone()]).is_err());
    assert!(e.assemble(&prepared, &[buyer, seller]).is_ok());
}

#[test]
fn visible_transport_failure_and_dropped_release_are_followed_by_one_canonical_payout() {
    let e = example(ChainAction::Release);
    let tx = signed(&e);
    let (msg, _) = decode_wire(&tx.wire).unwrap();
    let index = |key: Pubkey| msg.account_keys.iter().position(|k| *k == key).unwrap();
    let from = index(e.vault);
    let to = index(key(&e.terms.seller_destination).unwrap());
    let mint = index(key(&e.deployment.mint).unwrap());
    let auth = index(key(&e.terms.escrow_account).unwrap());
    let mut transfer = vec![12];
    transfer.extend(2_000_000u64.to_le_bytes());
    transfer.push(6);
    let balance = |i: usize, n: &str| json!({"accountIndex":i,"mint":e.deployment.mint,"uiTokenAmount":{"amount":n,"decimals":6}});
    let canonical = json!({"slot":35,"blockTime":250,"transaction":[tx.wire,"base64"],"meta":{"err":null,
        "preTokenBalances":[balance(from,"2000001"),balance(to,"1")],"postTokenBalances":[balance(from,"1"),balance(to,"2000001")],
        "innerInstructions":[{"index":1,"instructions":[{"programIdIndex":index(key(TOKEN_PROGRAM).unwrap()),"accounts":[from,mint,to,auth],"data":bs58::encode(transfer).into_string(),"stackHeight":2}]}]}});
    let mock = Arc::new(Ledger {
        dep: e.deployment.clone(),
        state: Mutex::new(Responses {
            disconnected: true,
            ..Responses::default()
        }),
    });
    let rpc = Rpc::with_transport(e.deployment.clone(), mock.clone());
    let check = || rpc.reconcile(&e, &tx.signature, Some(&tx));
    assert!(check().unwrap_err().is_unavailable());
    {
        let mut s = mock.state.lock().unwrap();
        s.disconnected = false;
        s.status = json!({"confirmationStatus":"processed"});
    }
    assert_eq!(check().unwrap().outcome, ChainOutcome::Pending);
    mock.state.lock().unwrap().status = Value::Null;
    assert_eq!(check().unwrap().outcome, ChainOutcome::Unknown);
    assert_eq!(
        rpc.reconcile(&e, &tx.signature, None).unwrap().outcome,
        ChainOutcome::Unknown
    );
    let mut escrow = escrow_bytes(&e);
    escrow[488] = 2;
    escrow[489..521].copy_from_slice(&[5; 32]);
    escrow[521] = 1;
    escrow[522..554].copy_from_slice(&[6; 32]);
    escrow[554] = 1;
    {
        let mut s = mock.state.lock().unwrap();
        s.transaction = canonical;
        s.escrow = Some(escrow);
    }
    for _ in 0..2 {
        let r = check().unwrap();
        assert_eq!(r.outcome, ChainOutcome::FinalizedSuccess);
        assert_eq!(r.slot, Some(35));
    }
}

#[test]
fn external_refund_requires_its_own_checked_transfer_and_terminal_replays_preserve_failure() {
    let mut e = example(ChainAction::Refund);
    let stranger = actor(14);
    e.payer = Some(address(&stranger));
    let p = e
        .build(&Pubkey::new_from_array([12; 32]).to_string(), 180, 80)
        .unwrap();
    let signature = e
        .sign_with(&p, &hex_string(stranger.verifying_key().as_bytes()), |b| {
            stranger.sign(b).to_bytes()
        })
        .unwrap();
    let tx = e.assemble(&p, &[signature]).unwrap();
    e.payer = None;
    let (msg, _) = decode_wire(&tx.wire).unwrap();
    let idx = |k: Pubkey| msg.account_keys.iter().position(|x| *x == k).unwrap();
    let v = idx(e.vault);
    let m = idx(key(&e.deployment.mint).unwrap());
    let b = idx(key(&e.terms.buyer_destination).unwrap());
    let authority = idx(key(&e.terms.escrow_account).unwrap());
    let mut transfer = vec![12];
    transfer.extend(2_000_000u64.to_le_bytes());
    transfer.push(6);
    let amount = |i, n: &str| json!({"accountIndex":i,"mint":e.deployment.mint,"uiTokenAmount":{"amount":n,"decimals":6}});
    let mut chain = json!({"slot":90,"blockTime":201,"transaction":[tx.wire,"base64"],"meta":{"err":null,
        "preTokenBalances":[amount(v,"2000001"),amount(b,"3000000")],"postTokenBalances":[amount(v,"1"),amount(b,"5000000")],
        "innerInstructions":[{"index":1,"instructions":[{"programIdIndex":idx(key(TOKEN_PROGRAM).unwrap()),"accounts":[v,m,b,authority],"data":bs58::encode(transfer).into_string(),"stackHeight":2}]}]}});
    let mut escrow = escrow_bytes(&e);
    escrow[488] = 3;
    let mock = Arc::new(Ledger {
        dep: e.deployment.clone(),
        state: Mutex::new(Responses {
            transaction: chain.clone(),
            escrow: Some(escrow),
            ..Responses::default()
        }),
    });
    let rpc = Rpc::with_transport(e.deployment.clone(), mock.clone());
    let r = || rpc.reconcile(&e, &tx.signature, None);
    assert_eq!(r().unwrap().outcome, ChainOutcome::FinalizedSuccess);
    chain["meta"]["innerInstructions"][0]["index"] = json!(0);
    mock.state.lock().unwrap().transaction = chain.clone();
    assert!(matches!(r(), Err(Error::Invalid(_))));
    chain["meta"]["err"] = json!({"InstructionError":[1,{"Custom":6006}]});
    mock.state.lock().unwrap().transaction = chain;
    let replay = r().unwrap();
    assert_eq!(replay.outcome, ChainOutcome::FinalizedFailure);
    assert_eq!(
        replay.known_terminal,
        Some(settle463_core::settlement::OrderState::Refunded)
    );
}

#[test]
fn a_batched_refund_verifies_each_order_against_its_own_invocation() {
    let mut first = example(ChainAction::Refund);
    first.payer = None;
    let mut second = first.clone();
    second.nonce = [22; 32];
    second.terms.buyer = hex_string(actor(22).verifying_key().as_bytes());
    second.terms.order_id = hex_string(
        &settle463_core::order_id(
            &second.terms.environment,
            &second.terms.buyer,
            &hex_string(&second.nonce),
        )
        .unwrap(),
    );
    let Settlement::Solana {
        escrow_pda,
        vault_token_account,
        buyer_token_account,
        ..
    } = second
        .deployment
        .settlement(
            &second.terms.buyer,
            &second.terms.seller,
            &hex_string(&second.nonce),
        )
        .unwrap()
    else {
        unreachable!()
    };
    second.terms.escrow_account = escrow_pda;
    second.terms.buyer_destination = buyer_token_account;
    second.vault = key(&vault_token_account).unwrap();
    let orders = [&first, &second];
    let fee_payer = actor(14);
    let payer = address(&fee_payer);
    let refunds = orders.map(|e| e.escrow_ix(payer).unwrap());
    let message = Message::new(&refunds, Some(&payer));
    let raw = message.serialize();
    let sig = fee_payer.sign(&raw).to_bytes();
    let locator = bs58::encode(sig).into_string();
    let wire = STANDARD.encode([&[1u8][..], &sig[..], &raw].concat());
    let idx = |k| message.account_keys.iter().position(|x| x == &k).unwrap();
    let amount = |i, value: &str| json!({"accountIndex":i,"mint":first.deployment.mint,"uiTokenAmount":{"amount":value,"decimals":6}});
    let mut pre = vec![];
    let mut post = vec![];
    let mut invocations = vec![];
    let transfer = [vec![12], 2_000_000u64.to_le_bytes().to_vec(), vec![6]].concat();
    for (index, e) in orders.iter().enumerate() {
        let v = idx(e.vault);
        let b = idx(key(&e.terms.buyer_destination).unwrap());
        pre.extend([amount(v, "2000001"), amount(b, "3000000")]);
        post.extend([amount(v, "1"), amount(b, "5000000")]);
        invocations.push(json!({"index":index,"instructions":[{
            "programIdIndex":idx(key(TOKEN_PROGRAM).unwrap()),
            "accounts":[v,idx(key(&e.deployment.mint).unwrap()),b,idx(key(&e.terms.escrow_account).unwrap())],
            "data":bs58::encode(&transfer).into_string(),"stackHeight":2
        }]}));
    }
    let response = json!({"slot":90,"blockTime":201,"transaction":[wire,"base64"],"meta":{"err":null,
        "innerInstructions":invocations,"preTokenBalances":pre,"postTokenBalances":post}});
    for (index, e) in orders.iter().enumerate() {
        let mut escrow = escrow_bytes(e);
        escrow[488] = 3;
        let mock = Arc::new(Ledger {
            dep: e.deployment.clone(),
            state: Mutex::new(Responses {
                transaction: response.clone(),
                escrow: Some(escrow),
                ..Responses::default()
            }),
        });
        let rpc = Rpc::with_transport(e.deployment.clone(), mock.clone());
        assert_eq!(
            rpc.reconcile(e, &locator, None).unwrap().outcome,
            ChainOutcome::FinalizedSuccess
        );
        let mut other_cpi = response.clone();
        other_cpi["meta"]["innerInstructions"][index]["instructions"] =
            response["meta"]["innerInstructions"][1 - index]["instructions"].clone();
        mock.state.lock().unwrap().transaction = other_cpi;
        assert!(matches!(
            rpc.reconcile(e, &locator, None),
            Err(Error::Invalid(_))
        ));
    }
    let duplicate = Message::new(&[refunds[0].clone(), refunds[0].clone()], Some(&payer));
    assert!(first.external_refund(&duplicate).is_err());
}
