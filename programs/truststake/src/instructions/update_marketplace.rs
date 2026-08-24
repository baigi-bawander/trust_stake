use anchor_lang::prelude::*;

use crate::{
    constants::{MARKETPLACE_SEED, SEED_VERSION},
    events::MarketplaceUpdated,
    state::{validate_bond_bps, validate_complaint_window, Marketplace},
};

/// Accounts for [`handler`]. `seeds` re-derive the PDA from the loaded
/// account's own `marketplace_id`, never from a caller-supplied ID, so a
/// substituted `Marketplace` account can never pass this check (see
/// docs/DESIGN-v2.md, "Program conventions").
#[event_cpi]
#[derive(Accounts)]
pub struct UpdateMarketplaceAccountConstraints<'info> {
    pub authority: Signer<'info>,

    #[account(
        mut,
        seeds = [MARKETPLACE_SEED, SEED_VERSION, marketplace.marketplace_id.as_ref()],
        bump = marketplace.bump,
        has_one = authority,
    )]
    pub marketplace: Account<'info, Marketplace>,
}

/// Updates whichever settings are `Some`; the rest keep their current
/// value. A receipt-signer change stamps `prev_receipt_signer` and
/// `signer_rotated_at` only when the key actually differs, so old
/// receipts stay valid across a rotation (Phase 3's `raise_dispute`
/// relies on this). Window and bond changes never touch permits already
/// granted; those froze their own copies at grant time (Phase 2).
///
/// Only a field the caller actually supplied is validated. A stored value
/// the caller left untouched is grandfathered rather than re-checked
/// against today's bounds: if a later upgrade tightens `MAX_BOND_BPS`, a
/// marketplace whose already-stored `bond_bps` falls outside the new range
/// must still be able to call this handler to rotate an unrelated field,
/// `receipt_signer` above all, during a key-compromise incident. Re-running
/// both checks unconditionally would lock that marketplace out of its own
/// emergency rotation until it first lowered a bond rate nobody asked it
/// to touch.
pub fn handler(
    ctx: Context<UpdateMarketplaceAccountConstraints>,
    new_receipt_signer: Option<Pubkey>,
    new_arbiter: Option<Pubkey>,
    new_complaint_window: Option<i64>,
    new_bond_bps: Option<u16>,
) -> Result<()> {
    let marketplace = &mut ctx.accounts.marketplace;

    if let Some(complaint_window) = new_complaint_window {
        validate_complaint_window(complaint_window)?;
        marketplace.complaint_window = complaint_window;
    }

    if let Some(bond_bps) = new_bond_bps {
        validate_bond_bps(bond_bps)?;
        marketplace.bond_bps = bond_bps;
    }

    if let Some(receipt_signer) = new_receipt_signer {
        if receipt_signer != marketplace.receipt_signer {
            marketplace.prev_receipt_signer = marketplace.receipt_signer;
            marketplace.receipt_signer = receipt_signer;
            marketplace.signer_rotated_at = Clock::get()?.unix_timestamp;
        }
    }

    if let Some(arbiter) = new_arbiter {
        marketplace.arbiter = arbiter;
    }

    emit_cpi!(MarketplaceUpdated {
        marketplace: marketplace.key(),
        receipt_signer: marketplace.receipt_signer,
        arbiter: marketplace.arbiter,
        complaint_window: marketplace.complaint_window,
        bond_bps: marketplace.bond_bps,
    });

    Ok(())
}
