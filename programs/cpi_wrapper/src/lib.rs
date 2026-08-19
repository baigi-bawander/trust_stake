//! A test fixture, not part of the protocol. It exists so two attacks on
//! `raise_dispute`'s signature check can actually be executed rather than
//! argued about (docs/TESTING.md, "Attacks on the signature check"):
//!
//! - `forward` invokes another program through CPI, which is how
//!   `test_introspection_rejects_cpi_wrapper` reaches `raise_dispute`
//!   without being a top-level instruction.
//! - `noop` accepts any bytes and does nothing, which is how
//!   `test_introspection_rejects_wrong_program` puts an instruction
//!   carrying a real Ed25519 verify instruction's payload, under a
//!   program ID that is not the Ed25519 precompile, in front of
//!   `raise_dispute`.
//!
//! Nothing in `programs/truststake` depends on this at runtime, and it is
//! never deployed to a cluster: `anchor deploy` takes `-p truststake`.

use anchor_lang::{
    prelude::*,
    solana_program::{instruction::Instruction, program::invoke},
};

declare_id!("67mpWaL4h4otDVxcYbDnpCESgqJ7d8dCrT6uvydTh194");

#[program]
pub mod cpi_wrapper {
    use super::*;

    /// Invokes `target_program` with `data` and every remaining account,
    /// passing each account's signer and writable flags straight through.
    pub fn forward<'info>(
        ctx: Context<'info, ForwardAccountConstraints<'info>>,
        data: Vec<u8>,
    ) -> Result<()> {
        let accounts = ctx
            .remaining_accounts
            .iter()
            .map(|account| AccountMeta {
                pubkey: *account.key,
                is_signer: account.is_signer,
                is_writable: account.is_writable,
            })
            .collect();

        let mut account_infos = ctx.remaining_accounts.to_vec();
        account_infos.push(ctx.accounts.target_program.to_account_info());

        invoke(
            &Instruction {
                program_id: ctx.accounts.target_program.key(),
                accounts,
                data,
            },
            &account_infos,
        )?;

        Ok(())
    }

    /// Succeeds whatever it is handed, so a transaction can carry an
    /// arbitrary payload at a position where something else is being
    /// tested.
    pub fn noop(_ctx: Context<NoopAccountConstraints>, payload: Vec<u8>) -> Result<()> {
        // Carried, never inspected: what the tests need is bytes sitting
        // at a chosen position in the transaction.
        _ = payload;
        Ok(())
    }
}

#[derive(Accounts)]
pub struct ForwardAccountConstraints<'info> {
    /// CHECK: the program being invoked; a fixture forwards wherever it
    /// is pointed.
    pub target_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct NoopAccountConstraints {}
