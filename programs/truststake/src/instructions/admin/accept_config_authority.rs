use anchor_lang::prelude::*;

use crate::{
    constants::{CONFIG_SEED, SEED_VERSION},
    events::ConfigAuthorityAccepted,
    state::Config,
};

/// Accounts for [`handler`]. Only the pubkey named in
/// `config.pending_authority` may accept; `has_one` enforces the match.
#[event_cpi]
#[derive(Accounts)]
pub struct AcceptConfigAuthorityAccountConstraints<'info> {
    pub pending_authority: Signer<'info>,

    #[account(
        mut,
        seeds = [CONFIG_SEED, SEED_VERSION],
        bump = config.bump,
        has_one = pending_authority,
    )]
    pub config: Account<'info, Config>,
}

/// Completes the transfer: `config.authority` becomes the caller, and
/// `pending_authority` resets so it cannot be accepted a second time.
pub fn handler(ctx: Context<AcceptConfigAuthorityAccountConstraints>) -> Result<()> {
    let config = &mut ctx.accounts.config;
    let previous_authority = config.authority;
    config.authority = ctx.accounts.pending_authority.key();
    config.pending_authority = Pubkey::default();

    emit_cpi!(ConfigAuthorityAccepted {
        previous_authority,
        new_authority: config.authority,
    });

    Ok(())
}
