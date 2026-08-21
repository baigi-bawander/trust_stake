//! Phase 1 test suite (docs/TESTING.md): every test here belongs to
//! Phase 1 because the last handler it calls is one of the nine Phase 1
//! handlers. See the end-of-task report for the mapping against
//! docs/TESTING.md's named tests.

mod common;

use anchor_lang::{prelude::*, solana_program::program_pack::Pack, InstructionData, ToAccountMetas};
use anchor_spl::{token::spl_token, token_2022::spl_token_2022};
use common::{assert_error_code, event_cpi_accounts, initial_admin_keypair, usdc, World};
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_system_interface::error::SystemError;
use truststake::{
    constants::{MAX_BOND_BPS, MAX_COMPLAINT_WINDOW_SECONDS, MIN_COMPLAINT_WINDOW_SECONDS},
    error::TrustStakeError,
};

const SOL: u64 = 1_000_000_000;
const DEFAULT_CHAIN_ID: u8 = 1;
const DEFAULT_BOND_BPS: u16 = 1_000;

fn marketplace_id(tag: u8) -> [u8; 16] {
    [tag; 16]
}

fn unique_pubkey() -> Pubkey {
    Keypair::new().pubkey()
}

fn funded_keypair(world: &mut World) -> Keypair {
    let keypair = Keypair::new();
    world.svm.airdrop(&keypair.pubkey(), 10 * SOL).expect("airdrop");
    keypair
}

/// Registers a marketplace with unremarkable, valid settings and returns
/// its authority keypair and address. Shared setup for tests whose focus
/// is somewhere else entirely.
fn setup_marketplace(world: &mut World, tag: u8) -> (Keypair, Pubkey) {
    let authority = funded_keypair(world);
    let id = marketplace_id(tag);
    world
        .register_marketplace(
            &authority,
            id,
            unique_pubkey(),
            unique_pubkey(),
            MIN_COMPLAINT_WINDOW_SECONDS,
            DEFAULT_BOND_BPS,
        )
        .expect("register_marketplace succeeds");
    (authority, world.marketplace_pda(&id))
}

// ---------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------

#[test]
fn test_initialize_config_succeeds() {
    let mut world = World::new();
    let admin = initial_admin_keypair();

    let result = world.initialize_config(&admin, DEFAULT_CHAIN_ID);
    assert!(result.is_ok(), "initialize_config should succeed: {result:?}");

    let config = world.read_config();
    assert_eq!(config.version, 1);
    assert_eq!(config.authority, admin.pubkey());
    assert_eq!(config.pending_authority, Pubkey::default());
    assert_eq!(config.collateral_mint, world.mint);
    assert_eq!(config.chain_id, DEFAULT_CHAIN_ID);
}

#[test]
fn test_initialize_config_cannot_be_frontrun() {
    let mut world = World::new();
    let impostor = funded_keypair(&mut world);

    let result = world.initialize_config(&impostor, DEFAULT_CHAIN_ID);
    assert_error_code(&result, u32::from(TrustStakeError::NotInitialAdmin));
    assert!(
        world.svm.get_account(&world.config_pda()).is_none(),
        "config must not have been created"
    );
}

#[test]
fn test_initialize_config_rejects_double_init() {
    let mut world = World::new();
    let admin = initial_admin_keypair();

    world
        .initialize_config(&admin, DEFAULT_CHAIN_ID)
        .expect("first initialize_config succeeds");

    // Same signer, same arguments: without a fresh blockhash the second
    // transaction is byte-identical to the first and LiteSVM rejects it
    // as already processed before the program ever runs, which would
    // hide the real "account already in use" check this test is for.
    world.svm.expire_blockhash();
    let result = world.initialize_config(&admin, DEFAULT_CHAIN_ID);
    assert_error_code(&result, SystemError::AccountAlreadyInUse as u32);
}

