//! Replays examples/devnet_demo.rs's ENTIRE instruction sequence against
//! LiteSVM -- the original 8-step walk, then all four time-gated stages --
//! same handlers, same order, same amounts, so a broken walk is caught here
//! for free instead of by real devnet SOL. Read examples/devnet_demo.rs
//! alongside this file, and change both together: the demo hand-builds each
//! `Instruction` for a real RPC client, while this test reaches the same
//! instructions through the `World` harness's thinner wrappers
//! (common/mod.rs), so nothing here is shared code that would hide a drift
//! between the two. LiteSVM can warp its own clock; devnet cannot -- this is
//! the ONLY place the 1-hour, 2-day, and 30-day waits get proven before real
//! SOL is spent on them.
//!
//! Two tests share the setup/stage helpers below:
//!
//!  - `test_devnet_demo_sequence_parity` replays the sequence against a
//!    BLANK chain, matching a genuinely fresh deployment.
//!  - `test_devnet_demo_resumed_after_prior_slash` first replays the
//!    ORIGINAL eight-step walk once (using ITS OWN fixed, historical
//!    amounts -- staked 500, committed 400, withdraw 100, an 80-claim
//!    dispute upheld), landing the chain in exactly the state the real
//!    2026-08-24 devnet run left behind (staked 320, committed 320, free
//!    0), and only THEN replays a second pass on top of that -- exactly
//!    what a resumed run of devnet_demo.rs does. A blank-chain-only test
//!    cannot see a bug that only exists once a slash has moved staked and
//!    committed together (SellerStake::slash): every fresh-start assumption
//!    a step makes is trivially true on a blank chain and stays untested
//!    for real devnet's actual, never-fresh state.
//!
//! ## Coverage map: all 19 handlers (lib.rs), where each is exercised here
//!
//!  1. initialize_config             -- Step 2
//!  2. propose_config_authority      -- 1.1 (twice: admin->governance, governance->admin)
//!  3. accept_config_authority       -- 1.1 (twice: governance, admin)
//!  4. register_marketplace          -- Step 3 (CashDesk, PixelBazaar), 1.2 (SwiftMarket)
//!  5. update_marketplace            -- 1.3 (PixelBazaar bond), 1.4 (CashDesk receipt signer)
//!  6. propose_marketplace_authority -- 1.5
//!  7. accept_marketplace_authority  -- 1.5
//!  8. initialize_stake              -- Step 4 (seller), 1.11 (seller_b)
//!  9. add_stake                     -- Step 4, 1.6 (top-up), 1.11 (seller_b)
//! 10. withdraw_stake                -- Step 6 (succeeds, then must-fail)
//! 11. grant_permit                  -- Step 5 (x2), 1.7 (SwiftMarket), 1.11 (seller_b x2)
//! 12. increase_permit               -- 1.8
//! 13. revoke_permit                 -- 1.11 (x2, seller_b)
//! 14. release_permit                -- Stage 3 (seller_b x CashDesk)
//! 15. release_permit_early          -- Stage 2 (seller_b x PixelBazaar)
//! 16. raise_dispute                 -- Step 7, 1.9 (SwiftMarket), 1.10 (CashDesk second)
//! 17. resolve_dispute               -- Step 7 (upheld), 1.10 (rejected)
//! 18. expire_dispute                -- Stage 4.1 (SwiftMarket dispute)
//! 19. close_dispute                 -- Stage 4.2 (CashDesk second dispute, original dispute)
mod common;

use common::{assert_error_code, initial_admin_keypair, usdc, World};
use anchor_lang::prelude::Pubkey;
use solana_keypair::Keypair;
use solana_signer::Signer;
use truststake::{
    constants::{CHAIN_ID_DEVNET, CLOCK_SKEW_TOLERANCE_SECONDS, DISPUTE_EXPIRY_SECONDS, MAX_COMPLAINT_WINDOW_SECONDS, RECEIPT_DOMAIN, SECONDS_PER_DAY},
    error::TrustStakeError,
    receipt::OrderReceipt,
    state::DisputeStatus,
};

/// CashDesk: 2-day complaint window (the protocol floor), 10% bond. The
/// buyer's dispute in this test lands against CashDesk.
const CASHDESK_ID: [u8; 16] = *b"cashdesk-demo-01";
const CASHDESK_COMPLAINT_WINDOW_SECONDS: i64 = 2 * SECONDS_PER_DAY;
const CASHDESK_BOND_BPS: u16 = 1_000;

/// PixelBazaar: 7-day window, 5% bond, visibly distinct from CashDesk's.
/// Never disputed in the original 8-step walk -- its permit's untouched
/// final state is the whole point of that walk's step 8.
const PIXELBAZAAR_ID: [u8; 16] = *b"pixelbazaar-demo";
const PIXELBAZAAR_COMPLAINT_WINDOW_SECONDS: i64 = 7 * SECONDS_PER_DAY;
const PIXELBAZAAR_BOND_BPS: u16 = 500;

/// SwiftMarket: 30-day window (the protocol ceiling), 0 bond -- legal on
/// purpose (CLAUDE.md's deliberate-tradeoffs list).
const SWIFTMARKET_ID: [u8; 16] = *b"swiftmarket-demo";
const SWIFTMARKET_COMPLAINT_WINDOW_SECONDS: i64 = MAX_COMPLAINT_WINDOW_SECONDS;
const SWIFTMARKET_BOND_BPS: u16 = 0;

