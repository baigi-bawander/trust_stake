//! Phase 3 test suite (docs/TESTING.md): every test here belongs to
//! Phase 3 because the last handler it calls is one of the four dispute
//! handlers (`raise_dispute`, `resolve_dispute`, `expire_dispute`,
//! `close_dispute`). The signature-check section came first, before the
//! happy path existed, so the handler was built against the attacks
//! rather than retrofitted to them.

mod common;

use anchor_lang::{prelude::*, InstructionData, ToAccountMetas};
use anchor_spl::token::spl_token::error::TokenError;
use common::{
    assert_error_code, assert_precompile_error, ed25519_verify_instruction, event_cpi_accounts,
    initial_admin_keypair, usdc, World,
};
use solana_account::Account;
use solana_ed25519_program::{
    new_ed25519_instruction_with_signature, DATA_START, PUBKEY_SERIALIZED_SIZE,
    SIGNATURE_OFFSETS_SERIALIZED_SIZE, SIGNATURE_OFFSETS_START, SIGNATURE_SERIALIZED_SIZE,
};
use solana_instruction::{AccountMeta, BorrowedAccountMeta, BorrowedInstruction, Instruction};
use solana_keypair::Keypair;
use solana_message::{Message, VersionedMessage};
use solana_precompile_error::PrecompileError;
use solana_signer::Signer;
use solana_system_interface::error::SystemError;
use solana_transaction::versioned::VersionedTransaction;
use truststake::{
    constants::{
        CLOCK_SKEW_TOLERANCE_SECONDS, DISPUTE_EXPIRY_SECONDS, MAX_BOND_BPS, MAX_COMPLAINT_WINDOW_SECONDS,
        MIN_COMPLAINT_WINDOW_SECONDS, RECEIPT_DOMAIN, SECONDS_PER_DAY,
    },
    error::TrustStakeError,
    receipt::OrderReceipt,
    state::DisputeStatus,
};

const SOL: u64 = 1_000_000_000;
const DEFAULT_CHAIN_ID: u8 = 1;
const DEFAULT_BOND_BPS: u16 = 1_000;

/// Long enough that the permit's complaint window, not the receipt's own
/// expiry, is the binding deadline in every test that is not about the
/// expiry itself.
const RECEIPT_LIFETIME: i64 = 14 * SECONDS_PER_DAY;

/// The wire limit a transaction must fit inside (`PACKET_DATA_SIZE`).
const TRANSACTION_SIZE_LIMIT: usize = 1_232;

fn order_id(tag: u8) -> [u8; 32] {
    [tag; 32]
}

fn funded_keypair(world: &mut World) -> Keypair {
    let keypair = Keypair::new();
    world.svm.airdrop(&keypair.pubkey(), 10 * SOL).expect("airdrop");
    keypair
}

/// A registered marketplace together with the keys a test needs to act as
/// it. The backend key that signs receipts and the key that resolves
/// complaints are separate, which is what the integration requirements
/// ask of a real marketplace (docs/DESIGN-v2.md).
struct Market {
    authority: Keypair,
    receipt_signer: Keypair,
    arbiter: Keypair,
    id: [u8; 16],
    pubkey: Pubkey,
}

/// The state every dispute test starts from: one marketplace, one seller
/// with collateral and a permit, one buyer holding enough to post a bond.
struct Scenario {
    world: World,
    market: Market,
    seller: Keypair,
    seller_token_account: Pubkey,
    buyer: Keypair,
    buyer_token_account: Pubkey,
}

fn setup_world() -> World {
    let mut world = World::new();
    let admin = initial_admin_keypair();
    world.initialize_config(&admin, DEFAULT_CHAIN_ID).unwrap();
    world
}

fn setup_marketplace(world: &mut World, tag: u8, complaint_window: i64, bond_bps: u16) -> Market {
    let authority = funded_keypair(world);
    let receipt_signer = Keypair::new();
    let arbiter = funded_keypair(world);
    let id = [tag; 16];

    world
        .register_marketplace(
            &authority,
            id,
            receipt_signer.pubkey(),
            arbiter.pubkey(),
            complaint_window,
            bond_bps,
        )
        .expect("register_marketplace succeeds");

    Market {
        authority,
        receipt_signer,
        arbiter,
        id,
        pubkey: world.marketplace_pda(&id),
    }
}

fn setup_staked_seller(world: &mut World, staked: u64) -> (Keypair, Pubkey) {
    let seller = funded_keypair(world);
    world.initialize_stake(&seller).expect("initialize_stake succeeds");
    let token_account = world.create_funded_token_account(world.mint, seller.pubkey(), staked);
    if staked > 0 {
        world
            .add_stake(&seller, token_account, staked)
            .expect("add_stake succeeds");
    }
    (seller, token_account)
}

fn setup_buyer(world: &mut World, funds: u64) -> (Keypair, Pubkey) {
    let buyer = funded_keypair(world);
    let token_account = world.create_funded_token_account(world.mint, buyer.pubkey(), funds);
    (buyer, token_account)
}

/// $300 staked behind a $150 permit at a 2-day marketplace charging a 10%
/// bond, and a buyer holding $100. Asymmetric on purpose: no two figures
/// here coincide, so a test cannot pass because two different quantities
/// happen to be equal.
fn scenario(tag: u8) -> Scenario {
    scenario_with(tag, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS, usdc(300), usdc(150))
}

fn scenario_with(tag: u8, complaint_window: i64, bond_bps: u16, staked: u64, permit_cap: u64) -> Scenario {
    scenario_with_buyer_funds(tag, complaint_window, bond_bps, staked, permit_cap, usdc(100))
}

fn scenario_with_buyer_funds(
    tag: u8,
    complaint_window: i64,
    bond_bps: u16,
    staked: u64,
    permit_cap: u64,
    buyer_funds: u64,
) -> Scenario {
    let mut world = setup_world();
    let market = setup_marketplace(&mut world, tag, complaint_window, bond_bps);
    let (seller, seller_token_account) = setup_staked_seller(&mut world, staked);
    world
        .grant_permit(&seller, market.pubkey, permit_cap)
        .expect("grant_permit succeeds");
    let (buyer, buyer_token_account) = setup_buyer(&mut world, buyer_funds);

    Scenario {
        world,
        market,
        seller,
        seller_token_account,
        buyer,
        buyer_token_account,
    }
}

/// A receipt for `amount`, issued now and valid for two weeks, of the
/// shape a marketplace's backend would sign.
fn receipt_for(
    world: &World,
    market: &Market,
    seller: &Keypair,
    buyer: &Keypair,
    order: [u8; 32],
    amount: u64,
) -> OrderReceipt {
    let issued_at = world.now();
    OrderReceipt {
        domain: RECEIPT_DOMAIN,
        program_id: world.program_id,
        chain_id: DEFAULT_CHAIN_ID,
        marketplace_id: market.id,
        seller: seller.pubkey(),
        buyer: buyer.pubkey(),
        order_id: order,
        amount,
        issued_at,
        expires_at: issued_at + RECEIPT_LIFETIME,
    }
}

/// `[num_signatures, padding]` plus the 14-byte offsets struct, which is
/// what a canonical Ed25519 instruction opens with. Hand-built here so
/// the attack tests can put whatever they like in each field; the honest
/// form comes from `solana_ed25519_program` instead.
#[allow(clippy::too_many_arguments)]
fn ed25519_instruction_with_header(
    signature_offset: u16,
    signature_instruction_index: u16,
    public_key_offset: u16,
    public_key_instruction_index: u16,
    message_data_offset: u16,
    message_data_size: u16,
    message_instruction_index: u16,
    body: &[u8],
) -> Instruction {
    let mut data = vec![1u8, 0u8];
    for field in [
        signature_offset,
        signature_instruction_index,
        public_key_offset,
        public_key_instruction_index,
        message_data_offset,
        message_data_size,
        message_instruction_index,
    ] {
        data.extend_from_slice(&field.to_le_bytes());
    }
    data.extend_from_slice(body);

    Instruction {
        program_id: solana_sdk_ids::ed25519_program::ID,
        accounts: vec![],
        data,
    }
}

/// An instruction targeting the fixture program that succeeds whatever it
/// is handed, so a transaction can carry an arbitrary payload at a
/// position where something else is under test.
fn fixture_noop_instruction(payload: Vec<u8>) -> Instruction {
    Instruction::new_with_bytes(
        cpi_wrapper::ID,
        &cpi_wrapper::instruction::Noop { payload }.data(),
        vec![],
    )
}

// ---------------------------------------------------------------------
// Attacks on the signature check
//
// docs/TESTING.md: "A weakness here produces forged marketplace receipts
// with no key compromise at all, which defeats every other protection at
// once."
// ---------------------------------------------------------------------

#[test]
fn test_introspection_rejects_forged_sysvar() {
    // The cheapest attack on the whole program. The attacker builds an
    // account whose data is a genuine Instructions-sysvar serialisation
    // of a transaction that never happened, one whose "Ed25519
    // instruction" verifies a receipt they wrote with a key they own, and
    // passes it in the sysvar slot. The transaction they actually send
    // contains no Ed25519 instruction at all. Everything the handler
    // knows about its own transaction comes out of this account, so the
    // address check is the only thing standing between the attacker and
    // every other check at once.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(1);

    let attacker_signer = Keypair::new();
    let forged_receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xF0), usdc(80));
    let fabricated_verify = ed25519_verify_instruction(&forged_receipt, &attacker_signer);

    let forged_sysvar_address = Pubkey::new_unique();
    let raise_instruction = world.raise_dispute_instruction_with_sysvar(
        buyer.pubkey(),
        buyer_token_account,
        market.pubkey,
        seller.pubkey(),
        forged_receipt.order_id,
        usdc(80),
        forged_sysvar_address,
    );

    // Serialised exactly as the runtime serialises the real sysvar, so
    // nothing except the address distinguishes it.
    let borrowed = [
        BorrowedInstruction {
            program_id: &fabricated_verify.program_id,
            accounts: vec![],
            data: &fabricated_verify.data,
        },
        BorrowedInstruction {
            program_id: &raise_instruction.program_id,
            accounts: raise_instruction
                .accounts
                .iter()
                .map(|meta| BorrowedAccountMeta {
                    pubkey: &meta.pubkey,
                    is_signer: meta.is_signer,
                    is_writable: meta.is_writable,
                })
                .collect(),
            data: &raise_instruction.data,
        },
    ];
    let mut fabricated_data = solana_instructions_sysvar::construct_instructions_data(&borrowed);
    solana_instructions_sysvar::store_current_index_checked(&mut fabricated_data, 1)
        .expect("stamp the current instruction index");

    world
        .svm
        .set_account(
            forged_sysvar_address,
            Account {
                lamports: 10 * SOL,
                data: fabricated_data,
                owner: attacker_signer.pubkey(),
                executable: false,
                rent_epoch: 0,
            },
        )
        .expect("plant the forged sysvar account");

    let result = world.send_raise_dispute(
        &[raise_instruction],
        &buyer,
        market.pubkey,
        seller.pubkey(),
        forged_receipt.order_id,
    );
    assert_error_code(&result, u32::from(TrustStakeError::InvalidInstructionsSysvar));

    assert!(
        world
            .svm
            .get_account(&world.dispute_pda(&market.pubkey, &seller.pubkey(), &forged_receipt.order_id))
            .is_none(),
        "no dispute record may exist after the forged sysvar is rejected"
    );
}