#[test]
fn test_account_type_substitution_rejected() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();
    let (_marketplace_authority, marketplace) = setup_marketplace(&mut world, 1);

    // A real, correctly-owned Marketplace account passed where a Config
    // is expected must fail on the discriminator, before seeds are even
    // considered.
    let (event_authority, program) = event_cpi_accounts(&world.program_id);
    let instruction = Instruction::new_with_bytes(
        world.program_id,
        &truststake::instruction::ProposeConfigAuthority {
            new_authority: unique_pubkey(),
        }
        .data(),
        truststake::accounts::ProposeConfigAuthorityAccountConstraints {
            authority: admin.pubkey(),
            config: marketplace,
            event_authority,
            program,
        }
        .to_account_metas(None),
    );

    let result = world.send_instructions(&[instruction], &admin.pubkey(), &[&admin]);
    assert_error_code(
        &result,
        u32::from(anchor_lang::error::ErrorCode::AccountDiscriminatorMismatch),
    );
}

#[test]
fn test_config_authority_transfer_succeeds() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();

    let new_authority = funded_keypair(&mut world);

    world
        .propose_config_authority(&admin, new_authority.pubkey())
        .expect("propose_config_authority succeeds");
    assert_eq!(world.read_config().pending_authority, new_authority.pubkey());

    world
        .accept_config_authority(&new_authority)
        .expect("accept_config_authority succeeds");
    let config = world.read_config();
    assert_eq!(config.authority, new_authority.pubkey());
    assert_eq!(config.pending_authority, Pubkey::default());
}

// ---------------------------------------------------------------------
// Marketplace
// ---------------------------------------------------------------------

#[test]
fn test_register_marketplace_succeeds() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();

    let authority = funded_keypair(&mut world);
    let id = marketplace_id(10);
    let receipt_signer = unique_pubkey();
    let arbiter = unique_pubkey();

    world
        .register_marketplace(
            &authority,
            id,
            receipt_signer,
            arbiter,
            MIN_COMPLAINT_WINDOW_SECONDS,
            DEFAULT_BOND_BPS,
        )
        .expect("register_marketplace succeeds");

    let marketplace_pubkey = world.marketplace_pda(&id);
    let marketplace = world.read_marketplace(&marketplace_pubkey);
    assert_eq!(marketplace.version, 1);
    assert_eq!(marketplace.marketplace_id, id);
    assert_eq!(marketplace.authority, authority.pubkey());
    assert_eq!(marketplace.pending_authority, Pubkey::default());
    assert_eq!(marketplace.receipt_signer, receipt_signer);
    assert_eq!(marketplace.prev_receipt_signer, Pubkey::default());
    assert_eq!(marketplace.signer_rotated_at, 0);
    assert_eq!(marketplace.arbiter, arbiter);
    assert_eq!(marketplace.complaint_window, MIN_COMPLAINT_WINDOW_SECONDS);
    assert_eq!(marketplace.bond_bps, DEFAULT_BOND_BPS);
    assert_eq!(marketplace.disputes_total, 0);
    assert_eq!(marketplace.disputes_upheld, 0);
    assert_eq!(marketplace.disputes_abandoned, 0);
    assert_eq!(marketplace.total_slashed, 0);

    let bond_vault = world.read_token_account(&world.bond_vault_pda(&marketplace_pubkey));
    assert_eq!(bond_vault.amount, 0);
    assert_eq!(bond_vault.mint, world.mint);
}

#[test]
fn test_register_marketplace_rejects_duplicate_id() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();

    let id = marketplace_id(11);
    let (_authority, _marketplace) = setup_marketplace_with_id(&mut world, id);

    let second_authority = funded_keypair(&mut world);
    let result = world.register_marketplace(
        &second_authority,
        id,
        unique_pubkey(),
        unique_pubkey(),
        MIN_COMPLAINT_WINDOW_SECONDS,
        DEFAULT_BOND_BPS,
    );
    assert_error_code(&result, SystemError::AccountAlreadyInUse as u32);
}

fn setup_marketplace_with_id(world: &mut World, id: [u8; 16]) -> (Keypair, Pubkey) {
    let authority = funded_keypair(world);
    world
        .register_marketplace(
            &authority,
            id,
            unique_pubkey(),
            unique_pubkey(),
            MIN_COMPLAINT_WINDOW_SECONDS,
            DEFAULT_BOND_BPS,
        )
        .expect("register_marketplace succeeds");
    (authority, world.marketplace_pda(&id))
}

