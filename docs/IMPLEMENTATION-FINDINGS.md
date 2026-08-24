# TrustStake v2 implementation findings

**Source reviewed:** the built program at the end of Phase 3, not the design.
[AUDIT-v2-FINDINGS.md](AUDIT-v2-FINDINGS.md) reviewed `DESIGN-v2.md` before any code existed
and numbered its findings TS-01 to TS-30; this file continues at TS-31 so the two never
collide. [SECURITY-CONTEXT.md](SECURITY-CONTEXT.md) is the map the hunt worked from.

**Method:** sharp-edge sweep over Solana and Anchor footguns, then variant analysis on each
root cause found. Every finding below was reproduced by executing it against the compiled
program in LiteSVM, not inferred from reading. The proofs were run and then deleted rather
than committed, because a test that asserts today's broken behaviour would have to be
rewritten by whoever fixes it; the code to reproduce each one is inline here.

**Totals:** 4 findings. 1 High, 3 Low. No theft path was found: no finding lets an attacker
take a token that is not theirs, and every one of the design's stated protections held under
the attacks written against it in Phase 3.

---

## TS-31 (High): one minor unit of USDC permanently freezes a seller's collateral

**Location:** `instructions/add_stake.rs:78`, `instructions/withdraw_stake.rs:92`,
`instructions/resolve_dispute.rs:222`, all three via the same convention.

**Root cause.** Each of those handlers ends by reloading the vault and requiring
`stake_vault.amount == stake.staked`. That compares the ledger against a balance the program
does not control: a token account accepts a transfer from anyone, and the owner never
consents. Nothing in the program reconciles the two figures afterwards. Every write to
`stake.staked` is a delta applied by a handler; none is ever computed from the vault's real
balance, so once the two disagree they disagree forever.

**Reproduction.** An unrelated third party, holding one minor unit of the collateral mint,
sends it to a seller's vault with a bare Token Program transfer. No signature from the
seller, no interaction with this program:

```rust
let donation = spl_token::instruction::transfer(
    &spl_token::ID, &attacker_token_account, &stake_vault, &attacker.pubkey(), &[], 1,
).unwrap();
common::send(&mut world.svm, &[donation], &attacker.pubkey(), &[&attacker]).unwrap();
```

After that transaction, against a seller holding $300 with an $80 complaint open, all three
of these fail with `ConservationViolation`, confirmed by execution:

- `resolve_dispute`: the arbiter can never pay the buyer, in either direction.
- `withdraw_stake`: the seller can never retrieve any part of $300.
- `add_stake`: the seller cannot even top the account up.

The program's own log shows the gap it refuses to proceed past: `Left: 300000001`,
`Right: 300000000`.

**Impact.** Any seller can be targeted by anyone for the price of one minor unit plus a
transaction fee, and the result is that their entire collateral becomes unreachable and every
complaint against them becomes unresolvable. The attacker gains nothing, so this is denial of
service rather than theft, but it is permanent under the deployed bytecode. It is recoverable
only by upgrading the program, which the deployment supports; the upgrade authority is
`constants::INITIAL_ADMIN`, a single ordinary keypair, not a multisig, and moving it to one
is a known future step, not yet done. Rated High rather than Critical on that basis alone:
with a frozen program it would be Critical.

`expire_dispute` carries no conservation assertion, so a frozen seller's permits can still be
unfrozen and released after 30 days. The collateral behind them still cannot move.

