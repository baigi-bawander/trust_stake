use anchor_lang::prelude::*;

use crate::{
    constants::{ACCOUNT_VERSION, MARKETPLACE_SEED, PERMIT_SEED, SEED_VERSION, STAKE_SEED},
    error::TrustStakeError,
    events::PermitGranted,
    state::{Marketplace, SellerStake, SlashPermit},
};

/// Accounts for [`handler`]. `stake`'s seeds bind it to the live `seller`
/// signer directly (no `has_one` needed, same pattern as
/// `AddStakeAccountConstraints`). `marketplace` self-validates against its
/// own stored `marketplace_id` (docs/DESIGN-v2.md, "Program conventions"),
/// so a substituted marketplace account can never be used to freeze bogus
/// `complaint_window`/`bond_bps` values onto the new permit.
#[event_cpi]
#[derive(Accounts)]
pub struct GrantPermitAccountConstraints<'info> {
    #[account(mut)]
    pub seller: Signer<'info>,

    #[account(
        mut,
        seeds = [STAKE_SEED, SEED_VERSION, seller.key().as_ref()],
        bump = stake.bump,
        constraint = stake.version == ACCOUNT_VERSION @ TrustStakeError::AccountVersionMismatch,
    )]
    pub stake: Account<'info, SellerStake>,

    #[account(
        seeds = [MARKETPLACE_SEED, SEED_VERSION, marketplace.marketplace_id.as_ref()],
        bump = marketplace.bump,
        constraint = marketplace.version == ACCOUNT_VERSION @ TrustStakeError::AccountVersionMismatch,
    )]
    pub marketplace: Account<'info, Marketplace>,

    #[account(
        init,
        payer = seller,
        space = SlashPermit::DISCRIMINATOR.len() + SlashPermit::INIT_SPACE,
        seeds = [PERMIT_SEED, SEED_VERSION, seller.key().as_ref(), marketplace.key().as_ref()],
        bump,
    )]
    pub permit: Account<'info, SlashPermit>,

    pub system_program: Program<'info, System>,
}

/// Creates the seller's permit for this marketplace and locks `max_slashable`
/// out of their free collateral. Requires the seller's own signature --
/// that is the entire meaning of opting in (docs/DESIGN-v2.md, "Instruction
/// handlers") -- and `committed + max_slashable <= staked`.
/// `complaint_window` and `bond_bps` are copied from the marketplace's
/// current settings and frozen on the permit forever (decision 10);
/// `revoked_at` starts at the `i64::MAX` sentinel for "active", never zero,
/// which would make `release_permit`'s window check trivially true.
/// `granted_at` is stamped from `Clock` here and nowhere else: it marks
/// the start of this permit's era, which is what lets `raise_dispute`
/// refuse a receipt left over from whatever occupied this address before.
pub fn handler(ctx: Context<GrantPermitAccountConstraints>, max_slashable: u64) -> Result<()> {
    require!(max_slashable > 0, TrustStakeError::ZeroAmount);

    ctx.accounts.stake.commit(max_slashable)?;

    let seller = ctx.accounts.seller.key();
    let marketplace_key = ctx.accounts.marketplace.key();
    let complaint_window = ctx.accounts.marketplace.complaint_window;
    let bond_bps = ctx.accounts.marketplace.bond_bps;
    let granted_at = Clock::get()?.unix_timestamp;

    ctx.accounts.permit.set_inner(SlashPermit {
        version: ACCOUNT_VERSION,
        bump: ctx.bumps.permit,
        seller,
        marketplace: marketplace_key,
        max_slashable,
        slashed: 0,
        open_disputes: 0,
        granted_at,
        revoked_at: i64::MAX,
        complaint_window,
        bond_bps,
        reserved: [0; 24],
    });

    emit_cpi!(PermitGranted {
        seller,
        marketplace: marketplace_key,
        max_slashable,
        complaint_window,
        bond_bps,
    });

    Ok(())
}
