use anchor_lang::prelude::*;

use crate::{
    constants::{DISPUTE_SEED, STAKE_SEED},
    error::TrustStakeError,
    state::{Dispute, SellerStake},
};

/// Accounts for [`handler`]. One open `Dispute` PDA per (seller, buyer)
/// pair — a second dispute from the same buyer against the same seller
/// while one is still open will fail to init (see CLAUDE.md: no multiple
/// open disputes yet).
#[derive(Accounts)]
pub struct RaiseDispute<'info> {
    /// The buyer filing the claim. Pays the new dispute account's rent.
    #[account(mut)]
    pub buyer: Signer<'info>,

    /// CHECK: only used to derive the stake and dispute PDAs.
    pub seller: UncheckedAccount<'info>,

    /// Must already exist — a seller with no stake can't be disputed.
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

/// Opens a claim for `claim` lamports against `seller`'s stake. Rejects
/// zero-lamport claims and anything larger than the seller's *current*
/// stake — not the amount originally staked, so a seller who has already
/// been partly slashed can't be over-claimed against.
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
