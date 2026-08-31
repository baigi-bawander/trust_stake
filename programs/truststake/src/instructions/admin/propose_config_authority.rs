use anchor_lang::prelude::*;

use crate::{
    constants::{ACCOUNT_VERSION, CONFIG_SEED, SEED_VERSION},
    error::TrustStakeError,
    events::ConfigAuthorityProposed,
    state::Config,
};

/// Accounts for [`handler`]. Only the current `Config.authority` may
/// propose a new one; `has_one` enforces that the caller matches
/// `config.authority`.
#[event_cpi]
#[derive(Accounts)]
pub struct ProposeConfigAuthorityAccountConstraints<'info> {
    pub authority: Signer<'info>,

    #[account(
        mut,
        seeds = [CONFIG_SEED, SEED_VERSION],
        bump = config.bump,
        has_one = authority,
        constraint = config.version == ACCOUNT_VERSION @ TrustStakeError::AccountVersionMismatch,
    )]
    pub config: Account<'info, Config>,
}

/// Records `new_authority` as `Config.pending_authority`. Takes effect
/// only once `new_authority` signs `accept_config_authority`;
/// `config.authority` is untouched until then.
pub fn handler(
    ctx: Context<ProposeConfigAuthorityAccountConstraints>,
    new_authority: Pubkey,
) -> Result<()> {
    let config = &mut ctx.accounts.config;
    let current_authority = config.authority;
    config.pending_authority = new_authority;

    emit_cpi!(ConfigAuthorityProposed {
        current_authority,
        pending_authority: new_authority,
    });

    Ok(())
}
