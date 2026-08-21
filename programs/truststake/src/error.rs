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
    #[msg("Vault balance is below the recorded ledger")]
    ConservationViolation,
    #[msg("Committed collateral would exceed staked collateral")]
    CommittedExceedsStaked,
    #[msg("Permit is already revoked")]
    PermitAlreadyRevoked,
    #[msg("Complaint window has not elapsed since revocation")]
    ComplaintWindowNotElapsed,
    #[msg("Permit has open disputes")]
    OpenDisputesRemaining,
    #[msg("Permit must be revoked first")]
    PermitNotRevoked,
    #[msg("Instructions sysvar account is not the real one")]
    InvalidInstructionsSysvar,
    #[msg("raise_dispute must be a top-level instruction, not reached through CPI")]
    MustBeTopLevelInstruction,
    #[msg("No Ed25519 verify instruction immediately before this one")]
    MissingEd25519Instruction,
    #[msg("Ed25519 verify instruction is not the canonical single-signature form")]
    MalformedEd25519Instruction,
    #[msg("Receipt was not signed by the marketplace's receipt signer")]
    WrongReceiptSigner,
    #[msg("Receipt carries the wrong domain prefix")]
    WrongReceiptDomain,
    #[msg("Receipt was issued for a different program")]
    WrongReceiptProgram,
    #[msg("Receipt was issued for a different cluster")]
    WrongChainId,
    #[msg("Receipt is past its expiry")]
    ReceiptExpired,
    #[msg("Receipt is dated further in the future than the allowed clock skew tolerance")]
    ReceiptIssuedInFuture,
    #[msg("Complaint window for this receipt has closed")]
    ComplaintWindowClosed,
    #[msg("Receipt names a different marketplace")]
    ReceiptMarketplaceMismatch,
    #[msg("Receipt names a different seller")]
    ReceiptSellerMismatch,
    #[msg("Receipt names a different buyer")]
    ReceiptBuyerMismatch,
    #[msg("Receipt names a different order")]
    ReceiptOrderMismatch,
    #[msg("Receipt was issued after the permit was revoked")]
    ReceiptIssuedAfterRevocation,
    #[msg("Claim exceeds the receipt's order amount")]
    ClaimExceedsReceipt,
    #[msg("Only the marketplace's arbiter may resolve this dispute")]
    NotArbiter,
    #[msg("Dispute is not open")]
    DisputeNotOpen,
    #[msg("Dispute is still open")]
    DisputeStillOpen,
    #[msg("Dispute has not reached its expiry")]
    DisputeNotExpired,
    #[msg("Dispute's receipt is still inside its complaint window")]
    DisputeNotClosable,
    #[msg("Dispute has passed its expiry and can only be expired, not resolved")]
    DisputeExpired,
}
