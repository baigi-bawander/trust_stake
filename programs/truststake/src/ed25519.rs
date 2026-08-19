//! Reading a genuine Ed25519 verification out of the transaction the
//! handler is running in (docs/DESIGN-v2.md, "`raise_dispute`, the one
//! with real complexity", checks 0 to 4).
//!
//! The Ed25519 program is a precompile: it verifies signatures but writes
//! nothing and returns nothing, so a program cannot ask it whether it ran.
//! The only way to know is to read the transaction's own instruction list
//! back out of the Instructions sysvar and check, byte by byte, that the
//! instruction sitting immediately before this one is a verification of
//! exactly the message this handler is about to act on, by exactly one
//! key.
//!
//! This is the most exploitable surface in the program, so it is its own
//! module with its own error variants rather than inline in the handler.
//! The header layout and the checks against it follow
//! [GuidoDipietro/solana-ed25519-secp256k1-sig-verification](https://github.com/GuidoDipietro/solana-ed25519-secp256k1-sig-verification),
//! with one difference forced by design decision 5: that reference already
//! knows the pubkey and message and confirms the instruction carries them,
//! while this one reads both *out of* the instruction, since sending the
//! receipt twice would waste roughly 190 bytes of a 1,232-byte
//! transaction. Reading rather than confirming is why every field of the
//! header has to be checked here, the three instruction indices included:
//! whatever those fields say, the handler believes.

use anchor_lang::prelude::*;
use solana_ed25519_program::{
    DATA_START, PUBKEY_SERIALIZED_SIZE, SIGNATURE_OFFSETS_START, SIGNATURE_SERIALIZED_SIZE,
};
use solana_instructions_sysvar::{load_current_index_checked, load_instruction_at_checked};

use crate::{error::TrustStakeError, receipt::OrderReceipt};

/// Byte offsets inside a canonical single-signature Ed25519 instruction,
/// in the order
/// `solana_ed25519_program::new_ed25519_instruction_with_signature` writes
/// them: a signature count and a padding byte, one 14-byte offsets
/// struct, then the public key, the signature, and the message.
const PUBLIC_KEY_OFFSET: u16 = DATA_START as u16;
const SIGNATURE_OFFSET: u16 = PUBLIC_KEY_OFFSET + PUBKEY_SERIALIZED_SIZE as u16;
const MESSAGE_OFFSET: u16 = SIGNATURE_OFFSET + SIGNATURE_SERIALIZED_SIZE as u16;

/// Field offsets inside the 14-byte `Ed25519SignatureOffsets` struct,
/// relative to the start of the instruction data.
const SIGNATURE_OFFSET_FIELD: usize = SIGNATURE_OFFSETS_START;
const SIGNATURE_INSTRUCTION_INDEX_FIELD: usize = SIGNATURE_OFFSET_FIELD + 2;
const PUBLIC_KEY_OFFSET_FIELD: usize = SIGNATURE_INSTRUCTION_INDEX_FIELD + 2;
const PUBLIC_KEY_INSTRUCTION_INDEX_FIELD: usize = PUBLIC_KEY_OFFSET_FIELD + 2;
const MESSAGE_OFFSET_FIELD: usize = PUBLIC_KEY_INSTRUCTION_INDEX_FIELD + 2;
const MESSAGE_SIZE_FIELD: usize = MESSAGE_OFFSET_FIELD + 2;
const MESSAGE_INSTRUCTION_INDEX_FIELD: usize = MESSAGE_SIZE_FIELD + 2;

/// A receipt the transaction has genuinely proved a signature over, and
/// the key that signed it. Which key is *acceptable* is not decided here:
/// that is the caller's business, since it depends on the marketplace's
/// current and previous receipt signers and on the receipt's own
/// `issued_at`.
pub struct SignedReceipt {
    pub signer: Pubkey,
    pub receipt: OrderReceipt,
}

