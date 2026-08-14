use anchor_lang::prelude::*;
use anchor_spl::token_interface::Mint;

use crate::{
    constants::{ACCOUNT_VERSION, CONFIG_SEED, INITIAL_ADMIN, SEED_VERSION},
    error::TrustStakeError,
    events::ConfigInitialized,
    state::Config,
};

/// Accounts for [`handler`]. Only the compiled-in initial admin may call
/// this, so a freshly deployed program cannot have its config front-run
/// by whoever notices the deployment first.
#[event_cpi]
#[derive(Accounts)]
pub struct InitializeConfigAccountConstraints<'info> {
    #[account(
        mut,
        constraint = admin.key() == INITIAL_ADMIN @ TrustStakeError::NotInitialAdmin,
    )]
    pub admin: Signer<'info>,

    #[account(
        init,
        payer = admin,
        space = Config::DISCRIMINATOR.len() + Config::INIT_SPACE,
        seeds = [CONFIG_SEED, SEED_VERSION],
        bump,
    )]
    pub config: Account<'info, Config>,

    /// The protocol-wide collateral mint, pinned by address. There is no
    /// `update_config`, so a deployment that pins the wrong mint here can
    /// only be replaced, never repaired.
    pub mint: InterfaceAccount<'info, Mint>,

    pub system_program: Program<'info, System>,
}

/// Pins the protocol's collateral mint and chain tag, and records
/// `ctx.accounts.admin` as the first `Config.authority`. Runs exactly
/// once per deployment; `Config.authority` moves afterwards only through
/// the two-step transfer (`propose_config_authority` /
/// `accept_config_authority`).
pub fn handler(ctx: Context<InitializeConfigAccountConstraints>, chain_id: u8) -> Result<()> {
    ctx.accounts.config.set_inner(Config {
        version: ACCOUNT_VERSION,
        bump: ctx.bumps.config,
        authority: ctx.accounts.admin.key(),
        pending_authority: Pubkey::default(),
        collateral_mint: ctx.accounts.mint.key(),
        chain_id,
        reserved: [0; 64],
    });

    emit_cpi!(ConfigInitialized {
        authority: ctx.accounts.admin.key(),
        collateral_mint: ctx.accounts.mint.key(),
        chain_id,
    });

    Ok(())
}
