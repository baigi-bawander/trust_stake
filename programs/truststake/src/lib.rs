pub mod constants;
pub mod error;
pub mod events;
pub mod instructions;
pub mod state;

use anchor_lang::prelude::*;

pub use constants::*;
pub use events::*;
pub use instructions::*;
pub use state::*;

declare_id!("3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2");

#[program]
pub mod truststake {
    use super::*;

    /// Pins the protocol's collateral mint and chain tag, and records the
    /// caller as the first `Config.authority`. Only the compiled-in
    /// initial admin may call this.
    pub fn initialize_config(
        ctx: Context<InitializeConfigAccountConstraints>,
        chain_id: u8,
    ) -> Result<()> {
        instructions::admin::initialize_config::handler(ctx, chain_id)
    }

    /// Names a new `Config.authority`. Takes effect only once the named
    /// key accepts.
    pub fn propose_config_authority(
        ctx: Context<ProposeConfigAuthorityAccountConstraints>,
        new_authority: Pubkey,
    ) -> Result<()> {
        instructions::admin::propose_config_authority::handler(ctx, new_authority)
    }

    /// Completes a proposed `Config.authority` transfer.
    pub fn accept_config_authority(
        ctx: Context<AcceptConfigAuthorityAccountConstraints>,
    ) -> Result<()> {
        instructions::admin::accept_config_authority::handler(ctx)
    }

    /// Registers a new marketplace tenant and creates its `bond_vault`.
    pub fn register_marketplace(
        ctx: Context<RegisterMarketplaceAccountConstraints>,
        marketplace_id: [u8; 16],
        receipt_signer: Pubkey,
        arbiter: Pubkey,
        complaint_window: i64,
        bond_bps: u16,
    ) -> Result<()> {
        instructions::register_marketplace::handler(
            ctx,
            marketplace_id,
            receipt_signer,
            arbiter,
            complaint_window,
            bond_bps,
        )
    }

    /// Updates whichever of a marketplace's settings are supplied; the
    /// rest keep their current value.
    pub fn update_marketplace(
        ctx: Context<UpdateMarketplaceAccountConstraints>,
        new_receipt_signer: Option<Pubkey>,
        new_arbiter: Option<Pubkey>,
        new_complaint_window: Option<i64>,
        new_bond_bps: Option<u16>,
    ) -> Result<()> {
        instructions::update_marketplace::handler(
            ctx,
            new_receipt_signer,
            new_arbiter,
            new_complaint_window,
            new_bond_bps,
        )
    }

    /// Names a new `Marketplace.authority`. Takes effect only once the
    /// named key accepts.
    pub fn propose_marketplace_authority(
        ctx: Context<ProposeMarketplaceAuthorityAccountConstraints>,
        new_authority: Pubkey,
    ) -> Result<()> {
        instructions::propose_marketplace_authority::handler(ctx, new_authority)
    }

    /// Completes a proposed `Marketplace.authority` transfer.
    pub fn accept_marketplace_authority(
        ctx: Context<AcceptMarketplaceAuthorityAccountConstraints>,
    ) -> Result<()> {
        instructions::accept_marketplace_authority::handler(ctx)
    }

    /// Creates a seller's one-and-only `SellerStake` account and its
    /// `stake_vault`. Moves no collateral; `add_stake` does that.
    pub fn initialize_stake(ctx: Context<InitializeStakeAccountConstraints>) -> Result<()> {
        instructions::initialize_stake::handler(ctx)
    }

    /// Locks `amount` of additional collateral into the seller's
    /// `stake_vault`.
    pub fn add_stake(ctx: Context<AddStakeAccountConstraints>, amount: u64) -> Result<()> {
        instructions::add_stake::handler(ctx, amount)
    }
}
