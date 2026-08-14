use anchor_lang::prelude::*;

/// Network-wide settings, one PDA per deployment (`["config", "v2"]`).
/// `collateral_mint` and `chain_id` are fixed at `initialize_config` and
/// never change: there is no `update_config`, since changing the mint once
/// vaults hold tokens would strand every balance, and the chain tag is a
/// deployment fact. Only `authority` moves, through the two-step transfer.
#[account]
#[derive(InitSpace)]
pub struct Config {
    pub version: u8,
    pub bump: u8,
    pub authority: Pubkey,
    pub pending_authority: Pubkey,
    pub collateral_mint: Pubkey,
    pub chain_id: u8,
    pub reserved: [u8; 64],
}
