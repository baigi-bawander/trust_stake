use anchor_lang::prelude::*;

#[error_code]
pub enum TrustStakeError {
    #[msg("Only the compiled-in initial admin may call this")]
    NotInitialAdmin,
    #[msg("Complaint window must be between 2 and 30 days")]
    ComplaintWindowOutOfBounds,
    #[msg("Bond rate exceeds the protocol ceiling of 20%")]
    BondBpsTooHigh,
    #[msg("Amount must be greater than zero")]
    ZeroAmount,
    #[msg("Mint does not match the account it is bound to")]
    WrongMint,
    #[msg("Arithmetic overflow")]
    MathOverflow,
    #[msg("Vault balance disagrees with the recorded ledger")]
    ConservationViolation,
}
