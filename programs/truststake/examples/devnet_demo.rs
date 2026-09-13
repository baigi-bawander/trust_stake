//! Walks TWO original marketplaces sharing ONE seller's collateral pool,
//! then a THIRD marketplace and a SECOND seller, against Solana devnet,
//! printing a transaction signature and an explorer URL for every step, so
//! every one of the program's 19 handlers (`lib.rs`) is visible onchain, not
//! only in tests. docs/DESIGN-v2.md's Build order assigns this file's v2
//! rewrite to Phase 4 Step 2; this revision extends that same walk to the
//! remaining 11 handlers across four wall-clock-gated stages.
//!
//! tests/test_devnet_demo_parity.rs's test_devnet_demo_sequence_parity
//! replays this exact instruction sequence, same handlers and same
//! arguments, against LiteSVM (which can warp its own clock) before any of
//! it touches devnet. Read that test alongside this file, and change both
//! together: if the sequence below changes, that test is what catches a
//! mistake for free instead of this script teaching it at real cost.
//!
//! Run with:
//!   cargo run --example devnet_demo --features devnet_demo --manifest-path programs/truststake/Cargo.toml
//!
//! Optionally with `--stage N` (N = 1..=4) to attempt only that stage; the
//! default attempts every stage, in order, and each stage (and each
//! time-gated action inside stage 4) independently checks whether it is due
//! yet, printing what remains and exiting 0 rather than failing when it is
//! not.
//!
//! The wallet at ~/.config/solana/id.json (or TRUSTSTAKE_ADMIN_KEYPAIR, if
//! set) pays for setup and must equal constants::INITIAL_ADMIN, the only
//! signer initialize_config accepts. Every throwaway wallet below is funded
//! by a direct System Program transfer rather than an airdrop, since devnet
//! airdrops are rate-limited, and every funding transfer tops up to a
//! target balance rather than sending a fixed amount unconditionally, so
//! re-running an already-funded wallet costs nothing.
//!
//! ## Resumability, and why nearly everything below checks chain state first
//!
//! This script is meant to be run more than once: three of the protocol's
//! waits (release_permit_early's clock-skew tolerance, release_permit's
//! complaint window, expire_dispute/close_dispute's 30-day marks) cannot be
//! skipped on devnet, because devnet runs on real clock time. The four
//! stages below exist for exactly that reason -- see "Why four stages" in
//! the task history for the full derivation -- and a user checking on stage
//! 2's progress two days after stage 1 must re-run this same binary and
//! reach stage 2's logic without stage 1's already-finished steps failing
//! or repeating first.
//!
//! That requirement turned out to reach further back than the four new
//! stages: of the original eight steps below (initialize_config through
//! resolve_dispute), only initialize_config and the test mint were actually
//! idempotent before this revision. `register_marketplace`,
//! `initialize_stake`, and `grant_permit` all use Anchor's `init`, which
//! hard-errors on a second call against an already-occupied PDA; the two
//! marketplace IDs and both throwaway token accounts were generated fresh
//! (via `Keypair::new()`) on every run with no persistence at all; and
//! `raise_dispute`'s `order_id` was likewise fresh-random every run, so a
//! second run would have raised (and upheld) a second, unwanted $80 claim
//! rather than erroring. Since stage 2/3/4 require this file's `main` to
//! run start-to-finish on every later invocation, any one of those gaps
//! would have broken the very first resumed run, before it ever reached the
//! new stage logic. Every step below is now gated on live chain state (does
//! the account already exist, does its recorded value already match) the
//! same way the original `initialize_config` step already gated on
//! `Config` existing, and the two marketplace IDs and every dispute
//! `order_id` this file cares about are recovered by scanning the
//! program's own accounts (`find_marketplace_by_authority`,
//! `find_dispute_by_marketplace_seller_status`) rather than assumed, since
//! the original 2026-08-24 run predates any of this file's persistence and
//! left no record of the random IDs it chose.
use {
    anchor_lang::{
        prelude::{Discriminator, Pubkey, Space},
        solana_program::{instruction::Instruction, program_pack::Pack, system_instruction},
        AccountDeserialize, InstructionData, ToAccountMetas,
    },
    anchor_spl::token::spl_token,
    serde_json::{json, Value},
    solana_commitment_config::CommitmentConfig,
    solana_ed25519_program::new_ed25519_instruction_with_signature,
    solana_instruction::error::InstructionError,
    solana_keypair::{read_keypair_file, write_keypair_file, Keypair},
    solana_message::{Message, VersionedMessage},
    solana_rpc_client::{
        api::{config::RpcSendTransactionConfig, request::TokenAccountsFilter},
        rpc_client::RpcClient,
    },
    solana_signature::Signature,
    solana_signer::Signer,
    solana_transaction::versioned::VersionedTransaction,
    solana_transaction_error::TransactionError,
    std::{
        env,
        error::Error,
        fs,
        path::{Path, PathBuf},
        thread::sleep,
        time::{Duration, SystemTime, UNIX_EPOCH},
    },
    truststake::{
        constants::{
            BOND_VAULT_SEED, CHAIN_ID_DEVNET, CLOCK_SKEW_TOLERANCE_SECONDS, CONFIG_SEED, DISPUTE_SEED,
            INITIAL_ADMIN, MARKETPLACE_SEED, MAX_COMPLAINT_WINDOW_SECONDS,
            PERMIT_SEED, RECEIPT_DOMAIN, SECONDS_PER_DAY, SEED_VERSION, STAKE_SEED, VAULT_SEED,
        },
        error::TrustStakeError,
        receipt::OrderReceipt,
        state::{Config, DisputeRecord, DisputeStatus, Marketplace, SellerStake, SlashPermit},
    },
};

const DEVNET_URL: &str = "https://api.devnet.solana.com";

/// USDC's decimals; the demo's test mint matches it exactly
/// (docs/DESIGN-v2.md, "Instruction handlers") so every figure below reads
/// as dollars. `usdc(500)` is `500_000_000`.
const MINT_DECIMALS: u8 = 6;

const fn usdc(major_units: u64) -> u64 {
    major_units * 10u64.pow(MINT_DECIMALS as u32)
}

fn format_usdc(minor_units: u64) -> String {
    format!("{:.6}", minor_units as f64 / usdc(1) as f64)
}

/// devnet's tag; mainnet is `CHAIN_ID_MAINNET` (docs/DESIGN-v2.md, "Instruction handlers").
const DEVNET_CHAIN_ID: u8 = CHAIN_ID_DEVNET;

const STAKE_AMOUNT: u64 = usdc(500);
const CASHDESK_PERMIT: u64 = usdc(200);
const PIXELBAZAAR_PERMIT: u64 = usdc(200);
/// Step 6's amounts are derived from LIVE staked/committed at the moment
/// the step runs, never from constants: `SellerStake::slash` moves staked
/// and committed together, so a fresh-chain assumption ("committed is
/// always 400") breaks the instant any dispute has ever been upheld
/// against this seller -- which happened in the 2026-08-24 run, and on
/// every later run that reaches Step 7. If free collateral (staked -
/// committed) is below this floor, the step tops it up first, the same
/// pattern 1.6 already uses, rather than skip the demonstration or risk a
/// zero-amount withdrawal.
const WITHDRAW_DEMO_MIN_FREE: u64 = usdc(20);
const ORDER_AMOUNT: u64 = usdc(80);
const CLAIM_AMOUNT: u64 = ORDER_AMOUNT;
/// Comfortably covers the bond (10% of ORDER_AMOUNT = usdc(8)) with
/// headroom, the same rent-floor-headroom reasoning applied to token
/// balances rather than lamports.
const BUYER_TOKEN_FUNDING: u64 = usdc(10);

const CASHDESK_COMPLAINT_WINDOW_SECONDS: i64 = 2 * SECONDS_PER_DAY;
const PIXELBAZAAR_COMPLAINT_WINDOW_SECONDS: i64 = 7 * SECONDS_PER_DAY;
const CASHDESK_BOND_BPS: u16 = 1_000; // 10%
const PIXELBAZAAR_BOND_BPS: u16 = 500; // 5%, visibly distinct from CashDesk's

// ==================== Stage 1 constants ====================

/// Zero is legal on purpose (CLAUDE.md's deliberate-tradeoffs list); this is
/// the first time devnet shows both ends of the legal bond range at once,
/// next to CashDesk's 1,000 bps.
const SWIFTMARKET_ID: [u8; 16] = *b"swiftmarket-demo";
const SWIFTMARKET_COMPLAINT_WINDOW_SECONDS: i64 = MAX_COMPLAINT_WINDOW_SECONDS;
const SWIFTMARKET_BOND_BPS: u16 = 0;

const PIXELBAZAAR_NEW_BOND_BPS: u16 = 300;

/// 1.6 tops up the ORIGINAL seller's free collateral (staked - committed) to
/// at least this much before granting SwiftMarket's permit, rather than
/// adding a fixed amount unconditionally: on devnet today that free balance
/// is already zero (500 staked, 400 committed, 80 slashed --
/// docs/TESTING.md), so a fresh run adds exactly the $200 the task
/// describes, and a resumed run adds only whatever is still missing.
const SELLER_FREE_COLLATERAL_TARGET: u64 = usdc(200);

const SWIFTMARKET_GRANT: u64 = usdc(100);
const SWIFTMARKET_INCREASE: u64 = usdc(50);
const SWIFTMARKET_PERMIT_TOTAL: u64 = SWIFTMARKET_GRANT + SWIFTMARKET_INCREASE;

const SWIFTMARKET_ORDER_AMOUNT: u64 = usdc(80);
const SWIFTMARKET_CLAIM: u64 = usdc(40);
/// SwiftMarket's bond is 0, but the buyer's token account still needs to
/// exist for the (zero-amount) transfer_checked CPI to succeed.
const SWIFTMARKET_BUYER_TOKEN_FUNDING: u64 = usdc(1);

const CASHDESK_SECOND_ORDER_AMOUNT: u64 = usdc(60);
const CASHDESK_SECOND_CLAIM: u64 = usdc(25);
/// How far before CashDesk's receipt-signer rotation (1.4) the second
/// dispute's receipt (1.10) is backdated, so it unambiguously predates
/// `signer_rotated_at` and exercises `raise_dispute`'s `signed_by_previous`
/// branch rather than landing on the boundary.
const BACKDATE_BEFORE_ROTATION_SECONDS: i64 = 60;

const SELLER_B_STAKE: u64 = usdc(100);
const SELLER_B_CASHDESK_PERMIT: u64 = usdc(50);
const SELLER_B_PIXELBAZAAR_PERMIT: u64 = usdc(50);

/// A few multiples of Solana's 5,000-lamport base fee per signature
/// (https://docs.anza.xyz/consensus/fees#current-fee-structure), as
/// headroom on top of rent for each throwaway wallet's transactions.
const SIGNATURE_FEE_HEADROOM: u64 = 20_000;

/// Where each throwaway wallet's keypair is persisted, one file per role in
/// the standard Solana CLI keypair format (`solana_keypair::write_keypair_file`
/// writes exactly what `read_keypair_file` reads). This is what lets a
/// failed or partial run be inspected and resumed with the same wallets
/// rather than orphaning whatever the previous run funded and left
/// mid-walk. `.gitignore` excludes this directory; nothing here is ever
/// printed, only the corresponding pubkeys.
fn keypairs_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/.devnet-demo-keypairs")
}

/// Where stage progress and recovered chain facts (marketplace IDs, dispute
/// order IDs, revocation timestamps) are persisted between invocations --
/// see the module doc comment's "Resumability" section. `.gitignore`
/// excludes this directory.
fn state_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/.devnet-demo-state")
}

fn progress_path() -> PathBuf {
    state_dir().join("progress.json")
}

/// Loads the progress file, or an empty object if this is the first
/// invocation under this revision of the script.
fn load_progress() -> Result<Value, Box<dyn Error>> {
    let path = progress_path();
    if !path.exists() {
        return Ok(json!({}));
    }
    let text = fs::read_to_string(&path).map_err(|error| format!("failed to read {path:?}: {error}"))?;
    Ok(serde_json::from_str(&text).map_err(|error| format!("failed to parse {path:?} as JSON: {error}"))?)
}

fn save_progress(progress: &Value) -> Result<(), Box<dyn Error>> {
    let dir = state_dir();
    fs::create_dir_all(&dir).map_err(|error| format!("failed to create {dir:?}: {error}"))?;
    let path = progress_path();
    let text = serde_json::to_string_pretty(progress).expect("Value serialization cannot fail");
    fs::write(&path, text).map_err(|error| format!("failed to write {path:?}: {error}").into())
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_decode(text: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    if text.len() % 2 != 0 {
        return Err(format!("hex string {text:?} has odd length").into());
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|error| format!("invalid hex in {text:?}: {error}").into()))
        .collect()
}

fn get_bytes16(progress: &Value, key: &str) -> Option<[u8; 16]> {
    let text = progress.get(key)?.as_str()?;
    let bytes = hex_decode(text).ok()?;
    bytes.try_into().ok()
}

fn set_bytes16(progress: &mut Value, key: &str, value: [u8; 16]) {
    progress[key] = json!(hex_encode(&value));
}

fn get_bytes32(progress: &Value, key: &str) -> Option<[u8; 32]> {
    let text = progress.get(key)?.as_str()?;
    let bytes = hex_decode(text).ok()?;
    bytes.try_into().ok()
}

fn set_bytes32(progress: &mut Value, key: &str, value: [u8; 32]) {
    progress[key] = json!(hex_encode(&value));
}

fn get_i64(progress: &Value, key: &str) -> Option<i64> {
    progress.get(key)?.as_i64()
}

fn set_i64(progress: &mut Value, key: &str, value: i64) {
    progress[key] = json!(value);
}

fn get_bool(progress: &Value, key: &str) -> bool {
    progress.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn set_bool(progress: &mut Value, key: &str, value: bool) {
    progress[key] = json!(value);
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time must be after the unix epoch")
        .as_secs() as i64
}

/// Reads `--stage N` off argv, if present. `N` must be 1..=4.
fn parse_stage_arg() -> Result<Option<u8>, Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    for i in 1..args.len() {
        if args[i] == "--stage" {
            let value = args
                .get(i + 1)
                .ok_or("--stage requires a value (1-4)")?;
            let stage: u8 = value
                .parse()
                .map_err(|_| format!("--stage value must be an integer 1-4, got {value:?}"))?;
            if !(1..=4).contains(&stage) {
                return Err(format!("--stage must be 1-4, got {stage}").into());
            }
            return Ok(Some(stage));
        }
    }
    Ok(None)
}

/// Loads `{dir}/{name}.json` if a previous run already created it, so a
/// resumed run reuses the same wallet (and therefore the same onchain
/// accounts) instead of generating an unfunded stranger; otherwise
/// generates a fresh keypair and persists it for next time.
fn load_or_create_keypair(dir: &Path, name: &str) -> Result<Keypair, Box<dyn Error>> {
    let path = dir.join(format!("{name}.json"));
    if path.exists() {
        return read_keypair_file(&path).map_err(|error| format!("failed to read {path:?}: {error}").into());
    }
    let keypair = Keypair::new();
    write_keypair_file(&keypair, &path).map_err(|error| format!("failed to write {path:?}: {error}"))?;
    Ok(keypair)
}

/// Bundles the values almost every function below needs, so their
/// signatures carry one parameter for "the chain" rather than five.
struct Chain<'a> {
    client: &'a RpcClient,
    program_id: Pubkey,
    event_authority: Pubkey,
    config: Pubkey,
    mint: Pubkey,
}

fn marketplace_pda(program_id: Pubkey, marketplace_id: [u8; 16]) -> Pubkey {
    Pubkey::find_program_address(&[MARKETPLACE_SEED, SEED_VERSION, marketplace_id.as_ref()], &program_id).0
}

fn bond_vault_pda(program_id: Pubkey, marketplace: Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[BOND_VAULT_SEED, SEED_VERSION, marketplace.as_ref()], &program_id).0
}

fn stake_pda(program_id: Pubkey, seller: Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[STAKE_SEED, SEED_VERSION, seller.as_ref()], &program_id).0
}

fn stake_vault_pda(program_id: Pubkey, seller: Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[VAULT_SEED, SEED_VERSION, seller.as_ref()], &program_id).0
}

fn permit_pda(program_id: Pubkey, seller: Pubkey, marketplace: Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[PERMIT_SEED, SEED_VERSION, seller.as_ref(), marketplace.as_ref()], &program_id).0
}

fn dispute_pda(program_id: Pubkey, marketplace: Pubkey, seller: Pubkey, order_id: [u8; 32]) -> Pubkey {
    Pubkey::find_program_address(
        &[DISPUTE_SEED, SEED_VERSION, marketplace.as_ref(), seller.as_ref(), order_id.as_ref()],
        &program_id,
    )
    .0
}

// ==================== Guard decisions (pure functions) ====================
//
// Every guard below that decides "have I already done this?" makes that
// decision here first -- no RpcClient, no I/O -- so `#[cfg(test)] mod
// tests` at the bottom of this file can exercise every branch directly,
// including the ones a resumed run only reaches after an earlier step has
// destroyed or changed the very state a naive existence check would have
// relied on. See docs/DEMO-SCRIPT-FINDINGS.md for the defects these close.

/// Whether to grant a permit at a PDA this script might have granted
/// before. `lifecycle_done` is checked BEFORE existence: once a permit's
/// full cycle (grant, revoke, release) has actually completed, this
/// returns `SkipLifecycleDone` even though the account no longer exists on
/// chain. An existence-only check (D1's bug) cannot tell "never granted"
/// apart from "granted, then released by a later stage of this same
/// script" and re-grants into the second case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PermitGrantDecision {
    Grant,
    SkipLifecycleDone,
    SkipAlreadyGranted,
}

fn decide_permit_grant(lifecycle_done: bool, permit_exists: bool) -> PermitGrantDecision {
    if lifecycle_done {
        PermitGrantDecision::SkipLifecycleDone
    } else if permit_exists {
        PermitGrantDecision::SkipAlreadyGranted
    } else {
        PermitGrantDecision::Grant
    }
}

