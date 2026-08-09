use anchor_lang::prelude::*;

use crate::{constants::CONFIG_SEED, state::Config};

/// Accounts for [`handler`]. There is exactly one `Config` PDA per
/// deployment; whoever calls this first becomes the arbiter permanently
/// (see the "single arbiter key" tradeoff in CLAUDE.md).
#[derive(Accounts)]
pub struct InitializeConfig<'info> {
    /// Whoever signs this becomes the only key `resolve_dispute` will ever
    /// accept. Not validated against anything else — first caller wins.
    #[account(mut)]
    pub arbiter: Signer<'info>,

    /// Fails with an "already in use" error if this has already been
    /// called once for this deployment; there's no re-initialize path.
    #[account(
        init,
        payer = arbiter,
        space = 8 + Config::INIT_SPACE,
        seeds = [CONFIG_SEED],
        bump,
    )]
    pub config: Account<'info, Config>,

    pub system_program: Program<'info, System>,
}

/// Records `ctx.accounts.arbiter` as the permanent dispute-resolution
/// authority for this deployment. Run this exactly once, before any
/// `resolve_dispute` call is possible.
pub fn handler(ctx: Context<InitializeConfig>) -> Result<()> {
    ctx.accounts.config.set_inner(Config {
        arbiter: ctx.accounts.arbiter.key(),
        bump: ctx.bumps.config,
    });

    Ok(())
}
