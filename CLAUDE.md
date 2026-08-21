# TrustStake

Staked reputation for peer-to-peer marketplaces on Solana. Full pitch, architecture, and rationale are in [README.md](README.md) — read that first for the "why." This file is operating guidance for working in the code, not a duplicate of it.

## Current state (update this section when it changes)

- **Stage:** working prototype, submitted as an Edversity/Superteam Pakistan capstone.
- **Deployed:** devnet, program ID `3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2`.
- **Tests:** 3/3 passing (`cargo test`, LiteSVM). A real end-to-end run against devnet has also been executed and verified on Solana Explorer — see the commit history for those transaction signatures.
- **Repo:** `https://github.com/baigi-bawander/trust_stake`

## How to treat this codebase

This is a prototype, not a finished product — nothing here is final, and improving it later is expected and welcome. The point of this file isn't "don't touch anything," it's to make sure changes are made **on purpose**, by someone who knows what they're changing and why, rather than a fresh session silently "fixing" something it mistook for a bug.

Concretely:

- The list below ("Deliberate simplifications") describes the *current* tradeoffs and the reasoning behind each. Treat that reasoning as context to weigh, not a rule that blocks work — if a task calls for building real DAO-based arbitration, or adding unstaking, or switching to USDC, go ahead.
- The one ask: when you change something on this list, **update this file and the matching section of README.md** ("Prototype scope") to reflect the new reality. The failure mode this file exists to prevent is documentation quietly going stale — not change itself.
- If you're not sure whether something is a deliberate tradeoff or an actual bug, the README's "Prototype scope" section and the git history are the sources of truth — check there before assuming either way.

## v2 rebuild in progress

The list below describes `main`, the deployed prototype. A ground-up rebuild is underway on
branch `v2-rebuild` (multi-tenant, SPL-token collateral, unstaking, no single arbiter) and
several items below no longer apply there. See `docs/DESIGN-v2.md` for what's actually
happening on that branch. This section and the rest of this file get rewritten together once
that branch lands, per Phase 4 of that doc's build order — not incrementally per phase, so it
doesn't get rewritten three times while the architecture is still moving.

**Nothing is deployed on v2.** There is no live state and no migration concern, so a change
that breaks an account layout is still cheap on this branch.

**Build and test on this branch:** `anchor build`, then `OPENSSL_NO_VENDOR=1 cargo test` from
`programs/truststake/`. The env var is required in this environment; without it the vendored
OpenSSL build fails on a clock-skew check. LiteSVM loads the pre-built
`target/deploy/truststake.so` rather than the native test binary, so any handler change needs
`anchor build` before the tests reflect it. A `build.rs` guard fails the compile if that
`.so` is stale. Current state is 119 tests, all passing.

### Deliberate tradeoffs on v2-rebuild, not bugs

`docs/DESIGN-v2.md` has an "Honest limitations" section with eleven entries, plus numbered
design decisions. Those are considered and recorded, not oversights. Read them before
reporting anything as a defect. The five most often mistaken for bugs:

- **`INITIAL_ADMIN` is a hardcoded pubkey.** It stops a freshly deployed program having its
  config front-run by whoever notices the deployment first. Authority moves off it afterwards
  through the two-step transfer.
- **`Marketplace.bond_bps` has a ceiling but no floor.** Zero is legal on purpose. A floor
  would price out honest buyers with small claims.
- **`SellerStake.disputes_total` only ever increases, and is inflatable.** The counters are a
  convenience. Attack-resistant reputation is computed offchain from events, which carry the
  marketplace.
- **`Marketplace` keeps a single `prev_receipt_signer` slot, not a ring.** A second rotation
  inside one complaint window overwrites the first. A cooldown was considered and rejected,
  because it blocks the emergency it exists to handle.
- **The vault conservation check is `>=`, not `==`.** This is a fix, not a slip: exact
  equality let anyone brick a seller's vault by donating one token unit into it. The
  dangerous direction, a vault holding less than its ledger, is still caught.

### Review history on v2-rebuild

Phases 1, 2 and 3 have each had their own security review, and the dispute lifecycle has had
two dedicated passes including one adversarial pass that executed real exploit probes. The
weak spot has consistently been **cross-phase interaction**, which phase-scoped review cannot
see. Both of the worst bugs found so far lived there: a permit released and re-granted at the
same address (Phase 2) interacting with a receipt's replay guard (Phase 3), which produced an
actual double-slash of seller funds; and a PDA seed that did not name every identity it was
the sole guard for, which permanently locked a buyer out of ever filing a complaint. Both are
fixed. Assume a third of the same kind exists until you have checked.

## Deliberate simplifications, as of now

- **Single arbiter key** (`resolve_dispute` only accepts one hardcoded authority via `Config`). A real deployment needs a multisig or DAO vote — this was scoped down for a testable MVP within a hackathon timeframe, not because multi-party arbitration is hard to justify.
- **Native SOL, not a stablecoin.** Avoids token-account plumbing. USDC is the more realistic asset for real sellers.
- **One open dispute per buyer/seller pair**, no appeals, no expiry, no partial refunds.
- **No unstaking instruction** — a seller with a clean record can't withdraw collateral yet.
- **No frontend.** The mechanism is demonstrated via tests and the devnet demo script, not a UI.

## Working in this repo

- **Program logic:** `programs/truststake/src/` — `lib.rs` is the entrypoint/index, `state.rs` defines the three accounts, `instructions/` has the four handlers.
- **Unit tests:** `programs/truststake/tests/test_truststake.rs` — LiteSVM, in-process, fast. Run with `cargo test` from `programs/truststake/`.
- **Real devnet demo:** `programs/truststake/examples/devnet_demo.rs` — replays the same flow as real transactions. Run with `cargo run --example devnet_demo` from `programs/truststake/`. It funds two throwaway keypairs via direct transfer (not airdrop, since devnet airdrops are rate-limited) and is idempotent on `initialize_config` (skips it if the `Config` account already exists on-chain).
- **Build:** `anchor build` (not plain `cargo build` — Solana programs need the SBF target, which `anchor build` invokes via `cargo build-sbf`).
- **Deploy:** `anchor deploy` / `solana program deploy` — costs real devnet SOL (program rent-exemption is ~1.37 SOL for the current binary size). Check `solana balance --url devnet` first. If a deploy fails partway, check `solana program show --buffers --url devnet` before retrying — there may be a paid-for buffer account worth resuming from (`solana program deploy --buffer <address> ...`) instead of paying rent again from scratch.

## Known gotchas hit while building this

- **Anchor 1.0.x is a recent major version** with breaking changes from 0.32 (`CpiContext::new` takes a `Pubkey` now, not an `AccountInfo`; IDL handling changed). If you're referencing older Anchor examples/tutorials, expect some to be stale.
- **A wallet that ends a transaction with a nonzero balance must stay above the rent-exempt minimum for a bare account**, or the transaction is rejected in preflight. This bit the devnet demo script the first time: funding a throwaway wallet with *exactly* what it spends leaves it at a small nonzero remainder below that floor. Fund with an extra `get_minimum_balance_for_rent_exemption(0)` worth of headroom for any wallet that isn't being fully drained to zero.
