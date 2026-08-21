//! The `World` test harness (docs/DESIGN-v2.md, Phase 1: "Build the
//! `World` test harness in this phase, not at the end"). One LiteSVM
//! instance, one method per instruction handler, and `assert_invariants`
//! called automatically at the end of every method so no test can forget
//! it. Each later phase adds its own handler methods here rather than
//! repeating setup per test.
//!
//! The test mint created in [`World::new`] has 6 decimals, matching real
//! USDC; Phases 2 and 3 inherit it unchanged (docs/DESIGN-v2.md,
//! "Instruction handlers").
//!
//! `mod common;` is textually included into every `tests/test_phaseN.rs`
//! binary separately, so each one compiles its own private copy of this
//! whole module. Rust's dead-code lint runs per binary, so a method used
//! by only one phase's test file (for example `grant_permit`, unused
//! from `test_phase1.rs`'s side) is flagged as unused there even though
//! another sibling binary calls it. Same shape of issue as
//! `instructions.rs`'s `#![allow(ambiguous_glob_reexports)]`: a real
//! per-binary Rust fact, not a sign of actually-dead code.
#![allow(dead_code)]

use std::{
    collections::HashMap,
    env,
    time::{SystemTime, UNIX_EPOCH},
};

use anchor_lang::{
    prelude::*,
    solana_program::{program_pack::Pack, system_instruction},
    AccountDeserialize, InstructionData, ToAccountMetas,
};
use anchor_spl::{token::spl_token, token_2022::spl_token_2022};
use litesvm::{
    types::{FailedTransactionMetadata, TransactionResult},
    LiteSVM,
};
use solana_ed25519_program::new_ed25519_instruction_with_signature;
use solana_instruction::{error::InstructionError, Instruction};
use solana_keypair::{read_keypair_file, Keypair};
use solana_message::{Message, VersionedMessage};
use solana_precompile_error::PrecompileError;
use solana_signer::Signer;
use solana_transaction::versioned::VersionedTransaction;
use solana_transaction_error::TransactionError;

use truststake::{
    constants::{
        BOND_VAULT_SEED, CONFIG_SEED, DISPUTE_SEED, INITIAL_ADMIN, MARKETPLACE_SEED, PERMIT_SEED,
        SEED_VERSION, STAKE_SEED, VAULT_SEED,
    },
    receipt::OrderReceipt,
    state::{Config, DisputeRecord, DisputeStatus, Marketplace, SellerStake, SlashPermit},
};

const SOL: u64 = 1_000_000_000;

/// Matches real USDC, so the figures in test assertions read like dollars
/// and every decimals-dependent code path is exercised honestly.
pub const TEST_MINT_DECIMALS: u8 = 6;

/// Converts a dollar-readable major-unit amount into the minor units the
/// program actually operates on, e.g. `usdc(150)` is `150_000_000`.
pub fn usdc(major_units: u64) -> u64 {
    major_units * 10u64.pow(TEST_MINT_DECIMALS as u32)
}

/// The deploy wallet, the same one `constants::INITIAL_ADMIN` compiles in.
/// Read fresh from disk each call, rather than stored on `World`, so
/// callers never fight the borrow checker over `world.initialize_config(&world.field, ...)`.
///
/// Path resolution: `TRUSTSTAKE_ADMIN_KEYPAIR` env var if set, else
/// `~/.config/solana/id.json` (the Solana CLI default, matching
/// `examples/devnet_demo.rs`'s convention) for local convenience. Either
/// way, `World::new()` loads the real `target/deploy/truststake.so`, built
/// with the real `INITIAL_ADMIN` baked in, and `initialize_config` accepts
/// no other signer by design, so the loaded keypair's pubkey must match it
/// exactly. Checked here, not left to surface downstream: without this
/// check, a wrong keypair fails `initialize_config` with `NotInitialAdmin`
/// and then cascades into every other test that depends on `Config`
/// existing, which reads as a wall of unrelated failures instead of the
/// one wrong-keypair problem it actually is.
pub fn initial_admin_keypair() -> Keypair {
    let path = env::var("TRUSTSTAKE_ADMIN_KEYPAIR").unwrap_or_else(|_| {
        format!(
            "{}/.config/solana/id.json",
            env::var("HOME").expect("HOME must be set to locate the Solana CLI keypair")
        )
    });
    let keypair =
        read_keypair_file(&path).unwrap_or_else(|error| panic!("failed to read keypair at {path}: {error}"));
    assert_eq!(
        keypair.pubkey(),
        INITIAL_ADMIN,
        "keypair at {path} (pubkey {}) does not match constants::INITIAL_ADMIN ({}); \
         set TRUSTSTAKE_ADMIN_KEYPAIR to the deploy wallet's keypair file",
        keypair.pubkey(),
        INITIAL_ADMIN,
    );
    keypair
}

/// Sends a transaction directly against the harness's `LiteSVM`, with no
/// `assert_invariants` call. Every `World` method below wraps this and
/// checks invariants afterward; call this bare form only when a test
/// needs to bypass that on purpose, such as
/// `test_conservation_asserted_onchain` deliberately leaving the ledger
/// and the vault disagreeing to prove the program's own runtime check
/// (not the harness's) is what catches it.
pub fn send(
    svm: &mut LiteSVM,
    instructions: &[Instruction],
    payer: &Pubkey,
    signers: &[&Keypair],
) -> TransactionResult {
    let blockhash = svm.latest_blockhash();
    let message = Message::new_with_blockhash(instructions, Some(payer), &blockhash);
    let transaction = VersionedTransaction::try_new(VersionedMessage::Legacy(message), signers)
        .expect("transaction signing must succeed");
    svm.send_transaction(transaction)
}

/// The two accounts `#[event_cpi]` appends to every accounts struct in
/// this program: the `event_authority` PDA and the program itself.
pub fn event_cpi_accounts(program_id: &Pubkey) -> (Pubkey, Pubkey) {
    let (event_authority, _bump) = Pubkey::find_program_address(&[b"__event_authority"], program_id);
    (event_authority, *program_id)
}

/// The canonical single-signature Ed25519 instruction verifying
/// `signer`'s signature over `receipt`, which is the first half of every
/// well-formed `raise_dispute` transaction. Attack tests that need a
/// malformed one build it themselves rather than bending this.
pub fn ed25519_verify_instruction(receipt: &OrderReceipt, signer: &Keypair) -> Instruction {
    let message = receipt.message();
    let signature: [u8; 64] = signer.sign_message(&message).into();
    new_ed25519_instruction_with_signature(&message, &signature, &signer.pubkey().to_bytes())
}

