use anchor_lang::prelude::*;

/// A seller's bounded, revocable line of credit to one marketplace
/// (`["permit", "v2", seller, marketplace]`). Declared now because
/// docs/DESIGN-v2.md's account model fixes all five account layouts
/// together; no handler creates or reads this until `grant_permit`
/// (Phase 2).
#[account]
#[derive(InitSpace)]
pub struct SlashPermit {
    pub version: u8,
    pub bump: u8,
    pub seller: Pubkey,
    pub marketplace: Pubkey,
    /// Increase-only: the seller can raise this but never lower it.
    pub max_slashable: u64,
    pub slashed: u64,
    /// Blocks release of THIS permit only.
    pub open_disputes: u16,
    /// `i64::MAX` means active. A zero sentinel would make the release
    /// check trivially true for every active permit.
    pub revoked_at: i64,
    /// Frozen at grant from the marketplace's setting at that moment.
    pub complaint_window: i64,
    /// Frozen at grant from the marketplace's setting at that moment.
    pub bond_bps: u16,
    pub reserved: [u8; 32],
}
