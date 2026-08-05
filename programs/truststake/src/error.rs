use anchor_lang::prelude::*;

#[error_code]
pub enum TrustStakeError {
    #[msg("Amount must be greater than zero")]
    ZeroAmount,
    #[msg("Claim exceeds the seller's staked collateral")]
    ClaimTooLarge,
    #[msg("Only the arbiter may resolve disputes")]
    NotArbiter,
}