#[test]
fn test_introspection_rejects_crossed_indices() {
    // The attack the design doc calls the most exploitable surface in the
    // program, and the one checking byte offsets alone does not catch.
    //
    // The transaction is [ed25519 verify, raise_dispute, fixture noop].
    // The Ed25519 instruction's three instruction indices all point at
    // the noop, where the attacker has placed their own throwaway key,
    // their own message, and a genuine signature over it. The precompile
    // therefore verifies something real and the transaction is valid.
    // Meanwhile the Ed25519 instruction's own body, at the canonical
    // offsets this handler reads, holds the marketplace's real receipt
    // signer and a receipt nobody ever signed.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(2);

    let forged_receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xC0), usdc(80));

    // The decoy the precompile is pointed at: the attacker signs the very
    // receipt they want honoured, with a key of their own, and lays the
    // bytes out inside the noop instruction so that they sit at exactly
    // the canonical offsets. An 8-byte Anchor discriminator and a 4-byte
    // Borsh vector length come first, so four bytes of filler put the
    // public key at offset 16, the signature at 48 and the message at
    // 112, byte for byte where a real Ed25519 instruction keeps them.
    let throwaway = Keypair::new();
    let decoy_message = forged_receipt.message();
    let decoy_signature: [u8; 64] = throwaway.sign_message(&decoy_message).into();
    let mut decoy_payload = vec![0u8; DATA_START - (8 + 4)];
    decoy_payload.extend_from_slice(&throwaway.pubkey().to_bytes());
    decoy_payload.extend_from_slice(&decoy_signature);
    decoy_payload.extend_from_slice(&decoy_message);
    let decoy_instruction = fixture_noop_instruction(decoy_payload);
    const DECOY_INDEX: u16 = 2;

    // Every offset in the header below is canonical, so an offsets-only
    // check passes. What the handler would read from those offsets, in
    // the Ed25519 instruction's own body, is the marketplace's real
    // signer next to a receipt no key ever signed.
    let mut body = Vec::new();
    body.extend_from_slice(&market.receipt_signer.pubkey().to_bytes());
    body.extend_from_slice(&[0u8; SIGNATURE_SERIALIZED_SIZE]);
    body.extend_from_slice(&forged_receipt.message());

    let crossed_verify = ed25519_instruction_with_header(
        (DATA_START + PUBKEY_SERIALIZED_SIZE) as u16,
        DECOY_INDEX,
        DATA_START as u16,
        DECOY_INDEX,
        (DATA_START + PUBKEY_SERIALIZED_SIZE + SIGNATURE_SERIALIZED_SIZE) as u16,
        OrderReceipt::INIT_SPACE as u16,
        DECOY_INDEX,
        &body,
    );

    let raise_instruction = world.raise_dispute_instruction(
        buyer.pubkey(),
        buyer_token_account,
        market.pubkey,
        seller.pubkey(),
        forged_receipt.order_id,
        usdc(80),
    );

    let result = world.send_raise_dispute(
        &[crossed_verify.clone(), raise_instruction, decoy_instruction.clone()],
        &buyer,
        market.pubkey,
        seller.pubkey(),
        forged_receipt.order_id,
    );
    assert_error_code(&result, u32::from(TrustStakeError::MalformedEd25519Instruction));

    // Nothing but this program's index check stands in the way: with
    // `raise_dispute` swapped out for a filler instruction, so the decoy
    // still sits at index 2, the same crossed-index verification is
    // accepted by the precompile and the transaction succeeds.
    world.svm.expire_blockhash();
    world
        .send_instructions(
            &[crossed_verify, fixture_noop_instruction(Vec::new()), decoy_instruction],
            &buyer.pubkey(),
            &[&buyer],
        )
        .expect("the crossed-index signature genuinely verifies");
}

#[test]
fn test_introspection_rejects_missing_ed25519() {
    // A transaction with instructions in it, but no verification
    // anywhere: the neighbour is the fixture's no-op.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(3);

    let order = order_id(0x30);
    let raise_instruction = world.raise_dispute_instruction(
        buyer.pubkey(),
        buyer_token_account,
        market.pubkey,
        seller.pubkey(),
        order,
        usdc(80),
    );

    let result = world.send_raise_dispute(
        &[fixture_noop_instruction(Vec::new()), raise_instruction],
        &buyer,
        market.pubkey,
        seller.pubkey(),
        order,
    );
    assert_error_code(&result, u32::from(TrustStakeError::MissingEd25519Instruction));
}

#[test]
fn test_introspection_rejects_wrong_program() {
    // An instruction whose payload is a byte-for-byte copy of a genuine
    // Ed25519 verify instruction's data, submitted to a program that is
    // not the precompile. A check that scanned instruction data for the
    // right shape would be fooled; checking the program ID is what is not.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(4);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0x40), usdc(80));
    let genuine_verify = ed25519_verify_instruction(&receipt, &market.receipt_signer);
    let impostor = fixture_noop_instruction(genuine_verify.data.clone());

    let raise_instruction = world.raise_dispute_instruction(
        buyer.pubkey(),
        buyer_token_account,
        market.pubkey,
        seller.pubkey(),
        receipt.order_id,
        usdc(80),
    );

    let result = world.send_raise_dispute(
        &[impostor, raise_instruction],
        &buyer,
        market.pubkey,
        seller.pubkey(),
        receipt.order_id,
    );
    assert_error_code(&result, u32::from(TrustStakeError::MissingEd25519Instruction));
}

#[test]
fn test_introspection_rejects_different_message() {
    // Two ways the bytes that were verified can differ from the bytes the
    // handler acts on, and both have to fail.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(5);

    // One: a real verification of a real receipt, by the marketplace's
    // own key, over a message that is not the one sitting where this
    // handler reads. Only the message index is crossed here, so every
    // offset in the header is canonical and the pubkey and signature are
    // the instruction's own -- the narrowest possible version of the
    // attack, and the one an offsets-only check cannot see.
    let decoy_receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0x50), usdc(80));
    let decoy_message = decoy_receipt.message();
    let throwaway = Keypair::new();
    let decoy_signature: [u8; 64] = throwaway.sign_message(&decoy_message).into();

    // Padded so the decoy message begins at offset 112 of the noop's own
    // data, matching the canonical message offset exactly.
    let message_offset = DATA_START + PUBKEY_SERIALIZED_SIZE + SIGNATURE_SERIALIZED_SIZE;
    let mut decoy_payload = vec![0u8; message_offset - (8 + 4)];
    decoy_payload.extend_from_slice(&decoy_message);
    let decoy_instruction = fixture_noop_instruction(decoy_payload);

    let unsigned_receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0x51), usdc(80));
    let mut body = Vec::new();
    body.extend_from_slice(&throwaway.pubkey().to_bytes());
    body.extend_from_slice(&decoy_signature);
    body.extend_from_slice(&unsigned_receipt.message());

    let crossed_message_verify = ed25519_instruction_with_header(
        (DATA_START + PUBKEY_SERIALIZED_SIZE) as u16,
        u16::MAX,
        DATA_START as u16,
        u16::MAX,
        message_offset as u16,
        OrderReceipt::INIT_SPACE as u16,
        2,
        &body,
    );
    let raise_instruction = world.raise_dispute_instruction(
        buyer.pubkey(),
        buyer_token_account,
        market.pubkey,
        seller.pubkey(),
        unsigned_receipt.order_id,
        usdc(80),
    );
    let result = world.send_raise_dispute(
        &[crossed_message_verify, raise_instruction, decoy_instruction],
        &buyer,
        market.pubkey,
        seller.pubkey(),
        unsigned_receipt.order_id,
    );
    assert_error_code(&result, u32::from(TrustStakeError::MalformedEd25519Instruction));

    // Two: nothing crossed at all, but the receipt that was signed is for
    // a different order than the one being disputed. The handler acts on
    // what the message says, never on what the instruction's arguments
    // claim.
    world.svm.expire_blockhash();
    let signed_receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0x52), usdc(80));
    let result = world.raise_dispute_against(
        &buyer,
        buyer_token_account,
        market.pubkey,
        seller.pubkey(),
        order_id(0x53),
        &signed_receipt,
        &market.receipt_signer,
        usdc(80),
    );
    assert_error_code(&result, u32::from(TrustStakeError::ReceiptOrderMismatch));
}

#[test]
fn test_introspection_accepts_self_referential_indices() {
    // The precompile reads `u16::MAX` as "this instruction's own data"
    // and any other value as an absolute index, so an instruction that
    // names its own position is the same instruction by a different
    // spelling. Both spellings say the thing the check is there to
    // require, so both are accepted; this pins the one the standard
    // builder does not emit.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(58);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0x54), usdc(80));
    let message = receipt.message();
    let signature: [u8; 64] = market.receipt_signer.sign_message(&message).into();
    let mut body = Vec::new();
    body.extend_from_slice(&market.receipt_signer.pubkey().to_bytes());
    body.extend_from_slice(&signature);
    body.extend_from_slice(&message);

    // Index 0: where this instruction itself sits in the transaction.
    let self_indexed_verify = ed25519_instruction_with_header(
        (DATA_START + PUBKEY_SERIALIZED_SIZE) as u16,
        0,
        DATA_START as u16,
        0,
        (DATA_START + PUBKEY_SERIALIZED_SIZE + SIGNATURE_SERIALIZED_SIZE) as u16,
        OrderReceipt::INIT_SPACE as u16,
        0,
        &body,
    );
    let raise_instruction = world.raise_dispute_instruction(
        buyer.pubkey(),
        buyer_token_account,
        market.pubkey,
        seller.pubkey(),
        receipt.order_id,
        usdc(80),
    );

    world
        .send_raise_dispute(
            &[self_indexed_verify, raise_instruction],
            &buyer,
            market.pubkey,
            seller.pubkey(),
            receipt.order_id,
        )
        .expect("indices naming the Ed25519 instruction's own position are accepted");
}

#[test]
fn test_introspection_rejects_multiple_signatures() {
    // The canonical form this handler reads carries exactly one
    // signature. A second offsets entry would sit where the public key is
    // expected, so the count is asserted rather than inferred from what
    // happens to parse.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(59);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0x55), usdc(80));
    let message = receipt.message();
    let signature: [u8; 64] = market.receipt_signer.sign_message(&message).into();

    // Two well-formed entries verifying the same real signature, so the
    // precompile is satisfied and the transaction reaches the handler.
    // Both entries sit before the body, which puts every offset 14 bytes
    // further along than the canonical layout.
    let public_key_offset = (SIGNATURE_OFFSETS_START + 2 * SIGNATURE_OFFSETS_SERIALIZED_SIZE) as u16;
    let signature_offset = public_key_offset + PUBKEY_SERIALIZED_SIZE as u16;
    let message_offset = signature_offset + SIGNATURE_SERIALIZED_SIZE as u16;
    let mut data = vec![2u8, 0u8];
    for _ in 0..2 {
        for field in [
            signature_offset,
            u16::MAX,
            public_key_offset,
            u16::MAX,
            message_offset,
            message.len() as u16,
            u16::MAX,
        ] {
            data.extend_from_slice(&field.to_le_bytes());
        }
    }
    data.extend_from_slice(&market.receipt_signer.pubkey().to_bytes());
    data.extend_from_slice(&signature);
    data.extend_from_slice(&message);

    let verify_instruction = Instruction {
        program_id: solana_sdk_ids::ed25519_program::ID,
        accounts: vec![],
        data,
    };
    let raise_instruction = world.raise_dispute_instruction(
        buyer.pubkey(),
        buyer_token_account,
        market.pubkey,
        seller.pubkey(),
        receipt.order_id,
        usdc(80),
    );

    let result = world.send_raise_dispute(
        &[verify_instruction, raise_instruction],
        &buyer,
        market.pubkey,
        seller.pubkey(),
        receipt.order_id,
    );
    assert_error_code(&result, u32::from(TrustStakeError::MalformedEd25519Instruction));
}

#[test]
fn test_introspection_rejects_unexpected_index() {
    // A real verification, by the real key, over the real receipt, but
    // one instruction further away than `raise_dispute` looks. The
    // position is derived from the current index rather than searched
    // for, so nothing is found.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(6);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0x60), usdc(80));
    let verify_instruction = ed25519_verify_instruction(&receipt, &market.receipt_signer);
    let raise_instruction = world.raise_dispute_instruction(
        buyer.pubkey(),
        buyer_token_account,
        market.pubkey,
        seller.pubkey(),
        receipt.order_id,
        usdc(80),
    );

    let result = world.send_raise_dispute(
        &[verify_instruction, fixture_noop_instruction(Vec::new()), raise_instruction],
        &buyer,
        market.pubkey,
        seller.pubkey(),
        receipt.order_id,
    );
    assert_error_code(&result, u32::from(TrustStakeError::MissingEd25519Instruction));
}

