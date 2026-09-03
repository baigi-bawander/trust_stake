# devnet_demo.rs — findings from four real devnet runs

Written 2026-09-03, after runs on 2026-08-31, 2026-09-01, 2026-09-03 (plain) and 2026-09-03
(`--stage 3`). Every claim below was verified against the compiled program's source or
against live devnet state at the moment of writing, not inferred from the script's own
printout — the script's printout has been wrong twice and is not evidence.

This file exists so the analysis is not re-derived. It is the input to the demo-script fix
task. It deliberately does NOT cover documentation accuracy across the repo; that lives in
`REPO-PRESENTATION.md`.

---

## The verdict

Five surprises across four runs. **Zero were program faults.** In three of them the program
was the thing that stopped the script.

| Date | Surprise | Cause | Program's part |
|---|---|---|---|
| 08-31 | A withdrawal that "must fail" succeeded | Step 6 used a hardcoded `committed = 400` when a prior slash had made it 320 | **Correct.** 350 really was ≥ 320 |
| 08-31 | Step 5 printed `320 committed … (200 + 200)` | printed permit CAPS beside true `committed` | not involved |
| 09-01 | Step 1.6 staked an extra 147.50 | guard targets *pre-grant* free collateral, evaluated *post-grant* | **Correct.** A legal `add_stake` |
| 09-03 | Step 1.11 re-granted a released permit; stage 2 then died | guard asks "does the permit ACCOUNT exist", which cannot distinguish *never granted* from *granted then released* | **Caught it** — `PermitNotRevoked` (6011) |
| — | Dashboard total 10.50 high | frontend summed decided bonds | not involved |

Invariants re-checked against live devnet 2026-09-03, all exact:
`committed == Σ(permit remaining)` per seller (50 = 50, 470 = 470); `committed <= staked`;
`stake_vault balance == staked` (100.00 and 670.00, to the unit); every `bond_vault` at zero
with the only open dispute carrying a zero bond.

---

## The species

**A guard that infers "have I already done this?" from live chain state, where that state is
ambiguous between "never done" and "done, then undone by a later step."**

