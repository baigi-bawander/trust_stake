use anchor_lang::prelude::*;

#[event]
pub struct ConfigInitialized {
    pub authority: Pubkey,
    pub collateral_mint: Pubkey,
    pub chain_id: u8,
}

#[event]
pub struct ConfigAuthorityProposed {
    pub current_authority: Pubkey,
    pub pending_authority: Pubkey,
}

#[event]
pub struct ConfigAuthorityAccepted {
    pub previous_authority: Pubkey,
    pub new_authority: Pubkey,
}

#[event]
pub struct MarketplaceRegistered {
    pub marketplace: Pubkey,
    pub marketplace_id: [u8; 16],
    pub authority: Pubkey,
    pub receipt_signer: Pubkey,
    pub arbiter: Pubkey,
    pub complaint_window: i64,
    pub bond_bps: u16,
}

#[event]
pub struct MarketplaceUpdated {
    pub marketplace: Pubkey,
    pub receipt_signer: Pubkey,
    pub arbiter: Pubkey,
    pub complaint_window: i64,
    pub bond_bps: u16,
}

#[event]
pub struct MarketplaceAuthorityProposed {
    pub marketplace: Pubkey,
    pub current_authority: Pubkey,
    pub pending_authority: Pubkey,
}

#[event]
pub struct MarketplaceAuthorityAccepted {
    pub marketplace: Pubkey,
    pub previous_authority: Pubkey,
    pub new_authority: Pubkey,
}

#[event]
pub struct StakeInitialized {
    pub seller: Pubkey,
    pub stake_vault: Pubkey,
}

#[event]
pub struct StakeAdded {
    pub seller: Pubkey,
    pub amount: u64,
    pub staked: u64,
}

#[event]
pub struct StakeWithdrawn {
    pub seller: Pubkey,
    pub amount: u64,
    pub staked: u64,
}

#[event]
pub struct PermitGranted {
    pub seller: Pubkey,
    pub marketplace: Pubkey,
    pub max_slashable: u64,
    pub complaint_window: i64,
    pub bond_bps: u16,
}

#[event]
pub struct PermitIncreased {
    pub seller: Pubkey,
    pub marketplace: Pubkey,
    pub delta: u64,
    pub max_slashable: u64,
}

#[event]
pub struct PermitRevoked {
    pub seller: Pubkey,
    pub marketplace: Pubkey,
    pub revoked_at: i64,
}

#[event]
pub struct PermitReleased {
    pub seller: Pubkey,
    pub marketplace: Pubkey,
    pub remaining_allowance: u64,
}

#[event]
pub struct PermitReleasedEarly {
    pub seller: Pubkey,
    pub marketplace: Pubkey,
    pub remaining_allowance: u64,
}

#[event]
pub struct DisputeRaised {
    pub marketplace: Pubkey,
    pub seller: Pubkey,
    pub buyer: Pubkey,
    pub order_id: [u8; 32],
    pub claim: u64,
    pub bond: u64,
    pub expires_at: i64,
    pub closable_after: i64,
}

/// Carries the buyer and the outcome, which is what makes a buyer's
/// history computable offchain without storing any of it onchain
/// (docs/DESIGN-v2.md, "What this design deliberately does not do").
/// `payout` is zero on the rejected path, where the bond moves to the
/// seller instead.
#[event]
pub struct DisputeResolved {
    pub marketplace: Pubkey,
    pub seller: Pubkey,
    pub buyer: Pubkey,
    pub order_id: [u8; 32],
    pub upheld: bool,
    pub claim: u64,
    pub payout: u64,
    pub bond: u64,
}

/// The marketplace's permanent public mark for never deciding
/// (decision 7). Nobody is paid; the bond goes back to the buyer.
#[event]
pub struct DisputeExpired {
    pub marketplace: Pubkey,
    pub seller: Pubkey,
    pub buyer: Pubkey,
    pub order_id: [u8; 32],
    pub bond: u64,
}

#[event]
pub struct DisputeClosed {
    pub marketplace: Pubkey,
    pub buyer: Pubkey,
    pub order_id: [u8; 32],
    pub status: u8,
}