#[test]
fn test_introspection_rejects_out_of_bounds_offsets() {
    // Offsets that run past the end of the instruction data. Two layers
    // catch this and the outer one fires first: the precompile bounds-
    // checks its own reads, so the transaction dies at instruction 0
    // before this program runs. That is why the offsets this handler
    // slices at can never be out of range -- it pins every one of them to
    // a constant first -- and why the slicing still goes through checked
    // `get` rather than indexing. The layer inside the handler is
    // exercised by test_introspection_rejects_wrong_message_length, where
    // the precompile is satisfied and the length check is what refuses.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(7);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0x70), usdc(80));
    let body = {
        let mut body = Vec::new();
        body.extend_from_slice(&market.receipt_signer.pubkey().to_bytes());
        body.extend_from_slice(&[0u8; SIGNATURE_SERIALIZED_SIZE]);
        body.extend_from_slice(&receipt.message());
        body
    };
    let message_offset = (DATA_START + PUBKEY_SERIALIZED_SIZE + SIGNATURE_SERIALIZED_SIZE) as u16;
    let out_of_bounds_verify = ed25519_instruction_with_header(
        (DATA_START + PUBKEY_SERIALIZED_SIZE) as u16,
        u16::MAX,
        DATA_START as u16,
        u16::MAX,
        message_offset,
        // One byte more message than the instruction actually carries.
        OrderReceipt::INIT_SPACE as u16 + 1,
        u16::MAX,
        &body,
    );

    let raise_instruction = world.raise_dispute_instruction(
        buyer.pubkey(),
        buyer_token_account,
        market.pubkey,
        seller.pubkey(),
        receipt.order_id,
        usdc(80),
    );

    let result = world.send_raise_dispute(
        &[out_of_bounds_verify, raise_instruction],
        &buyer,
        market.pubkey,
        seller.pubkey(),
        receipt.order_id,
    );
    assert_precompile_error(&result, PrecompileError::InvalidDataOffsets);
}

#[test]
fn test_introspection_rejects_wrong_message_length() {
    // A message the marketplace really did sign, of a length that is not
    // a receipt's. The precompile is perfectly happy with it, so this is
    // the handler's own length assertion refusing, and the reason it has
    // to be exact rather than a minimum: a longer message would otherwise
    // deserialise from its first 190 bytes and carry arbitrary trailing
    // bytes nobody checked.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(8);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0x80), usdc(80));

    for message in [
        {
            let mut longer = receipt.message();
            longer.push(0);
            longer
        },
        {
            let mut shorter = receipt.message();
            shorter.pop();
            shorter
        },
    ] {
        let signature: [u8; 64] = market.receipt_signer.sign_message(&message).into();
        let verify_instruction = new_ed25519_instruction_with_signature(
            &message,
            &signature,
            &market.receipt_signer.pubkey().to_bytes(),
        );
        let raise_instruction = world.raise_dispute_instruction(
            buyer.pubkey(),
            buyer_token_account,
            market.pubkey,
            seller.pubkey(),
            receipt.order_id,
            usdc(80),
        );

        let result = world.send_raise_dispute(
            &[verify_instruction, raise_instruction],
            &buyer,
            market.pubkey,
            seller.pubkey(),
            receipt.order_id,
        );
        assert_error_code(&result, u32::from(TrustStakeError::MalformedEd25519Instruction));
        world.svm.expire_blockhash();
    }
}

#[test]
fn test_introspection_rejects_cpi_wrapper() {
    // `raise_dispute` reached through a wrapper program rather than
    // directly. The Instructions sysvar describes top-level instructions
    // only, so what sits next to the wrapper's call says nothing about
    // what sits next to this handler's; the handler refuses rather than
    // reasoning about a neighbour it cannot see.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(9);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0x90), usdc(80));
    let verify_instruction = ed25519_verify_instruction(&receipt, &market.receipt_signer);
    let raise_instruction = world.raise_dispute_instruction(
        buyer.pubkey(),
        buyer_token_account,
        market.pubkey,
        seller.pubkey(),
        receipt.order_id,
        usdc(80),
    );

    // The wrapper takes the target program first and forwards every
    // remaining account, flags intact.
    let mut wrapper_accounts = vec![AccountMeta::new_readonly(world.program_id, false)];
    wrapper_accounts.extend(raise_instruction.accounts.iter().cloned());
    let wrapper_instruction = Instruction::new_with_bytes(
        cpi_wrapper::ID,
        &cpi_wrapper::instruction::Forward {
            data: raise_instruction.data.clone(),
        }
        .data(),
        wrapper_accounts,
    );

    let result = world.send_raise_dispute(
        &[verify_instruction, wrapper_instruction],
        &buyer,
        market.pubkey,
        seller.pubkey(),
        receipt.order_id,
    );
    assert_error_code(&result, u32::from(TrustStakeError::MustBeTopLevelInstruction));
}

#[test]
fn test_introspection_rejects_missing_domain_tag() {
    // A marketplace's receipt signer is an ordinary Ed25519 key that may
    // sign other things. The domain prefix is what stops one of those
    // other signatures being presented here as a receipt, so both the
    // shape without the prefix and the shape with the wrong prefix have
    // to fail.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(10);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xA0), usdc(80));

    // The same fields, signed without the domain prefix in front of them.
    let undomained = receipt.message()[RECEIPT_DOMAIN.len()..].to_vec();
    let signature: [u8; 64] = market.receipt_signer.sign_message(&undomained).into();
    let verify_instruction = new_ed25519_instruction_with_signature(
        &undomained,
        &signature,
        &market.receipt_signer.pubkey().to_bytes(),
    );
    let raise_instruction = world.raise_dispute_instruction(
        buyer.pubkey(),
        buyer_token_account,
        market.pubkey,
        seller.pubkey(),
        receipt.order_id,
        usdc(80),
    );
    let result = world.send_raise_dispute(
        &[verify_instruction, raise_instruction],
        &buyer,
        market.pubkey,
        seller.pubkey(),
        receipt.order_id,
    );
    assert_error_code(&result, u32::from(TrustStakeError::MalformedEd25519Instruction));
    world.svm.expire_blockhash();

    // Right length, wrong prefix: now the domain check itself is what
    // refuses, rather than the length.
    let mut wrong_domain = receipt.clone();
    wrong_domain.domain = *b"truststake:receipt:v9";
    let result = world.raise_dispute(
        &buyer,
        buyer_token_account,
        market.pubkey,
        &wrong_domain,
        &market.receipt_signer,
        usdc(80),
    );
    assert_error_code(&result, u32::from(TrustStakeError::WrongReceiptDomain));
}

// ---------------------------------------------------------------------
// raise_dispute, the happy path
// ---------------------------------------------------------------------

#[test]
fn test_raise_dispute_succeeds() {
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(11);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xB0), usdc(80));
    let filed_at = world.now();
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .expect("raise_dispute succeeds");

    let dispute = world.read_dispute(&world.dispute_pda(&market.pubkey, &seller.pubkey(), &receipt.order_id));
    assert_eq!(dispute.version, 1);
    assert_eq!(dispute.marketplace, market.pubkey);
    assert_eq!(dispute.seller, seller.pubkey());
    assert_eq!(dispute.buyer, buyer.pubkey());
    assert_eq!(dispute.order_id, receipt.order_id);
    assert_eq!(dispute.claim, usdc(80));
    // 10% of $80, and the buyer paid exactly that.
    assert_eq!(dispute.bond, usdc(8));
    assert_eq!(dispute.created_at, filed_at);
    assert_eq!(dispute.expires_at, filed_at + DISPUTE_EXPIRY_SECONDS);
    // Bound to the protocol-wide maximum, not this marketplace's own
    // (shorter) window: see raise_dispute's handler doc comment.
    assert_eq!(dispute.closable_after, receipt.issued_at + MAX_COMPLAINT_WINDOW_SECONDS);
    assert_eq!(dispute.status, DisputeStatus::Open as u8);

    assert_eq!(world.token_balance(&buyer_token_account), usdc(92));
    assert_eq!(world.token_balance(&world.bond_vault_pda(&market.pubkey)), usdc(8));

    // The freeze lands on this permit only, and both denominators count
    // at raise time rather than at resolution.
    let permit = world.read_permit(&world.permit_pda(&seller.pubkey(), &market.pubkey));
    assert_eq!(permit.open_disputes, 1);
    assert_eq!(permit.slashed, 0);
    let stake = world.read_seller_stake(&world.stake_pda(&seller.pubkey()));
    assert_eq!(stake.disputes_total, 1);
    assert_eq!(stake.disputes_lost, 0);
    assert_eq!(stake.staked, usdc(300));
    assert_eq!(stake.committed, usdc(150));
    let marketplace = world.read_marketplace(&market.pubkey);
    assert_eq!(marketplace.disputes_total, 1);
    assert_eq!(marketplace.disputes_upheld, 0);
    assert_eq!(marketplace.disputes_abandoned, 0);
}

// ---------------------------------------------------------------------
// Attacks by a malicious buyer
// ---------------------------------------------------------------------

#[test]
fn test_dispute_requires_receipt() {
    // Filing with nothing but the instruction itself: no signature, no
    // receipt, no proof a purchase ever happened. This is the check that
    // makes a stranger unable to touch a seller's collateral at all.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(12);

    let order = order_id(0xC1);
    let raise_instruction = world.raise_dispute_instruction(
        buyer.pubkey(),
        buyer_token_account,
        market.pubkey,
        seller.pubkey(),
        order,
        usdc(80),
    );

    let result = world.send_raise_dispute(&[raise_instruction], &buyer, market.pubkey, seller.pubkey(), order);
    assert_error_code(&result, u32::from(TrustStakeError::MissingEd25519Instruction));
}

#[test]
fn test_dispute_rejects_forged_signature() {
    // A perfectly valid signature, over a perfectly well-formed receipt,
    // by a key that is not this marketplace's receipt signer.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(13);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xC2), usdc(80));
    let impostor = Keypair::new();

    let result = world.raise_dispute(
        &buyer,
        buyer_token_account,
        market.pubkey,
        &receipt,
        &impostor,
        usdc(80),
    );
    assert_error_code(&result, u32::from(TrustStakeError::WrongReceiptSigner));
}

#[test]
fn test_dispute_rejects_tampered_amount() {
    // The buyer takes a genuine $20 receipt and edits it into a $200 one
    // after the fact. The signature covers every byte of the message, so
    // the precompile refuses before this program is ever reached.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(14);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xC3), usdc(20));
    let mut verify_instruction = ed25519_verify_instruction(&receipt, &market.receipt_signer);

    // Overwrite the amount inside the signed message with $200, leaving
    // the signature as it was.
    let amount_offset = DATA_START
        + PUBKEY_SERIALIZED_SIZE
        + SIGNATURE_SERIALIZED_SIZE
        + (RECEIPT_DOMAIN.len() + 32 + 1 + 16 + 32 + 32 + 32);
    verify_instruction.data[amount_offset..amount_offset + 8].copy_from_slice(&usdc(200).to_le_bytes());

    let raise_instruction = world.raise_dispute_instruction(
        buyer.pubkey(),
        buyer_token_account,
        market.pubkey,
        seller.pubkey(),
        receipt.order_id,
        usdc(200),
    );

    let result = world.send_raise_dispute(
        &[verify_instruction, raise_instruction],
        &buyer,
        market.pubkey,
        seller.pubkey(),
        receipt.order_id,
    );
    assert_precompile_error(&result, PrecompileError::InvalidSignature);
}

#[test]
fn test_dispute_rejects_expired_receipt() {
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(15);

    let mut receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xC4), usdc(80));
    // Expiring exactly now is expired: the check is `expires_at > now`.
    receipt.expires_at = world.now();

    let result = world.raise_dispute(
        &buyer,
        buyer_token_account,
        market.pubkey,
        &receipt,
        &market.receipt_signer,
        usdc(80),
    );
    assert_error_code(&result, u32::from(TrustStakeError::ReceiptExpired));

    // One second of life left, and it files.
    world.svm.expire_blockhash();
    receipt.expires_at = world.now() + 1;
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .expect("a receipt one second from expiry still files");
}

