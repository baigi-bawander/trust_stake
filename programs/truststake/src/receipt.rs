use anchor_lang::prelude::*;

/// A marketplace's proof that an order happened, signed by its
/// `receipt_signer` in its own backend and handed to the buyer
/// (docs/DESIGN-v2.md, "The offchain receipt"). It is never stored
/// onchain: `raise_dispute` reads it out of the message bytes of the
/// Ed25519 instruction that verified it, which is why this type lives
/// outside `state/`.
///
/// An offchain signature that authorises spending has to commit to
/// everything it authorises, so every field here is checked against the
/// accounts actually passed into `raise_dispute`, not merely against the
/// signature: `domain` and `program_id` bind it to this program,
/// `chain_id` stops a devnet signature being replayed on mainnet, and the
/// rest name exactly which seller, which buyer, which order and how much.
///
/// `#[derive(InitSpace)]` is what fixes the serialised length, which the
/// Ed25519 header's `message_data_size` is asserted against exactly. Every
/// field is fixed-width, so `INIT_SPACE` is the whole message length and
/// no receipt can be padded or truncated (pinned by
/// [`serialised_length_matches_init_space`]).
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Debug, PartialEq, Eq)]
pub struct OrderReceipt {
    /// `constants::RECEIPT_DOMAIN`. First field, so it is also the signed
    /// message's prefix.
    pub domain: [u8; 21],
    pub program_id: Pubkey,
    pub chain_id: u8,
    pub marketplace_id: [u8; 16],
    pub seller: Pubkey,
    pub buyer: Pubkey,
    pub order_id: [u8; 32],
    /// The order's value in minor units, and the ceiling on what a buyer
    /// may claim against it.
    pub amount: u64,
    pub issued_at: i64,
    pub expires_at: i64,
}

impl OrderReceipt {
    /// The exact bytes a marketplace's backend signs, and the exact bytes
    /// `raise_dispute` reads back out of the Ed25519 instruction's
    /// message. Anything that signs a receipt goes through here, so the
    /// two sides cannot drift apart.
    pub fn message(&self) -> Vec<u8> {
        let mut message = Vec::with_capacity(Self::INIT_SPACE);
        self.serialize(&mut message)
            .expect("a fixed-width receipt serialises into a Vec");
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::RECEIPT_DOMAIN;

    /// The Ed25519 header check rejects any message whose length is not
    /// `OrderReceipt::INIT_SPACE`, so the two must agree byte for byte:
    /// a field whose Borsh encoding is not fixed-width would make
    /// `INIT_SPACE` an upper bound rather than the exact length, and the
    /// check would start rejecting honest receipts.
    #[test]
    fn serialised_length_matches_init_space() {
        let receipt = OrderReceipt {
            domain: RECEIPT_DOMAIN,
            program_id: crate::ID,
            chain_id: 1,
            marketplace_id: [7; 16],
            seller: Pubkey::new_unique(),
            buyer: Pubkey::new_unique(),
            order_id: [9; 32],
            amount: 80_000_000,
            issued_at: 1_760_000_000,
            expires_at: 1_760_600_000,
        };

        let serialised = receipt.message();
        assert_eq!(serialised.len(), OrderReceipt::INIT_SPACE);
        assert_eq!(
            &serialised[..RECEIPT_DOMAIN.len()],
            RECEIPT_DOMAIN.as_slice(),
            "the domain must be the signed message's prefix"
        );
    }
}
