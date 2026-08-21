use anchor_lang::prelude::*;
use anchor_spl::token_interface::{transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked};

use crate::{
    constants::{BOND_VAULT_SEED, DISPUTE_SEED, MARKETPLACE_SEED, PERMIT_SEED, SEED_VERSION, STAKE_SEED, VAULT_SEED},
    error::TrustStakeError,
    events::DisputeResolved,
    state::{DisputeRecord, DisputeStatus, Marketplace, SellerStake, SlashPermit},
};

/// Accounts for [`handler`]. Every account is chained off the
/// `DisputeRecord`, never off a caller-supplied marketplace, permit or
/// stake: `permit`, `stake` and `stake_vault` all derive their seeds from
/// `dispute.seller`, and `buyer_token_account` is bound to
/// `dispute.buyer`. Without that, anyone registers their own
/// marketplace, appoints themselves arbiter, and resolves someone else's
/// complaint, or upholds one and routes the payout to themselves
/// (docs/DESIGN-v2.md, "resolve_dispute").
///
/// `arbiter` is checked against the marketplace's *live* `arbiter` field
/// rather than a copy frozen on the permit, which is decision 10: money
/// terms freeze, keys stay live, so a staff change does not strand every
/// pending complaint.
#[event_cpi]
#[derive(Accounts)]
pub struct ResolveDisputeAccountConstraints<'info> {
    pub arbiter: Signer<'info>,

    #[account(
        mut,
        seeds = [MARKETPLACE_SEED, SEED_VERSION, marketplace.marketplace_id.as_ref()],
        bump = marketplace.bump,
        constraint = marketplace.arbiter == arbiter.key() @ TrustStakeError::NotArbiter,
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
        seeds = [STAKE_SEED, SEED_VERSION, dispute.seller.as_ref()],
        bump = stake.bump,
    )]
    pub stake: Box<Account<'info, SellerStake>>,

    #[account(
        mut,
        seeds = [VAULT_SEED, SEED_VERSION, dispute.seller.as_ref()],
        bump,
    )]
    pub stake_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [BOND_VAULT_SEED, SEED_VERSION, marketplace.key().as_ref()],
        bump,
    )]
    pub bond_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        address = stake_vault.mint @ TrustStakeError::WrongMint,
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