/// Whether to call `revoke_permit` on a permit this run's grant step just
/// confirmed exists. `SkipStaleCache` is the ensure_revoked-staleness fix:
/// a cached `revoked_at` from a permit's PRIOR era at this same PDA must
/// never be handed out as if it described the CURRENT, still-unrevoked
/// permit -- and revoking on that era's behalf isn't this run's call
/// either (D1: a permit this run did not grant isn't this run's to
/// act on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PermitRevokeDecision {
    Revoke,
    UseLiveRevokedAt(i64),
    SkipLifecycleDone,
    SkipStaleCache,
}

fn decide_permit_revoke(lifecycle_done: bool, live_revoked_at: Option<i64>, cached_revoked_at: Option<i64>) -> PermitRevokeDecision {
    if lifecycle_done {
        return PermitRevokeDecision::SkipLifecycleDone;
    }
    match live_revoked_at {
        Some(revoked_at) => PermitRevokeDecision::UseLiveRevokedAt(revoked_at),
        None if cached_revoked_at.is_some() => PermitRevokeDecision::SkipStaleCache,
        None => PermitRevokeDecision::Revoke,
    }
}

/// Whether to call `release_permit`/`release_permit_early` on a permit
/// stage 1 already granted and (usually) revoked. `permit_exists` alone is
/// exactly the D1-mirror bug: it cannot distinguish "released, done" from
/// "never granted." `SkipUnrevoked` is what keeps this guard from ever
/// calling release on a permit that ISN'T revoked -- true both mid-flow
/// (revoke hasn't run yet this invocation) and for the stray shape D1
/// describes (a previous bug re-granted at this PDA after release; this
/// permit's current era isn't this guard's to finish).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PermitReleaseDecision {
    Proceed { revoked_at: i64 },
    NotYetDue { remaining_seconds: i64 },
    SkipLifecycleDone,
    SkipAlreadyReleased,
    SkipUnrevoked,
    NotYetReachable,
}

fn decide_permit_release(
    lifecycle_done: bool,
    permit_exists: bool,
    live_revoked_at: Option<i64>,
    cached_revoked_at: Option<i64>,
    wait_seconds: i64,
    now: i64,
) -> PermitReleaseDecision {
    if lifecycle_done {
        return PermitReleaseDecision::SkipLifecycleDone;
    }
    if !permit_exists {
        return if cached_revoked_at.is_some() {
            PermitReleaseDecision::SkipAlreadyReleased
        } else {
            PermitReleaseDecision::NotYetReachable
        };
    }
    let Some(revoked_at) = live_revoked_at else {
        return PermitReleaseDecision::SkipUnrevoked;
    };
    let due_at = revoked_at + wait_seconds;
    if now < due_at {
        PermitReleaseDecision::NotYetDue { remaining_seconds: due_at - now }
    } else {
        PermitReleaseDecision::Proceed { revoked_at }
    }
}

/// Whether Step 1.6 still needs to top up the original seller's free
/// collateral before SwiftMarket's permit is granted. D3's bug was
/// re-evaluating "free >= target" on every run, including runs where
/// 1.7/1.8 already committed against that free balance -- the target only
/// ever meant "before the grant." Once the permit exists, this step's job
/// is done regardless of what free collateral looks like now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopUpDecision {
    TopUp { shortfall: u64 },
    SkipPermitAlreadyGranted,
    SkipAlreadyAtTarget,
}

fn decide_free_collateral_topup(swiftmarket_permit_exists: bool, free: u64, target: u64) -> TopUpDecision {
    if swiftmarket_permit_exists {
        TopUpDecision::SkipPermitAlreadyGranted
    } else if free >= target {
        TopUpDecision::SkipAlreadyAtTarget
    } else {
        TopUpDecision::TopUp { shortfall: target - free }
    }
}

/// Whether Step 7's one-time CashDesk dispute still needs raising.
/// `cashdesk_permit_slashed` is read from the SlashPermit account, which
/// stage 4 never closes (only DisputeRecord accounts are closable) -- so
/// unlike a scan for an Upheld DisputeRecord, this signal survives stage
/// 4.2 forever, and it is already correct the very first time this flag is
/// consulted, even against a dispute raised before this flag existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step7DisputeDecision {
    Proceed,
    SkipFlagSet,
    SkipAlreadySlashed,
}

fn decide_step7_dispute(step7_done_flag: bool, cashdesk_permit_slashed: u64, claim_amount: u64) -> Step7DisputeDecision {
    if step7_done_flag {
        Step7DisputeDecision::SkipFlagSet
    } else if cashdesk_permit_slashed >= claim_amount {
        Step7DisputeDecision::SkipAlreadySlashed
    } else {
        Step7DisputeDecision::Proceed
    }
}

/// Whether stage 1.10's second CashDesk dispute still needs resolving.
/// `dispute_status` is `None` when the DisputeRecord no longer exists --
/// stage 4.2 closes exactly this record, and `close_dispute` only ever
/// succeeds on a non-Open dispute (`try_close_dispute` checks that itself),
/// so "gone" always implies "was resolved," never "still open."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SecondDisputeResolveDecision {
    Proceed,
    SkipFlagSet,
    SkipDisputeGone,
    SkipAlreadyResolved,
}

fn decide_second_dispute_resolve(resolved_flag: bool, dispute_status: Option<u8>) -> SecondDisputeResolveDecision {
    if resolved_flag {
        return SecondDisputeResolveDecision::SkipFlagSet;
    }
    match dispute_status {
        None => SecondDisputeResolveDecision::SkipDisputeGone,
        Some(status) if status != DisputeStatus::Open as u8 => SecondDisputeResolveDecision::SkipAlreadyResolved,
        Some(_) => SecondDisputeResolveDecision::Proceed,
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let keypair_path = env::var("TRUSTSTAKE_ADMIN_KEYPAIR").unwrap_or_else(|_| {
        format!(
            "{}/.config/solana/id.json",
            env::var("HOME").expect("HOME must be set to locate the Solana CLI keypair")
        )
    });
    let admin = read_keypair_file(&keypair_path)
        .map_err(|error| format!("failed to read keypair at {keypair_path}: {error}"))?;

    // Every run after the first must reuse the SAME admin keypair: it is
    // both the only signer initialize_config ever accepts (checked
    // onchain) and, informally, the mint authority this script chose on
    // the first run. A different keypair here fails fast, before spending
    // anything, rather than partway through.
    if admin.pubkey() != INITIAL_ADMIN {
        return Err(format!(
            "keypair at {keypair_path} (pubkey {}) does not match constants::INITIAL_ADMIN ({}); \
             set TRUSTSTAKE_ADMIN_KEYPAIR to the deploy wallet's keypair file",
            admin.pubkey(),
            INITIAL_ADMIN
        )
        .into());
    }

    let forced_stage = parse_stage_arg()?;
    let should_run = |stage: u8| forced_stage.is_none() || forced_stage == Some(stage);

    let program_id = truststake::id();
    let config = Pubkey::find_program_address(&[CONFIG_SEED, SEED_VERSION], &program_id).0;
    let event_authority = Pubkey::find_program_address(&[b"__event_authority"], &program_id).0;

    let keypairs_dir = keypairs_dir();
    fs::create_dir_all(&keypairs_dir).map_err(|error| format!("failed to create {keypairs_dir:?}: {error}"))?;
    let seller = load_or_create_keypair(&keypairs_dir, "seller")?;
    let buyer = load_or_create_keypair(&keypairs_dir, "buyer")?;
    let cashdesk_authority = load_or_create_keypair(&keypairs_dir, "cashdesk_authority")?;
    let cashdesk_receipt_signer = load_or_create_keypair(&keypairs_dir, "cashdesk_receipt_signer")?;
    let cashdesk_arbiter = load_or_create_keypair(&keypairs_dir, "cashdesk_arbiter")?;
    let pixelbazaar_authority = load_or_create_keypair(&keypairs_dir, "pixelbazaar_authority")?;
    let pixelbazaar_receipt_signer = load_or_create_keypair(&keypairs_dir, "pixelbazaar_receipt_signer")?;
    let pixelbazaar_arbiter = load_or_create_keypair(&keypairs_dir, "pixelbazaar_arbiter")?;

    println!("Admin (deploy wallet, INITIAL_ADMIN): {}", admin.pubkey());
    println!("Seller (throwaway):                   {}", seller.pubkey());
    println!("Buyer (throwaway):                    {}", buyer.pubkey());
    println!("CashDesk authority (throwaway):       {}", cashdesk_authority.pubkey());
    println!("CashDesk receipt signer (unfunded):   {}", cashdesk_receipt_signer.pubkey());
    println!("CashDesk arbiter (unfunded):          {}", cashdesk_arbiter.pubkey());
    println!("PixelBazaar authority (throwaway):    {}", pixelbazaar_authority.pubkey());
    println!("PixelBazaar receipt signer (unfunded):{}", pixelbazaar_receipt_signer.pubkey());
    println!("PixelBazaar arbiter (unfunded):       {}", pixelbazaar_arbiter.pubkey());
    println!(
        "(a receipt signer only ever signs an offchain Ed25519 message, never a Solana \
         transaction, and an arbiter only ever signs -- never pays for -- resolve_dispute, so \
         neither role needs a funded wallet)"
    );
    println!();

    let client = RpcClient::new_with_commitment(DEVNET_URL, CommitmentConfig::confirmed());
    let mut progress = load_progress()?;

    fund_wallets(
        &client,
        &admin,
        &seller,
        &buyer,
        &cashdesk_authority,
        &pixelbazaar_authority,
    )?;

    // ==================== Step 1: test mint and starting balances ====================
    println!("=== Step 1: test mint and starting balances ===");
    let existing_config = client.get_account(&config).ok();

    let mint = if let Some(account) = &existing_config {
        let config_state = Config::try_deserialize(&mut account.data.as_slice())
            .map_err(|error| format!("failed to deserialize the Config account: {error}"))?;
        println!(
            "Config already exists at {config}; reusing its pinned mint {}",
            config_state.collateral_mint
        );
        config_state.collateral_mint
    } else {
        // Real devnet USDC exists but this program cannot mint it, so the
        // demo pins its own 6-decimal test mint here, then hands it to
        // initialize_config below -- the pinning site. initialize_config has
        // no update path: once a mint is pinned into Config, a wrong choice
        // can only be replaced by a fresh deployment, never repaired.
        let (mint, signature) = create_test_mint(&client, &admin)?;
        print_step("Create test mint", &signature);
        mint
    };

    let seller_token_account =
        load_or_find_or_create_token_account(&client, &keypairs_dir, "seller", &admin, &mint, &seller.pubkey())?;
    let buyer_token_account =
        load_or_find_or_create_token_account(&client, &keypairs_dir, "buyer", &admin, &mint, &buyer.pubkey())?;

    top_up_token_balance(&client, &admin, &mint, &seller_token_account, STAKE_AMOUNT)?;
    top_up_token_balance(&client, &admin, &mint, &buyer_token_account, BUYER_TOKEN_FUNDING)?;
    println!();

    // ==================== Step 2: initialize_config ====================
    println!("=== Step 2: initialize_config (chain_id = {DEVNET_CHAIN_ID}, devnet) ===");
    if existing_config.is_some() {
        println!("Already done in a previous run, skipping");
    } else {
        let instruction = Instruction::new_with_bytes(
            program_id,
            &truststake::instruction::InitializeConfig { chain_id: DEVNET_CHAIN_ID }.data(),
            truststake::accounts::InitializeConfigAccountConstraints {
                admin: admin.pubkey(),
                config,
                mint,
                system_program: anchor_lang::system_program::ID,
                event_authority,
                program: program_id,
            }
            .to_account_metas(None),
        );
        let signature = send(&client, &[instruction], &admin.pubkey(), &[&admin])?;
        print_step("initialize_config", &signature);
    }
    println!();

    let chain = Chain { client: &client, program_id, event_authority, config, mint };

    // ==================== Step 3: register/recover two marketplaces ====================
    println!("=== Step 3: register two marketplaces against the same seller's future collateral ===");
    let (cashdesk, cashdesk_id) = find_or_register_legacy_marketplace(
        &chain,
        &mut progress,
        "cashdesk_marketplace_id",
        &cashdesk_authority,
        cashdesk_receipt_signer.pubkey(),
        cashdesk_arbiter.pubkey(),
        CASHDESK_COMPLAINT_WINDOW_SECONDS,
        CASHDESK_BOND_BPS,
        "CashDesk",
    )?;
    save_progress(&progress)?;
    let (pixelbazaar, _pixelbazaar_id) = find_or_register_legacy_marketplace(
        &chain,
        &mut progress,
        "pixelbazaar_marketplace_id",
        &pixelbazaar_authority,
        pixelbazaar_receipt_signer.pubkey(),
        pixelbazaar_arbiter.pubkey(),
        PIXELBAZAAR_COMPLAINT_WINDOW_SECONDS,
        PIXELBAZAAR_BOND_BPS,
        "PixelBazaar",
    )?;
    save_progress(&progress)?;
    println!();

    // ==================== Step 4: seller stakes ====================
    // "Tops up to" rather than "locks, once": a slash (Step 7, or a prior
    // run's Step 7) reduces staked, and this target-based top-up is what
    // makes a resumed run restore it rather than leave it short.
    println!("=== Step 4: seller tops up staked collateral to {} ===", format_usdc(STAKE_AMOUNT));
    let stake = stake_pda(program_id, seller.pubkey());
    let stake_vault = stake_vault_pda(program_id, seller.pubkey());
    if client.get_account(&stake).is_err() {
        let instruction = Instruction::new_with_bytes(
            program_id,
            &truststake::instruction::InitializeStake {}.data(),
            truststake::accounts::InitializeStakeAccountConstraints {
                seller: seller.pubkey(),
                config,
                mint,
                stake,
                stake_vault,
                token_program: spl_token::ID,
                system_program: anchor_lang::system_program::ID,
                event_authority,
                program: program_id,
            }
            .to_account_metas(None),
        );
        let signature = send(&client, &[instruction], &seller.pubkey(), &[&seller])?;
        print_step("initialize_stake", &signature);
    } else {
        println!("initialize_stake: already done, skipping");
    }
    {
        let current = read_seller_stake(&client, &stake)?.staked;
        if current < STAKE_AMOUNT {
            let shortfall = STAKE_AMOUNT - current;
            top_up_token_balance(&client, &admin, &mint, &seller_token_account, shortfall)?;
            let instruction = Instruction::new_with_bytes(
                program_id,
                &truststake::instruction::AddStake { amount: shortfall }.data(),
                truststake::accounts::AddStakeAccountConstraints {
                    seller: seller.pubkey(),
                    stake,
                    stake_vault,
                    mint,
                    seller_token_account,
                    token_program: spl_token::ID,
                    event_authority,
                    program: program_id,
                }
                .to_account_metas(None),
            );
            let signature = send(&client, &[instruction], &seller.pubkey(), &[&seller])?;
            print_step(&format!("add_stake({})", format_usdc(shortfall)), &signature);
        } else {
            println!("add_stake: staked is already {} >= {}, skipping", format_usdc(current), format_usdc(STAKE_AMOUNT));
        }
    }
    println!();

    // ==================== Step 5: grant a permit to each marketplace ====================
    println!("=== Step 5: seller grants each marketplace its own permit, from the SAME stake ===");
    ensure_permit(&chain, &progress, "seller_cashdesk_permit_lifecycle_done", &seller, stake, cashdesk, CASHDESK_PERMIT, "CashDesk")?;
    ensure_permit(&chain, &progress, "seller_pixelbazaar_permit_lifecycle_done", &seller, stake, pixelbazaar, PIXELBAZAAR_PERMIT, "PixelBazaar")?;

    let cashdesk_permit_addr = permit_pda(program_id, seller.pubkey(), cashdesk);
    let pixelbazaar_permit_addr = permit_pda(program_id, seller.pubkey(), pixelbazaar);
    let cashdesk_permit_state = read_permit(&client, &cashdesk_permit_addr)?;
    let pixelbazaar_permit_state = read_permit(&client, &pixelbazaar_permit_addr)?;
    let stake_state = read_seller_stake(&client, &stake)?;
    // Each permit's REMAINING allowance (cap - slashed), read live, not the
    // amount it was originally granted: a prior slash (Step 7, on an
    // earlier run) leaves the grant amount unchanged but the remaining
    // allowance lower, and `committed` (SellerStake) already tracks the
    // remaining figure, not the granted one, so this breakdown must too or
    // it wouldn't add back up to the number just printed.
    println!(
        "committed = {} of {} staked ({} CashDesk remaining + {} PixelBazaar remaining)",
        format_usdc(stake_state.committed),
        format_usdc(stake_state.staked),
        format_usdc(cashdesk_permit_state.max_slashable - cashdesk_permit_state.slashed),
        format_usdc(pixelbazaar_permit_state.max_slashable - pixelbazaar_permit_state.slashed),
    );
    println!();

    // ==================== Step 6: withdraw free collateral, then hit the cap ====================
    println!("=== Step 6: withdraw free collateral, then demonstrate the cap refuses more (one-time demo) ===");
    if get_bool(&progress, "step6_withdraw_demo_done") {
        println!("Already demonstrated in a previous run, skipping (re-demonstrating a one-time boundary proof is not meaningful).");
    } else {
        let stake_state = read_seller_stake(&client, &stake)?;
        let mut free = stake_state.staked.saturating_sub(stake_state.committed);
        println!(
            "  Live state: staked = {}, committed = {}, free = {}",
            format_usdc(stake_state.staked),
            format_usdc(stake_state.committed),
            format_usdc(free)
        );
        if free < WITHDRAW_DEMO_MIN_FREE {
            let shortfall = WITHDRAW_DEMO_MIN_FREE - free;
            println!(
                "  Free collateral {} is below the {} this demo needs; topping up by {} first.",
                format_usdc(free),
                format_usdc(WITHDRAW_DEMO_MIN_FREE),
                format_usdc(shortfall)
            );
            top_up_token_balance(&client, &admin, &mint, &seller_token_account, shortfall)?;
            let instruction = Instruction::new_with_bytes(
                program_id,
                &truststake::instruction::AddStake { amount: shortfall }.data(),
                truststake::accounts::AddStakeAccountConstraints {
                    seller: seller.pubkey(),
                    stake,
                    stake_vault,
                    mint,
                    seller_token_account,
                    token_program: spl_token::ID,
                    event_authority,
                    program: program_id,
                }
                .to_account_metas(None),
            );
            let signature = send(&client, &[instruction], &seller.pubkey(), &[&seller])?;
            print_step(&format!("add_stake({}) -- topping up free collateral for the withdraw demo", format_usdc(shortfall)), &signature);
            free = WITHDRAW_DEMO_MIN_FREE;
        }

        let withdraw_amount = free / 2;
        {
            let instruction = Instruction::new_with_bytes(
                program_id,
                &truststake::instruction::WithdrawStake { amount: withdraw_amount }.data(),
                truststake::accounts::WithdrawStakeAccountConstraints {
                    seller: seller.pubkey(),
                    stake,
                    stake_vault,
                    mint,
                    seller_token_account,
                    token_program: spl_token::ID,
                    event_authority,
                    program: program_id,
                }
                .to_account_metas(None),
            );
            let signature = send(&client, &[instruction], &seller.pubkey(), &[&seller])?;
            print_step(&format!("withdraw_stake({}) -- succeeds: free collateral was {}", format_usdc(withdraw_amount), format_usdc(free)), &signature);
        }

        let stake_after_first = read_seller_stake(&client, &stake)?;
        let free_after = stake_after_first.staked.saturating_sub(stake_after_first.committed);
        let withdraw_attempt_too_much = free_after + 1;
        println!(
            "  After that withdrawal: staked = {}, committed = {}, free = {}. Attempting to withdraw {}, which the cap must refuse...",
            format_usdc(stake_after_first.staked),
            format_usdc(stake_after_first.committed),
            format_usdc(free_after),
            format_usdc(withdraw_attempt_too_much)
        );
        {
            let instruction = Instruction::new_with_bytes(
                program_id,
                &truststake::instruction::WithdrawStake { amount: withdraw_attempt_too_much }.data(),
                truststake::accounts::WithdrawStakeAccountConstraints {
                    seller: seller.pubkey(),
                    stake,
                    stake_vault,
                    mint,
                    seller_token_account,
                    token_program: spl_token::ID,
                    event_authority,
                    program: program_id,
                }
                .to_account_metas(None),
            );
            // send_allowing_failure, not send: this transaction is SUPPOSED
            // to fail, and the point is proving the cap is enforced
            // onchain by the deployed program, for a real fee, rather than
            // merely rejected by client-side preflight simulation before
            // anything was spent.
            let (signature, outcome) = send_allowing_failure(&client, &[instruction], &seller.pubkey(), &[&seller])?;
            print_step(&format!("withdraw_stake({}) -- must fail", format_usdc(withdraw_attempt_too_much)), &signature);
            match outcome {
                Some(TransactionError::InstructionError(_, InstructionError::Custom(code)))
                    if code == u32::from(TrustStakeError::CommittedExceedsStaked) =>
                {
                    println!(
                        "  Failed exactly as designed: CommittedExceedsStaked (committed collateral \
                         would exceed staked collateral)"
                    );
                }
                Some(other) => {
                    return Err(format!("expected CommittedExceedsStaked, got a different onchain failure: {other:?}").into());
                }
                None => return Err("expected the second withdrawal to fail onchain, but it succeeded".into()),
            }
        }
        set_bool(&mut progress, "step6_withdraw_demo_done", true);
        save_progress(&progress)?;
    }
    println!();

    // ==================== Step 7: a buyer disputes an order on CashDesk ====================
    println!("=== Step 7: a CashDesk buyer disputes a {} order (one-time demo) ===", format_usdc(ORDER_AMOUNT));
    let cashdesk_permit_addr = permit_pda(program_id, seller.pubkey(), cashdesk);
    let cashdesk_permit_before_step7 = read_permit(&client, &cashdesk_permit_addr)?;
    let step7_done_flag = get_bool(&progress, "step7_cashdesk_dispute_done");
    match decide_step7_dispute(step7_done_flag, cashdesk_permit_before_step7.slashed, CLAIM_AMOUNT) {
        Step7DisputeDecision::SkipFlagSet => {
            println!("Already demonstrated in a previous run, skipping.");
        }
        Step7DisputeDecision::SkipAlreadySlashed => {
            // Migration path: this flag postdates the 2026-08-24 run that
            // actually raised and upheld this claim. `slashed` on the
            // SlashPermit account is never reset by anything later in this
            // script (unlike the DisputeRecord stage 4.2 eventually closes),
            // so it proves the claim already landed even with the flag unset.
            println!(
                "Already demonstrated (CashDesk's permit already shows {} slashed, from before this flag existed), skipping.",
                format_usdc(cashdesk_permit_before_step7.slashed)
            );
            set_bool(&mut progress, "step7_cashdesk_dispute_done", true);
            save_progress(&progress)?;
        }
        Step7DisputeDecision::Proceed => {
            let order_id = Keypair::new().pubkey().to_bytes();
            let issued_at = now_unix();
            let receipt = OrderReceipt {
                domain: RECEIPT_DOMAIN,
                program_id,
                chain_id: DEVNET_CHAIN_ID,
                marketplace_id: cashdesk_id,
                seller: seller.pubkey(),
                buyer: buyer.pubkey(),
                order_id,
                amount: ORDER_AMOUNT,
                issued_at,
                expires_at: issued_at + 7 * SECONDS_PER_DAY,
            };
            // Build the signed bytes with OrderReceipt::message(), never by
            // hand: the Ed25519 header below asserts message_data_size
            // EXACTLY, and every field of OrderReceipt is fixed-width for that
            // reason (src/receipt.rs).
            let message = receipt.message();
            let signature_bytes: [u8; 64] = cashdesk_receipt_signer.sign_message(&message).into();
            let verify_instruction = new_ed25519_instruction_with_signature(
                &message,
                &signature_bytes,
                &cashdesk_receipt_signer.pubkey().to_bytes(),
            );

            let cashdesk_permit = permit_pda(program_id, seller.pubkey(), cashdesk);
            let dispute = dispute_pda(program_id, cashdesk, seller.pubkey(), order_id);
            let cashdesk_bond_vault = bond_vault_pda(program_id, cashdesk);
            let raise_dispute_instruction = Instruction::new_with_bytes(
                program_id,
                &truststake::instruction::RaiseDispute { order_id, claim: CLAIM_AMOUNT }.data(),
                truststake::accounts::RaiseDisputeAccountConstraints {
                    buyer: buyer.pubkey(),
                    config,
                    marketplace: cashdesk,
                    stake,
                    permit: cashdesk_permit,
                    dispute,
                    bond_vault: cashdesk_bond_vault,
                    mint,
                    buyer_token_account,
                    instructions_sysvar: solana_instructions_sysvar::ID,
                    token_program: spl_token::ID,
                    system_program: anchor_lang::system_program::ID,
                    event_authority,
                    program: program_id,
                }
                .to_account_metas(None),
            );
            // The Ed25519 verify instruction MUST sit immediately before
            // raise_dispute in the same transaction: raise_dispute derives its
            // position as current_index - 1 (docs/DESIGN-v2.md, "raise_dispute,
            // the one with real complexity", check 1). They cannot be split
            // across two transactions.
            let signature = send(&client, &[verify_instruction, raise_dispute_instruction], &buyer.pubkey(), &[&buyer])?;
            print_step("[Ed25519 verify, raise_dispute]", &signature);

            // CashDesk's arbiter resolves upheld. admin is the fee payer;
            // cashdesk_arbiter only co-signs, since resolve_dispute's `arbiter`
            // account is never `mut` -- it authorizes the ruling and pays
            // nothing, which is exactly the "trusted judge, not a funded
            // participant" role decision 2 (docs/DESIGN-v2.md) describes.
            let resolve_instruction = Instruction::new_with_bytes(
                program_id,
                &truststake::instruction::ResolveDispute { upheld: true }.data(),
                truststake::accounts::ResolveDisputeAccountConstraints {
                    arbiter: cashdesk_arbiter.pubkey(),
                    marketplace: cashdesk,
                    dispute,
                    permit: cashdesk_permit,
                    stake,
                    stake_vault,
                    bond_vault: cashdesk_bond_vault,
                    mint,
                    buyer_token_account,
                    token_program: spl_token::ID,
                    event_authority,
                    program: program_id,
                }
                .to_account_metas(None),
            );
            let signature = send(&client, &[resolve_instruction], &admin.pubkey(), &[&admin, &cashdesk_arbiter])?;
            print_step("resolve_dispute(upheld = true)", &signature);
            println!("CashDesk upheld the claim; see the payout reflected in CashDesk's permit below.");
            set_bool(&mut progress, "step7_cashdesk_dispute_done", true);
            save_progress(&progress)?;
        }
    }
    println!();

    // ==================== Step 8: the payoff -- one slashed, one untouched ====================
    println!("=== Step 8: final state of both original permits ===");
    let cashdesk_permit = permit_pda(program_id, seller.pubkey(), cashdesk);
    let pixelbazaar_permit = permit_pda(program_id, seller.pubkey(), pixelbazaar);
    let cashdesk_permit_state = read_permit(&client, &cashdesk_permit)?;
    let pixelbazaar_permit_state = read_permit(&client, &pixelbazaar_permit)?;

    if cashdesk_permit_state.slashed != CLAIM_AMOUNT {
        return Err(format!(
            "expected CashDesk's permit to have slashed {}, found {}",
            format_usdc(CLAIM_AMOUNT),
            format_usdc(cashdesk_permit_state.slashed)
        )
        .into());
    }
    if pixelbazaar_permit_state.slashed != 0 {
        return Err(format!(
            "expected PixelBazaar's permit to be untouched, found {} slashed",
            format_usdc(pixelbazaar_permit_state.slashed)
        )
        .into());
    }

    println!(
        "  CashDesk    permit: cap = {}, slashed = {}, remaining = {}",
        format_usdc(cashdesk_permit_state.max_slashable),
        format_usdc(cashdesk_permit_state.slashed),
        format_usdc(cashdesk_permit_state.max_slashable - cashdesk_permit_state.slashed),
    );
    println!(
        "  PixelBazaar permit: cap = {}, slashed = {}, remaining = {}",
        format_usdc(pixelbazaar_permit_state.max_slashable),
        format_usdc(pixelbazaar_permit_state.slashed),
        format_usdc(pixelbazaar_permit_state.max_slashable - pixelbazaar_permit_state.slashed),
    );
    println!();

    // ==================== Stages 1-4 ====================
    let roster = StageRoster {
        admin: &admin,
        seller: &seller,
        buyer: &buyer,
        seller_token_account,
        buyer_token_account,
        cashdesk,
        cashdesk_id,
        cashdesk_authority: &cashdesk_authority,
        cashdesk_receipt_signer: &cashdesk_receipt_signer,
        cashdesk_arbiter: &cashdesk_arbiter,
        pixelbazaar,
        pixelbazaar_authority: &pixelbazaar_authority,
        stake,
    };

    if should_run(1) {
        run_stage_1(&chain, &mut progress, &roster)?;
        save_progress(&progress)?;
        println!();
    }
    if should_run(2) {
        run_stage_2(&chain, &mut progress, &roster)?;
        println!();
    }
    if should_run(3) {
        run_stage_3(&chain, &mut progress, &roster)?;
        println!();
    }
    if should_run(4) {
        run_stage_4(&chain, &progress, &roster)?;
        println!();
    }

    Ok(())
}

/// The values stage 1-4 functions need from the original eight-step walk.
/// Bundled for the same reason `Chain` is: these functions already take
/// several stage-specific parameters of their own.
struct StageRoster<'a> {
    admin: &'a Keypair,
    seller: &'a Keypair,
    buyer: &'a Keypair,
    seller_token_account: Pubkey,
    buyer_token_account: Pubkey,
    cashdesk: Pubkey,
    cashdesk_id: [u8; 16],
    cashdesk_authority: &'a Keypair,
    cashdesk_receipt_signer: &'a Keypair,
    cashdesk_arbiter: &'a Keypair,
    pixelbazaar: Pubkey,
    pixelbazaar_authority: &'a Keypair,
    stake: Pubkey,
}

