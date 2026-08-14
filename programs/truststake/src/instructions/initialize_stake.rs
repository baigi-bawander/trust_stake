use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::{
    constants::{ACCOUNT_VERSION, CONFIG_SEED, SEED_VERSION, STAKE_SEED, VAULT_SEED},
    error::TrustStakeError,
    events::StakeInitialized,
    state::{Config, SellerStake},
};

/// Accounts for [`handler`]. Creates the seller's one-and-only
/// `SellerStake` PDA and its `stake_vault`, but moves no collateral;
/// `add_stake` does that. Splitting the two is what lets a seller slashed
/// to zero re-fund the same account instead of it staying permanently
/// dead, which is the prototype's hole this rebuild fixes.
#[event_cpi]
#[derive(Accounts)]
pub struct InitializeStakeAccountConstraints<'info> {
    #[account(mut)]
    pub seller: Signer<'info>,

    #[account(
        seeds = [CONFIG_SEED, SEED_VERSION],
        bump = config.bump,
    )]
    pub config: Account<'info, Config>,

    #[account(
        address = config.collateral_mint @ TrustStakeError::WrongMint,
    )]
    pub mint: InterfaceAccount<'info, Mint>,

    #[account(
        init,
        payer = seller,
        space = SellerStake::DISCRIMINATOR.len() + SellerStake::INIT_SPACE,
        seeds = [STAKE_SEED, SEED_VERSION, seller.key().as_ref()],
        bump,
    )]
    pub stake: Account<'info, SellerStake>,

    #[account(
        init,
        payer = seller,
        token::mint = mint,
        token::authority = stake,
        seeds = [VAULT_SEED, SEED_VERSION, seller.key().as_ref()],
        bump,
    )]
    pub stake_vault: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<InitializeStakeAccountConstraints>) -> Result<()> {
    ctx.accounts.stake.set_inner(SellerStake {
        version: ACCOUNT_VERSION,
        bump: ctx.bumps.stake,
        seller: ctx.accounts.seller.key(),
        staked: 0,
        committed: 0,
        disputes_total: 0,
        disputes_lost: 0,
        reserved: [0; 64],
    });

    emit_cpi!(StakeInitialized {
        seller: ctx.accounts.seller.key(),
        stake_vault: ctx.accounts.stake_vault.key(),
    });

    Ok(())
}
