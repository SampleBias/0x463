use crate::AnyResult;
use serde_json::{json, Value};
use settle463_core::{
    ident::{base58, decimal, hex_string},
    receipt::{ReceiptTrust, VerifiedSolanaBundle},
    settlement::{Agreement, ChainOutcome},
    wire::{Request, Settlement},
};
use settle463_sdk::{Error, Identity, LedgerAction, LedgerSigning, Result as SdkResult};
use settle463_solana::{ChainAction, Deployment, Expected, Prepared, Rpc, TOKEN_PROGRAM};
use std::{fs, path::Path};

pub struct Validator {
    pub deployment: Deployment,
    pub rpc: Rpc,
}
fn sdk<T>(r: settle463_solana::Result<T>) -> SdkResult<T> {
    r.map_err(|e| {
        if e.is_unavailable() {
            Error::Unavailable
        } else {
            Error::PolicyRejected
        }
    })
}
impl Validator {
    pub fn load(path: &Path) -> AnyResult<Self> {
        let deployment: Deployment = serde_json::from_slice(&fs::read(path)?)?;
        let rpc = Rpc::new(&deployment)?;
        rpc.validate_deployment()?;
        Ok(Self { deployment, rpc })
    }
    fn expected(&self, a: &Agreement, act: LedgerAction<'_>) -> SdkResult<Expected> {
        let (_, q) = a.terms(&self.deployment.environment())?;
        let Settlement::Solana {
            buyer_wallet,
            seller_wallet,
            ..
        } = &q.settlement
        else {
            return Err(Error::Environment);
        };
        let verifier = bs58::encode(settle463_core::ident::hex::<32>(&q.verifier)?).into_string();
        let (kind, d, v, payer) = match act {
            LedgerAction::Fund => (ChainAction::Fund, None, None, buyer_wallet),
            LedgerAction::CommitDelivery(d) => {
                (ChainAction::CommitDelivery, Some(d), None, seller_wallet)
            }
            LedgerAction::Release(d, v) => (ChainAction::Release, Some(d), Some(v), &verifier),
            LedgerAction::Refund(d) => (ChainAction::Refund, d, None, buyer_wallet),
        };
        sdk(Expected::new(&self.deployment, a, kind, d, v, payer))
    }
}
fn draft(v: &Value) -> SdkResult<Prepared> {
    let text = |name: &str| v[name].as_str().ok_or(Error::Linkage);
    Ok(Prepared {
        message: text("message")?.into(),
        context_slot: decimal(text("context_slot")?)?,
        last_valid_block_height: decimal(text("last_valid_block_height")?)?,
    })
}
impl LedgerSigning for Validator {
    fn settlement(&self, r: &Request) -> SdkResult<Settlement> {
        sdk(self.deployment.settlement(&r.buyer, &r.seller, &r.nonce))
    }
    fn sign(
        &self,
        a: &Agreement,
        act: LedgerAction<'_>,
        v: &Value,
        public_hex: &str,
        sign: &dyn Fn(&[u8]) -> [u8; 64],
    ) -> SdkResult<String> {
        sdk(self
            .rpc
            .sign_with(&self.expected(a, act)?, &draft(v)?, public_hex, sign))
    }
    fn assemble(
        &self,
        a: &Agreement,
        act: LedgerAction<'_>,
        v: &Value,
        sigs: &[String],
    ) -> SdkResult<Value> {
        let signed = sdk(self.expected(a, act)?.assemble(&draft(v)?, sigs))?;
        Ok(
            json!({"signature":signed.signature,"wire":signed.wire,"context_slot":signed.context_slot.to_string(),
            "last_valid_block_height":signed.last_valid_block_height.to_string()}),
        )
    }
}