const PIXELBAZAAR_NEW_BOND_BPS: u16 = 300;

/// Everything steps 6 onward and stages 1-4 need from the original
/// setup (config, two marketplaces, one seller staked and granted two
/// permits). Bundled so the two tests below and `run_stage_1_through_4`
/// share one definition instead of five-plus positional parameters each.
struct DemoActors {
    admin: Keypair,
    seller: Keypair,
    buyer: Keypair,
    cashdesk_authority: Keypair,
    cashdesk_receipt_signer: Keypair,
    cashdesk_arbiter: Keypair,
    pixelbazaar_authority: Keypair,
    seller_token_account: Pubkey,
    buyer_token_account: Pubkey,
    cashdesk: Pubkey,
    pixelbazaar: Pubkey,
    stake_pda: Pubkey,
    cashdesk_permit_pda: Pubkey,
    pixelbazaar_permit_pda: Pubkey,
}

/// Steps 1-5 of the original walk: test mint (via `World::new`), config,
/// two marketplaces, one seller staked to `stake_amount`, and a permit
/// granted to each marketplace. Identical for both tests below -- neither
/// test's bug lives here.
fn setup_steps_1_to_5(world: &mut World, stake_amount: u64, cashdesk_permit: u64, pixelbazaar_permit: u64) -> DemoActors {
    let admin = initial_admin_keypair();
    let seller = Keypair::new();
    let buyer = Keypair::new();
    let cashdesk_authority = Keypair::new();
    let cashdesk_receipt_signer = Keypair::new();
    let cashdesk_arbiter = Keypair::new();
    let pixelbazaar_authority = Keypair::new();
    let pixelbazaar_receipt_signer = Keypair::new();
    let pixelbazaar_arbiter = Keypair::new();

    const SOL: u64 = 1_000_000_000;
    world.svm.airdrop(&seller.pubkey(), SOL).expect("airdrop seller");
    world.svm.airdrop(&buyer.pubkey(), SOL).expect("airdrop buyer");
    world.svm.airdrop(&cashdesk_authority.pubkey(), SOL).expect("airdrop cashdesk authority");
    world.svm.airdrop(&pixelbazaar_authority.pubkey(), SOL).expect("airdrop pixelbazaar authority");
    world.svm.airdrop(&cashdesk_arbiter.pubkey(), SOL).expect("airdrop cashdesk arbiter");

    let seller_token_account = world.create_funded_token_account(world.mint, seller.pubkey(), 0);
    let buyer_token_account = world.create_funded_token_account(world.mint, buyer.pubkey(), usdc(10));

    world.initialize_config(&admin, CHAIN_ID_DEVNET).expect("initialize_config must succeed");

    world
        .register_marketplace(
            &cashdesk_authority,
            CASHDESK_ID,
            cashdesk_receipt_signer.pubkey(),
            cashdesk_arbiter.pubkey(),
            CASHDESK_COMPLAINT_WINDOW_SECONDS,
            CASHDESK_BOND_BPS,
        )
        .expect("register_marketplace(CashDesk) must succeed");
    world
        .register_marketplace(
            &pixelbazaar_authority,
            PIXELBAZAAR_ID,
            pixelbazaar_receipt_signer.pubkey(),
            pixelbazaar_arbiter.pubkey(),
            PIXELBAZAAR_COMPLAINT_WINDOW_SECONDS,
            PIXELBAZAAR_BOND_BPS,
        )
        .expect("register_marketplace(PixelBazaar) must succeed");
    let cashdesk = world.marketplace_pda(&CASHDESK_ID);
    let pixelbazaar = world.marketplace_pda(&PIXELBAZAAR_ID);

    world.initialize_stake(&seller).expect("initialize_stake must succeed");
    let stake_pda = world.stake_pda(&seller.pubkey());
    top_up_staked(world, &seller, seller_token_account, stake_pda, stake_amount);

    world
        .grant_permit(&seller, cashdesk, cashdesk_permit)
        .expect("grant_permit(CashDesk) must succeed");
    world
        .grant_permit(&seller, pixelbazaar, pixelbazaar_permit)
        .expect("grant_permit(PixelBazaar) must succeed");

    let stake_after_grants = world.read_seller_stake(&stake_pda);
    assert_eq!(stake_after_grants.staked, stake_amount);
    assert_eq!(stake_after_grants.committed, cashdesk_permit + pixelbazaar_permit);

    let cashdesk_permit_pda = world.permit_pda(&seller.pubkey(), &cashdesk);
    let pixelbazaar_permit_pda = world.permit_pda(&seller.pubkey(), &pixelbazaar);

    DemoActors {
        admin,
        seller,
        buyer,
        cashdesk_authority,
        cashdesk_receipt_signer,
        cashdesk_arbiter,
        pixelbazaar_authority,
        seller_token_account,
        buyer_token_account,
        cashdesk,
        pixelbazaar,
        stake_pda,
        cashdesk_permit_pda,
        pixelbazaar_permit_pda,
    }
}

