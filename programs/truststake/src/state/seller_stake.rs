use anchor_lang::prelude::*;

use crate::error::TrustStakeError;

/// A seller's shared collateral, one PDA per seller globally
/// (`["stake", "v2", seller]`), backing every marketplace they sell on.
/// `initialize_stake` creates this account and its `stake_vault` only;
/// `add_stake` moves collateral. That split is what lets a seller slashed
/// to zero call `add_stake` and trade again, rather than being stuck with
/// a permanently dead account.
#[account]
#[derive(InitSpace)]
pub struct SellerStake {
    pub version: u8,
    pub bump: u8,
    pub seller: Pubkey,
    /// Always equal to `stake_vault`'s token balance.
    pub staked: u64,
    /// Sum of REMAINING allowance across active permits, that is
    /// `max_slashable - slashed` summed over every unreleased
    /// `SlashPermit` (Phase 2). Defined as the sum of caps would break
    /// `committed <= staked` the first time anyone is slashed.
    pub committed: u64,
    pub disputes_total: u32,
    pub disputes_lost: u32,
    pub reserved: [u8; 64],
}

impl SellerStake {
    /// Locks `amount` more collateral against a permit's cap. The one
    /// place `committed <= staked` is enforced, so `grant_permit` and
    /// `increase_permit` do not each restate the bound.
    pub fn commit(&mut self, amount: u64) -> Result<()> {
        let committed = self
            .committed
            .checked_add(amount)
            .ok_or(TrustStakeError::MathOverflow)?;
        require!(committed <= self.staked, TrustStakeError::CommittedExceedsStaked);
        self.committed = committed;
        Ok(())
    }

    /// Frees `amount` of previously committed collateral back to
    /// withdrawable. `release_permit` and `release_permit_early` both call
    /// this with the permit's remaining allowance.
    pub fn release_commitment(&mut self, amount: u64) -> Result<()> {
        self.committed = self
            .committed
            .checked_sub(amount)
            .ok_or(TrustStakeError::MathOverflow)?;
        Ok(())
    }
}
