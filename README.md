# TrustStake

Staked reputation for peer-to-peer marketplaces on Solana: a seller locks collateral, and a marketplace's arbiter can slash it to pay a buyer who was scammed.

## Deployment

| Cluster | Program ID | Data length |
| --- | --- | --- |
| Devnet | `3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2` | 649,264 bytes |

Upgrade authority: `EE4skmuEcaL4ybktFhp7sUfr84to78KQKoNsAAu8L7jG`. Not deployed to mainnet.

## Verify this deployment

```bash
solana-verify verify-from-repo https://github.com/baigi-bawander/trust_stake \
  --url https://api.devnet.solana.com \
  --program-id 3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2 \
  --library-name truststake \
  --commit-hash a1fb8e9 \
  --base-image quay.io/ottersec/anchor:v1.1.2
```

- Requires Docker running and `cargo install solana-verify`.
- `--base-image` is required: the default image's cargo can't build this project (`cpufeatures-0.3.0` needs the edition2024 Cargo feature).
- `--library-name truststake` is required because the workspace also contains a test fixture program, `cpi_wrapper`.
- `anchor verify` does not work here: it fails to parse a `solana-program` version from `Cargo.lock`, since this program depends on `anchor-lang`/`anchor-spl` rather than `solana-program` directly.
- The command ends with a prompt asking whether to upload verification data onchain. Answering `n` completes the check and writes nothing.
- Commit `a1fb8e9` is cited because it's the commit whose tree was actually verified; later commits touch only documentation, not program source, so the deployed binary still matches it.

Run on 2026-09-10, this reported "Program hash matches" with hash `4d93d69683c40d52eeb389d5db5d7e140831bd97c9a1e3bc9ccfb2fcae2f08fd` on both sides.

## How it works

| Account | PDA seeds | Holds |
| --- | --- | --- |
| `Config` | `["config", "v2"]` | The protocol admin, the collateral mint pinned at `initialize_config`, and the chain ID checked against every receipt. One per deployment. |
| `Marketplace` | `["market", "v2", marketplace_id]` | A tenant's receipt-signing key, arbiter, and the complaint window and bond rate it freezes onto every permit it grants. Owns a `bond_vault` token account holding live dispute bonds. |
| `SellerStake` | `["stake", "v2", seller]` | One per seller, shared across every marketplace they sell on: amount staked, amount committed to open permits, and dispute counts. Owns a `stake_vault` token account holding the seller's collateral. |
| `SlashPermit` | `["permit", "v2", seller, marketplace]` | A seller's bounded, revocable line of credit to one marketplace: maximum slashable amount, amount already slashed, and the complaint window and bond rate frozen at grant. |
| `DisputeRecord` | `["dispute", "v2", marketplace, seller, order_id]` | One open or resolved complaint against one order: the claim, the bond deposited, and its expiry and closing timestamps. Its address also doubles as the replay guard against a second complaint on the same order. |

Collateral lives in a separate token vault, not on `SellerStake` itself, so a slash only ever moves vault tokens and never touches the account's own rent-exempt balance.

## Instruction set

| Phase | Instruction | Does |
| --- | --- | --- |
| Foundation | `initialize_config` | Creates `Config`, pinning the collateral mint and admin once. |
| Foundation | `propose_config_authority` | Starts a two-step handoff of the admin key. |
| Foundation | `accept_config_authority` | Completes the admin handoff. |
| Foundation | `register_marketplace` | Registers a tenant with its receipt signer, arbiter, complaint window and bond rate. |
| Foundation | `update_marketplace` | Updates a tenant's live settings; permits already granted keep their frozen terms. |
| Foundation | `propose_marketplace_authority` | Starts a two-step handoff of a marketplace's authority. |
| Foundation | `accept_marketplace_authority` | Completes the marketplace authority handoff. |
| Foundation | `initialize_stake` | Creates a seller's shared stake account and its collateral vault. |
| Foundation | `add_stake` | Deposits collateral into the vault. |
| Collateral lifecycle | `withdraw_stake` | Withdraws collateral, gated so staked can never drop below committed. |
| Collateral lifecycle | `grant_permit` | Grants a seller's bounded, revocable exposure to one marketplace. |
| Collateral lifecycle | `increase_permit` | Raises a permit's cap. |
| Collateral lifecycle | `revoke_permit` | Starts winding a permit down. |
| Collateral lifecycle | `release_permit` | Releases a revoked permit after its complaint window has passed. |
| Collateral lifecycle | `release_permit_early` | Releases a revoked permit early, gated by a clock-skew wait. |
| Disputes | `raise_dispute` | Opens a complaint backed by a marketplace-signed receipt, verified by the Ed25519 precompile in the same transaction. |
| Disputes | `resolve_dispute` | The marketplace's own arbiter decides the complaint, either way. |
| Disputes | `expire_dispute` | Frees the seller's committed collateral if nobody decides within 30 days. |
| Disputes | `close_dispute` | Reclaims a resolved or expired dispute's rent. |