/// Asserts that `result` failed with exactly `InstructionError::Custom(expected_code)`,
/// never merely that it failed (docs/TESTING.md: "every negative test
/// asserts the specific error"). Build `expected_code` with
/// `u32::from(truststake::error::TrustStakeError::Variant)` for a program
/// error, or `u32::from(anchor_lang::error::ErrorCode::Variant)` for an
/// Anchor constraint error.
pub fn assert_error_code(result: &TransactionResult, expected_code: u32) {
    match result {
        Err(FailedTransactionMetadata {
            err: TransactionError::InstructionError(_, InstructionError::Custom(code)),
            ..
        }) => assert_eq!(*code, expected_code, "wrong error code"),
        other => panic!("expected InstructionError::Custom({expected_code}), got {other:?}"),
    }
}

/// Asserts the transaction failed inside the Ed25519 precompile at
/// instruction 0, before `raise_dispute` ran at all. A precompile
/// reports its error as `InstructionError::Custom(discriminant)`, which
/// is the same shape a program error takes, so the instruction index is
/// what separates "this signature does not verify" from "the program
/// rejected this receipt".
pub fn assert_precompile_error(result: &TransactionResult, expected: PrecompileError) {
    match result {
        Err(FailedTransactionMetadata {
            err: TransactionError::InstructionError(0, InstructionError::Custom(code)),
            ..
        }) => assert_eq!(*code, expected as u32, "wrong precompile error"),
        other => panic!("expected the Ed25519 precompile to fail with {expected:?}, got {other:?}"),
    }
}

pub struct World {
    pub svm: LiteSVM,
    pub program_id: Pubkey,
    /// The protocol's pinned collateral mint: 6 decimals, Classic Token
    /// Program, matching real USDC.
    pub mint: Pubkey,
    mint_authority: Keypair,
    /// Generic fee-payer for harness setup (mint/token-account creation).
    /// Distinct from `initial_admin_keypair()`, which is reserved for
    /// calls that specifically need the compiled-in protocol admin.
    payer: Keypair,
    /// Total minor units of `mint` ever minted by this harness. Every
    /// token account this harness knows about (`stakes`' vaults,
    /// `marketplaces`' bond vaults, and `token_accounts`) must sum to
    /// exactly this, which is the "nothing is created or destroyed"
    /// invariant (docs/TESTING.md).
    minted_total: u64,
    stakes: Vec<Pubkey>,
    marketplaces: Vec<Pubkey>,
    /// Every permit ever successfully granted, by PDA. Deliberately not
    /// pruned when a permit is released and its account closed:
    /// `assert_invariants` treats a tracked pubkey with no account behind
    /// it as released and skips it, and dedupes on push, so re-granting
    /// at the same (seller, marketplace) address after a release is safe
    /// to track twice.
    permits: Vec<Pubkey>,
    /// Every dispute ever successfully raised, by PDA. Not pruned when
    /// `close_dispute` deletes one, for the same reason `permits` is not:
    /// `assert_invariants` treats a tracked pubkey with no account behind
    /// it as closed and skips it.
    disputes: Vec<Pubkey>,
    token_accounts: Vec<Pubkey>,
}

impl World {
    pub fn new() -> World {
        let program_id = truststake::id();
        let mut svm = LiteSVM::new();
        svm.add_program(program_id, include_bytes!("../../../../target/deploy/truststake.so"))
            .expect("failed to load truststake.so; run `anchor build` first");
        // The CPI and wrong-program fixture (programs/cpi_wrapper), loaded
        // at its own declared ID so its Anchor entrypoint accepts the
        // calls. Present in every World rather than only where it is used:
        // it is a few kilobytes and one `add_program` call.
        svm.add_program(
            cpi_wrapper::ID,
            include_bytes!("../../../../target/deploy/cpi_wrapper.so"),
        )
        .expect("failed to load cpi_wrapper.so; run `anchor build` first");

        // LiteSVM's default Clock sysvar starts at unix_timestamp 0. A real
        // cluster's timestamp is never 0, and `update_marketplace` uses 0 as
        // the "never rotated" sentinel for `signer_rotated_at` (a genuine
        // rotation must never collide with it), so every timestamp-reading
        // handler needs a realistic clock from the first transaction on.
        let mut clock = svm.get_sysvar::<Clock>();
        clock.unix_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time must be after the unix epoch")
            .as_secs() as i64;
        svm.set_sysvar(&clock);

        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 1_000 * SOL).expect("airdrop payer");

        let admin = initial_admin_keypair();
        svm.airdrop(&admin.pubkey(), 1_000 * SOL)
            .expect("airdrop initial admin");

        let mint_authority = Keypair::new();
        svm.airdrop(&mint_authority.pubkey(), 10 * SOL)
            .expect("airdrop mint authority");

        let mint = Keypair::new();
        let space = spl_token::state::Mint::LEN;
        let rent = svm.minimum_balance_for_rent_exemption(space);
        let create_mint_account = system_instruction::create_account(
            &payer.pubkey(),
            &mint.pubkey(),
            rent,
            space as u64,
            &spl_token::ID,
        );
        let initialize_mint = spl_token::instruction::initialize_mint2(
            &spl_token::ID,
            &mint.pubkey(),
            &mint_authority.pubkey(),
            None,
            TEST_MINT_DECIMALS,
        )
        .expect("build initialize_mint2 instruction");

        send(
            &mut svm,
            &[create_mint_account, initialize_mint],
            &payer.pubkey(),
            &[&payer, &mint],
        )
        .expect("create the test collateral mint");

