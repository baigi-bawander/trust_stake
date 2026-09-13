# TrustStake v2 audit findings

TrustStake has no paid external audit. What exists: three independent design-stage reviews
(adversarial security, mechanism-design/incentives, feasibility and scale), run blind to
each other before any code was written, producing 61 findings; three per-phase security
reviews of the built program; and two dedicated passes over the dispute lifecycle, one of
them adversarial and executing real exploit probes against the compiled program in LiteSVM.
Six cross-phase bugs were found and fixed since, none of them visible to any single-phase
review. 152 tests pass today (`cargo test` from `programs/truststake/`), plus a real devnet
run recorded in [TESTING.md](TESTING.md).

None of this is a substitute for an external audit, and it should not be read as one. An
external audit should start with the scope listed near the bottom of this file.

## Summary

| Severity | Count | Fixed | Open |
| --- | --- | --- | --- |
| Critical | 10 | 8 | 2 |
| High | 21 | 16 | 5 |
| Medium | 16 | 9 | 7 |
| Low | 14 | 6 | 8 |
| **Total** | **61** | **39** | **22** |

## Findings

Status legend: **Fixed** the code changed to close it. **Deliberate** considered and left as
is, reasoning documented elsewhere. **Deferred** an acknowledged gap with no fix planned yet.
**Superseded** the mechanism it targeted was removed by a later design change. IDs are not
renumbered across the four review passes; a Source column keeps the schemes apart.

