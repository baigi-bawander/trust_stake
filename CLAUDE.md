# TrustStake

Staked reputation for peer-to-peer marketplaces on Solana. Full pitch, architecture, and rationale are in [README.md](README.md) — read that first for the "why." This file is operating guidance for working in the code, not a duplicate of it.

## Current state (update this section when it changes)

- **Stage:** working prototype, submitted as an Edversity/Superteam Pakistan capstone. Devnet
  runs `main`'s binary, now v2; see "The v2 rebuild" below.
- **Deployed:** devnet, program ID `3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2`. Upgraded in
  place to v2 on 2026-08-24 (data length 649,264 bytes, deployed in slot 487349484). v1 no
  longer exists at this address; v1's own transaction history stays valid on Solana Explorer
  regardless, since upgrading a program does not change chain history.
- **Tests:** `main` has 132 tests passing (`cargo test` from `programs/truststake/`, LiteSVM),
  plus a real devnet run with every signature recorded in
  [docs/TESTING.md](docs/TESTING.md).
- **Repo:** `https://github.com/baigi-bawander/trust_stake`
- **Upgrade authority / admin wallet:** `~/.config/solana/id.json` (pubkey
  `EE4skmuEcaL4ybktFhp7sUfr84to78KQKoNsAAu8L7jG`) is simultaneously the program's upgrade
  authority, the hardcoded `INITIAL_ADMIN`, and, as of the devnet deployment above, the devnet
  test mint's authority. Losing this key forfeits all three at once. Back it up outside the
  repository. Never commit it.

## How to treat this codebase

This is a prototype, not a finished product — nothing here is final, and improving it later is expected and welcome. The point of this file isn't "don't touch anything," it's to make sure changes are made **on purpose**, by someone who knows what they're changing and why, rather than a fresh session silently "fixing" something it mistook for a bug.

Concretely:

- The list below ("Deliberate simplifications") describes the *current* tradeoffs and the reasoning behind each. Treat that reasoning as context to weigh, not a rule that blocks work — if a task calls for building real DAO-based arbitration, storing buyer reputation onchain, or turning on a leverage multiplier, go ahead.
- The one ask: when you change something on this list, **update this file and the matching section of README.md** ("Current tradeoffs") to reflect the new reality. The failure mode this file exists to prevent is documentation quietly going stale — not change itself.
- If you're not sure whether something is a deliberate tradeoff or an actual bug, the README's "Current tradeoffs" section and the git history are the sources of truth — check there before assuming either way.

## The v2 rebuild

`main` is v2: a ground-up rebuild that replaced the original single-arbiter, native-SOL
prototype with a multi-tenant protocol (SPL-token collateral, unstaking, no single arbiter).
`docs/DESIGN-v2.md` has the full design rationale and build order behind that rebuild.

**v2 is deployed to devnet**, at the program ID above. `Config` is initialized (chain_id 1, a
demo-created 6-decimal test mint), and two marketplaces, one stake, two permits and one
resolved dispute exist as real state from `examples/devnet_demo.rs`. This is still devnet, not
mainnet: a change that breaks an account layout means a fresh deploy and a fresh demo run, not
a production migration.

**Build and test:** `anchor build`, then `OPENSSL_NO_VENDOR=1 cargo test` from
`programs/truststake/`. The env var is required in this environment; without it the vendored
OpenSSL build fails on a clock-skew check. LiteSVM loads the pre-built
`target/deploy/truststake.so` rather than the native test binary, so any handler change needs
`anchor build` before the tests reflect it. A `build.rs` guard fails the compile if that
`.so` is stale. Current state is 132 tests, all passing.

### Deliberate tradeoffs on v2, not bugs

`docs/DESIGN-v2.md` has an "Honest limitations" section with twelve entries, plus numbered
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

### Review history on v2

Phases 1, 2 and 3 have each had their own security review, and the dispute lifecycle has had
two dedicated passes including one adversarial pass that executed real exploit probes. The
weak spot has consistently been **cross-phase interaction**, which phase-scoped review cannot
see. Four of the worst bugs found so far lived there, all fixed: a permit released and
re-granted at the same address (Phase 2) interacting with a receipt's replay guard (Phase 3),
which produced an actual double-slash of seller funds; a PDA seed that did not name every
identity it was the sole guard for, which permanently locked a buyer out of ever filing a
complaint; `SlashPermit` carrying no field naming which era granted it, so a receipt from
a fully wound-down era could be filed against whatever got granted next at the same address —
fixed by `granted_at` and a bound in `raise_dispute` (docs/DESIGN-v2.md, check 7); and
`release_permit_early` allowing a revoked permit to be released immediately, with no minimum
wait, so a permit could be wound down and a new one granted at the same address inside the
clock-skew tolerance `raise_dispute`'s check 7 depends on, fixed by requiring
`now >= permit.revoked_at + CLOCK_SKEW_TOLERANCE_SECONDS`, deliberately reusing check 7's own
constant since the two are one guarantee split across two handlers. Assume a fifth of the same
kind exists until you have checked.