/// Funds every wallet that pays rent or a transaction fee of its own, each
/// topped up to its target balance rather than sent a fixed amount
/// unconditionally, so a resumed run funds only the shortfall (or nothing).
/// Receipt signers and arbiters are deliberately excluded: neither ever
/// pays for anything (see the printed roster note in `main`).
fn fund_wallets(
    client: &RpcClient,
    admin: &Keypair,
    seller: &Keypair,
    buyer: &Keypair,
    cashdesk_authority: &Keypair,
    pixelbazaar_authority: &Keypair,
) -> Result<(), Box<dyn Error>> {
    let wallet_rent_floor = client.get_minimum_balance_for_rent_exemption(0)?;
    let stake_rent = client.get_minimum_balance_for_rent_exemption(SellerStake::DISCRIMINATOR.len() + SellerStake::INIT_SPACE)?;
    let token_account_rent = client.get_minimum_balance_for_rent_exemption(spl_token::state::Account::LEN)?;
    let permit_rent = client.get_minimum_balance_for_rent_exemption(SlashPermit::DISCRIMINATOR.len() + SlashPermit::INIT_SPACE)?;
    let marketplace_rent =
        client.get_minimum_balance_for_rent_exemption(Marketplace::DISCRIMINATOR.len() + Marketplace::INIT_SPACE)?;
    let dispute_rent =
        client.get_minimum_balance_for_rent_exemption(DisputeRecord::DISCRIMINATOR.len() + DisputeRecord::INIT_SPACE)?;

    // Seller signs 6 transactions across the original walk:
    // initialize_stake (pays stake_rent AND stake_vault's
    // token_account_rent together), add_stake, two grant_permit calls
    // (each pays permit_rent), and two withdraw_stake calls -- the second
    // of which is expected to fail, but a transaction that fails onchain
    // execution still pays its base fee. Stage 1 funds its own additional
    // seller transactions separately (fund_wallets_stage1).
    let seller_funding = stake_rent + token_account_rent + 2 * permit_rent + 6 * SIGNATURE_FEE_HEADROOM + wallet_rent_floor;
    top_up_balance(client, admin, &seller.pubkey(), seller_funding, "seller")?;

    // Buyer signs 1 transaction: raise_dispute (two instructions, one
    // signature), which pays dispute_rent.
    let buyer_funding = dispute_rent + SIGNATURE_FEE_HEADROOM + wallet_rent_floor;
    top_up_balance(client, admin, &buyer.pubkey(), buyer_funding, "buyer")?;

    // Each marketplace authority signs 1 transaction: register_marketplace,
    // which pays rent for both the marketplace account and its bond_vault.
    let marketplace_authority_funding = marketplace_rent + token_account_rent + SIGNATURE_FEE_HEADROOM + wallet_rent_floor;
    top_up_balance(client, admin, &cashdesk_authority.pubkey(), marketplace_authority_funding, "CashDesk authority")?;
    top_up_balance(client, admin, &pixelbazaar_authority.pubkey(), marketplace_authority_funding, "PixelBazaar authority")?;
    println!();
    Ok(())
}

/// Transfers `admin -> to` only the shortfall needed to reach `target`
/// lamports, skipping entirely if `to` is already funded. This is what
/// makes every funding call in this file safe to repeat on every
/// invocation rather than only the first.
fn top_up_balance(client: &RpcClient, admin: &Keypair, to: &Pubkey, target: u64, label: &str) -> Result<(), Box<dyn Error>> {
    let current = client.get_balance(to)?;
    if current >= target {
        println!("Fund {label}: already at {current} lamports (target {target}), skipping");
        return Ok(());
    }
    let shortfall = target - current;
    let signature = transfer_sol(client, admin, to, shortfall)?;
    print_step(&format!("Fund {label} ({shortfall} lamports, topping up to {target})"), &signature);
    Ok(())
}

/// Mints only the shortfall needed for `token_account` to hold at least
/// `target` minor units, skipping entirely if it already does.
fn top_up_token_balance(client: &RpcClient, mint_authority: &Keypair, mint: &Pubkey, token_account: &Pubkey, target: u64) -> Result<(), Box<dyn Error>> {
    let current = read_token_balance(client, token_account)?;
    if current >= target {
        return Ok(());
    }
    let shortfall = target - current;
    let signature = mint_to_account(client, mint_authority, mint, token_account, shortfall)?;
    print_step(&format!("Mint {} to {token_account}", format_usdc(shortfall)), &signature);
    Ok(())
}

fn read_token_balance(client: &RpcClient, token_account: &Pubkey) -> Result<u64, Box<dyn Error>> {
    let account = client.get_account(token_account)?;
    Ok(spl_token::state::Account::unpack(&account.data)
        .map_err(|error| format!("failed to unpack token account {token_account}: {error}"))?
        .amount)
}

/// Finds an existing token account for `owner` under `mint` (via
/// `getTokenAccountsByOwner`, filtered server-side) and persists it, or
/// creates and persists a new one if none exists. The persisted file is
/// what makes every later invocation reuse the SAME token account rather
/// than the onchain scan being needed every time; the scan itself is what
/// recovers the seller's and buyer's real token accounts from the
/// 2026-08-24 run, which predates this persistence file existing at all
/// (see the module doc comment's "Resumability" section).
fn load_or_find_or_create_token_account(
    client: &RpcClient,
    dir: &Path,
    name: &str,
    payer: &Keypair,
    mint: &Pubkey,
    owner: &Pubkey,
) -> Result<Pubkey, Box<dyn Error>> {
    let path = dir.join(format!("{name}_token_account.pubkey"));
    if path.exists() {
        let text = fs::read_to_string(&path).map_err(|error| format!("failed to read {path:?}: {error}"))?;
        let pubkey: Pubkey = text.trim().parse().map_err(|error| format!("invalid pubkey in {path:?}: {error}"))?;
        return Ok(pubkey);
    }

    let existing = client.get_token_accounts_by_owner(owner, TokenAccountsFilter::Mint(*mint))?;
    let token_account = if let Some(entry) = existing.first() {
        let pubkey: Pubkey = entry
            .pubkey
            .parse()
            .map_err(|error| format!("RPC returned an invalid pubkey {:?}: {error}", entry.pubkey))?;
        println!("Recovered existing {name} token account from chain: {pubkey}");
        pubkey
    } else {
        let (token_account, signature) = create_token_account(client, payer, mint, owner)?;
        print_step(&format!("Create {name} token account"), &signature);
        token_account
    };

    fs::write(&path, token_account.to_string()).map_err(|error| format!("failed to write {path:?}: {error}"))?;
    Ok(token_account)
}

