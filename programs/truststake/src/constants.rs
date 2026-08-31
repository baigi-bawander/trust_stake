use anchor_lang::prelude::*;
use anchor_spl::token_interface::spl_token_2022::extension::ExtensionType;

/// Every v2 seed list carries this component so v2 PDAs cannot collide
/// with the deployed v1 program's layout.
pub const SEED_VERSION: &[u8] = b"v2";

pub const CONFIG_SEED: &[u8] = b"config";
pub const MARKETPLACE_SEED: &[u8] = b"market";
pub const STAKE_SEED: &[u8] = b"stake";
pub const VAULT_SEED: &[u8] = b"vault";
pub const BOND_VAULT_SEED: &[u8] = b"bonds";
pub const PERMIT_SEED: &[u8] = b"permit";
pub const DISPUTE_SEED: &[u8] = b"dispute";

/// Written into every account's `version` field at creation, so a later
/// phase can migrate the layout without touching accounts already holding
/// real collateral.
pub const ACCOUNT_VERSION: u8 = 1;

/// The constant prefix every `OrderReceipt` starts with. It carries the
/// receipt format's version, so the struct does not also need a version
/// field, and it is what stops a `receipt_signer` key that also signs
/// something else from having one of those other signatures reinterpreted
/// as a receipt.
///
/// Changing this value in an upgrade is silently unrecoverable: a receipt
/// is never stored onchain, only handed to the buyer at the moment of sale
/// and re-verified from its own bytes when `raise_dispute` runs
/// (`receipt.rs`). Every receipt a marketplace's backend has already
/// signed under the old domain is bytes sitting in buyers' hands, off the
/// chain and out of this program's reach; changing `RECEIPT_DOMAIN` makes
/// every one of them fail `raise_dispute`'s domain check forever, with no
/// migration path, because there is no account to migrate.
pub const RECEIPT_DOMAIN: [u8; 21] = *b"truststake:receipt:v1";

/// `initialize_config` only accepts this signer, so a freshly deployed
/// program cannot have its config front-run by whoever notices the
/// deployment first. This is the deploy wallet; `Config.authority` moves
/// to wherever the protocol wants afterwards, only through the two-step
/// transfer (`propose_config_authority` / `accept_config_authority`).
pub const INITIAL_ADMIN: Pubkey = pubkey!("EE4skmuEcaL4ybktFhp7sUfr84to78KQKoNsAAu8L7jG");

/// The token extensions a mint may carry and still be accepted as the
/// protocol's collateral by `initialize_config`. An allow-list rather
/// than a list of banned extensions: the Token Extensions Program keeps
/// adding extension types, and a deny-list would admit every future one
/// by default, including whatever the next one turns out to break.
///
/// Empty is the strict starting point, not a placeholder. Every entry
/// added here is a promise that the protocol's five token-moving handlers
/// behave correctly against a mint carrying it, which for most extensions
/// means proving that the amount credited to a vault equals the amount
/// debited from the sender. Widening it later is a deliberate and safe
/// change: it only ever admits mints a fresh deployment could pin, and
/// `Config.collateral_mint` is pinned per deployment, so no already-live
/// deployment's collateral changes underneath it.
pub const ALLOWED_MINT_EXTENSIONS: &[ExtensionType] = &[];

/// The only two values `Config.chain_id` may hold; `initialize_config`
/// rejects everything else. `chain_id` is pinned forever once set (there is
/// no `update_config`), so an unchecked value here is unrepairable: a typo
/// would let every receipt from the wrong cluster verify successfully
/// (`raise_dispute`'s `WrongChainId` check compares against whatever is
/// stored, not against reality).
pub const CHAIN_ID_DEVNET: u8 = 1;
pub const CHAIN_ID_MAINNET: u8 = 2;

pub const SECONDS_PER_DAY: i64 = 24 * 60 * 60;

/// Bounds on `Marketplace.complaint_window` (and the copy frozen onto
/// each `SlashPermit` at grant time): a cash-trading marketplace and a
/// shipped-goods marketplace need genuinely different windows, so the
/// protocol only bounds the range rather than fixing one value.
pub const MIN_COMPLAINT_WINDOW_SECONDS: i64 = 2 * SECONDS_PER_DAY;
pub const MAX_COMPLAINT_WINDOW_SECONDS: i64 = 30 * SECONDS_PER_DAY;

/// `validate_complaint_window` (`state/marketplace.rs`) rejects anything
/// outside `MIN_COMPLAINT_WINDOW_SECONDS..=MAX_COMPLAINT_WINDOW_SECONDS`.
/// If the floor ever exceeded the ceiling that range would be empty, so no
/// `complaint_window` value could ever pass, and `register_marketplace`
/// would reject every caller with `ComplaintWindowOutOfBounds`.
const _: () = assert!(
    MIN_COMPLAINT_WINDOW_SECONDS <= MAX_COMPLAINT_WINDOW_SECONDS,
    "MIN_COMPLAINT_WINDOW_SECONDS must not exceed MAX_COMPLAINT_WINDOW_SECONDS -- validate_complaint_window's range would be empty and register_marketplace would always fail"
);

