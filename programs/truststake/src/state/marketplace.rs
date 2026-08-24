use anchor_lang::prelude::*;

use crate::{
    constants::{MAX_BOND_BPS, MAX_COMPLAINT_WINDOW_SECONDS, MIN_COMPLAINT_WINDOW_SECONDS},
    error::TrustStakeError,
};

/// A tenant of the protocol (`["market", "v2", marketplace_id]`).
/// `marketplace_id` is the client-chosen, immutable identity every
/// subordinate PDA derives from; `authority`, `receipt_signer` and
/// `arbiter` are ordinary fields that rotate through `update_marketplace`
/// and the two-step authority transfer, so a lost or rotated key never
/// orphans a seller's permit or the dispute history.
#[account]
#[derive(InitSpace)]
pub struct Marketplace {
    pub version: u8,
    pub bump: u8,
    pub marketplace_id: [u8; 16],
    pub authority: Pubkey,
    pub pending_authority: Pubkey,
    pub receipt_signer: Pubkey,
    /// Valid for receipts issued before `signer_rotated_at`, so an honest
    /// key rotation doesn't silently void every outstanding buyer's claim.
    pub prev_receipt_signer: Pubkey,
    pub signer_rotated_at: i64,
    pub arbiter: Pubkey,
    /// 2..30 days. Copied onto each new permit at grant time (Phase 2).
    pub complaint_window: i64,
    /// <= `MAX_BOND_BPS`. Copied onto each new permit at grant time (Phase 2).
    pub bond_bps: u16,
    pub disputes_total: u32,
    pub disputes_upheld: u32,
    pub disputes_abandoned: u32,
    pub total_slashed: u64,
    pub reserved: [u8; 64],
}

/// Rejects a `complaint_window` a `Marketplace` must never carry. Split out
/// from `validate_bond_bps` so `update_marketplace` can validate only the
/// field a caller actually supplied, rather than re-checking a stored value
/// the caller left untouched against today's bounds (see that handler's
/// doc comment).
pub fn validate_complaint_window(complaint_window: i64) -> Result<()> {
    require!(
        (MIN_COMPLAINT_WINDOW_SECONDS..=MAX_COMPLAINT_WINDOW_SECONDS).contains(&complaint_window),
        TrustStakeError::ComplaintWindowOutOfBounds
    );
    Ok(())
}

/// Rejects a `bond_bps` a `Marketplace` must never carry. See
/// `validate_complaint_window`.
pub fn validate_bond_bps(bond_bps: u16) -> Result<()> {
    require!(bond_bps <= MAX_BOND_BPS, TrustStakeError::BondBpsTooHigh);
    Ok(())
}

/// Rejects settings a `Marketplace` must never carry, checked once at the
/// point they are both first accepted (`register_marketplace`, where there
/// is no stored value yet to grandfather).
pub fn validate_marketplace_settings(complaint_window: i64, bond_bps: u16) -> Result<()> {
    validate_complaint_window(complaint_window)?;
    validate_bond_bps(bond_bps)?;
    Ok(())
}
