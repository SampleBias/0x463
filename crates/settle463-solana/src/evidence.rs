use super::*;
use crate::transaction::{decode_wire, decoded_ix};
use serde_json::{json, Value};
use settle463_core::settlement::{OrderState, OrderView};
use solana_message::Message;

impl Expected {
    fn decode_escrow(&self, bytes: &[u8]) -> Result<OrderView> {
        ensure(
            bytes.len() == 698 && bytes[..8] == discriminator("account", "Escrow"),
            "escrow size or discriminator",
        )?;
        let t = &self.terms;
        let expected = [
            hex(&t.order_id).map_err(invalid)?,
            hex(&t.request_id).map_err(invalid)?,
            hex(&t.quote_id).map_err(invalid)?,
            hex(&t.acceptance_id).map_err(invalid)?,
            hex(&t.policy_hash).map_err(invalid)?,
            hex(&t.buyer).map_err(invalid)?,
            hex(&t.seller).map_err(invalid)?,
            hex(&t.verifier).map_err(invalid)?,
            key(&self.deployment.mint)?.to_bytes(),
            key(&t.buyer_destination)?.to_bytes(),
            key(&t.seller_destination)?.to_bytes(),
            self.vault.to_bytes(),
            key(TOKEN_PROGRAM)?.to_bytes(),
            key(&self.deployment.config)?.to_bytes(),
        ];
        ensure(
            bytes[8..456]
                .chunks_exact(32)
                .zip(expected)
                .all(|(a, b)| a == b),
            "escrow immutable agreement fields",
        )?;
        let principal = self.principal()?;
        ensure(
            bytes[456..464] == principal.to_le_bytes()
                && bytes[464..488]
                    .chunks_exact(8)
                    .zip(self.times()?)
                    .all(|(a, b)| a == b.to_le_bytes()),
            "escrow principal or deadlines",
        )?;
        let state = match bytes[488] {
            0 => OrderState::Funded,
            1 => OrderState::Delivered,
            2 => OrderState::Paid,
            3 => OrderState::Refunded,
            _ => return Err(invalid("escrow state")),
        };
        let opt = |id: &[u8], flag: u8| match flag {
            1 => Ok(Some(hex_string(id))),
            0 if id.iter().all(|b| *b == 0) => Ok(None),
            _ => Err(invalid("invalid escrow evidence flag")),
        };
        let delivery_id = opt(&bytes[489..521], bytes[521])?;
        let verdict_id = opt(&bytes[522..554], bytes[554])?;
        ensure(
            match state {
                OrderState::Funded => delivery_id.is_none() && verdict_id.is_none(),
                OrderState::Delivered => delivery_id.is_some() && verdict_id.is_none(),
                OrderState::Paid => delivery_id.is_some() && verdict_id.is_some(),
                OrderState::Refunded => verdict_id.is_none(),
            },
            "state and delivery/verdict flags disagree",
        )?;
        let (_, bump) = Pubkey::find_program_address(
            &[
                b"settle463",
                b"escrow",
                b"v1",
                from_hex(&t.buyer)?.as_ref(),
                &self.nonce,
            ],
            &key(&self.deployment.program_id)?,
        );
        ensure(
            bytes[555] == bump
                && usize::from(bytes[556]) == t.environment.len()
                && bytes[557..557 + t.environment.len()] == *t.environment.as_bytes()
                && bytes[557 + t.environment.len()..].iter().all(|b| *b == 0),
            "escrow deployment or bump",
        )?;
        let (held, paid, refunded) = match state {
            OrderState::Funded | OrderState::Delivered => (principal, 0, 0),
            OrderState::Paid => (0, principal, 0),
            OrderState::Refunded => (0, 0, principal),
        };
        Ok(OrderView {
            state,
            review: t.clone(),
            held,
            paid,
            refunded,
            delivery_id,
            verdict_id,
            terminal_clock: None,
            terminal_locator: None,
        })
    }

