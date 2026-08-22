# TrustStake v2 test plan

A test suite that only proves the happy path works is worth very little here. This program
holds other people's money and hands a marketplace a signed line of credit against it, so
the suite's job is to prove that **the specific things that would let someone get robbed or
locked out cannot happen**.

This file is the checklist. It maps to [DESIGN-v2.md](DESIGN-v2.md) for what the program is
meant to do and to [AUDIT-v2-FINDINGS.md](AUDIT-v2-FINDINGS.md) for where each attack came
from. Every test named here should exist by the end of Phase 4, and each phase adds its own
slice as it lands.

## Running the suite

```bash
anchor build                                  # SBF target; plain cargo build will not work
cd programs/truststake && cargo test          # unit and integration, LiteSVM, in-process
cargo run --example devnet_demo               # real devnet signatures, costs SOL
```

Tests run against LiteSVM with the `precompiles` feature enabled, which is what allows
Ed25519 signature verification to execute in-process. If that feature is off, every dispute
test silently has nowhere to run.

**`cargo test` cannot silently run against a stale program.** The tests load the compiled
program through `include_bytes!` in `tests/common/mod.rs`, so the `.so` is baked into the
test binary at compile time; `include_bytes!` only fails if the file is absent, and `cargo
test` has no idea that `programs/truststake/src/` is what produces it, since the two go
through entirely different toolchains. `programs/truststake/build.rs` closes that gap: it
runs before any of this crate compiles, compares `target/deploy/{truststake,cpi_wrapper}.so`
against the newest file in the source that builds each one, and fails the build outright,
naming `anchor build` as the fix, if a `.so` is missing or older than its source. This is
exactly the failure mode a branch switch causes, since a different branch's program is
different source built into the same `target/`, and it used to be caught only by noticing a
suite pass suspiciously fast.

**Which tests belong to which phase:** a test lands in the phase that builds the last
instruction handler it calls, not the phase of the attacker it belongs to. So
`test_seller_cannot_release_permit_with_open_dispute` sits in the seller section below but is
a Phase 3 test, because it needs `raise_dispute` to exist. A phase is done when every test it
can run, runs.

## Continuous integration

`.github/workflows/ci.yml` runs `anchor build`, `cargo check --tests` and `cargo clippy
--tests` on every push and pull request. It does not run `cargo test`, on purpose:
`initialize_config` accepts only the signer hardcoded as `constants::INITIAL_ADMIN`, and
every test in this suite depends on that call succeeding first. That key is the real deploy
wallet's and, per its own doc comment, likely also the program's upgrade authority -- not
something to put in CI secrets. Beyond the ordinary risk of a CI secret leaking, `cargo
test`/`cargo check` run `build.rs` from every dependency in the tree with full access to the
process environment, which is a wider exfiltration surface than "CI secret" usually implies.

What the job still catches without that key: compile errors in the program and in the test
harness itself (`cargo check --tests` type-checks every file under `tests/`, including
`tests/common/mod.rs`, without executing anything in it), clippy regressions, and, via
`programs/truststake/build.rs`, a `target/deploy/*.so` older than the source that built it,
the same stale-artifact trap a branch switch causes locally. It proves nothing about
behaviour: run `anchor build && cargo test` locally before merging, same as this file already
says to.

Making `INITIAL_ADMIN` build-time configurable, so CI could bake a throwaway admin into its
own build and run the full suite, was considered and declined. It is mechanically possible.
It was declined because it turns a hardcoded, unconditional guarantee -- only one key can
ever call `initialize_config` on a given compiled program -- into one whose safety depends on
a CI workflow never being misconfigured to carry that override into a real deploy build,
trading a compiler-enforced fact for a process one. That trade is not worth it for coverage
this job already gets most of the value of: the behaviour a full run would additionally prove
is exactly what a human runs `cargo test` locally to confirm before merging anyway.

## Rules the suite follows

**Build every account through the program's own instruction handlers.** Hand-crafted
account data injected straight into the test environment hides missing initialisation logic
and missing constraints. A program can pass a full suite that way while being unusable once
deployed. The only exception is state a foreign program would have created, such as a token
mint.