This is the *same species* as the program's own worst bugs — a PDA address reused across
eras, where existence alone does not identify the era (see `CLAUDE.md`, "Review history on
v2", bugs 1-4). The program closed it with `SlashPermit.granted_at` and `raise_dispute`'s
check 7. The script never got the equivalent treatment.

The pattern generalises: any guard reading a value that a **later** step can destroy or
decrease is unsafe on a resumed run. Creation and destruction of the same address, by
different steps of the same script, is what makes existence uninformative.

---

## Why the existing tests cannot catch this class at all

`tests/test_devnet_demo_parity.rs` states its own design in its module docs: it reaches the
same instructions "through the `World` harness's thinner wrappers … so nothing here is shared
code that would hide a drift between the two." That is the right call for handler-sequence
parity — and it means **the parity tests never execute a single guard**. They replay the
instruction sequence; they do not run the script's decision code.

Measured 2026-09-03:

- `examples/devnet_demo.rs` — ~2,471 lines, **0** `#[test]` / `#[cfg(test)]`
- 69 lines matching idempotency-guard patterns (`already`, `skipping`, `is_err()`,
  `is_ok()`, `try_read_*`, `get_i64(progress …)`)

So the demo script is the only substantial Rust in this repo with **no test coverage of its
own logic**, and both of its real bugs lived exactly there. Neither the 151-test suite nor
the two parity tests could have caught either one. That is a structural gap, not bad luck.

---

## Confirmed defects

### D1 — Step 1.11 re-grants permits that stages 2 and 3 delete (LIVE, already fired)

`stage1_11_seller_b` decides "already granted" from the permit account's existence. Stage 2
(`release_permit_early`) and stage 3 (`release_permit`) both **close** that account — that is
their entire purpose. On 2026-09-03 a plain re-run found the PixelBazaar address empty,
granted a fresh un-revoked permit, and stage 2 then failed on it with `PermitNotRevoked`.
Stage 3 never ran that pass.

`progress.json` already records what the guard needs: `seller_b_cashdesk_revoked_at` and
`seller_b_pixelbazaar_revoked_at`. Either key existing means that permit's whole life is
over.

**Artifact still on devnet:** permit `CyxVyj1U…`, seller B × PixelBazaar, cap 50.00, granted
2026-09-03 11:35 UTC, never revoked. It holds 50.00 of seller B's collateral. The fixed
script must tolerate it (skip, print a note) rather than crash on it or try to reuse it.

### D2 — Step 7's guard is deleted by stage 4.2 (LATENT, ARMS ON 2026-09-23)

The most consequential finding here, and it was not previously known.

Step 7 skips when `find_dispute_by_marketplace_seller_status(cashdesk, seller, Upheld)`
returns `Some`. Stage 4.2 calls **the identical function** and then `close_dispute`s exactly
that record. So the run that completes stage 4.2 destroys the evidence step 7 depends on.

Timeline, from the disputes' own on-chain `closable_after`:

- **Sep 23 run** — safe. Steps 1-8 run before stages, so step 7 still sees the record and
  skips; stage 4.2 then closes it.
- **Sep 30 run (or any later run) — step 7 finds nothing and re-raises.**

What then happens, traced through the program rather than assumed: step 7 signs the receipt
with `cashdesk_receipt_signer`, the ORIGINAL key. Step 1.4 rotated CashDesk's signer on
2026-08-31 (`signer_rotated_at = 1788206869`). `raise_dispute` accepts the previous signer
only while `receipt.issued_at < marketplace.signer_rotated_at`
(`instructions/raise_dispute.rs:153-158`), and a fresh receipt is stamped `now`. So the
transaction is **refused with `WrongReceiptSigner`** and the script dies before stage 4 runs.

**Severity: blocking, not financial.** Had that rotation not happened, step 7 would have
raised and upheld a second 80.00 claim and slashed seller A for real — the CashDesk permit
still has 120.00 of remaining allowance. The program refuses by luck of an unrelated demo
step, not by design. Treat this as the one that would have cost money.

### D3 — Step 1.6's target is evaluated at the wrong point in the sequence (LIVE, self-limiting)

`stage1_6_top_up_seller_free_collateral` skips when free collateral ≥ 200. That target only
makes sense *before* 1.7/1.8 commit 150 of it. Post-grant, free sits near 50, so on the
second run it topped up 147.50 and pushed seller A's stake 522.50 → 670.00. It is
self-limiting — free now sits at exactly 200 and later runs skip — but the reasoning is
wrong and the shape is the same as D1/D2.

### D4 — Suspected siblings, not yet confirmed

Every guard reading a destroyable subject is suspect. Known candidates to check, not verified:

- **Step 5** (`grant_permit`, seller A × CashDesk / PixelBazaar) — "already granted" from
  account existence. Seller A's permits are never released by the script today, so this is
  latent rather than live. It arms the moment any step releases one.
- **Step 4** (`add_stake` to a `staked >= 500` target) — `staked` decreases on slash and
  withdrawal, so this can re-fire. Arguably a converging top-up rather than a one-time
  demonstration, which would make it acceptable; needs a decision, not an assumption.
- **Stage 2 / stage 3's own guards** — both skip when the permit account is missing. That is
  correct only while nothing re-creates it, which is precisely what D1 does. Mirror image of
  the same flaw.

Enumerating the rest is the fix task's first phase.

---

## What the fix must satisfy

1. **Guards for one-time demonstrations must read recorded progress, not live chain state.**
   Steps 6 and 7's *intent* is already right — step 6 uses the flag
   `step6_withdraw_demo_done` correctly; step 7 uses a chain query and is D2.
2. **Guards that legitimately read the chain must be converging operations**, not one-time
   proofs — "top up to target" is fine if the target is correct at the point it runs.
3. **The script must be safe against today's real devnet state**, including `CyxVyj1U…`, not
   only against a blank chain.
4. **The decision logic must become testable.** The parity tests structurally cannot reach
   it, so the guard decisions need extracting into pure functions with their own unit tests,
   including regression cases for D1, D2 and D3.
5. **No devnet spending from the fix task.** The user runs the script themselves.

---

## Traps for whoever picks this up

- **Do not run `cargo run --example devnet_demo` against devnet.** The user runs it.
- **The plain command is currently a trap** — it re-fires D1 every time. `--stage N` avoids
  stage 1 but NOT steps 1-8, which are ungated (`should_run` gates only stages 1-4,
  `examples/devnet_demo.rs:871-885`). So `--stage N` does not protect against D2.
- **Marketplace IDs are random**, generated by `random_marketplace_id()` and cached in
  `examples/.devnet-demo-state/progress.json`. They are not fixed strings; an earlier task
  briefing wrongly assumed they were.
- **The script's own printouts are not evidence.** Two have been wrong. Verify against chain.
- On rejection, `resolve_dispute` moves the bond into the **seller's** stake vault (raising
  `SellerStake.staked`), not to the marketplace — "the only path where a seller's free
  balance grows without a deposit" (`instructions/resolve_dispute.rs`). Confirmed on chain:
  the 2026-08-31 rejection moved 2.50 and took staked 520.00 → 522.50.
