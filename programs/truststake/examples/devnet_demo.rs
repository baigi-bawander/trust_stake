//! Replays the initialize_config -> create_stake -> raise_dispute ->
//! resolve_dispute sequence from tests/test_truststake.rs as real
//! transactions against Solana devnet, so each step produces a
//! publicly-verifiable transaction signature.
//!
//! Run with:
//!   cargo run --example devnet_demo --manifest-path programs/truststake/Cargo.toml
//!
//! The wallet at ~/.config/solana/id.json pays for everything and acts as
//! the arbiter. It funds two throwaway keypairs, seller and buyer, with a
//! direct System Program transfer rather than an airdrop, since devnet
//! airdrops are rate-limited.

use {
    anchor_lang::{
        prelude::{Pubkey, Space},
        solana_program::{instruction::Instruction, system_instruction},
        AccountDeserialize, InstructionData, ToAccountMetas,
    },
    solana_commitment_config::CommitmentConfig,
    solana_keypair::{read_keypair_file, Keypair},
    solana_message::{Message, VersionedMessage},
    solana_rpc_client::rpc_client::RpcClient,
    solana_signature::Signature,
    solana_signer::Signer,
    solana_transaction::versioned::VersionedTransaction,
    std::error::Error,
    truststake::state::{Dispute, SellerStake},
};

const DEVNET_URL: &str = "https://api.devnet.solana.com";
const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

/// Collateral the seller locks in `create_stake`. Small on purpose: the
/// whole point of this script is minimizing devnet SOL usage, unlike the
/// illustrative 5 SOL used in tests/test_truststake.rs.
const STAKE_AMOUNT: u64 = LAMPORTS_PER_SOL / 100; // 0.01 SOL

/// What the buyer claims in `raise_dispute`. Must be <= STAKE_AMOUNT.
const DISPUTE_CLAIM: u64 = LAMPORTS_PER_SOL / 200; // 0.005 SOL

/// A few multiples of Solana's 5,000-lamport base fee per signature
/// (https://docs.anza.xyz/consensus/fees#current-fee-structure), as
/// headroom on top of rent for each throwaway wallet's one transaction.
const SIGNATURE_FEE_HEADROOM: u64 = 20_000;