**Use asymmetric, nonzero values.** A test with `cap = 0` or `claim = amount` only exercises
the point where two opposite comparisons coincide. Use $150 caps and $80 claims, and test
both sides of every boundary: one below, exactly at, one above.

**Assert state, not absence of error.** A passing transaction is not a passing test. Check
the vault balance, the counters, the status byte, and the token balances of every party.

**Every negative test asserts the specific error.** A test that passes because the
transaction failed for an unrelated reason is worse than no test, because it will keep
passing after the real check is deleted.

---

## Invariants

These must hold after every single instruction handler, in every test. Assert them in a
shared helper the `World` harness calls automatically at the end of each method, so no test
can forget.

- **Vault matches the ledger.** `stake_vault` token balance equals `SellerStake.staked`,
  always, including after a rejected complaint moves a bond into it.
- **Committed matches the permits.** `SellerStake.committed` equals the sum of
  `max_slashable - slashed` across that seller's active (unreleased) permits. Remaining
  allowance, not caps: a slash lowers both `staked` and `committed` together, and defining
  this as the sum of caps would break the next invariant the first time anyone is slashed.
- **Backing holds.** `committed <= staked`. Assert it immediately after an upheld dispute
  against a fully committed seller, which is the case that breaks if the two figures are
  not moved together.
- **Permits are bounded.** `permit.slashed <= permit.max_slashable`, for every permit.
- **Bond pool matches its records.** `bond_vault` balance for a marketplace equals the sum
  of `bond` across its open dispute records.
- **Nothing is created or destroyed.** Total USDC across the vault, the bond pool, and every
  participant's token account is constant across any instruction that is not a deposit or a
  withdrawal. This one catches more than all the others combined.
- **Counters never underflow.** `open_disputes` and `slashed` use checked arithmetic, and
  `disputes_lost <= disputes_total`.
- **Accounts stay rent-exempt.** No account's lamport balance drops below the rent-exempt
  minimum for its size.

Write these as a `assert_invariants(&world)` helper. Property tests below reuse it.

---

## Attacks by a malicious seller

The seller's incentive is to take payment and keep the goods, or to escape with collateral
that should have covered a complaint.

- `test_seller_cannot_withdraw_committed`: withdrawal leaving `staked < committed` fails.
- `test_seller_cannot_withdraw_during_window`: after revoking, withdrawal of that permit's
  collateral fails until `revoked_at + complaint_window` has passed.
- `test_seller_cannot_release_permit_with_open_dispute`: release fails while
  `open_disputes > 0`, even after the window elapses.
- `test_release_frees_committed`: releasing a permit drops `stake.committed` by exactly that
  permit's remaining allowance, and revoking alone drops it by nothing. Both directions
  matter: a release that forgets the subtraction leaves the seller permanently unable to
  withdraw, and a revoke that performs it early is the exit scam decision 8 exists to stop.
- `test_seller_cannot_lower_permit_cap`: any attempt to reduce `max_slashable` fails. This
  is the slash-evasion path: watch for a pending complaint, drop the cap to the
  already-slashed amount, and the wronged buyer collects nothing.
- `test_seller_cannot_regrant_smaller_during_window`: revoking and granting a new smaller
  permit to the same marketplace does not reduce exposure on complaints already open against
  the old one.
- `test_seller_cannot_overgrant`: permits summing beyond `staked` are rejected, both on the
  first grant and on an increase.
- `test_seller_cannot_grant_without_signing`: `grant_permit` without the seller's signature
  fails. This is the entire meaning of opting in.
- `test_seller_repeated_withdrawals_respect_committed`: many small withdrawals cannot
  accumulate past the gate through rounding.
- `test_seller_zero_amounts_rejected`: zero stake, zero withdrawal, zero cap all rejected.
- `test_seller_recovers_from_zero`: **positive test.** A seller slashed to zero calls
  `add_stake` and trades again. This is the prototype's dead-account hole and must stay
  fixed.

## Attacks by a malicious buyer

The buyer's incentive is to receive the goods and reclaim the money, or to claim against a
seller they never dealt with.

- `test_dispute_requires_receipt`: filing with no Ed25519 instruction in the transaction
  fails.
