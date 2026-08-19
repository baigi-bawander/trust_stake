pub mod constants;
pub mod ed25519;
pub mod error;
pub mod events;
pub mod instructions;
pub mod receipt;
pub mod state;

use anchor_lang::prelude::*;

pub use constants::*;
pub use events::*;
pub use instructions::*;
pub use receipt::*;
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

    /// Moves `amount` out of the seller's `stake_vault`, gated on
    /// `staked - amount >= committed`.
    pub fn withdraw_stake(ctx: Context<WithdrawStakeAccountConstraints>, amount: u64) -> Result<()> {
        instructions::withdraw_stake::handler(ctx, amount)
    }

    /// Creates the seller's `SlashPermit` for a marketplace, locking
    /// `max_slashable` out of their free collateral and freezing the
    /// marketplace's current `complaint_window`/`bond_bps` onto it.
    pub fn grant_permit(ctx: Context<GrantPermitAccountConstraints>, max_slashable: u64) -> Result<()> {
        instructions::grant_permit::handler(ctx, max_slashable)
    }

    /// Adds `delta` to an existing permit's `max_slashable`. Increase-only:
    /// there is no handler that lowers a cap.
    pub fn increase_permit(ctx: Context<IncreasePermitAccountConstraints>, delta: u64) -> Result<()> {
        instructions::increase_permit::handler(ctx, delta)
    }

    /// Stamps `revoked_at`, starting the permit's complaint window. Frees
    /// no collateral by itself; `release_permit` does that once the
    /// window elapses.
    pub fn revoke_permit(ctx: Context<RevokePermitAccountConstraints>) -> Result<()> {
        instructions::revoke_permit::handler(ctx)
    }

    /// Permissionless once `now >= revoked_at + complaint_window` and
    /// `open_disputes == 0`. Subtracts the permit's remaining allowance
    /// from `stake.committed`, closes the permit, and refunds its rent to
    /// the seller.
    pub fn release_permit(ctx: Context<ReleasePermitAccountConstraints>) -> Result<()> {
        instructions::release_permit::handler(ctx)
    }

    /// Does what `release_permit` does but skips the wait when both the
    /// seller and the marketplace's current authority sign.
    pub fn release_permit_early(ctx: Context<ReleasePermitEarlyAccountConstraints>) -> Result<()> {
        instructions::release_permit_early::handler(ctx)
    }

    /// Opens a complaint about `order_id`, proved by a
    /// marketplace-signed receipt that the transaction's Ed25519
    /// instruction verifies. Must be submitted as
    /// `[Ed25519Program verify, raise_dispute]`, in that order.
    pub fn raise_dispute(
        ctx: Context<RaiseDisputeAccountConstraints>,
        order_id: [u8; 32],
        claim: u64,
    ) -> Result<()> {
        instructions::raise_dispute::handler(ctx, order_id, claim)
    }

    /// Decides an open complaint. Signed by the marketplace's arbiter.
    /// Upheld pays the buyer out of the seller's collateral and returns
    /// the bond; rejected forfeits the bond to the seller.
    pub fn resolve_dispute(ctx: Context<ResolveDisputeAccountConstraints>, upheld: bool) -> Result<()> {
        instructions::resolve_dispute::handler(ctx, upheld)
    }

    /// Permissionless once a complaint has sat undecided for 30 days.
    /// Nobody is paid, the bond returns to the buyer, the permit's freeze
    /// lifts, and the marketplace takes a public mark.
    pub fn expire_dispute(ctx: Context<ExpireDisputeAccountConstraints>) -> Result<()> {
        instructions::expire_dispute::handler(ctx)
    }

    /// Permissionless once a complaint is decided and its receipt is too
    /// old to reuse. Deletes the record and refunds its rent to the
    /// buyer.
    pub fn close_dispute(ctx: Context<CloseDisputeAccountConstraints>) -> Result<()> {
        instructions::close_dispute::handler(ctx)
    }
}
