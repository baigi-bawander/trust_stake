use anchor_lang::prelude::*;

use crate::{
    constants::{ACCOUNT_VERSION, PERMIT_SEED, SEED_VERSION, STAKE_SEED},
    error::TrustStakeError,
    events::PermitReleased,
    state::{SellerStake, SlashPermit},
};

/// Accounts for [`handler`]. Permissionless (docs/DESIGN-v2.md, decision 8):
/// `caller` is an unconstrained signer, present only to pay the
/// transaction fee, never checked against anything. `seller` is the rent
/// destination the permit's own `close` pays out to; it is bound to
/// `permit.seller` via `has_one`, not required to sign, since anyone may
/// trigger a release once the window has elapsed. `stake`'s seeds are
/// derived from the live `seller` account rather than from `permit`'s
/// fields, so they don't depend on read-after-`close` ordering within
/// `permit`'s own constraint list. `close` goes last in `permit`'s
/// attribute, per Program conventions.
#[event_cpi]
#[derive(Accounts)]
pub struct ReleasePermitAccountConstraints<'info> {
    pub caller: Signer<'info>,

    /// CHECK: rent destination only, credited by `permit`'s `close`;
    /// bound to `permit.seller` via `has_one` below.
    #[account(mut)]
    pub seller: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [PERMIT_SEED, SEED_VERSION, permit.seller.as_ref(), permit.marketplace.as_ref()],
        bump = permit.bump,
        has_one = seller,
        constraint = permit.version == ACCOUNT_VERSION @ TrustStakeError::AccountVersionMismatch,
        close = seller,
    )]
    pub permit: Account<'info, SlashPermit>,

    #[account(
        mut,
        seeds = [STAKE_SEED, SEED_VERSION, seller.key().as_ref()],
        bump = stake.bump,
        constraint = stake.version == ACCOUNT_VERSION @ TrustStakeError::AccountVersionMismatch,
    )]
    pub stake: Account<'info, SellerStake>,
}

/// Requires the permit be revoked, `now >= revoked_at + complaint_window`,
/// and `open_disputes == 0` (docs/DESIGN-v2.md, "Instruction handlers").
/// The revoked check is explicit rather than relying on the `i64::MAX`
/// sentinel to overflow `checked_add` below: this handler is
/// permissionless, so that overflow is the only thing standing between an
/// unrevoked permit and release, and a future edit to the arithmetic
/// (e.g. swapping in `saturating_add`) would silently delete it.
/// Subtracts the permit's remaining allowance (`max_slashable - slashed`)
/// from `stake.committed`, which is the entire point of releasing, then
/// the account constraints close the permit and refund its rent to the
/// seller, who paid it at grant.
pub fn handler(ctx: Context<ReleasePermitAccountConstraints>) -> Result<()> {
    let permit = &ctx.accounts.permit;
    require!(permit.revoked_at != i64::MAX, TrustStakeError::PermitNotRevoked);

    let now = Clock::get()?.unix_timestamp;
    let release_at = permit
        .revoked_at
        .checked_add(permit.complaint_window)
        .ok_or(TrustStakeError::MathOverflow)?;
    require!(now >= release_at, TrustStakeError::ComplaintWindowNotElapsed);
    require!(permit.open_disputes == 0, TrustStakeError::OpenDisputesRemaining);

    let remaining_allowance = permit
        .max_slashable
        .checked_sub(permit.slashed)
        .ok_or(TrustStakeError::MathOverflow)?;
    let seller = permit.seller;
    let marketplace = permit.marketplace;

    ctx.accounts.stake.release_commitment(remaining_allowance)?;

    emit_cpi!(PermitReleased {
        seller,
        marketplace,
        remaining_allowance,
    });

    Ok(())
}
