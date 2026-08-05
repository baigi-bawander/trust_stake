use anchor_lang::prelude::*;

use crate::{
    constants::{CONFIG_SEED, DISPUTE_SEED, STAKE_SEED},
    error::TrustStakeError,
    state::{Config, Dispute, SellerStake},
};

#[derive(Accounts)]
pub struct ResolveDispute<'info> {
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

    #[account(
        mut,
        seeds = [STAKE_SEED, stake.seller.as_ref()],
        bump = stake.bump,
    )]
    pub stake: Account<'info, SellerStake>,

    #[account(
        mut,
        close = buyer,
        seeds = [DISPUTE_SEED, stake.seller.as_ref(), buyer.key().as_ref()],
        bump = dispute.bump,
        has_one = buyer,
    )]
    pub dispute: Account<'info, Dispute>,
}

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