/// Mirrors devnet_demo.rs's Step 4 / 1.6 top-up pattern: tops `staked` up
/// to `target`, minting and adding only the shortfall (0 if already at or
/// above target). Returns the amount actually added, so a caller can fold
/// it into its own before/after bookkeeping instead of re-deriving it.
fn top_up_staked(world: &mut World, seller: &Keypair, seller_token_account: Pubkey, stake_pda: Pubkey, target: u64) -> u64 {
    let current = world.read_seller_stake(&stake_pda).staked;
    if current >= target {
        return 0;
    }
    let shortfall = target - current;
    world.mint_to(&seller_token_account, shortfall);
    world.add_stake(seller, seller_token_account, shortfall).expect("add_stake (top-up) must succeed");
    shortfall
}

/// Raises a receipt-backed dispute against CashDesk for `order_amount` and
/// has CashDesk's arbiter uphold it for `claim`, mirroring the original
/// walk's Step 7. Returns the order_id, which stage 4 needs to locate this
/// same `DisputeRecord` for `close_dispute`.
fn raise_and_uphold_cashdesk_dispute(world: &mut World, actors: &DemoActors, order_amount: u64, claim: u64) -> [u8; 32] {
    let now = world.now();
    let order_id = Keypair::new().pubkey().to_bytes();
    let receipt = OrderReceipt {
        domain: RECEIPT_DOMAIN,
        program_id: truststake::ID,
        chain_id: CHAIN_ID_DEVNET,
        marketplace_id: CASHDESK_ID,
        seller: actors.seller.pubkey(),
        buyer: actors.buyer.pubkey(),
        order_id,
        amount: order_amount,
        issued_at: now,
        expires_at: now + 7 * SECONDS_PER_DAY,
    };
    world
        .raise_dispute(&actors.buyer, actors.buyer_token_account, actors.cashdesk, &receipt, &actors.cashdesk_receipt_signer, claim)
        .expect("raise_dispute must succeed");
    world
        .resolve_dispute(&actors.cashdesk_arbiter, actors.cashdesk, order_id, actors.buyer_token_account, true)
        .expect("resolve_dispute(upheld = true) must succeed");
    order_id
}

/// Mirrors devnet_demo.rs's (post-fix) Step 6: withdraws half of LIVE free
/// collateral (staked - committed), topping up first if free is below
/// `min_free`, then attempts to withdraw one more than whatever is free
/// afterward -- which the cap must refuse. Deriving both amounts from live
/// state, rather than fixed constants, is what makes this step correct
/// whether the chain is fresh or already carries a prior slash (see the
/// module doc comment). Returns (top_up_amount, withdraw_amount) so a
/// caller can fold them into its own before/after bookkeeping.
fn withdraw_free_collateral_demo(world: &mut World, actors: &DemoActors, min_free: u64) -> (u64, u64) {
    let stake_state = world.read_seller_stake(&actors.stake_pda);
    let mut free = stake_state.staked.saturating_sub(stake_state.committed);
    let mut top_up_amount = 0;
    if free < min_free {
        top_up_amount = min_free - free;
        world.mint_to(&actors.seller_token_account, top_up_amount);
        world
            .add_stake(&actors.seller, actors.seller_token_account, top_up_amount)
            .expect("add_stake (withdraw-demo top-up) must succeed");
        free = min_free;
    }

    let withdraw_amount = free / 2;
    world
        .withdraw_stake(&actors.seller, actors.seller_token_account, withdraw_amount)
        .expect("withdraw_stake (half of live free collateral) must succeed");

    let stake_after = world.read_seller_stake(&actors.stake_pda);
    let free_after = stake_after.staked.saturating_sub(stake_after.committed);
    let withdraw_attempt_too_much = free_after + 1;
    let result = world.withdraw_stake(&actors.seller, actors.seller_token_account, withdraw_attempt_too_much);
    assert_error_code(&result, u32::from(TrustStakeError::CommittedExceedsStaked));

    (top_up_amount, withdraw_amount)
}

#[test]
fn test_devnet_demo_sequence_parity() {
    // usdc() is a plain fn (not const fn), so these are computed here
    // rather than as module-level consts.
    let stake_amount: u64 = usdc(500);
    let cashdesk_permit: u64 = usdc(200);
    let pixelbazaar_permit: u64 = usdc(200);
    let order_amount: u64 = usdc(80);
    let claim_amount: u64 = order_amount;

    let mut world = World::new();
    let actors = setup_steps_1_to_5(&mut world, stake_amount, cashdesk_permit, pixelbazaar_permit);

    // Step 6: withdraw half of the live free collateral, then hit the cap
    // on a second attempt. On a blank chain, free = 500 - 400 = 100, so no
    // top-up is needed (top_up_amount == 0).
    let (topup_amount, withdraw_amount) = withdraw_free_collateral_demo(&mut world, &actors, usdc(20));
    assert_eq!(topup_amount, 0, "a blank chain already has 100 free, above the demo's minimum");

    // Step 7: a CashDesk buyer disputes an 80 order; CashDesk's arbiter
    // upholds it.
    let original_upheld_order_id = raise_and_uphold_cashdesk_dispute(&mut world, &actors, order_amount, claim_amount);

    // Step 8: CashDesk's permit is visibly slashed; PixelBazaar's is
    // completely untouched, even though both draw on the same stake.
    let cashdesk_permit_state = world.read_permit(&actors.cashdesk_permit_pda);
    let pixelbazaar_permit_state = world.read_permit(&actors.pixelbazaar_permit_pda);

    assert_eq!(cashdesk_permit_state.slashed, claim_amount);
    assert_eq!(cashdesk_permit_state.max_slashable, cashdesk_permit);
    assert_eq!(pixelbazaar_permit_state.slashed, 0);
    assert_eq!(pixelbazaar_permit_state.max_slashable, pixelbazaar_permit);

    let stake_final = world.read_seller_stake(&actors.stake_pda);
    assert_eq!(stake_final.staked, stake_amount + topup_amount - withdraw_amount - claim_amount);
    assert_eq!(stake_final.committed, cashdesk_permit + pixelbazaar_permit - claim_amount);

    // The original 8-step walk stops here (see docs/TESTING.md). Stages 1-4
    // pick up from this exact state.
    run_stage_1_through_4(&mut world, &actors, original_upheld_order_id);
}

