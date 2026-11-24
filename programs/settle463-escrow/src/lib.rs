//! Six-decimal classic SPL Token escrow for the 0x463 signed settlement protocol.

use anchor_lang::{prelude::*, solana_program::bpf_loader_upgradeable, system_program};
use anchor_spl::{
    associated_token::{self, get_associated_token_address_with_program_id, AssociatedToken},
    token::{self, Mint, TokenAccount, TransferChecked},
};
mod state;
pub use state::*;

declare_id!("3CEGonqu1gQEsT2vJGfesoW1p25NcYuRp7McC4yFSrbd");

#[program]
pub mod settle463_escrow {
    use super::*;

    pub fn initialize_config(ctx: Context<InitializeConfig>, genesis_hash: [u8; 32]) -> Result<()> {
        let a = ctx.accounts;
        let (data_address, _) =
            Pubkey::find_program_address(&[ctx.program_id.as_ref()], &bpf_loader_upgradeable::ID);
        require_keys_eq!(
            a.program_data.key(),
            data_address,
            EscrowError::BadDeployment
        );
        require!(
            a.program_data.upgrade_authority_address == Some(a.authority.key()),
            EscrowError::Unauthorized
        );
        check_token_program(&a.token_program)?;
        check_mint(&a.mint)?;
        a.config.set_inner(Config {
            bump: ctx.bumps.config,
            mint: a.mint.key(),
            token_program: token::ID,
            decimals: DECIMALS,
            genesis_hash,
        });
        Ok(())
    }

    pub fn fund(ctx: Context<Fund>, args: FundArgs) -> Result<()> {
        let a = ctx.accounts;
        if a.escrow.owner == ctx.program_id {
            let old = Escrow::try_deserialize(&mut &a.escrow.try_borrow_data()?[..])?;
            old.require_open()?;
            return err!(EscrowError::BadState);
        }
        require!(
            a.escrow.data_is_empty() && *a.escrow.owner == system_program::ID,
            EscrowError::BadState
        );
        let buyer = a.buyer.key();
        let seller = a.seller.key();
        require!(args.principal > 0, EscrowError::ZeroPrincipal);
        require!(
            args.fund_by < args.deliver_by && args.deliver_by < args.resolve_by,
            EscrowError::DeadlineOrder
        );
        require!(
            Clock::get()?.unix_timestamp < args.fund_by,
            EscrowError::Deadline
        );
        require!(
            buyer != seller && buyer != args.verifier && seller != args.verifier,
            EscrowError::RoleCollision
        );
        check_token_program(&a.token_program)?;
        require_keys_eq!(
            a.config.token_program,
            token::ID,
            EscrowError::BadTokenProgram
        );
        require_keys_eq!(a.mint.key(), a.config.mint, EscrowError::Mint);
        require!(a.config.decimals == DECIMALS, EscrowError::Mint);
        check_mint(&a.mint)?;
        let expected = format!(
            "solana:{}:{}:{}",
            Pubkey::new_from_array(a.config.genesis_hash),
            ctx.program_id,
            a.config.key()
        );
        require!(
            args.environment.len() <= 141 && args.environment == expected,
            EscrowError::BadDeployment
        );
        let mut environment = [0u8; 141];
        environment[..expected.len()].copy_from_slice(expected.as_bytes());
        let escrow_key = a.escrow.key();
        let mint = a.mint.key();
        check_ata(&a.buyer_destination, &buyer, &mint)?;
        require_keys_eq!(
            a.vault.key(),
            ata(&escrow_key, &mint),
            EscrowError::Destination
        );
        require_keys_eq!(
            a.seller_destination.key(),
            ata(&seller, &mint),
            EscrowError::Destination
        );
        if !a.seller_destination.data_is_empty()
            || *a.seller_destination.owner != system_program::ID
        {
            check_ata(&a.seller_destination, &seller, &mint)?;
        }
        let bump = [ctx.bumps.escrow];
        let seeds = escrow_seeds(&buyer, &args.request_nonce, &bump);
        allocate_escrow(&a.escrow, &a.buyer, &[&seeds])?;
        if *a.vault.owner == system_program::ID && a.vault.data_is_empty() {
            associated_token::create_idempotent(CpiContext::new(
                a.associated_token_program.key(),
                associated_token::Create {
                    payer: a.buyer.to_account_info(),
                    associated_token: a.vault.to_account_info(),
                    authority: a.escrow.to_account_info(),
                    mint: a.mint.to_account_info(),
                    system_program: a.system_program.to_account_info(),
                    token_program: a.token_program.to_account_info(),
                },
            ))?;
        }
        check_ata(&a.vault, &escrow_key, &mint)?;
        move_principal(
            TransferChecked {
                from: a.buyer_destination.to_account_info(),
                mint: a.mint.to_account_info(),
                to: a.vault.to_account_info(),
                authority: a.buyer.to_account_info(),
            },
            args.principal,
            &[],
        )?;
        Escrow {
            order_id: args.order_id,
            request_id: args.request_id,
            quote_id: args.quote_id,
            acceptance_id: args.acceptance_id,
            policy_hash: args.policy_hash,
            buyer,
            seller,
            verifier: args.verifier,
            mint,
            buyer_destination: a.buyer_destination.key(),
            seller_destination: a.seller_destination.key(),
            vault: a.vault.key(),
            token_program: token::ID,
            config: a.config.key(),
            principal: args.principal,
            fund_by: args.fund_by,
            deliver_by: args.deliver_by,
            resolve_by: args.resolve_by,
            state: FUNDED,
            delivery_id: [0; 32],
            delivery_set: 0,
            verdict_id: [0; 32],
            verdict_set: 0,
            bump: bump[0],
            environment_len: expected.len() as u8,
            environment,
        }
        .try_serialize(&mut &mut a.escrow.try_borrow_mut_data()?[..])?;
        Ok(())
    }

