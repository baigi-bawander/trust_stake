use anchor_lang::prelude::*;

/// An open or resolved complaint (`["dispute", "v2", marketplace,
/// order_id]`). Declared now for the same reason as `SlashPermit`; no
/// handler creates or reads this until `raise_dispute` (Phase 3), which
/// is also where the `status` discriminants (starting at 1, so a zeroed
/// account can never read as a valid state) are defined.
#[account]
#[derive(InitSpace)]
pub struct DisputeRecord {
    pub version: u8,
    pub bump: u8,
    pub marketplace: Pubkey,
    pub seller: Pubkey,
    pub buyer: Pubkey,
    pub order_id: [u8; 32],
    pub claim: u64,
    /// The amount actually deposited, not recomputed later from a
    /// possibly-changed `bond_bps`.
    pub bond: u64,
    pub created_at: i64,
    /// `created_at + DISPUTE_EXPIRY` (30 days, protocol constant).
    pub expires_at: i64,
    /// `receipt.issued_at + permit.complaint_window`.
    pub closable_after: i64,
    pub status: u8,
    pub reserved: [u8; 32],
}