/// Reproduces the real 2026-08-31 devnet failure: a resumed run of
/// devnet_demo.rs against a chain that already carries a prior slash.
/// Phase A replays the ORIGINAL eight-step walk once, using ITS OWN fixed,
/// historical amounts (this is what actually happened on devnet on
/// 2026-08-24 and cannot be "derived from live state" because at that
/// point there was no prior state to derive from). Phase B then replays a
/// SECOND pass on top of that -- exactly what devnet_demo.rs's `main` does
/// on every later invocation -- which is where the real run broke before
/// this fix: Step 6's withdrawal amounts were hardcoded against a
/// fresh-chain assumption (committed = 400) that Phase A's own slash
/// already falsified (committed = 320), so the fixed second withdrawal
/// (50) fell inside the real cap instead of outside it. Phase B below now
/// calls the SAME live-derivation helper Test 1 uses, which is what makes
/// this pass.
#[test]
fn test_devnet_demo_resumed_after_prior_slash() {
    let stake_amount: u64 = usdc(500);
    let cashdesk_permit: u64 = usdc(200);
    let pixelbazaar_permit: u64 = usdc(200);

    let mut world = World::new();
    let actors = setup_steps_1_to_5(&mut world, stake_amount, cashdesk_permit, pixelbazaar_permit);

    // ---- Phase A: the 2026-08-24 run (fixed, historical amounts) ----
    world
        .withdraw_stake(&actors.seller, actors.seller_token_account, usdc(100))
        .expect("Phase A: withdraw_stake(100) must succeed on the fresh chain");
    let original_upheld_order_id = raise_and_uphold_cashdesk_dispute(&mut world, &actors, usdc(80), usdc(80));

    let stake_after_phase_a = world.read_seller_stake(&actors.stake_pda);
    assert_eq!(stake_after_phase_a.staked, usdc(320), "Phase A must leave staked at devnet's real 2026-08-24 figure");
    assert_eq!(stake_after_phase_a.committed, usdc(320), "Phase A must leave committed at devnet's real 2026-08-24 figure");
    world.svm.expire_blockhash(); // else Phase B's identical withdraw_stake(100) below is deduped as AlreadyProcessed

    // ---- Phase B: a resumed run against that already-slashed chain ----
    // Step 4-equivalent: top up staked back to the fixed target, exactly
    // like devnet_demo.rs's real (idempotent) Step 4.
    let topup = top_up_staked(&mut world, &actors.seller, actors.seller_token_account, actors.stake_pda, stake_amount);
    assert_eq!(topup, usdc(180), "500 - 320 staked");

    // Step 5: both permits already granted (existence-gated in the real
    // script), nothing to do.

    // Step 6: this is the regression check. Free collateral here is 500 -
    // 320 = 180 (well above the 20 minimum), so no top-up occurs; the
    // withdraw/attempt amounts (90, then 91) are derived live rather than
    // being the stale 100/50 that broke the real 2026-08-31 run.
    let (topup_amount, withdraw_amount) = withdraw_free_collateral_demo(&mut world, &actors, usdc(20));
    assert_eq!(topup_amount, 0, "180 free is already above the demo's 20 minimum");
    assert_eq!(withdraw_amount, usdc(90), "half of the live 180 free collateral");

    // Step 7: the Upheld dispute already exists from Phase A
    // (existence-gated in the real script); nothing to do.

    run_stage_1_through_4(&mut world, &actors, original_upheld_order_id);
}

