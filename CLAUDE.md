# TrustStake

Staked reputation for peer-to-peer marketplaces on Solana. Full pitch, architecture, and rationale are in [README.md](README.md) — read that first for the "why." This file is operating guidance for working in the code, not a duplicate of it.

## Current state (update this section when it changes)

- **Stage:** working prototype, submitted as an Edversity/Superteam Pakistan capstone. Devnet
  runs `main`'s binary, now v2; see "The v2 rebuild" below.
- **Deployed:** devnet, program ID `3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2`. First
  upgraded in place to v2 on 2026-08-24 (slot 487349484), and again on 2026-08-31 (slot
  491019091), which is the deploy live now. Data length stays 649,264 bytes — the account was
  allocated with headroom, and the current binary is 596,864 bytes, so the upgrade fitted
  without an extend. Before 2026-08-31 the deployed bytes were commit `34c5483`'s; they are
  now `main`'s, verified byte-for-byte by `solana program dump` against the local
  `target/deploy/truststake.so` (both SHA-256
  `101a10d8145870f77a2b79f66d41c8af85252a6678a21083fcf4316551369bfc`). That upgrade changed 23
  files under `programs/truststake/src/` and no account layout: `git diff 34c5483..main --
  programs/truststake/src/state/ programs/truststake/src/state.rs | grep -E "^[+-]\s+pub "`
  returns nothing, and `ACCOUNT_VERSION` is 1 on both sides, so the devnet accounts written
  under the old binary are still read correctly by this one. v1 no longer exists at this
  address; v1's own transaction history stays valid on Solana Explorer regardless, since
  upgrading a program does not change chain history.
- **Tests:** `main` has 152 tests passing (`cargo test` from `programs/truststake/`, LiteSVM),
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
`.so` is stale. Current state is 152 tests, all passing. A separate suite,
`OPENSSL_NO_VENDOR=1 cargo test --example devnet_demo --features devnet_demo` from the same
directory, unit-tests the demo script's own idempotency-guard decisions (28 tests) — see the
"Real devnet demo" entry below.

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
constant since the two are one guarantee split across two handlers.

