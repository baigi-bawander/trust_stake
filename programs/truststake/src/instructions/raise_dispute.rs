use anchor_lang::prelude::*;
use anchor_spl::token_interface::{transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked};

use crate::{
    constants::{
        ACCOUNT_VERSION, BOND_VAULT_SEED, BPS_DENOMINATOR, CLOCK_SKEW_TOLERANCE_SECONDS, CONFIG_SEED,
        DISPUTE_EXPIRY_SECONDS, DISPUTE_SEED, MARKETPLACE_SEED, MAX_COMPLAINT_WINDOW_SECONDS, PERMIT_SEED,
        RECEIPT_DOMAIN, SEED_VERSION, STAKE_SEED,
    },
    ed25519::{verify_signed_receipt, SignedReceipt},
    error::TrustStakeError,
    events::DisputeRaised,
    state::{Config, DisputeRecord, DisputeStatus, Marketplace, SellerStake, SlashPermit},
};

/// Accounts for [`handler`]. Every account that carries data is boxed:
/// this is the widest struct in the program, and unboxed it does not fit
/// the 4KB stack frame (docs/DESIGN-v2.md, Phase 3).
///
/// `stake` and `marketplace` each self-validate against their own stored
/// fields, and `permit` derives from both of them rather than from
/// anything the caller supplies, so the three cannot be mixed and
/// matched. `mint` binds against `bond_vault.mint`: the bond is the only
/// thing this handler moves, and the vault's mint was already checked
/// against `Config` when `register_marketplace` created it.
#[event_cpi]
#[derive(Accounts)]
#[instruction(order_id: [u8; 32])]
pub struct RaiseDisputeAccountConstraints<'info> {
    #[account(mut)]
    pub buyer: Signer<'info>,

    /// Carried for `chain_id` alone, which is the one receipt field with
    /// no account of its own to check against.
    #[account(
        seeds = [CONFIG_SEED, SEED_VERSION],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, Config>>,

    #[account(
        mut,
        seeds = [MARKETPLACE_SEED, SEED_VERSION, marketplace.marketplace_id.as_ref()],
        bump = marketplace.bump,
    )]
    pub marketplace: Box<Account<'info, Marketplace>>,

    #[account(
        mut,
        seeds = [STAKE_SEED, SEED_VERSION, stake.seller.as_ref()],
        bump = stake.bump,
    )]
    pub stake: Box<Account<'info, SellerStake>>,

    #[account(
        mut,
        seeds = [PERMIT_SEED, SEED_VERSION, stake.seller.as_ref(), marketplace.key().as_ref()],
        bump = permit.bump,
    )]
    pub permit: Box<Account<'info, SlashPermit>>,

    /// The replay guard: a second complaint about the same order collides
    /// with this address and `init` fails (docs/DESIGN-v2.md, "The
    /// offchain receipt").
    #[account(
        init,
        payer = buyer,
        space = DisputeRecord::DISCRIMINATOR.len() + DisputeRecord::INIT_SPACE,
        seeds = [DISPUTE_SEED, SEED_VERSION, marketplace.key().as_ref(), stake.seller.as_ref(), order_id.as_ref()],
        bump,
    )]
    pub dispute: Box<Account<'info, DisputeRecord>>,

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
        token::authority = buyer,
    )]
    pub buyer_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: address-checked against the real Instructions sysvar inside
    /// `ed25519::verify_signed_receipt`, which is the first thing the
    /// handler does. Left unchecked here rather than carrying an
    /// `address` constraint so that the module which reads this account
    /// is also the module that proves it is the right one.
    pub instructions_sysvar: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