#[test]
fn test_marketplace_window_bounds_enforced() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();
    let authority = funded_keypair(&mut world);

    let result = world.register_marketplace(
        &authority,
        marketplace_id(20),
        unique_pubkey(),
        unique_pubkey(),
        MIN_COMPLAINT_WINDOW_SECONDS - 1,
        DEFAULT_BOND_BPS,
    );
    assert_error_code(&result, u32::from(TrustStakeError::ComplaintWindowOutOfBounds));

    world
        .register_marketplace(
            &authority,
            marketplace_id(21),
            unique_pubkey(),
            unique_pubkey(),
            MIN_COMPLAINT_WINDOW_SECONDS,
            DEFAULT_BOND_BPS,
        )
        .expect("floor window succeeds");

    world
        .register_marketplace(
            &authority,
            marketplace_id(22),
            unique_pubkey(),
            unique_pubkey(),
            MAX_COMPLAINT_WINDOW_SECONDS,
            DEFAULT_BOND_BPS,
        )
        .expect("ceiling window succeeds");

    let result = world.register_marketplace(
        &authority,
        marketplace_id(23),
        unique_pubkey(),
        unique_pubkey(),
        MAX_COMPLAINT_WINDOW_SECONDS + 1,
        DEFAULT_BOND_BPS,
    );
    assert_error_code(&result, u32::from(TrustStakeError::ComplaintWindowOutOfBounds));

    // validate_marketplace_settings is shared: update_marketplace enforces
    // the same bounds as register_marketplace.
    let marketplace = world.marketplace_pda(&marketplace_id(21));
    let result = world.update_marketplace(
        &authority,
        marketplace,
        None,
        None,
        Some(MAX_COMPLAINT_WINDOW_SECONDS + 1),
        None,
    );
    assert_error_code(&result, u32::from(TrustStakeError::ComplaintWindowOutOfBounds));
}

#[test]
fn test_marketplace_bond_ceiling_enforced() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();
    let authority = funded_keypair(&mut world);

    world
        .register_marketplace(
            &authority,
            marketplace_id(30),
            unique_pubkey(),
            unique_pubkey(),
            MIN_COMPLAINT_WINDOW_SECONDS,
            MAX_BOND_BPS,
        )
        .expect("ceiling bond_bps succeeds");

    let result = world.register_marketplace(
        &authority,
        marketplace_id(31),
        unique_pubkey(),
        unique_pubkey(),
        MIN_COMPLAINT_WINDOW_SECONDS,
        MAX_BOND_BPS + 1,
    );
    assert_error_code(&result, u32::from(TrustStakeError::BondBpsTooHigh));

    let marketplace = world.marketplace_pda(&marketplace_id(30));
    let result = world.update_marketplace(&authority, marketplace, None, None, None, Some(MAX_BOND_BPS + 1));
    assert_error_code(&result, u32::from(TrustStakeError::BondBpsTooHigh));
}

#[test]
fn test_update_marketplace_succeeds() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();

    let authority = funded_keypair(&mut world);
    let id = marketplace_id(40);
    let original_receipt_signer = unique_pubkey();
    world
        .register_marketplace(
            &authority,
            id,
            original_receipt_signer,
            unique_pubkey(),
            MIN_COMPLAINT_WINDOW_SECONDS,
            DEFAULT_BOND_BPS,
        )
        .unwrap();
    let marketplace = world.marketplace_pda(&id);

    let new_receipt_signer = unique_pubkey();
    let new_arbiter = unique_pubkey();
    world
        .update_marketplace(
            &authority,
            marketplace,
            Some(new_receipt_signer),
            Some(new_arbiter),
            Some(MAX_COMPLAINT_WINDOW_SECONDS),
            Some(MAX_BOND_BPS),
        )
        .expect("update_marketplace succeeds");

    let updated = world.read_marketplace(&marketplace);
    assert_eq!(updated.receipt_signer, new_receipt_signer);
    assert_eq!(updated.prev_receipt_signer, original_receipt_signer);
    assert!(updated.signer_rotated_at > 0);
    assert_eq!(updated.arbiter, new_arbiter);
    assert_eq!(updated.complaint_window, MAX_COMPLAINT_WINDOW_SECONDS);
    assert_eq!(updated.bond_bps, MAX_BOND_BPS);
}

#[test]
fn test_update_marketplace_requires_authority() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();
    let (_authority, marketplace) = setup_marketplace(&mut world, 41);

    let impostor = funded_keypair(&mut world);
    let result = world.update_marketplace(&impostor, marketplace, Some(unique_pubkey()), None, None, None);
    assert_error_code(&result, u32::from(anchor_lang::error::ErrorCode::ConstraintHasOne));
}

