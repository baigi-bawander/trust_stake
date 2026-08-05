use anchor_lang::{
    prelude::*,
    system_program::{transfer, Transfer},
};

use crate::{constants::STAKE_SEED, error::TrustStakeError, state::SellerStake};

#[derive(Accounts)]
pub struct CreateStake<'info> {
    #[account(mut)]
    pub seller: Signer<'info>,

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
