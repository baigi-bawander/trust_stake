use anchor_lang::{
    prelude::*,
    system_program::{transfer, Transfer},
};

use crate::{constants::STAKE_SEED, error::TrustStakeError, state::SellerStake};

/// Accounts for [`handler`]. Creates the seller's one-and-only `SellerStake`
/// PDA and funds it with their collateral in the same instruction.
#[derive(Accounts)]
pub struct CreateStake<'info> {
    /// The seller locking up collateral. Pays both the new account's rent
    /// and the staked amount itself.
    #[account(mut)]
    pub seller: Signer<'info>,

    /// One stake account per seller; re-running this after it exists fails,
    /// since there is no "add more collateral" path yet (see CLAUDE.md).
    #[account(
        init,
        payer = seller,
        space = 8 + SellerStake::INIT_SPACE,
        seeds = [STAKE_SEED, seller.key().as_ref()],
        bump,
    )]
    pub stake: Account<'info, SellerStake>,

    pub system_program: Program<'info, System>,
}

/// Locks `amount` lamports of collateral for `ctx.accounts.seller`.
///
/// Two transfers of value happen here, worth telling apart: `init` above
/// moves the stake PDA's own rent-exempt balance, while the explicit
/// `transfer` below moves the collateral on top of that. Only the latter is
/// ever slashable — see `resolve_dispute`.
pub fn handler(ctx: Context<CreateStake>, amount: u64) -> Result<()> {
    require!(amount > 0, TrustStakeError::ZeroAmount);

    // Collateral sits on the stake PDA, on top of its rent-exempt balance.
    transfer(
        CpiContext::new(
            System::id(),
            Transfer {
                from: ctx.accounts.seller.to_account_info(),
                to: ctx.accounts.stake.to_account_info(),
            },
        ),
        amount,
    )?;

    ctx.accounts.stake.set_inner(SellerStake {
        seller: ctx.accounts.seller.key(),
        staked: amount,
        disputes_lost: 0,
        bump: ctx.bumps.stake,
    });

    Ok(())
}