    pub fn commit_delivery(ctx: Context<CommitDelivery>, delivery_id: [u8; 32]) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow;
        escrow.require_open()?;
        require_keys_eq!(
            ctx.accounts.seller.key(),
            escrow.seller,
            EscrowError::Unauthorized
        );
        escrow.allow_delivery(Clock::get()?.unix_timestamp)?;
        escrow.delivery_id = delivery_id;
        escrow.delivery_set = 1;
        escrow.state = DELIVERED;
        Ok(())
    }

    pub fn release(
        ctx: Context<Settle>,
        request_nonce: [u8; 32],
        delivery_id: [u8; 32],
        verdict_id: [u8; 32],
    ) -> Result<()> {
        let a = ctx.accounts;
        a.escrow.require_open()?;
        require_keys_eq!(a.caller.key(), a.escrow.verifier, EscrowError::Unauthorized);
        a.escrow
            .allow_release(Clock::get()?.unix_timestamp, &delivery_id)?;
        a.pay(
            &request_nonce,
            &a.escrow.seller,
            &a.escrow.seller_destination,
        )?;
        a.escrow.verdict_id = verdict_id;
        a.escrow.verdict_set = 1;
        a.escrow.state = PAID;
        Ok(())
    }

    pub fn refund(ctx: Context<Settle>, request_nonce: [u8; 32]) -> Result<()> {
        let a = ctx.accounts;
        a.escrow.allow_refund(Clock::get()?.unix_timestamp)?;
        a.pay(&request_nonce, &a.escrow.buyer, &a.escrow.buyer_destination)?;
        a.escrow.state = REFUNDED;
        Ok(())
    }
}

fn escrow_seeds<'a>(buyer: &'a Pubkey, nonce: &'a [u8; 32], bump: &'a [u8; 1]) -> [&'a [u8]; 6] {
    [b"settle463", b"escrow", b"v1", buyer.as_ref(), nonce, bump]
}

fn check_token_program(program: &AccountInfo<'_>) -> Result<()> {
    require!(
        program.key == &token::ID && program.executable,
        EscrowError::BadTokenProgram
    );
    Ok(())
}

fn check_mint(account: &AccountInfo<'_>) -> Result<()> {
    require_keys_eq!(*account.owner, token::ID, EscrowError::Mint);
    let mint = Mint::try_deserialize(&mut &account.try_borrow_data()?[..])
        .map_err(|_| error!(EscrowError::Mint))?;
    require!(mint.decimals == DECIMALS, EscrowError::Mint);
    Ok(())
}

fn ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    get_associated_token_address_with_program_id(owner, mint, &token::ID)
}

fn token_account(account: &AccountInfo<'_>) -> Result<TokenAccount> {
    require_keys_eq!(*account.owner, token::ID, EscrowError::Destination);
    TokenAccount::try_deserialize(&mut &account.try_borrow_data()?[..])
        .map_err(|_| error!(EscrowError::Destination))
}

fn check_ata(account: &AccountInfo<'_>, owner: &Pubkey, mint: &Pubkey) -> Result<()> {
    require_keys_eq!(*account.key, ata(owner, mint), EscrowError::Destination);
    let state = token_account(account)?;
    require_keys_eq!(state.mint, *mint, EscrowError::Destination);
    require_keys_eq!(state.owner, *owner, EscrowError::Destination);
    Ok(())
}

fn move_principal(accounts: TransferChecked<'_>, principal: u64, seeds: &[&[&[u8]]]) -> Result<()> {
    let source = accounts.from.clone();
    let destination = accounts.to.clone();
    let before_source = token_account(&source)?.amount;
    let before_destination = token_account(&destination)?.amount;
    token::transfer_checked(
        CpiContext::new_with_signer(token::ID, accounts, seeds),
        principal,
        DECIMALS,
    )?;
    let after_source = token_account(&source)?.amount;
    let after_destination = token_account(&destination)?.amount;
    require!(
        before_source.checked_sub(after_source) == Some(principal)
            && after_destination.checked_sub(before_destination) == Some(principal),
        EscrowError::Amount
    );
    Ok(())
}