| ID | Source | Severity | Finding | Status |
| --- | --- | --- | --- | --- |
| TS-01 | Security | Critical | Ed25519 introspection checked offsets, not instruction indices; a forged marketplace receipt needed no key compromise | Fixed, check 2 validates all three indices |
| TS-02 | Security | Critical | `revoked_at == 0` sentinel made the release timelock always true for an active permit | Fixed, `i64::MAX` sentinel |
| TS-03 | Security | Critical | Unstake wasn't gated on `committed`, and the cooldown was shorter than the complaint window, a clean exit scam | Fixed, the unstake mechanism was removed; `withdraw_stake` gates on `committed` directly |
| TS-04 | Security | Critical | No `status == Open` guard let a dispute resolve repeatedly, draining the bond vault | Fixed, `status == Open` is the first check |
| TS-05 | Security | Critical | The dispute PDA's `order_id` wasn't verified against the signed receipt's own field | Fixed, check 6 binds it to the seed |
| TS-06 | Security | Critical | The arbiter check only proved the signer was arbiter of a caller-supplied marketplace account | Fixed, `dispute` binds to `marketplace` via `has_one` (resolve_dispute.rs:45); permit, stake and vault derive from `dispute.seller`/`dispute.marketplace` PDA seeds instead of a caller-supplied account |
| TS-07 | Security | High | `raise_dispute` didn't bind `receipt.seller`/`receipt.buyer` to the accounts passed in | Fixed, check 6 |
| TS-08 | Security | High | The daily slash cap was self-set by the marketplace it restrained, and its base was inflatable | Fixed, the daily cap was removed entirely |
| TS-09 | Security | High | `update_marketplace` could retroactively rewrite live permit terms | Fixed, permit terms freeze at grant |
| TS-10 | Security | High | Payout destinations in `resolve_dispute` weren't bound to `dispute.buyer`/`dispute.seller` | Fixed, bound via PDA seeds derived from `dispute.seller`/`dispute.marketplace` (permit, stake, vault) and `token::authority = dispute.buyer` (buyer_token_account), not `has_one` |
| TS-11 | Security | High | The permit cap was a TOCTOU race between raise and resolve | Fixed, payout clamps to remaining allowance at resolve time, not checked at raise time |
| TS-12 | Security | High | Repeat `request_unstake` without resetting the timer could bypass the cooldown | Superseded, the unstake/cooldown mechanism no longer exists |
| TS-13 | Security | High | A permit could be shrunk to evade a slash while a dispute against it was pending | Fixed, `release_permit` requires `open_disputes == 0` |
| TS-14 | Security | High | A permit could release while a dispute against it was still open | Fixed, same `open_disputes == 0` gate |
| TS-15 | Security | High | The convergent freeze: one never-resolved dispute could lock a seller's entire collateral forever | Fixed, complaints scoped to the permit, permissionless 30-day expiry |
| TS-16 | Security | High | `Marketplace` seeded by its own authority key made key rotation structurally impossible | Fixed, marketplaces now carry an immutable ID |
| TS-17 | Security | High | `order_id` reuse by one seller permanently blocks a later dispute at that seed | Deliberate, published as a marketplace integration requirement, not a protocol fix |
| TS-18 | Security | Medium | The bond was recomputed from live `bond_bps` instead of the amount actually deposited | Fixed, `DisputeRecord.bond` stores the deposited amount |
| TS-19 | Security | Medium | No domain separation on the signed receipt | Fixed, `domain`, `program_id` and `chain_id` fields added |
| TS-20 | Security | Medium | `committed` accounting after a slash or revoke was unspecified | Fixed, `committed` redefined as the sum of remaining permit allowances |
| TS-21 | Security | Medium | Token-extension transfer-fee/hook risk if the collateral mint weren't classic SPL Token | Fixed, `ALLOWED_MINT_EXTENSIONS` allow-list, empty by default |
| TS-22 | Security | Medium | No collateral reservation at raise time; a marketplace could race another for shared stake | Fixed, `committed <= staked` enforced at grant time |
| TS-23 | Security | Medium | Daily-cap clamp-versus-reject behavior was unspecified | Superseded, the daily cap was removed entirely |
| TS-24 | Security | Medium | `initialize_config` was front-runnable on a freshly deployed program ID | Fixed, the signer must equal the hardcoded `INITIAL_ADMIN` |
| TS-25 | Security | Medium | Instruction introspection isn't CPI-safe; it reads only top-level instructions | Deliberate, `raise_dispute` is a top-level-only instruction by design |
| TS-26 | Security | Medium | `issued_at + complaint_window` could overflow | Fixed, checked addition |
| TS-27 | Security | Low | Permissionless registration lets a bad operator reset its track record under a new key | Deliberate, a speed bump rather than a full bypass; sellers must re-grant |
| TS-28 | Security | Low | Signer requirements were unstated; `grant_permit` must require the seller's own signature | Fixed, seller signature required |
| TS-29 | Security | Low | Arithmetic and rent hardening: u128 basis-point math, checked subtraction, a minimum bond | Deliberate, arithmetic hardened; a bond floor was rejected, `bond_bps` may legally be zero |
| TS-30 | Security | Low | Governance hygiene: two-step transfer, a version field, upgrade authority moved to a multisig | Deferred, two-step transfer and the version field shipped; the multisig move has not |
| F1 | Mechanism | Critical | Every buyer protection was a live-read, un-timelocked marketplace parameter | Fixed, permit terms freeze at grant and the daily cap is gone |
| F2 | Mechanism | Critical | The convergent freeze (see TS-15) | Fixed, same fix |
| F3 | Mechanism | Critical | A marketplace can self-deal any permit in two transactions | Deliberate, the accepted trust model: a marketplace is trusted up to the permit cap |
| F4 | Mechanism | Critical | Reputation is fabricable in both directions for one to one hundred dollars via a self-owned marketplace | Deliberate, counters are marketplace-reported; attack-resistant reputation is computed offchain from events |
| F5 | Mechanism | High | A permit-amount decrease could bypass the exit timelock | Fixed, caps are increase-only; lowering one requires revoke, wait, and regrant |
| F6 | Mechanism | High | "Fully backed" described the permit, not order volume; over-cap claims failed outright with no record | Fixed, over-cap claims are now recorded rather than rejected |
| F7 | Mechanism | High | Bond math can't separate honest from frivolous claims, and a rejected bond pays the seller | Deliberate, the rent refund lowers the filing-cost problem; forfeit-to-seller is accepted under the same trust model as F3 |
| F8 | Mechanism | High | The fully-backed invariant was violable at the pending-unstake boundary | Superseded, the unstake cooldown no longer exists |
| F9 | Mechanism | High | The daily cap was a speed limit, not a bound, and protected no individual seller | Superseded, the daily cap was removed entirely |
| F10 | Mechanism | Medium | The sole arbiter is unpaid, marketplace-appointed, and swappable with no cooldown | Deliberate, no independent arbitration by design; the arbiter slot can migrate to a jury program later |
| F11 | Mechanism | Medium | A marketplace's public track record has no defined "good" direction and no decay | Deferred, no mechanism added |
| F12 | Mechanism | Medium | The unstake cooldown was shorter than the complaint window, restated as rational best play | Superseded, the unstake cooldown no longer exists |
| F13 | Mechanism | Medium | A future leverage feature would turn cheap reputation-fabrication into real solvency risk | Deliberate, leverage isn't implemented |
| F14 | Mechanism | Medium | No protocol revenue funds arbiter pay, monitoring, or an insurance backstop | Deliberate, no protocol fee, by design |
| Finding 1 | Feasibility | High | The LiteSVM precompile feature flag wasn't enabled | Fixed, `features = ["precompiles"]` |
| Finding 2 | Feasibility | High | `anchor-lang` doesn't re-export the sysvar helpers `raise_dispute` needs | Fixed, direct dependencies on `solana-instructions-sysvar`/`solana-sdk-ids` |
| Finding 3 | Feasibility | High | `anchor-spl` version and feature misconfiguration | Fixed, exact-pinned with the correct features |
| Finding 4 | Feasibility | High | `RaiseDispute`'s accounts risked exceeding the 4KB SBF stack frame | Fixed, cold-path accounts boxed |
| Finding 5 | Feasibility | Medium | A deploy could cost more than one devnet faucet request | Fixed, measured ahead of time and deployed successfully |
| Finding 6 | Feasibility | Medium | The fresh-program-ID migration strategy would strand funded collateral | Fixed, versioned PDA seeds replace it |
| Finding 7 | Feasibility | Low | No efficient enumeration query path exists for open disputes | Deferred, needs an offchain indexer, budgeted before a UI is built |
| Finding 8 | Feasibility | Low | `emit!` can silently truncate past the 10KB log limit | Fixed, `emit_cpi!` used instead |
| Finding 9 | Feasibility | Low | No other program can read a seller's reputation without coupling to this program's exact version | Deferred, `export_reputation` not yet built |
| Finding 10 | Feasibility | Low | Same marketplace-seeded-by-authority problem as TS-16 | Fixed, same fix |
| Finding 11 | Feasibility | Low | The convergent freeze, restated as a pure liveness problem | Fixed, same fix as TS-15 |
| Finding 12 | Feasibility | Low | Never-closed `DisputeRecord`s: rent could exceed a small order's own bond | Fixed, `closable_after` plus permissionless `close_dispute` |
| Finding 13 | Feasibility | Low | Per-marketplace write-lock hotspots at scale | Deliberate, not a near-term problem; sharded counters is the known fix |
| TS-31 | Implementation | High | A single minor unit donated to a vault permanently froze a seller's collateral, via an exact-equality conservation check | Fixed 2026-08-25, the check now uses `>=` |
| TS-32 | Implementation | Low | A second signer rotation inside one complaint window voids outstanding claims | Deliberate, single `prev_receipt_signer` slot; a cooldown was rejected, see CLAUDE.md |
| TS-33 | Implementation | Low | The worst-case freeze after revocation is `complaint_window + 30 days`, not 30 days alone | Fixed, decision 7 now states the real bound |
| TS-34 | Implementation | Low | Expiry depends on the buyer's token account staying transferable | Deliberate, mitigated but not fully closed: any owned token account may be supplied, and the mint-extension allow-list blocks default-frozen mints |

