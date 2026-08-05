pub mod constants;
pub mod error;
pub mod instructions;
pub mod state;

use anchor_lang::prelude::*;

pub use constants::*;
pub use instructions::*;
pub use state::*;

declare_id!("3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2");

#[program]
pub mod truststake {
    use super::*;

    /// Set the arbiter that rules on every dispute. Runs once.
    pub fn initialize_config(ctx: Context<InitializeConfig>) -> Result<()> {
        initialize_config::handler(ctx)
    }

    /// Lock collateral that buyers can claim against.
    pub fn create_stake(ctx: Context<CreateStake>, amount: u64) -> Result<()> {
        create_stake::handler(ctx, amount)
    }

    /// File a complaint against a staked seller.
    pub fn raise_dispute(ctx: Context<RaiseDispute>, claim: u64) -> Result<()> {
        raise_dispute::handler(ctx, claim)
    }

    /// Rule on a dispute. Upholding slashes the seller's stake to the buyer.
    pub fn resolve_dispute(ctx: Context<ResolveDispute>, uphold: bool) -> Result<()> {
        resolve_dispute::handler(ctx, uphold)
    }
}
