use anchor_lang::prelude::*;

use crate::{
    constants::{DISPUTE_SEED, STAKE_SEED},
    error::TrustStakeError,
    state::{Dispute, SellerStake},
};

#[derive(Accounts)]
pub struct RaiseDispute<'info> {
    #[account(mut)]
    pub buyer: Signer<'info>,

    /// CHECK: only used to derive the stake and dispute PDAs.
    pub seller: UncheckedAccount<'info>,

    #[account(
        seeds = [STAKE_SEED, seller.key().as_ref()],
        bump = stake.bump,
        has_one = seller,
    )]
    pub stake: Account<'info, SellerStake>,

    #[account(
        init,
        payer = buyer,
        space = 8 + Dispute::INIT_SPACE,
        seeds = [DISPUTE_SEED, seller.key().as_ref(), buyer.key().as_ref()],
        bump,
    )]
    pub dispute: Account<'info, Dispute>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<RaiseDispute>, claim: u64) -> Result<()> {
    require!(claim > 0, TrustStakeError::ZeroAmount);
    require!(
        claim <= ctx.accounts.stake.staked,
        TrustStakeError::ClaimTooLarge
    );

    ctx.accounts.dispute.set_inner(Dispute {
        seller: ctx.accounts.seller.key(),
        buyer: ctx.accounts.buyer.key(),
        claim,
        bump: ctx.bumps.dispute,
    });

    Ok(())
}
