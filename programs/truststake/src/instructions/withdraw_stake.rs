use anchor_lang::prelude::*;
use anchor_spl::token_interface::{transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked};

use crate::{
    constants::{SEED_VERSION, STAKE_SEED, VAULT_SEED},
    error::TrustStakeError,
    events::StakeWithdrawn,
    state::SellerStake,
};

/// Accounts for [`handler`]. `mint` binds against `stake_vault.mint` rather
/// than `Config.collateral_mint`, same reasoning as `AddStakeAccountConstraints`:
/// the vault's mint was already checked against `Config` when
/// `initialize_stake` created it, so this handler doesn't need to carry
/// `Config` at all.
#[event_cpi]
#[derive(Accounts)]
pub struct WithdrawStakeAccountConstraints<'info> {
    pub seller: Signer<'info>,

    #[account(
        mut,
        seeds = [STAKE_SEED, SEED_VERSION, seller.key().as_ref()],
        bump = stake.bump,
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

/// Moves `amount` out of `stake_vault` back to the seller's own token
/// account, gated on `staked - amount >= committed` (docs/DESIGN-v2.md,
/// "Instruction handlers"): collateral above `committed` withdraws
/// immediately, but nothing backing an active permit can leave. State is
/// written before the transfer CPI, which is signed by the `stake` PDA
/// since it is `stake_vault`'s token authority, and the vault is reloaded
/// and checked against the ledger afterward: only a vault caught short of
/// `stake.staked` fails the transaction, not one sitting above it, since
/// anyone can push the vault above the ledger with an unsolicited deposit
/// and that must not be able to freeze a seller's own withdrawal (see
/// docs/DESIGN-v2.md, "Program conventions").
pub fn handler(ctx: Context<WithdrawStakeAccountConstraints>, amount: u64) -> Result<()> {
    require!(amount > 0, TrustStakeError::ZeroAmount);

    let remaining = ctx
        .accounts
        .stake
        .staked
        .checked_sub(amount)
        .ok_or(TrustStakeError::MathOverflow)?;
    require!(
        remaining >= ctx.accounts.stake.committed,
        TrustStakeError::CommittedExceedsStaked
    );
    ctx.accounts.stake.staked = remaining;

    let seller_key = ctx.accounts.seller.key();
    let stake_bump = ctx.accounts.stake.bump;
    let signer_seeds: &[&[u8]] = &[STAKE_SEED, SEED_VERSION, seller_key.as_ref(), &[stake_bump]];

    transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.stake_vault.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.seller_token_account.to_account_info(),
                authority: ctx.accounts.stake.to_account_info(),
            },
            &[signer_seeds],
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

    emit_cpi!(StakeWithdrawn {
        seller: seller_key,
        amount,
        staked: ctx.accounts.stake.staked,
    });

    Ok(())
}
