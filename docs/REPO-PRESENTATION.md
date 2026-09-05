# How this repo presents itself, measured against shipped Solana programs

Research done 2026-09-01. Every figure here was measured by command at the moment of
writing, not recalled. The comparison repos were fetched live via the GitHub API, not
described from memory.

This file is the source of truth for **presentation and packaging**. It is not a second
copy of the design rationale (`DESIGN-v2.md`), the deployment ledger (`CLAUDE.md`), or the
test record (`TESTING.md`) — where those already own a fact, this file points at them
rather than restating it.

## What was compared

Four mainnet, audited, single-purpose Solana programs, chosen because each is the closest
available analogue to TrustStake's shape:

| Repo | What it is |
|---|---|
| `Squads-Protocol/v4` | Multisig. AGPL-3.0. 196 stars. |
| `Ellipsis-Labs/phoenix-v1` | Orderbook. MIT. 278 stars. |
| `drift-labs/protocol-v2` | Perps DEX. Apache-2.0. |
| `marinade-finance/liquid-staking-program` | Liquid staking. |

## Finding 1: the README is a signpost, not a manual

| Repo | README |
|---|---|
| Marinade | ~35 lines |
| Phoenix | ~40 lines |
| Drift | ~90 lines, mostly build commands |
| Squads | ~180 lines (the outlier) |
| **TrustStake** | **200 lines** |

Phoenix's entire README is: one sentence of what it is, a link to the docs site, a link to
the audit PDF, a link to the bug bounty, the command to verify the deployed program against
the source, and two build commands. No problem statement, no market, no rationale — those
live on an external docs site the README links to.

The lesson is layering, not deletion. TrustStake's depth is a genuine asset; it is just all
presented at one level, so a visitor meets 200 lines of README with six more documents
behind it and no signposting about which one they want.

## Finding 2: five conventions TrustStake does not follow

1. **`SECURITY.md` at the repo root.** Three of the four have it. GitHub gives this file a
   dedicated Security tab and surfaces it when someone opens an issue. TrustStake's
   `docs/SECURITY-CONTEXT.md` is invisible to that mechanism. Phoenix's is a bug-bounty
   severity table; Drift's points at `bug-bounty/README.md`.
2. **An `audits/` directory, or a minimal `AUDIT.md`.** Drift's `AUDIT.md` is 214 bytes —
   two markdown links to PDFs, nothing else. TrustStake has no audits, which is correct for
   a prototype, but the convention is where readers look first.
3. **A reproducible build claim.** Phoenix publishes a single command anyone can run:
   `solana-verify verify-from-repo -um --program-id <ID> <repo url>`. Squads documents the
   longer `solana-verify build` / `get-executable-hash` / `get-program-hash` form. Both
   turn "trust the author's notes" into "check it yourself." TrustStake has done the
   equivalent by hand (SHA-256 `101a10d8...` matched between `solana program dump` and the
   local `.so`, recorded in `CLAUDE.md`) but publishes no way for a reader to repeat it.
   Both Phoenix and Drift also commit a `.verified-build.json`.
4. **A deployment ledger in the README.** Marinade's README is a dated list: version, what
   changed, the commit hash, a link to the deploy transaction, and which audits covered it.
   TrustStake keeps exactly this in `CLAUDE.md`, addressed to future AI sessions rather
   than to the public.
5. **Releases and tags.** All four tag versions. TrustStake has none, so no visitor can
   tell which commit is the binary currently on devnet.

## Finding 3: the real problem is publishing, not writing

Measured 2026-09-01:

- `origin/main` last commit: `99479c0`, **2026-08-25**
- Local `main` is **8 commits ahead**

Everything since 25 August exists only on one laptop: the six cross-phase bug fixes, the
v2 devnet upgrade at slot 491019091, the four-stage demo, the `build.rs` guard fix. A
visitor to the GitHub repo today reads a week-old prototype with no sign any of it
happened.

**Nothing else in this file matters as much as pushing.**

## Finding 4: the prose-to-code ratio

