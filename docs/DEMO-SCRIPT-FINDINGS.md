# devnet_demo.rs, findings from four real devnet runs

Written 2026-09-03, after runs on 2026-08-31, 2026-09-01, 2026-09-03 (plain) and 2026-09-03
(`--stage 3`). Every claim below was verified against the compiled program's source or
against live devnet state at the moment of writing, not inferred from the script's own
printout: the script's printout has been wrong twice and is not evidence.

This file is the input to the demo-script fix task, so the analysis is not re-derived.

## The verdict

Four surprises across four runs. Zero were program faults; in three of them the program was
the thing that stopped the script.

| Date | Surprise | Cause | Program's part |
| --- | --- | --- | --- |
| 08-31 | A withdrawal that "must fail" succeeded | Step 6 used a hardcoded `committed = 400` when a prior slash had made it 320 | Correct: 350 really was >= 320 |
| 08-31 | Step 5 printed `320 committed (200 + 200)` | Printed permit caps beside true `committed` | Not involved |
| 09-01 | Step 1.6 staked an extra 147.50 | Guard targets pre-grant free collateral, evaluated post-grant | Correct: a legal `add_stake` |
| 09-03 | Step 1.11 re-granted a released permit; stage 2 then died | Guard asks "does the permit account exist," which cannot distinguish never-granted from granted-then-released | Caught it, `PermitNotRevoked` (6011) |

Invariants re-checked against live devnet 2026-09-03, all exact: `committed == sum(permit
remaining)` per seller (50 = 50, 470 = 470); `committed <= staked`; `stake_vault balance ==
staked` (100.00 and 670.00, to the unit); every `bond_vault` at zero with the only open
dispute carrying a zero bond.

## The species

A guard that infers "have I already done this?" from live chain state, where that state is
ambiguous between "never done" and "done, then undone by a later step."

This is the same species as the program's own worst bugs, a PDA address reused across eras,
where existence alone does not identify the era (see CLAUDE.md, "Review history on v2," bugs
1-4). The program closed it with `SlashPermit.granted_at` and `raise_dispute`'s check 7. The
script never got the equivalent treatment. The pattern generalises: any guard reading a value
that a later step can destroy or decrease is unsafe on a resumed run.

## Why the existing tests cannot catch this class at all

`tests/test_devnet_demo_parity.rs` replays the instruction sequence through the `World`
harness's thinner wrappers, so it never executes a single guard: it exercises handler-sequence
parity, not the script's own decision code.

Measured 2026-09-03:

| What | Count |
| --- | --- |
| `examples/devnet_demo.rs` lines | ~2,471 |
| `#[test]` / `#[cfg(test)]` in that file at the time | 0 |
| Lines matching idempotency-guard patterns (`already`, `skipping`, `is_err()`, `is_ok()`, `try_read_*`, `get_i64(progress ...)`) | 69 |

So the demo script was the only substantial Rust in this repo with no test coverage of its
own logic, and both of its real bugs lived exactly there.

## Confirmed defects

### D1, Step 1.11 re-grants permits that stages 2 and 3 delete (live, already fired)

`stage1_11_seller_b` decided "already granted" from the permit account's existence. Stages 2
(`release_permit_early`) and 3 (`release_permit`) both close that account, which is their
entire purpose. On 2026-09-03 a plain re-run found the PixelBazaar address empty, granted a
fresh un-revoked permit, and stage 2 then failed on it with `PermitNotRevoked`; stage 3 never
ran. `progress.json` now records `seller_b_cashdesk_revoked_at` and
`seller_b_pixelbazaar_revoked_at`; either key existing means that permit's whole life is over.

**Artifact still on devnet:** permit `CyxVyj1U...`, seller B x PixelBazaar, cap 50.00, granted
2026-09-03 11:35 UTC, never revoked, holding 50.00 of seller B's collateral. The fixed script
tolerates it rather than crashing on it or reusing it.

### D2, step 7's guard is deleted by stage 4.2 (latent, arms on 2026-09-23)

