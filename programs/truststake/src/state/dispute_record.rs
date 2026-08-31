use anchor_lang::prelude::*;

/// The states a [`DisputeRecord`] can be in. The discriminants start at 1
/// so a zeroed account can never read as a valid state
/// (docs/DESIGN-v2.md, "Account model"). Stored as the plain `u8` the
/// account model fixes, rather than as a Borsh enum, so a byte that is
/// none of these four is unreadable rather than silently mapped.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DisputeStatus {
    Open = 1,
    Upheld = 2,
    Rejected = 3,
    Abandoned = 4,
}

/// An open or resolved complaint (`["dispute", "v2", marketplace, seller,
/// order_id]`), created by `raise_dispute` and deleted by `close_dispute`
/// once its receipt is too old to reuse. The account's existence is also
/// the replay guard: a second complaint against the same order collides
/// with this PDA and cannot be created.
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
    /// `receipt.issued_at + max(permit.complaint_window,
    /// MAX_COMPLAINT_WINDOW_SECONDS)`, not the live permit's own window
    /// alone: a permit PDA carries no nonce, so a grandfathered
    /// marketplace's wider window must not let a re-grant at the same
    /// address outlive the record that blocks its own receipts from being
    /// replayed (`raise_dispute`'s handler doc comment; docs/DESIGN-v2.md).
    pub closable_after: i64,
    /// One of [`DisputeStatus`], as `DisputeStatus::Open as u8`.
    pub status: u8,
    pub reserved: [u8; 32],
}