**Where it came from.** Not an implementation slip. `DESIGN-v2.md`'s Program conventions ask
for exactly this ("Every handler that moves tokens reloads the vault afterwards and requires
`stake_vault.amount == stake.staked`"), and `TESTING.md`'s `test_conservation_asserted_onchain`
pins the failing behaviour as the desired one. Fixing the code means amending both documents,
which is why this is reported rather than quietly patched.

**Recommended fix.** Change the three assertions from `==` to `>=`, requiring the vault to
hold at least what the ledger claims. That keeps the entire reason the check exists: the
dangerous direction is a vault holding *less* than the ledger, which is what a drain, a
mispriced payout or a transfer-fee mint produces, and `>=` still catches every one of them.
The benign direction, a stranger's donation, stops being fatal.

Donated dust would then sit in the vault unowned. If that is worth reclaiming, a
permissionless `sync_stake` that raises `staked` to the vault balance sweeps it to the seller
and is safe on its own terms, since it only ever increases `staked` and `committed <= staked`
is preserved. That is a second instruction and a Phase 4 decision; the `>=` change is not.

---

## TS-32 (Low): a second signer rotation inside one complaint window voids outstanding claims

**Location:** `instructions/update_marketplace.rs`, the rotation branch;
`instructions/raise_dispute.rs` check 3.

**Root cause.** `Marketplace` keeps exactly one `prev_receipt_signer`. A second rotation
overwrites it, and `raise_dispute` accepts only the current signer or that single previous
one, so receipts signed by the key from two rotations ago become unfileable even while they
are still inside their complaint window.

**Reproduction.** A marketplace rotates its receipt signer, then rotates again an hour later
on realising the replacement came off the same compromised machine. A buyer holding a genuine
receipt signed by the original key, still inside the 2-day window, is refused with
`WrongReceiptSigner`. Confirmed by execution.

**Impact.** The buyer loses their remedy for an order that really happened. The scenario that
triggers it, rotating twice in quick succession, is precisely a key-compromise incident,
which is the case `prev_receipt_signer` exists to survive. A malicious marketplace gains
nothing here that decision 2 does not already grant it, so the damage lands on honest
operators. Low severity, but it defeats the stated purpose of the mechanism in the one
situation the mechanism is for.

**Recommended fix.** Reject a rotation that lands within `complaint_window` of the last one,
which is a single `require!` in `update_marketplace` and makes "at most one signer change can
be in flight inside any window" true by construction. Keeping a ring of previous signers with
their own timestamps also works and costs account space and a loop.

---

## TS-33 (Low): the worst-case freeze after revocation is `complaint_window + 30 days`

**Location:** documentation rather than code. `DESIGN-v2.md` decision 7 and the
`complaint_window` ceiling in decision 12.

A seller reading decision 7 concludes that an undecided complaint costs them 30 days. The
real bound is longer. A receipt issued one second before revocation stays filable for the
whole complaint window, and a dispute opened at the end of that window then runs its own
30-day expiry. Against a marketplace with the maximum 30-day window, revoking today can leave
collateral committed for roughly 60 days.

Nothing here is broken: every timer behaves as the code says, and the ceiling on
`complaint_window` still bounds the total. The figure a seller is told should be the one they
can actually plan around.

**Recommended fix.** State the bound as `complaint_window + DISPUTE_EXPIRY` wherever the
30-day figure is quoted to sellers.

---

## TS-34 (Low): expiry depends on the buyer's token account staying transferable

**Location:** `instructions/expire_dispute.rs`, the `buyer_token_account` constraint and the
mandatory bond return.

Expiry is the seller's only escape from a marketplace that never decides, and it cannot
complete unless the bond can be sent back to the buyer. If the buyer's token account is
merely closed, anyone can recreate it, since creating an associated token account for another
owner is permissionless, and the freeze lifts. If the mint's freeze authority freezes the
buyer's account, no destination exists and the seller's permit stays frozen until it thaws.

`DESIGN-v2.md` records the issuer's freeze power as a limitation, but describes it against
the seller's vault. This is a different path to the same place, and it turns a third party's
account status into the seller's liveness dependency.

**Recommended fix.** None that is clearly worth its complexity, which is why this is Low.
Splitting expiry from the bond return, so the freeze lifts unconditionally and the bond stays
claimable separately, would close it at the cost of another instruction and another account
that must be reaped later.

---

## Checked and clean

Worth recording, because knowing where someone has already looked is most of the value of a
hunt.

- **Duplicate-account substitution.** Passing a vault as `buyer_token_account` in
  `resolve_dispute` or `expire_dispute` cannot reach a self-transfer: both vaults are owned
  by PDAs, and the constraint binds the destination to `dispute.buyer`, who had to sign the
  raise transaction. A PDA cannot sign a top-level transaction, so the two can never be the
  same account.
- **Bump canonicalisation.** Every stored bump originates from `ctx.bumps` at `init`, so
  `bump = x.bump` reads a canonical bump; the rest recompute.
- **Cross-marketplace bleed.** A slash cannot reach collateral committed elsewhere: each
  permit's remaining allowance is a term in `committed`, which is bounded by `staked`, so the
  payout's cap term is always the tighter of the two clamps.
- **Close and revive.** `close_dispute` frees the record's address, and what prevents the
  receipt being filed again is that it has aged out of its window by then, which is the
  condition `close_dispute` waits for.
- **Arithmetic.** 29 `checked_*` sites, no `saturating_*` on any balance, basis points
  through `u128`, and the bond rounds toward the buyer paying more.
- **Framework footguns.** No `unsafe`, no `init_if_needed`, no unbounded loops, no account
  left unbound, and the three `UncheckedAccount`s are two `has_one` rent destinations and the
  address-checked Instructions sysvar.
- **Marketplace ID squatting** is possible, since IDs are first-come-first-served, but it
  costs an attacker rent to reserve a name and moves no money. Informational.