#[test]
fn test_dispute_rejects_receipt_issued_in_future() {
    // `receipt.expires_at > now` and `now < issued_at + complaint_window`
    // both pass for a receipt dated years ahead: the first because its
    // expiry is computed from that same future `issued_at`, the second
    // because a future `issued_at` only pushes the window further out.
    // Nothing before this check bounds `issued_at` against the wall
    // clock at all, so a receipt dated years ahead would push
    // `closable_after` out by the same margin and make
    // `MAX_COMPLAINT_WINDOW_SECONDS` meaningless as a real deadline.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(58);

    let now = world.now();
    let mut receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xE1), usdc(80));

    // Years ahead: comfortably outside any reasonable clock skew.
    receipt.issued_at = now + 400 * SECONDS_PER_DAY;
    receipt.expires_at = receipt.issued_at + RECEIPT_LIFETIME;
    let result = world.raise_dispute(
        &buyer,
        buyer_token_account,
        market.pubkey,
        &receipt,
        &market.receipt_signer,
        usdc(80),
    );
    assert_error_code(&result, u32::from(TrustStakeError::ReceiptIssuedInFuture));

    // One second past the tolerance: still refused.
    world.svm.expire_blockhash();
    receipt.issued_at = now + CLOCK_SKEW_TOLERANCE_SECONDS + 1;
    receipt.expires_at = receipt.issued_at + RECEIPT_LIFETIME;
    let result = world.raise_dispute(
        &buyer,
        buyer_token_account,
        market.pubkey,
        &receipt,
        &market.receipt_signer,
        usdc(80),
    );
    assert_error_code(&result, u32::from(TrustStakeError::ReceiptIssuedInFuture));

    // Exactly at the tolerance: an honest clock-skew case, and it files.
    world.svm.expire_blockhash();
    receipt.issued_at = now + CLOCK_SKEW_TOLERANCE_SECONDS;
    receipt.expires_at = receipt.issued_at + RECEIPT_LIFETIME;
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .expect("a receipt exactly at the clock-skew tolerance still files");
}

#[test]
fn test_dispute_rejects_receipt_outside_window() {
    // The window that matters is the one frozen on the permit when the
    // seller signed it, never the marketplace's current setting. A
    // marketplace that stretches its window afterwards must not be able
    // to reach back and revive receipts the seller's permit had already
    // aged out.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(16);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xC5), usdc(80));

    world
        .update_marketplace(
            &market.authority,
            market.pubkey,
            None,
            None,
            Some(30 * SECONDS_PER_DAY),
            None,
        )
        .expect("update_marketplace succeeds");
    assert_eq!(
        world.read_marketplace(&market.pubkey).complaint_window,
        30 * SECONDS_PER_DAY
    );

    // Three days on: inside the marketplace's new 30-day window, outside
    // the permit's frozen 2-day one.
    world.warp_seconds(3 * SECONDS_PER_DAY);

    let result = world.raise_dispute(
        &buyer,
        buyer_token_account,
        market.pubkey,
        &receipt,
        &market.receipt_signer,
        usdc(80),
    );
    assert_error_code(&result, u32::from(TrustStakeError::ComplaintWindowClosed));
}

#[test]
fn test_dispute_rejects_wrong_buyer() {
    // A leaked receipt must be worthless to whoever finds it: the receipt
    // names its buyer and the transaction's signer has to be that buyer.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        ..
    } = scenario(17);

    let (interloper, interloper_token_account) = setup_buyer(&mut world, usdc(100));
    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xC6), usdc(80));

    let result = world.raise_dispute(
        &interloper,
        interloper_token_account,
        market.pubkey,
        &receipt,
        &market.receipt_signer,
        usdc(80),
    );
    assert_error_code(&result, u32::from(TrustStakeError::ReceiptBuyerMismatch));
}

#[test]
fn test_dispute_rejects_wrong_seller() {
    // A valid receipt against one seller, filed against a different
    // seller's collateral. Both sellers hold permits at this
    // marketplace, so the only thing that separates them is the receipt.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(18);

    let (other_seller, _other_token_account) = setup_staked_seller(&mut world, usdc(300));
    world
        .grant_permit(&other_seller, market.pubkey, usdc(150))
        .expect("grant_permit succeeds");

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xC7), usdc(80));

    let result = world.raise_dispute_against(
        &buyer,
        buyer_token_account,
        market.pubkey,
        other_seller.pubkey(),
        receipt.order_id,
        &receipt,
        &market.receipt_signer,
        usdc(80),
    );
    assert_error_code(&result, u32::from(TrustStakeError::ReceiptSellerMismatch));
}

#[test]
fn test_dispute_rejects_cross_marketplace_receipt() {
    // Two marketplaces run by one operator, sharing a receipt signer:
    // that shared key is what makes this the marketplace-ID check being
    // tested rather than the signer check. A receipt from the first must
    // not reach the permit the seller granted the second.
    let mut world = setup_world();
    let first = setup_marketplace(&mut world, 19, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    let second = setup_marketplace(&mut world, 20, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    world
        .update_marketplace(
            &second.authority,
            second.pubkey,
            Some(first.receipt_signer.pubkey()),
            None,
            None,
            None,
        )
        .expect("update_marketplace succeeds");

    let (seller, _seller_token_account) = setup_staked_seller(&mut world, usdc(300));
    world.grant_permit(&seller, first.pubkey, usdc(150)).unwrap();
    world.grant_permit(&seller, second.pubkey, usdc(100)).unwrap();
    let (buyer, buyer_token_account) = setup_buyer(&mut world, usdc(100));

    let receipt = receipt_for(&world, &first, &seller, &buyer, order_id(0xC8), usdc(80));

    let result = world.raise_dispute(
        &buyer,
        buyer_token_account,
        second.pubkey,
        &receipt,
        &first.receipt_signer,
        usdc(80),
    );
    assert_error_code(&result, u32::from(TrustStakeError::ReceiptMarketplaceMismatch));
}

#[test]
fn test_dispute_rejects_claim_above_receipt() {
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(21);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xC9), usdc(80));

    // One cent over the order's value.
    let result = world.raise_dispute(
        &buyer,
        buyer_token_account,
        market.pubkey,
        &receipt,
        &market.receipt_signer,
        usdc(80) + 1,
    );
    assert_error_code(&result, u32::from(TrustStakeError::ClaimExceedsReceipt));

    // Exactly the order's value files: the bound is inclusive.
    world.svm.expire_blockhash();
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .expect("a claim for the whole order files");
}

#[test]
fn test_dispute_rejects_wrong_chain_id() {
    // Without this, a devnet signature is replayable on mainnet.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(22);

    let mut receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xCA), usdc(80));
    receipt.chain_id = DEFAULT_CHAIN_ID + 1;

    let result = world.raise_dispute(
        &buyer,
        buyer_token_account,
        market.pubkey,
        &receipt,
        &market.receipt_signer,
        usdc(80),
    );
    assert_error_code(&result, u32::from(TrustStakeError::WrongChainId));
}

#[test]
fn test_dispute_rejects_wrong_program_id() {
    // The other half of domain separation: a receipt naming a different
    // program is not a receipt for this one, even on the same cluster.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(23);

    let mut receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xCB), usdc(80));
    receipt.program_id = Pubkey::new_unique();

    let result = world.raise_dispute(
        &buyer,
        buyer_token_account,
        market.pubkey,
        &receipt,
        &market.receipt_signer,
        usdc(80),
    );
    assert_error_code(&result, u32::from(TrustStakeError::WrongReceiptProgram));
}

#[test]
fn test_dispute_replay_blocked() {
    // The record's own address is the replay guard, so the second filing
    // collides with an account that already exists -- before resolution
    // and after it alike.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(24);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xCC), usdc(80));
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .expect("the first filing succeeds");

    world.svm.expire_blockhash();
    let result = world.raise_dispute(
        &buyer,
        buyer_token_account,
        market.pubkey,
        &receipt,
        &market.receipt_signer,
        usdc(80),
    );
    assert_error_code(&result, SystemError::AccountAlreadyInUse as u32);

    world
        .resolve_dispute(&market.arbiter, market.pubkey, receipt.order_id, buyer_token_account, false)
        .expect("resolve_dispute succeeds");

    world.svm.expire_blockhash();
    let result = world.raise_dispute(
        &buyer,
        buyer_token_account,
        market.pubkey,
        &receipt,
        &market.receipt_signer,
        usdc(80),
    );
    assert_error_code(&result, SystemError::AccountAlreadyInUse as u32);
}

