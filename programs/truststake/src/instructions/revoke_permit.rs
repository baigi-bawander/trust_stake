use anchor_lang::prelude::*;

use crate::{
    constants::{PERMIT_SEED, SEED_VERSION},
    error::TrustStakeError,
    events::PermitRevoked,
    state::SlashPermit,
};

/// Accounts for [`handler`]. `permit` self-validates against its own
/// stored `seller`/`marketplace` fields and `has_one = seller` binds the
/// live signer to it, same pattern as `IncreasePermitAccountConstraints`.
#[event_cpi]
#[derive(Accounts)]
pub struct RevokePermitAccountConstraints<'info> {
    pub seller: Signer<'info>,

    #[account(
        mut,
        seeds = [PERMIT_SEED, SEED_VERSION, permit.seller.as_ref(), permit.marketplace.as_ref()],
        bump = permit.bump,
        has_one = seller,
    )]
    pub permit: Account<'info, SlashPermit>,
}

/// Stamps `revoked_at` and nothing else (docs/DESIGN-v2.md, "Instruction
/// handlers"): it stops new receipts immediately but frees no collateral
/// by itself, since `stake.committed` only drops at `release_permit`.
/// Requires the permit to currently be active: re-stamping an
/// already-revoked permit would silently push its release date later,
/// which only ever hurts the seller who called it, but there's no reason
/// to allow it silently rather than reject it outright.
pub fn handler(ctx: Context<RevokePermitAccountConstraints>) -> Result<()> {
    let permit = &mut ctx.accounts.permit;
    require!(permit.revoked_at == i64::MAX, TrustStakeError::PermitAlreadyRevoked);
    permit.revoked_at = Clock::get()?.unix_timestamp;

    emit_cpi!(PermitRevoked {
        seller: permit.seller,
        marketplace: permit.marketplace,
        revoked_at: permit.revoked_at,
    });

    Ok(())
}