/// Opens a complaint against a seller, backed by a marketplace-signed
/// receipt that the transaction proves a signature over. The checks run
/// in the order docs/DESIGN-v2.md numbers them, 0 to 10; checks 0 to 4
/// live in [`crate::ed25519`], and everything from 5 on is here, which is
/// where the receipt stops being merely signed and starts having to agree
/// with the accounts actually passed in.
///
/// State is written before the bond transfer, per the house ordering rule
/// (checks, effects, interactions); the design doc's step 10 lists the
/// transfer and the counters together without ordering them against each
/// other.
///
/// `closable_after` is stored as `receipt.issued_at +
/// MAX_COMPLAINT_WINDOW_SECONDS`, never the live permit's own (possibly
/// shorter) window: a permit PDA carries no nonce, so a seller can
/// revoke, wait out the window, release, and re-grant at the same
/// address under a marketplace that has since raised its complaint
/// window. A `closable_after` frozen from the window in effect at filing
/// time would let that re-grant outlive the record that blocks this same
/// receipt from being replayed. Every permit's window is bounded to
/// `MAX_COMPLAINT_WINDOW_SECONDS` at grant time, so that constant is a
/// true upper bound for any permit that could ever occupy the PDA -- the
/// accepted cost is that a buyer's rent refund can wait up to 30 days
/// even on a marketplace with a 2-day window.
pub fn handler(ctx: Context<RaiseDisputeAccountConstraints>, order_id: [u8; 32], claim: u64) -> Result<()> {
    let SignedReceipt { signer, receipt } =
        verify_signed_receipt(&ctx.accounts.instructions_sysvar.to_account_info())?;

    let now = Clock::get()?.unix_timestamp;
    let marketplace_key = ctx.accounts.marketplace.key();
    let buyer_key = ctx.accounts.buyer.key();

    // 3. The signing key is the marketplace's current receipt signer, or
    // the one it rotated away from for a receipt issued before that
    // rotation: an honest key change must not void every outstanding
    // buyer's claim. The `signer_rotated_at != 0` guard makes the second
    // branch unreachable rather than merely unusable for a marketplace
    // that has never rotated, whose `prev_receipt_signer` is still the
    // default pubkey.
    let marketplace = &ctx.accounts.marketplace;
    let signed_by_current = signer == marketplace.receipt_signer;
    let signed_by_previous = marketplace.signer_rotated_at != 0
        && signer == marketplace.prev_receipt_signer
        && receipt.issued_at < marketplace.signer_rotated_at;
    require!(
        signed_by_current || signed_by_previous,
        TrustStakeError::WrongReceiptSigner
    );

    // 5. The receipt is for this program, this cluster, and is still
    // inside both of its lifetimes: its own expiry, and the complaint
    // window frozen on the permit at grant time (never the marketplace's
    // current setting, which it can change).
    require!(
        receipt.domain == RECEIPT_DOMAIN,
        TrustStakeError::WrongReceiptDomain
    );
    require_keys_eq!(receipt.program_id, crate::ID, TrustStakeError::WrongReceiptProgram);
    require_eq!(
        receipt.chain_id,
        ctx.accounts.config.chain_id,
        TrustStakeError::WrongChainId
    );
    require!(receipt.expires_at > now, TrustStakeError::ReceiptExpired);
    let window_closes_at = receipt
        .issued_at
        .checked_add(ctx.accounts.permit.complaint_window)
        .ok_or(TrustStakeError::MathOverflow)?;
    require!(now < window_closes_at, TrustStakeError::ComplaintWindowClosed);
    require!(
        receipt.issued_at <= now.checked_add(CLOCK_SKEW_TOLERANCE_SECONDS).ok_or(TrustStakeError::MathOverflow)?,
        TrustStakeError::ReceiptIssuedInFuture
    );

    // 6. Every party named in the signed receipt is the party actually
    // passed in. A valid signature over someone else's order must not be
    // usable here.
    require!(
        receipt.marketplace_id == ctx.accounts.marketplace.marketplace_id,
        TrustStakeError::ReceiptMarketplaceMismatch
    );
    require_keys_eq!(
        receipt.seller,
        ctx.accounts.stake.seller,
        TrustStakeError::ReceiptSellerMismatch
    );
    require_keys_eq!(receipt.buyer, buyer_key, TrustStakeError::ReceiptBuyerMismatch);
    require!(receipt.order_id == order_id, TrustStakeError::ReceiptOrderMismatch);

    // 7. A revoked permit still covers receipts issued before the
    // revocation, for as long as their window runs.
    let permit = &ctx.accounts.permit;
    require!(
        permit.revoked_at == i64::MAX || receipt.issued_at < permit.revoked_at,
        TrustStakeError::ReceiptIssuedAfterRevocation
    );

    // 8. A claim larger than the permit's remaining balance is accepted
    // and pays out whatever remains at resolution; only a claim larger
    // than the order itself is rejected.
    require!(claim > 0, TrustStakeError::ZeroAmount);
    require!(claim <= receipt.amount, TrustStakeError::ClaimExceedsReceipt);

    let bond = bond_for(claim, permit.bond_bps)?;

    // The stored replay guard's expiry, unlike `window_closes_at` above:
    // see the handler doc comment for why this must be the protocol-wide
    // maximum rather than this permit's own window.
    let closable_after = receipt
        .issued_at
        .checked_add(MAX_COMPLAINT_WINDOW_SECONDS)
        .ok_or(TrustStakeError::MathOverflow)?;

    // 9 and 10. The record is created, then the counters, then the bond
    // moves.
    ctx.accounts.dispute.set_inner(DisputeRecord {
        version: ACCOUNT_VERSION,
        bump: ctx.bumps.dispute,
        marketplace: marketplace_key,
        seller: ctx.accounts.stake.seller,
        buyer: buyer_key,
        order_id,
        claim,
        bond,
        created_at: now,
        expires_at: now
            .checked_add(DISPUTE_EXPIRY_SECONDS)
            .ok_or(TrustStakeError::MathOverflow)?,
        closable_after,
        status: DisputeStatus::Open as u8,
        reserved: [0; 32],
    });

    let permit = &mut ctx.accounts.permit;
    permit.open_disputes = permit
        .open_disputes
        .checked_add(1)
        .ok_or(TrustStakeError::MathOverflow)?;

    // Both totals count at raise time, not at resolution: an abandoned
    // complaint left outside the total would make the marketplace's
    // abandonment rate uncomputable, which is the one number decision 7
    // relies on.
    let stake = &mut ctx.accounts.stake;
    stake.disputes_total = stake
        .disputes_total
        .checked_add(1)
        .ok_or(TrustStakeError::MathOverflow)?;

    let marketplace = &mut ctx.accounts.marketplace;
    marketplace.disputes_total = marketplace
        .disputes_total
        .checked_add(1)
        .ok_or(TrustStakeError::MathOverflow)?;

    transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.buyer_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.bond_vault.to_account_info(),
                authority: ctx.accounts.buyer.to_account_info(),
            },
        ),
        bond,
        ctx.accounts.mint.decimals,
    )?;

    // No `stake_vault` reload-and-compare here, unlike every other
    // token-moving handler: this one never touches the seller's vault.
    // The bond pool's ledger is the set of open dispute records, which
    // the program cannot enumerate, so that half of conservation is
    // asserted by the test harness rather than at runtime.

    emit_cpi!(DisputeRaised {
        marketplace: marketplace_key,
        seller: ctx.accounts.dispute.seller,
        buyer: buyer_key,
        order_id,
        claim,
        bond,
        expires_at: ctx.accounts.dispute.expires_at,
        closable_after,
    });

    Ok(())
}

