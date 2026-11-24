use super::*;
use crate::{artifact::digest, ident::base58, wire::Settlement};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SolanaLedger {
    pub ledger_kind: String,
    pub genesis: String,
    pub program_id: String,
    pub config: String,
    pub order_pda: String,
    pub mint: String,
    pub amount: String,
    pub beneficiary: String,
    pub vault: String,
    pub funding_signature: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_signature: Option<String>,
    pub terminal_signature: String,
    pub terminal_instruction: String,
    pub finalized_slot: String,
    pub evidence_digest: String,
}

impl SolanaLedger {
    fn digest(&self) -> Result<String, ReceiptError> {
        let mut value = serde_json::to_value(self).map_err(|_| InvalidEvidence)?;
        value
            .as_object_mut()
            .ok_or(InvalidEvidence)?
            .remove("evidence_digest");
        Ok(digest(
            &canonical::canonical(&value).map_err(|_| InvalidEvidence)?,
        ))
    }

    pub fn seal(mut self) -> Result<Self, ReceiptError> {
        self.evidence_digest = self.digest()?;
        Ok(self)
    }

    fn validate(&self, c: &Checked) -> Result<(), ReceiptError> {
        let review = &c.view.review;
        let (_, quote) = c
            .agreement
            .terms(&review.environment)
            .map_err(|_| InvalidEvidence)?;
        let Settlement::Solana {
            program_id,
            config,
            escrow_pda,
            mint,
            vault_token_account,
            ..
        } = &quote.settlement
        else {
            return Err(InvalidEvidence);
        };
        let instruction = if c.view.state == OrderState::Paid {
            "release"
        } else {
            "refund"
        };
        if self.ledger_kind != "solana"
            || self.evidence_digest != self.digest()?
            || self.program_id != *program_id
            || self.config != *config
            || format!(
                "solana:{}:{}:{}",
                self.genesis, self.program_id, self.config
            ) != review.environment
            || self.order_pda != *escrow_pda
            || self.vault != *vault_token_account
            || self.mint != *mint
            || self.amount != review.principal
            || self.beneficiary != c.receipt.destination
            || self.terminal_instruction != instruction
            || self.delivery_signature.is_some() != c.delivery.is_some()
            || self.funding_signature == self.terminal_signature
        {
            return Err(InvalidEvidence);
        }
        decimal(&self.finalized_slot).map_err(|_| InvalidEvidence)?;
        base58::<64>(&self.funding_signature).map_err(|_| InvalidEvidence)?;
        base58::<64>(&self.terminal_signature).map_err(|_| InvalidEvidence)?;
        if let Some(sig) = &self.delivery_signature {
            base58::<64>(sig).map_err(|_| InvalidEvidence)?;
            if sig == &self.funding_signature || sig == &self.terminal_signature {
                return Err(InvalidEvidence);
            }
        }
        Ok(())
    }
}

pub struct VerifiedSolanaBundle {
    pub agreement: Agreement,
    pub delivery: Option<SignedMessage>,
    pub verdict: Option<SignedMessage>,
    pub receipt: Receipt,
    pub ledger: SolanaLedger,
}

/// Checks signed evidence and ledger references. Finality and token movement require RPC reconciliation.
pub fn verify_solana_bundle(
    bytes: &[u8],
    trust: &ReceiptTrust,
) -> Result<VerifiedSolanaBundle, ReceiptError> {
    let b: Bundle<SolanaLedger> = decode(bytes, "solana:")?;
    let c = checked(&b, None, &b.ledger.terminal_signature)?;
    let expected = terminal_receipt(
        &c.view,
        c.verdict
            .as_ref()
            .filter(|_| c.view.state == OrderState::Refunded),
        "solana",
        "finalized",
        None,
    )?;
    if expected != c.receipt {
        return Err(InvalidEvidence);
    }
    b.ledger.validate(&c)?;
    if &issuer_trust(&expected) != trust {
        return Err(ReceiptError::Untrusted);
    }
    Ok(VerifiedSolanaBundle {
        agreement: c.agreement,
        delivery: c.delivery,
        verdict: c.verdict,
        receipt: c.receipt,
        ledger: b.ledger,
    })
}

pub fn build_solana_bundle(
    agreement: &Agreement,
    delivery: Option<&SignedMessage>,
    verdict: Option<&SignedMessage>,
    receipt: &SignedMessage,
    observations: &[SignedMessage],
    ledger: &SolanaLedger,
) -> Result<Vec<u8>, ReceiptError> {
    let Body::Receipt(body) = &receipt.unsigned().payload else {
        return Err(InvalidEvidence);
    };
    let bytes = assemble(agreement, delivery, verdict, receipt, observations, ledger)?;
    verify_solana_bundle(&bytes, &issuer_trust(body))?;
    Ok(bytes)
}
