use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::{
    constants::{ACCOUNT_VERSION, BOND_VAULT_SEED, CONFIG_SEED, MARKETPLACE_SEED, SEED_VERSION},
    error::TrustStakeError,
    events::MarketplaceRegistered,
    state::{validate_marketplace_settings, Config, Marketplace},
};

/// Accounts for [`handler`]. The registering authority pays rent for both
/// the `Marketplace` account and its `bond_vault`; there is no lazy
/// creation path, since `init_if_needed` is banned and `raise_dispute`
/// (Phase 3) must find the vault already there.
#[event_cpi]
#[derive(Accounts)]
#[instruction(marketplace_id: [u8; 16])]
pub struct RegisterMarketplaceAccountConstraints<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

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
        payer = authority,
        space = Marketplace::DISCRIMINATOR.len() + Marketplace::INIT_SPACE,
        seeds = [MARKETPLACE_SEED, SEED_VERSION, marketplace_id.as_ref()],
        bump,
    )]
    pub marketplace: Account<'info, Marketplace>,

    #[account(
        init,
        payer = authority,
        token::mint = mint,
        token::authority = marketplace,
        seeds = [BOND_VAULT_SEED, SEED_VERSION, marketplace.key().as_ref()],
        bump,
    )]
    pub bond_vault: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

/// Registers a new tenant. `marketplace_id` is the client-chosen,
/// immutable identity that every subordinate PDA derives from;
/// `authority`, `receipt_signer` and `arbiter` are ordinary fields that
/// rotate later through `update_marketplace` and the two-step authority
/// transfer.
pub fn handler(
    ctx: Context<RegisterMarketplaceAccountConstraints>,
    marketplace_id: [u8; 16],
    receipt_signer: Pubkey,
    arbiter: Pubkey,
    complaint_window: i64,
    bond_bps: u16,
) -> Result<()> {
    validate_marketplace_settings(complaint_window, bond_bps)?;

    ctx.accounts.marketplace.set_inner(Marketplace {
        version: ACCOUNT_VERSION,
        bump: ctx.bumps.marketplace,
        marketplace_id,
        authority: ctx.accounts.authority.key(),
        pending_authority: Pubkey::default(),
        receipt_signer,
        prev_receipt_signer: Pubkey::default(),
        signer_rotated_at: 0,
        arbiter,
        complaint_window,
        bond_bps,
        disputes_total: 0,
        disputes_upheld: 0,
        disputes_abandoned: 0,
        total_slashed: 0,
        reserved: [0; 64],
    });

    emit_cpi!(MarketplaceRegistered {
        marketplace: ctx.accounts.marketplace.key(),
        marketplace_id,
        authority: ctx.accounts.authority.key(),
        receipt_signer,
        arbiter,
        complaint_window,
        bond_bps,
    });

    Ok(())
}