| | Lines |
|---|---|
| Program source (`programs/truststake/src/**/*.rs`) | 3,608 |
| Tests (`programs/truststake/tests/**/*.rs`) | 8,083 |
| Documentation (`README.md`, `CLAUDE.md`, `docs/*.md`) | 2,836 |

Roughly one line of prose per line of program. `DESIGN-v2.md` alone is 1,018 lines and
carries 19 honest-limitation entries — more self-disclosure than any of the four
comparison repos publish. That is a strength being presented as a wall.

`CLAUDE.md` (424 lines, addressed to AI assistants) sitting in the repo root is also
unusual for a human visitor to encounter first.

## Finding 5: GitHub page metadata

Fetched live from the API on 2026-09-01 for `baigi-bawander/trust_stake`:

- Description — set, accurate. Good.
- Topics — `solana`, `anchor`, `rust`, `smart-contracts`. Good.
- Homepage/website field — **empty**. Should point at the dashboard once hosted.
- Releases — **none**.
- Wiki and Projects — enabled but empty. Empty tabs read as abandoned; turn them off.
- Issues: 0 open, 0 stars, 0 forks.

## What to actually do, in priority order

1. **Push.** Everything else is cosmetic next to this. **Done 2026-09-04** (`c1194d4`,
   CI run #11 green).
2. **`solana-verify`** — publish the verify command in the README, and commit a
   `.verified-build.json`. Highest credibility per unit of effort, and it works on devnet.
   Note: this must be done without disturbing the `declare_id!` / keypair situation
   documented in `CLAUDE.md` ("Never run `anchor keys sync`").
3. **Move `docs/SECURITY-CONTEXT.md` to a root `SECURITY.md`**, or add a root `SECURITY.md`
   that links to it, so GitHub's Security tab appears.
4. **Shorten the README to a signpost** and move the problem statement, rationale, and
   account/instruction reference behind links. Add the Marinade-style deployment ledger,
   sourced from `CLAUDE.md`'s "Current state" section.
5. **Tag a release** matching the binary currently on devnet.
6. **Set the homepage field**; disable the empty Wiki and Projects tabs.

## Where the open work is tracked (pointers, not copies)

- **Deliberate deferrals** — `DESIGN-v2.md`, end of "Risks and open items": frontend, SAS
  attestation mirroring, appeals, partial refunds, leverage mode, stored buyer reputation,
  `export_reputation`, marketplace registration bonds, marketplace-signed sales-volume
  summaries. Two worth pulling forward before anyone integrates: `export_reputation` (so
  another program can read a seller's record without coupling to this program's exact
  Anchor and Borsh versions) and an offchain indexer (there is no enumeration query path).
- **Design tradeoffs** — `DESIGN-v2.md`, "Honest limitations" (19 entries) and "What this
  design deliberately does not do"; mirrored in `README.md`'s "Current tradeoffs" and
  `CLAUDE.md`'s "Deliberate simplifications". Those four sections go stale together.
- **Deployment and IDL state** — `CLAUDE.md`, "Current state" and the IDL debt entry.
- **Devnet demo stage schedule** — `examples/.devnet-demo-state/progress.json` (gitignored)
  and `CLAUDE.md`'s devnet demo entry.

There are **zero** `TODO`, `FIXME`, or `unimplemented!` markers anywhere in
`programs/truststake/src/` (verified by grep, 2026-09-01). Nothing in the program is
half-built.

## Chores not recorded anywhere else

- The two v1-era leftover accounts on devnet (`BTz2X7iCpXCSLSEWQRyWM31v5rQ5QqJnwJi9b9LmzRSJ`,
  41 bytes, v1 `Config`; `FXgrh8osZawRur4FqGBiTb9xQspRyNTy7K9PXrQp5ojr`, 53 bytes, v1
  `SellerStake`) hold ~0.0074 SOL and can never be closed — the v1 code that owned them was
  overwritten by the in-place upgrade. As of 2026-09-05 they make `getProgramAccounts` return
  **15** total, where the five v2 account types (1 `Config`, 3 `Marketplace`, 2
  `SellerStake`, 4 `SlashPermit`, 3 `DisputeRecord`) account for 13 — both figures grown from
  this file's 2026-09-01 research as the four-stage demo has run; see the audit section's A7.
  Parked deliberately; sweeping the two leftovers would mean shipping a handler purely to
  reclaim a fraction of a cent of devnet SOL.
- ~~`CLAUDE.md` describes the fifth cross-phase bug (`closable_after`) backwards.~~
  **RESOLVED 2026-09-05, see the audit section's A5.** `DESIGN-v2.md` repeated the same
  error and was fixed alongside it.
- The dashboard's frozen fallback snapshot carries pre-stage-2 figures and one invented
  dispute amount. Best refreshed after stage 3 lands rather than twice.
- The dashboard is not under version control.

## The one item that is more than a chore

**The upgrade authority is a single ordinary keypair.** `EE4skmuEcaL4ybktFhp7sUfr84to78KQKoNsAAu8L7jG`
is simultaneously the program's upgrade authority, the hardcoded `INITIAL_ADMIN`, and the
devnet test mint's authority — one key, three roles, no multisig. `DESIGN-v2.md` names
moving both the protocol authority and the upgrade authority to a Squads multisig as a
known future step. Reasonable for a devnet capstone, and the README correctly claims
nothing to the contrary, but it is the largest single gap between this and something
holding other people's money.

---

# Documentation accuracy audit

Added 2026-09-03 (A1-A6). A read of all seven `.md` files in the repo, with every numeric
claim re-measured by command rather than compared against an earlier version of itself.
Listed worst-first. A second sweep on 2026-09-05 found six more (A7-A12) plus a duplicate of
A4 (A13), all from facts that had moved since the first sweep rather than from anything the
first sweep missed. **These were findings for a dedicated documentation session; that session
happened 2026-09-05, and all thirteen are now resolved -- see "Checked and accurate" below.**

## A1 — `README.md` publishes a deploy command this project forbids

`README.md`'s Deploy section instructs:

```bash
solana config set --url devnet
anchor deploy
```

`CLAUDE.md` says never to run that: `anchor deploy` defaults to `--program-keypair
target/deploy/truststake-keypair.json`, so it deploys a **brand-new program** at
`G8B59KZpf7siepeb3PRX3KuPk4LC1LZarF8YmuPHGUZ`, spends devnet SOL, and touches nothing that is
actually deployed. The correct form is `solana program deploy --program-id
3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2 target/deploy/truststake.so`.

This is the only finding here that actively misleads a reader into doing the wrong thing, and
the README is the public face of the repo. Fix first.

**RESOLVED 2026-09-05.** README's Deploy section now gives the `solana program deploy
--program-id ...` form and explains why `anchor deploy` is wrong, carrying across the
`solana program show --buffers` retry advice already in `CLAUDE.md`.

## A2 — Three documents carry three different test counts, none confirmed

| Source | Claim |
|---|---|
| `README.md` (two places) | 132 |
| `docs/SECURITY-CONTEXT.md` | 132 |
| `CLAUDE.md` | 152 |
| `grep -rh '#\[test\]' programs/ --include=*.rs \| wc -l` (2026-09-03) | 151 |
| **`OPENSSL_NO_VENDOR=1 cargo test`, run 2026-09-03** | **152 — authoritative** |

**RESOLVED: `CLAUDE.md`'s 152 is correct; do not "fix" it.** The grep undercounts by
exactly one because Anchor's `declare_id!` macro generates a `test_id` test that has no
`#[test]` attribute in source. Cargo reports 14 lib + 2 parity + 32 phase1 + 33 phase2 +
71 phase3 = 152. Only `README.md` and `docs/SECURITY-CONTEXT.md` need correcting, from 132
to 152. A separate 28-test suite covers the demo script's guards and is counted apart:
`OPENSSL_NO_VENDOR=1 cargo test --example devnet_demo --features devnet_demo`.

**Both corrected 2026-09-05**, re-verified against a fresh `cargo test` run (152) and the
guard suite (28); both documents now also mention the separate guard suite.

## A3 — `docs/SECURITY-CONTEXT.md` undercounts the checked-arithmetic sites

Line 187 claims 29 `checked_*` call sites. `grep -rn checked_ programs/truststake/src/ | wc -l`
returns **34** (2026-09-03). `docs/IMPLEMENTATION-FINDINGS.md` also says 29, but it is
explicitly a dated Phase 3 snapshot, so it is stale-by-design rather than wrong.

**RESOLVED 2026-09-05**, re-verified at 34. The same pass also caught and fixed two adjacent
stale figures on the same lines: the file's line-count claim (9,197 lines/39 files, now
15,209/41 for all of `programs/`) and its error-variant count (38, now 40 — two variants,
`AccountVersionMismatch` and `UnsupportedMintExtension`, were appended since it was written;
both are asserted by name in `tests/`, confirmed by grep).

