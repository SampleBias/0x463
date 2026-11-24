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