/// Grants `permit` for `marketplace` unless its lifecycle is already fully
/// complete (`lifecycle_key` in progress.json) or it already exists.
/// `lifecycle_key` is checked BEFORE existence -- see
/// `decide_permit_grant`'s doc comment for why: an existence-only check
/// (Anchor's `init` hard-errors against an already-occupied PDA, so SOME
/// check is required for Step 5 to be safe to repeat) cannot tell "never
/// granted" apart from "granted, then released by a later stage of this
/// same script," and re-grants into the second case (D1).
fn ensure_permit(
    chain: &Chain,
    progress: &Value,
    lifecycle_key: &str,
    seller: &Keypair,
    stake: Pubkey,
    marketplace: Pubkey,
    max_slashable: u64,
    name: &str,
) -> Result<(), Box<dyn Error>> {
    let permit = permit_pda(chain.program_id, seller.pubkey(), marketplace);
    let lifecycle_done = get_bool(progress, lifecycle_key);
    let permit_exists = chain.client.get_account(&permit).is_ok();
    match decide_permit_grant(lifecycle_done, permit_exists) {
        PermitGrantDecision::SkipLifecycleDone => {
            println!("grant_permit({name}): permit lifecycle already complete (granted, revoked, released), skipping.");
        }
        PermitGrantDecision::SkipAlreadyGranted => {
            println!("grant_permit({name}): already granted, skipping");
        }
        PermitGrantDecision::Grant => {
            let signature = send_grant_permit(chain, seller, stake, marketplace, max_slashable)?;
            print_step(&format!("grant_permit({name}, {})", format_usdc(max_slashable)), &signature);
        }
    }
    Ok(())
}

fn send_grant_permit(
    chain: &Chain,
    seller: &Keypair,
    stake: Pubkey,
    marketplace: Pubkey,
    max_slashable: u64,
) -> Result<Signature, Box<dyn Error>> {
    let permit = permit_pda(chain.program_id, seller.pubkey(), marketplace);
    let instruction = Instruction::new_with_bytes(
        chain.program_id,
        &truststake::instruction::GrantPermit { max_slashable }.data(),
        truststake::accounts::GrantPermitAccountConstraints {
            seller: seller.pubkey(),
            stake,
            marketplace,
            permit,
            system_program: anchor_lang::system_program::ID,
            event_authority: chain.event_authority,
            program: chain.program_id,
        }
        .to_account_metas(None),
    );
    send(chain.client, &[instruction], &seller.pubkey(), &[seller])
}

/// Registers a new marketplace tenant, deriving its `bond_vault` PDA
/// alongside it.
#[allow(clippy::too_many_arguments)]
fn send_register_marketplace(
    chain: &Chain,
    authority: &Keypair,
    marketplace_id: [u8; 16],
    receipt_signer: Pubkey,
    arbiter: Pubkey,
    complaint_window: i64,
    bond_bps: u16,
) -> Result<(Pubkey, Signature), Box<dyn Error>> {
    let marketplace = marketplace_pda(chain.program_id, marketplace_id);
    let bond_vault = bond_vault_pda(chain.program_id, marketplace);

    let instruction = Instruction::new_with_bytes(
        chain.program_id,
        &truststake::instruction::RegisterMarketplace { marketplace_id, receipt_signer, arbiter, complaint_window, bond_bps }
            .data(),
        truststake::accounts::RegisterMarketplaceAccountConstraints {
            authority: authority.pubkey(),
            config: chain.config,
            mint: chain.mint,
            marketplace,
            bond_vault,
            token_program: spl_token::ID,
            system_program: anchor_lang::system_program::ID,
            event_authority: chain.event_authority,
            program: chain.program_id,
        }
        .to_account_metas(None),
    );

    let signature = send(chain.client, &[instruction], &authority.pubkey(), &[authority])?;
    Ok((marketplace, signature))
}

/// Recovers CashDesk/PixelBazaar's real onchain identity, chosen randomly
/// by the pre-persistence `random_marketplace_id()` on the 2026-08-24 run
/// (see the module doc comment). Order of preference: the progress file's
/// cache (fast path, valid once discovered or created at least once under
/// this revision), then an onchain scan of the program's own accounts
/// matching by `authority` (the recovery path, needed exactly once), then
/// registering fresh with a new random ID (the only path a genuinely new
/// deployment would take).
#[allow(clippy::too_many_arguments)]
fn find_or_register_legacy_marketplace(
    chain: &Chain,
    progress: &mut Value,
    progress_key: &str,
    authority: &Keypair,
    receipt_signer: Pubkey,
    arbiter: Pubkey,
    complaint_window: i64,
    bond_bps: u16,
    name: &str,
) -> Result<(Pubkey, [u8; 16]), Box<dyn Error>> {
    if let Some(id) = get_bytes16(progress, progress_key) {
        let marketplace = marketplace_pda(chain.program_id, id);
        if chain.client.get_account(&marketplace).is_ok() {
            println!("{name}: using cached marketplace id from {}", progress_path().display());
            return Ok((marketplace, id));
        }
        println!("{name}: cached marketplace id in progress.json does not exist onchain; re-discovering");
    }

    if let Some((id, marketplace)) = find_marketplace_by_authority(chain, authority.pubkey())? {
        println!("{name}: recovered existing marketplace from chain (authority match): {marketplace}");
        set_bytes16(progress, progress_key, id);
        return Ok((marketplace, id));
    }

    let id = random_marketplace_id();
    let (marketplace, signature) =
        send_register_marketplace(chain, authority, id, receipt_signer, arbiter, complaint_window, bond_bps)?;
    print_step(
        &format!("register_marketplace({name}, window = {} days, bond = {} bps)", complaint_window / SECONDS_PER_DAY, bond_bps),
        &signature,
    );
    set_bytes16(progress, progress_key, id);
    Ok((marketplace, id))
}

/// Scans every account the program owns (cheap: a handful of accounts on
/// this deployment) and returns the first `Marketplace` whose `authority`
/// matches. `Marketplace::try_deserialize` returns `Err` for any
/// non-Marketplace account (wrong discriminator) or a stale layout (wrong
/// size for the current struct, which is how two orphaned v1-era accounts
/// sharing today's `Config`/`SellerStake` discriminators -- confirmed by
/// direct inspection of devnet's account list, at different PDAs since v1
/// predates the `SEED_VERSION` seed component -- are silently skipped
/// rather than misread.
#[allow(clippy::type_complexity)]
fn find_marketplace_by_authority(chain: &Chain, authority: Pubkey) -> Result<Option<([u8; 16], Pubkey)>, Box<dyn Error>> {
    for (pubkey, account) in chain.client.get_program_accounts(&chain.program_id)? {
        if let Ok(marketplace) = Marketplace::try_deserialize(&mut account.data.as_slice()) {
            if marketplace.authority == authority {
                return Ok(Some((marketplace.marketplace_id, pubkey)));
            }
        }
    }
    Ok(None)
}

/// Same scan as `find_marketplace_by_authority`, for `DisputeRecord`
/// accounts matching `(marketplace, seller, status)`. Used both as the
/// idempotency gate for the original walk's Step 7 (has the one-time
/// CashDesk dispute already been raised and upheld) and by stage 4 to
/// locate the ORIGINAL 2026-08-24 dispute, whose `order_id` -- unlike the
/// two new disputes stage 1 raises -- was never persisted anywhere.
fn find_dispute_by_marketplace_seller_status(
    chain: &Chain,
    marketplace: Pubkey,
    seller: Pubkey,
    status: DisputeStatus,
) -> Result<Option<(Pubkey, DisputeRecord)>, Box<dyn Error>> {
    for (pubkey, account) in chain.client.get_program_accounts(&chain.program_id)? {
        if let Ok(dispute) = DisputeRecord::try_deserialize(&mut account.data.as_slice()) {
            if dispute.marketplace == marketplace && dispute.seller == seller && dispute.status == status as u8 {
                return Ok(Some((pubkey, dispute)));
            }
        }
    }
    Ok(None)
}

fn read_seller_stake(client: &RpcClient, pubkey: &Pubkey) -> Result<SellerStake, Box<dyn Error>> {
    let account = client.get_account(pubkey)?;
    Ok(SellerStake::try_deserialize(&mut account.data.as_slice())
        .map_err(|error| format!("failed to deserialize SellerStake at {pubkey}: {error}"))?)
}

fn read_permit(client: &RpcClient, pubkey: &Pubkey) -> Result<SlashPermit, Box<dyn Error>> {
    let account = client.get_account(pubkey)?;
    Ok(SlashPermit::try_deserialize(&mut account.data.as_slice())
        .map_err(|error| format!("failed to deserialize SlashPermit at {pubkey}: {error}"))?)
}

fn read_marketplace(client: &RpcClient, pubkey: &Pubkey) -> Result<Marketplace, Box<dyn Error>> {
    let account = client.get_account(pubkey)?;
    Ok(Marketplace::try_deserialize(&mut account.data.as_slice())
        .map_err(|error| format!("failed to deserialize Marketplace at {pubkey}: {error}"))?)
}

fn read_config(client: &RpcClient, pubkey: &Pubkey) -> Result<Config, Box<dyn Error>> {
    let account = client.get_account(pubkey)?;
    Ok(Config::try_deserialize(&mut account.data.as_slice())
        .map_err(|error| format!("failed to deserialize Config at {pubkey}: {error}"))?)
}

fn read_dispute(client: &RpcClient, pubkey: &Pubkey) -> Result<DisputeRecord, Box<dyn Error>> {
    let account = client.get_account(pubkey)?;
    Ok(DisputeRecord::try_deserialize(&mut account.data.as_slice())
        .map_err(|error| format!("failed to deserialize DisputeRecord at {pubkey}: {error}"))?)
}

fn try_read_dispute(client: &RpcClient, pubkey: &Pubkey) -> Option<DisputeRecord> {
    let account = client.get_account(pubkey).ok()?;
    DisputeRecord::try_deserialize(&mut account.data.as_slice()).ok()
}

/// Creates the demo's own Classic Token Program mint, standing in for
/// USDC. See the comment at the `initialize_config` call site in `main`
/// for why this is the one and only place this mint may be chosen.
fn create_test_mint(client: &RpcClient, authority: &Keypair) -> Result<(Pubkey, Signature), Box<dyn Error>> {
    let mint = Keypair::new();
    let space = spl_token::state::Mint::LEN;
    let rent = client.get_minimum_balance_for_rent_exemption(space)?;

    let create_account_instruction =
        system_instruction::create_account(&authority.pubkey(), &mint.pubkey(), rent, space as u64, &spl_token::ID);
    let initialize_mint_instruction = spl_token::instruction::initialize_mint2(
        &spl_token::ID,
        &mint.pubkey(),
        &authority.pubkey(),
        None,
        MINT_DECIMALS,
    )
    .map_err(|error| format!("failed to build initialize_mint2 instruction: {error:?}"))?;

    let signature = send(
        client,
        &[create_account_instruction, initialize_mint_instruction],
        &authority.pubkey(),
        &[authority, &mint],
    )?;
    Ok((mint.pubkey(), signature))
}

/// Creates a Classic Token Program account under `mint`, owned by `owner`,
/// paid for by `payer`. `owner` never needs to sign: SPL Token's
/// `InitializeAccount3` writes the owner into the account's data without
/// requiring the owner's signature, which is what lets `admin` provision
/// throwaway sellers' and buyers' token accounts on their behalf here, the
/// way a devnet faucet would.
fn create_token_account(
    client: &RpcClient,
    payer: &Keypair,
    mint: &Pubkey,
    owner: &Pubkey,
) -> Result<(Pubkey, Signature), Box<dyn Error>> {
    let token_account = Keypair::new();
    let space = spl_token::state::Account::LEN;
    let rent = client.get_minimum_balance_for_rent_exemption(space)?;

    let create_account_instruction =
        system_instruction::create_account(&payer.pubkey(), &token_account.pubkey(), rent, space as u64, &spl_token::ID);
    let initialize_account_instruction = spl_token::instruction::initialize_account3(&spl_token::ID, &token_account.pubkey(), mint, owner)
        .map_err(|error| format!("failed to build initialize_account3 instruction: {error:?}"))?;

    let signature = send(
        client,
        &[create_account_instruction, initialize_account_instruction],
        &payer.pubkey(),
        &[payer, &token_account],
    )?;
    Ok((token_account.pubkey(), signature))
}

fn mint_to_account(
    client: &RpcClient,
    mint_authority: &Keypair,
    mint: &Pubkey,
    destination: &Pubkey,
    amount: u64,
) -> Result<Signature, Box<dyn Error>> {
    let instruction = spl_token::instruction::mint_to(&spl_token::ID, mint, destination, &mint_authority.pubkey(), &[], amount)
        .map_err(|error| format!("failed to build mint_to instruction: {error:?}"))?;
    send(client, &[instruction], &mint_authority.pubkey(), &[mint_authority])
}

fn random_marketplace_id() -> [u8; 16] {
    let mut id = [0u8; 16];
    id.copy_from_slice(&Keypair::new().pubkey().to_bytes()[..16]);
    id
}

/// Sends `instructions` in one transaction, paid for and signed by `payer`
/// plus whichever of `signers` the accounts require, and blocks until it
/// reaches the client's configured commitment level. Simulates first
/// (preflight): a transaction that would fail execution is rejected here
/// and never reaches the cluster, so it never costs a fee. Every step in
/// this file that is expected to succeed goes through this function; the
/// ones expected to fail go through `send_allowing_failure` instead.
fn send(client: &RpcClient, instructions: &[Instruction], payer: &Pubkey, signers: &[&Keypair]) -> Result<Signature, Box<dyn Error>> {
    let blockhash = client.get_latest_blockhash()?;
    let message = Message::new_with_blockhash(instructions, Some(payer), &blockhash);
    let transaction = VersionedTransaction::try_new(VersionedMessage::Legacy(message), signers)?;
    Ok(client.send_and_confirm_transaction(&transaction)?)
}

/// Submits without the usual preflight simulation and waits for the
/// transaction's own onchain outcome, succeed or fail. `send` simulates
/// first and never lets a failing transaction reach the cluster at all --
/// exactly wrong when the whole point is proving a check is enforced by
/// the deployed program on real devnet, at the cost of a real fee, not
/// merely caught by client-side simulation before anything was spent.
fn send_allowing_failure(
    client: &RpcClient,
    instructions: &[Instruction],
    payer: &Pubkey,
    signers: &[&Keypair],
) -> Result<(Signature, Option<TransactionError>), Box<dyn Error>> {
    let blockhash = client.get_latest_blockhash()?;
    let message = Message::new_with_blockhash(instructions, Some(payer), &blockhash);
    let transaction = VersionedTransaction::try_new(VersionedMessage::Legacy(message), signers)?;

    let config = RpcSendTransactionConfig { skip_preflight: true, ..RpcSendTransactionConfig::default() };
    let signature = client.send_transaction_with_config(&transaction, config)?;

    for _ in 0..60 {
        if let Some(status) = client.get_signature_status(&signature)? {
            return Ok((signature, status.err()));
        }
        sleep(Duration::from_millis(500));
    }
    Err(format!("timed out waiting for {signature} to be confirmed").into())
}

fn transfer_sol(client: &RpcClient, from: &Keypair, to: &Pubkey, lamports: u64) -> Result<Signature, Box<dyn Error>> {
    send(client, &[system_instruction::transfer(&from.pubkey(), to, lamports)], &from.pubkey(), &[from])
}

fn print_step(label: &str, signature: &Signature) {
    println!("{label}: {signature}");
    println!("  https://explorer.solana.com/tx/{signature}?cluster=devnet");
}

// ==================== Instruction senders for the 11 handlers new to this file ====================

fn send_propose_config_authority(chain: &Chain, authority: &Keypair, new_authority: Pubkey) -> Result<Signature, Box<dyn Error>> {
    let instruction = Instruction::new_with_bytes(
        chain.program_id,
        &truststake::instruction::ProposeConfigAuthority { new_authority }.data(),
        truststake::accounts::ProposeConfigAuthorityAccountConstraints {
            authority: authority.pubkey(),
            config: chain.config,
            event_authority: chain.event_authority,
            program: chain.program_id,
        }
        .to_account_metas(None),
    );
    send(chain.client, &[instruction], &authority.pubkey(), &[authority])
}

fn send_accept_config_authority(chain: &Chain, pending_authority: &Keypair) -> Result<Signature, Box<dyn Error>> {
    let instruction = Instruction::new_with_bytes(
        chain.program_id,
        &truststake::instruction::AcceptConfigAuthority {}.data(),
        truststake::accounts::AcceptConfigAuthorityAccountConstraints {
            pending_authority: pending_authority.pubkey(),
            config: chain.config,
            event_authority: chain.event_authority,
            program: chain.program_id,
        }
        .to_account_metas(None),
    );
    send(chain.client, &[instruction], &pending_authority.pubkey(), &[pending_authority])
}

