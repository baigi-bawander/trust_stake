use anchor_lang::prelude::*;
use anchor_spl::token_interface::{transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked};

use crate::{
    constants::{BOND_VAULT_SEED, DISPUTE_SEED, MARKETPLACE_SEED, PERMIT_SEED, SEED_VERSION},
    error::TrustStakeError,
    events::DisputeExpired,
    state::{DisputeRecord, DisputeStatus, Marketplace, SlashPermit},
};

/// Accounts for [`handler`]. Permissionless: `caller` is an
/// unconstrained signer, present only to pay the transaction fee, so a
/// seller frozen by a marketplace that has stopped answering does not
/// need that marketplace's cooperation to get free. Accounts chain off
/// the `DisputeRecord` exactly as they do in `resolve_dispute`.
#[event_cpi]
#[derive(Accounts)]
pub struct ExpireDisputeAccountConstraints<'info> {
    pub caller: Signer<'info>,

    #[account(
        mut,
        seeds = [MARKETPLACE_SEED, SEED_VERSION, marketplace.marketplace_id.as_ref()],
        bump = marketplace.bump,
    )]
    pub marketplace: Box<Account<'info, Marketplace>>,

    #[account(
        mut,
        seeds = [DISPUTE_SEED, SEED_VERSION, dispute.marketplace.as_ref(), dispute.order_id.as_ref()],
        bump = dispute.bump,
        has_one = marketplace,
    )]
    pub dispute: Box<Account<'info, DisputeRecord>>,

    #[account(
        mut,
        seeds = [PERMIT_SEED, SEED_VERSION, dispute.seller.as_ref(), dispute.marketplace.as_ref()],
        bump = permit.bump,
    )]
    pub permit: Box<Account<'info, SlashPermit>>,

    #[account(
        mut,
        seeds = [BOND_VAULT_SEED, SEED_VERSION, marketplace.key().as_ref()],
        bump,
    )]
    pub bond_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        address = bond_vault.mint @ TrustStakeError::WrongMint,
    )]
    pub mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        mut,
        token::mint = mint,
        token::authority = dispute.buyer,
    )]
    pub buyer_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
}

/// Abandons a complaint nobody decided within `DISPUTE_EXPIRY_SECONDS`.
/// Nobody is paid: the bond returns to the buyer in full, the freeze on
/// the permit lifts, and the marketplace takes the permanent public mark
/// for never deciding. Neither the seller's record nor the buyer's is
/// touched, because the failure belongs to whoever was supposed to
/// decide (docs/DESIGN-v2.md, "expire_dispute").
///
/// Expiring in the buyer's favour was rejected in design: it would hand
/// every genuine buyer a free option to keep the goods and reclaim the
/// money whenever a marketplace is slow.
pub fn handler(ctx: Context<ExpireDisputeAccountConstraints>) -> Result<()> {
    require!(
        ctx.accounts.dispute.status == DisputeStatus::Open as u8,
        TrustStakeError::DisputeNotOpen
    );

    let now = Clock::get()?.unix_timestamp;
    require!(
        now >= ctx.accounts.dispute.expires_at,
        TrustStakeError::DisputeNotExpired
    );

    let bond = ctx.accounts.dispute.bond;
    let marketplace_id = ctx.accounts.marketplace.marketplace_id;
    let marketplace_bump = ctx.accounts.marketplace.bump;

    let permit = &mut ctx.accounts.permit;
    permit.open_disputes = permit
        .open_disputes
        .checked_sub(1)
        .ok_or(TrustStakeError::MathOverflow)?;

    ctx.accounts.dispute.status = DisputeStatus::Abandoned as u8;

    let marketplace = &mut ctx.accounts.marketplace;
    marketplace.disputes_abandoned = marketplace
        .disputes_abandoned
        .checked_add(1)
        .ok_or(TrustStakeError::MathOverflow)?;

    let marketplace_signer_seeds: &[&[u8]] = &[
        MARKETPLACE_SEED,
        SEED_VERSION,
        marketplace_id.as_ref(),
        &[marketplace_bump],
    ];
    transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.bond_vault.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.buyer_token_account.to_account_info(),
                authority: ctx.accounts.marketplace.to_account_info(),
            },
            &[marketplace_signer_seeds],
        ),
        bond,
        ctx.accounts.mint.decimals,
    )?;

    // No `stake_vault` reload-and-compare, for the same reason
    // `raise_dispute` has none: this handler never touches the seller's
    // vault. The bond pool's ledger is the set of open dispute records,
    // which the program cannot enumerate, so that half of conservation is
    // asserted by the test harness rather than at runtime.

    emit_cpi!(DisputeExpired {
        marketplace: ctx.accounts.marketplace.key(),
        seller: ctx.accounts.dispute.seller,
        buyer: ctx.accounts.dispute.buyer,
        order_id: ctx.accounts.dispute.order_id,
        bond,
    });

    Ok(())
}
