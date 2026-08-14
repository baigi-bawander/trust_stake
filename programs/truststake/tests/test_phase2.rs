//! Phase 2 test suite (docs/TESTING.md): every test here belongs to
//! Phase 2 because the last handler it calls is one of the six Phase 2
//! handlers (`withdraw_stake`, `grant_permit`, `increase_permit`,
//! `revoke_permit`, `release_permit`, `release_permit_early`). See the
//! end-of-task report for the mapping against docs/TESTING.md's named
//! tests, including which ones are genuinely blocked on Phase 3.

mod common;

use anchor_lang::{prelude::*, InstructionData, ToAccountMetas};
use common::{assert_error_code, event_cpi_accounts, initial_admin_keypair, usdc, World};
use solana_instruction::{AccountMeta, Instruction};
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

/// A World with `Config` already initialized -- shared first line for
/// every Phase 2 test.
fn setup_world() -> World {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();
    world
}

fn setup_marketplace(world: &mut World, tag: u8, complaint_window: i64, bond_bps: u16) -> (Keypair, Pubkey) {
    let authority = funded_keypair(world);
    let id = marketplace_id(tag);
    world
        .register_marketplace(&authority, id, unique_pubkey(), unique_pubkey(), complaint_window, bond_bps)
        .expect("register_marketplace succeeds");
    (authority, world.marketplace_pda(&id))
}

/// A seller with `staked` USDC already locked in `stake_vault`, plus
/// `spare` more sitting in their own wallet for further `add_stake`
/// calls. Returns the seller and their own token account.
fn setup_staked_seller(world: &mut World, staked: u64, spare: u64) -> (Keypair, Pubkey) {
    let seller = funded_keypair(world);
    world.initialize_stake(&seller).expect("initialize_stake succeeds");
    let token_account = world.create_funded_token_account(world.mint, seller.pubkey(), staked + spare);
    if staked > 0 {
        world.add_stake(&seller, token_account, staked).expect("add_stake succeeds");
    }
    (seller, token_account)
}

// ---------------------------------------------------------------------
// withdraw_stake
// ---------------------------------------------------------------------

#[test]
fn test_withdraw_stake_succeeds() {
    let mut world = setup_world();
    let (seller, token_account) = setup_staked_seller(&mut world, usdc(300), 0);

    world
        .withdraw_stake(&seller, token_account, usdc(100))
        .expect("withdraw_stake succeeds");

    let stake_pubkey = world.stake_pda(&seller.pubkey());
    assert_eq!(world.read_seller_stake(&stake_pubkey).staked, usdc(200));
    assert_eq!(world.token_balance(&token_account), usdc(100));
    assert_eq!(world.token_balance(&world.stake_vault_pda(&seller.pubkey())), usdc(200));
}

