use anchor_lang::prelude::*;

/// Every v2 seed list carries this component so v2 PDAs cannot collide
/// with the deployed v1 program's layout.
pub const SEED_VERSION: &[u8] = b"v2";

pub const CONFIG_SEED: &[u8] = b"config";
pub const MARKETPLACE_SEED: &[u8] = b"market";
pub const STAKE_SEED: &[u8] = b"stake";
pub const VAULT_SEED: &[u8] = b"vault";
pub const BOND_VAULT_SEED: &[u8] = b"bonds";

/// Written into every account's `version` field at creation, so a later
/// phase can migrate the layout without touching accounts already holding
/// real collateral.
pub const ACCOUNT_VERSION: u8 = 1;

/// `initialize_config` only accepts this signer, so a freshly deployed
/// program cannot have its config front-run by whoever notices the
/// deployment first. This is the deploy wallet; `Config.authority` moves
/// to wherever the protocol wants afterwards, only through the two-step
/// transfer (`propose_config_authority` / `accept_config_authority`).
pub const INITIAL_ADMIN: Pubkey = pubkey!("EE4skmuEcaL4ybktFhp7sUfr84to78KQKoNsAAu8L7jG");

pub const SECONDS_PER_DAY: i64 = 24 * 60 * 60;

/// Bounds on `Marketplace.complaint_window` (and the copy frozen onto
/// each `SlashPermit` at grant time): a cash-trading marketplace and a
/// shipped-goods marketplace need genuinely different windows, so the
/// protocol only bounds the range rather than fixing one value.
pub const MIN_COMPLAINT_WINDOW_SECONDS: i64 = 2 * SECONDS_PER_DAY;
pub const MAX_COMPLAINT_WINDOW_SECONDS: i64 = 30 * SECONDS_PER_DAY;

/// Protocol ceiling on `Marketplace.bond_bps` (and the copy frozen onto
/// each `SlashPermit`): 2,000 basis points, 20%.
pub const MAX_BOND_BPS: u16 = 2_000;
