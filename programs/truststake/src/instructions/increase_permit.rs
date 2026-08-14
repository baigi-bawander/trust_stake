use anchor_lang::prelude::*;

use crate::{
    constants::{PERMIT_SEED, SEED_VERSION, STAKE_SEED},
    error::TrustStakeError,
    events::PermitIncreased,
    state::{SellerStake, SlashPermit},
};

/// Accounts for [`handler`]. `permit` self-validates against its own
/// stored `seller`/`marketplace` fields, the same pattern
/// `UpdateMarketplaceAccountConstraints` uses for `Marketplace`
/// (docs/DESIGN-v2.md, "Program conventions"), and `has_one = seller` binds
/// the live signer to it. No `marketplace` account is needed here:
/// `complaint_window` and `bond_bps` are frozen at grant and never
/// re-read from the live marketplace on an increase.
#[event_cpi]
#[derive(Accounts)]
pub struct IncreasePermitAccountConstraints<'info> {
    pub seller: Signer<'info>,

    #[account(
        mut,
        seeds = [STAKE_SEED, SEED_VERSION, seller.key().as_ref()],
        bump = stake.bump,
    )]
    pub stake: Account<'info, SellerStake>,

    #[account(
        mut,
        seeds = [PERMIT_SEED, SEED_VERSION, permit.seller.as_ref(), permit.marketplace.as_ref()],
        bump = permit.bump,
        has_one = seller,
    )]
    pub permit: Account<'info, SlashPermit>,
}

/// Adds `delta` to the permit's `max_slashable`, gated on
/// `committed + delta <= staked`. `max_slashable` is increase-only by
/// construction here: `delta` is unsigned and only ever added, so there is
/// no path that lowers it (docs/DESIGN-v2.md, "Instruction handlers"). To
/// lower exposure the seller must revoke and grant a smaller permit after
/// the window. Requires the permit still be active: increasing a cap the
/// seller has already started winding down is not a security hole (payout
/// stays bounded by the receipt regardless), but it is a confusing no-op
/// that only locks up more of the seller's own collateral, and
/// `revoke_permit` already rejects this same shape of action outright
/// rather than allowing it silently.
pub fn handler(ctx: Context<IncreasePermitAccountConstraints>, delta: u64) -> Result<()> {
    require!(delta > 0, TrustStakeError::ZeroAmount);
    require!(
        ctx.accounts.permit.revoked_at == i64::MAX,
        TrustStakeError::PermitAlreadyRevoked
    );

    ctx.accounts.stake.commit(delta)?;

    let max_slashable = ctx
        .accounts
        .permit
        .max_slashable
        .checked_add(delta)
        .ok_or(TrustStakeError::MathOverflow)?;
    ctx.accounts.permit.max_slashable = max_slashable;

    emit_cpi!(PermitIncreased {
        seller: ctx.accounts.seller.key(),
        marketplace: ctx.accounts.permit.marketplace,
        delta,
        max_slashable,
    });

    Ok(())
}