## Build and test

```bash
anchor build
cd programs/truststake && OPENSSL_NO_VENDOR=1 cargo test
```

`anchor build` has to run first: tests load the compiled `target/deploy/truststake.so` through LiteSVM rather than compiling the program natively, so a handler change needs a rebuild before the tests see it. `OPENSSL_NO_VENDOR=1` is required in this environment: without it, the vendored OpenSSL build fails on a clock-skew check.

## Deploy

```bash
solana config set --url devnet
solana program deploy --program-id 3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2 \
  target/deploy/truststake.so
```

Never `anchor deploy`: it defaults to `--program-keypair target/deploy/truststake-keypair.json`, which deploys a brand new program at a different address and spends devnet SOL without touching the program actually live at the address above. The command shown upgrades that program in place, authorized by `--upgrade-authority` (defaults to the configured keypair, `solana config get`).

If the new binary no longer fits the program account's current allocation, extend it first, with headroom so the next build doesn't need this again:

```bash
solana program extend 3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2 <difference-plus-headroom>
```

A program account can never shrink, so that rent is permanent. Deploying costs real devnet SOL, roughly 6,960 lamports per byte of the compiled `.so`; check `solana balance --url devnet` first. If a deploy fails partway, check `solana program show --buffers --url devnet` before retrying: there may be a paid-for buffer worth resuming from instead of paying rent again.

The IDL is committed at [idl/truststake.json](idl/truststake.json); see [idl/README.md](idl/README.md) for keeping it in sync, and CLAUDE.md's IDL entry for the onchain-upload status.

## Status

- 152 tests passing (`cargo test` from `programs/truststake/`).
- All 19 handlers are covered by the test suite, and 17 have also run in a real transaction on devnet across three completed demo stages.
- The remaining two, `expire_dispute` and `close_dispute`, are gated on real 30-day waits completing 2026-09-23 and 2026-09-30.

Full test plan and devnet transaction signatures: [docs/TESTING.md](docs/TESTING.md).

## Current tradeoffs

- **No independent arbitration**: each marketplace's own arbiter judges its own disputes.
- **No buyer reputation stored onchain**: buyer history is computed from events instead.
- **No identity or KYC data**, anywhere in the design.
- **No leverage multiplier**: reputation isn't yet expensive enough to fake to make this safe.
- **A permit caps total damage per marketplace, not per buyer.**
- **No appeals, no partial refunds, no protocol fee, no frontend.**

Full list and reasoning: [docs/DESIGN-v2.md](docs/DESIGN-v2.md), "Honest limitations, stated rather than papered over."

## Security

No paid external audit exists. See [SECURITY.md](SECURITY.md) for the disclosure process and [docs/AUDIT.md](docs/AUDIT.md) for what internal review has covered.

## Documentation

- [docs/DESIGN-v2.md](docs/DESIGN-v2.md): the full design, account model, every instruction handler, the offchain receipt format, and marketplace integration requirements.
- [docs/TESTING.md](docs/TESTING.md): the test plan behind the 152-test suite, plus the live devnet transaction signatures proving it runs onchain.
- [docs/SECURITY-CONTEXT.md](docs/SECURITY-CONTEXT.md): architectural reconnaissance written for whoever audits the program next.
- [docs/AUDIT.md](docs/AUDIT.md): 61 findings from three independent design-stage reviews plus a post-build sweep, and the six cross-phase bugs found since.

## Credit

The problem statement comes from ONDC India, published on [Superteam's idea board](https://superteam.fun/build/ideas/reputation-based-slashing). The design and implementation here are original.

## License

MIT, see [LICENSE](LICENSE).
