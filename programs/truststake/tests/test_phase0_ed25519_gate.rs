//! Phase 0 gate (docs/DESIGN-v2.md, "Build order"): proves that LiteSVM
//! 0.10.0's "precompiles" feature actually runs the native Ed25519 program
//! in-process. `raise_dispute` (Phase 3) has nowhere to run its signature
//! check if this doesn't pass. Throwaway: delete this file once Phase 3
//! lands its own signature-check tests covering the same ground.

use {
    litesvm::LiteSVM,
    solana_ed25519_program::new_ed25519_instruction_with_signature,
    solana_keypair::Keypair,
    solana_message::{Message, VersionedMessage},
    solana_signer::Signer,
    solana_transaction::versioned::VersionedTransaction,
};

#[test]
fn ed25519_verify_instruction_executes_in_litesvm() {
    let mut svm = LiteSVM::new();

    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000).unwrap();

    let receipt_signer = Keypair::new();
    let message = b"truststake phase 0 gate: ed25519 precompile is reachable";
    let signature: [u8; 64] = receipt_signer.sign_message(message).into();
    let pubkey_bytes = receipt_signer.pubkey().to_bytes();

    let verify_instruction =
        new_ed25519_instruction_with_signature(message, &signature, &pubkey_bytes);

    let blockhash = svm.latest_blockhash();
    let versioned_message = Message::new_with_blockhash(
        &[verify_instruction],
        Some(&payer.pubkey()),
        &blockhash,
    );
    let transaction =
        VersionedTransaction::try_new(VersionedMessage::Legacy(versioned_message), &[payer])
            .unwrap();

    let result = svm.send_transaction(transaction);
    assert!(
        result.is_ok(),
        "Ed25519 precompile verify failed inside LiteSVM: {:?}",
        result.err()
    );
}
