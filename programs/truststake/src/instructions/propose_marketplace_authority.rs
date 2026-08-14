use anchor_lang::prelude::*;

use crate::{
    constants::{MARKETPLACE_SEED, SEED_VERSION},
    events::MarketplaceAuthorityProposed,
    state::Marketplace,
};

/// Accounts for [`handler`]. Only the current `Marketplace.authority` may
/// propose a new one; `has_one` enforces that the caller matches
/// `marketplace.authority`. `seeds` self-validate against the loaded
/// account's own `marketplace_id`.
#[event_cpi]
#[derive(Accounts)]
pub struct ProposeMarketplaceAuthorityAccountConstraints<'info> {
    pub authority: Signer<'info>,

    #[account(
        mut,
        seeds = [MARKETPLACE_SEED, SEED_VERSION, marketplace.marketplace_id.as_ref()],
        bump = marketplace.bump,
        has_one = authority,
    )]
    pub marketplace: Account<'info, Marketplace>,
}

/// Records `new_authority` as `Marketplace.pending_authority`. Takes
/// effect only once `new_authority` signs `accept_marketplace_authority`.
pub fn handler(
    ctx: Context<ProposeMarketplaceAuthorityAccountConstraints>,
    new_authority: Pubkey,
) -> Result<()> {
    let marketplace = &mut ctx.accounts.marketplace;
    let current_authority = marketplace.authority;
    marketplace.pending_authority = new_authority;

    emit_cpi!(MarketplaceAuthorityProposed {
        marketplace: marketplace.key(),
        current_authority,
        pending_authority: new_authority,
    });

    Ok(())
}
