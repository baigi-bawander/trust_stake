use anchor_lang::prelude::*;
use anchor_spl::token_interface::{transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked};

use crate::{
    constants::{ACCOUNT_VERSION, SEED_VERSION, STAKE_SEED, VAULT_SEED},
    error::TrustStakeError,
    events::StakeAdded,
    state::SellerStake,
};

/// Accounts for [`handler`]. `mint` binds against `stake_vault.mint`
/// rather than `Config.collateral_mint`: the vault's mint was already
/// checked against `Config` when `initialize_stake` created it, so this
/// handler doesn't need to carry `Config` at all.
#[event_cpi]
#[derive(Accounts)]
pub struct AddStakeAccountConstraints<'info> {
    pub seller: Signer<'info>,

    #[account(
        mut,
        seeds = [STAKE_SEED, SEED_VERSION, seller.key().as_ref()],
        bump = stake.bump,
        constraint = stake.version == ACCOUNT_VERSION @ TrustStakeError::AccountVersionMismatch,
    )]
    pub stake: Account<'info, SellerStake>,

    #[account(
        mut,
        seeds = [VAULT_SEED, SEED_VERSION, seller.key().as_ref()],
        bump,
    )]
    pub stake_vault: InterfaceAccount<'info, TokenAccount>,

    #[account(
        address = stake_vault.mint @ TrustStakeError::WrongMint,
    )]
    pub mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        token::mint = mint,
        token::authority = seller,
    )]
    pub seller_token_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
}

/// Moves `amount` of collateral from the seller's own token account into
/// `stake_vault`. State is written before the transfer CPI (checks, then
/// effects, then interactions), and the vault is reloaded and checked
/// against the ledger afterwards: a token account accepts a transfer from
/// anyone without its owner's consent, so `stake_vault.amount` can run
/// ahead of `stake.staked` from an unsolicited deposit alone, and that
/// direction is harmless surplus rather than a bug. Only a vault caught
/// *short* of the ledger -- what a drain looks like -- fails the
/// transaction; see docs/DESIGN-v2.md, "Program conventions".
pub fn handler(ctx: Context<AddStakeAccountConstraints>, amount: u64) -> Result<()> {
    require!(amount > 0, TrustStakeError::ZeroAmount);

    let stake = &mut ctx.accounts.stake;
    stake.staked = stake
        .staked
        .checked_add(amount)
        .ok_or(TrustStakeError::MathOverflow)?;

    transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.seller_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.stake_vault.to_account_info(),
                authority: ctx.accounts.seller.to_account_info(),
            },
        ),
        amount,
        ctx.accounts.mint.decimals,
    )?;

    ctx.accounts.stake_vault.reload()?;
    require_gte!(
        ctx.accounts.stake_vault.amount,
        ctx.accounts.stake.staked,
        TrustStakeError::ConservationViolation
    );

    emit_cpi!(StakeAdded {
        seller: ctx.accounts.seller.key(),
        amount,
        staked: ctx.accounts.stake.staked,
    });

    Ok(())
}
