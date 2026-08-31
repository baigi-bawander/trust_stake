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

#[test]
fn test_devnet_demo_sequence_parity() {
    // usdc() is a plain fn (not const fn), so these are computed here
    // rather than as module-level consts.
    let stake_amount: u64 = usdc(500);
    let cashdesk_permit: u64 = usdc(200);
    let pixelbazaar_permit: u64 = usdc(200);
    let withdraw_amount: u64 = usdc(100);
    let withdraw_attempt_too_much: u64 = usdc(50);
    let order_amount: u64 = usdc(80);
    let claim_amount: u64 = order_amount;

    let mut world = World::new();
    let admin = initial_admin_keypair();

    let seller = Keypair::new();
    let buyer = Keypair::new();
    let cashdesk_authority = Keypair::new();
    let cashdesk_receipt_signer = Keypair::new();
    let cashdesk_arbiter = Keypair::new();
    let pixelbazaar_authority = Keypair::new();
    let pixelbazaar_receipt_signer = Keypair::new();
    let pixelbazaar_arbiter = Keypair::new();

    // Lamports for rent and fees, mirroring devnet_demo.rs's direct-transfer
    // funding of the same wallets. Neither receipt signer nor
    // pixelbazaar_arbiter is funded: neither one ever pays for anything.
    // cashdesk_arbiter IS funded here, unlike in devnet_demo.rs: World's
    // resolve_dispute (common/mod.rs) always uses the arbiter itself as the
    // transaction's fee payer, where the real demo instead makes `admin`
    // the payer and cashdesk_arbiter only a co-signer. That is a fee-payer
    // detail of the two callers, not a difference in what the program sees:
    // resolve_dispute's own accounts and arguments are identical either way.
    const SOL: u64 = 1_000_000_000;
    world.svm.airdrop(&seller.pubkey(), SOL).expect("airdrop seller");
    world.svm.airdrop(&buyer.pubkey(), SOL).expect("airdrop buyer");
    world
        .svm
        .airdrop(&cashdesk_authority.pubkey(), SOL)
        .expect("airdrop cashdesk authority");
    world
        .svm
        .airdrop(&pixelbazaar_authority.pubkey(), SOL)
        .expect("airdrop pixelbazaar authority");
    world
        .svm
        .airdrop(&cashdesk_arbiter.pubkey(), SOL)
        .expect("airdrop cashdesk arbiter");

    // Step 1 (devnet_demo.rs): test mint and starting balances.
    // World::new() already created the mint (6 decimals, matching real
    // USDC), the demo's own "create a test mint" step boiled down to a
    // fixture here.
    let seller_token_account = world.create_funded_token_account(world.mint, seller.pubkey(), stake_amount);
    let buyer_token_account = world.create_funded_token_account(world.mint, buyer.pubkey(), usdc(10));

    // Step 2: initialize_config(chain_id = CHAIN_ID_DEVNET).
    world.initialize_config(&admin, CHAIN_ID_DEVNET).expect("initialize_config must succeed");

    // Step 3: register two marketplaces with different IDs, receipt
    // signers, and bond_bps, sharing the same seller's future collateral.
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

    // Step 4: seller locks 500 of collateral, once.
    world.initialize_stake(&seller).expect("initialize_stake must succeed");
    world
        .add_stake(&seller, seller_token_account, stake_amount)
        .expect("add_stake must succeed");

    // Step 5: grant a permit to each marketplace, from the same stake.
    world
        .grant_permit(&seller, cashdesk, cashdesk_permit)
        .expect("grant_permit(CashDesk) must succeed");
    world
        .grant_permit(&seller, pixelbazaar, pixelbazaar_permit)
        .expect("grant_permit(PixelBazaar) must succeed");

    let stake_pda = world.stake_pda(&seller.pubkey());
    let stake_after_grants = world.read_seller_stake(&stake_pda);
    assert_eq!(stake_after_grants.staked, stake_amount);
    assert_eq!(stake_after_grants.committed, cashdesk_permit + pixelbazaar_permit);

    // Step 6: withdraw the free 100, then hit the cap on a second attempt.
    world
        .withdraw_stake(&seller, seller_token_account, withdraw_amount)
        .expect("withdraw_stake(100) must succeed: 500 - 100 >= 400 committed");
    let result = world.withdraw_stake(&seller, seller_token_account, withdraw_attempt_too_much);
    assert_error_code(&result, u32::from(TrustStakeError::CommittedExceedsStaked));

    // Step 7: a CashDesk buyer disputes an 80 order; [Ed25519 verify,
    // raise_dispute] as one transaction; CashDesk's arbiter upholds it.
    let now = world.now();
    let order_id = Keypair::new().pubkey().to_bytes();
    let receipt = OrderReceipt {
        domain: RECEIPT_DOMAIN,
        program_id: truststake::ID,
        chain_id: CHAIN_ID_DEVNET,
        marketplace_id: CASHDESK_ID,
        seller: seller.pubkey(),
        buyer: buyer.pubkey(),
        order_id,
        amount: order_amount,
        issued_at: now,
        expires_at: now + 7 * SECONDS_PER_DAY,
    };
    world
        .raise_dispute(&buyer, buyer_token_account, cashdesk, &receipt, &cashdesk_receipt_signer, claim_amount)
        .expect("raise_dispute must succeed");
    world
        .resolve_dispute(&cashdesk_arbiter, cashdesk, order_id, buyer_token_account, true)
        .expect("resolve_dispute(upheld = true) must succeed");

    // Step 8: CashDesk's permit is visibly slashed; PixelBazaar's is
    // completely untouched, even though both draw on the same stake.
    let cashdesk_permit_pda = world.permit_pda(&seller.pubkey(), &cashdesk);
    let pixelbazaar_permit_pda = world.permit_pda(&seller.pubkey(), &pixelbazaar);
    let cashdesk_permit_state = world.read_permit(&cashdesk_permit_pda);
    let pixelbazaar_permit_state = world.read_permit(&pixelbazaar_permit_pda);

    assert_eq!(cashdesk_permit_state.slashed, claim_amount);
    assert_eq!(cashdesk_permit_state.max_slashable, cashdesk_permit);
    assert_eq!(pixelbazaar_permit_state.slashed, 0);
    assert_eq!(pixelbazaar_permit_state.max_slashable, pixelbazaar_permit);

    let stake_final = world.read_seller_stake(&stake_pda);
    assert_eq!(stake_final.staked, stake_amount - withdraw_amount - claim_amount);
    assert_eq!(stake_final.committed, cashdesk_permit + pixelbazaar_permit - claim_amount);

    // The original 8-step walk stops here (see docs/TESTING.md). Stages 1-4
    // pick up from this exact state.
    let original_upheld_order_id = order_id;

    // ================= Stage 1: immediate steps (1.1 - 1.11) =================

    let governance = Keypair::new();
    let swiftmarket_authority = Keypair::new();
    let swiftmarket_authority_v2 = Keypair::new();
    let swiftmarket_receipt_signer = Keypair::new();
    let swiftmarket_arbiter = Keypair::new();
    let cashdesk_receipt_signer_v2 = Keypair::new();
    let buyer_swiftmarket = Keypair::new();
    let seller_b = Keypair::new();

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
        .propose_config_authority(&admin, governance.pubkey())
        .expect("propose_config_authority(admin -> governance) must succeed");
    world
        .accept_config_authority(&governance)
        .expect("accept_config_authority(governance) must succeed");
    assert_eq!(world.read_config().authority, governance.pubkey());
    world
        .propose_config_authority(&governance, admin.pubkey())
        .expect("propose_config_authority(governance -> admin) must succeed");
    world
        .accept_config_authority(&admin)
        .expect("accept_config_authority(admin) must succeed");
    let config_after_roundtrip = world.read_config();
    assert_eq!(config_after_roundtrip.authority, admin.pubkey());
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
        .update_marketplace(&pixelbazaar_authority, pixelbazaar, None, None, None, Some(PIXELBAZAAR_NEW_BOND_BPS))
        .expect("update_marketplace(PixelBazaar) must succeed");
    assert_eq!(world.read_marketplace(&pixelbazaar).bond_bps, PIXELBAZAAR_NEW_BOND_BPS);
    assert_eq!(
        world.read_permit(&pixelbazaar_permit_pda).bond_bps,
        PIXELBAZAAR_BOND_BPS,
        "PixelBazaar's existing permit must keep its frozen bond_bps, not follow the marketplace's live rate"
    );

    // 1.4: update_marketplace(CashDesk, new_receipt_signer). An honest
    // rotation must record the old signer and a non-zero rotation
    // timestamp, not silently void outstanding receipts.
    world
        .update_marketplace(&cashdesk_authority, cashdesk, Some(cashdesk_receipt_signer_v2.pubkey()), None, None, None)
        .expect("update_marketplace(CashDesk, new_receipt_signer) must succeed");
    let cashdesk_after_rotation = world.read_marketplace(&cashdesk);
    assert_eq!(cashdesk_after_rotation.receipt_signer, cashdesk_receipt_signer_v2.pubkey());
    assert_eq!(cashdesk_after_rotation.prev_receipt_signer, cashdesk_receipt_signer.pubkey());
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
    // against SwiftMarket. Free collateral is 0 here (500 staked, 400
    // committed - 80 slashed = 320 committed), matching devnet's real state.
    let stake_before_topup = world.read_seller_stake(&stake_pda);
    let free = stake_before_topup.staked.saturating_sub(stake_before_topup.committed);
    let topup_target = usdc(200);
    assert!(free < topup_target, "this test's fixture is expected to start with zero free collateral");
    let topup_amount = topup_target - free;
    world.mint_to(&seller_token_account, topup_amount);
    world
        .add_stake(&seller, seller_token_account, topup_amount)
        .expect("add_stake (1.6 top-up) must succeed");

    // 1.7 + 1.8: grant_permit(SwiftMarket, 100), then
    // increase_permit(SwiftMarket, +50). granted_at must survive the
    // increase unchanged: an increase modifies the permit's existing era,
    // it does not start a new one.
    let swiftmarket_grant = usdc(100);
    let swiftmarket_increase = usdc(50);
    world
        .grant_permit(&seller, swiftmarket, swiftmarket_grant)
        .expect("grant_permit(SwiftMarket) must succeed");
    let swiftmarket_permit_pda = world.permit_pda(&seller.pubkey(), &swiftmarket);
    let granted_at_before_increase = world.read_permit(&swiftmarket_permit_pda).granted_at;
    world
        .increase_permit(&seller, swiftmarket, swiftmarket_increase)
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
        seller: seller.pubkey(),
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
    let swiftmarket_dispute_pda = world.dispute_pda(&swiftmarket, &seller.pubkey(), &swiftmarket_receipt.order_id);
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
        seller: seller.pubkey(),
        buyer: buyer.pubkey(),
        order_id: Keypair::new().pubkey().to_bytes(),
        amount: cashdesk_second_order_amount,
        issued_at: cashdesk_second_issued_at,
        expires_at: cashdesk_second_issued_at + 7 * SECONDS_PER_DAY,
    };
    assert!(cashdesk_second_receipt.issued_at < cashdesk_after_rotation.signer_rotated_at);
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            cashdesk,
            &cashdesk_second_receipt,
            &cashdesk_receipt_signer, // the ORIGINAL signer, not v2
            cashdesk_second_claim,
        )
        .expect("raise_dispute(CashDesk, second, signed pre-rotation by the original key) must succeed");
    let stake_before_rejection = world.read_seller_stake(&stake_pda);
    world
        .resolve_dispute(&cashdesk_arbiter, cashdesk, cashdesk_second_receipt.order_id, buyer_token_account, false)
        .expect("resolve_dispute(upheld = false) must succeed");
    let cashdesk_second_dispute_pda = world.dispute_pda(&cashdesk, &seller.pubkey(), &cashdesk_second_receipt.order_id);
    let cashdesk_second_dispute = world.read_dispute(&cashdesk_second_dispute_pda);
    let stake_after_rejection = world.read_seller_stake(&stake_pda);
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
        .grant_permit(&seller_b, cashdesk, seller_b_cashdesk_permit)
        .expect("grant_permit(seller_b, CashDesk) must succeed");
    world
        .grant_permit(&seller_b, pixelbazaar, seller_b_pixelbazaar_permit)
        .expect("grant_permit(seller_b, PixelBazaar) must succeed");
    world.revoke_permit(&seller_b, cashdesk).expect("revoke_permit(seller_b, CashDesk) must succeed");
    world
        .revoke_permit(&seller_b, pixelbazaar)
        .expect("revoke_permit(seller_b, PixelBazaar) must succeed");
    let seller_b_cashdesk_permit_pda = world.permit_pda(&seller_b.pubkey(), &cashdesk);
    let seller_b_pixelbazaar_permit_pda = world.permit_pda(&seller_b.pubkey(), &pixelbazaar);
    let seller_b_cashdesk_revoked_at = world.read_permit(&seller_b_cashdesk_permit_pda).revoked_at;
    let seller_b_pixelbazaar_revoked_at = world.read_permit(&seller_b_pixelbazaar_permit_pda).revoked_at;
    assert!(seller_b_cashdesk_revoked_at != i64::MAX);
    assert!(seller_b_pixelbazaar_revoked_at != i64::MAX);

    // ================= Stage 2: release_permit_early (1 hour after 1.11) =================

    let stage2_due_at = seller_b_pixelbazaar_revoked_at + CLOCK_SKEW_TOLERANCE_SECONDS;
    world.warp_seconds(stage2_due_at - 1 - world.now());
    let early_result = world.release_permit_early(&seller_b, &pixelbazaar_authority, pixelbazaar);
    assert_error_code(&early_result, u32::from(TrustStakeError::EarlyReleaseWaitNotElapsed));
    world.svm.expire_blockhash(); // else the identical retry below is deduped as AlreadyProcessed

    world.warp_seconds(1); // now == stage2_due_at
    let seller_b_stake_before_early_release = world.read_seller_stake(&world.stake_pda(&seller_b.pubkey()));
    let pixelbazaar_remaining = {
        let permit = world.read_permit(&seller_b_pixelbazaar_permit_pda);
        permit.max_slashable - permit.slashed
    };
    world
        .release_permit_early(&seller_b, &pixelbazaar_authority, pixelbazaar)
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
    let release_result = world.release_permit(&admin, seller_b.pubkey(), cashdesk);
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
        .release_permit(&admin, seller_b.pubkey(), cashdesk)
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
    let close_early_result = world.close_dispute(&stranger, cashdesk, cashdesk_second_receipt.order_id);
    assert_error_code(&close_early_result, u32::from(TrustStakeError::DisputeNotClosable));
    world.svm.expire_blockhash(); // else the identical retry below is deduped as AlreadyProcessed

    world.warp_seconds(1); // now == cashdesk_second_closable_after
    let buyer_balance_before_close = world.svm.get_account(&buyer.pubkey()).expect("buyer exists").lamports;
    world
        .close_dispute(&stranger, cashdesk, cashdesk_second_receipt.order_id)
        .expect("close_dispute(CashDesk, second dispute) must succeed at closable_after");
    assert!(world.try_read_dispute(&cashdesk_second_dispute_pda).is_none(), "close_dispute must delete the record");
    let buyer_balance_after_close = world.svm.get_account(&buyer.pubkey()).expect("buyer exists").lamports;
    assert!(buyer_balance_after_close > buyer_balance_before_close, "close_dispute must refund rent to the buyer");

    // 4.1 + 4.2b: expire_dispute on the SwiftMarket dispute from 1.9, and
    // close_dispute on the ORIGINAL upheld dispute from Step 7 -- tied at
    // the same due-time, so both are asserted to fail together one second
    // before it and succeed together at it.
    let original_dispute_pda = world.dispute_pda(&cashdesk, &seller.pubkey(), &original_upheld_order_id);
    let original_closable_after = world.read_dispute(&original_dispute_pda).closable_after;
    assert_eq!(original_closable_after, swiftmarket_dispute_expires_at);

    world.warp_seconds(swiftmarket_dispute_expires_at - 1 - world.now());
    let expire_early_result = world.expire_dispute(&stranger, swiftmarket, swiftmarket_receipt.order_id, buyer_swiftmarket_token_account);
    assert_error_code(&expire_early_result, u32::from(TrustStakeError::DisputeNotExpired));
    world.svm.expire_blockhash();
    let close_original_early_result = world.close_dispute(&stranger, cashdesk, original_upheld_order_id);
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

    let buyer_balance_before_second_close = world.svm.get_account(&buyer.pubkey()).expect("buyer exists").lamports;
    world
        .close_dispute(&stranger, cashdesk, original_upheld_order_id)
        .expect("close_dispute(CashDesk, original dispute) must succeed at closable_after");
    assert!(world.try_read_dispute(&original_dispute_pda).is_none(), "close_dispute must delete the record");
    let buyer_balance_after_second_close = world.svm.get_account(&buyer.pubkey()).expect("buyer exists").lamports;
    assert!(
        buyer_balance_after_second_close > buyer_balance_before_second_close,
        "close_dispute must refund rent to the buyer"
    );
}