// A prefunded system-owned PDA needs only the remaining rent. Inspecting the account
// before allocation also gives terminal fund replays their stable custom error.
fn allocate_escrow<'info>(
    escrow: &UncheckedAccount<'info>,
    payer: &Signer<'info>,
    seeds: &[&[&[u8]]],
) -> Result<()> {
    let space = Escrow::DISCRIMINATOR.len() + Escrow::INIT_SPACE;
    let rent = Rent::get()?
        .minimum_balance(space)
        .saturating_sub(escrow.lamports());
    if rent > 0 {
        system_program::transfer(
            CpiContext::new(
                system_program::ID,
                system_program::Transfer {
                    from: payer.to_account_info(),
                    to: escrow.to_account_info(),
                },
            ),
            rent,
        )?;
    }
    system_program::allocate(
        CpiContext::new_with_signer(
            system_program::ID,
            system_program::Allocate {
                account_to_allocate: escrow.to_account_info(),
            },
            seeds,
        ),
        space as u64,
    )?;
    system_program::assign(
        CpiContext::new_with_signer(
            system_program::ID,
            system_program::Assign {
                account_to_assign: escrow.to_account_info(),
            },
            seeds,
        ),
        &crate::ID,
    )
}

#[derive(Accounts)]
pub struct InitializeConfig<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(init, payer=authority, space=8+Config::INIT_SPACE, seeds=[b"settle463", b"config", b"v1"], bump)]
    pub config: Account<'info, Config>,
    pub program_data: Account<'info, ProgramData>,
    /// CHECK: Classic token ownership, initialized mint data and decimals are checked in the handler.
    pub mint: UncheckedAccount<'info>,
    /// CHECK: Must be the executable classic SPL Token program.
    pub token_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(args: FundArgs)]
pub struct Fund<'info> {
    #[account(mut)]
    pub buyer: Signer<'info>,
    pub seller: Signer<'info>,
    #[account(seeds=[b"settle463", b"config", b"v1"], bump=config.bump)]
    pub config: Account<'info, Config>,
    /// CHECK: Canonical PDA, checked for replay before atomic allocation and serialization.
    #[account(mut, seeds=[b"settle463", b"escrow", b"v1", buyer.key().as_ref(), &args.request_nonce], bump)]
    pub escrow: UncheckedAccount<'info>,
    /// CHECK: Matched to the config and decoded as an initialized six-decimal classic mint.
    pub mint: UncheckedAccount<'info>,
    /// CHECK: Derived address, owner, mint and token program are verified.
    #[account(mut)]
    pub buyer_destination: UncheckedAccount<'info>,
    /// CHECK: Verified against seller and mint, including an as-yet uninitialized ATA.
    pub seller_destination: UncheckedAccount<'info>,
    /// CHECK: Verified as the escrow PDA's classic token ATA.
    #[account(mut)]
    pub vault: UncheckedAccount<'info>,
    /// CHECK: Must be the executable classic SPL Token program.
    pub token_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    pub associated_token_program: Program<'info, AssociatedToken>,
}

#[derive(Accounts)]
pub struct CommitDelivery<'info> {
    pub seller: Signer<'info>,
    #[account(mut)]
    pub escrow: Box<Account<'info, Escrow>>,
}

#[derive(Accounts)]
pub struct Settle<'info> {
    pub caller: Signer<'info>,
    #[account(mut)]
    pub escrow: Box<Account<'info, Escrow>>,
    /// CHECK: Compared to the immutable escrow mint and checked for classic SPL ownership and decimals.
    pub mint: UncheckedAccount<'info>,
    /// CHECK: Verified against the stored vault and rederived for the escrow PDA.
    #[account(mut)]
    pub vault: UncheckedAccount<'info>,
    /// CHECK: Verified against the stored destination and beneficiary's derived ATA.
    #[account(mut)]
    pub destination: UncheckedAccount<'info>,
    /// CHECK: Must be the executable classic SPL Token program.
    pub token_program: UncheckedAccount<'info>,
}

impl Settle<'_> {
    fn pay(&self, nonce: &[u8; 32], owner: &Pubkey, destination: &Pubkey) -> Result<()> {
        let escrow = &self.escrow;
        check_token_program(&self.token_program)?;
        require_keys_eq!(
            escrow.token_program,
            token::ID,
            EscrowError::BadTokenProgram
        );
        require_keys_eq!(self.mint.key(), escrow.mint, EscrowError::Mint);
        check_mint(&self.mint)?;
        require_keys_eq!(self.vault.key(), escrow.vault, EscrowError::Destination);
        require_keys_eq!(
            self.destination.key(),
            *destination,
            EscrowError::Destination
        );
        check_ata(&self.vault, &escrow.key(), &escrow.mint)?;
        check_ata(&self.destination, owner, &escrow.mint)?;
        let bump = [escrow.bump];
        let seeds = escrow_seeds(&escrow.buyer, nonce, &bump);
        let authority = Pubkey::create_program_address(&seeds, &crate::ID)
            .map_err(|_| error!(EscrowError::BadDeployment))?;
        require_keys_eq!(authority, escrow.key(), EscrowError::BadDeployment);
        move_principal(
            TransferChecked {
                from: self.vault.to_account_info(),
                mint: self.mint.to_account_info(),
                to: self.destination.to_account_info(),
                authority: escrow.to_account_info(),
            },
            escrow.principal,
            &[&seeds],
        )
    }
}
