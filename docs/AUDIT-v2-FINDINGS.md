# TrustStake v2 — Pre-Implementation Audit Findings

**Source reviewed:** `docs/DESIGN-v2.md` (design-only, no code written yet)
**Method:** three independent Opus agents, run blind to each other's output, each briefed with a distinct adversarial posture. Origin idea: [Superteam — reputation-based slashing](https://superteam.fun/build/ideas/reputation-based-slashing).
**Published reference (formatted, collapsible):** https://claude.ai/code/artifact/b7da1a7f-31f3-4636-bdf3-7c57d77c8071
**Totals:** 57 findings — 6 Critical, 17 High, 21 Medium, 13 Low/Info.

This file is the plain-text/markdown record of that review, meant to be readable by any future session (with or without internet access to the artifact link above). If you're picking this up cold: the design is a Solana protocol where sellers stake collateral, grant marketplaces a "permit" to slash it, and buyers dispute bad orders against that stake, with reputation tracked on-chain.

---

## The one bug all three reviewers found independently

**`open_disputes` lives on the seller's *global* stake account, not per-marketplace-permit. It blocks all withdrawal. Nothing decrements it except the offending marketplace's own arbiter calling `resolve_dispute` — and that's never required to happen. The dispute record is also never closed.**

Net effect: any marketplace (including one an attacker just registered for free) can open a one-cent dispute and never resolve it, permanently freezing that seller's *entire* collateral — including money committed to other, honest marketplaces — for about $0.50 in rent.

- Security lens (`TS-15`): a griefing/theft exploit — costs a throwaway marketplace registration + rent.
- Incentive lens (`F2`): called "the worst finding in the report" — it inverts the product's core pitch (portable shared collateral) into joint-and-several hostage-taking by the single worst marketplace a seller has ever touched.
- Feasibility lens (`Finding 11`): happens with zero malicious intent too — an arbiter who goes inactive, loses a key, or shuts down produces the identical permanent freeze.

