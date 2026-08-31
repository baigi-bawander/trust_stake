pub mod config;
pub mod dispute_record;
pub mod marketplace;
pub mod seller_stake;
pub mod slash_permit;

pub use config::*;
pub use dispute_record::*;
pub use marketplace::*;
pub use seller_stake::*;
pub use slash_permit::*;

/// A failure in this module does not mean the change that triggered it is
/// wrong -- it means the change alters how accounts already written under
/// the old layout get read back. Whoever updates the expected bytes below
/// must first decide what happens to those already-stored accounts,
/// including whether `constants::ACCOUNT_VERSION` needs bumping so a
/// stale account can be told apart from a fresh one (every handler that
/// loads existing state now rejects a `version` mismatch).
///
/// Asserting the derived `INIT_SPACE` alone is not enough: swapping two
/// same-sized fields leaves the size unchanged while corrupting how every
/// already-stored account is read, which is exactly the failure this
/// module exists to catch. Each test below instead constructs a real
/// instance with distinct, non-zero, non-repeating values in every field,
/// serializes it, and asserts the resulting bytes against a hardcoded
/// array -- pinning size, field order and field type together. The
/// `INIT_SPACE` assertion is kept alongside each as a second, more
/// readable signal, not as a substitute.
#[cfg(test)]
mod account_layout_is_pinned {
    use super::*;
    use anchor_lang::prelude::*;

    #[test]
    fn config() {
        assert_eq!(Config::INIT_SPACE, 163);

        let value = Config {
            version: 0x11,
            bump: 0x22,
            authority: Pubkey::new_from_array([0x01; 32]),
            pending_authority: Pubkey::new_from_array([0x02; 32]),
            collateral_mint: Pubkey::new_from_array([0x03; 32]),
            chain_id: 0x44,
            reserved: [0xAA; 64],
        };
        let mut data = Vec::new();
        value.try_serialize(&mut data).expect("Config serializes");

        #[rustfmt::skip]
        let expected: Vec<u8> = vec![
            155, 12, 170, 224, 30, 250, 204, 130,
            0x11, 0x22,
            1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
            2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
            3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
            0x44,
            170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170,
            170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170,
            170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170,
            170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170, 170,
        ];
        assert_eq!(data, expected);
    }

    #[test]
    fn marketplace() {
        assert_eq!(Marketplace::INIT_SPACE, 280);

        let value = Marketplace {
            version: 0x11,
            bump: 0x22,
            marketplace_id: [0x33; 16],
            authority: Pubkey::new_from_array([0x01; 32]),
            pending_authority: Pubkey::new_from_array([0x02; 32]),
            receipt_signer: Pubkey::new_from_array([0x04; 32]),
            prev_receipt_signer: Pubkey::new_from_array([0x05; 32]),
            signer_rotated_at: 0x1122334455,
            arbiter: Pubkey::new_from_array([0x06; 32]),
            complaint_window: 0x2233445566,
            bond_bps: 0x5566,
            disputes_total: 0x778899AA,
            disputes_upheld: 0x11223344,
            disputes_abandoned: 0x22334455,
            total_slashed: 0x3344556677889900,
            reserved: [0xBB; 64],
        };
        let mut data = Vec::new();
        value.try_serialize(&mut data).expect("Marketplace serializes");

        #[rustfmt::skip]
        let expected: Vec<u8> = vec![
            70, 222, 41, 62, 78, 3, 32, 174,
            0x11, 0x22,
            51, 51, 51, 51, 51, 51, 51, 51, 51, 51, 51, 51, 51, 51, 51, 51,
            1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
            2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
            4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
            5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5,
            85, 68, 51, 34, 17, 0, 0, 0,
            6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
            102, 85, 68, 51, 34, 0, 0, 0,
            102, 85,
            170, 153, 136, 119,
            68, 51, 34, 17,
            85, 68, 51, 34,
            0, 153, 136, 119, 102, 85, 68, 51,
            187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187,
            187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187,
            187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187,
            187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187, 187,
        ];
        assert_eq!(data, expected);
    }

