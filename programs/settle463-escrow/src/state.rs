use anchor_lang::prelude::*;

pub const FUNDED: u8 = 0;
pub const DELIVERED: u8 = 1;
pub const PAID: u8 = 2;
pub const REFUNDED: u8 = 3;
pub const DECIMALS: u8 = 6;

#[account]
#[derive(InitSpace)]
pub struct Config {
    pub bump: u8,
    pub mint: Pubkey,
    pub token_program: Pubkey,
    pub decimals: u8,
    pub genesis_hash: [u8; 32],
}

#[derive(AnchorDeserialize, AnchorSerialize, Clone)]
pub struct FundArgs {
    pub request_nonce: [u8; 32],
    pub order_id: [u8; 32],
    pub request_id: [u8; 32],
    pub quote_id: [u8; 32],
    pub acceptance_id: [u8; 32],
    pub policy_hash: [u8; 32],
    pub verifier: Pubkey,
    pub principal: u64,
    pub fund_by: i64,
    pub deliver_by: i64,
    pub resolve_by: i64,
    pub environment: String,
}

// InitSpace = 14*32 + 4*8 + 4 one-byte state/flag/bump fields + 2*32 IDs
// + 1 environment length + 141 environment bytes = 690; the discriminator adds 8.
#[account]
#[derive(InitSpace)]
pub struct Escrow {
    pub order_id: [u8; 32],
    pub request_id: [u8; 32],
    pub quote_id: [u8; 32],
    pub acceptance_id: [u8; 32],
    pub policy_hash: [u8; 32],
    pub buyer: Pubkey,
    pub seller: Pubkey,
    pub verifier: Pubkey,
    pub mint: Pubkey,
    pub buyer_destination: Pubkey,
    pub seller_destination: Pubkey,
    pub vault: Pubkey,
    pub token_program: Pubkey,
    pub config: Pubkey,
    pub principal: u64,
    pub fund_by: i64,
    pub deliver_by: i64,
    pub resolve_by: i64,
    pub state: u8,
    pub delivery_id: [u8; 32],
    pub delivery_set: u8,
    pub verdict_id: [u8; 32],
    pub verdict_set: u8,
    pub bump: u8,
    pub environment_len: u8,
    pub environment: [u8; 141],
}

impl Escrow {
    pub(crate) fn require_open(&self) -> Result<()> {
        require!(
            !matches!(self.state, PAID | REFUNDED),
            EscrowError::AlreadyComplete
        );
        Ok(())
    }

    pub(crate) fn allow_delivery(&self, now: i64) -> Result<()> {
        self.require_open()?;
        require!(
            self.state == FUNDED && self.delivery_set == 0,
            EscrowError::BadState
        );
        require!(now < self.deliver_by, EscrowError::Deadline);
        Ok(())
    }

    pub(crate) fn allow_release(&self, now: i64, delivery_id: &[u8; 32]) -> Result<()> {
        self.require_open()?;
        require!(self.delivery_set == 1, EscrowError::DeliveryMissing);
        require!(self.state == DELIVERED, EscrowError::BadState);
        require!(
            self.delivery_id == *delivery_id,
            EscrowError::DeliveryMissing
        );
        require!(now < self.resolve_by, EscrowError::Deadline);
        Ok(())
    }

    pub(crate) fn allow_refund(&self, now: i64) -> Result<()> {
        self.require_open()?;
        let deadline = match (self.state, self.delivery_set) {
            (FUNDED, 0) => self.deliver_by,
            (DELIVERED, 1) => self.resolve_by,
            _ => return err!(EscrowError::BadState),
        };
        require!(now >= deadline, EscrowError::Deadline);
        Ok(())
    }
}

