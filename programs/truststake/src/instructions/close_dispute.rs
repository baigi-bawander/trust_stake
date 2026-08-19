use anchor_lang::prelude::*;

use crate::{
    constants::{DISPUTE_SEED, SEED_VERSION},
    error::TrustStakeError,
    events::DisputeClosed,
    state::{DisputeRecord, DisputeStatus},
};

/// Accounts for [`handler`]. Permissionless, like `release_permit`:
/// `caller` pays the fee and is never checked against anything, and
/// `buyer` is the rent destination bound to `dispute.buyer` by `has_one`
/// rather than required to sign, since the buyer paid that rent at
/// `raise_dispute` and anyone may hand it back. `close` goes last in
/// `dispute`'s attribute, per Program conventions: Anchor evaluates
/// constraints in order, so anything after it would inspect an account
/// that has already been zeroed.
#[event_cpi]
#[derive(Accounts)]
pub struct CloseDisputeAccountConstraints<'info> {
    pub caller: Signer<'info>,

    /// CHECK: rent destination only, credited by `dispute`'s `close`;
    /// bound to `dispute.buyer` via `has_one` below.
    #[account(mut)]
    pub buyer: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [DISPUTE_SEED, SEED_VERSION, dispute.marketplace.as_ref(), dispute.order_id.as_ref()],
        bump = dispute.bump,
        has_one = buyer,
        close = buyer,
    )]
    pub dispute: Box<Account<'info, DisputeRecord>>,
}

/// Deletes a decided complaint's record once its receipt is too old to
/// reuse, refunding the rent to the buyer who paid it.
///
/// Both conditions matter. The record is the replay guard, so it may only
/// go once the receipt behind it would fail `raise_dispute`'s window
/// check anyway, which is what `closable_after` (the receipt's
/// `issued_at` plus the permit's complaint window) marks. And a still-open
/// complaint may never be deleted: closing one would erase the freeze on
/// the permit and the money owed with it.
///
/// The permanent record lives in the marketplace's and the seller's
/// counters and in the event log, neither of which this touches.
pub fn handler(ctx: Context<CloseDisputeAccountConstraints>) -> Result<()> {
    require!(
        ctx.accounts.dispute.status != DisputeStatus::Open as u8,
        TrustStakeError::DisputeStillOpen
    );

    let now = Clock::get()?.unix_timestamp;
    require!(
        now >= ctx.accounts.dispute.closable_after,
        TrustStakeError::DisputeNotClosable
    );

    emit_cpi!(DisputeClosed {
        marketplace: ctx.accounts.dispute.marketplace,
        buyer: ctx.accounts.buyer.key(),
        order_id: ctx.accounts.dispute.order_id,
        status: ctx.accounts.dispute.status,
    });

    Ok(())
}