- `test_dispute_rejects_forged_signature`: a receipt signed by any key other than the
  marketplace's receipt signer fails.
- `test_dispute_rejects_tampered_amount`: altering the amount after signing fails, because
  the signature covers every byte.
- `test_dispute_rejects_expired_receipt`: a receipt past `expires_at` fails.
- `test_dispute_rejects_receipt_outside_window`: a receipt older than
  `issued_at + complaint_window` fails, using the window frozen on the permit rather than
  the marketplace's live value.
- `test_dispute_rejects_wrong_buyer`: a valid receipt naming a different buyer cannot be
  filed by this signer. Anyone who observes a leaked receipt must not be able to file and
  collect ahead of the real buyer.
- `test_dispute_rejects_wrong_seller`: a valid receipt naming a different seller cannot be
  filed against this seller's collateral.
- `test_dispute_rejects_cross_marketplace_receipt`: a receipt from marketplace A cannot be
  filed against a permit for marketplace B.
- `test_dispute_rejects_claim_above_receipt`: `claim > receipt.amount` fails.
- `test_dispute_rejects_wrong_chain_id`: a receipt carrying the wrong cluster tag fails.
  Without this, a devnet signature is replayable on mainnet.
- `test_dispute_replay_blocked`: the same `order_id` cannot be disputed twice, before or
  after resolution.
- `test_dispute_replay_blocked_after_close`: after `close_dispute` deletes the record, the
  same receipt still cannot be reused, because it fails the window check by then. This is
  the test that proves closing records is safe.
- `test_dispute_rejects_receipt_issued_after_revocation`: a receipt dated after
  `revoked_at` fails.
- `test_dispute_rejects_stale_era_receipt_release_permit_early`: a permit is revoked,
  released early with `open_disputes == 0` (the era was never disputed), and re-granted at
  the same address with a different cap. A receipt from the old era, held rather than
  filed, still cannot be used against the new one, even though nothing about the old era
  was ever in dispute. Needs no complaint-window games: `release_permit_early` skips only
  that wait, so the whole cycle can land inside the receipt's original window; the receipt
  here is deliberately made stale by 6 hours, well past `CLOCK_SKEW_TOLERANCE_SECONDS`, so
  the test still exercises check 7's era bound rather than the minimum-wait guard below.
- `test_release_permit_early_enforces_clock_skew_wait`: boundary test for the minimum wait
  itself. One second before `revoked_at + CLOCK_SKEW_TOLERANCE_SECONDS`, `release_permit_early`
  fails with `EarlyReleaseWaitNotElapsed`; exactly at the boundary it succeeds.
- `test_dispute_rejects_cross_era_receipt_after_fast_early_release`: the fast-path replay
  this wait exists to close. A genuine, never-disputed era-1 receipt is issued, then the
  seller revokes, waits out exactly `CLOCK_SKEW_TOLERANCE_SECONDS`, releases early, and the
  marketplace re-grants -- realistic non-zero gaps throughout, not a same-instant probe.
  Filing the stale receipt against the fresh permit fails with `ReceiptIssuedBeforeGrant`,
  and era 2's collateral is untouched. Before the minimum-wait guard existed, the identical
  scenario with a fast (under-an-hour) revoke-release-regrant cycle let the receipt file
  and get upheld, slashing collateral that had nothing to do with the closed-out era.
- `test_dispute_rejects_stale_era_receipt_release_permit_widened_window`: the other
  reachable path to the same replay. Ordinary `release_permit` already consumes the
  receipt's original window by the time its own wait elapses, so this one needs the
  marketplace to raise its complaint window between release and re-grant -- which is
  exactly what `test_dispute_replay_blocked_across_regrant_with_longer_window` proves safe
  for the *closable_after* replay guard. `granted_at` is what closes this other route to
  the same address: the receipt still fails, and for a different reason than that test's.
- `test_dispute_grant_clock_skew_tolerance_boundary`: a receipt issued exactly
  `CLOCK_SKEW_TOLERANCE_SECONDS` before the permit's `granted_at` is accepted; one second
  earlier and it is refused. Pins the deliberate allowance for the marketplace's signing
  clock lagging the chain's, and which side of the boundary is inclusive.