## A4 — A recommended fix from the project's own findings was never applied

`docs/IMPLEMENTATION-FINDINGS.md` TS-33 (Low) records that sellers are told an undecided
complaint costs them 30 days, when the true worst case is `complaint_window +
DISPUTE_EXPIRY` — up to roughly 60 days against a 30-day-window marketplace. Its recommended
fix is to state that bound wherever the 30-day figure is quoted to sellers. A grep for
`complaint_window + DISPUTE_EXPIRY`, `60 days` and `window + 30` across `DESIGN-v2.md` and
`README.md` returns nothing, so it was never carried out.

**Partially already true, rest resolved 2026-09-05.** `DESIGN-v2.md` decision 7 already
states the compounded bound in prose ("the complaint window plus 30 days, up to 60") — the
grep above missed it only because that sentence doesn't end in the literal word "days" after
"60". Decision 7 is a recorded historical decision and was left as-is. `README.md`'s
Instructions section, which flatly said `expire_dispute` "frees the seller after 30 days"
with no caveat, is the one place the fix was genuinely missing; it now states the same
compounded bound.

## A5 — `CLAUDE.md` describes the fifth cross-phase bug backwards

`CLAUDE.md` says `closable_after` "was derived from the *live* permit's own
`complaint_window` rather than the protocol ceiling."
`git show 34c5483:programs/truststake/src/instructions/raise_dispute.rs` shows the pre-fix
code used `MAX_COMPLAINT_WINDOW_SECONDS` flat. Confirmed on chain: the 2026-08-24 dispute's
stored `closable_after` is ~30 days after issuance, not one CashDesk complaint window.
Check whether `DESIGN-v2.md` repeats the same error.