**Fix:** scope `open_disputes` to the permit, not the global stake; add a permissionless `expire_dispute` that force-resolves (in the buyer's favor) after a deadline.

---

## Consolidated priority list

**Fix before writing a handler (account-model changes — free now, a rewrite later):**
1. Scope `open_disputes` to the permit; add permissionless dispute expiry.
2. Fix the `revoked_at == 0` sentinel — as specified, "never revoked" and "revoked at the Unix epoch" are the same value, so the release timelock (`now >= revoked_at + complaint_window`) is trivially true for every active permit. Use `Option<i64>` or an `i64::MAX` sentinel instead.
3. Seed `Marketplace` by an immutable ID, not by the authority pubkey — as specified, a compromised/lost marketplace key can never be rotated without abandoning the whole account (and its reputation history).
4. Bind every receipt field (`order_id`, `marketplace`, `seller`, `buyer`) to the actual accounts passed into the instruction, explicitly — not just to the signature.
5. Chain `resolve_dispute`'s accounts off the `DisputeRecord` via `has_one`, never off caller-supplied marketplace/permit/stake accounts — otherwise anyone can register their own marketplace and resolve/collect from someone else's dispute.
6. Freeze buyer-relevant terms (complaint window, bond rate, signer key) at grant/issuance time, not read live — otherwise a marketplace can retroactively rewrite the deal on outstanding receipts.
7. Add a `version` byte right after the account discriminator + reserved padding on every account — free now, a full migration later.

**Fix before Phase 3 (dispute-flow logic):**
8. `resolve_dispute` needs an explicit `status == Open` guard — the record never closes, so without this it's callable repeatedly and drains the shared bond pool.
9. Validate Ed25519 instruction *indices*, not just offsets, in the signature-introspection check — otherwise an attacker can get the precompile to verify their own throwaway signature while the dispute code reads the pubkey/message from a different, attacker-controlled instruction.
10. `withdraw_unstake` must gate on committed collateral, and the unstake cooldown must be ≥ the complaint window — as specified (7-day cooldown vs. 14-day window) this is a clean, no-race exit scam.
11. The daily slash cap is self-set by the marketplace it's meant to restrain, and its base is inflatable with recoverable capital — needs a protocol-level ceiling and a per-permit (not per-book) cap.

**Framing fixes (not code — the document oversells what the mechanics deliver):**
12. `max_slashable` is effectively a blank check the seller writes to the marketplace, not a bounded/verified guarantee — say so plainly.
13. Reputation (seller and marketplace) is fabricable in both directions for $1–100 via a self-owned marketplace — needs a slashable registration bond + decayed/weighted counters, not raw lifetime totals.
14. No bond rate can mathematically separate honest claims from frivolous ones, and the forfeit currently pays the seller — funding a "bond farming" incentive for a colluding seller+arbiter.

**Two things the design doc has backwards (feasibility):**
15. Compute is not the real constraint (~40–70k CU against a 203k default budget — the Ed25519 instruction only contributes 3,000, not 200,000, to that). The real ceiling is **transaction size**: the design as written leaves only ~180 bytes / ~5 accounts of headroom before deferred features even land.
16. LiteSVM already supports what the doc worries it doesn't — `with_precompiles()` exists in the pinned 0.10.0 release behind a feature flag, not a version-bump migration.

---

## Report 1 — Security audit (30 findings: `TS-01`–`TS-30`)

**Mindset:** adversarial Solana auditor (OtterSec/Neodyme posture). Assumes Anchor implementation; v1 prototype skimmed for house style (unchecked seller/buyer accounts, PDA-only binding — habits that become risks in v2's larger model).

### Critical
- **TS-01** — Ed25519 introspection checks offsets but not instruction *indices*. An attacker can craft a transaction where the "verified" signature is their own, over their own message, while `raise_dispute` reads the pubkey/message from a different instruction the attacker also controls → fully forged marketplace receipts with no key compromise. **Fix:** assert `signature_instruction_index == public_key_instruction_index == message_instruction_index == u16::MAX` (or all equal the Ed25519 instruction's own index); bounds-check every offset before slicing; assert exact message length.
- **TS-02** — `revoked_at == 0` sentinel makes the release timelock always-true for active permits → instant full withdrawal, no revoke, no wait. **Fix:** `Option<i64>` or `i64::MAX` sentinel; explicit active/revoked branches, not one uniform comparison.
- **TS-03** — `withdraw_unstake` isn't gated on `committed`, and 7-day cooldown < 14-day complaint window → clean exit scam, no race needed. **Fix:** gate on `staked - pending_unstake >= committed`; re-check at withdrawal time; cooldown ≥ longest complaint window.
- **TS-04** — No `status == Open` guard on a dispute record that's never closed → repeatable resolution, drains the shared bond vault (other buyers' deposits), lets `open_disputes` underflow past the withdrawal gate. **Fix:** `require!(status == Open)` first line; non-zero "uninitialized" discriminant; `checked_sub`.
- **TS-05** — Replay-guard PDA seed (`order_id`) may be a raw instruction argument, not verified against the signed receipt's own `order_id` field → one valid receipt replays unboundedly under different seeds. **Fix:** `require!(order_id == receipt.order_id)` and marketplace-match, first lines of the handler.
- **TS-06** — `resolve_dispute`'s authority check ("signed by `marketplace.arbiter`") only proves the signer is arbiter of *whatever marketplace account they pass in* — and registration is free. Attacker registers their own marketplace, appoints self arbiter, resolves (and redirects payout from) anyone else's real dispute. **Fix:** chain every account off the `DisputeRecord` via `has_one`, never trust caller-supplied marketplace/permit/stake accounts directly.

### High
- **TS-07** — `raise_dispute` doesn't bind `receipt.seller`/`receipt.buyer` to the accounts passed in → disputes filed against innocent sellers; anyone who observes a leaked receipt can file and collect it before the real buyer.
- **TS-08** — Daily slash cap is self-set (`update_marketplace` controls it) and its base (`total_permitted`) is inflatable with fully-recoverable sock-puppet stake.
- **TS-09** — `update_marketplace` retroactively rewrites live permit terms — infinite complaint window to hold a departed seller hostage, 100% bond rate to block filing, signer rotation to void outstanding receipts instantly.
- **TS-10** — Payout destinations in `resolve_dispute` aren't bound to `dispute.buyer`/`dispute.seller` — an arbiter can uphold a dispute while routing the money to themselves.
- **TS-11** — Permit cap is a TOCTOU race between raise and resolve — parallel sock-puppet disputes can each pass the raise-time check against the same not-yet-decremented balance.
- **TS-12** — Repeat `request_unstake` without resetting the timer (if `pending_unstake` accumulates instead of replacing) bypasses the cooldown entirely via a pre-planted dust request.
- **TS-13** — Permit re-grant as slash-evasion: watch for a pending dispute, lower `max_slashable` to already-slashed before it resolves, and the buyer's upheld claim pays zero, permanently.
- **TS-14** — A permit can release while a dispute against it is still open and unresolved.
- **TS-15** — The convergent freeze finding (see top of file).
- **TS-16** — `Marketplace` seeded by its own authority key → key rotation is structurally impossible without abandoning the account's history.
- **TS-17** — `order_id` reuse (deliberate collusion, or accidental collision from a per-seller sequence schema) permanently blocks all but the first dispute at that seed.

### Medium
- **TS-18** — Bond recomputed from live `bond_bps` instead of the amount actually deposited → a rate hike after filing lets an arbiter overdraw the shared bond vault.
- **TS-19** — No domain separation on the signed receipt message → signer-key reuse across contexts, or cross-cluster (devnet/mainnet) replay risk.
- **TS-20** — `committed`/`total_permitted` accounting after a slash or revoke is unspecified → invariant drift, cap-base inflation over time even with no attacker.
- **TS-21** — Token-2022 transfer-fee/hook risk if the mint isn't pinned to classic SPL Token; USDC's freeze authority can freeze a stake vault, blocking slashing while reputation stays clean.
- **TS-22** — No collateral reservation at raise time → a marketplace can race an honest dispute at a *different* marketplace for the same shared stake pool.
- **TS-23** — Daily-cap clamp-vs-reject behavior unspecified; clamping lets a marketplace exhaust its own cap on purpose so a real claim resolves for near-zero and gets marked "Upheld."
- **TS-24** — `initialize_config` "first caller wins" is front-runnable on a freshly deployed program ID.
- **TS-25** — Instruction introspection isn't CPI-safe — reads top-level transaction instructions only.
- **TS-26** — `issued_at` unbounded above; `issued_at + complaint_window` can overflow/panic (DoS on a specific buyer's filing).

### Low / Info
- **TS-27** — Permissionless registration lets a bad operator reset its track record under a new key (speed bump, not a full bypass — sellers must re-grant).
- **TS-28** — Signer requirements unstated on most collateral/permit instructions — most critical: `grant_permit` **must** require the seller's own signature (the entire meaning of "opt-in").
- **TS-29** — Arithmetic/rent hardening: u128 for bps math, checked subtraction on permit caps, minimum absolute bond to stop zero-cost dust disputes.
- **TS-30** — Governance hygiene: two-step authority transfer, enforce or drop unused `version` fields, upgrade authority → multisig before real value moves.

---

## Report 2 — Mechanism-design / incentives review (14 findings: `F1`–`F14`)

**Mindset:** DeFi risk-desk posture (Gauntlet-style). Code assumed bug-free; asks only whether rational actors profit by following the rules exactly as written.

**Framing note:** decisions 3 (receipts), 4 (rate limit + track record), and 8 (buyer bond) are written as if they constrain the marketplace. They don't — they constrain strangers and buyers only. The honest description: a permit is a blank check the seller writes to the marketplace, cashable whenever the marketplace chooses.

### Tier 1 — protocol-defeating, cheap
- **F1** — Every buyer protection is a live-read, un-timelocked marketplace parameter. Rotate the signer key → void every outstanding receipt. Shrink the complaint window → kill pending claims *and* instantly mature revoked permits for early release. Spike the bond rate reactively → price a buyer out of filing. Same mutability lets a stolen key max the daily cap and drain.
- **F2** — The convergent freeze finding (see top of file).
- **F3** — Marketplace self-deals any permit in two transactions: sign itself a receipt, file, uphold as its own arbiter. Revocation doesn't fully protect against this — backdating `issued_at` keeps a departed seller exposed for one full complaint window after leaving.
- **F4** — Reputation fabricable both directions for $1–100 via a self-owned marketplace: manufacture a spotless record, or permanently poison a competitor's seller's reputation for about a dollar, no appeal.

### Tier 2 — real money, moderate cost
- **F5** — Permit-amount decreases may bypass the two-speed exit timelock entirely if not routed through revoke-and-wait.
- **F6** — "Fully backed" describes the permit, not outstanding order volume — claims past the cap fail outright with **no dispute record created**, understating real fraud; also creates a cross-marketplace bank run on shared stake.
- **F7** — Bond math can't separate honest claims from frivolous ones; fixed filing cost (rent) exceeds the bond on small P2P orders, so scammers just stay under it; forfeit currently pays the seller, incentivizing seller+arbiter collusion.
- **F8** — Fully-backed invariant violable at the pending-unstake boundary — permits can be granted during a withdrawal's cooldown window while `staked` still reads the old, higher number.
- **F9** — Daily cap is a speed limit, not a bound — ~70% of every permit drainable within one complaint window at suggested defaults; doesn't protect any individual seller since it's computed across a marketplace's whole book.

### Tier 3 — structural
- **F10** — The sole arbiter is unpaid, appointed by the marketplace (seller-aligned, not buyer-aligned), swappable per-case with no cooldown; "use a multisig" fixes key custody, not any of this.
- **F11** — Public track record has no defined "good" direction and no decay — a 100%-reject marketplace can market that as a feature.
- **F12** — Unstake cooldown < complaint window, restated as rational best-play: defect the same hour you start the withdrawal clock.
- **F13** — The deferred "2× leverage for clean record" feature turns today's cheap reputation-fabrication trick into a real solvency risk.
- **F14** — No protocol revenue anywhere funds arbiter pay, monitoring, or an insurance backstop.

**Five changes that close the most ground:** scope disputes to the permit + add expiry; freeze terms at issuance + timelock tightening changes; make marketplace registration cost something (bonded, aged, weighted reputation); per-permit daily cap + protocol ceiling; rewrite decisions 3/4/8 to state what they actually deliver.

---

## Report 3 — Feasibility & scale review (13 findings: `Finding 1`–`Finding 13`)

**Mindset:** production Solana systems engineer. Read the actual `Cargo.lock`/`Cargo.toml` and vendored crate sources rather than reasoning generically. Assumes honest actors, bug-free code.

### Corrections to the document's own risk list
- Compute is not the bottleneck — default budget ~203k CU (Ed25519 instruction contributes 3,000, not 200,000); `raise_dispute` uses ~40–70k. Real constraint: **transaction size** (1,232-byte limit; design as written leaves ~180 bytes headroom).
- LiteSVM already supports `with_precompiles()` in the pinned 0.10.0, behind a feature flag — one line, not a version-bump migration.
- "Redeploy under a fresh program ID" only works because the current prototype holds nothing of value — the same move against a funded deployment would permanently strand seller collateral (no migration instruction exists).

### Tier 1 — breaks in the first hour
- **Finding 1** — LiteSVM precompile feature flag not enabled (`litesvm = { version = "0.10.0", features = ["precompiles"] }`).
- **Finding 2** — `anchor-lang` 1.1.2 doesn't re-export `load_instruction_at_checked`/`load_current_index_checked`/`ed25519_program::ID` — need `solana-instructions-sysvar` and `solana-sdk-ids` as direct deps.
- **Finding 3** — `anchor-spl` version/feature misconfiguration (needs exact-pin matching anchor-lang, `default-features = false` selecting only `token`+`associated_token`, `idl-build` feature wiring, and a missing mint account in the token-transfer instructions).
- **Finding 4** — `RaiseDispute`'s account struct risks exceeding the 4KB SBF stack frame with ~14–15 accounts — box the cold-path accounts preemptively.

### Tier 2 — bites at first devnet deploy
- **Finding 5** — Deploy will cost ~5–7 SOL peak against a 2 SOL/request faucet cap — measure real `.so` size in Phase 1 via stub builds.
- **Finding 6** — Fresh-program-ID migration strategy needs revisiting before real funds are involved; prefer versioned PDA seeds (`["config","v2"]`) over a new address; add a version byte at a *fixed offset* + reserved padding on every account now.

### Tier 3 — bites building a UI/integration
- **Finding 7** — No efficient enumeration query path ("all open disputes for X") — point lookups by PDA are free/fast forever, but anything scanning needs `getProgramAccounts` with unindexed `memcmp`, which degrades forever and most RPC providers cap/rate-limit. Needs either an off-chain indexer (budget it) or an on-chain sequence-counter index pattern.
- **Finding 8** — `emit!` events can silently truncate past the 10KB log limit — use `emit_cpi!` instead so an indexer can't silently miss events.
- **Finding 9** — No other program can read a seller's reputation without hard-coupling to this program's exact Anchor/borsh version or hand-parsing byte offsets — add a small `export_reputation` instruction returning a frozen byte layout via `set_return_data`.
- **Finding 10** — Same marketplace-seeded-by-authority problem as `TS-16`, restated as an operational (not adversarial) failure.
- **Finding 11** — The convergent freeze finding, restated as a pure liveness/operations problem (no attacker needed).

### Tier 4 — bites after months/years
- **Finding 12** — Never-closed `DisputeRecord`s aren't as cheap as assumed — enumeration scans degrade forever, and per-dispute rent (~$0.36) can exceed the buyer's own bond on small P2P orders, inverting "honesty is free" for exactly the transactions this product targets. Add a `closable_after` timestamp + permissionless rent-refunding close.
- **Finding 13** — Per-marketplace write-lock hotspots (one marketplace account + one bond vault serializes that marketplace's throughput) — not a near-term problem, but the fix (sharded counters) is worth knowing about ahead of time.

---

## Notes for whoever reads this next

- This is a **design-stage** review — no handler code exists yet for v2, so "fix" above means "change the plan/account model," not "patch a bug in shipped code."
- The security and feasibility reports both independently flagged the `Marketplace` PDA being seeded by its own authority key as a rotation-blocking mistake (`TS-16` / `Finding 10`) — worth fixing alongside the freeze bug since both are seed-design decisions, cheapest to change before any account exists on-chain.
- Full verbatim agent transcripts (much longer, with complete attack-sequence prose) are in the conversation history that produced this file; this document and the linked artifact are the condensed, durable record.
