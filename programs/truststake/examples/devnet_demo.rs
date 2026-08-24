//! Walks TWO marketplaces sharing ONE seller's collateral pool against
//! Solana devnet, printing a transaction signature and an explorer URL for
//! every step, so the multi-tenant guarantee (decision 1, docs/DESIGN-v2.md)
//! is visible onchain, not only in tests.
//!
//! docs/DESIGN-v2.md's Build order assigns this file's v2 rewrite to Phase 4
//! Step 2. tests/test_devnet_demo_parity.rs's
//! test_devnet_demo_sequence_parity replays this exact instruction
//! sequence, same handlers and same arguments, against LiteSVM before any
//! of it touches devnet. Read that test alongside this file, and change
//! both together: if the sequence below changes, that test is what catches
//! a mistake for free instead of this script teaching it at 0.5 SOL a
//! lesson.
//!
//! Run with:
//!   cargo run --example devnet_demo --features devnet_demo --manifest-path programs/truststake/Cargo.toml
//!
//! The wallet at ~/.config/solana/id.json (or TRUSTSTAKE_ADMIN_KEYPAIR, if
//! set) pays for setup and must equal constants::INITIAL_ADMIN, the only
//! signer initialize_config accepts. Every throwaway wallet below is funded
//! by a direct System Program transfer rather than an airdrop, since devnet
//! airdrops are rate-limited, and each one that ends a transaction with a
//! nonzero balance is funded with an extra rent-exempt minimum of headroom
//! on top of what it actually spends, or the transaction that leaves it
//! there is rejected in preflight.
//!
//! What this walk does not do: release a permit. release_permit needs
//! revoked_at + complaint_window (floored at 2 days) to elapse, and
//! release_permit_early still needs CLOCK_SKEW_TOLERANCE_SECONDS (one
//! hour) after revocation. Neither fits inside a single run of this
//! script. The walk ends once the two permits' final states diverge (step
//! 8 below); tests/test_phase2.rs exercises release instead. Because
//! nothing here ever releases a permit, the seller's collateral and both
//! marketplaces stay frozen once the process exits; acceptable for a
//! devnet demo funded with test-mint tokens, not a resumable production
//! flow. What IS resumable is a failed or partial run: every throwaway
//! wallet is persisted to `examples/.devnet-demo-keypairs/` (gitignored,
//! never printed) via `load_or_create_keypair`, so re-running after a
//! failure reuses the same wallets and picks up wherever the previous run
//! left off, the same way `initialize_config` and the test mint are
//! already reused from an existing `Config` account.