- `test_dispute_underpaid_bond_rejected`: a bond transfer smaller than
  `claim * permit.bond_bps / 10_000` fails.
- `test_buyer_cannot_resolve`: a non-arbiter calling `resolve_dispute` fails.
- `test_buyer_cannot_expire_early`: `expire_dispute` before the deadline fails.
- `test_buyer_cannot_close_open_dispute`: `close_dispute` while `status == Open` fails.

## Attacks by a malicious marketplace

Decision 2 accepts that a marketplace can draw the full permit at will. **These tests prove
it cannot go one cent past that**, and cannot reach anything that is not its own.

- `test_marketplace_cannot_exceed_permit`: a claim above the permit's remaining balance
  pays out only the remainder. The dispute is still recorded and still counted, because
  rejecting it would leave the seller's public loss count understating real fraud.
- `test_marketplace_cannot_exceed_staked`: payout is clamped to the vault's real balance.
- `test_marketplace_cannot_resolve_foreign_dispute`: marketplace A's arbiter resolving a
  dispute belonging to marketplace B fails.
- `test_attacker_marketplace_cannot_resolve`: an attacker registers their own marketplace,
  appoints themselves arbiter, and passes their own marketplace account into
  `resolve_dispute` for someone else's dispute. Must fail. Every account has to be chained
  off the `DisputeRecord`, never off what the caller supplies.
- `test_arbiter_cannot_redirect_payout`: passing a token account other than
  `dispute.buyer`'s fails. An arbiter must not be able to uphold a complaint and route the
  money to themselves.
- `test_dispute_cannot_be_resolved_twice`: the second call fails on the `status == Open`
  guard. Without it, a never-closed record drains other buyers' bonds from the shared pool
  and underflows `open_disputes` past the release gate.
- `test_bond_uses_recorded_amount`: raising `bond_bps` after a complaint is filed does not
  change what that complaint pays out. The deposited amount is stored on the record and only
  that amount ever moves. Otherwise the shared bond pool is overdrawable.
- `test_marketplace_window_bounds_enforced`: registering or updating with a window below 2
  days or above 30 days fails.
- `test_marketplace_bond_ceiling_enforced`: `bond_bps` above 2,000 fails. A marketplace
  must not be able to price buyers out of complaining.
- `test_marketplace_settings_do_not_affect_existing_permits`: changing the window or bond
  rate after a permit is granted has no effect on that permit. Assert the frozen values
  directly.
- `test_signer_rotation_preserves_old_receipts`: a receipt issued before
  `signer_rotated_at` still verifies against the previous key; one issued after it does not.
  An honest rotation must not silently void every pending buyer's claim.
- `test_parallel_disputes_cannot_exceed_cap`: two complaints raised against the same permit
  before either resolves cannot collectively pay out more than the remaining cap. This is a
  check-then-act gap: both pass the raise-time check against the same undecremented balance.
- `test_marketplace_cannot_freeze_seller_forever`: a marketplace opens a complaint and
  never resolves it. After 30 days anyone calls `expire_dispute`, the freeze lifts, and the
  seller withdraws. **This is the convergent finding all three reviewers hit, and it is the
  single most important test in this file.**
- `test_dispute_scoped_to_permit`: a complaint at marketplace A does not block withdrawal
  of free collateral, does not block release of marketplace B's permit, and does not touch
  B's committed balance.

## Attacks on the signature check

This is the most exploitable surface in the design. A weakness here produces forged
marketplace receipts with no key compromise at all, which defeats every other protection at
once.

- `test_introspection_rejects_forged_sysvar`: **run this one first.** Pass an
  attacker-created account in the Instructions sysvar slot, containing a fabricated
  transaction whose "Ed25519 instruction" is entirely made up. Every other check in the
  handler reads out of that account, so if its address is not verified against
  `sysvar::instructions::ID` this single substitution defeats all of them at once. Cheaper
  for an attacker than the crossed-indices route below, and it lands in the same place.
- `test_introspection_rejects_missing_ed25519`: no verify instruction present.
- `test_introspection_rejects_wrong_program`: an instruction that looks right but targets a
  different program.
