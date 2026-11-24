//! Review, sign and reconcile classic SPL-token escrow transactions against a pinned deployment.

mod backend;
pub use backend::{IntentJournal, NativeReview, OrderBackend};
mod evidence;
mod rpc;
mod transaction;
pub use rpc::Rpc;
pub use transaction::{Prepared, SignedTx};

use serde::{Deserialize, Serialize};
use settle463_core::{
    ident::{base58, decimal, hex, hex_string, timestamp},
    settlement::{
        check_delivery, check_pass_policy, check_verdict, Agreement, ChainOutcome, Review,
    },
    wire::{Body, Settlement},
    SignedMessage,
};
use sha2::{Digest, Sha256};
use solana_pubkey::Pubkey;

pub const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const ATA_PROGRAM: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
const SYSTEM: Pubkey = Pubkey::new_from_array([0; 32]);
const LOADER: &str = "BPFLoaderUpgradeab1e11111111111111111111111";
const COMPUTE: &str = "ComputeBudget111111111111111111111111111111";
const CLOCK: &str = "SysvarC1ock11111111111111111111111111111111";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid Solana evidence: {0}")]
    Invalid(String),
    #[error("Solana RPC unavailable: {0}")]
    Unavailable(String),
}
impl Error {
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }
}
pub type Result<T> = std::result::Result<T, Error>;
fn invalid(reason: impl std::fmt::Display) -> Error {
    Error::Invalid(reason.to_string())
}
fn ensure(condition: bool, reason: &str) -> Result<()> {
    if !condition {
        return Err(invalid(reason));
    }
    Ok(())
}
fn key(text: &str) -> Result<Pubkey> {
    Ok(Pubkey::new_from_array(base58(text).map_err(invalid)?))
}
fn from_hex(text: &str) -> Result<Pubkey> {
    Ok(Pubkey::new_from_array(hex(text).map_err(invalid)?))
}
fn discriminator(kind: &str, name: &str) -> [u8; 8] {
    let hash = Sha256::digest(format!("{kind}:{name}").as_bytes());
    let mut d = [0; 8];
    d.copy_from_slice(&hash[..8]);
    d
}
fn ata(owner: &Pubkey, mint: &Pubkey) -> Result<Pubkey> {
    Ok(Pubkey::find_program_address(
        &[owner.as_ref(), key(TOKEN_PROGRAM)?.as_ref(), mint.as_ref()],
        &key(ATA_PROGRAM)?,
    )
    .0)
}
pub fn associated_token_address(wallet: &str, mint: &str) -> Result<String> {
    Ok(ata(&key(wallet)?, &key(mint)?)?.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deployment {
    pub rpc_url: String,
    pub genesis: String,
    pub program_id: String,
    pub config: String,
    pub mint: String,
    pub token_program: String,
}
impl Deployment {
    pub fn environment(&self) -> String {
        format!(
            "solana:{}:{}:{}",
            self.genesis, self.program_id, self.config
        )
    }
    pub fn config_address(program_id: &str) -> Result<String> {
        Ok(
            Pubkey::find_program_address(&[b"settle463", b"config", b"v1"], &key(program_id)?)
                .0
                .to_string(),
        )
    }
    fn check(&self) -> Result<()> {
        key(&self.genesis)?;
        key(&self.mint)?;
        ensure(
            self.token_program == TOKEN_PROGRAM
                && self.config == Self::config_address(&self.program_id)?,
            "deployment pins",
        )
    }
    pub fn settlement(
        &self,
        buyer_hex: &str,
        seller_hex: &str,
        nonce_hex: &str,
    ) -> Result<Settlement> {
        self.check()?;
        let buyer = from_hex(buyer_hex)?;
        let seller = from_hex(seller_hex)?;
        let nonce = hex::<32>(nonce_hex).map_err(invalid)?;
        let escrow = Pubkey::find_program_address(
            &[b"settle463", b"escrow", b"v1", buyer.as_ref(), &nonce],
            &key(&self.program_id)?,
        )
        .0;
        let mint = key(&self.mint)?;
        Ok(Settlement::Solana {
            mint: self.mint.clone(),
            token_program: self.token_program.clone(),
            buyer_wallet: buyer.to_string(),
            seller_wallet: seller.to_string(),
            buyer_token_account: ata(&buyer, &mint)?.to_string(),
            seller_token_account: ata(&seller, &mint)?.to_string(),
            escrow_pda: escrow.to_string(),
            vault_token_account: ata(&escrow, &mint)?.to_string(),
            config: self.config.clone(),
            program_id: self.program_id.clone(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainAction {
    Fund,
    CommitDelivery,
    Release,
    Refund,
}

impl ChainAction {
    pub fn name(self) -> &'static str {
        match self {
            Self::Fund => "fund",
            Self::CommitDelivery => "commit_delivery",
            Self::Release => "release",
            Self::Refund => "refund",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Expected {
    pub(crate) deployment: Deployment,
    pub(crate) terms: Review,
    nonce: [u8; 32],
    pub(crate) action: ChainAction,
    payer: Option<Pubkey>,
    delivery: Option<[u8; 32]>,
    verdict: Option<[u8; 32]>,
    vault: Pubkey,
}
impl Expected {
    pub fn new(
        deployment: &Deployment,
        agreement: &Agreement,
        action: ChainAction,
        delivery: Option<&SignedMessage>,
        verdict: Option<&SignedMessage>,
        fee_payer: &str,
    ) -> Result<Self> {
        let env = deployment.environment();
        let (r, q) = agreement.terms(&env).map_err(invalid)?;
        let settlement = deployment.settlement(&r.buyer, &r.seller, &r.nonce)?;
        ensure(
            q.settlement == settlement
                && q.asset.id == deployment.mint
                && q.asset.decimals == "6"
                && q.asset.kind == "spl-token",
            "signed agreement deployment or token accounts",
        )?;
        let Settlement::Solana {
            vault_token_account,
            ..
        } = settlement
        else {
            return Err(invalid("Solana settlement required"));
        };
        let terms = Review::from_agreement(agreement, &env).map_err(invalid)?;
        let payer = if fee_payer.is_empty() {
            None
        } else {
            Some(key(fee_payer)?)
        };
        if action == ChainAction::Fund {
            ensure(
                payer.is_none_or(|p| Some(p) == from_hex(&r.buyer).ok()),
                "fund fee payer must be buyer",
            )?;
        }
        if let Some(d) = delivery {
            check_delivery(&terms, d).map_err(invalid)?;
        }
        if let Some(v) = verdict {
            check_verdict(
                &terms,
                delivery.ok_or_else(|| invalid("missing delivery"))?,
                v,
            )
            .map_err(invalid)?;
            check_pass_policy(&q.artifact, v).map_err(invalid)?;
        }
        let decision = verdict.and_then(|v| match &v.unsigned().payload {
            Body::Verdict(b) => Some(b.decision.as_str()),
            _ => None,
        });
        ensure(
            match action {
                ChainAction::Fund => delivery.is_none() && verdict.is_none(),
                ChainAction::CommitDelivery => delivery.is_some() && verdict.is_none(),
                ChainAction::Release => delivery.is_some() && decision == Some("PASS"),
                ChainAction::Refund => decision.is_none() || decision == Some("FAIL"),
            },
            "action evidence",
        )?;
        Ok(Self {
            deployment: deployment.clone(),
            terms,
            action,
            nonce: hex(&r.nonce).map_err(invalid)?,
            delivery: delivery.map(|m| *m.id()),
            verdict: verdict.map(|m| *m.id()),
            vault: key(&vault_token_account)?,
            payer,
        })
    }
    pub fn review(&self) -> &Review {
        &self.terms
    }
    pub fn action(&self) -> ChainAction {
        self.action
    }
    pub fn vault(&self) -> String {
        self.vault.to_string()
    }
    fn principal(&self) -> Result<u64> {
        decimal(&self.terms.principal).map_err(invalid)
    }
    fn times(&self) -> Result<[i64; 3]> {
        let t = &self.terms.deadlines;
        Ok([
            timestamp(&t.fund_by).map_err(invalid)?,
            timestamp(&t.deliver_by).map_err(invalid)?,
            timestamp(&t.resolve_by).map_err(invalid)?,
        ])
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    pub outcome: ChainOutcome,
    pub slot: Option<u64>,
    pub clock: Option<i64>,
}
impl Resolution {
    fn status(outcome: ChainOutcome) -> Self {
        Self {
            outcome,
            slot: None,
            clock: None,
        }
    }
}

#[cfg(test)]
mod tests;
