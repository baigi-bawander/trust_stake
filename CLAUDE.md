# TrustStake

Staked reputation for peer-to-peer marketplaces on Solana. Full pitch, architecture, and rationale are in [README.md](README.md), read that first for the "why." This file is operating guidance for working in the code, not a duplicate of it.

## Current state (update this section when it changes)

- **Stage:** working prototype, submitted as an Edversity/Superteam Pakistan capstone. Devnet
  runs `main`'s binary, now v2, see "The v2 rebuild" below.
- **Deployed:** devnet, program ID `3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2`. First
  upgraded in place to v2 on 2026-08-24 (slot 487349484), and again on 2026-08-31 (slot
  491019091), which is the deploy live now. Data length stays 649,264 bytes: the account was
  allocated with headroom, and the current binary is 596,864 bytes, so the upgrade fitted
  without an extend. Before 2026-08-31 the deployed bytes were commit `34c5483`'s; they are
  now `main`'s, verified byte-for-byte by `solana program dump` against the local
  `target/deploy/truststake.so` (both SHA-256
  `101a10d8145870f77a2b79f66d41c8af85252a6678a21083fcf4316551369bfc`). That upgrade changed 23
  files under `programs/truststake/src/` and no account layout, and `ACCOUNT_VERSION` is 1 on
  both sides, so the devnet accounts written under the old binary are still read correctly by
  this one. v1 no longer exists at this address; v1's own transaction history stays valid on
  Solana Explorer regardless, since upgrading a program does not change chain history.
- **Tests:** `main` has 152 tests passing (`cargo test` from `programs/truststake/`, LiteSVM),
  plus a real devnet run with every signature recorded in
  [docs/TESTING.md](docs/TESTING.md).
- **Demo:** the four-stage devnet walk (`examples/devnet_demo.rs`, see "Real devnet demo"
  below) has completed stages 1-3, stage 1 on 2026-08-31, stage 2 on 2026-09-01, stage 3 on
  2026-09-03 (slot 492481349), putting 17 of the 19 handlers through a real transaction on
  devnet. Only `expire_dispute` and `close_dispute` remain, across three `DisputeRecord`s: the
  original CashDesk dispute from the Step 7 walk clears for `close_dispute` on 2026-09-23, and
  the SwiftMarket dispute (`expire_dispute`) and CashDesk's second dispute (`close_dispute`)
  both clear on 2026-09-30. `docs/TESTING.md`'s four-stage section has the signatures.
- **Repo:** `https://github.com/baigi-bawander/trust_stake`
- **Upgrade authority / admin wallet:** `~/.config/solana/id.json` (pubkey
  `EE4skmuEcaL4ybktFhp7sUfr84to78KQKoNsAAu8L7jG`) is simultaneously the program's upgrade
  authority, the hardcoded `INITIAL_ADMIN`, and the devnet test mint's authority. Losing this
  key forfeits all three at once. Back it up outside the repository. Never commit it.

## How to treat this codebase

This is a prototype, not a finished product, nothing here is final, and improving it later is expected and welcome. The point of this file isn't "don't touch anything," it's to make sure changes are made **on purpose**, by someone who knows what they're changing and why, rather than a fresh session silently "fixing" something it mistook for a bug.

Concretely:

- The tradeoffs recorded in "Deliberate tradeoffs on v2, not bugs" below, and the full list in `docs/DESIGN-v2.md`'s "Honest limitations, stated rather than papered over," describe the *current* reasoning, not rules that block work. If a task calls for building real DAO-based arbitration, storing buyer reputation onchain, or turning on a leverage multiplier, go ahead.
- The one ask: when you change something on either list, update `docs/DESIGN-v2.md`, this file's "Deliberate tradeoffs on v2, not bugs" section if the item is one of the six listed there, and README.md's "Current tradeoffs" section, so all three stay true together. The failure mode this file exists to prevent is documentation quietly going stale, not change itself.
- If you're not sure whether something is a deliberate tradeoff or an actual bug, `docs/DESIGN-v2.md` and the git history are the sources of truth, check there before assuming either way.