- `test_introspection_rejects_different_message`: the verify instruction genuinely verifies
  a signature, but over a different message than the one the handler reads.
- `test_introspection_rejects_unexpected_index`: the verify instruction placed somewhere
  other than immediately before `raise_dispute`.
- `test_introspection_rejects_crossed_indices`: **the important one.** The Ed25519
  instruction's header carries three instruction indices alongside its byte offsets. An
  attacker constructs a transaction where the precompile validly verifies their own
  throwaway signature, while those indices point at a different instruction the attacker
  also controls, so the handler reads an attacker-chosen key and message. Checking offsets
  alone does not catch this. Assert that all three indices refer to the Ed25519 instruction
  itself.
- `test_introspection_rejects_out_of_bounds_offsets`: offsets pointing past the end of the
  instruction data must be caught before any slicing happens, not panic.
- `test_introspection_rejects_wrong_message_length`: a message longer or shorter than the
  serialised receipt.
- `test_introspection_rejects_cpi_wrapper`: introspection reads top-level transaction
  instructions only. A transaction that reaches `raise_dispute` through a wrapper program
  must not be able to fool the check.
- `test_introspection_rejects_missing_domain_tag`: a signature over the same fields without
  the domain prefix must not verify, so a signer key reused in another context cannot be
  turned into a receipt.

## Attacks by an unrelated outsider

- `test_initialize_config_cannot_be_frontrun`: any signer other than the compiled-in
  initial admin fails.
- `test_pda_substitution_rejected`: passing a permit belonging to a different
  seller-and-marketplace pair fails.
- `test_substituted_marketplace_rejected`: a caller-supplied `Marketplace` account that is
  not the PDA for its own stored ID fails, so no permit or dispute address can be derived
  from a forged marketplace.
- `test_account_type_substitution_rejected`: passing a `Marketplace` account where a
  `Config` is expected fails on the discriminator.
- `test_fake_vault_rejected`: an attacker-owned token account passed as `stake_vault`
  fails.
- `test_wrong_mint_rejected`: a mint account other than `stake_vault.mint` fails, in every
  handler that moves tokens, and a vault created against a mint other than
  `Config.collateral_mint` fails at `initialize_stake`. This is the check that
  closes the transfer-fee risk: a fee-bearing mint would make the recorded `staked` disagree
  with the vault's real balance, quietly and permanently, and pinning one mint by address
  means one known fee policy.
- `test_transfer_fee_mint_rejected`: build a Token Extensions mint carrying a transfer fee
  and confirm it is rejected as a non-matching mint rather than accepted and silently
  under-transferring. Same code path as above, but worth its own test because it is the
  failure the audit actually described.
- `test_unbound_mint_rejected`: a handler that reads balances must reject a substituted mint
  account even when the vault itself looks correct.
- `test_unauthorized_authority_transfer_rejected`: only the current authority can propose,
  and only the named pending authority can accept.

## Failures with no attacker

Every one of these has caused real losses in production systems, and none of them requires
anyone to be malicious.

- `test_abandoned_marketplace_does_not_trap_seller`: the marketplace shuts down and its
  arbiter key is never used again. Expiry must free the seller. Same code path as the
  malicious freeze, different story, and the story is the more likely one.
- `test_dispute_seed_includes_seller`: two different sellers on one marketplace happen to
  reuse the same `order_id` -- the natural outcome of per-seller order numbering, which
  nothing forbids. Both buyers hold genuine receipts and both complaints must succeed. Before
  the `DisputeRecord` seed included the seller, the second complaint collided with the
  first's PDA and was refused, permanently.
- `test_window_boundary_exact`: a complaint filed at exactly `issued_at + complaint_window`
  and one second either side. Assert which side is inclusive and that it matches the doc.
- `test_expiry_boundary_exact`: the same for the 30-day dispute deadline.
- `test_release_boundary_exact`: the same for `revoked_at + complaint_window`.
- `test_issued_at_overflow`: a receipt with `issued_at` near `i64::MAX` must fail cleanly
  rather than panic on `issued_at + complaint_window`. An unchecked add here is a denial of
  service against one specific buyer's ability to file.