fn main() -> Result<(), Box<dyn Error>> {
    let keypair_path = format!(
        "{}/.config/solana/id.json",
        std::env::var("HOME").expect("HOME must be set to locate the Solana CLI keypair")
    );
    let arbiter = read_keypair_file(&keypair_path)
        .map_err(|error| format!("failed to read keypair at {keypair_path}: {error}"))?;
    let seller = Keypair::new();
    let buyer = Keypair::new();

    println!("Main wallet (fee payer + arbiter): {}", arbiter.pubkey());
    println!("Seller (throwaway):                {}", seller.pubkey());
    println!("Buyer (throwaway):                 {}", buyer.pubkey());
    println!();

    let client = RpcClient::new_with_commitment(DEVNET_URL, CommitmentConfig::confirmed());

    let program_id = truststake::id();
    let (config, _) = Pubkey::find_program_address(&[b"config"], &program_id);
    let (stake, _) =
        Pubkey::find_program_address(&[b"stake", seller.pubkey().as_ref()], &program_id);
    let (dispute, _) = Pubkey::find_program_address(
        &[
            b"dispute",
            seller.pubkey().as_ref(),
            buyer.pubkey().as_ref(),
        ],
        &program_id,
    );

    // Fund each throwaway wallet with exactly what it needs: rent for the
    // one account it initializes, its share of the staked/claimed amount,
    // and one transaction fee. This avoids devnet airdrops entirely.
    // A wallet that ends a transaction with a nonzero balance must stay
    // above the rent-exempt minimum for a bare (0-byte) System account, or
    // the transaction is rejected in preflight. Since seller/buyer spend
    // most of what they're given and aren't fully drained to zero, each
    // needs this floor on top of what it actually spends.
    let wallet_rent_floor = client.get_minimum_balance_for_rent_exemption(0)?;
    let stake_rent = client.get_minimum_balance_for_rent_exemption(8 + SellerStake::INIT_SPACE)?;
    let dispute_rent = client.get_minimum_balance_for_rent_exemption(8 + Dispute::INIT_SPACE)?;
    let seller_funding = stake_rent + STAKE_AMOUNT + SIGNATURE_FEE_HEADROOM + wallet_rent_floor;
    let buyer_funding = dispute_rent + SIGNATURE_FEE_HEADROOM + wallet_rent_floor;

    println!(
        "Funding seller: {seller_funding} lamports ({:.9} SOL = {stake_rent} stake-account rent + {STAKE_AMOUNT} stake + {SIGNATURE_FEE_HEADROOM} fee headroom + {wallet_rent_floor} wallet rent floor)",
        seller_funding as f64 / LAMPORTS_PER_SOL as f64
    );
    let signature = transfer_sol(&client, &arbiter, &seller.pubkey(), seller_funding)?;
    print_step("Fund seller", &signature);

    println!(
        "Funding buyer: {buyer_funding} lamports ({:.9} SOL = {dispute_rent} dispute-account rent + {SIGNATURE_FEE_HEADROOM} fee headroom + {wallet_rent_floor} wallet rent floor)",
        buyer_funding as f64 / LAMPORTS_PER_SOL as f64
    );
    let signature = transfer_sol(&client, &arbiter, &buyer.pubkey(), buyer_funding)?;
    print_step("Fund buyer", &signature);
    println!();

    // 1. initialize_config: the arbiter sets itself as the only key allowed
    // to resolve disputes. Idempotent: this only needs to happen once per
    // deployment, so skip it if a previous run already did it.
    if client.get_account(&config).is_ok() {
        println!("1/4 initialize_config: already done in a previous run, skipping");
    } else {
        let signature = send(
            &client,
            Instruction::new_with_bytes(
                program_id,
                &truststake::instruction::InitializeConfig {}.data(),
                truststake::accounts::InitializeConfig {
                    arbiter: arbiter.pubkey(),
                    config,
                    system_program: anchor_lang::system_program::ID,
                }
                .to_account_metas(None),
            ),
            &arbiter,
        )?;
        print_step("1/4 initialize_config", &signature);
    }

    // 2. create_stake: the seller locks STAKE_AMOUNT of collateral.
    let signature = send(
        &client,
        Instruction::new_with_bytes(
            program_id,
            &truststake::instruction::CreateStake {
                amount: STAKE_AMOUNT,
            }
            .data(),
            truststake::accounts::CreateStake {
                seller: seller.pubkey(),
                stake,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
        ),
        &seller,
    )?;
    print_step("2/4 create_stake", &signature);

    // 3. raise_dispute: the buyer files a claim against the seller's stake.
    let signature = send(
        &client,
        Instruction::new_with_bytes(
            program_id,
            &truststake::instruction::RaiseDispute {
                claim: DISPUTE_CLAIM,
            }
            .data(),
            truststake::accounts::RaiseDispute {
                buyer: buyer.pubkey(),
                seller: seller.pubkey(),
                stake,
                dispute,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
        ),
        &buyer,
    )?;
    print_step("3/4 raise_dispute", &signature);

    // 4. resolve_dispute(uphold: true): the arbiter rules for the buyer,
    // slashing the claimed amount from the seller's stake.
    let signature = send(
        &client,
        Instruction::new_with_bytes(
            program_id,
            &truststake::instruction::ResolveDispute { uphold: true }.data(),
            truststake::accounts::ResolveDispute {
                arbiter: arbiter.pubkey(),
                config,
                buyer: buyer.pubkey(),
                stake,
                dispute,
            }
            .to_account_metas(None),
        ),
        &arbiter,
    )?;
    print_step("4/4 resolve_dispute", &signature);
    println!();

    let stake_account = client.get_account(&stake)?;
    let seller_stake = SellerStake::try_deserialize(&mut stake_account.data.as_slice())
        .map_err(|error| format!("failed to deserialize the seller's stake account: {error}"))?;
    let buyer_balance = client.get_balance(&buyer.pubkey())?;

    println!("Final state:");
    println!(
        "  Seller stake {stake}: staked = {} lamports ({:.9} SOL), disputes_lost = {}",
        seller_stake.staked,
        seller_stake.staked as f64 / LAMPORTS_PER_SOL as f64,
        seller_stake.disputes_lost
    );
    println!(
        "  Buyer {}: balance = {buyer_balance} lamports ({:.9} SOL)",
        buyer.pubkey(),
        buyer_balance as f64 / LAMPORTS_PER_SOL as f64
    );

    Ok(())
}

/// Sends `instruction` in its own transaction, paid for and signed by
/// `payer`, and blocks until it reaches the client's configured commitment
/// level (confirmed).
fn send(
    client: &RpcClient,
    instruction: Instruction,
    payer: &Keypair,
) -> Result<Signature, Box<dyn Error>> {
    let blockhash = client.get_latest_blockhash()?;
    let message = Message::new_with_blockhash(&[instruction], Some(&payer.pubkey()), &blockhash);
    let transaction = VersionedTransaction::try_new(VersionedMessage::Legacy(message), &[payer])?;
    Ok(client.send_and_confirm_transaction(&transaction)?)
}

fn transfer_sol(
    client: &RpcClient,
    from: &Keypair,
    to: &Pubkey,
    lamports: u64,
) -> Result<Signature, Box<dyn Error>> {
    send(
        client,
        system_instruction::transfer(&from.pubkey(), to, lamports),
        from,
    )
}

fn print_step(label: &str, signature: &Signature) {
    println!("{label}: {signature}");
    println!("  https://explorer.solana.com/tx/{signature}?cluster=devnet");
}
