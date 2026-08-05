use {
    anchor_lang::{
        prelude::Pubkey, solana_program::instruction::Instruction, AccountDeserialize,
        InstructionData, ToAccountMetas,
    },
    litesvm::LiteSVM,
    solana_keypair::Keypair,
    solana_message::{Message, VersionedMessage},
    solana_signer::Signer,
    solana_transaction::versioned::VersionedTransaction,
    truststake::state::SellerStake,
};

const SOL: u64 = 1_000_000_000;

fn setup() -> (LiteSVM, Pubkey) {
    let program_id = truststake::id();
    let mut svm = LiteSVM::new();
    svm.add_program(
        program_id,
        include_bytes!("../../../target/deploy/truststake.so"),
    )
    .unwrap();
    (svm, program_id)
}

fn send(svm: &mut LiteSVM, ix: Instruction, payer: &Keypair) {
    let blockhash = svm.latest_blockhash();
    let msg = Message::new_with_blockhash(&[ix], Some(&payer.pubkey()), &blockhash);
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[payer]).unwrap();
    svm.send_transaction(tx).unwrap();
}

fn read_stake(svm: &LiteSVM, stake: &Pubkey) -> SellerStake {
    let account = svm.get_account(stake).unwrap();
    SellerStake::try_deserialize(&mut account.data.as_slice()).unwrap()
}

/// A seller stakes, a buyer disputes, the arbiter upholds it, and the
/// collateral moves from the seller's stake to the buyer.
#[test]
fn upheld_dispute_slashes_stake() {
    let (mut svm, program_id) = setup();

    let arbiter = Keypair::new();
    let seller = Keypair::new();
    let buyer = Keypair::new();
    for kp in [&arbiter, &seller, &buyer] {
        svm.airdrop(&kp.pubkey(), 10 * SOL).unwrap();
    }

    let (config, _) = Pubkey::find_program_address(&[b"config"], &program_id);
    let (stake, _) = Pubkey::find_program_address(
        &[b"stake", seller.pubkey().as_ref()],
        &program_id,
    );
    let (dispute, _) = Pubkey::find_program_address(
        &[b"dispute", seller.pubkey().as_ref(), buyer.pubkey().as_ref()],
        &program_id,
    );

    send(
        &mut svm,
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
    );

    send(
        &mut svm,
        Instruction::new_with_bytes(
            program_id,
            &truststake::instruction::CreateStake { amount: 5 * SOL }.data(),
            truststake::accounts::CreateStake {
                seller: seller.pubkey(),
                stake,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
        ),
        &seller,
    );
    assert_eq!(read_stake(&svm, &stake).staked, 5 * SOL);

    send(
        &mut svm,
        Instruction::new_with_bytes(
            program_id,
            &truststake::instruction::RaiseDispute { claim: 2 * SOL }.data(),
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
    );

    let buyer_before = svm.get_balance(&buyer.pubkey()).unwrap();

    send(
        &mut svm,
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
    );

    let after = read_stake(&svm, &stake);
    assert_eq!(after.staked, 3 * SOL, "2 SOL should be slashed");
    assert_eq!(after.disputes_lost, 1, "incident is recorded permanently");

    // The buyer receives the slashed collateral. The arbiter paid the fee, and
    // closing the dispute account refunds its rent on top.
    let buyer_after = svm.get_balance(&buyer.pubkey()).unwrap();
    assert!(
        buyer_after >= buyer_before + 2 * SOL,
        "buyer should receive the slashed collateral"
    );

    assert!(svm.get_account(&dispute).is_none_or(|a| a.data.is_empty()));
}

/// A rejected dispute leaves the seller's collateral and record untouched.
#[test]
fn rejected_dispute_leaves_stake_intact() {
    let (mut svm, program_id) = setup();

    let arbiter = Keypair::new();
    let seller = Keypair::new();
    let buyer = Keypair::new();
    for kp in [&arbiter, &seller, &buyer] {
        svm.airdrop(&kp.pubkey(), 10 * SOL).unwrap();
    }

    let (config, _) = Pubkey::find_program_address(&[b"config"], &program_id);
    let (stake, _) = Pubkey::find_program_address(
        &[b"stake", seller.pubkey().as_ref()],
        &program_id,
    );
    let (dispute, _) = Pubkey::find_program_address(
        &[b"dispute", seller.pubkey().as_ref(), buyer.pubkey().as_ref()],
        &program_id,
    );

    send(
        &mut svm,
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
    );

    send(
        &mut svm,
        Instruction::new_with_bytes(
            program_id,
            &truststake::instruction::CreateStake { amount: 5 * SOL }.data(),
            truststake::accounts::CreateStake {
                seller: seller.pubkey(),
                stake,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
        ),
        &seller,
    );

    send(
        &mut svm,
        Instruction::new_with_bytes(
            program_id,
            &truststake::instruction::RaiseDispute { claim: 2 * SOL }.data(),
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
    );

    send(
        &mut svm,
        Instruction::new_with_bytes(
            program_id,
            &truststake::instruction::ResolveDispute { uphold: false }.data(),
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
    );

    let after = read_stake(&svm, &stake);
    assert_eq!(after.staked, 5 * SOL);
    assert_eq!(after.disputes_lost, 0);
}

/// Only the configured arbiter can resolve a dispute.
#[test]
fn non_arbiter_cannot_resolve() {
    let (mut svm, program_id) = setup();

    let arbiter = Keypair::new();
    let seller = Keypair::new();
    let buyer = Keypair::new();
    let impostor = Keypair::new();
    for kp in [&arbiter, &seller, &buyer, &impostor] {
        svm.airdrop(&kp.pubkey(), 10 * SOL).unwrap();
    }

    let (config, _) = Pubkey::find_program_address(&[b"config"], &program_id);
    let (stake, _) = Pubkey::find_program_address(
        &[b"stake", seller.pubkey().as_ref()],
        &program_id,
    );
    let (dispute, _) = Pubkey::find_program_address(
        &[b"dispute", seller.pubkey().as_ref(), buyer.pubkey().as_ref()],
        &program_id,
    );

    send(
        &mut svm,
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
    );

    send(
        &mut svm,
        Instruction::new_with_bytes(
            program_id,
            &truststake::instruction::CreateStake { amount: 5 * SOL }.data(),
            truststake::accounts::CreateStake {
                seller: seller.pubkey(),
                stake,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
        ),
        &seller,
    );

    send(
        &mut svm,
        Instruction::new_with_bytes(
            program_id,
            &truststake::instruction::RaiseDispute { claim: 2 * SOL }.data(),
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
    );

    let ix = Instruction::new_with_bytes(
        program_id,
        &truststake::instruction::ResolveDispute { uphold: true }.data(),
        truststake::accounts::ResolveDispute {
            arbiter: impostor.pubkey(),
            config,
            buyer: buyer.pubkey(),
            stake,
            dispute,
        }
        .to_account_metas(None),
    );
    let blockhash = svm.latest_blockhash();
    let msg = Message::new_with_blockhash(&[ix], Some(&impostor.pubkey()), &blockhash);
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[&impostor]).unwrap();

    assert!(svm.send_transaction(tx).is_err(), "impostor must be rejected");
    assert_eq!(read_stake(&svm, &stake).staked, 5 * SOL);
}