#[test]
fn test_dispute_seed_includes_seller() {
    // Regression for a collision the dispute PDA's old seed allowed: two
    // sellers on the same marketplace filing under the same order_id --
    // the natural outcome of per-seller order numbering, which nothing in
    // the protocol forbids -- used to share one DisputeRecord address.
    // Without the seller in the seed, the first buyer to file freezes
    // that address and the second buyer's genuine, unrelated complaint is
    // refused, permanently: the address only frees at
    // `issued_at + MAX_COMPLAINT_WINDOW_SECONDS`, while filing requires
    // `now < issued_at + complaint_window`, so by the time the address is
    // free every receipt that could have used it is already out of
    // window. Both complaints here must succeed.
    let mut world = setup_world();
    let market = setup_marketplace(&mut world, 70, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);

    let (first_seller, _first_seller_token_account) = setup_staked_seller(&mut world, usdc(300));
    world
        .grant_permit(&first_seller, market.pubkey, usdc(150))
        .expect("grant_permit succeeds for the first seller");
    let (second_seller, _second_seller_token_account) = setup_staked_seller(&mut world, usdc(300));
    world
        .grant_permit(&second_seller, market.pubkey, usdc(150))
        .expect("grant_permit succeeds for the second seller");

    let (first_buyer, first_buyer_token_account) = setup_buyer(&mut world, usdc(100));
    let (second_buyer, second_buyer_token_account) = setup_buyer(&mut world, usdc(100));

    let shared_order = order_id(0x70);
    let first_receipt = receipt_for(&world, &market, &first_seller, &first_buyer, shared_order, usdc(80));
    let second_receipt = receipt_for(&world, &market, &second_seller, &second_buyer, shared_order, usdc(80));

    world
        .raise_dispute(
            &first_buyer,
            first_buyer_token_account,
            market.pubkey,
            &first_receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .expect("the first seller's complaint succeeds");

    world
        .raise_dispute(
            &second_buyer,
            second_buyer_token_account,
            market.pubkey,
            &second_receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .expect("a different seller's complaint against the same order_id must not collide with the first");
}

#[test]
fn test_dispute_replay_blocked_after_close() {
    // The test that proves deleting records is safe. Once the record is
    // gone the address is free again, and the only thing left stopping
    // the same receipt from being filed a second time is that it has aged
    // out of its complaint window -- which is exactly the moment
    // `close_dispute` waits for. `closable_after` is bound to
    // `MAX_COMPLAINT_WINDOW_SECONDS`, not this marketplace's 2-day
    // window (see raise_dispute's handler doc comment), so the receipt
    // is given a longer life than that bound: otherwise its own expiry,
    // not the window this test is about, would be what blocks the
    // replay.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(25);

    let receipt = OrderReceipt {
        expires_at: world.now() + MAX_COMPLAINT_WINDOW_SECONDS + SECONDS_PER_DAY,
        ..receipt_for(&world, &market, &seller, &buyer, order_id(0xCD), usdc(80))
    };
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .unwrap();
    world
        .resolve_dispute(&market.arbiter, market.pubkey, receipt.order_id, buyer_token_account, false)
        .unwrap();

    world.warp_seconds(MAX_COMPLAINT_WINDOW_SECONDS);
    let caller = funded_keypair(&mut world);
    world
        .close_dispute(&caller, market.pubkey, receipt.order_id)
        .expect("close_dispute succeeds once the receipt has aged out");
    assert!(world
        .svm
        .get_account(&world.dispute_pda(&market.pubkey, &seller.pubkey(), &receipt.order_id))
        .is_none());

    world.svm.expire_blockhash();
    let result = world.raise_dispute(
        &buyer,
        buyer_token_account,
        market.pubkey,
        &receipt,
        &market.receipt_signer,
        usdc(80),
    );
    assert_error_code(&result, u32::from(TrustStakeError::ComplaintWindowClosed));
}

#[test]
fn test_dispute_replay_blocked_across_regrant_with_longer_window() {
    // The exploit `closable_after`'s new formula closes: a permit PDA
    // carries no nonce, so a seller can revoke, wait out the window,
    // release, and re-grant at the same address once the marketplace has
    // raised its complaint window. If `closable_after` were still frozen
    // from the window in effect when the dispute was filed, the record
    // would become closable -- and its PDA free for the very same
    // receipt to be replayed -- before the re-granted permit's own
    // (longer) window would otherwise have allowed it. That is a real
    // double-slash: the seller pays out on the same order twice.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(59);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xE0), usdc(80));
    let issued_at = receipt.issued_at;
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .expect("the first filing succeeds");
    world
        .resolve_dispute(&market.arbiter, market.pubkey, receipt.order_id, buyer_token_account, false)
        .expect("resolve_dispute succeeds");

    // Revoke, wait out the permit's own (2-day) window, and release: the
    // seller is now free to re-grant at this marketplace.
    world
        .revoke_permit(&seller, market.pubkey)
        .expect("revoke_permit succeeds");
    world.warp_seconds(MIN_COMPLAINT_WINDOW_SECONDS);
    let caller = funded_keypair(&mut world);
    world
        .release_permit(&caller, seller.pubkey(), market.pubkey)
        .expect("release_permit succeeds");

    // The marketplace raises its window to the protocol ceiling, and the
    // re-grant copies it onto a fresh permit.
    world
        .update_marketplace(
            &market.authority,
            market.pubkey,
            None,
            None,
            Some(MAX_COMPLAINT_WINDOW_SECONDS),
            None,
        )
        .expect("update_marketplace succeeds");
    // Identical to the grant `scenario` already issued, so without a
    // fresh blockhash this would collide with that earlier transaction
    // rather than exercising the re-grant.
    world.svm.expire_blockhash();
    world
        .grant_permit(&seller, market.pubkey, usdc(150))
        .expect("grant_permit succeeds");

    // Exactly the instant the *original* 2-day window closed: old enough
    // that a `closable_after` frozen from that window would already let
    // `close_dispute` free the PDA, nowhere near old enough for the new
    // 30-day window to have closed the same receipt.
    assert_eq!(world.now(), issued_at + MIN_COMPLAINT_WINDOW_SECONDS);

    // Whether this succeeds or fails is exactly what distinguishes the
    // fixed behaviour from the bug: fixed, `closable_after` is still 28
    // days out, so this is refused and the record survives as the replay
    // guard it always was. The result is deliberately not asserted here
    // -- the real proof is that the replay below fails regardless of what
    // happened to this record.
    let _ = world.close_dispute(&caller, market.pubkey, receipt.order_id);

    world.svm.expire_blockhash();
    let result = world.raise_dispute(
        &buyer,
        buyer_token_account,
        market.pubkey,
        &receipt,
        &market.receipt_signer,
        usdc(80),
    );
    assert!(
        result.is_err(),
        "the same receipt must not be filable again through a revoke/release/re-grant cycle that only widens the window"
    );
}

#[test]
fn test_dispute_rejects_receipt_issued_after_revocation() {
    // Revocation stops new receipts immediately. Ones already issued
    // survive for the rest of their window, which is the whole reason
    // releasing collateral waits that window out.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(26);

    let earlier_receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xCE), usdc(80));
    world.warp_seconds(60);
    world
        .revoke_permit(&seller, market.pubkey)
        .expect("revoke_permit succeeds");
    world.warp_seconds(60);
    let later_receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xCF), usdc(80));

    let result = world.raise_dispute(
        &buyer,
        buyer_token_account,
        market.pubkey,
        &later_receipt,
        &market.receipt_signer,
        usdc(80),
    );
    assert_error_code(&result, u32::from(TrustStakeError::ReceiptIssuedAfterRevocation));

    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &earlier_receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .expect("a receipt issued before the revocation still files");
}

#[test]
fn test_dispute_underpaid_bond_rejected() {
    // The bond is computed by the program and taken from the buyer, never
    // supplied by them, so underpaying is not something a caller can
    // express: the only way to pay less is to hold less, and then the
    // transfer itself fails and no record is created.
    let Scenario {
        mut world,
        market,
        seller,
        ..
    } = scenario(27);

    // 10% of $80 is $8; this buyer is one cent short.
    let (buyer, buyer_token_account) = setup_buyer(&mut world, usdc(8) - 1);
    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xD0), usdc(80));

    let result = world.raise_dispute(
        &buyer,
        buyer_token_account,
        market.pubkey,
        &receipt,
        &market.receipt_signer,
        usdc(80),
    );
    assert_error_code(&result, TokenError::InsufficientFunds as u32);
    assert!(world
        .svm
        .get_account(&world.dispute_pda(&market.pubkey, &seller.pubkey(), &receipt.order_id))
        .is_none());

    // With the last cent, the same filing goes through and the buyer's
    // balance falls by exactly the bond. The retry is byte-identical to
    // the attempt above, which LiteSVM would reject as already processed
    // before the program ever ran.
    world.mint_to(&buyer_token_account, 1);
    world.svm.expire_blockhash();
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .expect("a buyer holding exactly the bond can file");
    assert_eq!(world.token_balance(&buyer_token_account), 0);
}

#[test]
fn test_buyer_cannot_resolve() {
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(28);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xD1), usdc(80));
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .unwrap();

    let result = world.resolve_dispute(&buyer, market.pubkey, receipt.order_id, buyer_token_account, true);
    assert_error_code(&result, u32::from(TrustStakeError::NotArbiter));

    // Nor can the marketplace's own authority, which is a different key
    // on purpose: compromising any one key must not move money.
    world.svm.expire_blockhash();
    let result = world.resolve_dispute(
        &market.authority,
        market.pubkey,
        receipt.order_id,
        buyer_token_account,
        true,
    );
    assert_error_code(&result, u32::from(TrustStakeError::NotArbiter));
}

#[test]
fn test_buyer_cannot_expire_early() {
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(29);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xD2), usdc(80));
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .unwrap();

    let result = world.expire_dispute(&buyer, market.pubkey, receipt.order_id, buyer_token_account);
    assert_error_code(&result, u32::from(TrustStakeError::DisputeNotExpired));
}

#[test]
fn test_buyer_cannot_close_open_dispute() {
    // Closing an open complaint would erase the freeze on the permit and
    // the money owed with it.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(30);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xD3), usdc(80));
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .unwrap();
    world.warp_seconds(MIN_COMPLAINT_WINDOW_SECONDS + 1);

    let result = world.close_dispute(&buyer, market.pubkey, receipt.order_id);
    assert_error_code(&result, u32::from(TrustStakeError::DisputeStillOpen));
}

// ---------------------------------------------------------------------
// resolve_dispute, both outcomes
// ---------------------------------------------------------------------

#[test]
fn test_resolve_dispute_upheld_pays_buyer() {
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(31);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xE0), usdc(80));
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .unwrap();

    world
        .resolve_dispute(&market.arbiter, market.pubkey, receipt.order_id, buyer_token_account, true)
        .expect("resolve_dispute upheld succeeds");

    // The buyer is made whole and gets the bond back: $100 - $8 bond +
    // $80 payout + $8 bond.
    assert_eq!(world.token_balance(&buyer_token_account), usdc(180));
    assert_eq!(world.token_balance(&world.bond_vault_pda(&market.pubkey)), 0);
    assert_eq!(world.token_balance(&world.stake_vault_pda(&seller.pubkey())), usdc(220));

    // `committed` and `staked` fall together, which is what keeps
    // `committed <= staked` true through a slash.
    let stake = world.read_seller_stake(&world.stake_pda(&seller.pubkey()));
    assert_eq!(stake.staked, usdc(220));
    assert_eq!(stake.committed, usdc(70));
    assert_eq!(stake.disputes_total, 1);
    assert_eq!(stake.disputes_lost, 1);

    let permit = world.read_permit(&world.permit_pda(&seller.pubkey(), &market.pubkey));
    assert_eq!(permit.slashed, usdc(80));
    assert_eq!(permit.max_slashable, usdc(150));
    assert_eq!(permit.open_disputes, 0);

    let marketplace = world.read_marketplace(&market.pubkey);
    assert_eq!(marketplace.disputes_upheld, 1);
    assert_eq!(marketplace.disputes_abandoned, 0);
    assert_eq!(marketplace.total_slashed, usdc(80));

    let dispute = world.read_dispute(&world.dispute_pda(&market.pubkey, &seller.pubkey(), &receipt.order_id));
    assert_eq!(dispute.status, DisputeStatus::Upheld as u8);
}

#[test]
fn test_resolve_dispute_rejected_pays_seller() {
    // The only path on which a seller's free balance grows without a
    // deposit: the forfeited bond lands in the vault, `staked` rises with
    // it, and `committed` does not move.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(32);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xE1), usdc(80));
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .unwrap();

    world
        .resolve_dispute(&market.arbiter, market.pubkey, receipt.order_id, buyer_token_account, false)
        .expect("resolve_dispute rejected succeeds");

    assert_eq!(world.token_balance(&buyer_token_account), usdc(92));
    assert_eq!(world.token_balance(&world.bond_vault_pda(&market.pubkey)), 0);
    assert_eq!(world.token_balance(&world.stake_vault_pda(&seller.pubkey())), usdc(308));

    let stake = world.read_seller_stake(&world.stake_pda(&seller.pubkey()));
    assert_eq!(stake.staked, usdc(308));
    assert_eq!(stake.committed, usdc(150));
    assert_eq!(stake.disputes_total, 1);
    assert_eq!(stake.disputes_lost, 0);

    let permit = world.read_permit(&world.permit_pda(&seller.pubkey(), &market.pubkey));
    assert_eq!(permit.slashed, 0);
    assert_eq!(permit.open_disputes, 0);

    let marketplace = world.read_marketplace(&market.pubkey);
    assert_eq!(marketplace.disputes_upheld, 0);
    assert_eq!(marketplace.total_slashed, 0);

    let dispute = world.read_dispute(&world.dispute_pda(&market.pubkey, &seller.pubkey(), &receipt.order_id));
    assert_eq!(dispute.status, DisputeStatus::Rejected as u8);
}

// ---------------------------------------------------------------------
// Attacks by a malicious marketplace
//
// Decision 2 accepts that a marketplace can draw the full permit at will.
// These prove it cannot go one cent past that, and cannot reach anything
// that is not its own.
// ---------------------------------------------------------------------

#[test]
fn test_marketplace_cannot_exceed_permit() {
    // A $200 order against a $50 permit. The claim is recorded in full
    // and counted in full -- rejecting it would leave the seller's public
    // loss count understating real fraud -- but only $50 can move.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario_with(33, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS, usdc(300), usdc(50));

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xE2), usdc(200));
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .expect("an over-cap claim is recorded, not rejected");

    let dispute = world.read_dispute(&world.dispute_pda(&market.pubkey, &seller.pubkey(), &receipt.order_id));
    assert_eq!(dispute.claim, usdc(80));

    world
        .resolve_dispute(&market.arbiter, market.pubkey, receipt.order_id, buyer_token_account, true)
        .expect("resolve_dispute upheld succeeds");

    // $50 paid out of a claim of $80, plus the $8 bond back.
    assert_eq!(world.token_balance(&buyer_token_account), usdc(150));
    let stake = world.read_seller_stake(&world.stake_pda(&seller.pubkey()));
    assert_eq!(stake.staked, usdc(250));
    assert_eq!(stake.committed, 0);
    let permit = world.read_permit(&world.permit_pda(&seller.pubkey(), &market.pubkey));
    assert_eq!(permit.slashed, usdc(50));
    assert_eq!(world.read_marketplace(&market.pubkey).total_slashed, usdc(50));
    // Counted, whatever it paid.
    assert_eq!(stake.disputes_lost, 1);
    assert_eq!(stake.disputes_total, 1);
}