The most consequential finding here. Step 7 skips when
`find_dispute_by_marketplace_seller_status(cashdesk, seller, Upheld)` returns `Some`. Stage 4.2
calls the identical function and then `close_dispute`s exactly that record, so the run that
completes stage 4.2 destroys the evidence step 7 depends on: a Sep 23 run is safe (steps 1-8
run before the stages, so step 7 still sees the record and skips; stage 4.2 then closes it),
but a Sep 30 run, or any later one, finds nothing and re-raises.

Traced through the program rather than assumed: step 7 signs with `cashdesk_receipt_signer`,
the original key. Step 1.4 rotated CashDesk's signer on 2026-08-31
(`signer_rotated_at = 1788206869`); `raise_dispute` accepts the previous signer only while
`receipt.issued_at < marketplace.signer_rotated_at` (`instructions/raise_dispute.rs:153-158`),
and a fresh receipt is stamped `now`, so the transaction is refused with `WrongReceiptSigner`
and the script dies before stage 4 runs.

**Severity: blocking, not financial.** Had that rotation not happened, step 7 would have
raised and upheld a second 80.00 claim and slashed seller A for real, since the CashDesk
permit still has 120.00 of remaining allowance; the program refuses by luck of an unrelated
demo step, not by design.

### D3, step 1.6's target is evaluated at the wrong point in the sequence (live, self-limiting)

`stage1_6_top_up_seller_free_collateral` skips when free collateral is at least 200, a target
that only makes sense before 1.7/1.8 commit 150 of it. Post-grant, free sits near 50, so on
the second run it topped up 147.50 and pushed seller A's stake from 522.50 to 670.00. It is
self-limiting (free now sits at exactly 200 and later runs skip), but the reasoning was wrong
and the shape matches D1/D2.

### D4, suspected siblings (not yet confirmed)

Every guard reading a destroyable subject is suspect:

- **Step 5** (`grant_permit`) infers "already granted" from account existence. Seller A's
  permits are never released by the script today, so this is latent, and arms the moment any
  step releases one.
- **Step 4** (`add_stake` to a `staked >= 500` target): `staked` decreases on slash and
  withdrawal, so this can re-fire. May be an acceptable converging top-up rather than a
  one-time demonstration; needs a decision, not an assumption.
- **Stage 2 and stage 3's own guards** both skip when the permit account is missing, correct
  only while nothing re-creates it, which is precisely what D1 does: the mirror image of the
  same flaw.

## What the fix must satisfy

1. Guards for one-time demonstrations must read recorded progress, not live chain state. Step
   6 already does this correctly via `step6_withdraw_demo_done`; step 7 reads the chain
   instead, which is D2.
2. Guards that legitimately read the chain must be converging operations, not one-time proofs:
   "top up to target" is fine if the target is correct at the point it runs.
3. The script must be safe against today's real devnet state, including `CyxVyj1U...`, not
   only a blank chain.
4. The decision logic must be testable: the parity tests structurally cannot reach it, so
   guard decisions need extracting into pure functions with their own unit tests, including
   regression cases for D1, D2 and D3.
5. No devnet spending from the fix task; the user runs the script.

## Traps for whoever picks this up

- **Do not run `cargo run --example devnet_demo` against devnet.** The user runs it.
- **Marketplace IDs are random**, generated by `random_marketplace_id()` and cached in
  `examples/.devnet-demo-state/progress.json`. They are not fixed strings; an earlier task
  briefing wrongly assumed they were.
- **The script's own printouts are not evidence.** Two have been wrong. Verify against chain.
- **On rejection, `resolve_dispute` moves the bond into the seller's stake vault** (raising
  `SellerStake.staked`), not to the marketplace, the only path where a seller's free balance
  grows without a deposit (`instructions/resolve_dispute.rs`). Confirmed onchain: the
  2026-08-31 rejection moved 2.50 and took staked from 520.00 to 522.50.
- **The plain command is safe again.** Fixed 2026-09-03 (commit `48dbbf1`): every guard,
  including D1 and D2 above, was extracted into a pure function with its own regression test
  (see CLAUDE.md's "Real devnet demo" entry). `--stage N` is no longer a safety measure, only
  a convenience for forcing one stage on or off its normal schedule.