A fifth has since turned up: `closable_after`, the field that lets `close_dispute` reclaim a
`DisputeRecord`'s rent, was derived from the *live* permit's own `complaint_window` rather
than the protocol ceiling, so a marketplace grandfathered onto a window wider than the
current `MAX_COMPLAINT_WINDOW_SECONDS` (see docs/DESIGN-v2.md's grandfathering entry) could
have its `DisputeRecord` closed by anyone before that marketplace's own longer window had
elapsed, freeing the PDA for the same still-valid receipt to be filed again and the same
order slashed twice — fixed by deriving `closable_after` from
`max(permit.complaint_window, MAX_COMPLAINT_WINDOW_SECONDS)` instead. Unlike the first four,
this one was latent rather than live: it needed a future edit to
`MAX_COMPLAINT_WINDOW_SECONDS` to arm it, not just the sequence of handlers already in the
program. A sibling of the same species turned up in the same pass — check 7's own
rejection of a stale-era receipt turns out to depend on
`CLOCK_SKEW_TOLERANCE_SECONDS <= MIN_COMPLAINT_WINDOW_SECONDS` holding, currently true by a
48x margin but, at the time, not actually what made check 7 correct (see below). Both are
an assumed, unrecorded relationship between two constants — a seam distinct from
the release/re-grant seam the first four bugs shared, and one a future constant edit can open
without touching a single handler.

A sixth has since been fixed, and it is that same sibling: `release_permit`'s wait was
`now >= revoked_at + complaint_window`, safe only if `CLOCK_SKEW_TOLERANCE_SECONDS <=
complaint_window` for whatever permit is being released. A compile-time assertion already
pinned `CLOCK_SKEW_TOLERANCE_SECONDS <= MIN_COMPLAINT_WINDOW_SECONDS`, but that assertion
compares two constants, while `grant_permit` copies `complaint_window` off the live
`Marketplace` with no re-validation against today's bounds (grandfathering, decision 10) —
so a marketplace registered before a future increase to `MIN_COMPLAINT_WINDOW_SECONDS` could
still be granting permits with a window below the new floor, at which point the assertion
would keep passing (it never reads the stored field) while `release_permit`'s actual wait
fell under the tolerance. Latent rather than live, exactly like the fifth: it needed a future
constant edit to arm, not just today's handlers. Fixed by flooring at the point of use —
`release_permit` now waits `revoked_at + complaint_window.max(CLOCK_SKEW_TOLERANCE_SECONDS)`
(`earliest_release`, `instructions/release_permit.rs`) — the same shape as the fifth bug's
fix. This was the third and last of three grandfathering-sensitive constant uses found this
arc: the bond clamp (`raise_dispute` takes `min(permit.bond_bps, MAX_BOND_BPS)`) and
`closable_after` were fixed earlier, this release-wait floor closes the third. All three
stored fields a protocol constant is compared against are now bounded at the point of use
rather than trusted to already sit inside the constant's range — that family is closed.

## Deliberate simplifications, as of now

- **No independent arbitration; each marketplace's own arbiter judges its own disputes**, up to the seller's permit cap. The arbiter is checked only as "is this the expected signer," never assumed to be a wallet, so a future jury program's PDA can occupy the slot with no change to this program; because terms are frozen per permit, that migration can happen one seller at a time.
- **No buyer reputation stored onchain.** A buyer identity costs nothing to abandon; buyer history is computable from `DisputeRaised`/`DisputeResolved`/`DisputeExpired` events instead of read from account state.
- **No identity or KYC data, anywhere in the design.** Everything written to Solana is public and permanent; verification stays inside a marketplace's own systems.
- **No leverage multiplier.** Backing above collateral is only safe once reputation is expensive to fabricate, which it currently is not.
- **A permit caps total damage across every buyer on one marketplace, not per-buyer coverage**; keeping order volume in line with a seller's live permit is left to the marketplace, published as an integration requirement.
- **No appeals, no partial refunds, no protocol fee, no frontend.**

See `docs/DESIGN-v2.md`, "What this design deliberately does not do" and "Honest limitations, stated rather than papered over," for the full list and the reasoning behind each; those two sections and this one and README.md's "Current tradeoffs" go stale together and get updated together.

## Working in this repo

- **Program logic:** `programs/truststake/src/` — `lib.rs` is the entrypoint/index (19 `pub fn`
  handlers, counted directly off it), `state.rs` now just re-exports `state/`, which defines
  five accounts (`config.rs`, `dispute_record.rs`, `marketplace.rs`, `seller_stake.rs`,
  `slash_permit.rs` — `ls programs/truststake/src/state/`), `instructions/` has the nineteen
  handlers.
- **Unit tests:** `programs/truststake/tests/` (`ls` — there is no `test_truststake.rs`) holds
  four LiteSVM integration suites plus a shared harness: `test_phase1.rs` (32 tests) covers the
  nine foundation handlers — config and marketplace setup, authority transfer, staking;
  `test_phase2.rs` (33 tests) covers the six collateral-lifecycle handlers —
  `withdraw_stake`, `grant_permit`, `increase_permit`, `revoke_permit`, `release_permit`,
  `release_permit_early`; `test_phase3.rs` (71 tests) covers the four dispute handlers —
  `raise_dispute`, `resolve_dispute`, `expire_dispute`, `close_dispute` — including the
  Ed25519/introspection attack-probe section; `test_devnet_demo_parity.rs` (2 tests) replays
  `examples/devnet_demo.rs`'s instruction sequence against LiteSVM before it spends real devnet
  SOL — one against a blank chain, one against a chain pre-seeded with the original eight-step
  walk's own state, since a blank-chain-only replay cannot see a bug that only exists once a
  slash has moved `staked` and `committed` together (see "Known gotchas" below); `common/mod.rs`
  is the shared `World` test harness both use, not a test file itself. Run with
  `OPENSSL_NO_VENDOR=1 cargo test` from `programs/truststake/`.
- **Real devnet demo:** `programs/truststake/examples/devnet_demo.rs` — exercises all 19 handlers
  (`lib.rs`) across three marketplaces and two sellers, in four wall-clock-gated stages, since
  three of the protocol's waits (`release_permit_early`'s clock-skew tolerance, `release_permit`'s
  complaint window, `expire_dispute`/`close_dispute`'s 30-day marks) are real time on devnet and
  cannot be skipped. Stage 1 runs immediately (config/marketplace authority transfers, a third
  marketplace SwiftMarket registered at 0 bps bond, a PixelBazaar bond-rate change, a CashDesk
  receipt-signer rotation, `increase_permit`, two more disputes — one left open, one raised
  pre-rotation and rejected — and a second seller who stakes, grants, and revokes two permits);
  stage 2 (`release_permit_early`) is due 1 hour later; stage 3 (`release_permit`) 2 days later;
  stage 4 (`expire_dispute`, `close_dispute` ×2) 30 days later. Gated behind the `devnet_demo`
  Cargo feature; run with `cargo run --example devnet_demo --features devnet_demo` from
  `programs/truststake/` — the command never changes, an optional `--stage N` (1-4) forces just
  one stage, and by default every stage currently due runs, printing what remains and exiting 0
  (not an error) for whichever isn't due yet. The signing wallet must match
  `constants::INITIAL_ADMIN`. Stage progress, and chain facts a fresh invocation cannot re-derive
  on its own (the two original marketplaces' onchain IDs, which were never persisted before this
  revision and are random rather than the fixed strings an earlier task briefing assumed; new
  disputes' `order_id`s; revocation timestamps), live in
  `programs/truststake/examples/.devnet-demo-state/progress.json` (gitignored, alongside the
  existing `.devnet-demo-keypairs/`). Every step, old and new, checks live chain state before
  acting — including the original eight steps, which turned out not to be as idempotent as they
  looked: `initialize_stake`/`grant_permit`/`register_marketplace` all use Anchor's `init`, which
  hard-errors on a second call, and `raise_dispute`'s `order_id` was fresh-random every run, so a
  naive re-run of the pre-existing script would have hard-failed or silently raised duplicate
  disputes the moment this task's stage design required running it more than once.
  `tests/test_devnet_demo_parity.rs` proves the entire four-stage sequence against LiteSVM first
  — including the boundary of every wait, both directions — before any of it spends devnet SOL.
  Parity coverage never exercises a guard, though — it replays the instruction sequence, not the
  script's own decision code — so every guard that decides "has this step already run" was, until
  the fix recorded in `docs/DEMO-SCRIPT-FINDINGS.md`, untested and prone to the same species of
  bug the program itself had already fixed in `SlashPermit.granted_at`: inferring "already done"
  from live chain state that a *later* step of the same script can destroy or decrease. Those
  guard decisions are now pure functions with their own `#[cfg(test)] mod tests` inside
  `devnet_demo.rs`, run via the command in "Build and test" above.
- **Build:** `anchor build` (not plain `cargo build` — Solana programs need the SBF target, which `anchor build` invokes via `cargo build-sbf`). `target/deploy/truststake-keypair.json`
  (pubkey `G8B59KZpf7siepeb3PRX3KuPk4LC1LZarF8YmuPHGUZ`, confirmed with `solana-keygen pubkey`)
  does not match `declare_id!` in `src/lib.rs` or either `[programs.*]` entry in `Anchor.toml`
  (both `3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2`, the deployed program). That keypair is a
  gitignored build artifact Anchor generated on some earlier build; the deployed program's own
  keypair was never kept locally. `anchor build` (verified on anchor-cli 1.1.2) prints `Program
  ID mismatch detected` and points at `anchor keys sync` and `--ignore-keys`, but the mismatch
  is a warning, not a build failure — the build still exits 0 and produces the `.so` either way,
  with or without `--ignore-keys`. **Never run `anchor keys sync`** — it resolves the mismatch
  the wrong way, rewriting `declare_id!` in source to match the throwaway keypair. Every PDA in
  this program derives from the program ID, so the rebuilt binary would address an entirely
  different set of accounts, orphaning the deployed program and all its devnet state, and
  invalidating the published IDL.
- **Deploy:** never plain `anchor deploy` — confirmed via `anchor deploy --help`, it defaults to
  `--program-keypair target/deploy/truststake-keypair.json`, so it would deploy a brand-new
  program at `G8B59KZpf7siepeb3PRX3KuPk4LC1LZarF8YmuPHGUZ` and spend devnet SOL, rather than
  upgrading the deployed one. An upgrade goes through `solana program deploy --program-id
  3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2 target/deploy/truststake.so`, authorised by
  `--upgrade-authority` (default: the configured keypair, `~/.config/solana/id.json`, confirmed
  with `solana config get`) — confirmed via `solana program deploy --help`, `--program-id` "can
  be an address for upgrades," and the program keypair is not needed for it. Either way, this
  costs real devnet SOL (program rent-exemption is roughly 6,960 lamports per byte of the
  compiled `.so`). Check `solana balance --url devnet` first. If a deploy fails partway, check
  `solana program show --buffers --url devnet` before retrying; there may be a paid-for buffer
  account worth resuming from (`solana program deploy --buffer <address> ...`) instead of paying
  rent again from scratch.
- **IDL, after any future deploy:** run `anchor idl upgrade
  3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2 -f target/idl/truststake.json
  --provider.cluster devnet`, never `anchor idl init`. The IDL account already exists (created
  2026-08-24), and `init` fails against an account that is already allocated. A stale IDL is
  worse than no IDL: Anchor numbers error codes positionally as 6000 + index and publishes
  that numbering in the IDL, so an outdated one mis-names every error and mis-decodes any
  instruction whose arguments changed.

  **This debt is still outstanding, and the upgrade above currently does not work.** It was
  attempted three times on 2026-08-31 alongside that day's program deploy, twice plainly and
  once with `--priority-fee 200000`, and failed identically every time with `[Error] The
  provided transaction plan failed to execute` / `Error: Failed to upgrade IDL`. That is *not*
  the "reports failure after succeeding" trap recorded under "Known gotchas" — it was checked
  onchain rather than believed, and the upload genuinely did not land. What actually happens:
  Anchor 1.1.2 stores the IDL in a Program Metadata program account
  (`ProgM6JCCvbYkfKqJYHePx4xxSUSqJp7rh8Lyv7nk7S`, address
  `A1r5sAi41UnpGBL9Xo2gk1T12LzCDVo3d79MEsM4xvkc`, seed `idl`), and `upgrade` is an alias for
  writing a buffer and then setting it. Each attempt allocated an 8,845-byte buffer and left it
  **partially written** — 6,532, then 7,495, then 7,495 non-zero bytes, so the zlib payload at
  offset 96 will not decompress — while every transaction that did land returned `err: None`.
  Non-deterministic truncation with no failing transaction points at write transactions being
  silently dropped by the public devnet RPC, which a priority fee did not fix. Three abandoned
  buffers now hold 0.06245208 SOL each (`6mkxvZp8SnBAfBg7iq3WQizT35QQJCZjpyyGFerkrWnm`,
  `CHTWvALh4J7Wjr4GkrQihhu5KyRnsu6H7QGLLKNzCQRL`,
  `H1P5eHX9a336bAWTtJ9HGxLwQU6q9sqVA8Geh9jRFcCN`). **Do not run `anchor idl close` to reclaim
  them** — it takes a program ID and a seed, so it targets the live IDL metadata account, not a
  buffer, and would destroy the published IDL. Worth trying next: a non-public devnet RPC
  endpoint via `--provider.cluster <url>`, or `anchor idl create-buffer` / `write-buffer
  --buffer <addr>` as two explicit steps so a partly-written buffer can be resumed instead of
  re-allocated.

  Meanwhile the published IDL is the 2026-08-24 one and is **safe to keep using**, verified by
  fetching and decompressing it and diffing against `target/idl/truststake.json`: all 19
  instructions present with identical names, arguments and discriminators; identical account
  list; every shared error code 6000-6037 identical. It differs in exactly two harmless ways —
  it lacks names for the two appended variants `AccountVersionMismatch` (6038) and
  `UnsupportedMintExtension` (6039), `TrustStakeError` having grown from 38 to 40; and one doc
  string on `DisputeRecord.closable_after` still describes the pre-fix formula. Nothing
  decodes wrongly, because both error variants were appended rather than inserted and no field
  order or type moved.
- **Before changing any protocol constant or any field on a stored account, search the
  entire project for every site that reads it, and report those sites before making the
  change.** `ACCOUNT_VERSION` was written at five call sites and read at zero for the
  project's entire history until this task gave it a constraint; a thirty-second search would
  have shown that at any point, and nobody ran one.
- **Every count or file path written into this file must be verified by running a command at
  the moment of writing it — never by reasoning, reading, or copying a figure from an earlier
  version of this file or from a prior conversation.** This file has carried a stale count on
  four separate occasions, each found by accident during unrelated work: `test_truststake.rs`
  named here after `tests/` had already moved to `test_phase1.rs`/`test_phase2.rs`/`test_phase3.rs`;
  a `checked_*` call-site count and an `.rs`-file count both left uncorrected after the code
  they counted grew; and "all four cross-phase bugs" left standing after this same file's own
  "Review history on v2" section had already recorded a fifth. A task that changes the test
  count, the handler count, the account count, or the "Honest limitations" entry count in
  `docs/DESIGN-v2.md` must update this file in the same session — not defer it, since deferred
  is how the previous four went stale.

## Solana MCP server

`.mcp.json` (commit 4d9a311) wires up an http MCP server named `solana` at
`https://mcp.solana.com/mcp`, no auth. Five tools, all exercised against this repo on
2026-08-27 rather than taken from their descriptions. What follows is the result of that,
not a restatement of the catalogue.

**Reach for it whenever** a question touches an Anchor API, a Solana runtime behaviour, a
precompile or sysvar layout, or an SPL instruction — before answering from model memory.
"Known gotchas," below, records that Anchor 1.0.x broke compatibility with 0.32 and that
older tutorials are stale; this server is the fix for exactly that. It also indexes
production program source, not just prose, which makes differential review possible: compare
a mechanism here against how a shipped program solves it and investigate every divergence.

- **`get_documentation`** — the most useful of the five. Required `section`: a source id or a
  section id, string or array. Returns a whole corpus, so pull only what you need (50KB per
  source, 200KB total).
- **`Solana_Documentation_Search`** — required `query`. Semantic RAG, returns ranked chunks.
  Cheap. Use for a narrow question or a specific error.
- **`list_sections`** — no args, ~33KB. The catalogue. Only needed when hunting a source id
  the list below doesn't already name.
- **`Solana_Expert__Ask_For_Help`** — **redundant, don't use it.** The identical query string
  through it and `Solana_Documentation_Search` returned the same twenty sources in the same
  order, differing only in the third decimal of the similarity scores. It is the same
  retrieval backend under a second name, and despite what the name suggests it synthesises no
  answer — it returns raw chunks like the search tool does. One tool, use the search one.
- **`program_autofixer`** — marginal here, and not part of any security argument. See below.

### Source ids that matter for this repo

Saves a 33KB `list_sections` call. All verified present.

- `gh-sealevel-attacks` — the canonical taxonomy of Solana-specific exploit classes, each with
  insecure/secure/recommended variants. **Our own cross-phase bugs fall inside it:** #1 and #3
  are class 9 (closing accounts — revival after close), #2 is class 8 (PDA sharing — a seed
  that doesn't name every identity it guards). Read this before any security pass; it names
  the shapes we have been finding by hand.
- `anchor-docs`, `gh-anchor` — Anchor 1.x canonical. Account constraints, and the constraint
  *execution order* (`close` runs in the exit handler, after the handler body).
- `gh-solana-sdk` — `ed25519-program` layout, sysvar helpers, the secp256k1 module's security
  notes, which spell out the introspection checks a verifier must make.
- `gh-simd` — SIMD-0152 is the precompile specification: `num_signatures`, the three
  `*_instruction_index` fields, `0xFFFF` meaning "this instruction."
- `gh-anchor-instruction-sysvar`, `gh-solana-ed25519-instruction`,
  `gh-solana-transaction-introspection`, `gh-anchor-escrow-introspection` — four independent
  takes on our exact receipt-verification pattern.
- `gh-drift-protocol-v2` — a production ed25519 verifier (`sig_verification.rs`) to diff
  `src/ed25519.rs` against.
- `gh-litesvm` — our test harness. `gh-spl-token`, `gh-spl-token-2022` — our collateral.
  `gh-idl-program` — onchain IDL storage, relevant to the IDL rule above.

`src/ed25519.rs` was checked against SIMD-0152 and the SDK's security notes this way on
2026-08-27: every attack vector those describe is closed. That is the first time our most
exploitable surface was validated against the specification rather than against reviewers'
reasoning. Re-run that comparison if the module changes.

### `program_autofixer`, and why it is not evidence

Required `code` (one file or concatenated modules as a string); optional `filename`,
`framework`, `dismissed`. A per-file pass over all 33 `.rs` files here (`find
programs/truststake/src -name '*.rs' | wc -l`) returned zero issues, independently
corroborated: 34 `checked_*` call sites (`grep -rn checked_ programs/truststake/src/ | wc -l`)
and no raw balance arithmetic; three `UncheckedAccount` fields all carrying `/// CHECK:`; every
`init` either given `space` or a token account under the rule's own exception.

It is a single-file static linter over a closed ruleset — no cross-file dataflow, no
instruction-ordering model, no view of the state machine. A control run on deliberately
broken code measured the ceiling: a handler with **no authorization check at all**, letting
any caller reassign admin and drain the vault, was reported only as `low — AccountInfo opts
out of typed validation`, while a missing `space` attribute was rated `high`. It checks
shapes, not authority, ordering, or state. All five cross-phase bugs (see "Review history on
v2," which documents a fifth beyond the four originally found here) are strictly harder than
the one it missed.

So: optional shape check before committing program changes, nothing more. A clean run is
never evidence in a cross-phase discussion and never shortens one. There is no dismissal
ledger — nothing has ever been flagged to dismiss, and one would only invite treating its
silence as assurance.

**Third-party disclosure:** `program_autofixer` transmits the code passed to it to a
third-party service. Acceptable here only because `baigi-bawander/trust_stake` is public and
MIT-licensed. A private fork must not call it unmodified.

## Known gotchas hit while building this

- **Anchor 1.0.x is a recent major version** with breaking changes from 0.32 (`CpiContext::new` takes a `Pubkey` now, not an `AccountInfo`; IDL handling changed). If you're referencing older Anchor examples/tutorials, expect some to be stale.
- **A wallet that ends a transaction with a nonzero balance must stay above the rent-exempt minimum for a bare account**, or the transaction is rejected in preflight. This bit the devnet demo script the first time: funding a throwaway wallet with *exactly* what it spends leaves it at a small nonzero remainder below that floor. Fund with an extra `get_minimum_balance_for_rent_exemption(0)` worth of headroom for any wallet that isn't being fully drained to zero.
- **`anchor build` can fail inside `programs/truststake/build.rs`'s staleness guard, and
  deleting the artifacts is the wrong fix.** The symptom is a panic that
  `target/deploy/cpi_wrapper.so is older than .../Cargo.lock`, ending in `Error: Building IDL
  failed`. It reads like a stale artifact; it is not. `anchor build` processes workspace
  members strictly one at a time and completely, running this crate's *host*-toolchain IDL
  pass (`TARGET=x86_64-unknown-linux-gnu`, `CARGO_FEATURE_IDL_BUILD=1`) after `truststake` but
  before `cpi_wrapper` is built at all — so a guard that demands a fresh `cpi_wrapper.so`
  fires during the very command that would produce it. Two things compounded it: the test
  harness used `include_bytes!` on that sibling artifact, so a clean `target/deploy/` broke
  `anchor build` outright even with the guard removed; and because cargo's freshness is
  content-based and `cargo build-sbf` re-copies into `target/deploy/` only when the artifact
  actually changed, a `Cargo.lock` whose *mtime* moved for any reason (a re-resolve, a
  checkout, an editor save) permanently outran a `.so` cargo correctly declined to rebuild —
  making the guard's own advice, "run `anchor build`", unsatisfiable. A previous session
  escaped by deleting the `.so` files, which only reset the mtimes; the failure returned the
  next time anything wrote `Cargo.lock`. Fixed on 2026-08-31 (commit `08d8322`): the guard
  skips the IDL pass, `tests/common/mod.rs` reads both artifacts at run time instead of
  embedding them, and `Cargo.lock` plus both manifests are compared by content against copies
  in `target/deploy/.build-guard/` taken at the last SBF build. If you see this error again,
  read `build.rs`'s module docs before touching anything — do not delete artifacts.
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
- **A devnet_demo.rs step whose amounts are hardcoded against a fresh-chain assumption breaks
  the instant a slash has ever touched the seller, and a blank-chain-only parity test cannot
  catch it.** `SellerStake::slash` reduces `staked` and `committed` TOGETHER, so a step written
  against "committed is always 400" (the value on a fresh chain, after two 200 permits) goes
  wrong wherever the difference `staked - committed` (free collateral) is what's actually being
  compared against, not either figure alone. This hit real devnet on 2026-08-31: Step 6's
  withdrawal amounts assumed committed = 400, but an $80 upheld dispute in the 2026-08-24 run
  had already reduced it to 320, so a withdrawal sized to land exactly at the (stale) cap
  landed comfortably inside the real one instead, and the "this must fail" half of the
  demonstration silently succeeded — proving nothing, not erroring loudly. `tests/test_devnet_demo_parity.rs`'s
  original test replayed the same sequence against a BLANK LiteSVM chain, where the fresh-chain
  assumption is always true by construction, so it kept passing throughout. The fix has two
  parts, both required: (1) derive the amount from live state immediately before the step —
  free collateral for a "must succeed" step, `free + 1` for a "must fail" one — never from a
  constant, the same pattern `stage1_6_top_up_seller_free_collateral` already used correctly
  before this bug was found; and (2) add a SECOND parity test that pre-seeds LiteSVM with the
  original eight-step walk's own state before replaying the sequence on top of it
  (`test_devnet_demo_resumed_after_prior_slash`), since only a chain that already carries a
  prior slash can exercise this class of bug at all. A future step written against "amount X is
  always safe/unsafe relative to committed" should be treated as suspect until proven to read
  `staked`/`committed` live at the point it runs.