    fn verify_state(&self, view: &OrderView) -> Result<()> {
        let delivery = self.delivery.map(|v| hex_string(&v));
        let verdict = self.verdict.map(|v| hex_string(&v));
        ensure(
            match self.action {
                ChainAction::Fund => true,
                ChainAction::CommitDelivery => {
                    view.delivery_id == delivery && view.state != OrderState::Funded
                }
                ChainAction::Release => {
                    view.state == OrderState::Paid
                        && view.delivery_id == delivery
                        && view.verdict_id == verdict
                }
                ChainAction::Refund => {
                    view.state == OrderState::Refunded
                        && view.delivery_id == delivery
                        && view.verdict_id.is_none()
                }
            },
            "finalized escrow contradicts action",
        )
    }
    // A permissionless refund may accompany unrelated instructions. Tie the transfer to the
    // escrow invocation itself and verify the complete net beneficiary and vault changes.
    fn external_refund(&self, msg: &Message) -> Result<usize> {
        let required = self.escrow_ix(msg.account_keys[0])?;
        let mut index = None;
        for (i, c) in msg.instructions.iter().enumerate() {
            let actual = decoded_ix(msg, c)?;
            if actual.program_id == required.program_id {
                ensure(
                    index.is_none()
                        && actual.data == required.data
                        && actual.accounts.len() == required.accounts.len()
                        && actual
                            .accounts
                            .iter()
                            .zip(&required.accounts)
                            .all(|(a, b)| {
                                a.pubkey == b.pubkey
                                    && (!b.is_writable || a.is_writable)
                                    && (!b.is_signer || a.is_signer)
                            }),
                    "external refund instruction",
                )?;
                index = Some(i);
            }
        }
        index.ok_or_else(|| invalid("no escrow refund instruction"))
    }
}

