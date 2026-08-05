use anchor_lang::prelude::*;

/// Network-wide settings. Initialized once.
#[account]
#[derive(InitSpace)]
pub struct Config {
    /// The only key allowed to resolve disputes.
    pub arbiter: Pubkey,
    pub bump: u8,
}

/// A seller's collateral. This account also holds the staked lamports.
#[account]
#[derive(InitSpace)]
pub struct SellerStake {
    pub seller: Pubkey,
    /// Lamports currently staked and slashable.
    pub staked: u64,
    /// Disputes resolved against this seller. Never decreases.
    pub disputes_lost: u32,
    pub bump: u8,
}

/// An open complaint from a buyer against a seller.
#[account]
#[derive(InitSpace)]
pub struct Dispute {
    pub seller: Pubkey,
    pub buyer: Pubkey,
    /// Lamports the buyer is claiming.
    pub claim: u64,
    pub bump: u8,
}
