use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use bincode::Options;
use ed25519_dalek::{Signature, VerifyingKey};
use solana_instruction::{AccountMeta as Meta, Instruction};
use solana_message::Message;

const MAX_WIRE: usize = 1232;
const MAX_COMPUTE_UNITS: u32 = 1_400_000;

fn authorize_fees(msg: &Message) -> Result<()> {
    let program = key(COMPUTE)?;
    let mut limit_set = false;
    let mut price_set = false;
    for ix in &msg.instructions {
        if msg.account_keys.get(usize::from(ix.program_id_index)) != Some(&program) {
            continue;
        }
        match ix.data.as_slice() {
            [2, a, b, c, d]
                if !limit_set
                    && (1..=MAX_COMPUTE_UNITS).contains(&u32::from_le_bytes([*a, *b, *c, *d])) =>
            {
                limit_set = true;
            }
            [3, 0, 0, 0, 0, 0, 0, 0, 0] if !price_set => price_set = true,
            _ => {
                return Err(invalid(
                    "local signer requires zero priority fee and a valid compute unit limit",
                ))
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prepared {
    pub message: String,
    pub last_valid_block_height: u64,
    pub context_slot: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedTx {
    pub signature: String,
    pub wire: String,
    pub last_valid_block_height: u64,
    pub context_slot: u64,
}
impl SignedTx {
    pub fn message_bytes(&self) -> Result<Vec<u8>> {
        let (message, _) = decode_wire(&self.wire)?;
        Ok(message.serialize())
    }
}
fn ro(key: Pubkey) -> Meta {
    Meta::new_readonly(key, false)
}
fn rw(key: Pubkey) -> Meta {
    Meta::new(key, false)
}
fn signer(key: Pubkey) -> Meta {
    Meta::new_readonly(key, true)
}

pub(crate) fn decode_message(raw: &[u8]) -> Result<Message> {
    ensure(raw.len() <= MAX_WIRE, "transaction message too large")?;
    let msg: Message = bincode::options()
        .with_fixint_encoding()
        .with_limit(MAX_WIRE as u64)
        .reject_trailing_bytes()
        .deserialize(raw)
        .map_err(invalid)?;
    let h = &msg.header;
    let n = msg.account_keys.len();
    let sigs = usize::from(h.num_required_signatures);
    ensure(
        sigs > 0
            && sigs <= n
            && h.num_readonly_signed_accounts < h.num_required_signatures
            && usize::from(h.num_readonly_unsigned_accounts) <= n - sigs
            && !msg.has_duplicates(),
        "legacy message header",
    )?;
    ensure(msg.serialize() == raw, "noncanonical transaction encoding")?;
    Ok(msg)
}
pub(crate) fn decode_wire(wire: &str) -> Result<(Message, Vec<u8>)> {
    ensure(
        wire.len() <= MAX_WIRE.div_ceil(3) * 4,
        "transaction too large",
    )?;
    let bytes = STANDARD.decode(wire).map_err(invalid)?;
    let count = usize::from(*bytes.first().ok_or_else(|| invalid("empty transaction"))?);
    // A legacy transaction fits at most 12 signatures inside the packet limit.
    ensure(
        (1..=12).contains(&count) && bytes.len() <= MAX_WIRE,
        "transaction signature count",
    )?;
    let raw = bytes
        .get(1 + count * 64..)
        .ok_or_else(|| invalid("truncated signatures"))?;
    let msg = decode_message(raw)?;
    ensure(
        usize::from(msg.header.num_required_signatures) == count,
        "signature count disagrees with message",
    )?;
    for (k, s) in msg
        .account_keys
        .iter()
        .zip(bytes[1..1 + count * 64].chunks_exact(64))
    {
        VerifyingKey::from_bytes(&k.to_bytes())
            .map_err(invalid)?
            .verify_strict(raw, &Signature::from_slice(s).map_err(invalid)?)
            .map_err(invalid)?;
    }
    Ok((msg, bytes))
}
pub(crate) fn decoded_ix(
    msg: &Message,
    compiled: &solana_message::compiled_instruction::CompiledInstruction,
) -> Result<Instruction> {
    let get = |i: u8| {
        msg.account_keys
            .get(usize::from(i))
            .copied()
            .ok_or_else(|| invalid("instruction account index"))
    };
    let mut accounts = Vec::with_capacity(compiled.accounts.len());
    for &n in &compiled.accounts {
        let pubkey = get(n)?;
        let i = usize::from(n);
        let sign = msg.is_signer(i);
        let writable = if sign {
            i < usize::from(
                msg.header.num_required_signatures - msg.header.num_readonly_signed_accounts,
            )
        } else {
            i < msg.account_keys.len() - usize::from(msg.header.num_readonly_unsigned_accounts)
        };
        accounts.push(Meta {
            pubkey,
            is_signer: sign,
            is_writable: writable,
        });
    }
    Ok(Instruction {
        program_id: get(compiled.program_id_index)?,
        accounts,
        data: compiled.data.clone(),
    })
}

impl Expected {
    pub(crate) fn payer(&self) -> Result<Pubkey> {
        self.payer
            .ok_or_else(|| invalid("explicit transaction payer required to sign"))
    }
    fn create_ata(&self, payer: Pubkey) -> Result<Option<Instruction>> {
        let owner = match self.action {
            ChainAction::CommitDelivery => return Ok(None),
            ChainAction::Fund => key(&self.terms.escrow_account)?,
            ChainAction::Release => from_hex(&self.terms.seller)?,
            ChainAction::Refund => from_hex(&self.terms.buyer)?,
        };
        let mint = key(&self.deployment.mint)?;
        Ok(Some(Instruction {
            program_id: key(ATA_PROGRAM)?,
            accounts: vec![
                Meta::new(payer, true),
                rw(ata(&owner, &mint)?),
                ro(owner),
                ro(mint),
                ro(SYSTEM),
                ro(key(TOKEN_PROGRAM)?),
            ],
            data: vec![1],
        }))
    }
    pub(crate) fn escrow_ix(&self, payer: Pubkey) -> Result<Instruction> {
        let t = &self.terms;
        let mut data = discriminator("global", self.action.name()).to_vec();
        let escrow = key(&t.escrow_account)?;
        let mint = key(&self.deployment.mint)?;
        let token = key(TOKEN_PROGRAM)?;
        let buyer = from_hex(&t.buyer)?;
        let seller = from_hex(&t.seller)?;
        let verifier = from_hex(&t.verifier)?;
        let accounts = match self.action {
            ChainAction::Fund => {
                ensure(payer == buyer, "fund fee payer")?;
                data.extend(self.nonce);
                for v in [
                    &t.order_id,
                    &t.request_id,
                    &t.quote_id,
                    &t.acceptance_id,
                    &t.policy_hash,
                    &t.verifier,
                ] {
                    data.extend(hex::<32>(v).map_err(invalid)?);
                }
                data.extend(self.principal()?.to_le_bytes());
                for time in self.times()? {
                    data.extend(time.to_le_bytes());
                }
                data.extend((t.environment.len() as u32).to_le_bytes());
                data.extend(t.environment.as_bytes());
                vec![
                    Meta::new(buyer, true),
                    signer(seller),
                    ro(key(&self.deployment.config)?),
                    rw(escrow),
                    ro(mint),
                    rw(key(&t.buyer_destination)?),
                    ro(key(&t.seller_destination)?),
                    rw(self.vault),
                    ro(token),
                    ro(SYSTEM),
                    ro(key(ATA_PROGRAM)?),
                ]
            }
            ChainAction::CommitDelivery => {
                data.extend(
                    self.delivery
                        .ok_or_else(|| invalid("missing delivery id"))?,
                );
                vec![signer(seller), rw(escrow)]
            }
            ChainAction::Release => {
                data.extend(self.nonce);
                data.extend(
                    self.delivery
                        .ok_or_else(|| invalid("missing delivery id"))?,
                );
                data.extend(self.verdict.ok_or_else(|| invalid("missing verdict id"))?);
                vec![
                    signer(verifier),
                    rw(escrow),
                    ro(mint),
                    rw(self.vault),
                    rw(key(&t.seller_destination)?),
                    ro(token),
                ]
            }
            ChainAction::Refund => {
                data.extend(self.nonce);
                vec![
                    signer(payer),
                    rw(escrow),
                    ro(mint),
                    rw(self.vault),
                    rw(key(&t.buyer_destination)?),
                    ro(token),
                ]
            }
        };
        Ok(Instruction {
            program_id: key(&self.deployment.program_id)?,
            accounts,
            data,
        })
    }
    pub(crate) fn build(&self, hash: &str, height: u64, slot: u64) -> Result<Prepared> {
        let payer = self.payer()?;
        let mut ix = Vec::new();
        if let Some(create) = self.create_ata(payer)? {
            ix.push(create);
        }
        ix.push(self.escrow_ix(payer)?);
        let msg = Message::new_with_blockhash(&ix, Some(&payer), &hash.parse().map_err(invalid)?);
        Ok(Prepared {
            message: STANDARD.encode(msg.serialize()),
            last_valid_block_height: height,
            context_slot: slot,
        })
    }
    /// Decode each instruction and reconstruct the expected message from the signed agreement.
    /// The comparison includes global privileges, unused accounts, order and instruction data.
    pub(crate) fn review_message(&self, msg: &Message) -> Result<usize> {
        let payer = msg.account_keys[0];
        ensure(
            self.payer.is_none_or(|expected| expected == payer),
            "different fee payer",
        )?;
        let mut canonical = Vec::new();
        let mut ata_seen = false;
        let mut escrow_index = None;
        for (i, c) in msg.instructions.iter().enumerate() {
            let ix = decoded_ix(msg, c)?;
            ensure(escrow_index.is_none(), "extra instruction after escrow")?;
            if ix.program_id == key(COMPUTE)? {
                ensure(
                    !ata_seen
                        && ix.accounts.is_empty()
                        && matches!(
                            ix.data.as_slice(),
                            [2, _, _, _, _] | [3, _, _, _, _, _, _, _, _]
                        ),
                    "compute budget instruction",
                )?;
                canonical.push(ix);
            } else if ix.program_id == key(ATA_PROGRAM)? {
                ensure(!ata_seen, "multiple ATA creates")?;
                canonical.push(
                    self.create_ata(payer)?
                        .ok_or_else(|| invalid("unexpected ATA create"))?,
                );
                ata_seen = true;
            } else {
                canonical.push(self.escrow_ix(payer)?);
                escrow_index = Some(i);
            }
        }
        let index = escrow_index.ok_or_else(|| invalid("missing escrow instruction"))?;
        let expected = Message::new_with_blockhash(&canonical, Some(&payer), &msg.recent_blockhash);
        ensure(
            expected == *msg,
            "transaction differs from the reviewed instruction, account list, data or privileges",
        )?;
        Ok(index)
    }
    fn prepared(&self, prepared: &Prepared) -> Result<(Message, Vec<u8>)> {
        ensure(
            prepared.message.len() <= MAX_WIRE.div_ceil(3) * 4,
            "message length",
        )?;
        let raw = STANDARD.decode(&prepared.message).map_err(invalid)?;
        let msg = decode_message(&raw)?;
        self.review_message(&msg)?;
        Ok((msg, raw))
    }
    pub(crate) fn sign_with(
        &self,
        prepared: &Prepared,
        public_hex: &str,
        sign: impl FnOnce(&[u8]) -> [u8; 64],
    ) -> Result<String> {
        self.payer()?;
        let (msg, raw) = self.prepared(prepared)?;
        authorize_fees(&msg)?;
        let pubkey = from_hex(public_hex)?;
        ensure(
            msg.account_keys[..usize::from(msg.header.num_required_signatures)].contains(&pubkey),
            "key is not a required signer",
        )?;
        let signature = sign(&raw);
        VerifyingKey::from_bytes(&pubkey.to_bytes())
            .map_err(invalid)?
            .verify_strict(&raw, &Signature::from_bytes(&signature))
            .map_err(invalid)?;
        Ok(bs58::encode(signature).into_string())
    }
    pub fn assemble(&self, prepared: &Prepared, signatures: &[String]) -> Result<SignedTx> {
        let (msg, raw) = self.prepared(prepared)?;
        ensure(
            signatures.len() == usize::from(msg.header.num_required_signatures),
            "missing transaction signatures",
        )?;
        let mut wire = vec![msg.header.num_required_signatures];
        for sig in signatures {
            wire.extend(base58::<64>(sig).map_err(invalid)?);
        }
        wire.extend(raw);
        let tx = SignedTx {
            signature: signatures[0].clone(),
            wire: STANDARD.encode(wire),
            last_valid_block_height: prepared.last_valid_block_height,
            context_slot: prepared.context_slot,
        };
        self.validate_signed(&tx)?;
        Ok(tx)
    }
    pub fn validate_signed(&self, tx: &SignedTx) -> Result<()> {
        let (msg, wire) = decode_wire(&tx.wire)?;
        ensure(
            base58::<64>(&tx.signature).map_err(invalid)?.as_slice() == &wire[1..65],
            "transaction locator disagrees with first signature",
        )?;
        self.review_message(&msg)?;
        Ok(())
    }
}

pub(crate) fn config_message(dep: &Deployment, authority: Pubkey, hash: &str) -> Result<Message> {
    let program = key(&dep.program_id)?;
    let pdata = Pubkey::find_program_address(&[program.as_ref()], &key(LOADER)?).0;
    let mut data = discriminator("global", "initialize_config").to_vec();
    data.extend(base58::<32>(&dep.genesis).map_err(invalid)?);
    let ix = Instruction {
        program_id: program,
        accounts: vec![
            Meta::new(authority, true),
            rw(key(&dep.config)?),
            ro(pdata),
            ro(key(&dep.mint)?),
            ro(key(TOKEN_PROGRAM)?),
            ro(SYSTEM),
        ],
        data,
    };
    Ok(Message::new_with_blockhash(
        &[ix],
        Some(&authority),
        &hash.parse().map_err(invalid)?,
    ))
}
