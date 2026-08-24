//! Replays examples/devnet_demo.rs's instruction sequence against LiteSVM,
//! same handlers, same order, same amounts, so a broken walk is caught here
//! for free instead of by Step 3 spending real devnet SOL on it. Read
//! examples/devnet_demo.rs alongside this file, and change both together:
//! the demo hand-builds each `Instruction` for a real RPC client, while
//! this test reaches the same instructions through the `World` harness's
//! thinner wrappers (common/mod.rs), so nothing here is shared code that
//! would hide a drift between the two.
mod common;

use common::{assert_error_code, initial_admin_keypair, usdc, World};
use solana_keypair::Keypair;
use solana_signer::Signer;
use truststake::{
    constants::{CHAIN_ID_DEVNET, RECEIPT_DOMAIN, SECONDS_PER_DAY},
    error::TrustStakeError,
    receipt::OrderReceipt,
};

/// CashDesk: 2-day complaint window (the protocol floor), 10% bond. The
/// buyer's dispute in this test lands against CashDesk.
const CASHDESK_ID: [u8; 16] = *b"cashdesk-demo-01";
const CASHDESK_COMPLAINT_WINDOW_SECONDS: i64 = 2 * SECONDS_PER_DAY;
const CASHDESK_BOND_BPS: u16 = 1_000;

/// PixelBazaar: 7-day window, 5% bond, visibly distinct from CashDesk's.
/// Never disputed in this test -- its permit's untouched final state is
/// the whole point of step 8.
const PIXELBAZAAR_ID: [u8; 16] = *b"pixelbazaar-demo";
const PIXELBAZAAR_COMPLAINT_WINDOW_SECONDS: i64 = 7 * SECONDS_PER_DAY;
const PIXELBAZAAR_BOND_BPS: u16 = 500;

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

    // The walk stops here in both the demo and this test: release_permit
    // and release_permit_early are time-gated (see devnet_demo.rs's closing
    // note) and are exercised in tests/test_phase2.rs instead.
}
