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