        World {
            svm,
            program_id,
            mint: mint.pubkey(),
            mint_authority,
            payer,
            minted_total: 0,
            stakes: Vec::new(),
            marketplaces: Vec::new(),
            permits: Vec::new(),
            disputes: Vec::new(),
            token_accounts: Vec::new(),
        }
    }

    // ---- PDA derivation ----

    pub fn config_pda(&self) -> Pubkey {
        Pubkey::find_program_address(&[CONFIG_SEED, SEED_VERSION], &self.program_id).0
    }

    pub fn marketplace_pda(&self, marketplace_id: &[u8; 16]) -> Pubkey {
        Pubkey::find_program_address(
            &[MARKETPLACE_SEED, SEED_VERSION, marketplace_id.as_ref()],
            &self.program_id,
        )
        .0
    }

    pub fn bond_vault_pda(&self, marketplace: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(
            &[BOND_VAULT_SEED, SEED_VERSION, marketplace.as_ref()],
            &self.program_id,
        )
        .0
    }

    pub fn stake_pda(&self, seller: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(&[STAKE_SEED, SEED_VERSION, seller.as_ref()], &self.program_id).0
    }

    pub fn stake_vault_pda(&self, seller: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(&[VAULT_SEED, SEED_VERSION, seller.as_ref()], &self.program_id).0
    }

    pub fn permit_pda(&self, seller: &Pubkey, marketplace: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(
            &[PERMIT_SEED, SEED_VERSION, seller.as_ref(), marketplace.as_ref()],
            &self.program_id,
        )
        .0
    }

    pub fn dispute_pda(&self, marketplace: &Pubkey, seller: &Pubkey, order_id: &[u8; 32]) -> Pubkey {
        Pubkey::find_program_address(
            &[DISPUTE_SEED, SEED_VERSION, marketplace.as_ref(), seller.as_ref(), order_id.as_ref()],
            &self.program_id,
        )
        .0
    }

    /// Looks a dispute up by marketplace and order among tracked records
    /// rather than re-deriving its PDA: the seed also includes the
    /// seller, which these callers only learn by reading the record they
    /// are trying to find.
    fn find_dispute(&self, marketplace: &Pubkey, order_id: &[u8; 32]) -> Pubkey {
        self.disputes
            .iter()
            .copied()
            .find(|pubkey| {
                self.try_read_dispute(pubkey)
                    .is_some_and(|dispute| dispute.marketplace == *marketplace && dispute.order_id == *order_id)
            })
            .unwrap_or_else(|| panic!("no tracked dispute for marketplace {marketplace} order {order_id:?}"))
    }

    // ---- account readers ----

    pub fn read_config(&self) -> Config {
        let account = self
            .svm
            .get_account(&self.config_pda())
            .expect("config account must exist");
        Config::try_deserialize(&mut account.data.as_slice()).expect("valid Config data")
    }

    pub fn read_marketplace(&self, marketplace: &Pubkey) -> Marketplace {
        let account = self
            .svm
            .get_account(marketplace)
            .expect("marketplace account must exist");
        Marketplace::try_deserialize(&mut account.data.as_slice()).expect("valid Marketplace data")
    }

    pub fn read_seller_stake(&self, stake: &Pubkey) -> SellerStake {
        let account = self.svm.get_account(stake).expect("stake account must exist");
        SellerStake::try_deserialize(&mut account.data.as_slice()).expect("valid SellerStake data")
    }

    pub fn read_permit(&self, permit: &Pubkey) -> SlashPermit {
        let account = self.svm.get_account(permit).expect("permit account must exist");
        SlashPermit::try_deserialize(&mut account.data.as_slice()).expect("valid SlashPermit data")
    }

    /// `None` once `release_permit`/`release_permit_early` has closed the
    /// account; the permit half of `assert_invariants` uses this rather
    /// than `read_permit` to skip released permits instead of panicking.
    pub fn try_read_permit(&self, permit: &Pubkey) -> Option<SlashPermit> {
        let account = self.svm.get_account(permit)?;
        Some(SlashPermit::try_deserialize(&mut account.data.as_slice()).expect("valid SlashPermit data"))
    }

    pub fn read_dispute(&self, dispute: &Pubkey) -> DisputeRecord {
        let account = self.svm.get_account(dispute).expect("dispute account must exist");
        DisputeRecord::try_deserialize(&mut account.data.as_slice()).expect("valid DisputeRecord data")
    }

    /// `None` once `close_dispute` has deleted the record.
    pub fn try_read_dispute(&self, dispute: &Pubkey) -> Option<DisputeRecord> {
        let account = self.svm.get_account(dispute)?;
        Some(DisputeRecord::try_deserialize(&mut account.data.as_slice()).expect("valid DisputeRecord data"))
    }

    pub fn read_token_account(&self, pubkey: &Pubkey) -> spl_token::state::Account {
        let account = self.svm.get_account(pubkey).expect("token account must exist");
        spl_token::state::Account::unpack(&account.data).expect("valid token account data")
    }

    pub fn token_balance(&self, pubkey: &Pubkey) -> u64 {
        self.read_token_account(pubkey).amount
    }

    // ---- clock ----

    /// The harness's current `Clock` timestamp, which is what every
    /// handler reads and what receipts are dated against.
    pub fn now(&self) -> i64 {
        self.svm.get_sysvar::<Clock>().unix_timestamp
    }

    /// Advances the harness's `Clock` sysvar by `seconds`, for boundary
    /// tests on timestamp-gated handlers (`release_permit`'s complaint
    /// window, and dispute expiry). LiteSVM does not advance wall-clock
    /// time on its own between transactions.
    pub fn warp_seconds(&mut self, seconds: i64) {
        let mut clock = self.svm.get_sysvar::<Clock>();
        clock.unix_timestamp = clock
            .unix_timestamp
            .checked_add(seconds)
            .expect("test clock warp must not overflow i64");
        self.svm.set_sysvar(&clock);
    }

    // ---- token test fixtures ----

    /// A second, legitimate Classic Token Program mint, distinct from
    /// `self.mint`. Used only to prove a handler rejects it, never to
    /// hold real collateral.
    pub fn create_extra_mint(&mut self) -> Pubkey {
        let mint = Keypair::new();
        let authority = Keypair::new();
        let space = spl_token::state::Mint::LEN;
        let rent = self.svm.minimum_balance_for_rent_exemption(space);
        let create_mint_account = system_instruction::create_account(
            &self.payer.pubkey(),
            &mint.pubkey(),
            rent,
            space as u64,
            &spl_token::ID,
        );
        let initialize_mint = spl_token::instruction::initialize_mint2(
            &spl_token::ID,
            &mint.pubkey(),
            &authority.pubkey(),
            None,
            TEST_MINT_DECIMALS,
        )
        .expect("build initialize_mint2 instruction");

        send(
            &mut self.svm,
            &[create_mint_account, initialize_mint],
            &self.payer.pubkey(),
            &[&self.payer, &mint],
        )
        .expect("create extra mint");

        mint.pubkey()
    }

    /// A Token Extensions mint carrying a transfer-fee extension. Proves
    /// the mint-binding checks reject a fee-bearing mint as a
    /// non-matching mint, rather than silently under-transferring
    /// (docs/TESTING.md, `test_transfer_fee_mint_rejected`).
    pub fn create_transfer_fee_mint(&mut self, fee_bps: u16, maximum_fee: u64) -> Pubkey {
        let mint = Keypair::new();
        let space = spl_token_2022::extension::ExtensionType::try_calculate_account_len::<
            spl_token_2022::state::Mint,
        >(&[spl_token_2022::extension::ExtensionType::TransferFeeConfig])
        .expect("compute Token-2022 mint space with a transfer-fee extension");
        let rent = self.svm.minimum_balance_for_rent_exemption(space);

        let create_mint_account = system_instruction::create_account(
            &self.payer.pubkey(),
            &mint.pubkey(),
            rent,
            space as u64,
            &spl_token_2022::ID,
        );
        // Extension init must run before InitializeMint2 finalizes the
        // account; that is the order the Token Extensions Program requires.
        let initialize_fee_config = spl_token_2022::extension::transfer_fee::instruction::initialize_transfer_fee_config(
            &spl_token_2022::ID,
            &mint.pubkey(),
            None,
            None,
            fee_bps,
            maximum_fee,
        )
        .expect("build initialize_transfer_fee_config instruction");
        let initialize_mint = spl_token_2022::instruction::initialize_mint2(
            &spl_token_2022::ID,
            &mint.pubkey(),
            &self.mint_authority.pubkey(),
            None,
            TEST_MINT_DECIMALS,
        )
        .expect("build initialize_mint2 instruction");

        send(
            &mut self.svm,
            &[create_mint_account, initialize_fee_config, initialize_mint],
            &self.payer.pubkey(),
            &[&self.payer, &mint],
        )
        .expect("create transfer-fee mint");

        mint.pubkey()
    }

    /// Creates a Classic Token Program account for `mint`, owned by
    /// `owner`, optionally pre-funded. Only `mint == self.mint` balances
    /// are tracked for the conservation invariant; fixtures under a
    /// foreign mint (`create_extra_mint`, `create_transfer_fee_mint`)
    /// never hold real collateral.
    pub fn create_funded_token_account(&mut self, mint: Pubkey, owner: Pubkey, amount: u64) -> Pubkey {
        let token_account = Keypair::new();
        let space = spl_token::state::Account::LEN;
        let rent = self.svm.minimum_balance_for_rent_exemption(space);
        let create_account_instruction = system_instruction::create_account(
            &self.payer.pubkey(),
            &token_account.pubkey(),
            rent,
            space as u64,
            &spl_token::ID,
        );
        let initialize_account_instruction = spl_token::instruction::initialize_account3(
            &spl_token::ID,
            &token_account.pubkey(),
            &mint,
            &owner,
        )
        .expect("build initialize_account3 instruction");

        let mut instructions = vec![create_account_instruction, initialize_account_instruction];
        let mut signers: Vec<&Keypair> = vec![&self.payer, &token_account];
        if amount > 0 {
            instructions.push(
                spl_token::instruction::mint_to(
                    &spl_token::ID,
                    &mint,
                    &token_account.pubkey(),
                    &self.mint_authority.pubkey(),
                    &[],
                    amount,
                )
                .expect("build mint_to instruction"),
            );
            signers.push(&self.mint_authority);
        }

        send(&mut self.svm, &instructions, &self.payer.pubkey(), &signers)
            .expect("create funded token account");

        if mint == self.mint {
            self.token_accounts.push(token_account.pubkey());
            if amount > 0 {
                self.minted_total = self
                    .minted_total
                    .checked_add(amount)
                    .expect("minted_total overflow");
            }
        }

        token_account.pubkey()
    }

    /// Mints more of the collateral mint into an existing token account,
    /// for tests that need a balance topped up after it was created.
    /// Tracked in `minted_total` like every other mint, so the
    /// conservation invariant still holds afterwards.
    pub fn mint_to(&mut self, token_account: &Pubkey, amount: u64) {
        let instruction = spl_token::instruction::mint_to(
            &spl_token::ID,
            &self.mint,
            token_account,
            &self.mint_authority.pubkey(),
            &[],
            amount,
        )
        .expect("build mint_to instruction");

        send(
            &mut self.svm,
            &[instruction],
            &self.payer.pubkey(),
            &[&self.payer, &self.mint_authority],
        )
        .expect("mint into an existing token account");

        self.minted_total = self
            .minted_total
            .checked_add(amount)
            .expect("minted_total overflow");
    }

    /// Sends a fully custom instruction set, for the handful of tests
    /// that substitute an account the convenience methods below don't
    /// have a parameter for (account-type substitution, for example).
    /// Still runs `assert_invariants` afterward like every other method.
    pub fn send_instructions(
        &mut self,
        instructions: &[Instruction],
        payer: &Pubkey,
        signers: &[&Keypair],
    ) -> TransactionResult {
        let result = send(&mut self.svm, instructions, payer, signers);
        assert_invariants(self);
        result
    }

    // ---- instruction handlers ----

    pub fn initialize_config(&mut self, admin: &Keypair, chain_id: u8) -> TransactionResult {
        let config = self.config_pda();
        let (event_authority, program) = event_cpi_accounts(&self.program_id);

        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &truststake::instruction::InitializeConfig { chain_id }.data(),
            truststake::accounts::InitializeConfigAccountConstraints {
                admin: admin.pubkey(),
                config,
                mint: self.mint,
                system_program: anchor_lang::system_program::ID,
                event_authority,
                program,
            }
            .to_account_metas(None),
        );

        let result = send(&mut self.svm, &[instruction], &admin.pubkey(), &[admin]);
        assert_invariants(self);
        result
    }

    pub fn propose_config_authority(&mut self, authority: &Keypair, new_authority: Pubkey) -> TransactionResult {
        let config = self.config_pda();
        let (event_authority, program) = event_cpi_accounts(&self.program_id);

        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &truststake::instruction::ProposeConfigAuthority { new_authority }.data(),
            truststake::accounts::ProposeConfigAuthorityAccountConstraints {
                authority: authority.pubkey(),
                config,
                event_authority,
                program,
            }
            .to_account_metas(None),
        );

        let result = send(&mut self.svm, &[instruction], &authority.pubkey(), &[authority]);
        assert_invariants(self);
        result
    }

    pub fn accept_config_authority(&mut self, pending_authority: &Keypair) -> TransactionResult {
        let config = self.config_pda();
        let (event_authority, program) = event_cpi_accounts(&self.program_id);

        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &truststake::instruction::AcceptConfigAuthority {}.data(),
            truststake::accounts::AcceptConfigAuthorityAccountConstraints {
                pending_authority: pending_authority.pubkey(),
                config,
                event_authority,
                program,
            }
            .to_account_metas(None),
        );

        let result = send(
            &mut self.svm,
            &[instruction],
            &pending_authority.pubkey(),
            &[pending_authority],
        );
        assert_invariants(self);
        result
    }

    #[allow(clippy::too_many_arguments)]
    pub fn register_marketplace(
        &mut self,
        authority: &Keypair,
        marketplace_id: [u8; 16],
        receipt_signer: Pubkey,
        arbiter: Pubkey,
        complaint_window: i64,
        bond_bps: u16,
    ) -> TransactionResult {
        let config = self.config_pda();
        let marketplace = self.marketplace_pda(&marketplace_id);
        let bond_vault = self.bond_vault_pda(&marketplace);
        let (event_authority, program) = event_cpi_accounts(&self.program_id);

        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &truststake::instruction::RegisterMarketplace {
                marketplace_id,
                receipt_signer,
                arbiter,
                complaint_window,
                bond_bps,
            }
            .data(),
            truststake::accounts::RegisterMarketplaceAccountConstraints {
                authority: authority.pubkey(),
                config,
                mint: self.mint,
                marketplace,
                bond_vault,
                token_program: spl_token::ID,
                system_program: anchor_lang::system_program::ID,
                event_authority,
                program,
            }
            .to_account_metas(None),
        );

        let result = send(&mut self.svm, &[instruction], &authority.pubkey(), &[authority]);
        if result.is_ok() {
            self.marketplaces.push(marketplace);
        }
        assert_invariants(self);
        result
    }

    pub fn update_marketplace(
        &mut self,
        authority: &Keypair,
        marketplace: Pubkey,
        new_receipt_signer: Option<Pubkey>,
        new_arbiter: Option<Pubkey>,
        new_complaint_window: Option<i64>,
        new_bond_bps: Option<u16>,
    ) -> TransactionResult {
        let (event_authority, program) = event_cpi_accounts(&self.program_id);

        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &truststake::instruction::UpdateMarketplace {
                new_receipt_signer,
                new_arbiter,
                new_complaint_window,
                new_bond_bps,
            }
            .data(),
            truststake::accounts::UpdateMarketplaceAccountConstraints {
                authority: authority.pubkey(),
                marketplace,
                event_authority,
                program,
            }
            .to_account_metas(None),
        );

        let result = send(&mut self.svm, &[instruction], &authority.pubkey(), &[authority]);
        assert_invariants(self);
        result
    }

    pub fn propose_marketplace_authority(
        &mut self,
        authority: &Keypair,
        marketplace: Pubkey,
        new_authority: Pubkey,
    ) -> TransactionResult {
        let (event_authority, program) = event_cpi_accounts(&self.program_id);

        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &truststake::instruction::ProposeMarketplaceAuthority { new_authority }.data(),
            truststake::accounts::ProposeMarketplaceAuthorityAccountConstraints {
                authority: authority.pubkey(),
                marketplace,
                event_authority,
                program,
            }
            .to_account_metas(None),
        );

        let result = send(&mut self.svm, &[instruction], &authority.pubkey(), &[authority]);
        assert_invariants(self);
        result
    }

    pub fn accept_marketplace_authority(&mut self, pending_authority: &Keypair, marketplace: Pubkey) -> TransactionResult {
        let (event_authority, program) = event_cpi_accounts(&self.program_id);

        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &truststake::instruction::AcceptMarketplaceAuthority {}.data(),
            truststake::accounts::AcceptMarketplaceAuthorityAccountConstraints {
                pending_authority: pending_authority.pubkey(),
                marketplace,
                event_authority,
                program,
            }
            .to_account_metas(None),
        );

        let result = send(
            &mut self.svm,
            &[instruction],
            &pending_authority.pubkey(),
            &[pending_authority],
        );
        assert_invariants(self);
        result
    }

    pub fn initialize_stake(&mut self, seller: &Keypair) -> TransactionResult {
        let mint = self.mint;
        self.initialize_stake_with_mint(seller, mint, spl_token::ID)
    }

    /// Lower-level variant taking an explicit `mint` and the token
    /// program that actually owns it, for `test_wrong_mint_rejected` /
    /// `test_transfer_fee_mint_rejected`. The token program must match
    /// the mint's real owner (not always `spl_token::ID`): `stake_vault`'s
    /// `init` runs before `mint`'s own `address` constraint is checked
    /// (Anchor evaluates every `init` field first, then every other
    /// field's constraints, regardless of declaration order), so a
    /// mismatched token program fails inside that CPI with a generic
    /// Token Program error instead of exercising the `WrongMint` check
    /// this handler actually carries.
    pub fn initialize_stake_with_mint(
        &mut self,
        seller: &Keypair,
        mint: Pubkey,
        token_program: Pubkey,
    ) -> TransactionResult {
        let config = self.config_pda();
        let stake = self.stake_pda(&seller.pubkey());
        let stake_vault = self.stake_vault_pda(&seller.pubkey());
        let (event_authority, program) = event_cpi_accounts(&self.program_id);

        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &truststake::instruction::InitializeStake {}.data(),
            truststake::accounts::InitializeStakeAccountConstraints {
                seller: seller.pubkey(),
                config,
                mint,
                stake,
                stake_vault,
                token_program,
                system_program: anchor_lang::system_program::ID,
                event_authority,
                program,
            }
            .to_account_metas(None),
        );

        let result = send(&mut self.svm, &[instruction], &seller.pubkey(), &[seller]);
        if result.is_ok() {
            self.stakes.push(stake);
        }
        assert_invariants(self);
        result
    }

    fn add_stake_raw(
        &mut self,
        seller: &Keypair,
        accounts: truststake::accounts::AddStakeAccountConstraints,
        amount: u64,
    ) -> TransactionResult {
        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &truststake::instruction::AddStake { amount }.data(),
            accounts.to_account_metas(None),
        );
        let result = send(&mut self.svm, &[instruction], &seller.pubkey(), &[seller]);
        assert_invariants(self);
        result
    }

    pub fn add_stake(&mut self, seller: &Keypair, seller_token_account: Pubkey, amount: u64) -> TransactionResult {
        let mint = self.mint;
        let stake_vault = self.stake_vault_pda(&seller.pubkey());
        self.add_stake_with_accounts(seller, seller_token_account, mint, stake_vault, amount)
    }

    /// Lower-level variant taking an explicit `mint`, for
    /// `test_wrong_mint_rejected` / `test_unbound_mint_rejected` /
    /// `test_transfer_fee_mint_rejected`.
    pub fn add_stake_with_mint(
        &mut self,
        seller: &Keypair,
        seller_token_account: Pubkey,
        mint: Pubkey,
        amount: u64,
    ) -> TransactionResult {
        let stake_vault = self.stake_vault_pda(&seller.pubkey());
        self.add_stake_with_accounts(seller, seller_token_account, mint, stake_vault, amount)
    }

    /// Lower-level variant taking an explicit `stake_vault`, for
    /// `test_fake_vault_rejected`.
    pub fn add_stake_with_vault(
        &mut self,
        seller: &Keypair,
        seller_token_account: Pubkey,
        stake_vault: Pubkey,
        amount: u64,
    ) -> TransactionResult {
        let mint = self.mint;
        self.add_stake_with_accounts(seller, seller_token_account, mint, stake_vault, amount)
    }

    fn add_stake_with_accounts(
        &mut self,
        seller: &Keypair,
        seller_token_account: Pubkey,
        mint: Pubkey,
        stake_vault: Pubkey,
        amount: u64,
    ) -> TransactionResult {
        let stake = self.stake_pda(&seller.pubkey());
        let (event_authority, program) = event_cpi_accounts(&self.program_id);

        let accounts = truststake::accounts::AddStakeAccountConstraints {
            seller: seller.pubkey(),
            stake,
            stake_vault,
            mint,
            seller_token_account,
            token_program: spl_token::ID,
            event_authority,
            program,
        };

        self.add_stake_raw(seller, accounts, amount)
    }

    pub fn withdraw_stake(&mut self, seller: &Keypair, seller_token_account: Pubkey, amount: u64) -> TransactionResult {
        let mint = self.mint;
        let stake_vault = self.stake_vault_pda(&seller.pubkey());
        self.withdraw_stake_with_accounts(seller, seller_token_account, mint, stake_vault, amount)
    }

    /// Lower-level variant taking an explicit `mint`, for
    /// `test_wrong_mint_rejected` / `test_unbound_mint_rejected`.
    pub fn withdraw_stake_with_mint(
        &mut self,
        seller: &Keypair,
        seller_token_account: Pubkey,
        mint: Pubkey,
        amount: u64,
    ) -> TransactionResult {
        let stake_vault = self.stake_vault_pda(&seller.pubkey());
        self.withdraw_stake_with_accounts(seller, seller_token_account, mint, stake_vault, amount)
    }

    /// Lower-level variant taking an explicit `stake_vault`, for
    /// `test_fake_vault_rejected`.
    pub fn withdraw_stake_with_vault(
        &mut self,
        seller: &Keypair,
        seller_token_account: Pubkey,
        stake_vault: Pubkey,
        amount: u64,
    ) -> TransactionResult {
        let mint = self.mint;
        self.withdraw_stake_with_accounts(seller, seller_token_account, mint, stake_vault, amount)
    }

    fn withdraw_stake_with_accounts(
        &mut self,
        seller: &Keypair,
        seller_token_account: Pubkey,
        mint: Pubkey,
        stake_vault: Pubkey,
        amount: u64,
    ) -> TransactionResult {
        let stake = self.stake_pda(&seller.pubkey());
        let (event_authority, program) = event_cpi_accounts(&self.program_id);

        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &truststake::instruction::WithdrawStake { amount }.data(),
            truststake::accounts::WithdrawStakeAccountConstraints {
                seller: seller.pubkey(),
                stake,
                stake_vault,
                mint,
                seller_token_account,
                token_program: spl_token::ID,
                event_authority,
                program,
            }
            .to_account_metas(None),
        );

        let result = send(&mut self.svm, &[instruction], &seller.pubkey(), &[seller]);
        assert_invariants(self);
        result
    }

    pub fn grant_permit(&mut self, seller: &Keypair, marketplace: Pubkey, max_slashable: u64) -> TransactionResult {
        let stake = self.stake_pda(&seller.pubkey());
        let permit = self.permit_pda(&seller.pubkey(), &marketplace);
        let (event_authority, program) = event_cpi_accounts(&self.program_id);

        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &truststake::instruction::GrantPermit { max_slashable }.data(),
            truststake::accounts::GrantPermitAccountConstraints {
                seller: seller.pubkey(),
                stake,
                marketplace,
                permit,
                system_program: anchor_lang::system_program::ID,
                event_authority,
                program,
            }
            .to_account_metas(None),
        );

        let result = send(&mut self.svm, &[instruction], &seller.pubkey(), &[seller]);
        if result.is_ok() && !self.permits.contains(&permit) {
            self.permits.push(permit);
        }
        assert_invariants(self);
        result
    }

    pub fn increase_permit(&mut self, seller: &Keypair, marketplace: Pubkey, delta: u64) -> TransactionResult {
        let stake = self.stake_pda(&seller.pubkey());
        let permit = self.permit_pda(&seller.pubkey(), &marketplace);
        let (event_authority, program) = event_cpi_accounts(&self.program_id);

        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &truststake::instruction::IncreasePermit { delta }.data(),
            truststake::accounts::IncreasePermitAccountConstraints {
                seller: seller.pubkey(),
                stake,
                permit,
                event_authority,
                program,
            }
            .to_account_metas(None),
        );

        let result = send(&mut self.svm, &[instruction], &seller.pubkey(), &[seller]);
        assert_invariants(self);
        result
    }

    pub fn revoke_permit(&mut self, seller: &Keypair, marketplace: Pubkey) -> TransactionResult {
        let permit = self.permit_pda(&seller.pubkey(), &marketplace);
        let (event_authority, program) = event_cpi_accounts(&self.program_id);

        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &truststake::instruction::RevokePermit {}.data(),
            truststake::accounts::RevokePermitAccountConstraints {
                seller: seller.pubkey(),
                permit,
                event_authority,
                program,
            }
            .to_account_metas(None),
        );

        let result = send(&mut self.svm, &[instruction], &seller.pubkey(), &[seller]);
        assert_invariants(self);
        result
    }

    /// `caller` may be any funded keypair: `release_permit` is
    /// permissionless by design. `seller` is passed separately (rather
    /// than derived) because the whole point of this handler is that it
    /// need not be a signer.
    pub fn release_permit(&mut self, caller: &Keypair, seller: Pubkey, marketplace: Pubkey) -> TransactionResult {
        let stake = self.stake_pda(&seller);
        let permit = self.permit_pda(&seller, &marketplace);
        let (event_authority, program) = event_cpi_accounts(&self.program_id);

        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &truststake::instruction::ReleasePermit {}.data(),
            truststake::accounts::ReleasePermitAccountConstraints {
                caller: caller.pubkey(),
                seller,
                permit,
                stake,
                event_authority,
                program,
            }
            .to_account_metas(None),
        );

        let result = send(&mut self.svm, &[instruction], &caller.pubkey(), &[caller]);
        assert_invariants(self);
        result
    }

    pub fn release_permit_early(
        &mut self,
        seller: &Keypair,
        authority: &Keypair,
        marketplace: Pubkey,
    ) -> TransactionResult {
        let stake = self.stake_pda(&seller.pubkey());
        let permit = self.permit_pda(&seller.pubkey(), &marketplace);
        let (event_authority, program) = event_cpi_accounts(&self.program_id);

        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &truststake::instruction::ReleasePermitEarly {}.data(),
            truststake::accounts::ReleasePermitEarlyAccountConstraints {
                seller: seller.pubkey(),
                authority: authority.pubkey(),
                marketplace,
                permit,
                stake,
                event_authority,
                program,
            }
            .to_account_metas(None),
        );

        let result = send(
            &mut self.svm,
            &[instruction],
            &seller.pubkey(),
            &[seller, authority],
        );
        assert_invariants(self);
        result
    }

    // ---- disputes ----

    /// The `raise_dispute` instruction on its own, so the signature-check
    /// tests can pair it with a hand-built, misplaced, or entirely absent
    /// Ed25519 instruction.
    #[allow(clippy::too_many_arguments)]
    pub fn raise_dispute_instruction(
        &self,
        buyer: Pubkey,
        buyer_token_account: Pubkey,
        marketplace: Pubkey,
        seller: Pubkey,
        order_id: [u8; 32],
        claim: u64,
    ) -> Instruction {
        self.raise_dispute_instruction_with_sysvar(
            buyer,
            buyer_token_account,
            marketplace,
            seller,
            order_id,
            claim,
            solana_instructions_sysvar::ID,
        )
    }

    /// Lower-level variant taking whatever account should occupy the
    /// Instructions sysvar slot, for
    /// `test_introspection_rejects_forged_sysvar`.
    #[allow(clippy::too_many_arguments)]
    pub fn raise_dispute_instruction_with_sysvar(
        &self,
        buyer: Pubkey,
        buyer_token_account: Pubkey,
        marketplace: Pubkey,
        seller: Pubkey,
        order_id: [u8; 32],
        claim: u64,
        instructions_sysvar: Pubkey,
    ) -> Instruction {
        let (event_authority, program) = event_cpi_accounts(&self.program_id);

        Instruction::new_with_bytes(
            self.program_id,
            &truststake::instruction::RaiseDispute { order_id, claim }.data(),
            truststake::accounts::RaiseDisputeAccountConstraints {
                buyer,
                config: self.config_pda(),
                marketplace,
                stake: self.stake_pda(&seller),
                permit: self.permit_pda(&seller, &marketplace),
                dispute: self.dispute_pda(&marketplace, &seller, &order_id),
                bond_vault: self.bond_vault_pda(&marketplace),
                mint: self.mint,
                buyer_token_account,
                instructions_sysvar,
                token_program: spl_token::ID,
                system_program: anchor_lang::system_program::ID,
                event_authority,
                program,
            }
            .to_account_metas(None),
        )
    }

    /// Sends an already-built dispute transaction, tracking the record so
    /// `assert_invariants` starts counting it. Every signature-check test
    /// goes through here with its own instruction list.
    pub fn send_raise_dispute(
        &mut self,
        instructions: &[Instruction],
        buyer: &Keypair,
        marketplace: Pubkey,
        seller: Pubkey,
        order_id: [u8; 32],
    ) -> TransactionResult {
        let result = send(&mut self.svm, instructions, &buyer.pubkey(), &[buyer]);
        let dispute = self.dispute_pda(&marketplace, &seller, &order_id);
        if result.is_ok() && !self.disputes.contains(&dispute) {
            self.disputes.push(dispute);
        }
        assert_invariants(self);
        result
    }

    /// The whole two-instruction transaction: the Ed25519 verification of
    /// `receipt` by `receipt_signer`, then `raise_dispute` against the
    /// seller and order the receipt itself names.
    pub fn raise_dispute(
        &mut self,
        buyer: &Keypair,
        buyer_token_account: Pubkey,
        marketplace: Pubkey,
        receipt: &OrderReceipt,
        receipt_signer: &Keypair,
        claim: u64,
    ) -> TransactionResult {
        self.raise_dispute_against(
            buyer,
            buyer_token_account,
            marketplace,
            receipt.seller,
            receipt.order_id,
            receipt,
            receipt_signer,
            claim,
        )
    }

    /// Lower-level variant taking the seller and order ID the
    /// *instruction* names, which the happy path derives from the receipt.
    /// Splitting them is what lets a test file a valid signature over one
    /// seller's order against a different seller's collateral.
    #[allow(clippy::too_many_arguments)]
    pub fn raise_dispute_against(
        &mut self,
        buyer: &Keypair,
        buyer_token_account: Pubkey,
        marketplace: Pubkey,
        seller: Pubkey,
        order_id: [u8; 32],
        receipt: &OrderReceipt,
        receipt_signer: &Keypair,
        claim: u64,
    ) -> TransactionResult {
        let verify_instruction = ed25519_verify_instruction(receipt, receipt_signer);
        let raise_instruction = self.raise_dispute_instruction(
            buyer.pubkey(),
            buyer_token_account,
            marketplace,
            seller,
            order_id,
            claim,
        );
        self.send_raise_dispute(
            &[verify_instruction, raise_instruction],
            buyer,
            marketplace,
            seller,
            order_id,
        )
    }

    pub fn resolve_dispute(
        &mut self,
        arbiter: &Keypair,
        marketplace: Pubkey,
        order_id: [u8; 32],
        buyer_token_account: Pubkey,
        upheld: bool,
    ) -> TransactionResult {
        let dispute = self.find_dispute(&marketplace, &order_id);
        let seller = self.read_dispute(&dispute).seller;
        let (event_authority, program) = event_cpi_accounts(&self.program_id);

        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &truststake::instruction::ResolveDispute { upheld }.data(),
            truststake::accounts::ResolveDisputeAccountConstraints {
                arbiter: arbiter.pubkey(),
                marketplace,
                dispute,
                permit: self.permit_pda(&seller, &marketplace),
                stake: self.stake_pda(&seller),
                stake_vault: self.stake_vault_pda(&seller),
                bond_vault: self.bond_vault_pda(&marketplace),
                mint: self.mint,
                buyer_token_account,
                token_program: spl_token::ID,
                event_authority,
                program,
            }
            .to_account_metas(None),
        );

        let result = send(&mut self.svm, &[instruction], &arbiter.pubkey(), &[arbiter]);
        assert_invariants(self);
        result
    }

    /// `caller` may be any funded keypair: expiry is permissionless, which
    /// is the whole protection against a marketplace that stops
    /// answering.
    pub fn expire_dispute(
        &mut self,
        caller: &Keypair,
        marketplace: Pubkey,
        order_id: [u8; 32],
        buyer_token_account: Pubkey,
    ) -> TransactionResult {
        let dispute = self.find_dispute(&marketplace, &order_id);
        let seller = self.read_dispute(&dispute).seller;
        let (event_authority, program) = event_cpi_accounts(&self.program_id);

        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &truststake::instruction::ExpireDispute {}.data(),
            truststake::accounts::ExpireDisputeAccountConstraints {
                caller: caller.pubkey(),
                marketplace,
                dispute,
                permit: self.permit_pda(&seller, &marketplace),
                bond_vault: self.bond_vault_pda(&marketplace),
                mint: self.mint,
                buyer_token_account,
                token_program: spl_token::ID,
                event_authority,
                program,
            }
            .to_account_metas(None),
        );

        let result = send(&mut self.svm, &[instruction], &caller.pubkey(), &[caller]);
        assert_invariants(self);
        result
    }

    /// Permissionless too; `buyer` is read off the record rather than
    /// passed, since it is the rent destination and nothing else.
    pub fn close_dispute(&mut self, caller: &Keypair, marketplace: Pubkey, order_id: [u8; 32]) -> TransactionResult {
        let dispute = self.find_dispute(&marketplace, &order_id);
        let buyer = self.read_dispute(&dispute).buyer;
        let (event_authority, program) = event_cpi_accounts(&self.program_id);

        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &truststake::instruction::CloseDispute {}.data(),
            truststake::accounts::CloseDisputeAccountConstraints {
                caller: caller.pubkey(),
                buyer,
                dispute,
                event_authority,
                program,
            }
            .to_account_metas(None),
        );

        let result = send(&mut self.svm, &[instruction], &caller.pubkey(), &[caller]);
        assert_invariants(self);
        result
    }
}