- `test_bond_rounding_small_claims`: a claim small enough that `claim * bond_bps / 10_000`
  would truncate to zero. The bond rounds **up**, so assert a nonzero bond, and assert the
  buyer's balance falls by exactly the rounded-up figure. Rounding a payment in the payer's
  favour is the shape of bug that gets industrialised.
- `test_conservation_asserted_onchain`: force the vault balance *below* `staked` (written
  directly into account state, since nothing outside the program can move tokens out of a
  vault whose authority is the `stake` PDA) and confirm the next handler that moves tokens
  fails rather than proceeding on a wrong figure. This tests the runtime assertion, not the
  test-harness one. The check is `>=`, not exact equality, precisely so the opposite
  mismatch -- an unrelated party donating into the vault, which anyone can always do -- is
  not a violation; see `test_stake_vault_donation_does_not_brick_the_seller`.
- `test_stake_vault_donation_does_not_brick_the_seller`: an unrelated third party transfers
  one minor unit into a seller's vault, directly, outside the program. Confirm the seller can
  still withdraw, still add stake, and a dispute against them can still be resolved
  afterwards. A permanent, unrecoverable freeze from a single unsolicited deposit is a denial
  of service, not a conservation violation.
- `test_max_value_arithmetic`: `u64::MAX` stake, cap, and claim. Nothing may wrap.
- `test_rent_refund_on_close`: the rent goes to the buyer, and the account is genuinely
  gone afterwards.

---

## Property tests

Fixed test cases only prove the cases someone thought of. These generate the cases nobody
thought of.

Start with pure-function property tests using `proptest`, which needs no extra
infrastructure:

- payout is always `min(claim, permit remaining, staked)`, for all inputs
- bond calculation never overflows and never exceeds the claim
- window and expiry arithmetic never wraps for any `issued_at` and any legal window

Then a stateful sequence test: generate a random sequence of legal operations (stake,
grant, increase, revoke, release, raise, resolve, expire, close, withdraw) against a small
set of sellers and marketplaces, execute each, and run `assert_invariants` after every step.
The conservation invariant, that no USDC is created or destroyed, is what makes this worth
the effort.

Trident is the fuzzing framework built for Anchor programs and is the natural next step past
`proptest` here. Treat it as post-MVP unless the stateful test above turns up drift, in
which case it stops being optional.

---

## Limits and measurements

These are not pass-or-fail assertions so much as numbers to record and watch, but the first
one has a hard ceiling and needs a real test.

- `test_raise_dispute_transaction_size`: build the full two-instruction transaction and
  assert it serialises under **1,232 bytes**. Measured at **953** in Phase 3. Transaction
  size, not compute, is the binding constraint in this program. If it goes over, the fix is
  Address Lookup Tables, never dropping a check.
- `test_raise_dispute_compute`: record the consumed compute units. Measured at **42,229**
  in Phase 3 against a 200,000 default budget. If this comes in near the budget, something is
  wrong with an assumption rather than with the budget.
- `test_raise_dispute_stack_frame`: the account struct must not blow the 4KB stack frame.
  Box the cold-path accounts. This shows up as a build warning rather than a test failure,
  so check for it explicitly.
- Record the compiled program size after each phase. It drives deploy cost, which can peak
  at several SOL against a 2 SOL faucet limit.

---

## Regression coverage against the audit

Every critical and high finding from [AUDIT-v2-FINDINGS.md](AUDIT-v2-FINDINGS.md) needs a
named test that fails if the fix is ever removed. This table is the map. Keep it current
when tests are renamed.