#[test]
fn test_update_marketplace_signer_rotation_bookkeeping() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();

    let authority = funded_keypair(&mut world);
    let id = marketplace_id(42);
    let receipt_signer = unique_pubkey();
    world
        .register_marketplace(
            &authority,
            id,
            receipt_signer,
            unique_pubkey(),
            MIN_COMPLAINT_WINDOW_SECONDS,
            DEFAULT_BOND_BPS,
        )
        .unwrap();
    let marketplace = world.marketplace_pda(&id);

    // Updating to the SAME signer must not stamp a rotation.
    world
        .update_marketplace(&authority, marketplace, Some(receipt_signer), None, None, None)
        .expect("no-op signer update succeeds");
    let after_noop = world.read_marketplace(&marketplace);
    assert_eq!(after_noop.prev_receipt_signer, Pubkey::default());
    assert_eq!(after_noop.signer_rotated_at, 0);

    // Updating to a DIFFERENT signer must stamp both fields.
    let rotated_signer = unique_pubkey();
    world
        .update_marketplace(&authority, marketplace, Some(rotated_signer), None, None, None)
        .expect("rotation update succeeds");
    let after_rotation = world.read_marketplace(&marketplace);
    assert_eq!(after_rotation.receipt_signer, rotated_signer);
    assert_eq!(after_rotation.prev_receipt_signer, receipt_signer);
    assert!(after_rotation.signer_rotated_at > 0);
}

#[test]
fn test_marketplace_authority_transfer_succeeds() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();
    let (authority, marketplace) = setup_marketplace(&mut world, 50);

    let new_authority = funded_keypair(&mut world);

    world
        .propose_marketplace_authority(&authority, marketplace, new_authority.pubkey())
        .expect("propose_marketplace_authority succeeds");
    assert_eq!(
        world.read_marketplace(&marketplace).pending_authority,
        new_authority.pubkey()
    );

    world
        .accept_marketplace_authority(&new_authority, marketplace)
        .expect("accept_marketplace_authority succeeds");
    let updated = world.read_marketplace(&marketplace);
    assert_eq!(updated.authority, new_authority.pubkey());
    assert_eq!(updated.pending_authority, Pubkey::default());
}

#[test]
fn test_unauthorized_authority_transfer_rejected() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();

    // Config: only the current authority may propose.
    let config_impostor = funded_keypair(&mut world);
    let result = world.propose_config_authority(&config_impostor, unique_pubkey());
    assert_error_code(&result, u32::from(anchor_lang::error::ErrorCode::ConstraintHasOne));

    // Config: only the named pending authority may accept.
    let real_new_authority = funded_keypair(&mut world);
    world
        .propose_config_authority(&admin, real_new_authority.pubkey())
        .unwrap();
    let result = world.accept_config_authority(&config_impostor);
    assert_error_code(&result, u32::from(anchor_lang::error::ErrorCode::ConstraintHasOne));

    // Marketplace: same two checks.
    let (authority, marketplace) = setup_marketplace(&mut world, 51);
    let marketplace_impostor = funded_keypair(&mut world);
    let result = world.propose_marketplace_authority(&marketplace_impostor, marketplace, unique_pubkey());
    assert_error_code(&result, u32::from(anchor_lang::error::ErrorCode::ConstraintHasOne));

    let real_new_marketplace_authority = funded_keypair(&mut world);
    world
        .propose_marketplace_authority(&authority, marketplace, real_new_marketplace_authority.pubkey())
        .unwrap();
    let result = world.accept_marketplace_authority(&marketplace_impostor, marketplace);
    assert_error_code(&result, u32::from(anchor_lang::error::ErrorCode::ConstraintHasOne));
}

// ---------------------------------------------------------------------
// Seller collateral
// ---------------------------------------------------------------------