#[test]
fn test_seller_cannot_withdraw_committed() {
    let mut world = setup_world();
    let (seller, token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (_authority, marketplace) = setup_marketplace(&mut world, 1, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    world.grant_permit(&seller, marketplace, usdc(150)).unwrap();
    // staked = 300, committed = 150; free = 150.

    // One cent above the free boundary fails.
    let result = world.withdraw_stake(&seller, token_account, usdc(150) + 1);
    assert_error_code(&result, u32::from(TrustStakeError::CommittedExceedsStaked));

    // Exactly at the boundary succeeds: `staked - amount >= committed` is inclusive.
    world
        .withdraw_stake(&seller, token_account, usdc(150))
        .expect("withdrawal to the exact boundary succeeds");
    let stake = world.read_seller_stake(&world.stake_pda(&seller.pubkey()));
    assert_eq!(stake.staked, usdc(150));
    assert_eq!(stake.committed, usdc(150));
}

#[test]
fn test_seller_cannot_withdraw_during_window() {
    // TS-02: revoked_at's i64::MAX "active" sentinel must never collide
    // with 0, or `release_permit`'s window check would be trivially true
    // the instant a permit is revoked.
    let mut world = setup_world();
    let (seller, token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (_authority, marketplace) = setup_marketplace(&mut world, 2, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    world.grant_permit(&seller, marketplace, usdc(150)).unwrap();
    world.revoke_permit(&seller, marketplace).expect("revoke_permit succeeds");

    // Confirm release is still gated right after revocation, in the same instant.
    let caller = funded_keypair(&mut world);
    let result = world.release_permit(&caller, seller.pubkey(), marketplace);
    assert_error_code(&result, u32::from(TrustStakeError::ComplaintWindowNotElapsed));

    // committed is untouched by revoke alone, so the seller still can't
    // withdraw the committed collateral either.
    let result = world.withdraw_stake(&seller, token_account, usdc(150) + 1);
    assert_error_code(&result, u32::from(TrustStakeError::CommittedExceedsStaked));
}

#[test]
fn test_seller_repeated_withdrawals_respect_committed() {
    let mut world = setup_world();
    let (seller, token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (_authority, marketplace) = setup_marketplace(&mut world, 3, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    world.grant_permit(&seller, marketplace, usdc(150)).unwrap();
    // free = 150.

    // 14 * 10 = 140 of the 150 free balance, in small pieces.
    for _ in 0..14 {
        world
            .withdraw_stake(&seller, token_account, usdc(10))
            .expect("small withdrawal within the free balance succeeds");
        // Each call is byte-identical to the last (same amount, same
        // accounts); LiteSVM rejects a repeat against the same blockhash
        // as AlreadyProcessed before the program even runs.
        world.svm.expire_blockhash();
    }
    let stake_pubkey = world.stake_pda(&seller.pubkey());
    assert_eq!(world.read_seller_stake(&stake_pubkey).staked, usdc(160));

    // The accumulated small withdrawals still can't punch through the
    // committed gate: only 10 of free balance remains.
    let result = world.withdraw_stake(&seller, token_account, usdc(11));
    assert_error_code(&result, u32::from(TrustStakeError::CommittedExceedsStaked));

    world
        .withdraw_stake(&seller, token_account, usdc(10))
        .expect("final withdrawal to the exact boundary succeeds");
    assert_eq!(world.read_seller_stake(&stake_pubkey).staked, usdc(150));
}

#[test]
fn test_seller_recovers_from_zero() {
    // The prototype's dead-account hole (docs/DESIGN-v2.md, "Where the
    // code is today"): initialize_stake/add_stake's split means a seller
    // at zero balance is never stuck, since the SellerStake account
    // itself is never re-`init`'d. Phase 2 has no slashing yet, so this
    // reaches zero via a full withdrawal rather than a slash -- a
    // different route to the same state, exercising the identical
    // guarantee (add_stake still works once staked == 0).
    let mut world = setup_world();
    let (seller, token_account) = setup_staked_seller(&mut world, usdc(100), usdc(50));

    world
        .withdraw_stake(&seller, token_account, usdc(100))
        .expect("full withdrawal to zero succeeds");
    let stake_pubkey = world.stake_pda(&seller.pubkey());
    assert_eq!(world.read_seller_stake(&stake_pubkey).staked, 0);

    world
        .add_stake(&seller, token_account, usdc(50))
        .expect("add_stake after reaching zero succeeds");
    assert_eq!(world.read_seller_stake(&stake_pubkey).staked, usdc(50));
}

#[test]
fn test_withdraw_stake_rejects_foreign_token_account() {
    let mut world = setup_world();
    let (seller, _token_account) = setup_staked_seller(&mut world, usdc(100), 0);

    let other_owner = funded_keypair(&mut world);
    let foreign_token_account = world.create_funded_token_account(world.mint, other_owner.pubkey(), usdc(10));

    let result = world.withdraw_stake(&seller, foreign_token_account, usdc(10));
    assert_error_code(&result, u32::from(anchor_lang::error::ErrorCode::ConstraintTokenOwner));
}

#[test]
fn test_wrong_mint_rejected() {
    let mut world = setup_world();
    let (seller, token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let extra_mint = world.create_extra_mint();

    let stake_pubkey = world.stake_pda(&seller.pubkey());
    let vault_pubkey = world.stake_vault_pda(&seller.pubkey());
    let staked_before = world.read_seller_stake(&stake_pubkey).staked;
    let vault_balance_before = world.token_balance(&vault_pubkey);

    let result = world.withdraw_stake_with_mint(&seller, token_account, extra_mint, usdc(10));
    assert_error_code(&result, u32::from(TrustStakeError::WrongMint));

    // The vault itself looked correct throughout; rejection happened
    // before any transfer, so nothing moved (also covers
    // test_unbound_mint_rejected's point for this handler).
    assert_eq!(world.read_seller_stake(&stake_pubkey).staked, staked_before);
    assert_eq!(world.token_balance(&vault_pubkey), vault_balance_before);
}

#[test]
fn test_fake_vault_rejected() {
    let mut world = setup_world();
    let (seller, token_account) = setup_staked_seller(&mut world, usdc(100), 0);

    let attacker = funded_keypair(&mut world);
    let fake_vault = world.create_funded_token_account(world.mint, attacker.pubkey(), 0);

    let result = world.withdraw_stake_with_vault(&seller, token_account, fake_vault, usdc(10));
    assert_error_code(&result, u32::from(anchor_lang::error::ErrorCode::ConstraintSeeds));
}

// ---------------------------------------------------------------------
// grant_permit / increase_permit
// ---------------------------------------------------------------------

#[test]
fn test_grant_permit_succeeds() {
    let mut world = setup_world();
    let (seller, _token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (_authority, marketplace) = setup_marketplace(&mut world, 10, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);

    world
        .grant_permit(&seller, marketplace, usdc(150))
        .expect("grant_permit succeeds");

    let permit = world.read_permit(&world.permit_pda(&seller.pubkey(), &marketplace));
    assert_eq!(permit.version, 1);
    assert_eq!(permit.seller, seller.pubkey());
    assert_eq!(permit.marketplace, marketplace);
    assert_eq!(permit.max_slashable, usdc(150));
    assert_eq!(permit.slashed, 0);
    assert_eq!(permit.open_disputes, 0);
    assert_eq!(permit.revoked_at, i64::MAX);
    assert_eq!(permit.complaint_window, MIN_COMPLAINT_WINDOW_SECONDS);
    assert_eq!(permit.bond_bps, DEFAULT_BOND_BPS);

    let stake = world.read_seller_stake(&world.stake_pda(&seller.pubkey()));
    assert_eq!(stake.committed, usdc(150));
}

#[test]
fn test_seller_cannot_overgrant() {
    let mut world = setup_world();
    let (seller, _token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (_authority_one, marketplace_one) =
        setup_marketplace(&mut world, 11, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    let (_authority_two, marketplace_two) =
        setup_marketplace(&mut world, 12, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);

    // One cent above staked fails, on the very first grant.
    let result = world.grant_permit(&seller, marketplace_one, usdc(300) + 1);
    assert_error_code(&result, u32::from(TrustStakeError::CommittedExceedsStaked));

    // Exactly at staked succeeds.
    world
        .grant_permit(&seller, marketplace_one, usdc(300))
        .expect("grant at the exact boundary succeeds");

    // A second permit at a DIFFERENT marketplace, for even $1, now has no
    // free collateral to draw from -- committed is global across
    // marketplaces, per decision 1.
    let result = world.grant_permit(&seller, marketplace_two, usdc(1));
    assert_error_code(&result, u32::from(TrustStakeError::CommittedExceedsStaked));
}

#[test]
fn test_seller_cannot_overgrant_via_increase() {
    let mut world = setup_world();
    let (seller, _token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (_authority, marketplace) = setup_marketplace(&mut world, 13, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    world.grant_permit(&seller, marketplace, usdc(200)).unwrap();

    // 200 + 101 > 300.
    let result = world.increase_permit(&seller, marketplace, usdc(100) + 1);
    assert_error_code(&result, u32::from(TrustStakeError::CommittedExceedsStaked));

    // Exactly to the boundary succeeds.
    world
        .increase_permit(&seller, marketplace, usdc(100))
        .expect("increase to the exact boundary succeeds");
    let stake = world.read_seller_stake(&world.stake_pda(&seller.pubkey()));
    assert_eq!(stake.committed, usdc(300));
}

#[test]
fn test_increase_permit_succeeds() {
    let mut world = setup_world();
    let (seller, _token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (_authority, marketplace) = setup_marketplace(&mut world, 14, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    world.grant_permit(&seller, marketplace, usdc(100)).unwrap();

    world
        .increase_permit(&seller, marketplace, usdc(50))
        .expect("increase_permit succeeds");

    let permit = world.read_permit(&world.permit_pda(&seller.pubkey(), &marketplace));
    assert_eq!(permit.max_slashable, usdc(150));
    let stake = world.read_seller_stake(&world.stake_pda(&seller.pubkey()));
    assert_eq!(stake.committed, usdc(150));
}

#[test]
fn test_seller_cannot_lower_permit_cap() {
    // TS-13. `increase_permit` takes an unsigned delta that is only ever
    // added to `max_slashable` -- there is no instruction, and no
    // argument value, that could express "lower the cap" at all. The
    // protection is the API shape itself, not a runtime check, so this
    // test pins that shape: repeated increases only ever grow the cap,
    // by exactly the sum of every delta.
    let mut world = setup_world();
    let (seller, _token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (_authority, marketplace) = setup_marketplace(&mut world, 15, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    world.grant_permit(&seller, marketplace, usdc(100)).unwrap();
    let permit_pubkey = world.permit_pda(&seller.pubkey(), &marketplace);
    let after_grant = world.read_permit(&permit_pubkey).max_slashable;

    world.increase_permit(&seller, marketplace, usdc(20)).unwrap();
    let after_first_increase = world.read_permit(&permit_pubkey).max_slashable;
    assert!(after_first_increase > after_grant);
    assert_eq!(after_first_increase, after_grant + usdc(20));

    world.increase_permit(&seller, marketplace, usdc(5)).unwrap();
    let after_second_increase = world.read_permit(&permit_pubkey).max_slashable;
    assert!(after_second_increase > after_first_increase);
    assert_eq!(after_second_increase, after_grant + usdc(25));
}

#[test]
fn test_seller_cannot_grant_without_signing() {
    let mut world = setup_world();
    let (seller, _token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (_authority, marketplace) = setup_marketplace(&mut world, 16, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    let payer = funded_keypair(&mut world);

    let stake = world.stake_pda(&seller.pubkey());
    let permit = world.permit_pda(&seller.pubkey(), &marketplace);
    let (event_authority, program) = event_cpi_accounts(&world.program_id);

    // Same account metas GrantPermitAccountConstraints would generate,
    // except seller's is_signer is forced false: a relayer submitting on
    // the seller's behalf using only their public key, no signature at
    // all. Requiring the seller's signature is the entire meaning of
    // opting in (docs/DESIGN-v2.md, "Instruction handlers").
    let mut metas = truststake::accounts::GrantPermitAccountConstraints {
        seller: seller.pubkey(),
        stake,
        marketplace,
        permit,
        system_program: anchor_lang::system_program::ID,
        event_authority,
        program,
    }
    .to_account_metas(None);
    for meta in metas.iter_mut() {
        if meta.pubkey == seller.pubkey() {
            *meta = AccountMeta::new(seller.pubkey(), false);
        }
    }

    let instruction = Instruction::new_with_bytes(
        world.program_id,
        &truststake::instruction::GrantPermit { max_slashable: usdc(50) }.data(),
        metas,
    );

    let result = world.send_instructions(&[instruction], &payer.pubkey(), &[&payer]);
    assert_error_code(&result, u32::from(anchor_lang::error::ErrorCode::AccountNotSigner));
}

#[test]
fn test_seller_zero_amounts_rejected() {
    let mut world = setup_world();
    let (seller, token_account) = setup_staked_seller(&mut world, usdc(100), usdc(10));
    let (_authority, marketplace) = setup_marketplace(&mut world, 17, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);

    let result = world.add_stake(&seller, token_account, 0);
    assert_error_code(&result, u32::from(TrustStakeError::ZeroAmount));

    let result = world.withdraw_stake(&seller, token_account, 0);
    assert_error_code(&result, u32::from(TrustStakeError::ZeroAmount));

    let result = world.grant_permit(&seller, marketplace, 0);
    assert_error_code(&result, u32::from(TrustStakeError::ZeroAmount));

    world.grant_permit(&seller, marketplace, usdc(10)).expect("nonzero grant succeeds");
    let result = world.increase_permit(&seller, marketplace, 0);
    assert_error_code(&result, u32::from(TrustStakeError::ZeroAmount));
}

#[test]
fn test_marketplace_settings_do_not_affect_existing_permits() {
    let mut world = setup_world();
    let (seller, _token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (authority, marketplace) = setup_marketplace(&mut world, 18, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    world.grant_permit(&seller, marketplace, usdc(150)).unwrap();
    let permit_pubkey = world.permit_pda(&seller.pubkey(), &marketplace);

    let frozen = world.read_permit(&permit_pubkey);
    assert_eq!(frozen.complaint_window, MIN_COMPLAINT_WINDOW_SECONDS);
    assert_eq!(frozen.bond_bps, DEFAULT_BOND_BPS);

    world
        .update_marketplace(
            &authority,
            marketplace,
            None,
            None,
            Some(MAX_COMPLAINT_WINDOW_SECONDS),
            Some(MAX_BOND_BPS),
        )
        .expect("update_marketplace succeeds");
    let updated = world.read_marketplace(&marketplace);
    assert_eq!(updated.complaint_window, MAX_COMPLAINT_WINDOW_SECONDS);
    assert_eq!(updated.bond_bps, MAX_BOND_BPS);

    // The existing permit's frozen values are untouched by the update.
    let still_frozen = world.read_permit(&permit_pubkey);
    assert_eq!(still_frozen.complaint_window, MIN_COMPLAINT_WINDOW_SECONDS);
    assert_eq!(still_frozen.bond_bps, DEFAULT_BOND_BPS);

    // A permit granted after the update picks up the new settings.
    let (seller_two, _token_account_two) = setup_staked_seller(&mut world, usdc(300), 0);
    world.grant_permit(&seller_two, marketplace, usdc(50)).unwrap();
    let new_permit = world.read_permit(&world.permit_pda(&seller_two.pubkey(), &marketplace));
    assert_eq!(new_permit.complaint_window, MAX_COMPLAINT_WINDOW_SECONDS);
    assert_eq!(new_permit.bond_bps, MAX_BOND_BPS);
}

// ---------------------------------------------------------------------
// revoke_permit
// ---------------------------------------------------------------------

#[test]
fn test_revoke_permit_succeeds() {
    let mut world = setup_world();
    let (seller, _token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (_authority, marketplace) = setup_marketplace(&mut world, 19, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    world.grant_permit(&seller, marketplace, usdc(150)).unwrap();

    world.revoke_permit(&seller, marketplace).expect("revoke_permit succeeds");

    let permit = world.read_permit(&world.permit_pda(&seller.pubkey(), &marketplace));
    assert!(permit.revoked_at > 0 && permit.revoked_at < i64::MAX);

    // Revoking alone frees nothing.
    let stake = world.read_seller_stake(&world.stake_pda(&seller.pubkey()));
    assert_eq!(stake.committed, usdc(150));
}

#[test]
fn test_revoke_permit_rejects_double_revoke() {
    let mut world = setup_world();
    let (seller, _token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (_authority, marketplace) = setup_marketplace(&mut world, 20, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    world.grant_permit(&seller, marketplace, usdc(150)).unwrap();
    world.revoke_permit(&seller, marketplace).unwrap();
    world.svm.expire_blockhash();

    let result = world.revoke_permit(&seller, marketplace);
    assert_error_code(&result, u32::from(TrustStakeError::PermitAlreadyRevoked));
}

#[test]
fn test_seller_cannot_regrant_smaller_during_window() {
    // F5: without waiting out the window (or releasing), the permit PDA
    // still exists, so a second grant_permit at the same (seller,
    // marketplace) address -- smaller cap or not -- collides with `init`
    // rather than reducing exposure on whatever's already committed.
    let mut world = setup_world();
    let (seller, _token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (_authority, marketplace) = setup_marketplace(&mut world, 21, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    world.grant_permit(&seller, marketplace, usdc(150)).unwrap();
    world.revoke_permit(&seller, marketplace).unwrap();

    let result = world.grant_permit(&seller, marketplace, usdc(10));
    assert_error_code(&result, SystemError::AccountAlreadyInUse as u32);

    let stake = world.read_seller_stake(&world.stake_pda(&seller.pubkey()));
    assert_eq!(stake.committed, usdc(150));
}

// docs/TESTING.md's test_pda_substitution_rejected, permit half: this
// also serves as revoke_permit's authorization test.
#[test]
fn test_pda_substitution_rejected() {
    let mut world = setup_world();
    let (seller_a, _token_a) = setup_staked_seller(&mut world, usdc(300), 0);
    let (seller_b, _token_b) = setup_staked_seller(&mut world, usdc(300), 0);
    let (_authority, marketplace) = setup_marketplace(&mut world, 22, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    world.grant_permit(&seller_a, marketplace, usdc(100)).unwrap();
    world.grant_permit(&seller_b, marketplace, usdc(100)).unwrap();

    // seller_b signs, legitimately, but supplies seller_a's real permit
    // PDA instead of their own.
    let permit_a = world.permit_pda(&seller_a.pubkey(), &marketplace);
    let (event_authority, program) = event_cpi_accounts(&world.program_id);
    let instruction = Instruction::new_with_bytes(
        world.program_id,
        &truststake::instruction::RevokePermit {}.data(),
        truststake::accounts::RevokePermitAccountConstraints {
            seller: seller_b.pubkey(),
            permit: permit_a,
            event_authority,
            program,
        }
        .to_account_metas(None),
    );
    let result = world.send_instructions(&[instruction], &seller_b.pubkey(), &[&seller_b]);
    assert_error_code(&result, u32::from(anchor_lang::error::ErrorCode::ConstraintHasOne));

    // seller_a's permit is untouched.
    assert_eq!(world.read_permit(&permit_a).revoked_at, i64::MAX);
}

// docs/TESTING.md's test_account_type_substitution_rejected, permit half.
#[test]
fn test_account_type_substitution_rejected() {
    let mut world = setup_world();
    let (seller, _token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let config = world.config_pda();

    // A real, correctly-owned Config account passed where Marketplace is
    // expected must fail on the discriminator, before seeds are even
    // considered.
    let stake = world.stake_pda(&seller.pubkey());
    let permit = world.permit_pda(&seller.pubkey(), &config);
    let (event_authority, program) = event_cpi_accounts(&world.program_id);
    let instruction = Instruction::new_with_bytes(
        world.program_id,
        &truststake::instruction::GrantPermit { max_slashable: usdc(10) }.data(),
        truststake::accounts::GrantPermitAccountConstraints {
            seller: seller.pubkey(),
            stake,
            marketplace: config,
            permit,
            system_program: anchor_lang::system_program::ID,
            event_authority,
            program,
        }
        .to_account_metas(None),
    );
    let result = world.send_instructions(&[instruction], &seller.pubkey(), &[&seller]);
    assert_error_code(
        &result,
        u32::from(anchor_lang::error::ErrorCode::AccountDiscriminatorMismatch),
    );
}

// ---------------------------------------------------------------------
// release_permit / release_permit_early
// ---------------------------------------------------------------------

#[test]
fn test_release_frees_committed() {
    let mut world = setup_world();
    let (seller, token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (_authority, marketplace) = setup_marketplace(&mut world, 23, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    world.grant_permit(&seller, marketplace, usdc(150)).unwrap();
    let stake_pubkey = world.stake_pda(&seller.pubkey());

    // Revoking alone drops committed by nothing.
    world.revoke_permit(&seller, marketplace).unwrap();
    assert_eq!(world.read_seller_stake(&stake_pubkey).committed, usdc(150));

    world.warp_seconds(MIN_COMPLAINT_WINDOW_SECONDS + 1);
    let caller = funded_keypair(&mut world);
    world
        .release_permit(&caller, seller.pubkey(), marketplace)
        .expect("release_permit succeeds");

    // Releasing drops committed by exactly the permit's remaining
    // allowance (max_slashable - slashed = 150 - 0).
    assert_eq!(world.read_seller_stake(&stake_pubkey).committed, 0);

    // Now the full balance is free.
    world
        .withdraw_stake(&seller, token_account, usdc(300))
        .expect("full withdrawal succeeds once nothing is committed");
}

#[test]
fn test_release_permit_rejects_while_active() {
    // Never revoked: revoked_at is still the i64::MAX sentinel. Rejected
    // by the explicit revoked-permit check, not by the checked_add below
    // it overflowing -- that overflow is a backstop, not the guard.
    let mut world = setup_world();
    let (seller, _token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (_authority, marketplace) = setup_marketplace(&mut world, 24, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    world.grant_permit(&seller, marketplace, usdc(150)).unwrap();

    let caller = funded_keypair(&mut world);
    let result = world.release_permit(&caller, seller.pubkey(), marketplace);
    assert_error_code(&result, u32::from(TrustStakeError::PermitNotRevoked));
}

#[test]
fn test_release_permit_early_rejects_while_active() {
    // "Early" skips the wait, not the wind-down: a never-revoked permit
    // is not eligible for this path either, even with both signatures.
    let mut world = setup_world();
    let (seller, _token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (authority, marketplace) = setup_marketplace(&mut world, 26, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    world.grant_permit(&seller, marketplace, usdc(150)).unwrap();

    let result = world.release_permit_early(&seller, &authority, marketplace);
    assert_error_code(&result, u32::from(TrustStakeError::PermitNotRevoked));
}

#[test]
fn test_increase_permit_rejects_revoked() {
    // Increasing the cap on a permit already winding down is not
    // exploitable (payout stays bounded by the receipt regardless), but
    // it is a confusing no-op that only locks up more of the seller's own
    // collateral, so it is rejected the same way a double-revoke is.
    let mut world = setup_world();
    let (seller, _token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (_authority, marketplace) = setup_marketplace(&mut world, 27, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    world.grant_permit(&seller, marketplace, usdc(150)).unwrap();
    world.revoke_permit(&seller, marketplace).unwrap();

    let result = world.increase_permit(&seller, marketplace, usdc(10));
    assert_error_code(&result, u32::from(TrustStakeError::PermitAlreadyRevoked));
}

#[test]
fn test_release_boundary_exact() {
    let mut world = setup_world();
    let (seller, _token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (_authority, marketplace) = setup_marketplace(&mut world, 25, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    world.grant_permit(&seller, marketplace, usdc(150)).unwrap();
    world.revoke_permit(&seller, marketplace).unwrap();
    let caller = funded_keypair(&mut world);

    // One second before revoked_at + complaint_window fails.
    world.warp_seconds(MIN_COMPLAINT_WINDOW_SECONDS - 1);
    let result = world.release_permit(&caller, seller.pubkey(), marketplace);
    assert_error_code(&result, u32::from(TrustStakeError::ComplaintWindowNotElapsed));
    world.svm.expire_blockhash();

    // Exactly at the boundary succeeds: inclusive, matching the doc's
    // "now >= revoked_at + complaint_window".
    world.warp_seconds(1);
    world
        .release_permit(&caller, seller.pubkey(), marketplace)
        .expect("release exactly at the boundary succeeds");
}

#[test]
fn test_rent_refund_on_close() {
    let mut world = setup_world();
    let (seller, _token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (_authority, marketplace) = setup_marketplace(&mut world, 26, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    world.grant_permit(&seller, marketplace, usdc(150)).unwrap();

    world.revoke_permit(&seller, marketplace).unwrap();
    world.warp_seconds(MIN_COMPLAINT_WINDOW_SECONDS + 1);

    // Snapshot right before the one transaction expected to move the
    // seller's lamports: `revoke_permit` above is itself fee-paid by the
    // seller, so snapshotting any earlier would fold that fee into the
    // "before" baseline and throw off the exact-refund assertion below.
    let permit_pubkey = world.permit_pda(&seller.pubkey(), &marketplace);
    let rent_lamports = world.svm.get_account(&permit_pubkey).expect("permit exists").lamports;
    let seller_lamports_before = world.svm.get_account(&seller.pubkey()).expect("seller exists").lamports;

    let caller = funded_keypair(&mut world);
    world
        .release_permit(&caller, seller.pubkey(), marketplace)
        .expect("release_permit succeeds");

    // The rent goes to the seller, who paid it at grant. `caller`, not
    // `seller`, pays this transaction's fee, so the seller's balance
    // should move by exactly the refunded rent.
    let seller_lamports_after = world.svm.get_account(&seller.pubkey()).expect("seller exists").lamports;
    assert_eq!(seller_lamports_after, seller_lamports_before + rent_lamports);

    // The account is genuinely gone.
    assert!(
        world.svm.get_account(&permit_pubkey).is_none(),
        "permit account must no longer exist after release_permit's close"
    );
}

#[test]
fn test_release_permit_early_succeeds() {
    let mut world = setup_world();
    let (seller, token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (authority, marketplace) = setup_marketplace(&mut world, 27, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    world.grant_permit(&seller, marketplace, usdc(150)).unwrap();
    world.revoke_permit(&seller, marketplace).unwrap();

    // No warp at all: proves the wait is genuinely skipped, not just shortened.
    world
        .release_permit_early(&seller, &authority, marketplace)
        .expect("release_permit_early succeeds immediately with both signatures");

    let stake_pubkey = world.stake_pda(&seller.pubkey());
    assert_eq!(world.read_seller_stake(&stake_pubkey).committed, 0);
    world
        .withdraw_stake(&seller, token_account, usdc(300))
        .expect("full withdrawal succeeds immediately after the early release");

    let permit_pubkey = world.permit_pda(&seller.pubkey(), &marketplace);
    assert!(world.svm.get_account(&permit_pubkey).is_none());
}

#[test]
fn test_release_permit_early_requires_both_signatures() {
    let mut world = setup_world();
    let (seller, _token_account) = setup_staked_seller(&mut world, usdc(300), 0);
    let (authority, marketplace) = setup_marketplace(&mut world, 28, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    world.grant_permit(&seller, marketplace, usdc(150)).unwrap();
    world.revoke_permit(&seller, marketplace).unwrap();

    // Seller signs for real, but the marketplace authority is an
    // impostor: has_one on `marketplace` rejects it.
    let authority_impostor = funded_keypair(&mut world);
    let result = world.release_permit_early(&seller, &authority_impostor, marketplace);
    assert_error_code(&result, u32::from(anchor_lang::error::ErrorCode::ConstraintHasOne));
    world.svm.expire_blockhash();

    // The real authority signs for real, but "seller" is an impostor:
    // has_one on `permit` rejects it. Built manually since the
    // convenience method derives permit/stake from whichever seller is
    // passed in, and this scenario needs the REAL permit alongside a
    // fake seller.
    let seller_impostor = funded_keypair(&mut world);
    let stake = world.stake_pda(&seller.pubkey());
    let permit = world.permit_pda(&seller.pubkey(), &marketplace);
    let (event_authority, program) = event_cpi_accounts(&world.program_id);
    let instruction = Instruction::new_with_bytes(
        world.program_id,
        &truststake::instruction::ReleasePermitEarly {}.data(),
        truststake::accounts::ReleasePermitEarlyAccountConstraints {
            seller: seller_impostor.pubkey(),
            authority: authority.pubkey(),
            marketplace,
            permit,
            stake,
            event_authority,
            program,
        }
        .to_account_metas(None),
    );
    let result = world.send_instructions(
        &[instruction],
        &seller_impostor.pubkey(),
        &[&seller_impostor, &authority],
    );
    assert_error_code(&result, u32::from(anchor_lang::error::ErrorCode::ConstraintHasOne));

    // The permit is untouched by both failed attempts.
    let permit_pubkey = world.permit_pda(&seller.pubkey(), &marketplace);
    assert!(world.svm.get_account(&permit_pubkey).is_some());
}

// ---------------------------------------------------------------------
// Arithmetic hardening
// ---------------------------------------------------------------------

#[test]
fn test_max_value_arithmetic() {
    // The claim/payout side needs raise_dispute and is deferred to
    // Phase 3 (see the end-of-task report). This covers the stake/cap
    // side: checked arithmetic must reject an overflow cleanly rather
    // than wrapping, which would silently shrink `committed` below its
    // real value and defeat the cap entirely.
    let mut world = setup_world();
    let (seller, _token_account) = setup_staked_seller(&mut world, u64::MAX, 0);
    let (_authority, marketplace) = setup_marketplace(&mut world, 29, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);

    let near_max = u64::MAX - 100;
    world
        .grant_permit(&seller, marketplace, near_max)
        .expect("grant near u64::MAX succeeds");

    // committed = u64::MAX - 100; a delta of 200 overflows u64
    // arithmetic itself inside checked_add, not merely the staked bound.
    let result = world.increase_permit(&seller, marketplace, 200);
    assert_error_code(&result, u32::from(TrustStakeError::MathOverflow));

    let permit = world.read_permit(&world.permit_pda(&seller.pubkey(), &marketplace));
    assert_eq!(permit.max_slashable, near_max);
    let stake = world.read_seller_stake(&world.stake_pda(&seller.pubkey()));
    assert_eq!(stake.committed, near_max);
}
