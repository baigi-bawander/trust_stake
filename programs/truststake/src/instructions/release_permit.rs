use anchor_lang::prelude::*;

use crate::{
    constants::{ACCOUNT_VERSION, CLOCK_SKEW_TOLERANCE_SECONDS, PERMIT_SEED, SEED_VERSION, STAKE_SEED},
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

/// Requires the permit be revoked, `now >= revoked_at +
/// max(complaint_window, CLOCK_SKEW_TOLERANCE_SECONDS)` (see
/// `earliest_release`), and `open_disputes == 0` (docs/DESIGN-v2.md,
/// "Instruction handlers"). The revoked check is explicit rather than
/// relying on the `i64::MAX` sentinel to overflow `checked_add` below:
/// this handler is permissionless, so that overflow is the only thing
/// standing between an unrevoked permit and release, and a future edit to
/// the arithmetic (e.g. swapping in `saturating_add`) would silently
/// delete it. Subtracts the permit's remaining allowance (`max_slashable
/// - slashed`) from `stake.committed`, which is the entire point of
/// releasing, then the account constraints close the permit and refund
/// its rent to the seller, who paid it at grant.
pub fn handler(ctx: Context<ReleasePermitAccountConstraints>) -> Result<()> {
    let permit = &ctx.accounts.permit;
    require!(permit.revoked_at != i64::MAX, TrustStakeError::PermitNotRevoked);

    let now = Clock::get()?.unix_timestamp;
    let release_at = earliest_release(permit.revoked_at, permit.complaint_window)?;
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

/// The earliest instant a revoked permit may be released. Floored at
/// `CLOCK_SKEW_TOLERANCE_SECONDS` rather than using `complaint_window`
/// alone: `release_permit_early`'s minimum wait after revocation is that
/// same constant, and the two waits are one guarantee split across two
/// handlers (see that handler's doc comment for the full R/L/G
/// derivation). `raise_dispute` check 7 rejects a stale-era receipt only
/// as long as whichever release path actually ran waited at least
/// `CLOCK_SKEW_TOLERANCE_SECONDS` past revocation. `register_marketplace`
/// and `update_marketplace` both enforce `complaint_window >=
/// MIN_COMPLAINT_WINDOW_SECONDS`, which is comfortably above the
/// tolerance today -- but `grant_permit` copies a marketplace's
/// `complaint_window` onto each new permit with no re-validation, on
/// purpose (grandfathering, docs/DESIGN-v2.md), so a marketplace
/// registered before a future increase to `MIN_COMPLAINT_WINDOW_SECONDS`
/// can still be granting permits with a window below the new floor. This
/// flooring is what keeps the guarantee holding for that grandfathered
/// window instead of depending on the constants alone, the same way
/// `raise_dispute`'s `closable_after` was widened to depend on
/// `permit.complaint_window.max(MAX_COMPLAINT_WINDOW_SECONDS)` rather
/// than the live permit's window alone.
fn earliest_release(revoked_at: i64, complaint_window: i64) -> Result<i64> {
    revoked_at
        .checked_add(complaint_window.max(CLOCK_SKEW_TOLERANCE_SECONDS))
        .ok_or(TrustStakeError::MathOverflow.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_window_is_unaffected() {
        // A 2-day window, the protocol minimum reachable through
        // register_marketplace today, is well above the clock-skew
        // tolerance, so the floor is a no-op for every window currently
        // grantable.
        let two_days = 2 * 24 * 60 * 60;
        assert_eq!(earliest_release(1_000, two_days).unwrap(), 1_000 + two_days);
    }

    #[test]
    fn sub_tolerance_window_is_floored_at_the_skew_tolerance() {
        // Unreachable through register_marketplace/update_marketplace
        // today, since both enforce MIN_COMPLAINT_WINDOW_SECONDS -- but
        // reachable on a permit grandfathered before a future increase to
        // that constant. Without the floor this would return
        // revoked_at + 60, short-circuiting check 7's own
        // CLOCK_SKEW_TOLERANCE_SECONDS assumption.
        assert_eq!(
            earliest_release(1_000, 60).unwrap(),
            1_000 + CLOCK_SKEW_TOLERANCE_SECONDS
        );
    }

    #[test]
    fn window_equal_to_tolerance_is_the_boundary() {
        // At exactly CLOCK_SKEW_TOLERANCE_SECONDS the floor and the raw
        // window agree, so either derivation gives the same answer.
        assert_eq!(
            earliest_release(1_000, CLOCK_SKEW_TOLERANCE_SECONDS).unwrap(),
            1_000 + CLOCK_SKEW_TOLERANCE_SECONDS
        );
    }

    #[test]
    fn revoked_at_overflow_is_still_guarded() {
        // The MathOverflow path must survive the floor being added: a
        // revoked_at pinned at i64::MAX must still error rather than wrap.
        assert!(earliest_release(i64::MAX, 60).is_err());
    }
}