/// Stages 1-4 (see the module doc comment): shared between both tests
/// above, since neither test's bug lives here -- this is the part of the
/// file that was already correctly deriving its amounts from live state
/// (1.6's top-up, stage 2/3's remaining-allowance reads) before this task.
fn run_stage_1_through_4(world: &mut World, actors: &DemoActors, original_upheld_order_id: [u8; 32]) {
    // ================= Stage 1: immediate steps (1.1 - 1.11) =================

    let governance = Keypair::new();
    let swiftmarket_authority = Keypair::new();
    let swiftmarket_authority_v2 = Keypair::new();
    let swiftmarket_receipt_signer = Keypair::new();
    let swiftmarket_arbiter = Keypair::new();
    let cashdesk_receipt_signer_v2 = Keypair::new();
    let buyer_swiftmarket = Keypair::new();
    let seller_b = Keypair::new();

    const SOL: u64 = 1_000_000_000;
    for pubkey in [
        governance.pubkey(),
        swiftmarket_authority.pubkey(),
        swiftmarket_authority_v2.pubkey(),
        buyer_swiftmarket.pubkey(),
        seller_b.pubkey(),
    ] {
        world.svm.airdrop(&pubkey, SOL).expect("airdrop stage 1 wallet");
    }

    // 1.1: propose_config_authority / accept_config_authority, admin ->
    // governance -> admin. Config.authority confers no powers in this
    // program; this proves the transfer mechanism, not that the role does
    // anything (see docs/DESIGN-v2.md).
    world
        .propose_config_authority(&actors.admin, governance.pubkey())
        .expect("propose_config_authority(admin -> governance) must succeed");
    world
        .accept_config_authority(&governance)
        .expect("accept_config_authority(governance) must succeed");
    assert_eq!(world.read_config().authority, governance.pubkey());
    world
        .propose_config_authority(&governance, actors.admin.pubkey())
        .expect("propose_config_authority(governance -> admin) must succeed");
    world
        .accept_config_authority(&actors.admin)
        .expect("accept_config_authority(admin) must succeed");
    let config_after_roundtrip = world.read_config();
    assert_eq!(config_after_roundtrip.authority, actors.admin.pubkey());
    assert_eq!(config_after_roundtrip.pending_authority, Pubkey::default());

    // 1.2: register_marketplace(SwiftMarket, 30-day window, 0 bond). Zero
    // bond is legal on purpose; this is the first time both ends of the
    // legal bond range (0 and CashDesk's 1,000 bps) exist side by side.
    world
        .register_marketplace(
            &swiftmarket_authority,
            SWIFTMARKET_ID,
            swiftmarket_receipt_signer.pubkey(),
            swiftmarket_arbiter.pubkey(),
            SWIFTMARKET_COMPLAINT_WINDOW_SECONDS,
            SWIFTMARKET_BOND_BPS,
        )
        .expect("register_marketplace(SwiftMarket) must succeed");
    let swiftmarket = world.marketplace_pda(&SWIFTMARKET_ID);
    assert_eq!(world.read_marketplace(&swiftmarket).bond_bps, 0);

    // 1.3: update_marketplace(PixelBazaar, new_bond_bps). Must not touch
    // the original seller's already-granted PixelBazaar permit.
    world
        .update_marketplace(&actors.pixelbazaar_authority, actors.pixelbazaar, None, None, None, Some(PIXELBAZAAR_NEW_BOND_BPS))
        .expect("update_marketplace(PixelBazaar) must succeed");
    assert_eq!(world.read_marketplace(&actors.pixelbazaar).bond_bps, PIXELBAZAAR_NEW_BOND_BPS);
    assert_eq!(
        world.read_permit(&actors.pixelbazaar_permit_pda).bond_bps,
        PIXELBAZAAR_BOND_BPS,
        "PixelBazaar's existing permit must keep its frozen bond_bps, not follow the marketplace's live rate"
    );

    // 1.4: update_marketplace(CashDesk, new_receipt_signer). An honest
    // rotation must record the old signer and a non-zero rotation
    // timestamp, not silently void outstanding receipts.
    world
        .update_marketplace(&actors.cashdesk_authority, actors.cashdesk, Some(cashdesk_receipt_signer_v2.pubkey()), None, None, None)
        .expect("update_marketplace(CashDesk, new_receipt_signer) must succeed");
    let cashdesk_after_rotation = world.read_marketplace(&actors.cashdesk);
    assert_eq!(cashdesk_after_rotation.receipt_signer, cashdesk_receipt_signer_v2.pubkey());
    assert_eq!(cashdesk_after_rotation.prev_receipt_signer, actors.cashdesk_receipt_signer.pubkey());
    assert!(cashdesk_after_rotation.signer_rotated_at != 0);

    // 1.5: propose_marketplace_authority / accept_marketplace_authority,
    // SwiftMarket -> v2. Every later SwiftMarket-authority step must use v2.
    world
        .propose_marketplace_authority(&swiftmarket_authority, swiftmarket, swiftmarket_authority_v2.pubkey())
        .expect("propose_marketplace_authority(SwiftMarket -> v2) must succeed");
    world
        .accept_marketplace_authority(&swiftmarket_authority_v2, swiftmarket)
        .expect("accept_marketplace_authority(SwiftMarket, v2) must succeed");
    assert_eq!(world.read_marketplace(&swiftmarket).authority, swiftmarket_authority_v2.pubkey());

    // 1.6: top up the original seller's free collateral before granting
    // against SwiftMarket.
    let stake_before_topup = world.read_seller_stake(&actors.stake_pda);
    let free = stake_before_topup.staked.saturating_sub(stake_before_topup.committed);
    let topup_target = usdc(200);
    if free < topup_target {
        let topup_amount = topup_target - free;
        world.mint_to(&actors.seller_token_account, topup_amount);
        world
            .add_stake(&actors.seller, actors.seller_token_account, topup_amount)
            .expect("add_stake (1.6 top-up) must succeed");
    }
    let stake_after_topup = world.read_seller_stake(&actors.stake_pda);
    assert!(stake_after_topup.staked.saturating_sub(stake_after_topup.committed) >= topup_target);

    // 1.7 + 1.8: grant_permit(SwiftMarket, 100), then
    // increase_permit(SwiftMarket, +50). granted_at must survive the
    // increase unchanged: an increase modifies the permit's existing era,
    // it does not start a new one.
    let swiftmarket_grant = usdc(100);
    let swiftmarket_increase = usdc(50);
    world
        .grant_permit(&actors.seller, swiftmarket, swiftmarket_grant)
        .expect("grant_permit(SwiftMarket) must succeed");
    let swiftmarket_permit_pda = world.permit_pda(&actors.seller.pubkey(), &swiftmarket);
    let granted_at_before_increase = world.read_permit(&swiftmarket_permit_pda).granted_at;
    world
        .increase_permit(&actors.seller, swiftmarket, swiftmarket_increase)
        .expect("increase_permit(SwiftMarket) must succeed");
    let swiftmarket_permit_after_increase = world.read_permit(&swiftmarket_permit_pda);
    assert_eq!(swiftmarket_permit_after_increase.max_slashable, swiftmarket_grant + swiftmarket_increase);
    assert_eq!(
        swiftmarket_permit_after_increase.granted_at, granted_at_before_increase,
        "increase_permit must not change granted_at"
    );

    // 1.9: raise_dispute(SwiftMarket), deliberately left unresolved --
    // stage 4.1's expire_dispute target.
    let buyer_swiftmarket_token_account = world.create_funded_token_account(world.mint, buyer_swiftmarket.pubkey(), usdc(1));
    let swiftmarket_order_amount = usdc(80);
    let swiftmarket_claim = usdc(40);
    let swiftmarket_now = world.now();
    let swiftmarket_receipt = OrderReceipt {
        domain: RECEIPT_DOMAIN,
        program_id: truststake::ID,
        chain_id: CHAIN_ID_DEVNET,
        marketplace_id: SWIFTMARKET_ID,
        seller: actors.seller.pubkey(),
        buyer: buyer_swiftmarket.pubkey(),
        order_id: Keypair::new().pubkey().to_bytes(),
        amount: swiftmarket_order_amount,
        issued_at: swiftmarket_now,
        expires_at: swiftmarket_now + 7 * SECONDS_PER_DAY,
    };
    world
        .raise_dispute(
            &buyer_swiftmarket,
            buyer_swiftmarket_token_account,
            swiftmarket,
            &swiftmarket_receipt,
            &swiftmarket_receipt_signer,
            swiftmarket_claim,
        )
        .expect("raise_dispute(SwiftMarket) must succeed");
    let swiftmarket_dispute_pda = world.dispute_pda(&swiftmarket, &actors.seller.pubkey(), &swiftmarket_receipt.order_id);
    let swiftmarket_dispute_expires_at = world.read_dispute(&swiftmarket_dispute_pda).expires_at;
    assert_eq!(swiftmarket_dispute_expires_at, swiftmarket_now + DISPUTE_EXPIRY_SECONDS);

    // 1.10: raise_dispute + resolve_dispute(upheld = false) on CashDesk,
    // signed by the ORIGINAL cashdesk_receipt_signer (not v2) and dated
    // before 1.4's rotation -- exercises raise_dispute's
    // signed_by_previous branch: an honest rotation must not void an
    // outstanding receipt.
    let cashdesk_second_order_amount = usdc(60);
    let cashdesk_second_claim = usdc(25);
    let cashdesk_second_issued_at = cashdesk_after_rotation.signer_rotated_at - 60;
    let cashdesk_second_receipt = OrderReceipt {
        domain: RECEIPT_DOMAIN,
        program_id: truststake::ID,
        chain_id: CHAIN_ID_DEVNET,
        marketplace_id: CASHDESK_ID,
        seller: actors.seller.pubkey(),
        buyer: actors.buyer.pubkey(),
        order_id: Keypair::new().pubkey().to_bytes(),
        amount: cashdesk_second_order_amount,
        issued_at: cashdesk_second_issued_at,
        expires_at: cashdesk_second_issued_at + 7 * SECONDS_PER_DAY,
    };
    assert!(cashdesk_second_receipt.issued_at < cashdesk_after_rotation.signer_rotated_at);
    world
        .raise_dispute(
            &actors.buyer,
            actors.buyer_token_account,
            actors.cashdesk,
            &cashdesk_second_receipt,
            &actors.cashdesk_receipt_signer, // the ORIGINAL signer, not v2
            cashdesk_second_claim,
        )
        .expect("raise_dispute(CashDesk, second, signed pre-rotation by the original key) must succeed");
    let stake_before_rejection = world.read_seller_stake(&actors.stake_pda);
    world
        .resolve_dispute(&actors.cashdesk_arbiter, actors.cashdesk, cashdesk_second_receipt.order_id, actors.buyer_token_account, false)
        .expect("resolve_dispute(upheld = false) must succeed");
    let cashdesk_second_dispute_pda = world.dispute_pda(&actors.cashdesk, &actors.seller.pubkey(), &cashdesk_second_receipt.order_id);
    let cashdesk_second_dispute = world.read_dispute(&cashdesk_second_dispute_pda);
    let stake_after_rejection = world.read_seller_stake(&actors.stake_pda);
    assert_eq!(
        stake_after_rejection.staked,
        stake_before_rejection.staked + cashdesk_second_dispute.bond,
        "a rejected dispute's bond must move into the seller's vault"
    );

    // 1.11: a second seller (seller_b) stakes, grants a permit to each of
    // CashDesk and PixelBazaar, then revokes both -- using a second seller
    // keeps the original seller's richer state (two permits, one slashed)
    // intact, since both release paths CLOSE the permit account.
    let seller_b_stake_amount = usdc(100);
    let seller_b_cashdesk_permit = usdc(50);
    let seller_b_pixelbazaar_permit = usdc(50);
    let seller_b_token_account = world.create_funded_token_account(world.mint, seller_b.pubkey(), seller_b_stake_amount);
    world.initialize_stake(&seller_b).expect("initialize_stake(seller_b) must succeed");
    world
        .add_stake(&seller_b, seller_b_token_account, seller_b_stake_amount)
        .expect("add_stake(seller_b) must succeed");
    world
        .grant_permit(&seller_b, actors.cashdesk, seller_b_cashdesk_permit)
        .expect("grant_permit(seller_b, CashDesk) must succeed");
    world
        .grant_permit(&seller_b, actors.pixelbazaar, seller_b_pixelbazaar_permit)
        .expect("grant_permit(seller_b, PixelBazaar) must succeed");
    world.revoke_permit(&seller_b, actors.cashdesk).expect("revoke_permit(seller_b, CashDesk) must succeed");
    world
        .revoke_permit(&seller_b, actors.pixelbazaar)
        .expect("revoke_permit(seller_b, PixelBazaar) must succeed");
    let seller_b_cashdesk_permit_pda = world.permit_pda(&seller_b.pubkey(), &actors.cashdesk);
    let seller_b_pixelbazaar_permit_pda = world.permit_pda(&seller_b.pubkey(), &actors.pixelbazaar);
    let seller_b_cashdesk_revoked_at = world.read_permit(&seller_b_cashdesk_permit_pda).revoked_at;
    let seller_b_pixelbazaar_revoked_at = world.read_permit(&seller_b_pixelbazaar_permit_pda).revoked_at;
    assert!(seller_b_cashdesk_revoked_at != i64::MAX);
    assert!(seller_b_pixelbazaar_revoked_at != i64::MAX);

    // ================= Stage 2: release_permit_early (1 hour after 1.11) =================

    let stage2_due_at = seller_b_pixelbazaar_revoked_at + CLOCK_SKEW_TOLERANCE_SECONDS;
    world.warp_seconds(stage2_due_at - 1 - world.now());
    let early_result = world.release_permit_early(&seller_b, &actors.pixelbazaar_authority, actors.pixelbazaar);
    assert_error_code(&early_result, u32::from(TrustStakeError::EarlyReleaseWaitNotElapsed));
    world.svm.expire_blockhash(); // else the identical retry below is deduped as AlreadyProcessed

    world.warp_seconds(1); // now == stage2_due_at
    let seller_b_stake_before_early_release = world.read_seller_stake(&world.stake_pda(&seller_b.pubkey()));
    let pixelbazaar_remaining = {
        let permit = world.read_permit(&seller_b_pixelbazaar_permit_pda);
        permit.max_slashable - permit.slashed
    };
    world
        .release_permit_early(&seller_b, &actors.pixelbazaar_authority, actors.pixelbazaar)
        .expect("release_permit_early(seller_b, PixelBazaar) must succeed at the due time");
    let seller_b_stake_after_early_release = world.read_seller_stake(&world.stake_pda(&seller_b.pubkey()));
    assert_eq!(
        seller_b_stake_before_early_release.committed - seller_b_stake_after_early_release.committed,
        pixelbazaar_remaining
    );
    assert!(
        world.try_read_permit(&seller_b_pixelbazaar_permit_pda).is_none(),
        "release_permit_early must close the permit account"
    );

    // ================= Stage 3: release_permit (2 days after 1.11) =================

    let stage3_due_at = seller_b_cashdesk_revoked_at + CASHDESK_COMPLAINT_WINDOW_SECONDS.max(CLOCK_SKEW_TOLERANCE_SECONDS);
    world.warp_seconds(stage3_due_at - 1 - world.now());
    let release_result = world.release_permit(&actors.admin, seller_b.pubkey(), actors.cashdesk);
    assert_error_code(&release_result, u32::from(TrustStakeError::ComplaintWindowNotElapsed));
    world.svm.expire_blockhash(); // else the identical retry below is deduped as AlreadyProcessed

    world.warp_seconds(1); // now == stage3_due_at
    let seller_b_stake_before_release = world.read_seller_stake(&world.stake_pda(&seller_b.pubkey()));
    let cashdesk_remaining = {
        let permit = world.read_permit(&seller_b_cashdesk_permit_pda);
        permit.max_slashable - permit.slashed
    };
    // Permissionless: admin calls it for seller_b, demonstrating a third
    // party can free a seller's collateral.
    world
        .release_permit(&actors.admin, seller_b.pubkey(), actors.cashdesk)
        .expect("release_permit(seller_b, CashDesk) must succeed at the due time");
    let seller_b_stake_after_release = world.read_seller_stake(&world.stake_pda(&seller_b.pubkey()));
    assert_eq!(
        seller_b_stake_before_release.committed - seller_b_stake_after_release.committed,
        cashdesk_remaining
    );
    assert!(
        world.try_read_permit(&seller_b_cashdesk_permit_pda).is_none(),
        "release_permit must close the permit account"
    );

    // ================= Stage 4: expire_dispute, close_dispute (30 days) =================
    //
    // Three due-times converge near the same 30-day mark, but are not all
    // equal: cashdesk_second_closable_after lands 60 seconds BEFORE the
    // other two, because its receipt was deliberately backdated 60 seconds
    // in 1.10 (to predate CashDesk's signer rotation) while
    // swiftmarket_dispute_expires_at and original_closable_after both
    // derive from a receipt/dispute stamped at the very same frozen
    // LiteSVM clock value (nothing warps the clock between the original
    // Step 7 and the end of stage 1, so `world.now()` reads identically
    // throughout). Testing them in ascending order of due-time -- closing
    // the CashDesk second dispute first, then handling expire_dispute and
    // the original close together, since those two are exactly tied -- is
    // what keeps every negative assertion below meaningful: warping past a
    // later threshold first would silently skip the earlier one's
    // before-due check instead of proving it.

    let stranger = Keypair::new();
    world.svm.airdrop(&stranger.pubkey(), SOL).expect("airdrop stranger");

    // 4.2a: close_dispute on the rejected CashDesk dispute from 1.10 --
    // the earliest of the three thresholds. Fails before its
    // closable_after, succeeds at it. Permissionless.
    let cashdesk_second_closable_after = world.read_dispute(&cashdesk_second_dispute_pda).closable_after;
    assert!(cashdesk_second_closable_after < swiftmarket_dispute_expires_at);
    world.warp_seconds(cashdesk_second_closable_after - 1 - world.now());
    let close_early_result = world.close_dispute(&stranger, actors.cashdesk, cashdesk_second_receipt.order_id);
    assert_error_code(&close_early_result, u32::from(TrustStakeError::DisputeNotClosable));
    world.svm.expire_blockhash(); // else the identical retry below is deduped as AlreadyProcessed

    world.warp_seconds(1); // now == cashdesk_second_closable_after
    let buyer_balance_before_close = world.svm.get_account(&actors.buyer.pubkey()).expect("buyer exists").lamports;
    world
        .close_dispute(&stranger, actors.cashdesk, cashdesk_second_receipt.order_id)
        .expect("close_dispute(CashDesk, second dispute) must succeed at closable_after");
    assert!(world.try_read_dispute(&cashdesk_second_dispute_pda).is_none(), "close_dispute must delete the record");
    let buyer_balance_after_close = world.svm.get_account(&actors.buyer.pubkey()).expect("buyer exists").lamports;
    assert!(buyer_balance_after_close > buyer_balance_before_close, "close_dispute must refund rent to the buyer");

    // 4.1 + 4.2b: expire_dispute on the SwiftMarket dispute from 1.9, and
    // close_dispute on the ORIGINAL upheld dispute from Step 7 -- tied at
    // the same due-time, so both are asserted to fail together one second
    // before it and succeed together at it.
    let original_dispute_pda = world.dispute_pda(&actors.cashdesk, &actors.seller.pubkey(), &original_upheld_order_id);
    let original_closable_after = world.read_dispute(&original_dispute_pda).closable_after;
    assert_eq!(original_closable_after, swiftmarket_dispute_expires_at);

    world.warp_seconds(swiftmarket_dispute_expires_at - 1 - world.now());
    let expire_early_result = world.expire_dispute(&stranger, swiftmarket, swiftmarket_receipt.order_id, buyer_swiftmarket_token_account);
    assert_error_code(&expire_early_result, u32::from(TrustStakeError::DisputeNotExpired));
    world.svm.expire_blockhash();
    let close_original_early_result = world.close_dispute(&stranger, actors.cashdesk, original_upheld_order_id);
    assert_error_code(&close_original_early_result, u32::from(TrustStakeError::DisputeNotClosable));
    world.svm.expire_blockhash();

    world.warp_seconds(1); // now == swiftmarket_dispute_expires_at == original_closable_after
    let swiftmarket_before_expiry = world.read_marketplace(&swiftmarket);
    let swiftmarket_permit_before_expiry = world.read_permit(&swiftmarket_permit_pda);
    world
        .expire_dispute(&stranger, swiftmarket, swiftmarket_receipt.order_id, buyer_swiftmarket_token_account)
        .expect("expire_dispute(SwiftMarket) must succeed at expiry");
    let swiftmarket_dispute_after_expiry = world.read_dispute(&swiftmarket_dispute_pda);
    let swiftmarket_after_expiry = world.read_marketplace(&swiftmarket);
    let swiftmarket_permit_after_expiry = world.read_permit(&swiftmarket_permit_pda);
    assert_eq!(swiftmarket_dispute_after_expiry.status, DisputeStatus::Abandoned as u8);
    assert_eq!(swiftmarket_after_expiry.disputes_abandoned, swiftmarket_before_expiry.disputes_abandoned + 1);
    assert_eq!(swiftmarket_permit_after_expiry.open_disputes, swiftmarket_permit_before_expiry.open_disputes - 1);
    assert_eq!(swiftmarket_permit_after_expiry.open_disputes, 0, "SwiftMarket's permit had exactly one dispute; open_disputes must fall to zero");

    let buyer_balance_before_second_close = world.svm.get_account(&actors.buyer.pubkey()).expect("buyer exists").lamports;
    world
        .close_dispute(&stranger, actors.cashdesk, original_upheld_order_id)
        .expect("close_dispute(CashDesk, original dispute) must succeed at closable_after");
    assert!(world.try_read_dispute(&original_dispute_pda).is_none(), "close_dispute must delete the record");
    let buyer_balance_after_second_close = world.svm.get_account(&actors.buyer.pubkey()).expect("buyer exists").lamports;
    assert!(
        buyer_balance_after_second_close > buyer_balance_before_second_close,
        "close_dispute must refund rent to the buyer"
    );
}