/// Confirms the instruction immediately before the current one is a
/// canonical Ed25519 verification of a single message that deserialises
/// as an [`OrderReceipt`], and returns that receipt together with the key
/// that signed it.
///
/// The checks, in the order docs/DESIGN-v2.md numbers them:
///
/// 0. `instructions_sysvar` really is the Instructions sysvar. Everything
///    below reads out of this account, so without this check an attacker
///    supplies a fabricated account describing a transaction that never
///    happened and every remaining check passes against data they wrote
///    themselves.
/// 1. The current index comes from `load_current_index_checked` and the
///    Ed25519 instruction is found relative to it; its position is never
///    assumed to be zero. The instruction at the current index must
///    belong to this program, so the neighbour being inspected is
///    genuinely the one next to *this* call rather than next to some
///    wrapper program's call that reached here through CPI.
/// 2. All three instruction indices in the header point at the Ed25519
///    instruction itself, not merely at plausible byte offsets. Without
///    this, an attacker has the precompile verify their own throwaway
///    signature while these indices point at a different instruction they
///    also control, so the pubkey and message read below are ones nobody
///    signed. Every offset is bounds-checked before slicing and the
///    message length is asserted exactly.
/// 4. The receipt is deserialised from the verified message bytes, never
///    passed in a second time as an argument.
///
/// Check 3 (is this signer acceptable?) and checks 5 onward (do the
/// receipt's fields match the accounts?) belong to `raise_dispute`.
pub fn verify_signed_receipt(instructions_sysvar: &AccountInfo) -> Result<SignedReceipt> {
    require_keys_eq!(
        *instructions_sysvar.key,
        solana_instructions_sysvar::ID,
        TrustStakeError::InvalidInstructionsSysvar
    );

    let current_index = load_current_index_checked(instructions_sysvar)?;
    let current_instruction = load_instruction_at_checked(current_index as usize, instructions_sysvar)?;
    require_keys_eq!(
        current_instruction.program_id,
        crate::ID,
        TrustStakeError::MustBeTopLevelInstruction
    );

    let ed25519_index = current_index
        .checked_sub(1)
        .ok_or(TrustStakeError::MissingEd25519Instruction)?;
    let verify_instruction = load_instruction_at_checked(ed25519_index as usize, instructions_sysvar)?;
    require_keys_eq!(
        verify_instruction.program_id,
        solana_sdk_ids::ed25519_program::ID,
        TrustStakeError::MissingEd25519Instruction
    );
    require!(
        verify_instruction.accounts.is_empty(),
        TrustStakeError::MalformedEd25519Instruction
    );

    let data = verify_instruction.data.as_slice();
    let message_length =
        u16::try_from(OrderReceipt::INIT_SPACE).map_err(|_| TrustStakeError::MalformedEd25519Instruction)?;
    let expected_length = usize::from(MESSAGE_OFFSET)
        .checked_add(OrderReceipt::INIT_SPACE)
        .ok_or(TrustStakeError::MalformedEd25519Instruction)?;
    require_eq!(
        data.len(),
        expected_length,
        TrustStakeError::MalformedEd25519Instruction
    );

    // One signature, and the padding byte the offsets struct is aligned
    // with. The precompile itself ignores byte 1; requiring it to be zero
    // keeps the instruction to the single shape reasoned about here.
    require_eq!(
        *data.first().ok_or(TrustStakeError::MalformedEd25519Instruction)?,
        1u8,
        TrustStakeError::MalformedEd25519Instruction
    );
    require_eq!(
        *data.get(1).ok_or(TrustStakeError::MalformedEd25519Instruction)?,
        0u8,
        TrustStakeError::MalformedEd25519Instruction
    );

    require_eq!(
        read_u16(data, PUBLIC_KEY_OFFSET_FIELD)?,
        PUBLIC_KEY_OFFSET,
        TrustStakeError::MalformedEd25519Instruction
    );
    require_eq!(
        read_u16(data, SIGNATURE_OFFSET_FIELD)?,
        SIGNATURE_OFFSET,
        TrustStakeError::MalformedEd25519Instruction
    );
    require_eq!(
        read_u16(data, MESSAGE_OFFSET_FIELD)?,
        MESSAGE_OFFSET,
        TrustStakeError::MalformedEd25519Instruction
    );
    require_eq!(
        read_u16(data, MESSAGE_SIZE_FIELD)?,
        message_length,
        TrustStakeError::MalformedEd25519Instruction
    );

    // Check 2, the one an offsets-only check misses entirely.
    for index_field in [
        SIGNATURE_INSTRUCTION_INDEX_FIELD,
        PUBLIC_KEY_INSTRUCTION_INDEX_FIELD,
        MESSAGE_INSTRUCTION_INDEX_FIELD,
    ] {
        require!(
            refers_to_self(read_u16(data, index_field)?, ed25519_index),
            TrustStakeError::MalformedEd25519Instruction
        );
    }

    let signer_bytes: [u8; PUBKEY_SERIALIZED_SIZE] = slice_at(data, PUBLIC_KEY_OFFSET, PUBKEY_SERIALIZED_SIZE)?
        .try_into()
        .map_err(|_| TrustStakeError::MalformedEd25519Instruction)?;
    let message = slice_at(data, MESSAGE_OFFSET, OrderReceipt::INIT_SPACE)?;

    Ok(SignedReceipt {
        signer: Pubkey::from(signer_bytes),
        receipt: OrderReceipt::try_from_slice(message)
            .map_err(|_| TrustStakeError::MalformedEd25519Instruction)?,
    })
}

/// Whether one of the header's instruction indices refers to the Ed25519
/// instruction itself. The precompile reads `u16::MAX` as "this
/// instruction's own data" and any other value as an absolute index into
/// the transaction's top-level instructions, so both forms below name the
/// same bytes; anything else names an instruction the attacker chose.
fn refers_to_self(index: u16, ed25519_index: u16) -> bool {
    index == u16::MAX || index == ed25519_index
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16> {
    let bytes: [u8; 2] = data
        .get(offset..offset.saturating_add(2))
        .ok_or(TrustStakeError::MalformedEd25519Instruction)?
        .try_into()
        .map_err(|_| TrustStakeError::MalformedEd25519Instruction)?;
    Ok(u16::from_le_bytes(bytes))
}

/// Bounds-checked slice. The header checks above already pin every offset
/// to a constant and the data to an exact length, so nothing reaches here
/// out of range; going through `get` anyway means a later edit to those
/// checks produces an error rather than a panicking slice.
fn slice_at(data: &[u8], offset: u16, length: usize) -> Result<&[u8]> {
    let start = usize::from(offset);
    let end = start
        .checked_add(length)
        .ok_or(TrustStakeError::MalformedEd25519Instruction)?;
    data.get(start..end)
        .ok_or_else(|| TrustStakeError::MalformedEd25519Instruction.into())
}