## Cross-phase bugs

Phase-scoped review cannot see an interaction between two phases. Six bugs of this species
have been found and fixed; each is stated here as the invariant it taught, matching
CLAUDE.md's "Review history on v2" so the two cannot drift.

- A `SlashPermit` released and re-granted at the same address must not let a receipt's replay
  guard treat the new grant as a continuation of the old one, or a genuine receipt can
  double-slash the same collateral.
- A PDA seed that is the sole guard for an identity must name every identity it guards, or
  that identity can be permanently locked out of the action the seed protects.
- A stored permit must name which era granted it, or a receipt from a fully wound-down era
  can be filed against whatever gets granted next at the same address. Fixed by
  `SlashPermit.granted_at` and `raise_dispute` check 7.
- A revoked permit's early release must enforce its own minimum wait, or a wind-down-and-
  regrant cycle can complete inside the clock-skew tolerance check 7 depends on. Fixed by
  requiring `now >= revoked_at + CLOCK_SKEW_TOLERANCE_SECONDS`.
- A protocol ceiling compared against a stored, possibly grandfathered field must take the
  wider of the two, not the ceiling alone, or a grandfathered marketplace's `DisputeRecord`
  can be closed and its receipt replayed before its own longer window has elapsed. Fixed by
  deriving `closable_after` from `max(permit.complaint_window, MAX_COMPLAINT_WINDOW_SECONDS)`.
- The same pattern applies to `release_permit`'s wait: it must floor at
  `CLOCK_SKEW_TOLERANCE_SECONDS` rather than rely on a compile-time assertion between two
  constants, or a marketplace grandfathered below today's minimum window can release a permit
  before check 7's tolerance has actually elapsed. Fixed by
  `revoked_at + complaint_window.max(CLOCK_SKEW_TOLERANCE_SECONDS)`.

## Scope for an external audit

- `programs/truststake/src/ed25519.rs`, the most exploitable surface in the design. Already
  checked against SIMD-0152 and the Solana SDK's own security notes; worth independent
  re-verification.
- Cross-phase interaction between the collateral-lifecycle handlers and the dispute handlers.
  Every one of the six bugs above lived here, not inside any single handler.
- PDA seed coverage: confirm every seed names every identity it is the sole guard for.
- `raise_dispute`'s nine-check sequence, and its two grandfathering-sensitive constant
  comparisons (`closable_after`, `release_permit`'s wait).
- The mint-extension allow-list (`ALLOWED_MINT_EXTENSIONS`) and its interaction with the
  `>=` vault-conservation check.

## Depth

- [docs/DESIGN-v2.md](DESIGN-v2.md) for the full design and the reasoning behind each decision.
- [docs/SECURITY-CONTEXT.md](SECURITY-CONTEXT.md) for architectural reconnaissance written for
  an auditor.
- [docs/TESTING.md](TESTING.md) for the test plan and the live devnet transaction record.
