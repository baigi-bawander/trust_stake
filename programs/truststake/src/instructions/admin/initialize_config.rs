use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    spl_token_2022::{
        extension::{BaseStateWithExtensions, StateWithExtensions},
        state::Mint as MintState,
    },
    Mint,
};

use crate::{
    constants::{
        ACCOUNT_VERSION, ALLOWED_MINT_EXTENSIONS, CHAIN_ID_DEVNET, CHAIN_ID_MAINNET, CONFIG_SEED, INITIAL_ADMIN,
        SEED_VERSION,
    },
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
    /// only be replaced, never repaired. Which extensions it may carry is
    /// checked in [`handler`], where the allow-list can be named in the
    /// error.
    pub mint: InterfaceAccount<'info, Mint>,

    pub system_program: Program<'info, System>,
}

/// Pins the protocol's collateral mint and chain tag, and records
/// `ctx.accounts.admin` as the first `Config.authority`. Runs exactly
/// once per deployment; `Config.authority` moves afterwards only through
/// the two-step transfer (`propose_config_authority` /
/// `accept_config_authority`).
///
/// The mint is required to carry only extensions named in
/// `ALLOWED_MINT_EXTENSIONS`, which is empty, so today that means no
/// extensions at all. A `TransferFeeConfig` mint is the case that forces
/// this: it skims a percentage in transit, so a buyer's bond would arrive
/// in the bond vault short of the amount `raise_dispute` records on the
/// `DisputeRecord`, and both `resolve_dispute` and `expire_dispute` would
/// then try to move the full recorded bond out of a vault holding less
/// and revert forever, stranding `open_disputes` at 1 and the seller's
/// committed collateral with it. Because there is no `update_config`,
/// that is unrecoverable short of a program upgrade, so it is checked
/// here, once, rather than defended against in five token-moving
/// handlers.
///
/// What is tested is whether the mint carries the extension, never what
/// its fee is set to today. A transfer fee may be created at zero and
/// raised later by the `transfer_fee_config_authority` through
/// `SetTransferFee`, so a zero reading now promises nothing; whether the
/// extension is present at all is fixed when the mint is created and can
/// never change afterwards.
pub fn handler(ctx: Context<InitializeConfigAccountConstraints>, chain_id: u8) -> Result<()> {
    require!(
        chain_id == CHAIN_ID_DEVNET || chain_id == CHAIN_ID_MAINNET,
        TrustStakeError::InvalidChainId
    );

    // A Classic Token Program mint's data ends at the base struct, which
    // this unpacks to an empty extension list, so both token programs go
    // through the one path.
    let mint_info = ctx.accounts.mint.to_account_info();
    let mint_data = mint_info.try_borrow_data()?;
    let mint_state = StateWithExtensions::<MintState>::unpack(&mint_data)?;
    for extension in mint_state.get_extension_types()? {
        require!(
            ALLOWED_MINT_EXTENSIONS.contains(&extension),
            TrustStakeError::UnsupportedMintExtension
        );
    }
    drop(mint_data);

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