#[allow(clippy::too_many_arguments)]
fn send_update_marketplace(
    chain: &Chain,
    authority: &Keypair,
    marketplace: Pubkey,
    new_receipt_signer: Option<Pubkey>,
    new_arbiter: Option<Pubkey>,
    new_complaint_window: Option<i64>,
    new_bond_bps: Option<u16>,
) -> Result<Signature, Box<dyn Error>> {
    let instruction = Instruction::new_with_bytes(
        chain.program_id,
        &truststake::instruction::UpdateMarketplace { new_receipt_signer, new_arbiter, new_complaint_window, new_bond_bps }.data(),
        truststake::accounts::UpdateMarketplaceAccountConstraints {
            authority: authority.pubkey(),
            marketplace,
            event_authority: chain.event_authority,
            program: chain.program_id,
        }
        .to_account_metas(None),
    );
    send(chain.client, &[instruction], &authority.pubkey(), &[authority])
}

fn send_propose_marketplace_authority(
    chain: &Chain,
    authority: &Keypair,
    marketplace: Pubkey,
    new_authority: Pubkey,
) -> Result<Signature, Box<dyn Error>> {
    let instruction = Instruction::new_with_bytes(
        chain.program_id,
        &truststake::instruction::ProposeMarketplaceAuthority { new_authority }.data(),
        truststake::accounts::ProposeMarketplaceAuthorityAccountConstraints {
            authority: authority.pubkey(),
            marketplace,
            event_authority: chain.event_authority,
            program: chain.program_id,
        }
        .to_account_metas(None),
    );
    send(chain.client, &[instruction], &authority.pubkey(), &[authority])
}

fn send_accept_marketplace_authority(chain: &Chain, pending_authority: &Keypair, marketplace: Pubkey) -> Result<Signature, Box<dyn Error>> {
    let instruction = Instruction::new_with_bytes(
        chain.program_id,
        &truststake::instruction::AcceptMarketplaceAuthority {}.data(),
        truststake::accounts::AcceptMarketplaceAuthorityAccountConstraints {
            pending_authority: pending_authority.pubkey(),
            marketplace,
            event_authority: chain.event_authority,
            program: chain.program_id,
        }
        .to_account_metas(None),
    );
    send(chain.client, &[instruction], &pending_authority.pubkey(), &[pending_authority])
}

fn send_increase_permit(chain: &Chain, seller: &Keypair, stake: Pubkey, marketplace: Pubkey, delta: u64) -> Result<Signature, Box<dyn Error>> {
    let permit = permit_pda(chain.program_id, seller.pubkey(), marketplace);
    let instruction = Instruction::new_with_bytes(
        chain.program_id,
        &truststake::instruction::IncreasePermit { delta }.data(),
        truststake::accounts::IncreasePermitAccountConstraints {
            seller: seller.pubkey(),
            stake,
            permit,
            event_authority: chain.event_authority,
            program: chain.program_id,
        }
        .to_account_metas(None),
    );
    send(chain.client, &[instruction], &seller.pubkey(), &[seller])
}

fn send_revoke_permit(chain: &Chain, seller: &Keypair, marketplace: Pubkey) -> Result<Signature, Box<dyn Error>> {
    let permit = permit_pda(chain.program_id, seller.pubkey(), marketplace);
    let instruction = Instruction::new_with_bytes(
        chain.program_id,
        &truststake::instruction::RevokePermit {}.data(),
        truststake::accounts::RevokePermitAccountConstraints {
            seller: seller.pubkey(),
            permit,
            event_authority: chain.event_authority,
            program: chain.program_id,
        }
        .to_account_metas(None),
    );
    send(chain.client, &[instruction], &seller.pubkey(), &[seller])
}

/// `caller` pays and need not be `seller`: release_permit is permissionless
/// by design. `seller` is the pubkey (not necessarily a signer here).
fn send_release_permit(chain: &Chain, caller: &Keypair, seller: Pubkey, marketplace: Pubkey) -> Result<Signature, Box<dyn Error>> {
    let permit = permit_pda(chain.program_id, seller, marketplace);
    let stake = stake_pda(chain.program_id, seller);
    let instruction = Instruction::new_with_bytes(
        chain.program_id,
        &truststake::instruction::ReleasePermit {}.data(),
        truststake::accounts::ReleasePermitAccountConstraints {
            caller: caller.pubkey(),
            seller,
            permit,
            stake,
            event_authority: chain.event_authority,
            program: chain.program_id,
        }
        .to_account_metas(None),
    );
    send(chain.client, &[instruction], &caller.pubkey(), &[caller])
}

/// Both `seller` and the marketplace's current `authority` must sign;
/// `seller` pays.
fn send_release_permit_early(chain: &Chain, seller: &Keypair, authority: &Keypair, marketplace: Pubkey) -> Result<Signature, Box<dyn Error>> {
    let permit = permit_pda(chain.program_id, seller.pubkey(), marketplace);
    let stake = stake_pda(chain.program_id, seller.pubkey());
    let instruction = Instruction::new_with_bytes(
        chain.program_id,
        &truststake::instruction::ReleasePermitEarly {}.data(),
        truststake::accounts::ReleasePermitEarlyAccountConstraints {
            seller: seller.pubkey(),
            authority: authority.pubkey(),
            marketplace,
            permit,
            stake,
            event_authority: chain.event_authority,
            program: chain.program_id,
        }
        .to_account_metas(None),
    );
    send(chain.client, &[instruction], &seller.pubkey(), &[seller, authority])
}

/// Permissionless; `caller` pays. `dispute` and `permit` are passed in
/// (rather than re-derived) since the caller already had to read the
/// dispute record to find its `expires_at`.
fn send_expire_dispute(
    chain: &Chain,
    caller: &Keypair,
    marketplace: Pubkey,
    dispute: Pubkey,
    permit: Pubkey,
    buyer_token_account: Pubkey,
) -> Result<Signature, Box<dyn Error>> {
    let bond_vault = bond_vault_pda(chain.program_id, marketplace);
    let instruction = Instruction::new_with_bytes(
        chain.program_id,
        &truststake::instruction::ExpireDispute {}.data(),
        truststake::accounts::ExpireDisputeAccountConstraints {
            caller: caller.pubkey(),
            marketplace,
            dispute,
            permit,
            bond_vault,
            mint: chain.mint,
            buyer_token_account,
            token_program: spl_token::ID,
            event_authority: chain.event_authority,
            program: chain.program_id,
        }
        .to_account_metas(None),
    );
    send(chain.client, &[instruction], &caller.pubkey(), &[caller])
}

/// Permissionless; `caller` pays. `buyer` is the rent destination, read
/// off the dispute record by the caller rather than assumed.
fn send_close_dispute(chain: &Chain, caller: &Keypair, buyer: Pubkey, dispute: Pubkey) -> Result<Signature, Box<dyn Error>> {
    let instruction = Instruction::new_with_bytes(
        chain.program_id,
        &truststake::instruction::CloseDispute {}.data(),
        truststake::accounts::CloseDisputeAccountConstraints {
            caller: caller.pubkey(),
            buyer,
            dispute,
            event_authority: chain.event_authority,
            program: chain.program_id,
        }
        .to_account_metas(None),
    );
    send(chain.client, &[instruction], &caller.pubkey(), &[caller])
}

// ==================== Stage 1 ====================