## Deliberate simplifications, as of now

- **No independent arbitration; each marketplace's own arbiter judges its own disputes**, up to the seller's permit cap. The arbiter is checked only as "is this the expected signer," never assumed to be a wallet, so a future jury program's PDA can occupy the slot with no change to this program; because terms are frozen per permit, that migration can happen one seller at a time.
- **No buyer reputation stored onchain.** A buyer identity costs nothing to abandon; buyer history is computable from `DisputeRaised`/`DisputeResolved`/`DisputeExpired` events instead of read from account state.
- **No identity or KYC data, anywhere in the design.** Everything written to Solana is public and permanent; verification stays inside a marketplace's own systems.
- **No leverage multiplier.** Backing above collateral is only safe once reputation is expensive to fabricate, which it currently is not.
- **A permit caps total damage across every buyer on one marketplace, not per-buyer coverage**; keeping order volume in line with a seller's live permit is left to the marketplace, published as an integration requirement.
- **No appeals, no partial refunds, no protocol fee, no frontend.**

See `docs/DESIGN-v2.md`, "What this design deliberately does not do" and "Honest limitations, stated rather than papered over," for the full list and the reasoning behind each; those two sections and this one and README.md's "Current tradeoffs" go stale together and get updated together.

## Working in this repo

- **Program logic:** `programs/truststake/src/` — `lib.rs` is the entrypoint/index, `state.rs` defines the three accounts, `instructions/` has the four handlers.
- **Unit tests:** `programs/truststake/tests/test_truststake.rs` — LiteSVM, in-process, fast. Run with `cargo test` from `programs/truststake/`.
- **Real devnet demo:** `programs/truststake/examples/devnet_demo.rs` — walks two marketplaces (v2's SPL-token collateral, not v1's native SOL) sharing one seller's stake as real transactions: one stake, two independent permits, a withdrawal against the cap that succeeds next to one that is meant to fail, and a dispute proved through the Ed25519 precompile. Gated behind the `devnet_demo` Cargo feature; run with `cargo run --example devnet_demo --features devnet_demo` from `programs/truststake/`. The signing wallet must match `constants::INITIAL_ADMIN`. It funds several throwaway keypairs by direct transfer (not airdrop, since devnet airdrops are rate-limited) and is idempotent on `initialize_config` and the test mint it pins, reusing both from the existing `Config` account if one is already there. `tests/test_devnet_demo_parity.rs` proves the same instruction sequence against LiteSVM first, before any of it spends devnet SOL.
- **Build:** `anchor build` (not plain `cargo build` — Solana programs need the SBF target, which `anchor build` invokes via `cargo build-sbf`).
- **Deploy:** `anchor deploy` / `solana program deploy`, costs real devnet SOL (program
  rent-exemption is roughly 6,960 lamports per byte of the compiled `.so`). Check `solana
  balance --url devnet` first. If a deploy fails partway, check `solana program show --buffers
  --url devnet` before retrying; there may be a paid-for buffer account worth resuming from
  (`solana program deploy --buffer <address> ...`) instead of paying rent again from scratch.
- **IDL, after any future deploy:** run `anchor idl upgrade
  3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2 -f target/idl/truststake.json
  --provider.cluster devnet`, never `anchor idl init`. The IDL account already exists (created
  2026-08-24), and `init` fails against an account that is already allocated. A stale IDL is
  worse than no IDL: Anchor numbers error codes positionally as 6000 + index and publishes
  that numbering in the IDL, so an outdated one mis-names every error and mis-decodes any
  instruction whose arguments changed.

## Known gotchas hit while building this

- **Anchor 1.0.x is a recent major version** with breaking changes from 0.32 (`CpiContext::new` takes a `Pubkey` now, not an `AccountInfo`; IDL handling changed). If you're referencing older Anchor examples/tutorials, expect some to be stale.
- **A wallet that ends a transaction with a nonzero balance must stay above the rent-exempt minimum for a bare account**, or the transaction is rejected in preflight. This bit the devnet demo script the first time: funding a throwaway wallet with *exactly* what it spends leaves it at a small nonzero remainder below that floor. Fund with an extra `get_minimum_balance_for_rent_exemption(0)` worth of headroom for any wallet that isn't being fully drained to zero.
- **`anchor idl init` (Anchor 1.1.2) can report failure even after it fully succeeds.** It
  printed `Error: Failed to initialize IDL` on the only IDL upload this project has done, but
  every one of the 8 onchain transactions it sent showed `Status: Ok`, and fetching the
  account back and decompressing it matched the local IDL exactly. A reported failure here
  means check onchain state, not retry: retrying blind would have hit a second, misleading
  error, since the account already exists. The verification standard is per-transaction status
  (`solana confirm -v <signature>`) plus a content round-trip, not the account merely
  existing: `anchor idl fetch`, with or without `-o`, returns raw zlib-compressed hex in this
  version rather than decoded JSON, so decode with `bytes.fromhex(...)` then
  `zlib.decompress(...)` before comparing against the local `target/idl/*.json`.