## The v2 rebuild

`main` is v2: a ground-up rebuild that replaced the original single-arbiter, native-SOL
prototype with a multi-tenant protocol (SPL-token collateral, unstaking, no single arbiter).
`docs/DESIGN-v2.md` has the full design rationale and build order behind that rebuild.

**v2 is deployed to devnet**, at the program ID above. `Config` is initialized (chain_id 1, a
demo-created 6-decimal test mint), and `examples/devnet_demo.rs` has since built up real state
across three marketplaces (CashDesk, 2-day window/1,000 bps; PixelBazaar, 7-day/300 bps;
SwiftMarket, 30-day/0 bps), two `SellerStake`s, four `SlashPermit`s, and three
`DisputeRecord`s (one upheld, one rejected, one still open). `getProgramAccounts` against the
program ID returns 15 accounts in total: those twelve, the one live `Config`, and two
unreadable v1-era leftovers (a 41-byte `Config` and a 53-byte `SellerStake`, sized to match the
v1 structs at `git show 52ae454:programs/truststake/src/state.rs`) that the in-place upgrade
left behind and that can never be closed, since the v1 code that owned them no longer exists at
this address. This is still devnet, not mainnet: a change that breaks an account layout means a
fresh deploy and a fresh demo run, not a production migration.

**Build and test:** `anchor build`, then `OPENSSL_NO_VENDOR=1 cargo test` from
`programs/truststake/`. The env var is required in this environment; without it the vendored
OpenSSL build fails on a clock-skew check. LiteSVM loads the pre-built
`target/deploy/truststake.so` rather than the native test binary, so any handler change needs
`anchor build` before the tests reflect it. A `build.rs` guard fails the compile if that `.so`
is stale (see "Known gotchas" below). Current state is 152 tests, all passing. A separate
suite, `OPENSSL_NO_VENDOR=1 cargo test --example devnet_demo --features devnet_demo` from the
same directory, unit-tests the demo script's own idempotency-guard decisions (28 tests), see
"Real devnet demo" below.

### Deliberate tradeoffs on v2, not bugs

`docs/DESIGN-v2.md` has an "Honest limitations" section with nineteen entries, plus numbered
design decisions. Those are considered and recorded, not oversights. Read them before
reporting anything as a defect. The six most often mistaken for bugs:

- **`INITIAL_ADMIN` is a hardcoded pubkey.** It stops a freshly deployed program having its
  config front-run by whoever notices the deployment first. Authority moves off it afterwards
  through the two-step transfer.
- **`Marketplace.bond_bps` has a ceiling but no floor.** Zero is legal on purpose. A floor
  would price out honest buyers with small claims. The ceiling binds at registration and
  again where the bond is charged: `grant_permit` copies a grandfathered rate across
  unchecked on purpose, and `raise_dispute` takes `min(permit.bond_bps, MAX_BOND_BPS)`.
- **The collateral mint must carry no token extensions.** `initialize_config` checks the
  mint's extension list against `ALLOWED_MINT_EXTENSIONS`, which is empty, so a Classic Token
  Program mint passes and every Token Extensions mint carrying an extension is refused. An
  allow-list so future extension types are denied by default; widening it is a deliberate,
  safe change, since `Config.collateral_mint` is pinned per deployment.
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
see. Six bugs of this species have been found and fixed, each stated below as the invariant it
taught. This matches docs/AUDIT.md's "Cross-phase bugs" table so the two cannot drift; that
table has the finding-level detail.

- A `SlashPermit` released and re-granted at the same address must not let a receipt's replay
  guard treat the new grant as a continuation of the old one, or a genuine receipt can
  double-slash the same collateral.
- A PDA seed that is the sole guard for an identity must name every identity it guards, or
  that identity can be permanently locked out of the action the seed protects.
- A stored permit must name which era granted it, or a receipt from a fully wound-down era can
  be filed against whatever gets granted next at the same address. Fixed by
  `SlashPermit.granted_at` and `raise_dispute` check 7.