#[test]
fn test_initialize_stake_succeeds() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();
    let seller = funded_keypair(&mut world);

    world.initialize_stake(&seller).expect("initialize_stake succeeds");

    let stake_pubkey = world.stake_pda(&seller.pubkey());
    let stake = world.read_seller_stake(&stake_pubkey);
    assert_eq!(stake.version, 1);
    assert_eq!(stake.seller, seller.pubkey());
    assert_eq!(stake.staked, 0);
    assert_eq!(stake.committed, 0);
    assert_eq!(stake.disputes_total, 0);
    assert_eq!(stake.disputes_lost, 0);

    let vault = world.read_token_account(&world.stake_vault_pda(&seller.pubkey()));
    assert_eq!(vault.amount, 0);
    assert_eq!(vault.mint, world.mint);
}

#[test]
fn test_initialize_stake_rejects_double_init() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();
    let seller = funded_keypair(&mut world);

    world.initialize_stake(&seller).expect("first initialize_stake succeeds");

    // Same signer, same (empty) arguments: needs a fresh blockhash for
    // the same reason as test_initialize_config_rejects_double_init.
    world.svm.expire_blockhash();
    let result = world.initialize_stake(&seller);
    assert_error_code(&result, SystemError::AccountAlreadyInUse as u32);
}

#[test]
fn test_add_stake_succeeds() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();
    let seller = funded_keypair(&mut world);
    world.initialize_stake(&seller).unwrap();

    let seller_token_account = world.create_funded_token_account(world.mint, seller.pubkey(), usdc(300));

    world
        .add_stake(&seller, seller_token_account, usdc(150))
        .expect("add_stake succeeds");

    let stake_pubkey = world.stake_pda(&seller.pubkey());
    assert_eq!(world.read_seller_stake(&stake_pubkey).staked, usdc(150));
    assert_eq!(world.token_balance(&seller_token_account), usdc(150));
    assert_eq!(world.token_balance(&world.stake_vault_pda(&seller.pubkey())), usdc(150));

    // A second deposit accumulates rather than overwriting.
    world
        .add_stake(&seller, seller_token_account, usdc(50))
        .expect("second add_stake succeeds");
    assert_eq!(world.read_seller_stake(&stake_pubkey).staked, usdc(200));
}

#[test]
fn test_add_stake_rejects_zero() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();
    let seller = funded_keypair(&mut world);
    world.initialize_stake(&seller).unwrap();
    let seller_token_account = world.create_funded_token_account(world.mint, seller.pubkey(), usdc(10));

    let result = world.add_stake(&seller, seller_token_account, 0);
    assert_error_code(&result, u32::from(TrustStakeError::ZeroAmount));
}

#[test]
fn test_add_stake_rejects_foreign_token_account() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();
    let seller = funded_keypair(&mut world);
    world.initialize_stake(&seller).unwrap();

    let other_owner = funded_keypair(&mut world);
    let foreign_token_account = world.create_funded_token_account(world.mint, other_owner.pubkey(), usdc(100));

    let result = world.add_stake(&seller, foreign_token_account, usdc(10));
    assert_error_code(&result, u32::from(anchor_lang::error::ErrorCode::ConstraintTokenOwner));
}

#[test]
fn test_fake_vault_rejected() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();
    let seller = funded_keypair(&mut world);
    world.initialize_stake(&seller).unwrap();
    let seller_token_account = world.create_funded_token_account(world.mint, seller.pubkey(), usdc(100));

    // A real, legitimate token account under the right mint, just not
    // the seller's real stake_vault PDA.
    let attacker = funded_keypair(&mut world);
    let fake_vault = world.create_funded_token_account(world.mint, attacker.pubkey(), 0);

    let result = world.add_stake_with_vault(&seller, seller_token_account, fake_vault, usdc(10));
    assert_error_code(&result, u32::from(anchor_lang::error::ErrorCode::ConstraintSeeds));
}

#[test]
fn test_wrong_mint_rejected() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();
    let seller = funded_keypair(&mut world);
    let extra_mint = world.create_extra_mint();

    // A vault created against a mint other than Config.collateral_mint
    // fails at initialize_stake.
    let result = world.initialize_stake_with_mint(&seller, extra_mint, spl_token::ID);
    assert_error_code(&result, u32::from(TrustStakeError::WrongMint));

    // A mint account other than stake_vault.mint fails at add_stake.
    world.initialize_stake(&seller).expect("real initialize_stake succeeds");
    let seller_token_account = world.create_funded_token_account(world.mint, seller.pubkey(), usdc(100));
    let result = world.add_stake_with_mint(&seller, seller_token_account, extra_mint, usdc(10));
    assert_error_code(&result, u32::from(TrustStakeError::WrongMint));
}