    #[test]
    fn seller_stake() {
        assert_eq!(SellerStake::INIT_SPACE, 122);

        let value = SellerStake {
            version: 0x11,
            bump: 0x22,
            seller: Pubkey::new_from_array([0x07; 32]),
            staked: 0x1122334455667788,
            committed: 0x8877665544332211,
            disputes_total: 0x33445566,
            disputes_lost: 0x44556677,
            reserved: [0xCC; 64],
        };
        let mut data = Vec::new();
        value.try_serialize(&mut data).expect("SellerStake serializes");

        #[rustfmt::skip]
        let expected: Vec<u8> = vec![
            187, 104, 73, 26, 255, 109, 69, 239,
            0x11, 0x22,
            7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            136, 119, 102, 85, 68, 51, 34, 17,
            17, 34, 51, 68, 85, 102, 119, 136,
            102, 85, 68, 51,
            119, 102, 85, 68,
            204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204,
            204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204,
            204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204,
            204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204, 204,
        ];
        assert_eq!(data, expected);
    }

    #[test]
    fn slash_permit() {
        assert_eq!(SlashPermit::INIT_SPACE, 134);

        let value = SlashPermit {
            version: 0x11,
            bump: 0x22,
            seller: Pubkey::new_from_array([0x08; 32]),
            marketplace: Pubkey::new_from_array([0x09; 32]),
            max_slashable: 0x1111111111111111,
            slashed: 0x2222222222222222,
            open_disputes: 0x3333,
            revoked_at: 0x4444444444444444,
            complaint_window: 0x5555555555555555,
            bond_bps: 0x6666,
            granted_at: 0x7777777777777777,
            reserved: [0xDD; 24],
        };
        let mut data = Vec::new();
        value.try_serialize(&mut data).expect("SlashPermit serializes");

        #[rustfmt::skip]
        let expected: Vec<u8> = vec![
            99, 130, 197, 60, 205, 19, 67, 122,
            0x11, 0x22,
            8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8,
            9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9,
            17, 17, 17, 17, 17, 17, 17, 17,
            34, 34, 34, 34, 34, 34, 34, 34,
            51, 51,
            68, 68, 68, 68, 68, 68, 68, 68,
            85, 85, 85, 85, 85, 85, 85, 85,
            102, 102,
            119, 119, 119, 119, 119, 119, 119, 119,
            221, 221, 221, 221, 221, 221, 221, 221, 221, 221, 221, 221, 221, 221, 221, 221,
            221, 221, 221, 221, 221, 221, 221, 221,
        ];
        assert_eq!(data, expected);
    }

    #[test]
    fn dispute_record() {
        assert_eq!(DisputeRecord::INIT_SPACE, 203);

        let value = DisputeRecord {
            version: 0x11,
            bump: 0x22,
            marketplace: Pubkey::new_from_array([0x0A; 32]),
            seller: Pubkey::new_from_array([0x0B; 32]),
            buyer: Pubkey::new_from_array([0x0C; 32]),
            order_id: [0x0D; 32],
            claim: 0x1234567890ABCDEF,
            bond: 0x0FEDCBA987654321,
            created_at: 0x1111111111111111,
            expires_at: 0x2222222222222222,
            closable_after: 0x3333333333333333,
            status: 0x44,
            reserved: [0xEE; 32],
        };
        let mut data = Vec::new();
        value.try_serialize(&mut data).expect("DisputeRecord serializes");

        #[rustfmt::skip]
        let expected: Vec<u8> = vec![
            198, 199, 79, 209, 12, 215, 34, 47,
            0x11, 0x22,
            10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10,
            11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11,
            12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
            13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
            239, 205, 171, 144, 120, 86, 52, 18,
            33, 67, 101, 135, 169, 203, 237, 15,
            17, 17, 17, 17, 17, 17, 17, 17,
            34, 34, 34, 34, 34, 34, 34, 34,
            51, 51, 51, 51, 51, 51, 51, 51,
            0x44,
            238, 238, 238, 238, 238, 238, 238, 238, 238, 238, 238, 238, 238, 238, 238, 238,
            238, 238, 238, 238, 238, 238, 238, 238, 238, 238, 238, 238, 238, 238, 238, 238,
        ];
        assert_eq!(data, expected);
    }
}