/// Protocol ceiling on `Marketplace.bond_bps` (and the copy frozen onto
/// each `SlashPermit`): 2,000 basis points, 20%.
pub const MAX_BOND_BPS: u16 = 2_000;

/// Basis points in one whole, the divisor every `bond_bps` calculation
/// narrows back through.
pub const BPS_DENOMINATOR: u16 = 10_000;

/// `raise_dispute`'s `bond_for` (`instructions/raise_dispute.rs`) computes
/// `claim * bond_bps / BPS_DENOMINATOR`. If `MAX_BOND_BPS` ever exceeded
/// `BPS_DENOMINATOR`, a permit granted at the ceiling could demand a bond
/// larger than the claim itself -- more than 100% collateral to file one
/// complaint. Currently safe by 5x.
const _: () = assert!(
    MAX_BOND_BPS <= BPS_DENOMINATOR,
    "MAX_BOND_BPS must not exceed BPS_DENOMINATOR -- a bond above 100% of the claim"
);

/// How long a complaint may sit undecided before anyone may expire it
/// (decision 7). Fixed protocol-wide rather than per-marketplace: it is
/// the seller's protection against the marketplace that judges the
/// complaint, so the marketplace does not get to choose it.
///
/// Equal to `MAX_COMPLAINT_WINDOW_SECONDS` today, but incidentally: a
/// prior audit traced every site that reads either constant and confirmed
/// nothing depends on the two being equal. Do not build anything on that
/// equality holding, and do not assert it -- doing so would falsely imply
/// a dependency exists and would block a legitimate future edit to either
/// one.
pub const DISPUTE_EXPIRY_SECONDS: i64 = 30 * SECONDS_PER_DAY;

/// How far ahead of the onchain clock a receipt's `issued_at` may be
/// dated and still pass `raise_dispute`. A hard `issued_at <= now` is too
/// strict: Solana's onchain clock and a marketplace's signing server are
/// not synchronised, and the chain clock has historically lagged real
/// time, so a receipt signed at the honest instant it is issued can still
/// arrive with `issued_at` slightly ahead of `Clock::get()`. One hour is
/// negligible against the 30-day windows this protocol deals in, but
/// closes off a receipt dated years ahead, which would otherwise push
/// `closable_after` out by the same margin and leave the record's rent
/// stuck for as long.
///
/// Also reused, unmodified, as `release_permit_early`'s minimum wait
/// after revocation (see that handler's doc comment for the full
/// argument). That reuse is load-bearing: `raise_dispute` check 7 accepts
/// a receipt down to `granted_at - CLOCK_SKEW_TOLERANCE_SECONDS`, and the
/// early-release wait is only a safe substitute for the complaint window
/// because it is bounded by this SAME value. Widening this constant
/// widens both sides together; introducing a second constant for either
/// side would let them drift apart and reopen the cross-era replay this
/// pairing closes.
pub const CLOCK_SKEW_TOLERANCE_SECONDS: i64 = 60 * 60;

/// `raise_dispute`'s check 7 (`instructions/raise_dispute.rs`) rejects a
/// stale-era receipt left over from a permit wound down through the slow
/// `release_permit` path only while this holds. Derivation:
/// `release_permit` forces the permit closed no earlier than
/// `revoked_at + complaint_window`; `grant_permit`'s `init` can only
/// re-occupy that PDA after the close, so the earliest a fresh
/// `granted_at` can land is that same bound; check 7 accepts a receipt
/// whenever `receipt.issued_at >= granted_at - CLOCK_SKEW_TOLERANCE_SECONDS`;
/// and every old-era receipt is necessarily dated before `revoked_at`.
/// Chaining those: an old-era receipt is rejected only for as long as
/// `CLOCK_SKEW_TOLERANCE_SECONDS` does not exceed the wound-down permit's
/// own `complaint_window`, and the shortest a window is ever allowed to be
/// is `MIN_COMPLAINT_WINDOW_SECONDS`. The complaint-window check earlier
/// in the handler (5) does not independently cover this: by the time it
/// runs it is reading the *successor* permit's window, not the wound-down
/// one the stale receipt actually belongs to. Currently safe by 48x (one
/// hour against a two-day floor); violate it and a receipt from a fully
/// wound-down era becomes filable against whatever gets granted next at
/// the same permit PDA (docs/DESIGN-v2.md, "Honest limitations").
const _: () = assert!(
    CLOCK_SKEW_TOLERANCE_SECONDS <= MIN_COMPLAINT_WINDOW_SECONDS,
    "CLOCK_SKEW_TOLERANCE_SECONDS must not exceed MIN_COMPLAINT_WINDOW_SECONDS -- raise_dispute check 7 would no longer reject a stale-era receipt after the slow release_permit path"
);