#[test]
fn test_transfer_fee_mint_rejected() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();
    let seller = funded_keypair(&mut world);

    let fee_mint = world.create_transfer_fee_mint(100, u64::MAX);

    let result = world.initialize_stake_with_mint(&seller, fee_mint, spl_token_2022::ID);
    assert_error_code(&result, u32::from(TrustStakeError::WrongMint));
}

#[test]
fn test_unbound_mint_rejected() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();
    let seller = funded_keypair(&mut world);
    world.initialize_stake(&seller).unwrap();
    let seller_token_account = world.create_funded_token_account(world.mint, seller.pubkey(), usdc(100));
    let extra_mint = world.create_extra_mint();

    let stake_pubkey = world.stake_pda(&seller.pubkey());
    let vault_pubkey = world.stake_vault_pda(&seller.pubkey());
    let staked_before = world.read_seller_stake(&stake_pubkey).staked;
    let vault_balance_before = world.token_balance(&vault_pubkey);

    let result = world.add_stake_with_mint(&seller, seller_token_account, extra_mint, usdc(10));
    assert_error_code(&result, u32::from(TrustStakeError::WrongMint));

    // The vault itself looked correct throughout; rejection happened
    // before any transfer, so nothing moved.
    assert_eq!(world.read_seller_stake(&stake_pubkey).staked, staked_before);
    assert_eq!(world.token_balance(&vault_pubkey), vault_balance_before);
}

#[test]
fn test_conservation_asserted_onchain() {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();
    let seller = funded_keypair(&mut world);
    world.initialize_stake(&seller).unwrap();
    let seller_token_account = world.create_funded_token_account(world.mint, seller.pubkey(), usdc(300));
    world.add_stake(&seller, seller_token_account, usdc(100)).unwrap();

    // Force a mismatch in the direction that is still dangerous: shrink
    // the vault's real balance below the ledger's `staked`, which is
    // what a drain looks like. This can no longer be done with an
    // ordinary instruction -- `stake_vault`'s token authority is the
    // `stake` PDA, which nothing outside the program can sign for, so
    // the vault can only ever be pushed *above* the ledger from the
    // outside (a harmless direction; see
    // test_stake_vault_donation_does_not_brick_the_seller in
    // test_phase3.rs). The mismatch is written directly into account
    // state via the harness instead, standing in for whatever bug would
    // someday produce it.
    let vault = world.stake_vault_pda(&seller.pubkey());
    let mut vault_account = world.svm.get_account(&vault).expect("vault exists");
    let mut vault_state =
        spl_token::state::Account::unpack(&vault_account.data).expect("valid token account data");
    vault_state.amount -= usdc(50);
    Pack::pack(vault_state, &mut vault_account.data).expect("repack the shrunk token account");
    world
        .svm
        .set_account(vault, vault_account)
        .expect("force the vault below the ledger");

    // The next handler that moves tokens must catch the mismatch and
    // fail. Built directly with `common::send` rather than through
    // `World::add_stake`, because that method's own `assert_invariants`
    // call checks the same `vault.amount >= stake.staked` relation as the
    // runtime check and would therefore also (correctly) catch this same
    // deficit and panic before the onchain check gets a chance to run;
    // this test is specifically about the runtime assertion, not the
    // harness's own bookkeeping. A surplus, unlike a deficit, trips
    // neither check any more (see
    // test_stake_vault_donation_does_not_brick_the_seller in
    // test_phase3.rs), so this test only has one direction left to prove.
    let (event_authority, program) = event_cpi_accounts(&world.program_id);
    let instruction = Instruction::new_with_bytes(
        world.program_id,
        &truststake::instruction::AddStake { amount: usdc(10) }.data(),
        truststake::accounts::AddStakeAccountConstraints {
            seller: seller.pubkey(),
            stake: world.stake_pda(&seller.pubkey()),
            stake_vault: vault,
            mint: world.mint,
            seller_token_account,
            token_program: spl_token::ID,
            event_authority,
            program,
        }
        .to_account_metas(None),
    );
    let result = common::send(&mut world.svm, &[instruction], &seller.pubkey(), &[&seller]);
    assert_error_code(&result, u32::from(TrustStakeError::ConservationViolation));
}