fn assert_rent_exempt(world: &World, pubkey: &Pubkey) {
    let account = world
        .svm
        .get_account(pubkey)
        .unwrap_or_else(|| panic!("tracked account {pubkey} must exist"));
    let minimum = world.svm.minimum_balance_for_rent_exemption(account.data.len());
    assert!(
        account.lamports >= minimum,
        "{pubkey} dropped below the rent-exempt minimum"
    );
}

/// Checked after every `World` method (docs/TESTING.md, "Invariants").
pub fn assert_invariants(world: &World) {
    let mut total_tracked: u128 = 0;

    if world.svm.get_account(&world.config_pda()).is_some() {
        assert_rent_exempt(world, &world.config_pda());
    }

    // Read every tracked dispute once, up front. A closed record's
    // account no longer exists, so it is skipped rather than panicking,
    // and only records still Open count: those are exactly the ones whose
    // bond is still sitting in a bond vault and whose freeze is still on
    // a permit.
    let mut open_disputes_by_permit: HashMap<(Pubkey, Pubkey), u32> = HashMap::new();
    let mut bonds_by_marketplace: HashMap<Pubkey, u128> = HashMap::new();
    for dispute_pubkey in &world.disputes {
        let Some(dispute) = world.try_read_dispute(dispute_pubkey) else {
            continue;
        };
        assert_rent_exempt(world, dispute_pubkey);

        if dispute.status != DisputeStatus::Open as u8 {
            continue;
        }
        *open_disputes_by_permit
            .entry((dispute.seller, dispute.marketplace))
            .or_insert(0) += 1;
        *bonds_by_marketplace.entry(dispute.marketplace).or_insert(0) += dispute.bond as u128;
    }

    // Every tracked permit: a released permit's account is gone, so it is
    // skipped and contributes nothing to its seller's committed sum.
    let mut committed_by_seller: HashMap<Pubkey, u128> = HashMap::new();
    for permit_pubkey in &world.permits {
        let Some(permit) = world.try_read_permit(permit_pubkey) else {
            continue;
        };

        assert!(
            permit.slashed <= permit.max_slashable,
            "permit.slashed must never exceed permit.max_slashable for {permit_pubkey}"
        );
        // The counter that gates release, against the records that are
        // the truth behind it. A counter that fails to decrement locks
        // the seller's collateral up forever; one that decrements twice
        // lets a permit be released with money still owed.
        let open_disputes = open_disputes_by_permit
            .get(&(permit.seller, permit.marketplace))
            .copied()
            .unwrap_or(0);
        assert_eq!(
            u32::from(permit.open_disputes),
            open_disputes,
            "open_disputes must equal the number of Open dispute records against {permit_pubkey}"
        );

        let remaining_allowance = (permit.max_slashable - permit.slashed) as u128;
        *committed_by_seller.entry(permit.seller).or_insert(0) += remaining_allowance;

        assert_rent_exempt(world, permit_pubkey);
    }

    for stake_pubkey in &world.stakes {
        let stake = world.read_seller_stake(stake_pubkey);
        let vault_pubkey = world.stake_vault_pda(&stake.seller);
        let vault = world.read_token_account(&vault_pubkey);

        // Not exact equality: a token account accepts a transfer from
        // anyone without its owner's consent, so an unsolicited deposit
        // can push `vault.amount` above `stake.staked` without any
        // handler having done anything wrong. Only a vault caught short
        // of the ledger is a real problem, and that direction is still
        // asserted (docs/DESIGN-v2.md, "Program conventions").
        assert!(
            vault.amount >= stake.staked,
            "stake_vault balance must be at least SellerStake.staked for {stake_pubkey}"
        );
        let expected_committed = committed_by_seller.get(&stake.seller).copied().unwrap_or(0);
        assert_eq!(
            stake.committed as u128, expected_committed,
            "committed must equal the sum of (max_slashable - slashed) across {stake_pubkey}'s active permits"
        );
        assert!(
            stake.committed <= stake.staked,
            "committed must never exceed staked for {stake_pubkey}"
        );
        assert!(
            stake.disputes_lost <= stake.disputes_total,
            "disputes_lost must never exceed disputes_total for {stake_pubkey}"
        );

        assert_rent_exempt(world, stake_pubkey);
        assert_rent_exempt(world, &vault_pubkey);

        total_tracked += vault.amount as u128;
    }

    for marketplace_pubkey in &world.marketplaces {
        let bond_vault_pubkey = world.bond_vault_pda(marketplace_pubkey);
        let bond_vault = world.read_token_account(&bond_vault_pubkey);

        // The shared bond pool against the records that own it. A
        // resolution that pays out an amount other than the one recorded,
        // or pays one out twice, overdraws some other buyer's bond and
        // shows up here.
        let expected_bonds = bonds_by_marketplace
            .get(marketplace_pubkey)
            .copied()
            .unwrap_or(0);
        assert_eq!(
            bond_vault.amount as u128, expected_bonds,
            "bond_vault must hold exactly the sum of Open disputes' recorded bonds for {marketplace_pubkey}"
        );

        assert_rent_exempt(world, marketplace_pubkey);
        assert_rent_exempt(world, &bond_vault_pubkey);

        total_tracked += bond_vault.amount as u128;
    }

    for token_account_pubkey in &world.token_accounts {
        total_tracked += world.read_token_account(token_account_pubkey).amount as u128;
    }

    assert_eq!(
        total_tracked, world.minted_total as u128,
        "no collateral-mint USDC may be created or destroyed outside minting"
    );
}