- A revoked permit's early release must enforce its own minimum wait, or a wind-down-and-regrant
  cycle can complete inside the clock-skew tolerance check 7 depends on. Fixed by requiring
  `now >= revoked_at + CLOCK_SKEW_TOLERANCE_SECONDS`.
- A protocol ceiling compared against a stored, possibly grandfathered field must take the
  wider of the two, not the ceiling alone, or a grandfathered marketplace's `DisputeRecord` can
  be closed and its receipt replayed before its own longer window has elapsed. Fixed by
  deriving `closable_after` from `max(permit.complaint_window, MAX_COMPLAINT_WINDOW_SECONDS)`.
- The same pattern applies to `release_permit`'s wait: it must floor at
  `CLOCK_SKEW_TOLERANCE_SECONDS` rather than rely on a compile-time assertion between two
  constants, or a marketplace grandfathered below today's minimum window can release a permit
  before check 7's tolerance has actually elapsed. Fixed by
  `revoked_at + complaint_window.max(CLOCK_SKEW_TOLERANCE_SECONDS)`.

All three stored fields a protocol constant is compared against (the bond clamp, `closable_after`,
and `release_permit`'s wait) are now bounded at the point of use rather than trusted to already
sit inside the constant's range. A future protocol constant that gets compared against a
grandfathered, per-marketplace field should be treated as suspect until it is, too.

## Working in this repo

- **Program logic:** `programs/truststake/src/`, `lib.rs` is the entrypoint/index (19 `pub fn`
  handlers, counted directly off it), `state.rs` now just re-exports `state/`, which defines
  five accounts (`config.rs`, `dispute_record.rs`, `marketplace.rs`, `seller_stake.rs`,
  `slash_permit.rs`, `ls programs/truststake/src/state/`), `instructions/` has the nineteen
  handlers.
- **Unit tests:** `programs/truststake/tests/` (`ls`, there is no `test_truststake.rs`) holds
  four LiteSVM integration suites plus a shared harness: `test_phase1.rs` (32 tests) covers the
  nine foundation handlers, config and marketplace setup, authority transfer, staking;
  `test_phase2.rs` (33 tests) covers the six collateral-lifecycle handlers, `withdraw_stake`,
  `grant_permit`, `increase_permit`, `revoke_permit`, `release_permit`, `release_permit_early`;
  `test_phase3.rs` (71 tests) covers the four dispute handlers, `raise_dispute`,
  `resolve_dispute`, `expire_dispute`, `close_dispute`, including the Ed25519/introspection
  attack-probe section; `test_devnet_demo_parity.rs` (2 tests) replays `examples/devnet_demo.rs`'s
  instruction sequence against LiteSVM before it spends real devnet SOL, one against a blank
  chain, one against a chain pre-seeded with the original eight-step walk's own state, since a
  blank-chain-only replay cannot see a bug that only exists once a slash has moved `staked` and
  `committed` together (see "Known gotchas" below); `common/mod.rs` is the shared `World` test
  harness both use, not a test file itself. Run with `OPENSSL_NO_VENDOR=1 cargo test` from
  `programs/truststake/`.
- **Real devnet demo:** `programs/truststake/examples/devnet_demo.rs` exercises all 19 handlers
  (`lib.rs`) across three marketplaces and two sellers, in four wall-clock-gated stages, since
  three of the protocol's waits (`release_permit_early`'s clock-skew tolerance, `release_permit`'s
  complaint window, `expire_dispute`/`close_dispute`'s 30-day marks) are real time on devnet and
  cannot be skipped. Stage 1 runs immediately (config/marketplace authority transfers, a third
  marketplace SwiftMarket registered at 0 bps bond, a PixelBazaar bond-rate change, a CashDesk
  receipt-signer rotation, `increase_permit`, two more disputes, and a second seller who stakes,
  grants, and revokes two permits); stage 2 (`release_permit_early`) is due 1 hour later; stage 3
  (`release_permit`) 2 days later; stage 4 (`expire_dispute`, `close_dispute` x2) 30 days later.
  Gated behind the `devnet_demo` Cargo feature; run with `cargo run --example devnet_demo
  --features devnet_demo` from `programs/truststake/`, the command never changes, an optional
  `--stage N` (1-4) forces just one stage, and by default every stage currently due runs,
  printing what remains and exiting 0 (not an error) for whichever isn't due yet. The signing
  wallet must match `constants::INITIAL_ADMIN`. Stage progress and chain facts a fresh
  invocation cannot re-derive on its own (onchain IDs, `order_id`s, revocation timestamps) live
  in `programs/truststake/examples/.devnet-demo-state/progress.json` (gitignored, alongside the
  existing `.devnet-demo-keypairs/`). Every step is idempotency-guarded before acting, including
  the original eight steps: `initialize_stake`/`grant_permit`/`register_marketplace` all use
  Anchor's `init`, which hard-errors on a second call, and `raise_dispute`'s `order_id` was
  fresh-random every run, so a naive re-run would have hard-failed or silently raised duplicate
  disputes. `tests/test_devnet_demo_parity.rs` proves the entire four-stage sequence against
  LiteSVM first, including the boundary of every wait, both directions, before any of it spends
  devnet SOL. Parity coverage never exercises a guard, though, it replays the instruction
  sequence, not the script's own decision code, so every guard that decides "has this step
  already run" was, until the fix recorded in `docs/DEMO-SCRIPT-FINDINGS.md`, untested and prone
  to the same species of bug the program itself had already fixed in `SlashPermit.granted_at`:
  inferring "already done" from live chain state that a *later* step of the same script can
  destroy or decrease. Those guard decisions are now pure functions with their own
  `#[cfg(test)] mod tests` inside `devnet_demo.rs`, run via the command in "Build and test"
  above. The plain command is safe to re-run: `--stage N` is now only a convenience to force
  one stage on or off its normal schedule, not a safety measure.
- **Build:** `anchor build` (not plain `cargo build`, Solana programs need the SBF target, which
  `anchor build` invokes via `cargo build-sbf`). `target/deploy/truststake-keypair.json`
  (pubkey `G8B59KZpf7siepeb3PRX3KuPk4LC1LZarF8YmuPHGUZ`) does not match `declare_id!` in
  `src/lib.rs` or either `[programs.*]` entry in `Anchor.toml` (both
  `3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2`, the deployed program). That keypair is a
  gitignored build artifact Anchor generated on some earlier build; the deployed program's own
  keypair was never kept locally. `anchor build` (anchor-cli 1.1.2) prints `Program ID mismatch
  detected` and points at `anchor keys sync` and `--ignore-keys`, but the mismatch is a warning,
  not a build failure, the build still exits 0 and produces the `.so` either way. **Never run
  `anchor keys sync`**: it resolves the mismatch the wrong way, rewriting `declare_id!` in
  source to match the throwaway keypair. Every PDA in this program derives from the program ID,
  so the rebuilt binary would address an entirely different set of accounts, orphaning the
  deployed program and all its devnet state, and invalidating the published IDL.
- **Deploy:** never plain `anchor deploy`, it defaults to `--program-keypair
  target/deploy/truststake-keypair.json`, so it would deploy a brand-new program at
  `G8B59KZpf7siepeb3PRX3KuPk4LC1LZarF8YmuPHGUZ` and spend devnet SOL, rather than upgrading the
  deployed one. An upgrade goes through `solana program deploy --program-id
  3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2 target/deploy/truststake.so`, authorised by
  `--upgrade-authority` (default: the configured keypair, `~/.config/solana/id.json`), the
  program keypair is not needed for it. Either way, this costs real devnet SOL (program
  rent-exemption is roughly 6,960 lamports per byte of the compiled `.so`). Check `solana
  balance --url devnet` first. If a deploy fails partway, check `solana program show --buffers
  --url devnet` before retrying; there may be a paid-for buffer account worth resuming from
  (`solana program deploy --buffer <address> ...`) instead of paying rent again from scratch.
- **IDL, after any future deploy:** run `anchor idl upgrade
  3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2 -f target/idl/truststake.json --provider.cluster
  devnet`, never `anchor idl init`. The IDL account already exists (created 2026-08-24), and
  `init` fails against an account that is already allocated. A stale IDL is worse than no IDL:
  Anchor numbers error codes positionally as 6000 + index and publishes that numbering in the
  IDL, so an outdated one mis-names every error and mis-decodes any instruction whose arguments
  changed. The repo also carries a copy of the current IDL at
  [idl/truststake.json](idl/truststake.json) (see that directory's README), a real fix for
  clone-and-build consumers, independent of the onchain upload's status.

  The onchain upload of a fresher IDL is unresolved (Anchor 1.1.2 shells out to
  `npx --package=@solana-program/program-metadata@0.5.1`, and the actual execution step fails
  client-side inside that tool with no root cause identified yet, confirmed not a partial-write
  or corruption risk since the failing step submits nothing). This is an open, bounded
  limitation, not outstanding debt requiring action: the account it would update is cosmetic,
  since the published 2026-08-24 IDL is still safe to use (verified by fetching, decompressing,
  and diffing against `target/idl/truststake.json`: all 19 instructions and every shared error
  code 6000-6037 identical; it only lacks names for two appended error variants and one stale
  doc string, neither of which mis-decodes anything), and `idl/truststake.json` in-repo is the
  reliable path for consumers regardless.

  `program-metadata`'s `list-buffers` command lists five addresses under our authority.
  `A1r5sAi41UnpGBL9Xo2gk1T12LzCDVo3d79MEsM4xvkc` is the **live IDL metadata account**, not a
  buffer, running `close-buffer` or `anchor idl close` against it would destroy the published
  IDL. Only these three are genuinely dead, abandoned buffers, 0.06245208 SOL each:
  `6mkxvZp8SnBAfBg7iq3WQizT35QQJCZjpyyGFerkrWnm`,
  `CHTWvALh4J7Wjr4GkrQihhu5KyRnsu6H7QGLLKNzCQRL`,
  `H1P5eHX9a336bAWTtJ9HGxLwQU6q9sqVA8Geh9jRFcCN`. Reclaiming them, and any further attempt at the
  onchain upload, are both deliberately parked, do not act on either without being asked.
- **Before changing any protocol constant or any field on a stored account, search the entire
  project for every site that reads it, and report those sites before making the change.**
  `ACCOUNT_VERSION` was written at five call sites and read at zero for the project's entire
  history until this task gave it a constraint; a thirty-second search would have shown that at
  any point, and nobody ran one.
- **Every count or file path written into this file must be verified by running a command at
  the moment of writing it, never by reasoning, reading, or copying a figure from an earlier
  version of this file or from a prior conversation.** This file has carried a stale count on
  four separate occasions, each found by accident during unrelated work. A task that changes
  the test count, the handler count, the account count, or the "Honest limitations" entry count
  in `docs/DESIGN-v2.md` must update this file in the same session, not defer it, since deferred
  is how the previous occasions went stale.

## Solana MCP server

`.mcp.json` (commit 4d9a311) wires up an http MCP server named `solana` at
`https://mcp.solana.com/mcp`, no auth. Five tools, all exercised against this repo on
2026-08-27 rather than taken from their descriptions. What follows is the result of that, not a
restatement of the catalogue.

**Reach for it whenever** a question touches an Anchor API, a Solana runtime behaviour, a
precompile or sysvar layout, or an SPL instruction, before answering from model memory.
"Known gotchas," below, records that Anchor 1.0.x broke compatibility with 0.32 and that older
tutorials are stale; this server is the fix for exactly that. It also indexes production
program source, not just prose, which makes differential review possible: compare a mechanism
here against how a shipped program solves it and investigate every divergence.

- **`get_documentation`**, the most useful of the five. Required `section`: a source id or a
  section id, string or array. Returns a whole corpus, so pull only what you need (50KB per
  source, 200KB total).
- **`Solana_Documentation_Search`**, required `query`. Semantic RAG, returns ranked chunks.
  Cheap. Use for a narrow question or a specific error.
- **`list_sections`**, no args, ~33KB. The catalogue. Only needed when hunting a source id the
  list below doesn't already name.
- **`Solana_Expert__Ask_For_Help`**, **redundant, don't use it.** The identical query string
  through it and `Solana_Documentation_Search` returned the same twenty sources in the same
  order, differing only in the third decimal of the similarity scores. It is the same retrieval
  backend under a second name, and despite what the name suggests it synthesises no answer, it
  returns raw chunks like the search tool does. One tool, use the search one.
- **`program_autofixer`**, marginal here, and not part of any security argument. See below.

### Source ids that matter for this repo

Saves a 33KB `list_sections` call. All verified present.

- `gh-sealevel-attacks`, the canonical taxonomy of Solana-specific exploit classes, each with
  insecure/secure/recommended variants. **Our own cross-phase bugs fall inside it:** the
  replay and stale-era bugs are class 9 (closing accounts, revival after close), the PDA-seed
  bug is class 8 (PDA sharing, a seed that doesn't name every identity it guards). Read this
  before any security pass; it names the shapes we have been finding by hand.
- `anchor-docs`, `gh-anchor`, Anchor 1.x canonical. Account constraints, and the constraint
  *execution order* (`close` runs in the exit handler, after the handler body).
- `gh-solana-sdk`, `ed25519-program` layout, sysvar helpers, the secp256k1 module's security
  notes, which spell out the introspection checks a verifier must make.
- `gh-simd`, SIMD-0152 is the precompile specification: `num_signatures`, the three
  `*_instruction_index` fields, `0xFFFF` meaning "this instruction."
- `gh-anchor-instruction-sysvar`, `gh-solana-ed25519-instruction`,
  `gh-solana-transaction-introspection`, `gh-anchor-escrow-introspection`, four independent
  takes on our exact receipt-verification pattern.
- `gh-drift-protocol-v2`, a production ed25519 verifier (`sig_verification.rs`) to diff
  `src/ed25519.rs` against.
- `gh-litesvm`, our test harness. `gh-spl-token`, `gh-spl-token-2022`, our collateral.
  `gh-idl-program`, onchain IDL storage, relevant to the IDL rule above.

`src/ed25519.rs` was checked against SIMD-0152 and the SDK's security notes this way on
2026-08-27: every attack vector those describe is closed. That is the first time our most
exploitable surface was validated against the specification rather than against reviewers'
reasoning. Re-run that comparison if the module changes.

### `program_autofixer`, and why it is not evidence

It is a single-file static linter over a closed ruleset, no cross-file dataflow, no
instruction-ordering model, no view of the state machine. A control run on deliberately broken
code measured the ceiling: a handler with no authorization check at all, letting any caller
reassign admin and drain the vault, was reported only as `low`, while a missing `space`
attribute was rated `high`. It checks shapes, not authority, ordering, or state. All six
cross-phase bugs (see "Review history on v2") are strictly harder than the one it missed. A
clean run is optional confirmation before committing program changes, nothing more, and is
never evidence in a cross-phase discussion.

**Third-party disclosure:** `program_autofixer` transmits the code passed to it to a
third-party service. Acceptable here only because `baigi-bawander/trust_stake` is public and
MIT-licensed. A private fork must not call it unmodified.

## Known gotchas hit while building this

- **Anchor 1.0.x is a recent major version** with breaking changes from 0.32 (`CpiContext::new`
  takes a `Pubkey` now, not an `AccountInfo`; IDL handling changed). If you're referencing older
  Anchor examples/tutorials, expect some to be stale.
- **A wallet that ends a transaction with a nonzero balance must stay above the rent-exempt
  minimum for a bare account**, or the transaction is rejected in preflight. This bit the
  devnet demo script the first time: funding a throwaway wallet with *exactly* what it spends
  leaves it at a small nonzero remainder below that floor. Fund with an extra
  `get_minimum_balance_for_rent_exemption(0)` worth of headroom for any wallet that isn't being
  fully drained to zero.
- **`anchor build` can fail inside `programs/truststake/build.rs`'s staleness guard, and
  deleting the artifacts is the wrong fix.** The symptom is a panic that
  `target/deploy/cpi_wrapper.so is older than .../Cargo.lock`, ending in `Error: Building IDL
  failed`. It reads like a stale artifact; it is not. `anchor build` processes workspace members
  strictly one at a time and completely, running this crate's *host*-toolchain IDL pass after
  `truststake` but before `cpi_wrapper` is built at all, so a guard that demands a fresh
  `cpi_wrapper.so` fires during the very command that would produce it. This was compounded by
  the test harness embedding that sibling artifact via `include_bytes!` (so a clean
  `target/deploy/` broke `anchor build` outright even with the guard removed) and by
  `Cargo.lock`'s mtime moving independently of its content (a re-resolve, a checkout, an editor
  save), which permanently outran a `.so` cargo correctly declined to rebuild, making the
  guard's own advice, "run `anchor build`," unsatisfiable. A previous session escaped by
  deleting the `.so` files, which only reset the mtimes; the failure returned the next time
  anything wrote `Cargo.lock`. Fixed on 2026-08-31 (commit `08d8322`): the guard skips the IDL
  pass, `tests/common/mod.rs` reads both artifacts at run time instead of embedding them, and
  `Cargo.lock` plus both manifests are compared by content against copies in
  `target/deploy/.build-guard/` taken at the last SBF build. If you see this error again, read
  `build.rs`'s module docs before touching anything, do not delete artifacts.
- **`anchor idl init` (Anchor 1.1.2) can report failure even after it fully succeeds.** It
  printed `Error: Failed to initialize IDL` on the only IDL upload this project has done, but
  every one of the 8 onchain transactions it sent showed `Status: Ok`, and fetching the account
  back and decompressing it matched the local IDL exactly. A reported failure here means check
  onchain state, not retry: retrying blind would have hit a second, misleading error, since the
  account already exists. The verification standard is per-transaction status (`solana confirm
  -v <signature>`) plus a content round-trip, not the account merely existing: `anchor idl
  fetch`, with or without `-o`, returns raw zlib-compressed hex in this version rather than
  decoded JSON, so decode with `bytes.fromhex(...)` then `zlib.decompress(...)` before comparing
  against the local `target/idl/*.json`.
- **A devnet_demo.rs step whose amounts are hardcoded against a fresh-chain assumption breaks
  the instant a slash has ever touched the seller, and a blank-chain-only parity test cannot
  catch it.** `SellerStake::slash` reduces `staked` and `committed` TOGETHER, so a step written
  against "committed is always 400" (the value on a fresh chain) goes wrong wherever the
  difference `staked - committed` (free collateral) is what's actually being compared against,
  not either figure alone. This hit real devnet on 2026-08-31: a withdrawal step assumed
  committed = 400, but an earlier upheld dispute had already reduced it, so a withdrawal sized
  to land exactly at the (stale) cap landed comfortably inside the real one instead, and the
  "this must fail" half of the demonstration silently succeeded, proving nothing, not erroring
  loudly. The blank-chain-only parity test kept passing throughout, because the fresh-chain
  assumption is always true by construction there. The fix has two parts, both required: (1)
  derive the amount from live state immediately before the step, free collateral for a "must
  succeed" step, `free + 1` for a "must fail" one, never from a constant; and (2) a SECOND
  parity test that pre-seeds LiteSVM with prior-walk state before replaying on top of it
  (`test_devnet_demo_resumed_after_prior_slash`), since only a chain that already carries a
  prior slash can exercise this class of bug at all. A future step written against "amount X is
  always safe/unsafe relative to committed" should be treated as suspect until proven to read
  `staked`/`committed` live at the point it runs.
