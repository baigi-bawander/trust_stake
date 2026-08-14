use anchor_lang::prelude::*;

use crate::{
    constants::{MARKETPLACE_SEED, PERMIT_SEED, SEED_VERSION, STAKE_SEED},
    error::TrustStakeError,
    events::PermitReleasedEarly,
    state::{Marketplace, SellerStake, SlashPermit},
};

/// Accounts for [`handler`]. Does the same thing as
/// `ReleasePermitAccountConstraints` but requires both the seller and the
/// marketplace's current authority to sign, so it can skip the wait
/// (docs/DESIGN-v2.md, "Instruction handlers"). `marketplace`
/// self-validates against its own stored `marketplace_id` and binds
/// `authority` via `has_one`; `permit` additionally binds `marketplace` via
/// `has_one` so the caller can't pass some other, unrelated marketplace
/// whose authority happens to be willing to sign. `close` goes last in
/// `permit`'s attribute, per Program conventions.
#[event_cpi]
#[derive(Accounts)]
pub struct ReleasePermitEarlyAccountConstraints<'info> {
    #[account(mut)]
    pub seller: Signer<'info>,

    pub authority: Signer<'info>,

    #[account(
        seeds = [MARKETPLACE_SEED, SEED_VERSION, marketplace.marketplace_id.as_ref()],
        bump = marketplace.bump,
        has_one = authority,
    )]
    pub marketplace: Account<'info, Marketplace>,

    #[account(
        mut,
        seeds = [PERMIT_SEED, SEED_VERSION, permit.seller.as_ref(), permit.marketplace.as_ref()],
        bump = permit.bump,
        has_one = seller,
        has_one = marketplace,
        close = seller,
    )]
    pub permit: Account<'info, SlashPermit>,

    #[account(
        mut,
        seeds = [STAKE_SEED, SEED_VERSION, seller.key().as_ref()],
        bump = stake.bump,
    )]
    pub stake: Account<'info, SellerStake>,
}

/// Skips `release_permit`'s time-window check when both the seller and
/// the marketplace authority cooperate; the permit must still be revoked
/// and `open_disputes == 0` is still required. "Early" means skipping the
/// wait, not skipping the wind-down itself -- a still-active permit is not
/// eligible for either release path. Otherwise identical: subtracts the
/// permit's remaining allowance from `stake.committed`, then the account
/// constraints close the permit and refund its rent to the seller.
pub fn handler(ctx: Context<ReleasePermitEarlyAccountConstraints>) -> Result<()> {
    let permit = &ctx.accounts.permit;
    require!(permit.revoked_at != i64::MAX, TrustStakeError::PermitNotRevoked);
    require!(permit.open_disputes == 0, TrustStakeError::OpenDisputesRemaining);

    let remaining_allowance = permit
        .max_slashable
        .checked_sub(permit.slashed)
        .ok_or(TrustStakeError::MathOverflow)?;
    let seller = permit.seller;
    let marketplace = permit.marketplace;

    ctx.accounts.stake.release_commitment(remaining_allowance)?;

    emit_cpi!(PermitReleasedEarly {
        seller,
        marketplace,
        remaining_allowance,
    });

    Ok(())
}