| Finding | What it is | Test |
| --- | --- | --- |
| TS-01 | Crossed Ed25519 instruction indices | `test_introspection_rejects_crossed_indices` |
| TS-02 | Zero sentinel makes release timer always true | `test_seller_cannot_withdraw_during_window` |
| TS-03 | Exit scam via cooldown shorter than window | `test_seller_cannot_withdraw_committed` |
| TS-04 | Repeatable resolution drains the bond pool | `test_dispute_cannot_be_resolved_twice` |
| TS-05 | Order ID not checked against the signed receipt | `test_dispute_replay_blocked` |
| TS-06 | Self-registered marketplace resolves foreign disputes | `test_attacker_marketplace_cannot_resolve` |
| TS-07 | Receipt seller and buyer not bound to accounts | `test_dispute_rejects_wrong_seller`, `test_dispute_rejects_wrong_buyer` |
| TS-09 | Retroactive term changes | `test_marketplace_settings_do_not_affect_existing_permits` |
| TS-10 | Payout redirected away from the buyer | `test_arbiter_cannot_redirect_payout` |
| TS-11 | Parallel disputes race the cap | `test_parallel_disputes_cannot_exceed_cap` |
| TS-13 | Cap lowered to evade a pending slash | `test_seller_cannot_lower_permit_cap` |
| TS-14 | Permit released with a dispute open | `test_seller_cannot_release_permit_with_open_dispute` |
| TS-15 / F2 / Finding 11 | The convergent freeze | `test_marketplace_cannot_freeze_seller_forever`, `test_dispute_scoped_to_permit`, `test_abandoned_marketplace_does_not_trap_seller` |
| TS-16 / Finding 10 | Marketplace keyed by its own authority | `test_unauthorized_authority_transfer_rejected` plus a positive rotation test |
| TS-18 | Bond recomputed from live rate | `test_bond_uses_recorded_amount` |
| TS-19 | No domain separation | `test_introspection_rejects_missing_domain_tag`, `test_dispute_rejects_wrong_chain_id` |
| TS-21 | Transfer-fee mint breaks accounting | `test_transfer_fee_mint_rejected`, `test_wrong_mint_rejected` |
| TS-24 | Config front-running | `test_initialize_config_cannot_be_frontrun` |
| TS-25 | Introspection is not CPI-safe | `test_introspection_rejects_cpi_wrapper` |
| TS-26 | Timestamp overflow | `test_issued_at_overflow` |
| TS-28 | Grant without the seller's signature | `test_seller_cannot_grant_without_signing` |
| TS-29 | Arithmetic hardening | `test_max_value_arithmetic`, `test_bond_rounding_small_claims` |
| F1 | Live-read terms rewritten under sellers | `test_marketplace_settings_do_not_affect_existing_permits`, `test_signer_rotation_preserves_old_receipts` |
| F5 | Permit decrease bypasses the timelock | `test_seller_cannot_regrant_smaller_during_window` |
| F6 | Claims past the cap vanish from the record | `test_marketplace_cannot_exceed_permit` |
| F8 | Backing violated at the withdrawal boundary | `test_seller_repeated_withdrawals_respect_committed` |
| Finding 4 | Stack frame overflow | `test_raise_dispute_stack_frame` |

Findings that were resolved by removing a mechanism rather than by adding a check have no
test, by definition: **TS-08, TS-12, TS-20, TS-22, TS-23, F9 and F12** all concerned the
daily slash cap or the unstake cooldown, and neither exists in v2. **TS-27, F4, F10, F11,
F13, F14 and Finding 13** are accepted limitations recorded in
[DESIGN-v2.md](DESIGN-v2.md) rather than defects, and a test cannot assert a documented
tradeoff.

---

## What the suite deliberately does not prove

Stated so nobody mistakes coverage for safety.

- **That a marketplace will not rob a seller.** It can, up to the permit cap, and the design
  says so. The suite proves only that it cannot exceed the cap and cannot reach anything
  else.
- **That an arbiter judges correctly.** Nothing onchain can check whether a phone arrived.
- **That reputation numbers are honest.** A marketplace signing its own receipts and judging
  its own disputes can fabricate a record in either direction. The counters are only as
  trustworthy as the marketplaces reporting them.
- **That collateral covers all outstanding orders.** A permit caps total damage, not
  per-buyer coverage. Keeping order volume in line with the permit is the marketplace's job
  and is unenforceable onchain by design.

---

## Definition of done

Before a phase counts as finished:

- `cargo test` passes with no ignored tests and no warnings.
- Every test in this file that belongs to the phase exists and asserts a specific error
  rather than any error.
- `assert_invariants` runs after every harness method, with no exceptions carved out.
- Every new instruction handler has at least one positive test, one authorisation test, and
  one boundary test.
- The regression table above is updated if any test was renamed.