**RESOLVED 2026-09-05.** `DESIGN-v2.md` did repeat it, in the grandfathering entry under
"Honest limitations" (and in the account-model state table's formula for `closable_after`).
Both fixed alongside `CLAUDE.md`, all three now stating the pre-fix behaviour as "the
protocol ceiling `MAX_COMPLAINT_WINDOW_SECONDS` flat, rather than the live permit's own
(possibly wider, grandfathered) `complaint_window`" and the fix as `max(permit.complaint_window,
MAX_COMPLAINT_WINDOW_SECONDS)`.

## A6 — `README.md`'s IDL section does not mention the upload is blocked

It gives `anchor idl upgrade` as a routine post-deploy step. `CLAUDE.md` records that this
has failed on every attempt since 2026-08-31 and that the published IDL is the 2026-08-24
one. A reader following the README will hit an opaque failure with no warning.

**RESOLVED 2026-09-05.** README's IDL section now warns the command has failed on every
attempt since 2026-08-31 and states that the published 2026-08-24 IDL is safe to keep using
in the meantime, and why.

## A7 — `CLAUDE.md`'s "The v2 rebuild" devnet-state description was stale

Written when the demo had produced "two marketplaces, one stake, two permits and one
resolved dispute." `getProgramAccounts` against the program ID, decoded 2026-09-05, shows 3
`Marketplace`s (CashDesk 2-day/1,000 bps, PixelBazaar 7-day/300 bps, SwiftMarket 30-day/0
bps), 2 `SellerStake`s, 4 `SlashPermit`s and 3 `DisputeRecord`s (1 upheld, 1 rejected, 1
open) — 12 accounts, plus the one live `Config` and two unreadable v1-era leftovers (41-byte
`Config`, 53-byte `SellerStake`, sizes confirmed against `git show
52ae454:programs/truststake/src/state.rs`), for **15 accounts total**. See the "Chores"
section above, corrected to match.

**RESOLVED 2026-09-05.** `CLAUDE.md` now states this inventory directly.

## A8 — `CLAUDE.md`'s "Current state" section carried no demo-progress summary

A reader had to piece together stage completion from the "Real devnet demo" bullet's prose.
Stages 1-3 are done (2026-08-31, 2026-09-01, 2026-09-03 at slot 492481349 respectively),
putting 17 of 19 handlers through a real devnet transaction; only `expire_dispute` and
`close_dispute` remain, across three `DisputeRecord`s clearing on 2026-09-23 and 2026-09-30.

**RESOLVED 2026-09-05.** Added as a **Demo** bullet in "Current state."

## A9 — `DESIGN-v2.md` line ~955 undercounted the demo marketplaces

Said "demo marketplaces at 2 and 7" days; a third (SwiftMarket, 30 days) has existed on
devnet since stage 1 (2026-08-31).

**RESOLVED 2026-09-05.** Now reads "2, 7 and 30 (the third also at the 0 bps end of the
legal bond range)."

## A10 — `DESIGN-v2.md` decision 14 and line ~1007 read as contradicted by the same fact

Decision 14 says "demo two marketplaces... three would be noise," and the later changelog
entry repeats it. Both are recorded historical judgements, not left as an unexplained
contradiction with A9's fix.

**RESOLVED 2026-09-05** — not by rewriting either, which the task rules this file follows
forbid, but by a dated note after decision 14's table, in this document's own
`*Note, 2026-08-25: ...*` style, explaining that the third marketplace was added later for
its settings (the 0 bps and 30-day bounds), not as a second attempt at the portability
argument decision 14 already won with two.

## A11 — `CLAUDE.md`'s "Real devnet demo" bullet contradicted itself

Opened with "every step, old and new, checks live chain state before acting," then, a few
sentences later in the same bullet, described the 2026-09-03 fix that replaced exactly that
approach with `progress.json` flags and destroy-proof chain facts (`docs/DEMO-SCRIPT-FINDINGS.md`).

**RESOLVED 2026-09-05.** The opening claim now says "is idempotency-guarded," consistent
with what follows; also added: `--stage N` is no longer required for safety since that fix,
only as a convenience.

## A12 — `docs/TESTING.md`'s four-stage section was empty of results

Three of four stages have run for real since it was written; every row still read `_pending_`.

**RESOLVED 2026-09-05.** Filled in with the real signatures for stages 1-3 (23 transactions,
every one cross-checked programmatically against `getTransaction` for a matching instruction
name and `err: None` before being written down), converted from the pending table to the
prose-plus-Explorer-link pattern `docs/TESTING.md` already uses at "What the devnet run
proved," since a table cell holding six transactions (step 1.11) does not render usefully.
**The heading's "11 handlers" was checked, not changed** — 19 total minus the 8 the original
walk already covered is 11, and all eleven are confirmed exercised across the four stages;
it was correct.

## A13 — TS-33's fix was genuinely missing from `README.md`, already present in `DESIGN-v2.md`

Same underlying finding as A4, filed separately in this task's second sweep before the two
were recognised as one. See A4's resolution above for what was actually true in each file.

## Checked and accurate

`docs/AUDIT-v2-FINDINGS.md` and `docs/IMPLEMENTATION-FINDINGS.md` are both correct and
properly dated. TS-31 is marked resolved and the code matches (`require_gte!` in
`add_stake.rs` and `withdraw_stake.rs`). TS-32's fix was deliberately declined and
`CLAUDE.md` records why. All thirteen findings above (A1-A13) are resolved as of 2026-09-05;
each entry's own resolution note says what changed and, where a location turned out to
already be correct, says that instead of changing it.