pub fn fixture_key(deployment: &Path, session: &Path, role: &str) -> AnyResult<Identity> {
    let file = deployment
        .parent()
        .ok_or("invalid deployment path")?
        .join(format!("{role}.json"));
    let bytes: Vec<u8> = serde_json::from_slice(&fs::read(file)?)?;
    let pair: [u8; 64] = bytes
        .try_into()
        .map_err(|_| "invalid Solana keypair length")?;
    let seed = pair[..32].try_into()?;
    let identity = Identity::from_seed(&seed);
    if identity.public_hex() != hex_string(&pair[32..]) {
        return Err("invalid Solana keypair".into());
    }
    let name = if role == "coordinator" {
        "issuer"
    } else {
        role
    };
    crate::session::private_write(
        &session.join(format!("{name}.key")),
        hex_string(&seed).as_bytes(),
    )?;
    Ok(identity)
}

pub fn online(
    bundle: VerifiedSolanaBundle,
    trust: &ReceiptTrust,
    rpc_url: &str,
) -> (u8, &'static str) {
    let l = &bundle.ledger;
    let dep = Deployment {
        rpc_url: rpc_url.into(),
        genesis: l.genesis.clone(),
        program_id: l.program_id.clone(),
        config: l.config.clone(),
        mint: l.mint.clone(),
        token_program: TOKEN_PROGRAM.into(),
    };
    if dep.environment() != trust.environment {
        return (3, "untrusted");
    }
    let run = || -> settle463_solana::Result<(u8, &'static str)> {
        let rpc = Rpc::new(&dep)?;
        rpc.validate_deployment()?;
        let mut previous = 0;
        let terminal = if l.terminal_instruction == "release" {
            ChainAction::Release
        } else {
            ChainAction::Refund
        };
        let (_, quote) = bundle
            .agreement
            .terms(&trust.environment)
            .map_err(|e| settle463_solana::Error::Invalid(e.to_string()))?;
        let Settlement::Solana { buyer_wallet, .. } = &quote.settlement else {
            return Ok((2, "invalid_evidence"));
        };
        for (act, sig) in [
            (ChainAction::Fund, Some(&l.funding_signature)),
            (ChainAction::CommitDelivery, l.delivery_signature.as_ref()),
            (terminal, Some(&l.terminal_signature)),
        ] {
            let Some(sig) = sig else {
                continue;
            };
            let delivery = if act != ChainAction::Fund {
                bundle.delivery.as_ref()
            } else {
                None
            };
            let verdict = if act == ChainAction::Release {
                bundle.verdict.as_ref()
            } else {
                None
            };
            let e = Expected::new(
                &dep,
                &bundle.agreement,
                act,
                delivery,
                verdict,
                if act == ChainAction::Fund {
                    buyer_wallet
                } else {
                    ""
                },
            )?;
            let resolution = rpc.reconcile(&e, sig, None)?;
            match resolution.outcome {
                ChainOutcome::FinalizedSuccess => {
                    let Some(slot) = resolution.slot else {
                        return Ok((5, "unverifiable"));
                    };
                    if slot < previous || (act == terminal && slot.to_string() != l.finalized_slot)
                    {
                        return Ok((2, "invalid_evidence"));
                    }
                    previous = slot;
                }
                ChainOutcome::FinalizedFailure => return Ok((2, "invalid_evidence")),
                ChainOutcome::Pending => return Ok((4, "pending")),
                ChainOutcome::Expired | ChainOutcome::Unknown => return Ok((5, "unverifiable")),
            }
        }
        Ok((0, "ledger_verified"))
    };
    match run() {
        Ok(result) => result,
        Err(e) if e.is_unavailable() => (5, "unverifiable"),
        Err(_) => (2, "invalid_evidence"),
    }
}
pub fn check_pins(trust: &Value, env: &str) -> AnyResult<()> {
    let pins: Vec<_> = env.split(':').collect();
    if pins.len() != 4 || trust["genesis"] != pins[1] || trust["program_id"] != pins[2] {
        return Err("trust file must separately pin the deployment genesis and program ID".into());
    }
    base58::<32>(pins[1])?;
    base58::<32>(pins[2])?;
    Ok(())
}