/// Decides an open complaint, either way.
///
/// Upheld pays the buyer `min(claim, permit remaining, staked)` out of
/// the seller's collateral and returns the bond, dropping `committed` and
/// `staked` together so the cap survives the slash. Rejected moves the
/// bond into the seller's vault instead, which is the only path where a
/// seller's free balance grows without a deposit.
///
/// `status == Open` is the first check for the reason docs/DESIGN-v2.md
/// gives: without it a never-closed record is resolvable repeatedly,
/// draining other buyers' bonds out of the shared vault and underflowing
/// `open_disputes` past the release gate.
///
/// The second check, `now < dispute.expires_at`, is the exact complement
/// of `expire_dispute`'s `now >= dispute.expires_at`: together they leave
/// no instant where a dispute is neither resolvable nor expirable.
/// Without it, `resolve_dispute` has no deadline of its own, so an
/// arbiter can slash a seller long after the dispute became expirable,
/// turning `DISPUTE_EXPIRY_SECONDS` into a race the arbiter always wins
/// rather than the seller's protection docs/DESIGN-v2.md describes.
pub fn handler(ctx: Context<ResolveDisputeAccountConstraints>, upheld: bool) -> Result<()> {
    require!(
        ctx.accounts.dispute.status == DisputeStatus::Open as u8,
        TrustStakeError::DisputeNotOpen
    );
    let now = Clock::get()?.unix_timestamp;
    require!(now < ctx.accounts.dispute.expires_at, TrustStakeError::DisputeExpired);

    let bond = ctx.accounts.dispute.bond;
    let claim = ctx.accounts.dispute.claim;
    let seller = ctx.accounts.dispute.seller;
    let marketplace_key = ctx.accounts.marketplace.key();
    let stake_bump = ctx.accounts.stake.bump;
    let marketplace_bump = ctx.accounts.marketplace.bump;
    let marketplace_id = ctx.accounts.marketplace.marketplace_id;
    let decimals = ctx.accounts.mint.decimals;

    let payout = if upheld {
        let permit_remaining = ctx
            .accounts
            .permit
            .max_slashable
            .checked_sub(ctx.accounts.permit.slashed)
            .ok_or(TrustStakeError::MathOverflow)?;
        claim.min(permit_remaining).min(ctx.accounts.stake.staked)
    } else {
        0
    };

    let permit = &mut ctx.accounts.permit;
    permit.open_disputes = permit
        .open_disputes
        .checked_sub(1)
        .ok_or(TrustStakeError::MathOverflow)?;

    ctx.accounts.dispute.status = if upheld {
        DisputeStatus::Upheld as u8
    } else {
        DisputeStatus::Rejected as u8
    };

    if upheld {
        let permit = &mut ctx.accounts.permit;
        permit.slashed = permit
            .slashed
            .checked_add(payout)
            .ok_or(TrustStakeError::MathOverflow)?;

        let stake = &mut ctx.accounts.stake;
        stake.slash(payout)?;
        stake.disputes_lost = stake
            .disputes_lost
            .checked_add(1)
            .ok_or(TrustStakeError::MathOverflow)?;

        let marketplace = &mut ctx.accounts.marketplace;
        marketplace.disputes_upheld = marketplace
            .disputes_upheld
            .checked_add(1)
            .ok_or(TrustStakeError::MathOverflow)?;
        marketplace.total_slashed = marketplace
            .total_slashed
            .checked_add(payout)
            .ok_or(TrustStakeError::MathOverflow)?;
    } else {
        let stake = &mut ctx.accounts.stake;
        stake.staked = stake
            .staked
            .checked_add(bond)
            .ok_or(TrustStakeError::MathOverflow)?;
    }

    // The seller's vault pays out under the `stake` PDA's authority; the
    // bond pool pays out under the `marketplace` PDA's.
    let stake_signer_seeds: &[&[u8]] = &[STAKE_SEED, SEED_VERSION, seller.as_ref(), &[stake_bump]];
    let marketplace_signer_seeds: &[&[u8]] = &[
        MARKETPLACE_SEED,
        SEED_VERSION,
        marketplace_id.as_ref(),
        &[marketplace_bump],
    ];

    if upheld {
        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.key(),
                TransferChecked {
                    from: ctx.accounts.stake_vault.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.buyer_token_account.to_account_info(),
                    authority: ctx.accounts.stake.to_account_info(),
                },
                &[stake_signer_seeds],
            ),
            payout,
            decimals,
        )?;
    }

    // The bond returns to the buyer when the complaint is upheld and goes
    // to the seller's collateral when it is not. Only the amount recorded
    // on the record ever moves, never one recomputed from a `bond_bps`
    // the marketplace may have raised since.
    let bond_destination = if upheld {
        ctx.accounts.buyer_token_account.to_account_info()
    } else {
        ctx.accounts.stake_vault.to_account_info()
    };
    transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.bond_vault.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: bond_destination,
                authority: ctx.accounts.marketplace.to_account_info(),
            },
            &[marketplace_signer_seeds],
        ),
        bond,
        decimals,
    )?;

    ctx.accounts.stake_vault.reload()?;
    require_gte!(
        ctx.accounts.stake_vault.amount,
        ctx.accounts.stake.staked,
        TrustStakeError::ConservationViolation
    );

    emit_cpi!(DisputeResolved {
        marketplace: marketplace_key,
        seller,
        buyer: ctx.accounts.dispute.buyer,
        order_id: ctx.accounts.dispute.order_id,
        upheld,
        claim,
        payout,
        bond,
    });

    Ok(())
}