use {
    anchor_lang::{
        prelude::{Discriminator, Pubkey, Space},
        solana_program::{instruction::Instruction, program_pack::Pack, system_instruction},
        AccountDeserialize, InstructionData, ToAccountMetas,
    },
    anchor_spl::token::spl_token,
    solana_commitment_config::CommitmentConfig,
    solana_ed25519_program::new_ed25519_instruction_with_signature,
    solana_instruction::error::InstructionError,
    solana_keypair::{read_keypair_file, write_keypair_file, Keypair},
    solana_message::{Message, VersionedMessage},
    solana_rpc_client::{api::config::RpcSendTransactionConfig, rpc_client::RpcClient},
    solana_signature::Signature,
    solana_signer::Signer,
    solana_transaction::versioned::VersionedTransaction,
    solana_transaction_error::TransactionError,
    std::{
        env,
        error::Error,
        path::{Path, PathBuf},
        thread::sleep,
        time::{Duration, SystemTime, UNIX_EPOCH},
    },
    truststake::{
        constants::{
            BOND_VAULT_SEED, CHAIN_ID_DEVNET, CLOCK_SKEW_TOLERANCE_SECONDS, CONFIG_SEED, DISPUTE_SEED,
            INITIAL_ADMIN, MARKETPLACE_SEED, MIN_COMPLAINT_WINDOW_SECONDS, PERMIT_SEED, RECEIPT_DOMAIN,
            SECONDS_PER_DAY, SEED_VERSION, STAKE_SEED, VAULT_SEED,
        },
        error::TrustStakeError,
        receipt::OrderReceipt,
        state::{Config, DisputeRecord, Marketplace, SellerStake, SlashPermit},
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
const TOTAL_COMMITTED: u64 = CASHDESK_PERMIT + PIXELBAZAAR_PERMIT;
/// Succeeds: 500 - 100 = 400, exactly the committed floor (docs/DESIGN-v2.md,
/// "withdraw_stake requires staked - amount >= committed").
const WITHDRAW_AMOUNT: u64 = usdc(100);
/// Attempted after the above: 400 - 50 = 350 < 400 committed, so this MUST
/// fail. That is the point of step 6, not a mistake to fix.
const WITHDRAW_ATTEMPT_TOO_MUCH: u64 = usdc(50);
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

    let program_id = truststake::id();
    let config = Pubkey::find_program_address(&[CONFIG_SEED, SEED_VERSION], &program_id).0;
    let event_authority = Pubkey::find_program_address(&[b"__event_authority"], &program_id).0;

    let keypairs_dir = keypairs_dir();
    std::fs::create_dir_all(&keypairs_dir)
        .map_err(|error| format!("failed to create {keypairs_dir:?}: {error}"))?;
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

    let (seller_token_account, signature) = create_token_account(&client, &admin, &mint, &seller.pubkey())?;
    print_step("Create seller token account", &signature);
    let (buyer_token_account, signature) = create_token_account(&client, &admin, &mint, &buyer.pubkey())?;
    print_step("Create buyer token account", &signature);

    let signature = mint_to_account(&client, &admin, &mint, &seller_token_account, STAKE_AMOUNT)?;
    print_step(&format!("Mint {} to seller", format_usdc(STAKE_AMOUNT)), &signature);
    let signature = mint_to_account(&client, &admin, &mint, &buyer_token_account, BUYER_TOKEN_FUNDING)?;
    print_step(&format!("Mint {} to buyer", format_usdc(BUYER_TOKEN_FUNDING)), &signature);
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

    // ==================== Step 3: register two marketplaces ====================
    println!("=== Step 3: register two marketplaces against the same seller's future collateral ===");
    let cashdesk_id = random_marketplace_id();
    let cashdesk = register_marketplace(
        &client,
        program_id,
        config,
        mint,
        event_authority,
        &cashdesk_authority,
        cashdesk_id,
        cashdesk_receipt_signer.pubkey(),
        cashdesk_arbiter.pubkey(),
        CASHDESK_COMPLAINT_WINDOW_SECONDS,
        CASHDESK_BOND_BPS,
        "CashDesk",
    )?;

    let pixelbazaar_id = random_marketplace_id();
    let pixelbazaar = register_marketplace(
        &client,
        program_id,
        config,
        mint,
        event_authority,
        &pixelbazaar_authority,
        pixelbazaar_id,
        pixelbazaar_receipt_signer.pubkey(),
        pixelbazaar_arbiter.pubkey(),
        PIXELBAZAAR_COMPLAINT_WINDOW_SECONDS,
        PIXELBAZAAR_BOND_BPS,
        "PixelBazaar",
    )?;
    println!();

    // ==================== Step 4: seller stakes once ====================
    println!("=== Step 4: seller locks {} of collateral, once ===", format_usdc(STAKE_AMOUNT));
    let stake = Pubkey::find_program_address(&[STAKE_SEED, SEED_VERSION, seller.pubkey().as_ref()], &program_id).0;
    let stake_vault =
        Pubkey::find_program_address(&[VAULT_SEED, SEED_VERSION, seller.pubkey().as_ref()], &program_id).0;
    {
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
    }
    {
        let instruction = Instruction::new_with_bytes(
            program_id,
            &truststake::instruction::AddStake { amount: STAKE_AMOUNT }.data(),
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
        print_step(&format!("add_stake({})", format_usdc(STAKE_AMOUNT)), &signature);
    }
    println!();

    // ==================== Step 5: grant a permit to each marketplace ====================
    println!("=== Step 5: seller grants each marketplace its own permit, from the SAME stake ===");
    grant_permit(&client, program_id, event_authority, &seller, stake, cashdesk, CASHDESK_PERMIT, "CashDesk")?;
    grant_permit(
        &client,
        program_id,
        event_authority,
        &seller,
        stake,
        pixelbazaar,
        PIXELBAZAAR_PERMIT,
        "PixelBazaar",
    )?;

    let stake_state = read_seller_stake(&client, &stake)?;
    if stake_state.committed != TOTAL_COMMITTED || stake_state.staked != STAKE_AMOUNT {
        return Err(format!(
            "expected staked = {}, committed = {}; found staked = {}, committed = {}",
            format_usdc(STAKE_AMOUNT),
            format_usdc(TOTAL_COMMITTED),
            format_usdc(stake_state.staked),
            format_usdc(stake_state.committed)
        )
        .into());
    }
    println!(
        "committed = {} of {} staked ({} CashDesk + {} PixelBazaar)",
        format_usdc(stake_state.committed),
        format_usdc(stake_state.staked),
        format_usdc(CASHDESK_PERMIT),
        format_usdc(PIXELBAZAAR_PERMIT),
    );
    println!();

    // ==================== Step 6: withdraw the free balance, then hit the cap ====================
    println!("=== Step 6: withdraw the {} that is not committed to anyone ===", format_usdc(WITHDRAW_AMOUNT));
    {
        let instruction = Instruction::new_with_bytes(
            program_id,
            &truststake::instruction::WithdrawStake { amount: WITHDRAW_AMOUNT }.data(),
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
        print_step(
            &format!(
                "withdraw_stake({}) -- succeeds: {} - {} >= {} committed",
                format_usdc(WITHDRAW_AMOUNT),
                format_usdc(STAKE_AMOUNT),
                format_usdc(WITHDRAW_AMOUNT),
                format_usdc(TOTAL_COMMITTED)
            ),
            &signature,
        );
    }

    println!(
        "Attempting to withdraw another {}, which the cap must refuse...",
        format_usdc(WITHDRAW_ATTEMPT_TOO_MUCH)
    );
    {
        let instruction = Instruction::new_with_bytes(
            program_id,
            &truststake::instruction::WithdrawStake { amount: WITHDRAW_ATTEMPT_TOO_MUCH }.data(),
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
        // send_allowing_failure, not send: this transaction is SUPPOSED to
        // fail, and the point is proving the cap is enforced onchain by the
        // deployed program, for a real fee, rather than merely rejected by
        // client-side preflight simulation before anything was spent.
        let (signature, outcome) = send_allowing_failure(&client, &[instruction], &seller.pubkey(), &[&seller])?;
        print_step(&format!("withdraw_stake({}) -- must fail", format_usdc(WITHDRAW_ATTEMPT_TOO_MUCH)), &signature);
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
    println!();

    // ==================== Step 7: a buyer disputes an order on CashDesk ====================
    println!("=== Step 7: a CashDesk buyer disputes a {} order ===", format_usdc(ORDER_AMOUNT));
    let order_id = Keypair::new().pubkey().to_bytes();
    let issued_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time must be after the unix epoch")
        .as_secs() as i64;
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
    // Build the signed bytes with OrderReceipt::message(), never by hand:
    // the Ed25519 header below asserts message_data_size EXACTLY, and every
    // field of OrderReceipt is fixed-width for that reason (src/receipt.rs).
    let message = receipt.message();
    let signature_bytes: [u8; 64] = cashdesk_receipt_signer.sign_message(&message).into();
    let verify_instruction =
        new_ed25519_instruction_with_signature(&message, &signature_bytes, &cashdesk_receipt_signer.pubkey().to_bytes());

    let cashdesk_permit =
        Pubkey::find_program_address(&[PERMIT_SEED, SEED_VERSION, seller.pubkey().as_ref(), cashdesk.as_ref()], &program_id)
            .0;
    let dispute = Pubkey::find_program_address(
        &[DISPUTE_SEED, SEED_VERSION, cashdesk.as_ref(), seller.pubkey().as_ref(), order_id.as_ref()],
        &program_id,
    )
    .0;
    let cashdesk_bond_vault =
        Pubkey::find_program_address(&[BOND_VAULT_SEED, SEED_VERSION, cashdesk.as_ref()], &program_id).0;
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
    // position as current_index - 1 (docs/DESIGN-v2.md, "raise_dispute, the
    // one with real complexity", check 1). They cannot be split across two
    // transactions.
    let signature = send(&client, &[verify_instruction, raise_dispute_instruction], &buyer.pubkey(), &[&buyer])?;
    print_step("[Ed25519 verify, raise_dispute]", &signature);

    // CashDesk's arbiter resolves upheld. admin is the fee payer;
    // cashdesk_arbiter only co-signs, since resolve_dispute's `arbiter`
    // account is never `mut` -- it authorizes the ruling and pays nothing,
    // which is exactly the "trusted judge, not a funded participant" role
    // decision 2 (docs/DESIGN-v2.md) describes.
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
    println!();

    // ==================== Step 8: the payoff -- one slashed, one untouched ====================
    println!("=== Step 8: final state of both permits ===");
    let cashdesk_permit_state = read_permit(&client, &cashdesk_permit)?;
    let pixelbazaar_permit_pda =
        Pubkey::find_program_address(&[PERMIT_SEED, SEED_VERSION, seller.pubkey().as_ref(), pixelbazaar.as_ref()], &program_id)
            .0;
    let pixelbazaar_permit_state = read_permit(&client, &pixelbazaar_permit_pda)?;

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
    println!(
        "  Same seller, same stake, two independent caps: CashDesk's slashed rose to {}; \
         PixelBazaar's is still untouched at {} remaining.",
        format_usdc(cashdesk_permit_state.slashed),
        format_usdc(pixelbazaar_permit_state.max_slashable),
    );
    println!();
    println!(
        "The walk stops here: release_permit needs revoked_at + complaint_window to elapse \
         (complaint_window is floored at MIN_COMPLAINT_WINDOW_SECONDS = {MIN_COMPLAINT_WINDOW_SECONDS} \
         seconds, 2 days), and release_permit_early still needs CLOCK_SKEW_TOLERANCE_SECONDS \
         ({CLOCK_SKEW_TOLERANCE_SECONDS} seconds, one hour) after revocation. Neither fits a \
         single run of this script. Both are exercised in tests/test_phase2.rs instead."
    );

    Ok(())
}

/// Funds every wallet that pays rent or a transaction fee of its own.
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

    // Seller signs 6 transactions: initialize_stake (pays stake_rent AND
    // stake_vault's token_account_rent together), add_stake, two
    // grant_permit calls (each pays permit_rent), and two withdraw_stake
    // calls -- the second of which is expected to fail, but a transaction
    // that fails onchain execution still pays its base fee.
    let seller_funding = stake_rent + token_account_rent + 2 * permit_rent + 6 * SIGNATURE_FEE_HEADROOM + wallet_rent_floor;
    let signature = transfer_sol(client, admin, &seller.pubkey(), seller_funding)?;
    print_step(&format!("Fund seller ({} lamports)", seller_funding), &signature);

    // Buyer signs 1 transaction: raise_dispute (two instructions, one
    // signature), which pays dispute_rent.
    let buyer_funding = dispute_rent + SIGNATURE_FEE_HEADROOM + wallet_rent_floor;
    let signature = transfer_sol(client, admin, &buyer.pubkey(), buyer_funding)?;
    print_step(&format!("Fund buyer ({} lamports)", buyer_funding), &signature);

    // Each marketplace authority signs 1 transaction: register_marketplace,
    // which pays rent for both the marketplace account and its bond_vault.
    let marketplace_authority_funding = marketplace_rent + token_account_rent + SIGNATURE_FEE_HEADROOM + wallet_rent_floor;
    let signature = transfer_sol(client, admin, &cashdesk_authority.pubkey(), marketplace_authority_funding)?;
    print_step(&format!("Fund CashDesk authority ({} lamports)", marketplace_authority_funding), &signature);
    let signature = transfer_sol(client, admin, &pixelbazaar_authority.pubkey(), marketplace_authority_funding)?;
    print_step(&format!("Fund PixelBazaar authority ({} lamports)", marketplace_authority_funding), &signature);
    println!();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn register_marketplace(
    client: &RpcClient,
    program_id: Pubkey,
    config: Pubkey,
    mint: Pubkey,
    event_authority: Pubkey,
    authority: &Keypair,
    marketplace_id: [u8; 16],
    receipt_signer: Pubkey,
    arbiter: Pubkey,
    complaint_window: i64,
    bond_bps: u16,
    name: &str,
) -> Result<Pubkey, Box<dyn Error>> {
    let marketplace = Pubkey::find_program_address(&[MARKETPLACE_SEED, SEED_VERSION, marketplace_id.as_ref()], &program_id).0;
    let bond_vault = Pubkey::find_program_address(&[BOND_VAULT_SEED, SEED_VERSION, marketplace.as_ref()], &program_id).0;

    let instruction = Instruction::new_with_bytes(
        program_id,
        &truststake::instruction::RegisterMarketplace { marketplace_id, receipt_signer, arbiter, complaint_window, bond_bps }
            .data(),
        truststake::accounts::RegisterMarketplaceAccountConstraints {
            authority: authority.pubkey(),
            config,
            mint,
            marketplace,
            bond_vault,
            token_program: spl_token::ID,
            system_program: anchor_lang::system_program::ID,
            event_authority,
            program: program_id,
        }
        .to_account_metas(None),
    );

    let signature = send(client, &[instruction], &authority.pubkey(), &[authority])?;
    print_step(
        &format!("register_marketplace({name}, window = {} days, bond = {} bps)", complaint_window / SECONDS_PER_DAY, bond_bps),
        &signature,
    );
    Ok(marketplace)
}

#[allow(clippy::too_many_arguments)]
fn grant_permit(
    client: &RpcClient,
    program_id: Pubkey,
    event_authority: Pubkey,
    seller: &Keypair,
    stake: Pubkey,
    marketplace: Pubkey,
    max_slashable: u64,
    name: &str,
) -> Result<(), Box<dyn Error>> {
    let permit = Pubkey::find_program_address(&[PERMIT_SEED, SEED_VERSION, seller.pubkey().as_ref(), marketplace.as_ref()], &program_id).0;

    let instruction = Instruction::new_with_bytes(
        program_id,
        &truststake::instruction::GrantPermit { max_slashable }.data(),
        truststake::accounts::GrantPermitAccountConstraints {
            seller: seller.pubkey(),
            stake,
            marketplace,
            permit,
            system_program: anchor_lang::system_program::ID,
            event_authority,
            program: program_id,
        }
        .to_account_metas(None),
    );

    let signature = send(client, &[instruction], &seller.pubkey(), &[seller])?;
    print_step(&format!("grant_permit({name}, {})", format_usdc(max_slashable)), &signature);
    Ok(())
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
/// the seller's and buyer's token accounts on their behalf here, the way a
/// devnet faucet would.
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
/// one expected to fail (step 6's second withdrawal) goes through
/// `send_allowing_failure` instead.
fn send(client: &RpcClient, instructions: &[Instruction], payer: &Pubkey, signers: &[&Keypair]) -> Result<Signature, Box<dyn Error>> {
    let blockhash = client.get_latest_blockhash()?;
    let message = Message::new_with_blockhash(instructions, Some(payer), &blockhash);
    let transaction = VersionedTransaction::try_new(VersionedMessage::Legacy(message), signers)?;
    Ok(client.send_and_confirm_transaction(&transaction)?)
}

/// Submits without the usual preflight simulation and waits for the
/// transaction's own onchain outcome, succeed or fail. `send` simulates
/// first and never lets a failing transaction reach the cluster at all --
/// exactly wrong for step 6's second withdrawal, whose entire point is
/// proving `withdraw_stake`'s cap is enforced by the deployed program on
/// real devnet, at the cost of a real fee, not merely caught by
/// client-side simulation before anything was spent.
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