#[test]
fn test_marketplace_cannot_exceed_staked() {
    // A permit for everything the seller has, and a claim for more than
    // that. The payout is clamped to what the vault actually holds, so
    // the vault empties and never overdraws. (`committed <= staked` means
    // a permit's remaining allowance can never exceed `staked` either, so
    // the two clamps meet here rather than one hiding behind the other.)
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario_with(34, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS, usdc(100), usdc(100));

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xE3), usdc(200));
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(150),
        )
        .unwrap();

    world
        .resolve_dispute(&market.arbiter, market.pubkey, receipt.order_id, buyer_token_account, true)
        .expect("resolve_dispute upheld succeeds");

    assert_eq!(world.token_balance(&world.stake_vault_pda(&seller.pubkey())), 0);
    let stake = world.read_seller_stake(&world.stake_pda(&seller.pubkey()));
    assert_eq!(stake.staked, 0);
    assert_eq!(stake.committed, 0);
    // $100 of collateral plus the $15 bond returned, on top of the $85
    // the buyer had left after posting it.
    assert_eq!(world.token_balance(&buyer_token_account), usdc(200));
}

#[test]
fn test_marketplace_cannot_resolve_foreign_dispute() {
    // One marketplace's arbiter, resolving a complaint that belongs to
    // another. Every account in `resolve_dispute` chains off the record,
    // so the marketplace passed in has to be the record's own.
    let mut world = setup_world();
    let first = setup_marketplace(&mut world, 35, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    let second = setup_marketplace(&mut world, 36, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    let (seller, _seller_token_account) = setup_staked_seller(&mut world, usdc(300));
    world.grant_permit(&seller, first.pubkey, usdc(150)).unwrap();
    world.grant_permit(&seller, second.pubkey, usdc(100)).unwrap();
    let (buyer, buyer_token_account) = setup_buyer(&mut world, usdc(100));

    let receipt = receipt_for(&world, &first, &seller, &buyer, order_id(0xE4), usdc(80));
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            first.pubkey,
            &receipt,
            &first.receipt_signer,
            usdc(80),
        )
        .unwrap();

    // The second marketplace's arbiter, signing legitimately for their
    // own marketplace, pointed at the first marketplace's dispute.
    let result = resolve_dispute_with_marketplace(
        &mut world,
        &second.arbiter,
        second.pubkey,
        first.pubkey,
        receipt.order_id,
        seller.pubkey(),
        buyer_token_account,
        true,
    );
    assert_error_code(&result, u32::from(anchor_lang::error::ErrorCode::ConstraintHasOne));

    let dispute = world.read_dispute(&world.dispute_pda(&first.pubkey, &seller.pubkey(), &receipt.order_id));
    assert_eq!(dispute.status, DisputeStatus::Open as u8);
}

#[test]
fn test_attacker_marketplace_cannot_resolve() {
    // The same account-chaining check, from the other direction: an
    // attacker registers a marketplace of their own, appoints themselves
    // its arbiter, and passes it into someone else's dispute.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(37);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xE5), usdc(80));
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .unwrap();

    let attacker_market = setup_marketplace(&mut world, 38, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    let result = resolve_dispute_with_marketplace(
        &mut world,
        &attacker_market.arbiter,
        attacker_market.pubkey,
        market.pubkey,
        receipt.order_id,
        seller.pubkey(),
        buyer_token_account,
        true,
    );
    assert_error_code(&result, u32::from(anchor_lang::error::ErrorCode::ConstraintHasOne));

    let stake = world.read_seller_stake(&world.stake_pda(&seller.pubkey()));
    assert_eq!(stake.staked, usdc(300));
    assert_eq!(stake.disputes_lost, 0);
}

#[test]
fn test_arbiter_cannot_redirect_payout() {
    // An arbiter may decide a complaint either way, and may not decide
    // where the money goes: the destination is bound to the buyer named
    // on the record.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(39);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xE6), usdc(80));
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .unwrap();

    let arbiter_token_account = world.create_funded_token_account(world.mint, market.arbiter.pubkey(), 0);
    let result = world.resolve_dispute(
        &market.arbiter,
        market.pubkey,
        receipt.order_id,
        arbiter_token_account,
        true,
    );
    assert_error_code(&result, u32::from(anchor_lang::error::ErrorCode::ConstraintTokenOwner));
    assert_eq!(world.token_balance(&arbiter_token_account), 0);
}

#[test]
fn test_dispute_cannot_be_resolved_twice() {
    // Without the `status == Open` guard a never-closed record is
    // resolvable over and over, draining other buyers' bonds out of the
    // shared pool and underflowing `open_disputes` past the release gate.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(40);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xE7), usdc(80));
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .unwrap();
    world
        .resolve_dispute(&market.arbiter, market.pubkey, receipt.order_id, buyer_token_account, true)
        .unwrap();

    world.svm.expire_blockhash();
    let result = world.resolve_dispute(&market.arbiter, market.pubkey, receipt.order_id, buyer_token_account, true);
    assert_error_code(&result, u32::from(TrustStakeError::DisputeNotOpen));

    // Expiring a decided complaint is refused by the same guard.
    world.warp_seconds(DISPUTE_EXPIRY_SECONDS);
    let result = world.expire_dispute(&buyer, market.pubkey, receipt.order_id, buyer_token_account);
    assert_error_code(&result, u32::from(TrustStakeError::DisputeNotOpen));

    let stake = world.read_seller_stake(&world.stake_pda(&seller.pubkey()));
    assert_eq!(stake.staked, usdc(220), "only one payout ever left the vault");
    assert_eq!(stake.disputes_lost, 1);
}

#[test]
fn test_bond_uses_recorded_amount() {
    // Raising the bond rate after a complaint is filed must not change
    // what that complaint pays out. The deposited amount is stored on the
    // record and only that amount ever moves; recomputing it from the
    // live rate would let a marketplace overdraw the shared pool and pay
    // one buyer with another's bond.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(41);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xE8), usdc(80));
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .unwrap();
    assert_eq!(
        world
            .read_dispute(&world.dispute_pda(&market.pubkey, &seller.pubkey(), &receipt.order_id))
            .bond,
        usdc(8)
    );

    world
        .update_marketplace(&market.authority, market.pubkey, None, None, None, Some(MAX_BOND_BPS))
        .expect("update_marketplace succeeds");

    world
        .resolve_dispute(&market.arbiter, market.pubkey, receipt.order_id, buyer_token_account, true)
        .expect("resolve_dispute succeeds");

    // $8 back, not the $16 the new 20% rate would compute.
    assert_eq!(world.token_balance(&buyer_token_account), usdc(180));
    assert_eq!(world.token_balance(&world.bond_vault_pda(&market.pubkey)), 0);
}

#[test]
fn test_signer_rotation_preserves_old_receipts() {
    // An honest key rotation must not silently void every outstanding
    // buyer's claim, and must still stop the retired key from issuing
    // anything new.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(42);

    let before_rotation = receipt_for(&world, &market, &seller, &buyer, order_id(0xE9), usdc(80));

    world.warp_seconds(60);
    let new_signer = Keypair::new();
    world
        .update_marketplace(
            &market.authority,
            market.pubkey,
            Some(new_signer.pubkey()),
            None,
            None,
            None,
        )
        .expect("update_marketplace rotates the receipt signer");
    let rotated_at = world.read_marketplace(&market.pubkey).signer_rotated_at;
    assert!(rotated_at > before_rotation.issued_at);

    // Issued before the rotation, signed by the retired key: still good.
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &before_rotation,
            &market.receipt_signer,
            usdc(80),
        )
        .expect("a receipt predating the rotation still verifies");

    // Issued after it, signed by the retired key: refused.
    world.warp_seconds(60);
    let after_rotation = receipt_for(&world, &market, &seller, &buyer, order_id(0xEA), usdc(80));
    let result = world.raise_dispute(
        &buyer,
        buyer_token_account,
        market.pubkey,
        &after_rotation,
        &market.receipt_signer,
        usdc(80),
    );
    assert_error_code(&result, u32::from(TrustStakeError::WrongReceiptSigner));

    // And the new key works, which is the point of rotating.
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &after_rotation,
            &new_signer,
            usdc(80),
        )
        .expect("the current signer's receipts verify");
}

#[test]
fn test_parallel_disputes_cannot_exceed_cap() {
    // Two complaints raised against one permit before either is decided.
    // Both pass the raise-time checks against the same undecremented
    // balance, so the cap has to hold at resolution instead: the first
    // takes what is there and the second takes what is left, which is
    // nothing.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(43);

    let (second_buyer, second_buyer_token_account) = setup_buyer(&mut world, usdc(100));
    let first_receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xEB), usdc(150));
    let second_receipt = receipt_for(&world, &market, &seller, &second_buyer, order_id(0xEC), usdc(150));

    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &first_receipt,
            &market.receipt_signer,
            usdc(150),
        )
        .unwrap();
    world
        .raise_dispute(
            &second_buyer,
            second_buyer_token_account,
            market.pubkey,
            &second_receipt,
            &market.receipt_signer,
            usdc(150),
        )
        .unwrap();

    let permit_pubkey = world.permit_pda(&seller.pubkey(), &market.pubkey);
    assert_eq!(world.read_permit(&permit_pubkey).open_disputes, 2);

    world
        .resolve_dispute(
            &market.arbiter,
            market.pubkey,
            first_receipt.order_id,
            buyer_token_account,
            true,
        )
        .unwrap();
    world
        .resolve_dispute(
            &market.arbiter,
            market.pubkey,
            second_receipt.order_id,
            second_buyer_token_account,
            true,
        )
        .unwrap();

    // $150 of cap, $150 paid out in total, and the second buyer gets
    // their bond back and nothing else.
    let permit = world.read_permit(&permit_pubkey);
    assert_eq!(permit.slashed, usdc(150));
    assert_eq!(permit.max_slashable, usdc(150));
    assert_eq!(permit.open_disputes, 0);
    assert_eq!(world.token_balance(&buyer_token_account), usdc(250));
    assert_eq!(world.token_balance(&second_buyer_token_account), usdc(100));
    let stake = world.read_seller_stake(&world.stake_pda(&seller.pubkey()));
    assert_eq!(stake.staked, usdc(150));
    assert_eq!(stake.committed, 0);
    assert_eq!(stake.disputes_lost, 2);
    assert_eq!(world.read_marketplace(&market.pubkey).total_slashed, usdc(150));
}

#[test]
fn test_marketplace_cannot_freeze_seller_forever() {
    // docs/TESTING.md calls this the single most important test in the
    // file: the finding all three reviewers hit independently. A
    // marketplace opens a complaint and never decides it. Thirty days
    // later anyone at all can expire it, the freeze lifts, and the seller
    // gets their collateral out.
    let Scenario {
        mut world,
        market,
        seller,
        seller_token_account,
        buyer,
        buyer_token_account,
    } = scenario(44);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xED), usdc(80));
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .unwrap();

    // The seller does everything right: revokes, waits out the window.
    world.revoke_permit(&seller, market.pubkey).unwrap();
    world.warp_seconds(MIN_COMPLAINT_WINDOW_SECONDS + 1);

    let stranger = funded_keypair(&mut world);
    let result = world.release_permit(&stranger, seller.pubkey(), market.pubkey);
    assert_error_code(&result, u32::from(TrustStakeError::OpenDisputesRemaining));

    // Even with the marketplace's own cooperation, an open complaint
    // blocks the early path too.
    let result = world.release_permit_early(&seller, &market.authority, market.pubkey);
    assert_error_code(&result, u32::from(TrustStakeError::OpenDisputesRemaining));

    // The freeze is scoped: the $150 that was never committed is still
    // withdrawable throughout.
    world
        .withdraw_stake(&seller, seller_token_account, usdc(150))
        .expect("uncommitted collateral is not frozen by a complaint");

    // Thirty days after the complaint was raised, a stranger expires it.
    world.warp_seconds(DISPUTE_EXPIRY_SECONDS);
    world
        .expire_dispute(&stranger, market.pubkey, receipt.order_id, buyer_token_account)
        .expect("anyone may expire an abandoned complaint");

    // Nobody was paid, the bond went home, and the mark is on the
    // marketplace rather than on either party to the trade.
    assert_eq!(world.token_balance(&buyer_token_account), usdc(100));
    assert_eq!(world.token_balance(&world.bond_vault_pda(&market.pubkey)), 0);
    let marketplace = world.read_marketplace(&market.pubkey);
    assert_eq!(marketplace.disputes_abandoned, 1);
    assert_eq!(marketplace.disputes_upheld, 0);
    assert_eq!(marketplace.total_slashed, 0);
    let stake_before_release = world.read_seller_stake(&world.stake_pda(&seller.pubkey()));
    assert_eq!(stake_before_release.disputes_total, 1);
    assert_eq!(stake_before_release.disputes_lost, 0);
    let dispute = world.read_dispute(&world.dispute_pda(&market.pubkey, &seller.pubkey(), &receipt.order_id));
    assert_eq!(dispute.status, DisputeStatus::Abandoned as u8);

    // And now the seller gets out. (A fresh blockhash: this transaction
    // is byte-identical to the release that was refused above.)
    world.svm.expire_blockhash();
    world
        .release_permit(&stranger, seller.pubkey(), market.pubkey)
        .expect("release_permit succeeds once the complaint is expired");
    assert_eq!(
        world.read_seller_stake(&world.stake_pda(&seller.pubkey())).committed,
        0
    );
    world
        .withdraw_stake(&seller, seller_token_account, usdc(150))
        .expect("the seller withdraws the rest of their collateral");
    assert_eq!(world.token_balance(&world.stake_vault_pda(&seller.pubkey())), 0);
}