fn key_index(msg: &Message, address: Pubkey) -> Result<usize> {
    msg.account_keys
        .iter()
        .position(|k| *k == address)
        .ok_or_else(|| invalid("missing token account in transaction"))
}
fn token_amount(
    meta: &Value,
    which: &str,
    index: usize,
    mint: &str,
    owner: &Pubkey,
) -> Result<Option<u64>> {
    let rows = meta[which]
        .as_array()
        .ok_or_else(|| Error::Unavailable(format!("missing {which}")))?;
    let mut found = None;
    for row in rows
        .iter()
        .filter(|r| r["accountIndex"].as_u64() == Some(index as u64))
    {
        ensure(
            found.is_none()
                && row["mint"] == mint
                && row["uiTokenAmount"]["decimals"] == 6
                && row.get("owner").is_none_or(|o| o == &owner.to_string())
                && row.get("programId").is_none_or(|id| id == TOKEN_PROGRAM),
            "token balance metadata",
        )?;
        found = Some(
            decimal(
                row["uiTokenAmount"]["amount"]
                    .as_str()
                    .ok_or_else(|| invalid("token amount"))?,
            )
            .map_err(invalid)?,
        );
    }
    Ok(found)
}
fn inner(meta: &Value, at: usize) -> Result<&[Value]> {
    let groups = meta["innerInstructions"]
        .as_array()
        .ok_or_else(|| Error::Unavailable("inner instructions unavailable".into()))?;
    let mut matching = groups
        .iter()
        .filter(|v| v["index"].as_u64() == Some(at as u64));
    let Some(group) = matching.next() else {
        return Ok(&[]);
    };
    ensure(
        matching.next().is_none(),
        "duplicate inner instruction group",
    )?;
    group["instructions"]
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| invalid("inner instruction list"))
}
fn account_indices(ix: &Value) -> Result<Vec<usize>> {
    ix["accounts"]
        .as_array()
        .ok_or_else(|| invalid("inner instruction accounts"))?
        .iter()
        .map(|v| {
            v.as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| invalid("inner instruction index"))
        })
        .collect()
}
fn program_is(ix: &Value, msg: &Message, program: Pubkey) -> bool {
    ix["programIdIndex"]
        .as_u64()
        .and_then(|i| usize::try_from(i).ok())
        .and_then(|i| msg.account_keys.get(i))
        == Some(&program)
}
fn ata_initialized(
    msg: &Message,
    meta: &Value,
    account: usize,
    mint: usize,
    owner: &Pubkey,
    fund_index: Option<usize>,
) -> Result<bool> {
    let token = key(TOKEN_PROGRAM)?;
    let ata = key(ATA_PROGRAM)?;
    for (i, c) in msg.instructions.iter().enumerate() {
        let top = msg.account_keys.get(usize::from(c.program_id_index)) == Some(&ata)
            && matches!(c.data.as_slice(), [1] | [] | [0])
            && c.accounts
                .get(1)
                .is_some_and(|n| usize::from(*n) == account);
        if !top && fund_index != Some(i) {
            continue;
        }
        let mut nested = false;
        for ix in inner(meta, i)? {
            let height = ix["stackHeight"].as_u64().unwrap_or(2);
            if height == 2 {
                nested = false;
            }
            if !top && height == 2 && program_is(ix, msg, ata) {
                let args = account_indices(ix)?;
                let data = bs58::decode(
                    ix["data"]
                        .as_str()
                        .ok_or_else(|| invalid("ATA instruction data"))?,
                )
                .into_vec()
                .map_err(invalid)?;
                nested = matches!(data.as_slice(), [1] | [] | [0])
                    && args.get(1) == Some(&account)
                    && args.get(3) == Some(&mint);
            }
            if !(top && height == 2 || nested && height == 3) || !program_is(ix, msg, token) {
                continue;
            }
            let data = bs58::decode(
                ix["data"]
                    .as_str()
                    .ok_or_else(|| invalid("inner token data"))?,
            )
            .into_vec()
            .map_err(invalid)?;
            let args = account_indices(ix)?;
            if data.len() == 33
                && matches!(data[0], 16 | 18)
                && &data[1..] == owner.as_ref()
                && args.starts_with(&[account, mint])
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
fn transfer_evidence(e: &Expected, msg: &Message, meta: &Value, at: usize) -> Result<()> {
    if e.action == ChainAction::CommitDelivery {
        for ix in inner(meta, at)? {
            ensure(
                !program_is(ix, msg, key(TOKEN_PROGRAM)?),
                "token CPI in delivery",
            )?;
        }
        return Ok(());
    }
    let escrow = key(&e.terms.escrow_account)?;
    let buyer = from_hex(&e.terms.buyer)?;
    let seller = from_hex(&e.terms.seller)?;
    let (source, dest, source_owner, dest_owner) = match e.action {
        ChainAction::Fund => (key(&e.terms.buyer_destination)?, e.vault, buyer, escrow),
        ChainAction::Release => (e.vault, key(&e.terms.seller_destination)?, escrow, seller),
        ChainAction::Refund => (e.vault, key(&e.terms.buyer_destination)?, escrow, buyer),
        ChainAction::CommitDelivery => unreachable!(),
    };
    let from = key_index(msg, source)?;
    let to = key_index(msg, dest)?;
    let mint = key_index(msg, key(&e.deployment.mint)?)?;
    let auth = key_index(msg, source_owner)?;
    let mut transfer = vec![12];
    transfer.extend(e.principal()?.to_le_bytes());
    transfer.push(6);
    let mut matched = 0;
    for ix in inner(meta, at)? {
        if program_is(ix, msg, key(TOKEN_PROGRAM)?) && ix["stackHeight"].as_u64().unwrap_or(2) == 2
        {
            let data = bs58::decode(
                ix["data"]
                    .as_str()
                    .ok_or_else(|| invalid("inner transfer data"))?,
            )
            .into_vec()
            .map_err(invalid)?;
            ensure(
                ix.get("stackHeight").is_none_or(|v| v.is_null() || v == 2)
                    && data == transfer
                    && account_indices(ix)? == [from, mint, to, auth],
                "unexpected SPL CPI in escrow instruction",
            )?;
            matched += 1;
        }
    }
    ensure(
        matched == 1,
        "escrow must perform exactly one checked transfer",
    )?;
    let amount = |column: &str, index, owner: &Pubkey| {
        token_amount(meta, column, index, &e.deployment.mint, owner)
    };
    let src_pre = amount("preTokenBalances", from, &source_owner)?
        .ok_or_else(|| invalid("missing source pre-balance"))?;
    let src_post = amount("postTokenBalances", from, &source_owner)?
        .ok_or_else(|| invalid("missing source post-balance"))?;
    let dst_pre = match amount("preTokenBalances", to, &dest_owner)? {
        Some(n) => n,
        None if ata_initialized(
            msg,
            meta,
            to,
            mint,
            &dest_owner,
            (e.action == ChainAction::Fund).then_some(at),
        )? =>
        {
            0
        }
        None => {
            return Err(invalid(
                "missing destination pre-balance without ATA initialization",
            ))
        }
    };
    let dst_post = amount("postTokenBalances", to, &dest_owner)?
        .ok_or_else(|| invalid("missing destination post-balance"))?;
    ensure(
        src_pre.checked_sub(src_post) == Some(e.principal()?)
            && dst_post.checked_sub(dst_pre) == Some(e.principal()?),
        "finalized token deltas disagree with principal",
    )
}

impl Rpc {
    fn escrow_at(&self, e: &Expected, slot: u64) -> Result<Option<OrderView>> {
        ensure(self.deployment == e.deployment, "RPC deployment")?;
        let Some(a) = self.account(&e.terms.escrow_account, slot)? else {
            return Ok(None);
        };
        self.decode_owned_escrow(e, &a).map(Some)
    }
    fn decode_owned_escrow(&self, e: &Expected, a: &crate::rpc::Account) -> Result<OrderView> {
        ensure(
            !a.executable && a.owner == self.deployment.program_id,
            "escrow owner",
        )?;
        e.decode_escrow(&a.data)
    }
    pub fn inspect(&self, e: &Expected) -> Result<Option<OrderView>> {
        ensure(self.deployment == e.deployment, "RPC deployment")?;
        self.validate_deployment()?;
        let (escrow, vault) = self.escrow_and_vault(e)?;
        let Some(escrow) = escrow else {
            return Ok(None);
        };
        let v = self.decode_owned_escrow(e, &escrow)?;
        let vault = vault.ok_or_else(|| invalid("missing escrow vault"))?;
        let balance = self.token_balance(&vault, Some(&key(&e.terms.escrow_account)?))?;
        ensure(balance >= v.held, "vault balance below held principal")?;
        Ok(Some(v))
    }
    pub fn reconcile(
        &self,
        e: &Expected,
        signature: &str,
        stored: Option<&SignedTx>,
    ) -> Result<Resolution> {
        base58::<64>(signature).map_err(invalid)?;
        self.validate_deployment()?;
        if let Some(tx) = stored {
            ensure(tx.signature == signature, "stored signature mismatch")?;
            e.validate_signed(tx)?;
        }
        let v=self.call("getTransaction",json!([signature,{"encoding":"base64","commitment":"finalized","maxSupportedTransactionVersion":0}]))?;
        if v.is_null() {
            return self.missing(e, signature, stored);
        }
        let slot = v["slot"]
            .as_u64()
            .ok_or_else(|| invalid("finalized slot"))?;
        ensure(v["transaction"][1] == "base64", "transaction encoding")?;
        let wire = v["transaction"][0]
            .as_str()
            .ok_or_else(|| invalid("finalized transaction bytes"))?;
        let (msg, bytes) = decode_wire(wire)?;
        ensure(
            bytes[1..65] == base58::<64>(signature).map_err(invalid)?
                && stored.is_none_or(|tx| tx.wire == wire),
            "finalized bytes or signature differ from intent",
        )?;
        let at = if stored.is_none() && e.action == ChainAction::Refund {
            e.external_refund(&msg)?
        } else {
            e.review_message(&msg)?
        };
        let Some(err) = v["meta"].get("err") else {
            return Ok(Resolution::status(ChainOutcome::Unknown));
        };
        let clock = v["blockTime"].as_i64();
        if !err.is_null() {
            let mut known_terminal = None;
            if err["InstructionError"][0].as_u64() == Some(at as u64)
                && err["InstructionError"][1]["Custom"] == 6006
            {
                known_terminal = self
                    .escrow_at(e, slot)?
                    .map(|v| v.state)
                    .filter(|s| matches!(s, OrderState::Paid | OrderState::Refunded));
            }
            return Ok(Resolution {
                outcome: ChainOutcome::FinalizedFailure,
                slot: Some(slot),
                clock,
                known_terminal,
            });
        }
        transfer_evidence(e, &msg, &v["meta"], at)?;
        let Some(escrow) = self.escrow_at(e, slot)? else {
            return Err(invalid("finalized escrow missing"));
        };
        e.verify_state(&escrow)?;
        Ok(Resolution {
            outcome: ChainOutcome::FinalizedSuccess,
            slot: Some(slot),
            clock,
            known_terminal: None,
        })
    }
    /// A null transaction lookup alone cannot rule out a gap in RPC history. Follow
    /// finalized parent links to the block that supplied the signed recent blockhash.
    fn proved_absent(
        &self,
        signature: &str,
        hash: &str,
        mut slot: u64,
        context: u64,
    ) -> Result<bool> {
        if slot < context || slot - context > 1024 {
            return Ok(false);
        }
        let mut required_hash = None;
        for _ in 0..=1024 {
            let block=self.call("getBlock",json!([slot,{"commitment":"finalized","transactionDetails":"signatures","rewards":false,"maxSupportedTransactionVersion":0}]))?;
            let (Some(block_hash), Some(previous), Some(parent), Some(signatures)) = (
                block["blockhash"].as_str(),
                block["previousBlockhash"].as_str(),
                block["parentSlot"].as_u64(),
                block["signatures"].as_array(),
            ) else {
                return Ok(false);
            };
            if required_hash.as_deref().is_some_and(|h| h != block_hash)
                || signatures
                    .iter()
                    .any(|s| s.as_str().is_none_or(|s| s == signature))
            {
                return Ok(false);
            }
            if block_hash == hash {
                return Ok(slot <= context);
            }
            if slot == 0 || parent >= slot || slot < context {
                return Ok(false);
            }
            required_hash = Some(previous.to_owned());
            slot = parent;
        }
        Ok(false)
    }
    fn missing(&self, e: &Expected, sig: &str, stored: Option<&SignedTx>) -> Result<Resolution> {
        let s = self.call(
            "getSignatureStatuses",
            json!([[sig],{"searchTransactionHistory":true}]),
        )?;
        let row = s["value"]
            .as_array()
            .filter(|v| v.len() == 1)
            .ok_or_else(|| invalid("signature status list"))?
            .first()
            .ok_or_else(|| invalid("signature status"))?;
        if !row.is_null() {
            return Ok(Resolution::status(
                match row["confirmationStatus"].as_str() {
                    Some("processed" | "confirmed") => ChainOutcome::Pending,
                    _ => ChainOutcome::Unknown,
                },
            ));
        }
        let Some(tx) = stored else {
            return Ok(Resolution::status(ChainOutcome::Unknown));
        };
        let (msg, _) = decode_wire(&tx.wire)?;
        let h = self
            .call("getBlockHeight", json!([{"commitment":"finalized"}]))?
            .as_u64()
            .ok_or_else(|| invalid("finalized block height"))?;
        if h <= tx.last_valid_block_height {
            return Ok(Resolution::status(ChainOutcome::Unknown));
        }
        let valid = self.call(
            "isBlockhashValid",
            json!([msg.recent_blockhash.to_string(),{"commitment":"finalized"}]),
        )?;
        let earliest = self
            .call("getFirstAvailableBlock", json!([]))?
            .as_u64()
            .ok_or_else(|| invalid("ledger history bound"))?;
        if valid["value"] != false || earliest > tx.context_slot {
            return Ok(Resolution::status(ChainOutcome::Unknown));
        }
        let state = self.escrow_at(e, tx.context_slot)?;
        let absent = matches!(
            (e.action, state.as_ref().map(|v| v.state)),
            (ChainAction::Fund, None)
                | (ChainAction::CommitDelivery, Some(OrderState::Funded))
                | (ChainAction::Release, Some(OrderState::Delivered))
                | (
                    ChainAction::Refund,
                    Some(OrderState::Funded | OrderState::Delivered)
                )
        );
        let tip = valid["context"]["slot"].as_u64();
        let absent = if let Some(slot) = tip.filter(|_| absent) {
            self.proved_absent(
                sig,
                &msg.recent_blockhash.to_string(),
                slot,
                tx.context_slot,
            )?
        } else {
            false
        };
        Ok(Resolution::status(if absent {
            ChainOutcome::Expired
        } else {
            ChainOutcome::Unknown
        }))
    }
}
