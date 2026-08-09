use anchor_lang::prelude::*;

use crate::{
    constants::{CONFIG_SEED, DISPUTE_SEED, STAKE_SEED},
    error::TrustStakeError,
    state::{Config, Dispute, SellerStake},
};

/// Accounts for [`handler`]. This is the only instruction that moves
/// collateral out of a seller's stake, and the only one gated by the
/// `Config.arbiter` check — everything else in this program is permissionless.
#[derive(Accounts)]
pub struct ResolveDispute<'info> {
    /// Rejected up front if this isn't the one key `initialize_config` set.
    #[account(
        constraint = arbiter.key() == config.arbiter @ TrustStakeError::NotArbiter
    )]
    pub arbiter: Signer<'info>,

    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
    )]
    pub config: Account<'info, Config>,

    /// CHECK: receives the slashed collateral; matched against `dispute.buyer`.
    #[account(mut)]
    pub buyer: UncheckedAccount<'info>,

    /// The seller's collateral account. `stake.seller` (not a separate
    /// account input) is what derives this PDA, so the caller only needs
    /// to know the dispute, not the seller's address directly.
    #[account(
        mut,
        seeds = [STAKE_SEED, stake.seller.as_ref()],
        bump = stake.bump,
    )]
    pub stake: Account<'info, SellerStake>,

    /// Closed either way — upheld or rejected, the dispute is resolved and
    /// its rent is refunded to the buyer. There is no re-opening it.
    #[account(
        mut,
        close = buyer,
        seeds = [DISPUTE_SEED, stake.seller.as_ref(), buyer.key().as_ref()],
        bump = dispute.bump,
        has_one = buyer,
    )]
    pub dispute: Account<'info, Dispute>,
}

/// Rules on the open dispute. If `uphold` is true, transfers
/// `min(claim, current stake)` from the seller's collateral to the buyer
/// and permanently increments `disputes_lost` — capped at the current
/// stake so a seller who has already been partly slashed can't be slashed
/// below zero. If `uphold` is false, the seller's stake is untouched; only
/// the dispute account closes.
pub fn handler(ctx: Context<ResolveDispute>, uphold: bool) -> Result<()> {
    if uphold {
        let amount = ctx.accounts.dispute.claim.min(ctx.accounts.stake.staked);

        // The stake PDA is program-owned, so lamports move without a CPI.
        // Only `staked` is ever moved, leaving the rent-exempt balance intact.
        **ctx.accounts.stake.to_account_info().try_borrow_mut_lamports()? -= amount;
        **ctx.accounts.buyer.to_account_info().try_borrow_mut_lamports()? += amount;

        let stake = &mut ctx.accounts.stake;
        stake.staked -= amount;
        stake.disputes_lost += 1;
    }

    Ok(())
}