#[error_code]
pub enum EscrowError {
    #[msg("Wrong signer or upgrade authority")]
    Unauthorized,
    #[msg("Instruction does not apply to this state")]
    BadState,
    #[msg("Ledger time does not permit this action")]
    Deadline,
    #[msg("Token transfer delta differs from the principal")]
    Amount,
    #[msg("Mint or decimals differ from the deployment")]
    Mint,
    #[msg("Token account, mint, owner or destination differs from the agreement")]
    Destination,
    #[msg("Escrow is already terminal")]
    AlreadyComplete,
    #[msg("Buyer, seller and verifier must be distinct")]
    RoleCollision,
    #[msg("Principal must be positive")]
    ZeroPrincipal,
    #[msg("Deployment requires the classic SPL Token program")]
    BadTokenProgram,
    #[msg("Deployment configuration or environment mismatch")]
    BadDeployment,
    #[msg("Required committed delivery is missing or differs")]
    DeliveryMissing,
    #[msg("Deadlines must be strictly increasing")]
    DeadlineOrder,
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_lang::Discriminator;

    fn escrow(state: u8, delivery_set: u8) -> Escrow {
        let bytes = [0u8; Escrow::INIT_SPACE];
        let mut escrow = Escrow::deserialize(&mut bytes.as_slice()).unwrap();
        escrow.state = state;
        escrow.delivery_set = delivery_set;
        escrow.deliver_by = 25;
        escrow.resolve_by = 50;
        escrow
    }

    #[test]
    fn allocated_accounts_match_published_offsets() {
        assert_eq!(Config::INIT_SPACE + Config::DISCRIMINATOR.len(), 106);
        assert_eq!(Escrow::INIT_SPACE + Escrow::DISCRIMINATOR.len(), 698);
        let mut sample = escrow(DELIVERED, 1);
        sample.principal = 0x0102030405060708;
        sample.environment_len = 3;
        sample.environment[..3].copy_from_slice(b"abc");
        let mut out = Vec::new();
        sample.try_serialize(&mut out).unwrap();
        assert_eq!(out.len(), 698);
        assert_eq!(&out[456..464], &sample.principal.to_le_bytes());
        assert_eq!(out[488], DELIVERED);
        assert_eq!(out[521], 1);
        assert_eq!(&out[556..560], b"\x03abc");
    }

    fn code(result: Result<()>) -> u32 {
        match result.unwrap_err() {
            anchor_lang::error::Error::AnchorError(err) => err.error_code_number,
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn mint_requires_classic_ownership_initialization_and_six_decimals() {
        for (owner, decimals, initialized, accepted) in [
            (anchor_spl::token::ID, 6, true, true),
            (anchor_spl::token::ID, 9, true, false),
            (anchor_spl::token::ID, 6, false, false),
            (Pubkey::default(), 6, true, false),
        ] {
            let key = Pubkey::new_unique();
            let mut lamports = 1;
            let mut data = [0u8; anchor_spl::token::Mint::LEN];
            data[44] = decimals;
            data[45] = u8::from(initialized);
            let account =
                AccountInfo::new(&key, false, false, &mut lamports, &mut data, &owner, false);
            let result = crate::check_mint(&account);
            if accepted {
                result.unwrap();
            } else {
                assert_eq!(code(result), 6004);
            }
        }
    }

    #[test]
    fn terminal_state_and_ledger_deadline_boundaries() {
        for (time, allowed) in [(24, true), (25, false), (26, false)] {
            let funded = escrow(FUNDED, 0);
            assert_eq!(funded.allow_delivery(time).is_ok(), allowed);
            assert_eq!(funded.allow_refund(time).is_ok(), !allowed);
        }
        let delivered = escrow(DELIVERED, 1);
        for (time, allowed) in [(49, true), (50, false), (51, false)] {
            assert_eq!(delivered.allow_release(time, &[0; 32]).is_ok(), allowed);
            assert_eq!(delivered.allow_refund(time).is_ok(), !allowed);
        }
        assert_eq!(code(delivered.allow_release(49, &[5; 32])), 6011);
        for state in [PAID, REFUNDED] {
            let terminal = escrow(state, 1);
            assert_eq!(code(terminal.allow_delivery(0)), 6006);
            assert_eq!(code(terminal.allow_release(0, &[0; 32])), 6006);
            assert_eq!(code(terminal.allow_refund(100)), 6006);
        }
    }
}