/// Counts the transactions and new rent-exempt accounts a from-scratch
/// stage 1 run creates and prints an estimated total cost, all read from
/// live rent rates rather than hardcoded lamport figures. A resumed run
/// that already completed part of stage 1 spends less than this estimate;
/// it is informational, not a gate (the task's own framing).
fn print_stage1_cost_estimate(chain: &Chain) -> Result<(), Box<dyn Error>> {
    let wallet_rent_floor = chain.client.get_minimum_balance_for_rent_exemption(0)?;
    let token_account_rent = chain.client.get_minimum_balance_for_rent_exemption(spl_token::state::Account::LEN)?;
    let marketplace_rent =
        chain.client.get_minimum_balance_for_rent_exemption(Marketplace::DISCRIMINATOR.len() + Marketplace::INIT_SPACE)?;
    let stake_rent = chain.client.get_minimum_balance_for_rent_exemption(SellerStake::DISCRIMINATOR.len() + SellerStake::INIT_SPACE)?;
    let permit_rent = chain.client.get_minimum_balance_for_rent_exemption(SlashPermit::DISCRIMINATOR.len() + SlashPermit::INIT_SPACE)?;
    let dispute_rent =
        chain.client.get_minimum_balance_for_rent_exemption(DisputeRecord::DISCRIMINATOR.len() + DisputeRecord::INIT_SPACE)?;

    // New rent-exempt accounts stage 1 creates, worst case (a from-scratch
    // run; a resumed run creates fewer): SwiftMarket marketplace + its bond
    // vault (register_marketplace), seller B's stake + its stake vault
    // (initialize_stake), three permits (SwiftMarket, seller B x CashDesk,
    // seller B x PixelBazaar), two dispute records (the SwiftMarket
    // dispute, the second CashDesk dispute), and two new token accounts
    // (buyer_swiftmarket's, seller B's).
    let new_accounts_rent = (marketplace_rent + token_account_rent)
        + (stake_rent + token_account_rent)
        + 3 * permit_rent
        + 2 * dispute_rent
        + 2 * token_account_rent;

    // Five new signing wallets that pay fees of their own: governance,
    // swiftmarket_authority, swiftmarket_authority_v2, buyer_swiftmarket,
    // seller_b. swiftmarket_receipt_signer/arbiter and
    // cashdesk_receipt_signer_v2 never sign a transaction, matching every
    // other receipt-signer/arbiter role in this file.
    let wallet_headroom = 5 * wallet_rent_floor;

    // Transaction count, worst case: 1.1 (4) + 1.2 (1) + 1.3 (1) + 1.4 (1)
    // + 1.5 (2) + 1.6 (1) + 1.7 (1) + 1.8 (1) + 1.9 (1) + 1.10 (2) + 1.11
    // (6, including two revokes) = 21.
    let transaction_count: u64 = 21;
    let fee_estimate = transaction_count * SIGNATURE_FEE_HEADROOM;

    let total = new_accounts_rent + wallet_headroom + fee_estimate;

    println!("=== Stage 1 cost estimate (informational; not a gate) ===");
    println!("  Up to {transaction_count} transactions, ~{fee_estimate} lamports fee headroom");
    println!("  New rent-exempt accounts: ~{new_accounts_rent} lamports");
    println!("  New wallet funding headroom: ~{wallet_headroom} lamports");
    println!("  Estimated total: ~{total} lamports (~{:.6} SOL)", total as f64 / 1_000_000_000.0);
    println!("  A resumed run that already completed part of stage 1 spends less than this.");
    println!();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn fund_wallets_stage1(
    client: &RpcClient,
    admin: &Keypair,
    governance: &Keypair,
    swiftmarket_authority: &Keypair,
    swiftmarket_authority_v2: &Keypair,
    buyer_swiftmarket: &Keypair,
    seller_b: &Keypair,
) -> Result<(), Box<dyn Error>> {
    let wallet_rent_floor = client.get_minimum_balance_for_rent_exemption(0)?;
    let token_account_rent = client.get_minimum_balance_for_rent_exemption(spl_token::state::Account::LEN)?;
    let marketplace_rent = client.get_minimum_balance_for_rent_exemption(Marketplace::DISCRIMINATOR.len() + Marketplace::INIT_SPACE)?;
    let stake_rent = client.get_minimum_balance_for_rent_exemption(SellerStake::DISCRIMINATOR.len() + SellerStake::INIT_SPACE)?;
    let permit_rent = client.get_minimum_balance_for_rent_exemption(SlashPermit::DISCRIMINATOR.len() + SlashPermit::INIT_SPACE)?;
    let dispute_rent = client.get_minimum_balance_for_rent_exemption(DisputeRecord::DISCRIMINATOR.len() + DisputeRecord::INIT_SPACE)?;

    // governance: accept_config_authority, then propose_config_authority
    // back to admin later in the same round trip. 2 transactions, no rent.
    top_up_balance(client, admin, &governance.pubkey(), 2 * SIGNATURE_FEE_HEADROOM + wallet_rent_floor, "governance")?;

    // swiftmarket_authority: register_marketplace (pays marketplace_rent +
    // bond_vault's token_account_rent) + propose_marketplace_authority.
    let swiftmarket_authority_funding = marketplace_rent + token_account_rent + 2 * SIGNATURE_FEE_HEADROOM + wallet_rent_floor;
    top_up_balance(client, admin, &swiftmarket_authority.pubkey(), swiftmarket_authority_funding, "swiftmarket_authority")?;

    // swiftmarket_authority_v2: accept_marketplace_authority only.
    top_up_balance(client, admin, &swiftmarket_authority_v2.pubkey(), SIGNATURE_FEE_HEADROOM + wallet_rent_floor, "swiftmarket_authority_v2")?;

    // buyer_swiftmarket: raise_dispute (pays dispute_rent).
    let buyer_swiftmarket_funding = dispute_rent + SIGNATURE_FEE_HEADROOM + wallet_rent_floor;
    top_up_balance(client, admin, &buyer_swiftmarket.pubkey(), buyer_swiftmarket_funding, "buyer_swiftmarket")?;

    // seller_b: initialize_stake (pays stake_rent + vault's
    // token_account_rent), add_stake, two grant_permit calls (each pays
    // permit_rent), two revoke_permit calls. 6 transactions.
    let seller_b_funding = stake_rent + token_account_rent + 2 * permit_rent + 6 * SIGNATURE_FEE_HEADROOM + wallet_rent_floor;
    top_up_balance(client, admin, &seller_b.pubkey(), seller_b_funding, "seller_b")?;

    println!();
    Ok(())
}

fn run_stage_1(chain: &Chain, progress: &mut Value, roster: &StageRoster) -> Result<(), Box<dyn Error>> {
    println!("=== Stage 1: immediate steps (1.1 - 1.11) ===");
    print_stage1_cost_estimate(chain)?;

    let dir = keypairs_dir();
    let governance = load_or_create_keypair(&dir, "governance")?;
    let swiftmarket_authority = load_or_create_keypair(&dir, "swiftmarket_authority")?;
    let swiftmarket_authority_v2 = load_or_create_keypair(&dir, "swiftmarket_authority_v2")?;
    let swiftmarket_receipt_signer = load_or_create_keypair(&dir, "swiftmarket_receipt_signer")?;
    let swiftmarket_arbiter = load_or_create_keypair(&dir, "swiftmarket_arbiter")?;
    let cashdesk_receipt_signer_v2 = load_or_create_keypair(&dir, "cashdesk_receipt_signer_v2")?;
    let buyer_swiftmarket = load_or_create_keypair(&dir, "buyer_swiftmarket")?;
    let seller_b = load_or_create_keypair(&dir, "seller_b")?;

    fund_wallets_stage1(
        chain.client,
        roster.admin,
        &governance,
        &swiftmarket_authority,
        &swiftmarket_authority_v2,
        &buyer_swiftmarket,
        &seller_b,
    )?;

    stage1_1_config_authority_roundtrip(chain, progress, roster.admin, &governance)?;
    save_progress(progress)?;
    println!();

    let swiftmarket = stage1_2_register_swiftmarket(
        chain,
        &swiftmarket_authority,
        swiftmarket_receipt_signer.pubkey(),
        swiftmarket_arbiter.pubkey(),
    )?;
    println!();

    stage1_3_update_pixelbazaar_bond(chain, roster)?;
    println!();

    stage1_4_rotate_cashdesk_receipt_signer(chain, roster, &cashdesk_receipt_signer_v2)?;
    println!();

    stage1_5_swiftmarket_authority_transfer(chain, swiftmarket, &swiftmarket_authority, &swiftmarket_authority_v2)?;
    println!();

    stage1_6_top_up_seller_free_collateral(chain, roster, swiftmarket)?;
    println!();

    stage1_7_and_1_8_swiftmarket_permit(chain, progress, roster, swiftmarket)?;
    println!();

    let buyer_swiftmarket_token_account =
        load_or_find_or_create_token_account(chain.client, &dir, "buyer_swiftmarket", roster.admin, &chain.mint, &buyer_swiftmarket.pubkey())?;
    top_up_token_balance(chain.client, roster.admin, &chain.mint, &buyer_swiftmarket_token_account, SWIFTMARKET_BUYER_TOKEN_FUNDING)?;
    stage1_9_swiftmarket_dispute(
        chain,
        progress,
        roster,
        swiftmarket,
        &buyer_swiftmarket,
        buyer_swiftmarket_token_account,
        &swiftmarket_receipt_signer,
    )?;
    save_progress(progress)?;
    println!();

    stage1_10_cashdesk_second_dispute(chain, progress, roster, roster.cashdesk_receipt_signer)?;
    save_progress(progress)?;
    println!();

    stage1_11_seller_b(chain, progress, roster, &seller_b)?;
    save_progress(progress)?;

    println!();
    println!("Stage 1: complete.");
    Ok(())
}

/// Config.authority round-trips admin -> governance -> admin across four
/// transactions, exercising propose_config_authority/accept_config_authority
/// twice each. The start state and the fully-converged end state are BOTH
/// `(authority = admin, pending_authority = default)` -- indistinguishable
/// from Config's fields alone -- so `config_authority_roundtrip_done` in
/// progress.json is what tells a fresh start apart from an already-finished
/// one; every OTHER state along the way is unambiguous and drives the loop
/// below directly from chain state, with `sent_any` distinguishing "just
/// reached the converged state after sending a transaction this call" from
/// "still at the start, having sent nothing yet."
fn stage1_1_config_authority_roundtrip(chain: &Chain, progress: &mut Value, admin: &Keypair, governance: &Keypair) -> Result<(), Box<dyn Error>> {
    println!("--- 1.1: propose_config_authority / accept_config_authority round trip (admin -> governance -> admin) ---");
    println!("  Note: Config.authority currently confers no powers in this program (see docs/DESIGN-v2.md);");
    println!("  this step proves the transfer mechanism works, not that the role does anything.");

    if get_bool(progress, "config_authority_roundtrip_done") {
        println!("  Already done in a previous run, skipping.");
        return Ok(());
    }

    let mut sent_any = false;
    for _ in 0..4 {
        let state = read_config(chain.client, &chain.config)?;
        if sent_any && state.authority == admin.pubkey() && state.pending_authority == Pubkey::default() {
            break;
        }
        if state.authority == governance.pubkey() && state.pending_authority == admin.pubkey() {
            let signature = send_accept_config_authority(chain, admin)?;
            print_step("accept_config_authority(admin)", &signature);
        } else if state.authority == governance.pubkey() && state.pending_authority == Pubkey::default() {
            let signature = send_propose_config_authority(chain, governance, admin.pubkey())?;
            print_step("propose_config_authority(governance -> admin)", &signature);
        } else if state.authority == admin.pubkey() && state.pending_authority == governance.pubkey() {
            let signature = send_accept_config_authority(chain, governance)?;
            print_step("accept_config_authority(governance)", &signature);
        } else {
            // authority == admin, pending == default: only reachable here
            // (before `sent_any`) as the genuine fresh start, since
            // progress.json already confirmed the round trip never
            // finished before this call.
            let signature = send_propose_config_authority(chain, admin, governance.pubkey())?;
            print_step("propose_config_authority(admin -> governance)", &signature);
        }
        sent_any = true;
    }

    let state = read_config(chain.client, &chain.config)?;
    if state.authority != admin.pubkey() || state.pending_authority != Pubkey::default() {
        return Err(format!(
            "1.1 did not converge after 4 transactions: Config.authority={}, pending_authority={}",
            state.authority, state.pending_authority
        )
        .into());
    }
    set_bool(progress, "config_authority_roundtrip_done", true);
    println!("  1.1 complete: Config.authority is admin again, pending_authority is default.");
    Ok(())
}

fn stage1_2_register_swiftmarket(
    chain: &Chain,
    swiftmarket_authority: &Keypair,
    receipt_signer: Pubkey,
    arbiter: Pubkey,
) -> Result<Pubkey, Box<dyn Error>> {
    println!(
        "--- 1.2: register_marketplace(SwiftMarket, window = {} days, bond = {} bps) ---",
        SWIFTMARKET_COMPLAINT_WINDOW_SECONDS / SECONDS_PER_DAY,
        SWIFTMARKET_BOND_BPS
    );
    let swiftmarket = marketplace_pda(chain.program_id, SWIFTMARKET_ID);
    if chain.client.get_account(&swiftmarket).is_ok() {
        println!("  Already registered, skipping.");
        return Ok(swiftmarket);
    }
    let (swiftmarket, signature) = send_register_marketplace(
        chain,
        swiftmarket_authority,
        SWIFTMARKET_ID,
        receipt_signer,
        arbiter,
        SWIFTMARKET_COMPLAINT_WINDOW_SECONDS,
        SWIFTMARKET_BOND_BPS,
    )?;
    print_step("register_marketplace(SwiftMarket)", &signature);
    println!("  Zero bond is legal on purpose (CLAUDE.md's deliberate-tradeoffs list); this is the");
    println!("  first time devnet shows both ends of the legal bond range at once (CashDesk is 1,000 bps).");
    Ok(swiftmarket)
}

fn stage1_3_update_pixelbazaar_bond(chain: &Chain, roster: &StageRoster) -> Result<(), Box<dyn Error>> {
    println!("--- 1.3: update_marketplace(PixelBazaar, new_bond_bps = Some({PIXELBAZAAR_NEW_BOND_BPS})) ---");
    let current = read_marketplace(chain.client, &roster.pixelbazaar)?;
    if current.bond_bps == PIXELBAZAAR_NEW_BOND_BPS {
        println!("  Already applied, skipping.");
    } else {
        let signature =
            send_update_marketplace(chain, roster.pixelbazaar_authority, roster.pixelbazaar, None, None, None, Some(PIXELBAZAAR_NEW_BOND_BPS))?;
        print_step("update_marketplace(PixelBazaar, new_bond_bps)", &signature);
    }

    // Prove a marketplace can change its terms without touching an
    // already-granted permit: the original seller's PixelBazaar permit
    // must still carry the rate it was granted under.
    let permit = permit_pda(chain.program_id, roster.seller.pubkey(), roster.pixelbazaar);
    let permit_state = read_permit(chain.client, &permit)?;
    if permit_state.bond_bps != PIXELBAZAAR_BOND_BPS {
        return Err(format!(
            "expected the original seller's PixelBazaar permit to keep its frozen bond_bps {PIXELBAZAAR_BOND_BPS}, found {}",
            permit_state.bond_bps
        )
        .into());
    }
    println!("  Confirmed: the existing PixelBazaar permit still carries bond_bps {PIXELBAZAAR_BOND_BPS}, frozen at grant.");
    Ok(())
}

fn stage1_4_rotate_cashdesk_receipt_signer(chain: &Chain, roster: &StageRoster, new_signer: &Keypair) -> Result<(), Box<dyn Error>> {
    println!("--- 1.4: update_marketplace(CashDesk, new_receipt_signer = Some(cashdesk_receipt_signer_v2)) ---");
    let current = read_marketplace(chain.client, &roster.cashdesk)?;
    if current.receipt_signer == new_signer.pubkey() {
        println!("  Already rotated, skipping.");
    } else {
        let signature = send_update_marketplace(chain, roster.cashdesk_authority, roster.cashdesk, Some(new_signer.pubkey()), None, None, None)?;
        print_step("update_marketplace(CashDesk, new_receipt_signer)", &signature);
    }

    let updated = read_marketplace(chain.client, &roster.cashdesk)?;
    if updated.prev_receipt_signer != roster.cashdesk_receipt_signer.pubkey() {
        return Err(format!(
            "expected CashDesk's prev_receipt_signer to be the original signer {}, found {}",
            roster.cashdesk_receipt_signer.pubkey(),
            updated.prev_receipt_signer
        )
        .into());
    }
    if updated.signer_rotated_at == 0 {
        return Err("expected CashDesk's signer_rotated_at to be non-zero after rotation".into());
    }
    println!("  Confirmed: prev_receipt_signer holds the original key, signer_rotated_at = {}.", updated.signer_rotated_at);
    Ok(())
}

fn stage1_5_swiftmarket_authority_transfer(chain: &Chain, swiftmarket: Pubkey, authority: &Keypair, authority_v2: &Keypair) -> Result<(), Box<dyn Error>> {
    println!("--- 1.5: propose_marketplace_authority / accept_marketplace_authority (SwiftMarket -> v2) ---");
    let state = read_marketplace(chain.client, &swiftmarket)?;
    if state.authority == authority_v2.pubkey() {
        println!("  Already transferred, skipping.");
        return Ok(());
    }
    if state.pending_authority != authority_v2.pubkey() {
        let signature = send_propose_marketplace_authority(chain, authority, swiftmarket, authority_v2.pubkey())?;
        print_step("propose_marketplace_authority(SwiftMarket -> v2)", &signature);
    }
    let signature = send_accept_marketplace_authority(chain, authority_v2, swiftmarket)?;
    print_step("accept_marketplace_authority(SwiftMarket, v2)", &signature);
    println!("  From here on, SwiftMarket's authority is v2 -- every later step needing it must use v2.");
    Ok(())
}

fn stage1_6_top_up_seller_free_collateral(chain: &Chain, roster: &StageRoster, swiftmarket: Pubkey) -> Result<(), Box<dyn Error>> {
    println!("--- 1.6: add_stake (top up the original seller's free collateral for SwiftMarket) ---");
    let swiftmarket_permit = permit_pda(chain.program_id, roster.seller.pubkey(), swiftmarket);
    let swiftmarket_permit_exists = chain.client.get_account(&swiftmarket_permit).is_ok();
    let stake_state = read_seller_stake(chain.client, &roster.stake)?;
    let free = stake_state.staked.saturating_sub(stake_state.committed);
    let shortfall = match decide_free_collateral_topup(swiftmarket_permit_exists, free, SELLER_FREE_COLLATERAL_TARGET) {
        TopUpDecision::SkipPermitAlreadyGranted => {
            println!("  SwiftMarket's permit already exists -- this step's pre-grant top-up already happened, skipping.");
            return Ok(());
        }
        TopUpDecision::SkipAlreadyAtTarget => {
            println!("  Free collateral is already {} >= {}, skipping.", format_usdc(free), format_usdc(SELLER_FREE_COLLATERAL_TARGET));
            return Ok(());
        }
        TopUpDecision::TopUp { shortfall } => shortfall,
    };
    top_up_token_balance(chain.client, roster.admin, &chain.mint, &roster.seller_token_account, shortfall)?;
    let instruction = Instruction::new_with_bytes(
        chain.program_id,
        &truststake::instruction::AddStake { amount: shortfall }.data(),
        truststake::accounts::AddStakeAccountConstraints {
            seller: roster.seller.pubkey(),
            stake: roster.stake,
            stake_vault: stake_vault_pda(chain.program_id, roster.seller.pubkey()),
            mint: chain.mint,
            seller_token_account: roster.seller_token_account,
            token_program: spl_token::ID,
            event_authority: chain.event_authority,
            program: chain.program_id,
        }
        .to_account_metas(None),
    );
    let signature = send(chain.client, &[instruction], &roster.seller.pubkey(), &[roster.seller])?;
    print_step(&format!("add_stake({}) -- topping up free collateral for SwiftMarket", format_usdc(shortfall)), &signature);
    Ok(())
}

fn stage1_7_and_1_8_swiftmarket_permit(chain: &Chain, progress: &Value, roster: &StageRoster, swiftmarket: Pubkey) -> Result<(), Box<dyn Error>> {
    println!("--- 1.7: grant_permit(SwiftMarket, {}) ---", format_usdc(SWIFTMARKET_GRANT));
    let permit = permit_pda(chain.program_id, roster.seller.pubkey(), swiftmarket);
    let lifecycle_done = get_bool(progress, "seller_swiftmarket_permit_lifecycle_done");
    let permit_exists = chain.client.get_account(&permit).is_ok();
    let granted_at_before = match decide_permit_grant(lifecycle_done, permit_exists) {
        PermitGrantDecision::SkipLifecycleDone => {
            println!("  Permit lifecycle already complete (granted, revoked, released), skipping -- nothing to increase either.");
            return Ok(());
        }
        PermitGrantDecision::SkipAlreadyGranted => {
            println!("  Already granted, skipping.");
            read_permit(chain.client, &permit)?.granted_at
        }
        PermitGrantDecision::Grant => {
            let signature = send_grant_permit(chain, roster.seller, roster.stake, swiftmarket, SWIFTMARKET_GRANT)?;
            print_step(&format!("grant_permit(SwiftMarket, {})", format_usdc(SWIFTMARKET_GRANT)), &signature);
            read_permit(chain.client, &permit)?.granted_at
        }
    };

    println!("--- 1.8: increase_permit(SwiftMarket, +{}) ---", format_usdc(SWIFTMARKET_INCREASE));
    let current = read_permit(chain.client, &permit)?;
    if current.max_slashable < SWIFTMARKET_PERMIT_TOTAL {
        let delta = SWIFTMARKET_PERMIT_TOTAL - current.max_slashable;
        let signature = send_increase_permit(chain, roster.seller, roster.stake, swiftmarket, delta)?;
        print_step(&format!("increase_permit(SwiftMarket, +{})", format_usdc(delta)), &signature);
    } else {
        println!("  Already increased, skipping.");
    }

    let final_state = read_permit(chain.client, &permit)?;
    if final_state.max_slashable != SWIFTMARKET_PERMIT_TOTAL {
        return Err(format!(
            "expected SwiftMarket permit max_slashable {}, found {}",
            format_usdc(SWIFTMARKET_PERMIT_TOTAL),
            format_usdc(final_state.max_slashable)
        )
        .into());
    }
    if final_state.granted_at != granted_at_before {
        return Err(format!("expected granted_at to stay {granted_at_before} across increase_permit, found {}", final_state.granted_at).into());
    }
    println!(
        "  Confirmed: max_slashable = {}, granted_at unchanged at {} -- increase_permit modifies the era, it does not start a new one.",
        format_usdc(final_state.max_slashable),
        final_state.granted_at
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn stage1_9_swiftmarket_dispute(
    chain: &Chain,
    progress: &mut Value,
    roster: &StageRoster,
    swiftmarket: Pubkey,
    buyer_swiftmarket: &Keypair,
    buyer_swiftmarket_token_account: Pubkey,
    receipt_signer: &Keypair,
) -> Result<(), Box<dyn Error>> {
    println!(
        "--- 1.9: raise_dispute(SwiftMarket, claim = {}) -- deliberately left unresolved ---",
        format_usdc(SWIFTMARKET_CLAIM)
    );
    if let Some(order_id) = get_bytes32(progress, "swiftmarket_dispute_order_id") {
        let dispute = dispute_pda(chain.program_id, swiftmarket, roster.seller.pubkey(), order_id);
        if chain.client.get_account(&dispute).is_ok() {
            println!("  Already raised (order_id recorded in progress.json), skipping.");
            return Ok(());
        }
    }

    let order_id = Keypair::new().pubkey().to_bytes();
    let issued_at = now_unix();
    let receipt = OrderReceipt {
        domain: RECEIPT_DOMAIN,
        program_id: chain.program_id,
        chain_id: DEVNET_CHAIN_ID,
        marketplace_id: SWIFTMARKET_ID,
        seller: roster.seller.pubkey(),
        buyer: buyer_swiftmarket.pubkey(),
        order_id,
        amount: SWIFTMARKET_ORDER_AMOUNT,
        issued_at,
        expires_at: issued_at + 7 * SECONDS_PER_DAY,
    };
    let message = receipt.message();
    let signature_bytes: [u8; 64] = receipt_signer.sign_message(&message).into();
    let verify_instruction = new_ed25519_instruction_with_signature(&message, &signature_bytes, &receipt_signer.pubkey().to_bytes());

    let permit = permit_pda(chain.program_id, roster.seller.pubkey(), swiftmarket);
    let dispute = dispute_pda(chain.program_id, swiftmarket, roster.seller.pubkey(), order_id);
    let bond_vault = bond_vault_pda(chain.program_id, swiftmarket);
    let raise_instruction = Instruction::new_with_bytes(
        chain.program_id,
        &truststake::instruction::RaiseDispute { order_id, claim: SWIFTMARKET_CLAIM }.data(),
        truststake::accounts::RaiseDisputeAccountConstraints {
            buyer: buyer_swiftmarket.pubkey(),
            config: chain.config,
            marketplace: swiftmarket,
            stake: roster.stake,
            permit,
            dispute,
            bond_vault,
            mint: chain.mint,
            buyer_token_account: buyer_swiftmarket_token_account,
            instructions_sysvar: solana_instructions_sysvar::ID,
            token_program: spl_token::ID,
            system_program: anchor_lang::system_program::ID,
            event_authority: chain.event_authority,
            program: chain.program_id,
        }
        .to_account_metas(None),
    );
    let signature = send(chain.client, &[verify_instruction, raise_instruction], &buyer_swiftmarket.pubkey(), &[buyer_swiftmarket])?;
    print_step("[Ed25519 verify, raise_dispute] (SwiftMarket)", &signature);
    set_bytes32(progress, "swiftmarket_dispute_order_id", order_id);
    println!("  This dispute is stage 4's expire_dispute target (30 days from its creation).");
    Ok(())
}

/// Raises AND resolves(upheld = false) CashDesk's second dispute, signed by
/// the ORIGINAL cashdesk_receipt_signer (not v2) and backdated to before
/// 1.4's rotation, so it exercises raise_dispute's `signed_by_previous`
/// branch: an honest key rotation must not void an outstanding receipt.
fn stage1_10_cashdesk_second_dispute(
    chain: &Chain,
    progress: &mut Value,
    roster: &StageRoster,
    original_receipt_signer: &Keypair,
) -> Result<(), Box<dyn Error>> {
    println!("--- 1.10: raise_dispute + resolve_dispute(upheld = false) on CashDesk (second dispute) ---");

    let order_id = if let Some(existing) = get_bytes32(progress, "cashdesk_second_dispute_order_id") {
        existing
    } else {
        let marketplace_state = read_marketplace(chain.client, &roster.cashdesk)?;
        if marketplace_state.signer_rotated_at == 0 {
            return Err("1.10 requires CashDesk's receipt signer to have already been rotated (1.4 must run first)".into());
        }
        let order_id = Keypair::new().pubkey().to_bytes();
        let issued_at = marketplace_state.signer_rotated_at - BACKDATE_BEFORE_ROTATION_SECONDS;
        let receipt = OrderReceipt {
            domain: RECEIPT_DOMAIN,
            program_id: chain.program_id,
            chain_id: DEVNET_CHAIN_ID,
            marketplace_id: roster.cashdesk_id,
            seller: roster.seller.pubkey(),
            buyer: roster.buyer.pubkey(),
            order_id,
            amount: CASHDESK_SECOND_ORDER_AMOUNT,
            issued_at,
            expires_at: issued_at + 7 * SECONDS_PER_DAY,
        };
        let message = receipt.message();
        let signature_bytes: [u8; 64] = original_receipt_signer.sign_message(&message).into();
        let verify_instruction =
            new_ed25519_instruction_with_signature(&message, &signature_bytes, &original_receipt_signer.pubkey().to_bytes());

        let permit = permit_pda(chain.program_id, roster.seller.pubkey(), roster.cashdesk);
        let dispute = dispute_pda(chain.program_id, roster.cashdesk, roster.seller.pubkey(), order_id);
        let bond_vault = bond_vault_pda(chain.program_id, roster.cashdesk);
        let raise_instruction = Instruction::new_with_bytes(
            chain.program_id,
            &truststake::instruction::RaiseDispute { order_id, claim: CASHDESK_SECOND_CLAIM }.data(),
            truststake::accounts::RaiseDisputeAccountConstraints {
                buyer: roster.buyer.pubkey(),
                config: chain.config,
                marketplace: roster.cashdesk,
                stake: roster.stake,
                permit,
                dispute,
                bond_vault,
                mint: chain.mint,
                buyer_token_account: roster.buyer_token_account,
                instructions_sysvar: solana_instructions_sysvar::ID,
                token_program: spl_token::ID,
                system_program: anchor_lang::system_program::ID,
                event_authority: chain.event_authority,
                program: chain.program_id,
            }
            .to_account_metas(None),
        );
        let signature = send(chain.client, &[verify_instruction, raise_instruction], &roster.buyer.pubkey(), &[roster.buyer])?;
        print_step("[Ed25519 verify, raise_dispute] (CashDesk, second dispute, signed pre-rotation)", &signature);
        println!(
            "  receipt.issued_at ({issued_at}) < CashDesk.signer_rotated_at ({}), signed by the pre-rotation key.",
            marketplace_state.signer_rotated_at
        );
        set_bytes32(progress, "cashdesk_second_dispute_order_id", order_id);
        order_id
    };

    let dispute = dispute_pda(chain.program_id, roster.cashdesk, roster.seller.pubkey(), order_id);
    let resolved_flag = get_bool(progress, "cashdesk_second_dispute_resolved");
    let dispute_state_opt = if resolved_flag { None } else { try_read_dispute(chain.client, &dispute) };
    let decision = decide_second_dispute_resolve(resolved_flag, dispute_state_opt.as_ref().map(|state| state.status));
    let dispute_state = match decision {
        SecondDisputeResolveDecision::SkipFlagSet => {
            println!("  Already resolved (recorded in a previous run), skipping.");
            return Ok(());
        }
        SecondDisputeResolveDecision::SkipDisputeGone => {
            // D2-sibling: stage 4.2a eventually closes exactly this
            // DisputeRecord. close_dispute only ever succeeds on a
            // non-Open dispute, so its absence here always implies it was
            // already resolved -- never that raise_dispute needs redoing.
            println!("  Dispute record no longer exists (closed in a previous run); treating as already resolved.");
            set_bool(progress, "cashdesk_second_dispute_resolved", true);
            return Ok(());
        }
        SecondDisputeResolveDecision::SkipAlreadyResolved => {
            println!("  Already resolved, skipping.");
            set_bool(progress, "cashdesk_second_dispute_resolved", true);
            return Ok(());
        }
        SecondDisputeResolveDecision::Proceed => dispute_state_opt.expect("Proceed implies the dispute account was read successfully"),
    };

    let stake_before = read_seller_stake(chain.client, &roster.stake)?;
    let permit = permit_pda(chain.program_id, roster.seller.pubkey(), roster.cashdesk);
    let bond_vault = bond_vault_pda(chain.program_id, roster.cashdesk);
    let resolve_instruction = Instruction::new_with_bytes(
        chain.program_id,
        &truststake::instruction::ResolveDispute { upheld: false }.data(),
        truststake::accounts::ResolveDisputeAccountConstraints {
            arbiter: roster.cashdesk_arbiter.pubkey(),
            marketplace: roster.cashdesk,
            dispute,
            permit,
            stake: roster.stake,
            stake_vault: stake_vault_pda(chain.program_id, roster.seller.pubkey()),
            bond_vault,
            mint: chain.mint,
            buyer_token_account: roster.buyer_token_account,
            token_program: spl_token::ID,
            event_authority: chain.event_authority,
            program: chain.program_id,
        }
        .to_account_metas(None),
    );
    let signature = send(chain.client, &[resolve_instruction], &roster.admin.pubkey(), &[roster.admin, roster.cashdesk_arbiter])?;
    print_step("resolve_dispute(upheld = false) (CashDesk, second dispute)", &signature);

    let stake_after = read_seller_stake(chain.client, &roster.stake)?;
    let bond = dispute_state.bond;
    if stake_after.staked != stake_before.staked + bond {
        return Err(format!(
            "expected stake.staked to rise by exactly the bond {} ({} -> {}), found {}",
            format_usdc(bond),
            format_usdc(stake_before.staked),
            format_usdc(stake_before.staked + bond),
            format_usdc(stake_after.staked)
        )
        .into());
    }
    println!(
        "  Confirmed: rejection moved the bond ({}) into the seller's vault; staked {} -> {}.",
        format_usdc(bond),
        format_usdc(stake_before.staked),
        format_usdc(stake_after.staked)
    );
    println!("  This dispute is stage 4's close_dispute target.");
    set_bool(progress, "cashdesk_second_dispute_resolved", true);
    Ok(())
}

fn stage1_11_seller_b(chain: &Chain, progress: &mut Value, roster: &StageRoster, seller_b: &Keypair) -> Result<(), Box<dyn Error>> {
    println!("--- 1.11: a second seller (seller_b) stakes, grants two permits, revokes both ---");
    let stake = stake_pda(chain.program_id, seller_b.pubkey());
    let stake_vault = stake_vault_pda(chain.program_id, seller_b.pubkey());

    if chain.client.get_account(&stake).is_err() {
        let instruction = Instruction::new_with_bytes(
            chain.program_id,
            &truststake::instruction::InitializeStake {}.data(),
            truststake::accounts::InitializeStakeAccountConstraints {
                seller: seller_b.pubkey(),
                config: chain.config,
                mint: chain.mint,
                stake,
                stake_vault,
                token_program: spl_token::ID,
                system_program: anchor_lang::system_program::ID,
                event_authority: chain.event_authority,
                program: chain.program_id,
            }
            .to_account_metas(None),
        );
        let signature = send(chain.client, &[instruction], &seller_b.pubkey(), &[seller_b])?;
        print_step("initialize_stake(seller_b)", &signature);
    } else {
        println!("  initialize_stake(seller_b): already done, skipping.");
    }

    let seller_b_token_account =
        load_or_find_or_create_token_account(chain.client, &keypairs_dir(), "seller_b", roster.admin, &chain.mint, &seller_b.pubkey())?;
    let current_staked = read_seller_stake(chain.client, &stake)?.staked;
    if current_staked < SELLER_B_STAKE {
        let shortfall = SELLER_B_STAKE - current_staked;
        top_up_token_balance(chain.client, roster.admin, &chain.mint, &seller_b_token_account, shortfall)?;
        let instruction = Instruction::new_with_bytes(
            chain.program_id,
            &truststake::instruction::AddStake { amount: shortfall }.data(),
            truststake::accounts::AddStakeAccountConstraints {
                seller: seller_b.pubkey(),
                stake,
                stake_vault,
                mint: chain.mint,
                seller_token_account: seller_b_token_account,
                token_program: spl_token::ID,
                event_authority: chain.event_authority,
                program: chain.program_id,
            }
            .to_account_metas(None),
        );
        let signature = send(chain.client, &[instruction], &seller_b.pubkey(), &[seller_b])?;
        print_step(&format!("add_stake(seller_b, {})", format_usdc(shortfall)), &signature);
    } else {
        println!("  add_stake(seller_b): already staked {} >= {}, skipping.", format_usdc(current_staked), format_usdc(SELLER_B_STAKE));
    }

    ensure_permit(chain, progress, "seller_b_cashdesk_permit_lifecycle_done", seller_b, stake, roster.cashdesk, SELLER_B_CASHDESK_PERMIT, "seller_b x CashDesk")?;
    ensure_permit(
        chain,
        progress,
        "seller_b_pixelbazaar_permit_lifecycle_done",
        seller_b,
        stake,
        roster.pixelbazaar,
        SELLER_B_PIXELBAZAAR_PERMIT,
        "seller_b x PixelBazaar",
    )?;

    let cashdesk_revoked_at = ensure_revoked(
        chain,
        progress,
        "seller_b_cashdesk_permit_lifecycle_done",
        "seller_b_cashdesk_revoked_at",
        seller_b,
        roster.cashdesk,
        "seller_b x CashDesk",
    )?;
    let pixelbazaar_revoked_at = ensure_revoked(
        chain,
        progress,
        "seller_b_pixelbazaar_permit_lifecycle_done",
        "seller_b_pixelbazaar_revoked_at",
        seller_b,
        roster.pixelbazaar,
        "seller_b x PixelBazaar",
    )?;

    match cashdesk_revoked_at {
        Some(revoked_at) => println!("  seller_b's CashDesk permit revoked at {revoked_at} (stage 3's release_permit target, 2-day window);"),
        None => println!("  seller_b's CashDesk permit: nothing to report this run (see above)."),
    }
    match pixelbazaar_revoked_at {
        Some(revoked_at) => println!("  seller_b's PixelBazaar permit revoked at {revoked_at} (stage 2's release_permit_early target)."),
        None => println!("  seller_b's PixelBazaar permit: nothing to report this run (see above)."),
    }
    println!("  Using a second seller keeps the original seller's richer state (two permits, one slashed) intact,");
    println!("  since both release paths CLOSE the permit account.");
    Ok(())
}

/// Revokes `marketplace`'s permit for `seller` unless its lifecycle is
/// already complete or the LIVE permit account is already revoked. Reads
/// the permit account's own `revoked_at` field as ground truth rather than
/// trusting a cached value blindly -- the ensure_revoked-staleness fix: a
/// cached value from an EARLIER era at this same PDA (left behind by D1's
/// bug) must never be reported as this era's revocation time, and this run
/// doesn't revoke on that stray era's behalf either (D1: a permit this
/// run did not grant isn't this run's to act on).
fn ensure_revoked(
    chain: &Chain,
    progress: &mut Value,
    lifecycle_key: &str,
    revoked_key: &str,
    seller: &Keypair,
    marketplace: Pubkey,
    name: &str,
) -> Result<Option<i64>, Box<dyn Error>> {
    let lifecycle_done = get_bool(progress, lifecycle_key);
    let permit = permit_pda(chain.program_id, seller.pubkey(), marketplace);
    let cached_revoked_at = get_i64(progress, revoked_key);
    let live_revoked_at = if lifecycle_done {
        None
    } else {
        let state = read_permit(chain.client, &permit)?;
        (state.revoked_at != i64::MAX).then_some(state.revoked_at)
    };

    match decide_permit_revoke(lifecycle_done, live_revoked_at, cached_revoked_at) {
        PermitRevokeDecision::SkipLifecycleDone => {
            println!("  {name}: permit lifecycle already complete, skipping revoke.");
            Ok(None)
        }
        PermitRevokeDecision::SkipStaleCache => {
            println!(
                "  {name}: a permit exists at this address but is unrevoked, while progress.json remembers \
                 an earlier revocation at this same PDA -- a previous run re-granted here after release \
                 (docs/DEMO-SCRIPT-FINDINGS.md, D1). Leaving it untouched."
            );
            Ok(None)
        }
        PermitRevokeDecision::UseLiveRevokedAt(revoked_at) => {
            if cached_revoked_at != Some(revoked_at) {
                set_i64(progress, revoked_key, revoked_at);
            }
            Ok(Some(revoked_at))
        }
        PermitRevokeDecision::Revoke => {
            let signature = send_revoke_permit(chain, seller, marketplace)?;
            print_step(&format!("revoke_permit({name})"), &signature);
            let revoked_at = read_permit(chain.client, &permit)?.revoked_at;
            set_i64(progress, revoked_key, revoked_at);
            Ok(Some(revoked_at))
        }
    }
}

// ==================== Stage 2 ====================

fn run_stage_2(chain: &Chain, progress: &mut Value, roster: &StageRoster) -> Result<(), Box<dyn Error>> {
    println!("=== Stage 2: release_permit_early on seller B's PixelBazaar permit ===");
    let seller_b = load_or_create_keypair(&keypairs_dir(), "seller_b")?;
    let permit = permit_pda(chain.program_id, seller_b.pubkey(), roster.pixelbazaar);
    let lifecycle_key = "seller_b_pixelbazaar_permit_lifecycle_done";

    let lifecycle_done = get_bool(progress, lifecycle_key);
    let permit_exists = chain.client.get_account(&permit).is_ok();
    let cached_revoked_at = get_i64(progress, "seller_b_pixelbazaar_revoked_at");
    let live_revoked_at = if permit_exists {
        let state = read_permit(chain.client, &permit)?;
        (state.revoked_at != i64::MAX).then_some(state.revoked_at)
    } else {
        None
    };
    let now = now_unix();

    let revoked_at = match decide_permit_release(lifecycle_done, permit_exists, live_revoked_at, cached_revoked_at, CLOCK_SKEW_TOLERANCE_SECONDS, now) {
        PermitReleaseDecision::SkipLifecycleDone => {
            println!("  Already released in a previous run, skipping.");
            return Ok(());
        }
        PermitReleaseDecision::SkipAlreadyReleased => {
            println!("  Permit account no longer exists and a revocation was already recorded; treating release as already complete.");
            set_bool(progress, lifecycle_key, true);
            save_progress(progress)?;
            return Ok(());
        }
        PermitReleaseDecision::SkipUnrevoked => {
            println!(
                "  A permit exists at this address but is not revoked -- that's stage 1.11's job, not stage 2's. \
                 Treating stage 2 as already complete for this permit rather than acting on state it doesn't own \
                 (docs/DEMO-SCRIPT-FINDINGS.md, D1: a prior run's bug can leave exactly this shape)."
            );
            return Ok(());
        }
        PermitReleaseDecision::NotYetReachable => {
            println!("  Not yet reachable: stage 1.11 has not recorded seller_b's PixelBazaar revocation yet. Run stage 1 first.");
            return Ok(());
        }
        PermitReleaseDecision::NotYetDue { remaining_seconds } => {
            println!("  Not yet due ({remaining_seconds} seconds remaining).");
            return Ok(());
        }
        PermitReleaseDecision::Proceed { revoked_at } => revoked_at,
    };
    let due_at = revoked_at + CLOCK_SKEW_TOLERANCE_SECONDS;
    println!("  Required: now >= revoked_at ({revoked_at}) + CLOCK_SKEW_TOLERANCE_SECONDS ({CLOCK_SKEW_TOLERANCE_SECONDS}) = {due_at}. Actual now: {now}.");

    let stake = stake_pda(chain.program_id, seller_b.pubkey());
    let stake_before = read_seller_stake(chain.client, &stake)?;
    let permit_state = read_permit(chain.client, &permit)?;
    let remaining_allowance = permit_state.max_slashable - permit_state.slashed;

    // Requires BOTH seller_b and PixelBazaar's current authority to sign;
    // the script holds both.
    let signature = send_release_permit_early(chain, &seller_b, roster.pixelbazaar_authority, roster.pixelbazaar)?;
    print_step("release_permit_early(seller_b, PixelBazaar)", &signature);

    let stake_after = read_seller_stake(chain.client, &stake)?;
    if stake_before.committed - stake_after.committed != remaining_allowance {
        return Err(format!(
            "expected seller_b's committed to drop by exactly the remaining allowance {}, dropped by {} instead",
            format_usdc(remaining_allowance),
            format_usdc(stake_before.committed - stake_after.committed)
        )
        .into());
    }
    if chain.client.get_account(&permit).is_ok() {
        return Err("expected the permit account to no longer exist after release_permit_early".into());
    }
    println!("  Confirmed: committed dropped by {}, permit account is gone.", format_usdc(remaining_allowance));
    set_bool(progress, lifecycle_key, true);
    save_progress(progress)?;
    Ok(())
}

// ==================== Stage 3 ====================

fn run_stage_3(chain: &Chain, progress: &mut Value, roster: &StageRoster) -> Result<(), Box<dyn Error>> {
    println!("=== Stage 3: release_permit on seller B's CashDesk permit ===");
    let seller_b = load_or_create_keypair(&keypairs_dir(), "seller_b")?;
    let permit = permit_pda(chain.program_id, seller_b.pubkey(), roster.cashdesk);
    let lifecycle_key = "seller_b_cashdesk_permit_lifecycle_done";

    let lifecycle_done = get_bool(progress, lifecycle_key);
    let permit_exists = chain.client.get_account(&permit).is_ok();
    let cached_revoked_at = get_i64(progress, "seller_b_cashdesk_revoked_at");
    // Stage 3's wait, unlike stage 2's, depends on the LIVE permit's own
    // complaint_window (grandfathering, docs/DESIGN-v2.md) -- only readable
    // while the permit exists, so `wait_seconds` falls back to the
    // clock-skew tolerance alone when it doesn't (that branch of
    // decide_permit_release never uses it).
    let (live_revoked_at, wait_seconds) = if permit_exists {
        let state = read_permit(chain.client, &permit)?;
        ((state.revoked_at != i64::MAX).then_some(state.revoked_at), state.complaint_window.max(CLOCK_SKEW_TOLERANCE_SECONDS))
    } else {
        (None, CLOCK_SKEW_TOLERANCE_SECONDS)
    };
    let now = now_unix();

    let revoked_at = match decide_permit_release(lifecycle_done, permit_exists, live_revoked_at, cached_revoked_at, wait_seconds, now) {
        PermitReleaseDecision::SkipLifecycleDone => {
            println!("  Already released in a previous run, skipping.");
            return Ok(());
        }
        PermitReleaseDecision::SkipAlreadyReleased => {
            println!("  Permit account no longer exists and a revocation was already recorded; treating release as already complete.");
            set_bool(progress, lifecycle_key, true);
            save_progress(progress)?;
            return Ok(());
        }
        PermitReleaseDecision::SkipUnrevoked => {
            println!(
                "  A permit exists at this address but is not revoked -- that's stage 1.11's job, not stage 3's. \
                 Treating stage 3 as already complete for this permit rather than acting on state it doesn't own \
                 (docs/DEMO-SCRIPT-FINDINGS.md, D1: a prior run's bug can leave exactly this shape)."
            );
            return Ok(());
        }
        PermitReleaseDecision::NotYetReachable => {
            println!("  Not yet reachable: stage 1.11 has not recorded seller_b's CashDesk revocation yet. Run stage 1 first.");
            return Ok(());
        }
        PermitReleaseDecision::NotYetDue { remaining_seconds } => {
            println!("  Not yet due ({remaining_seconds} seconds remaining).");
            return Ok(());
        }
        PermitReleaseDecision::Proceed { revoked_at } => revoked_at,
    };
    let due_at = revoked_at + wait_seconds;
    println!(
        "  Required: now >= revoked_at ({revoked_at}) + max(complaint_window, clock-skew tolerance) ({wait_seconds}) = {due_at}. Actual now: {now}."
    );

    let permit_state = read_permit(chain.client, &permit)?;
    let remaining_allowance = permit_state.max_slashable - permit_state.slashed;
    let stake = stake_pda(chain.program_id, seller_b.pubkey());
    let stake_before = read_seller_stake(chain.client, &stake)?;
    let rent_before = chain.client.get_balance(&seller_b.pubkey())?;

    // Permissionless: called by admin, to demonstrate onchain that a third
    // party can free a seller's collateral.
    let signature = send_release_permit(chain, roster.admin, seller_b.pubkey(), roster.cashdesk)?;
    print_step("release_permit(admin calls it for seller_b, CashDesk)", &signature);

    let stake_after = read_seller_stake(chain.client, &stake)?;
    if stake_before.committed - stake_after.committed != remaining_allowance {
        return Err(format!(
            "expected seller_b's committed to drop by exactly the remaining allowance {}, dropped by {} instead",
            format_usdc(remaining_allowance),
            format_usdc(stake_before.committed - stake_after.committed)
        )
        .into());
    }
    if chain.client.get_account(&permit).is_ok() {
        return Err("expected the permit account to no longer exist after release_permit".into());
    }
    let rent_after = chain.client.get_balance(&seller_b.pubkey())?;
    if rent_after <= rent_before {
        return Err(format!("expected seller_b's balance to rise from the refunded rent ({rent_before} -> {rent_after})").into());
    }
    println!(
        "  Confirmed: committed dropped by {}, permit account is gone, rent went to seller_b ({rent_before} -> {rent_after} lamports).",
        format_usdc(remaining_allowance)
    );
    set_bool(progress, lifecycle_key, true);
    save_progress(progress)?;
    Ok(())
}

// ==================== Stage 4 ====================

fn run_stage_4(chain: &Chain, progress: &Value, roster: &StageRoster) -> Result<(), Box<dyn Error>> {
    println!("=== Stage 4: expire_dispute (SwiftMarket) and close_dispute (CashDesk x2) ===");
    let dir = keypairs_dir();
    let stranger = load_or_create_keypair(&dir, "stranger")?;
    let stranger_target = 3 * SIGNATURE_FEE_HEADROOM + chain.client.get_minimum_balance_for_rent_exemption(0)?;
    top_up_balance(chain.client, roster.admin, &stranger.pubkey(), stranger_target, "stranger")?;

    stage4_1_expire_swiftmarket_dispute(chain, progress, roster, &stranger)?;
    println!();
    stage4_2_close_cashdesk_disputes(chain, progress, roster, &stranger)?;
    Ok(())
}

/// Permissionless, called by `stranger` (neither the buyer nor the
/// marketplace) to show onchain that a stranger can break the deadlock.
fn stage4_1_expire_swiftmarket_dispute(chain: &Chain, progress: &Value, roster: &StageRoster, stranger: &Keypair) -> Result<(), Box<dyn Error>> {
    println!("--- 4.1: expire_dispute (SwiftMarket dispute from 1.9) ---");
    let Some(order_id) = get_bytes32(progress, "swiftmarket_dispute_order_id") else {
        println!("  Not yet reachable: stage 1.9 has not raised the SwiftMarket dispute yet. Run stage 1 first.");
        return Ok(());
    };
    let swiftmarket = marketplace_pda(chain.program_id, SWIFTMARKET_ID);
    let dispute = dispute_pda(chain.program_id, swiftmarket, roster.seller.pubkey(), order_id);
    let dispute_state = read_dispute(chain.client, &dispute)?;
    if dispute_state.status != DisputeStatus::Open as u8 {
        println!("  Already expired, skipping.");
        return Ok(());
    }

    let now = now_unix();
    println!("  Required: now >= expires_at ({}). Actual now: {now}.", dispute_state.expires_at);
    if now < dispute_state.expires_at {
        println!("  Not yet due ({} seconds remaining).", dispute_state.expires_at - now);
        return Ok(());
    }

    let permit = permit_pda(chain.program_id, roster.seller.pubkey(), swiftmarket);
    let marketplace_before = read_marketplace(chain.client, &swiftmarket)?;
    let permit_before = read_permit(chain.client, &permit)?;

    let buyer_swiftmarket_token_account =
        load_or_find_or_create_token_account(chain.client, &keypairs_dir(), "buyer_swiftmarket", roster.admin, &chain.mint, &dispute_state.buyer)?;
    let signature = send_expire_dispute(chain, stranger, swiftmarket, dispute, permit, buyer_swiftmarket_token_account)?;
    print_step("expire_dispute(stranger calls it, SwiftMarket)", &signature);

    let dispute_after = read_dispute(chain.client, &dispute)?;
    let marketplace_after = read_marketplace(chain.client, &swiftmarket)?;
    let permit_after = read_permit(chain.client, &permit)?;
    if dispute_after.status != DisputeStatus::Abandoned as u8 {
        return Err("expected dispute status to be Abandoned after expire_dispute".into());
    }
    if marketplace_after.disputes_abandoned != marketplace_before.disputes_abandoned + 1 {
        return Err("expected SwiftMarket's disputes_abandoned to rise by 1".into());
    }
    if permit_after.open_disputes != permit_before.open_disputes - 1 {
        return Err("expected the SwiftMarket permit's open_disputes to fall by 1".into());
    }
    if permit_after.open_disputes != 0 {
        return Err(format!("expected the SwiftMarket permit's open_disputes to fall to zero, found {}", permit_after.open_disputes).into());
    }
    println!(
        "  Confirmed: status = Abandoned, disputes_abandoned {} -> {}, open_disputes {} -> {}.",
        marketplace_before.disputes_abandoned, marketplace_after.disputes_abandoned, permit_before.open_disputes, permit_after.open_disputes
    );
    Ok(())
}

fn stage4_2_close_cashdesk_disputes(chain: &Chain, progress: &Value, roster: &StageRoster, stranger: &Keypair) -> Result<(), Box<dyn Error>> {
    println!("--- 4.2: close_dispute (CashDesk: the rejected second dispute, and the original upheld one) ---");

    if let Some(order_id) = get_bytes32(progress, "cashdesk_second_dispute_order_id") {
        let dispute = dispute_pda(chain.program_id, roster.cashdesk, roster.seller.pubkey(), order_id);
        try_close_dispute(chain, stranger, dispute, "CashDesk second dispute (1.10, rejected)")?;
    } else {
        println!("  CashDesk second dispute: not yet reachable, stage 1.10 has not run yet.");
    }

    match find_dispute_by_marketplace_seller_status(chain, roster.cashdesk, roster.seller.pubkey(), DisputeStatus::Upheld)? {
        Some((dispute, _)) => {
            try_close_dispute(chain, stranger, dispute, "CashDesk original dispute (2026-08-24 run, upheld)")?;
        }
        None => println!(
            "  CashDesk original dispute: no Upheld DisputeRecord found for this seller on CashDesk \
             (unexpected; the original walk's Step 7 should have created one)."
        ),
    }
    Ok(())
}

fn try_close_dispute(chain: &Chain, stranger: &Keypair, dispute: Pubkey, label: &str) -> Result<(), Box<dyn Error>> {
    let Some(dispute_state) = try_read_dispute(chain.client, &dispute) else {
        println!("  {label}: already closed, skipping.");
        return Ok(());
    };
    if dispute_state.status == DisputeStatus::Open as u8 {
        println!("  {label}: still open, cannot close yet.");
        return Ok(());
    }

    let now = now_unix();
    println!("  {label}: required now >= closable_after ({}). Actual now: {now}.", dispute_state.closable_after);
    if now < dispute_state.closable_after {
        println!("  {label}: not yet due ({} seconds remaining).", dispute_state.closable_after - now);
        return Ok(());
    }

    let rent_before = chain.client.get_balance(&dispute_state.buyer)?;
    // Permissionless: called by a stranger.
    let signature = send_close_dispute(chain, stranger, dispute_state.buyer, dispute)?;
    print_step(&format!("close_dispute({label})"), &signature);

    if chain.client.get_account(&dispute).is_ok() {
        return Err(format!("expected {label}'s dispute record to no longer exist after close_dispute").into());
    }
    let rent_after = chain.client.get_balance(&dispute_state.buyer)?;
    if rent_after <= rent_before {
        return Err(format!("expected {label}'s buyer balance to rise from the refunded rent ({rent_before} -> {rent_after})").into());
    }
    println!("  {label}: confirmed closed, rent refunded to buyer ({rent_before} -> {rent_after} lamports).");
    Ok(())
}

// ==================== Guard decision unit tests ====================
//
// These exercise the pure decision functions above directly -- no
// RpcClient, no devnet -- since tests/test_devnet_demo_parity.rs replays
// this file's instruction sequence against LiteSVM without ever running a
// guard (see that file's module docs), so this is the only place any of
// this script's own decision logic gets tested at all. Named regressions
// map onto docs/DEMO-SCRIPT-FINDINGS.md's defect list (D1, D1-mirror,
// ensure_revoked staleness, D2, D2-sibling, D3, the two latent D1 siblings,
// and D1's stray-permit tolerance).
#[cfg(test)]
mod tests {
    use super::*;

    // ---------- decide_permit_grant ----------

    #[test]
    fn grant_when_never_granted() {
        assert_eq!(decide_permit_grant(false, false), PermitGrantDecision::Grant);
    }

    #[test]
    fn skip_when_already_granted_this_era() {
        assert_eq!(decide_permit_grant(false, true), PermitGrantDecision::SkipAlreadyGranted);
    }

    #[test]
    fn d1_regrant_after_release_is_recognized_as_done() {
        // D1: the permit account is gone (released by an earlier stage of
        // this same run/invocation), which the pre-fix guard read as "never
        // granted" and re-granted into. The lifecycle flag, once true, must
        // win over that same observation.
        assert_eq!(decide_permit_grant(true, false), PermitGrantDecision::SkipLifecycleDone);
    }

    #[test]
    fn lifecycle_done_flag_wins_even_if_account_somehow_reexists() {
        assert_eq!(decide_permit_grant(true, true), PermitGrantDecision::SkipLifecycleDone);
    }

    #[test]
    fn latent_sibling_step5_grant_guard_respects_lifecycle_flag() {
        // Step 5 grants the ORIGINAL seller's CashDesk/PixelBazaar permits
        // with this exact function; nothing releases them today, so
        // lifecycle_done is always false in practice, but the guard must
        // still be ready the moment a release step is added.
        assert_eq!(decide_permit_grant(true, false), PermitGrantDecision::SkipLifecycleDone);
        assert_eq!(decide_permit_grant(false, false), PermitGrantDecision::Grant);
    }

    #[test]
    fn latent_sibling_step1_7_swiftmarket_grant_guard_respects_lifecycle_flag() {
        // Step 1.7 grants the ORIGINAL seller's SwiftMarket permit with the
        // same decision function, same reasoning as the CashDesk/PixelBazaar
        // case above.
        assert_eq!(decide_permit_grant(true, false), PermitGrantDecision::SkipLifecycleDone);
        assert_eq!(decide_permit_grant(false, false), PermitGrantDecision::Grant);
    }

    // ---------- decide_permit_revoke ----------

    #[test]
    fn revoke_when_freshly_granted_and_unrevoked_with_no_history() {
        assert_eq!(decide_permit_revoke(false, None, None), PermitRevokeDecision::Revoke);
    }

    #[test]
    fn use_live_value_when_already_revoked() {
        assert_eq!(decide_permit_revoke(false, Some(1_700_000_000), None), PermitRevokeDecision::UseLiveRevokedAt(1_700_000_000));
    }

    #[test]
    fn revoke_skips_entirely_once_lifecycle_is_done() {
        assert_eq!(decide_permit_revoke(true, None, None), PermitRevokeDecision::SkipLifecycleDone);
    }

    #[test]
    fn ensure_revoked_staleness_cached_value_not_trusted_when_permit_is_unrevoked() {
        // The permit currently at this PDA is NOT revoked (live_revoked_at
        // is None), yet progress.json remembers a revocation from an
        // EARLIER era at the same PDA. That cached value must not be
        // handed out as if it described the live permit.
        let stale_cached_revoked_at = Some(1_700_000_000);
        assert_eq!(decide_permit_revoke(false, None, stale_cached_revoked_at), PermitRevokeDecision::SkipStaleCache);
    }

    #[test]
    fn item8_stray_unrevoked_permit_is_not_revoked_by_this_run() {
        // Same chain shape as the staleness case: a pre-existing unrevoked
        // permit this run did not create. The fix must not act on it
        // (neither report the stale cached value nor call revoke_permit).
        let stale_cached_revoked_at = Some(1_788_000_000);
        assert_eq!(decide_permit_revoke(false, None, stale_cached_revoked_at), PermitRevokeDecision::SkipStaleCache);
    }

    // ---------- decide_permit_release ----------

    #[test]
    fn release_proceeds_once_due() {
        let decision = decide_permit_release(false, true, Some(1_000), Some(1_000), 100, 1_200);
        assert_eq!(decision, PermitReleaseDecision::Proceed { revoked_at: 1_000 });
    }

    #[test]
    fn release_not_yet_due() {
        let decision = decide_permit_release(false, true, Some(1_000), Some(1_000), 100, 1_050);
        assert_eq!(decision, PermitReleaseDecision::NotYetDue { remaining_seconds: 50 });
    }

    #[test]
    fn release_skips_once_lifecycle_is_done() {
        let decision = decide_permit_release(true, true, Some(1_000), Some(1_000), 100, 999_999);
        assert_eq!(decision, PermitReleaseDecision::SkipLifecycleDone);
    }

    #[test]
    fn release_not_yet_reachable_when_nothing_recorded() {
        let decision = decide_permit_release(false, false, None, None, 100, 1_200);
        assert_eq!(decision, PermitReleaseDecision::NotYetReachable);
    }

    #[test]
    fn release_self_heals_when_account_gone_but_revocation_was_recorded() {
        let decision = decide_permit_release(false, false, None, Some(1_000), 100, 1_200);
        assert_eq!(decision, PermitReleaseDecision::SkipAlreadyReleased);
    }

    #[test]
    fn d1_mirror_regrant_after_release_is_not_treated_as_already_released_or_due() {
        // D1-mirror: the permit account EXISTS again (D1 re-granted it) but
        // is not revoked. The pre-fix guard inferred "already released"
        // purely from non-existence, which is exactly backwards once the
        // account has been re-created. The fixed guard must neither treat
        // this as done nor attempt release_permit_early against it.
        let decision = decide_permit_release(false, true, None, Some(1_000), 100, 1_200);
        assert_eq!(decision, PermitReleaseDecision::SkipUnrevoked);
    }

    #[test]
    fn item8_stray_unrevoked_permit_release_guard_does_not_attempt_release() {
        let decision = decide_permit_release(false, true, None, Some(1_788_000_000), CLOCK_SKEW_TOLERANCE_SECONDS, 2_000_000_000);
        assert_eq!(decision, PermitReleaseDecision::SkipUnrevoked);
    }

    // ---------- decide_free_collateral_topup ----------

    #[test]
    fn topup_needed_before_permit_granted() {
        assert_eq!(decide_free_collateral_topup(false, 0, usdc(200)), TopUpDecision::TopUp { shortfall: usdc(200) });
    }

    #[test]
    fn topup_skips_when_already_at_target_pre_grant() {
        assert_eq!(decide_free_collateral_topup(false, usdc(200), usdc(200)), TopUpDecision::SkipAlreadyAtTarget);
    }

    #[test]
    fn d3_topup_target_correct_after_later_commits() {
        // The permit already exists (1.7/1.8 already committed 150 of the
        // 200 this step topped up), so live free collateral is only 50 --
        // below the 200 target. The pre-fix guard re-evaluated "free >=
        // 200" post-grant and topped up again, overshooting. Once the
        // permit exists, this step's job is already done regardless of
        // what free collateral looks like now.
        assert_eq!(decide_free_collateral_topup(true, usdc(50), usdc(200)), TopUpDecision::SkipPermitAlreadyGranted);
    }

    // ---------- decide_step7_dispute ----------

    #[test]
    fn step7_proceeds_when_never_done() {
        assert_eq!(decide_step7_dispute(false, 0, usdc(80)), Step7DisputeDecision::Proceed);
    }

    #[test]
    fn step7_skips_when_flag_already_set() {
        assert_eq!(decide_step7_dispute(true, 0, usdc(80)), Step7DisputeDecision::SkipFlagSet);
    }

    #[test]
    fn d2_step7_guard_survives_stage4_close() {
        // Stage 4.2 closes the DisputeRecord that step 7's OLD guard
        // scanned for, so after that close a fresh scan finds nothing and
        // the old guard would re-raise a second real claim. The permit's
        // `slashed` total is never touched by close_dispute, so it still
        // proves the claim already landed even with the flag unset (the
        // exact migration state of a run predating this flag).
        assert_eq!(decide_step7_dispute(false, usdc(80), usdc(80)), Step7DisputeDecision::SkipAlreadySlashed);
    }

    // ---------- decide_second_dispute_resolve ----------

    #[test]
    fn second_dispute_proceeds_while_open() {
        assert_eq!(decide_second_dispute_resolve(false, Some(DisputeStatus::Open as u8)), SecondDisputeResolveDecision::Proceed);
    }

    #[test]
    fn second_dispute_skips_when_flag_already_set() {
        assert_eq!(decide_second_dispute_resolve(true, Some(DisputeStatus::Open as u8)), SecondDisputeResolveDecision::SkipFlagSet);
    }

    #[test]
    fn second_dispute_skips_when_already_resolved_onchain() {
        assert_eq!(decide_second_dispute_resolve(false, Some(DisputeStatus::Rejected as u8)), SecondDisputeResolveDecision::SkipAlreadyResolved);
    }

    #[test]
    fn d2_sibling_dispute_gone_is_treated_as_resolved_not_reraised() {
        // D2-sibling: stage 4.2a closes exactly this DisputeRecord. The
        // pre-fix guard did a hard `read_dispute(...)?` that error-crashes
        // the whole script once the account is gone. close_dispute only
        // ever succeeds on a non-Open dispute, so "gone" always implies
        // "was resolved" -- never "still open, re-resolve it."
        assert_eq!(decide_second_dispute_resolve(false, None), SecondDisputeResolveDecision::SkipDisputeGone);
    }
}