/// The buyer's refundable bond, `claim * bond_bps / 10_000` **rounded
/// up** (docs/DESIGN-v2.md, "Program conventions"). Rounding toward the
/// buyer paying more is what stops a claim small enough to truncate to
/// zero from buying a free complaint.
fn bond_for(claim: u64, bond_bps: u16) -> Result<u64> {
    let denominator = u128::from(BPS_DENOMINATOR);
    let rounded_up = u128::from(claim)
        .checked_mul(u128::from(bond_bps))
        .ok_or(TrustStakeError::MathOverflow)?
        .checked_add(denominator - 1)
        .ok_or(TrustStakeError::MathOverflow)?;
    u64::try_from(rounded_up / denominator).map_err(|_| TrustStakeError::MathOverflow.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::MAX_BOND_BPS;

    #[test]
    fn bond_rounds_up() {
        // 10% of one minor unit is 0.0001, which truncating division
        // would report as a free complaint.
        assert_eq!(bond_for(1, 1_000).unwrap(), 1);
        // 10% of $0.000019 is 0.0000019: still one minor unit, not two.
        assert_eq!(bond_for(19, 1_000).unwrap(), 2);
        // An exact multiple is not rounded up past itself.
        assert_eq!(bond_for(10_000_000, 1_000).unwrap(), 1_000_000);
        // Zero basis points is the only way to a zero bond, and no
        // marketplace can be registered above the ceiling.
        assert_eq!(bond_for(80_000_000, 0).unwrap(), 0);
        assert_eq!(bond_for(80_000_000, MAX_BOND_BPS).unwrap(), 16_000_000);
    }

    #[test]
    fn bond_never_wraps() {
        // u64::MAX * 2_000 overflows u64 many times over and must land in
        // u128 before it is divided back down.
        assert_eq!(
            bond_for(u64::MAX, MAX_BOND_BPS).unwrap(),
            ((u128::from(u64::MAX) * 2_000 + 9_999) / 10_000) as u64
        );
    }
}