#[test]
fn test_dispute_scoped_to_permit() {
    // A complaint at one marketplace freezes that marketplace's permit
    // and nothing else. The old design's global counter meant the
    // smallest, most forgotten marketplace a seller ever signed up with
    // could freeze all of their collateral for the price of one filing.
    let mut world = setup_world();
    let frozen_market = setup_marketplace(&mut world, 45, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    let other_market = setup_marketplace(&mut world, 46, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS);
    let (seller, seller_token_account) = setup_staked_seller(&mut world, usdc(300));
    world.grant_permit(&seller, frozen_market.pubkey, usdc(150)).unwrap();
    world.grant_permit(&seller, other_market.pubkey, usdc(100)).unwrap();
    let (buyer, buyer_token_account) = setup_buyer(&mut world, usdc(100));

    let receipt = receipt_for(&world, &frozen_market, &seller, &buyer, order_id(0xEE), usdc(80));
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            frozen_market.pubkey,
            &receipt,
            &frozen_market.receipt_signer,
            usdc(80),
        )
        .unwrap();

    // The other marketplace's permit is untouched.
    let other_permit = world.read_permit(&world.permit_pda(&seller.pubkey(), &other_market.pubkey));
    assert_eq!(other_permit.open_disputes, 0);
    assert_eq!(world.token_balance(&world.bond_vault_pda(&other_market.pubkey)), 0);

    // Free collateral still withdraws: $300 staked, $250 committed.
    world
        .withdraw_stake(&seller, seller_token_account, usdc(50))
        .expect("free collateral is unaffected by a complaint elsewhere");

    // And the other permit still releases on its own schedule.
    world.revoke_permit(&seller, other_market.pubkey).unwrap();
    world.warp_seconds(MIN_COMPLAINT_WINDOW_SECONDS + 1);
    let stranger = funded_keypair(&mut world);
    world
        .release_permit(&stranger, seller.pubkey(), other_market.pubkey)
        .expect("a complaint at one marketplace does not block release at another");

    let stake = world.read_seller_stake(&world.stake_pda(&seller.pubkey()));
    assert_eq!(stake.committed, usdc(150));
    assert_eq!(stake.staked, usdc(250));
}

#[test]
fn test_seller_cannot_release_permit_with_open_dispute() {
    // Deferred out of Phase 2, which could enforce `open_disputes == 0`
    // but had no way to make it nonzero. Both release paths refuse, and
    // they keep refusing however long the window has been over.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(47);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xEF), usdc(80));
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .unwrap();

    world.revoke_permit(&seller, market.pubkey).unwrap();
    world.warp_seconds(MIN_COMPLAINT_WINDOW_SECONDS * 10);

    let stranger = funded_keypair(&mut world);
    let result = world.release_permit(&stranger, seller.pubkey(), market.pubkey);
    assert_error_code(&result, u32::from(TrustStakeError::OpenDisputesRemaining));

    let result = world.release_permit_early(&seller, &market.authority, market.pubkey);
    assert_error_code(&result, u32::from(TrustStakeError::OpenDisputesRemaining));

    // Once it is decided, release goes through: the block is the open
    // complaint, not the complaint's existence.
    world
        .resolve_dispute(&market.arbiter, market.pubkey, receipt.order_id, buyer_token_account, false)
        .unwrap();
    world.svm.expire_blockhash();
    world
        .release_permit(&stranger, seller.pubkey(), market.pubkey)
        .expect("release succeeds once nothing is open");
}

/// `resolve_dispute` with the marketplace account and the dispute's
/// marketplace chosen separately, which the harness method derives from
/// each other. Only the tests that pass a foreign marketplace need this.
#[allow(clippy::too_many_arguments)]
fn resolve_dispute_with_marketplace(
    world: &mut World,
    arbiter: &Keypair,
    marketplace: Pubkey,
    dispute_marketplace: Pubkey,
    order: [u8; 32],
    seller: Pubkey,
    buyer_token_account: Pubkey,
    upheld: bool,
) -> litesvm::types::TransactionResult {
    let (event_authority, program) = event_cpi_accounts(&world.program_id);
    let instruction = Instruction::new_with_bytes(
        world.program_id,
        &truststake::instruction::ResolveDispute { upheld }.data(),
        truststake::accounts::ResolveDisputeAccountConstraints {
            arbiter: arbiter.pubkey(),
            marketplace,
            dispute: world.dispute_pda(&dispute_marketplace, &seller, &order),
            permit: world.permit_pda(&seller, &dispute_marketplace),
            stake: world.stake_pda(&seller),
            stake_vault: world.stake_vault_pda(&seller),
            bond_vault: world.bond_vault_pda(&marketplace),
            mint: world.mint,
            buyer_token_account,
            token_program: anchor_spl::token::spl_token::ID,
            event_authority,
            program,
        }
        .to_account_metas(None),
    );

    world.send_instructions(&[instruction], &arbiter.pubkey(), &[arbiter])
}

#[test]
fn test_substituted_marketplace_rejected() {
    // The dispute half of docs/TESTING.md's item: "no permit or dispute
    // address can be derived from a forged marketplace". Every seed a
    // dispute uses runs through `marketplace.key()`, so an attacker who
    // could pass a `Marketplace` account of their own choosing would
    // choose the addresses of the permit, the bond vault and the record
    // along with it. The account planted below is a byte-perfect copy of
    // a real marketplace, owned by the program and carrying the right
    // discriminator, sitting at an address that is not the PDA for the ID
    // inside it -- which is the one thing the seeds re-derivation looks
    // at.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(60);

    let genuine = world.svm.get_account(&market.pubkey).expect("marketplace exists");
    let forged_address = Pubkey::new_unique();
    world
        .svm
        .set_account(forged_address, genuine.clone())
        .expect("plant a copy of the marketplace at an address of the attacker's choosing");

    // The seeds re-derivation refusing, on a handler whose only account
    // is the marketplace itself. The authority signing is the genuine
    // one, copied along with everything else, so nothing but the address
    // is left to catch this.
    let result = world.update_marketplace(
        &market.authority,
        forged_address,
        None,
        None,
        Some(30 * SECONDS_PER_DAY),
        None,
    );
    assert_error_code(&result, u32::from(anchor_lang::error::ErrorCode::ConstraintSeeds));

    // And through `raise_dispute`, where the same substitution fails
    // earlier still: the permit, the bond vault and the record all derive
    // their addresses from the marketplace account passed in, so under a
    // forged one none of them exists. Nor could the attacker create them
    // there, because `grant_permit` derives the permit from a marketplace
    // account that self-validates the same way.
    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0x1C), usdc(80));
    let raise_instruction = world.raise_dispute_instruction(
        buyer.pubkey(),
        buyer_token_account,
        forged_address,
        seller.pubkey(),
        receipt.order_id,
        usdc(80),
    );

    let result = world.send_raise_dispute(
        &[
            ed25519_verify_instruction(&receipt, &market.receipt_signer),
            raise_instruction,
        ],
        &buyer,
        forged_address,
        seller.pubkey(),
        receipt.order_id,
    );
    assert_error_code(&result, u32::from(anchor_lang::error::ErrorCode::AccountNotInitialized));
}

// ---------------------------------------------------------------------
// Failures with no attacker
//
// Every one of these has caused real losses in production systems, and
// none of them requires anyone to be malicious.
// ---------------------------------------------------------------------

#[test]
fn test_abandoned_marketplace_does_not_trap_seller() {
    // The likelier version of the freeze: nobody is being malicious, the
    // marketplace has simply shut down and its arbiter key will never be
    // used again. The seller's entire collateral is committed to it, so
    // without expiry there is no way out at all.
    let Scenario {
        mut world,
        market,
        seller,
        seller_token_account,
        buyer,
        buyer_token_account,
    } = scenario_with(48, MIN_COMPLAINT_WINDOW_SECONDS, DEFAULT_BOND_BPS, usdc(200), usdc(200));

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0x11), usdc(80));
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .unwrap();
    world.revoke_permit(&seller, market.pubkey).unwrap();

    // Nothing is withdrawable in the meantime: it is all committed.
    let result = world.withdraw_stake(&seller, seller_token_account, 1);
    assert_error_code(&result, u32::from(TrustStakeError::CommittedExceedsStaked));

    world.warp_seconds(DISPUTE_EXPIRY_SECONDS);
    let stranger = funded_keypair(&mut world);
    world
        .expire_dispute(&stranger, market.pubkey, receipt.order_id, buyer_token_account)
        .expect("expiry needs nothing from the marketplace");
    world
        .release_permit(&stranger, seller.pubkey(), market.pubkey)
        .expect("release_permit succeeds");
    world
        .withdraw_stake(&seller, seller_token_account, usdc(200))
        .expect("the seller recovers everything");

    assert_eq!(world.token_balance(&seller_token_account), usdc(200));
    assert_eq!(world.token_balance(&buyer_token_account), usdc(100));
}

#[test]
fn test_stake_vault_donation_does_not_brick_the_seller() {
    // A token account accepts a transfer from anyone without its owner's
    // consent, and no handler recomputes `stake.staked` from the vault --
    // every write to it is a delta -- so an unrelated third party can
    // always push `stake_vault.amount` above the ledger by donating into
    // it directly. Before the vault/ledger check relaxed to `>=`, that
    // single deposit permanently bricked the seller: every handler that
    // moves tokens demanded exact equality, so the seller could never
    // again withdraw, top up collateral, or have a dispute against them
    // resolved. None of this requires anyone to be malicious; a buyer who
    // fat-fingers a refund straight to the vault address has the same
    // effect.
    let Scenario {
        mut world,
        market,
        seller,
        seller_token_account,
        buyer,
        buyer_token_account,
    } = scenario(61);

    let stranger = funded_keypair(&mut world);
    let stranger_token_account = world.create_funded_token_account(world.mint, stranger.pubkey(), 1);
    let vault = world.stake_vault_pda(&seller.pubkey());
    let donation = anchor_spl::token::spl_token::instruction::transfer(
        &anchor_spl::token::spl_token::ID,
        &stranger_token_account,
        &vault,
        &stranger.pubkey(),
        &[],
        1,
    )
    .expect("build donation instruction");
    common::send(&mut world.svm, &[donation], &stranger.pubkey(), &[&stranger])
        .expect("an unsolicited deposit into the vault succeeds: a token account takes transfers from anyone");

    // The seller can still withdraw uncommitted collateral...
    world
        .withdraw_stake(&seller, seller_token_account, usdc(50))
        .expect("withdrawal still succeeds after a donation into the vault");

    // ...can still add more...
    world
        .add_stake(&seller, seller_token_account, usdc(10))
        .expect("adding more stake still succeeds after a donation into the vault");

    // ...and a dispute against them can still be resolved.
    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0xE2), usdc(80));
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .expect("raise_dispute still succeeds after a donation into the vault");
    world
        .resolve_dispute(&market.arbiter, market.pubkey, receipt.order_id, buyer_token_account, true)
        .expect("resolve_dispute still succeeds after a donation into the vault");
}

