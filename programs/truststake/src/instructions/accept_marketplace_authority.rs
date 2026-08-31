use anchor_lang::prelude::*;

use crate::{
    constants::{ACCOUNT_VERSION, MARKETPLACE_SEED, SEED_VERSION},
    error::TrustStakeError,
    events::MarketplaceAuthorityAccepted,
    state::Marketplace,
};

/// Accounts for [`handler`]. Only the pubkey named in
/// `marketplace.pending_authority` may accept; `has_one` enforces the
/// match. `seeds` self-validate against the loaded account's own
/// `marketplace_id`.
#[event_cpi]
#[derive(Accounts)]
pub struct AcceptMarketplaceAuthorityAccountConstraints<'info> {
    pub pending_authority: Signer<'info>,

    #[account(
        mut,
        seeds = [MARKETPLACE_SEED, SEED_VERSION, marketplace.marketplace_id.as_ref()],
        bump = marketplace.bump,
        has_one = pending_authority,
        constraint = marketplace.version == ACCOUNT_VERSION @ TrustStakeError::AccountVersionMismatch,
    )]
    pub marketplace: Account<'info, Marketplace>,
}

/// Completes the transfer: `marketplace.authority` becomes the caller,
/// and `pending_authority` resets so it cannot be accepted a second time.
pub fn handler(ctx: Context<AcceptMarketplaceAuthorityAccountConstraints>) -> Result<()> {
    let marketplace = &mut ctx.accounts.marketplace;
    let previous_authority = marketplace.authority;
    marketplace.authority = ctx.accounts.pending_authority.key();
    marketplace.pending_authority = Pubkey::default();

    emit_cpi!(MarketplaceAuthorityAccepted {
        marketplace: marketplace.key(),
        previous_authority,
        new_authority: marketplace.authority,
    });

    Ok(())
}