#[test]
fn test_window_boundary_exact() {
    // The complaint window is inclusive at the start and exclusive at the
    // end: the handler requires `now < issued_at + complaint_window`.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(49);

    let first = receipt_for(&world, &market, &seller, &buyer, order_id(0x12), usdc(80));
    let second = receipt_for(&world, &market, &seller, &buyer, order_id(0x13), usdc(80));
    assert_eq!(first.issued_at, second.issued_at);

    // One second before the window closes.
    world.warp_seconds(MIN_COMPLAINT_WINDOW_SECONDS - 1);
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &first,
            &market.receipt_signer,
            usdc(80),
        )
        .expect("a complaint one second inside the window files");

    // Exactly on it.
    world.warp_seconds(1);
    let result = world.raise_dispute(
        &buyer,
        buyer_token_account,
        market.pubkey,
        &second,
        &market.receipt_signer,
        usdc(80),
    );
    assert_error_code(&result, u32::from(TrustStakeError::ComplaintWindowClosed));
}

#[test]
fn test_expiry_boundary_exact() {
    // Expiry is inclusive: `now >= expires_at`.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(50);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0x14), usdc(80));
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .unwrap();

    let caller = funded_keypair(&mut world);
    world.warp_seconds(DISPUTE_EXPIRY_SECONDS - 1);
    let result = world.expire_dispute(&caller, market.pubkey, receipt.order_id, buyer_token_account);
    assert_error_code(&result, u32::from(TrustStakeError::DisputeNotExpired));

    world.svm.expire_blockhash();
    world.warp_seconds(1);
    world
        .expire_dispute(&caller, market.pubkey, receipt.order_id, buyer_token_account)
        .expect("expiry exactly at the deadline succeeds");
}

#[test]
fn test_resolve_dispute_rejects_after_expiry() {
    // The exact complement of `test_expiry_boundary_exact`: past
    // `expires_at`, `resolve_dispute` must refuse and `expire_dispute`
    // must be the only path left. Without this gate, an arbiter could
    // slash a seller long after the dispute became expirable, turning
    // `DISPUTE_EXPIRY_SECONDS` into a race the arbiter always wins rather
    // than the seller's protection it is meant to be.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(60);

    let first = receipt_for(&world, &market, &seller, &buyer, order_id(0x15), usdc(80));
    let second = receipt_for(&world, &market, &seller, &buyer, order_id(0x16), usdc(80));
    assert_eq!(first.issued_at, second.issued_at);

    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &first,
            &market.receipt_signer,
            usdc(80),
        )
        .unwrap();
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &second,
            &market.receipt_signer,
            usdc(80),
        )
        .unwrap();

    // One second before the deadline: still resolvable.
    world.warp_seconds(DISPUTE_EXPIRY_SECONDS - 1);
    world
        .resolve_dispute(&market.arbiter, market.pubkey, first.order_id, buyer_token_account, false)
        .expect("resolving one second before expiry still succeeds");

    // Exactly on it: resolution is refused...
    world.svm.expire_blockhash();
    world.warp_seconds(1);
    let result = world.resolve_dispute(&market.arbiter, market.pubkey, second.order_id, buyer_token_account, false);
    assert_error_code(&result, u32::from(TrustStakeError::DisputeExpired));

    // ...and expiry is the only path left, exactly where
    // `test_expiry_boundary_exact` shows it opening.
    let caller = funded_keypair(&mut world);
    world
        .expire_dispute(&caller, market.pubkey, second.order_id, buyer_token_account)
        .expect("expire_dispute still succeeds at the same instant resolve_dispute is refused");
}

#[test]
fn test_issued_at_overflow() {
    // A receipt dated near the end of representable time. The window
    // arithmetic must fail cleanly rather than wrapping, which would
    // otherwise be a denial of service against one specific buyer's
    // ability to file.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(51);

    let mut receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0x15), usdc(80));
    receipt.issued_at = i64::MAX - 100;
    receipt.expires_at = i64::MAX;

    let result = world.raise_dispute(
        &buyer,
        buyer_token_account,
        market.pubkey,
        &receipt,
        &market.receipt_signer,
        usdc(80),
    );
    assert_error_code(&result, u32::from(TrustStakeError::MathOverflow));
}

#[test]
fn test_bond_rounding_small_claims() {
    // The target market is small orders, so the smallest possible claim
    // has to behave. 10% of one minor unit truncates to zero, and a zero
    // bond is a free complaint; rounding toward the buyer paying more is
    // what stops that.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(52);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0x16), 1);
    let balance_before = world.token_balance(&buyer_token_account);

    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            1,
        )
        .expect("a one-cent claim files");

    let dispute = world.read_dispute(&world.dispute_pda(&market.pubkey, &seller.pubkey(), &receipt.order_id));
    assert_eq!(dispute.bond, 1, "the bond rounds up, never down to zero");
    assert_eq!(world.token_balance(&buyer_token_account), balance_before - 1);
    assert_eq!(world.token_balance(&world.bond_vault_pda(&market.pubkey)), 1);
}

#[test]
fn test_max_value_arithmetic() {
    // The claim and payout half of docs/TESTING.md's item; Phase 2's file
    // covers the stake and cap half. Everything is at `u64::MAX` at once,
    // and nothing may wrap. The marketplace charges no bond, so the buyer
    // starts with nothing: the seller's stake is the entire supply of the
    // mint, and one more minor unit could not be minted to anyone.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario_with_buyer_funds(53, MIN_COMPLAINT_WINDOW_SECONDS, 0, u64::MAX, u64::MAX, 0);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0x17), u64::MAX);
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            u64::MAX,
        )
        .expect("a claim at u64::MAX files");

    let dispute = world.read_dispute(&world.dispute_pda(&market.pubkey, &seller.pubkey(), &receipt.order_id));
    assert_eq!(dispute.claim, u64::MAX);
    assert_eq!(dispute.bond, 0);

    world
        .resolve_dispute(&market.arbiter, market.pubkey, receipt.order_id, buyer_token_account, true)
        .expect("resolving a u64::MAX claim succeeds");

    let stake = world.read_seller_stake(&world.stake_pda(&seller.pubkey()));
    assert_eq!(stake.staked, 0);
    assert_eq!(stake.committed, 0);
    assert_eq!(world.token_balance(&buyer_token_account), u64::MAX);
    let permit = world.read_permit(&world.permit_pda(&seller.pubkey(), &market.pubkey));
    assert_eq!(permit.slashed, u64::MAX);
    assert_eq!(world.read_marketplace(&market.pubkey).total_slashed, u64::MAX);
}

#[test]
fn test_rent_refund_on_close() {
    // The dispute half of docs/TESTING.md's item; Phase 2's file covers
    // the permit half. The buyer paid this record's rent when they filed,
    // so the buyer gets it back, whoever sends the closing transaction.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(54);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0x18), usdc(80));
    world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .unwrap();
    world
        .resolve_dispute(&market.arbiter, market.pubkey, receipt.order_id, buyer_token_account, true)
        .unwrap();

    let dispute_pubkey = world.dispute_pda(&market.pubkey, &seller.pubkey(), &receipt.order_id);

    // Closing before the receipt has aged out is refused: the record is
    // still the replay guard until then.
    let caller = funded_keypair(&mut world);
    let result = world.close_dispute(&caller, market.pubkey, receipt.order_id);
    assert_error_code(&result, u32::from(TrustStakeError::DisputeNotClosable));

    // `closable_after` is bound to `MAX_COMPLAINT_WINDOW_SECONDS`, not
    // this marketplace's own (shorter) window.
    world.warp_seconds(MAX_COMPLAINT_WINDOW_SECONDS);
    world.svm.expire_blockhash();

    // Snapshot immediately before the one transaction that should move
    // the buyer's lamports, and let someone else pay its fee.
    let rent_lamports = world.svm.get_account(&dispute_pubkey).expect("dispute exists").lamports;
    let buyer_lamports_before = world.svm.get_account(&buyer.pubkey()).expect("buyer exists").lamports;

    world
        .close_dispute(&caller, market.pubkey, receipt.order_id)
        .expect("close_dispute succeeds");

    let buyer_lamports_after = world.svm.get_account(&buyer.pubkey()).expect("buyer exists").lamports;
    assert_eq!(buyer_lamports_after, buyer_lamports_before + rent_lamports);
    assert!(
        world.svm.get_account(&dispute_pubkey).is_none(),
        "the record is genuinely gone, not merely emptied"
    );
}

// ---------------------------------------------------------------------
// Limits and measurements
//
// docs/TESTING.md: transaction size, not compute, is the binding
// constraint in this program.
// ---------------------------------------------------------------------

#[test]
fn test_raise_dispute_transaction_size() {
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(55);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0x19), usdc(80));
    let verify_instruction = ed25519_verify_instruction(&receipt, &market.receipt_signer);
    let raise_instruction = world.raise_dispute_instruction(
        buyer.pubkey(),
        buyer_token_account,
        market.pubkey,
        seller.pubkey(),
        receipt.order_id,
        usdc(80),
    );

    let blockhash = world.svm.latest_blockhash();
    let message = Message::new_with_blockhash(
        &[verify_instruction.clone(), raise_instruction.clone()],
        Some(&buyer.pubkey()),
        &blockhash,
    );
    let transaction = VersionedTransaction::try_new(VersionedMessage::Legacy(message), &[&buyer])
        .expect("signing succeeds");
    let serialised = bincode::serialize(&transaction).expect("a transaction serialises");

    println!(
        "raise_dispute transaction: {} bytes of {TRANSACTION_SIZE_LIMIT} ({} accounts, {}-byte Ed25519 instruction)",
        serialised.len(),
        raise_instruction.accounts.len(),
        verify_instruction.data.len(),
    );
    assert!(
        serialised.len() <= TRANSACTION_SIZE_LIMIT,
        "raise_dispute must fit in one transaction: {} bytes",
        serialised.len()
    );

    // The transaction measured is the transaction that works.
    world
        .send_raise_dispute(
            &[verify_instruction, raise_instruction],
            &buyer,
            market.pubkey,
            seller.pubkey(),
            receipt.order_id,
        )
        .expect("the measured transaction is a working one");
}

#[test]
fn test_raise_dispute_compute() {
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(56);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0x1A), usdc(80));
    let metadata = world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .expect("raise_dispute succeeds");

    // The default budget is 200,000 CU per instruction. Anything close to
    // it would mean an assumption is wrong rather than the budget.
    println!(
        "raise_dispute compute: {} units",
        metadata.compute_units_consumed
    );
    assert!(
        metadata.compute_units_consumed < 200_000,
        "raise_dispute consumed {} units",
        metadata.compute_units_consumed
    );
}

#[test]
fn test_raise_dispute_stack_frame() {
    // The account struct is the widest in the program, and unboxed it
    // overruns the 4KB stack frame. At build time that shows up as a
    // warning; at run time it shows up as an access violation in the
    // logs and a failed transaction, which is what this asserts against.
    let Scenario {
        mut world,
        market,
        seller,
        buyer,
        buyer_token_account,
        ..
    } = scenario(57);

    let receipt = receipt_for(&world, &market, &seller, &buyer, order_id(0x1B), usdc(80));
    let metadata = world
        .raise_dispute(
            &buyer,
            buyer_token_account,
            market.pubkey,
            &receipt,
            &market.receipt_signer,
            usdc(80),
        )
        .expect("raise_dispute succeeds");

    let stack_complaints: Vec<&String> = metadata
        .logs
        .iter()
        .filter(|line| line.contains("stack") || line.contains("Access violation"))
        .collect();
    assert!(
        stack_complaints.is_empty(),
        "raise_dispute reported a stack problem: {stack_complaints:?}"
    );
}
