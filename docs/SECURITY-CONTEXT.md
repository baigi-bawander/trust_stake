# TrustStake v2 security context

Architectural reconnaissance of the program as it stands at the end of Phase 3, written for
whoever audits it next. It maps what exists and where the consequences live; it does not
report findings. [DESIGN-v2.md](DESIGN-v2.md) says what the program is meant to do and why,
[TESTING.md](TESTING.md) says what the suite proves, and
[AUDIT-v2-FINDINGS.md](AUDIT-v2-FINDINGS.md) is where the earlier attacks came from.

Every claim here was read out of the source rather than remembered. Where a figure is
measured, the test that measures it is named.

## What the system does

A seller locks USDC into a program-owned vault. For each marketplace they sell on, they sign
a permit capping what that marketplace may take from it. A scammed buyer files a complaint
backed by a marketplace-signed offchain receipt, the marketplace's arbiter decides it, and
the program moves collateral to the buyer up to the permit's cap and never past it. One pot
of collateral backs a seller across every marketplace, and the loss record travels with them.

## Stack and dependencies

Rust only: 9,197 lines across 39 files, of which `programs/truststake/src` is the protocol
and `programs/cpi_wrapper` is a two-handler test fixture that is never deployed. Anchor
1.1.2 exactly, matched by `anchor-spl` 1.1.2, against Solana crates on the 3.x line. Tests
run in LiteSVM 0.10.0 with the `precompiles` feature, which is what lets Ed25519
verification execute in-process.

Three dependency facts carry security weight:

- **The `solana-*` crate line is split across major versions** inside LiteSVM's own tree, so
  `Pubkey` from one line is a different type to the compiler than `Pubkey` from the other.
  Every direct dependency is pinned `^3` for that reason, and `cargo tree -d` has to be
  re-run after adding one. The duplicate list is unchanged since Phase 0.
- **`solana-ed25519-program` is a program dependency, not just a test one**, so the Ed25519
  instruction's byte offsets come from the crate that defines them rather than being
  restated as literals in the one place where a wrong offset is exploitable.
- **`anchor-spl` carries `token_2022`** because its `idl-build` does not compile without it,
  not because the program uses Token Extensions.

There is no CI configuration in the repository. Nothing runs the suite except a person
typing `cargo test`, and TESTING.md's warning that a stale `.so` makes the suite pass
against the previous program has no automated backstop.

## Account model and the PDA graph

Five account types, all versioned with reserved padding, all seeded with a `"v2"` component
so they cannot collide with the deployed v1 layout.

- `Config` at `["config", "v2"]`: the protocol singleton. Pins `collateral_mint` and
  `chain_id`, both immutable: there is no `update_config`, so a deployment that pins the
  wrong mint cannot be repaired, only replaced.
- `Marketplace` at `["market", "v2", marketplace_id]`: the tenant, keyed by an immutable
  client-chosen ID so every key on it can rotate without orphaning anything.
- `SellerStake` at `["stake", "v2", seller]`: one per seller globally, holding `staked` and
  `committed`, with `stake_vault` at `["vault", "v2", seller]` under its authority.
- `SlashPermit` at `["permit", "v2", seller, marketplace]`: the seller's line of credit to
  one marketplace, with money terms frozen at grant.
- `DisputeRecord` at `["dispute", "v2", marketplace, order_id]`: one complaint. Its
  existence is the replay guard.
- `bond_vault` at `["bonds", "v2", marketplace]`: one shared pool per marketplace, holding
  every live bond, under the `Marketplace` PDA's authority.

**Everything subordinate keys off an address, never off an ID.** Only `Marketplace` itself is
keyed by the raw `marketplace_id`, and its accounts struct re-derives its own PDA from the ID
stored inside it, so a substituted marketplace account cannot be used to derive a permit, a
bond vault or a dispute address. That self-validation is what
`test_substituted_marketplace_rejected` exercises from both directions.

Two PDAs sign: `SellerStake` for payouts out of `stake_vault`, and `Marketplace` for
movements out of `bond_vault`.

## Trust boundaries

**Buyer to program, through an offchain signature.** The only boundary where the program
believes something it did not compute. A buyer hands it a `[Ed25519 verify, raise_dispute]`
transaction; the program reads the receipt out of the verify instruction's message bytes and
checks every field against the accounts actually passed. Everything downstream of a forged
receipt is worthless, which is why this surface gets its own module and its own ten tests.

**Marketplace to seller's collateral, bounded by the permit.** Not a boundary the program
defends: decision 2 states that a marketplace signs its own receipts and appoints its own
arbiter, so it can draw the full permit at will in two transactions. What is defended is the
cap, and that it cannot reach a permit belonging to anyone else.

**Arbiter to payout destination.** The arbiter chooses the outcome and nothing else. Both
destinations are bound to `dispute.buyer` and to the seller's own vault.

**Time as an authority.** Three deadlines carry real power: a receipt's complaint window, the
30-day dispute expiry, and a permit's release window. Expiry is the one that makes a
non-answering marketplace harmless, and it is permissionless by design.

**Program to Token program.** Every movement goes through `transfer_checked` with the mint
and decimals carried through, and the mint is bound to the vault being moved rather than to
`Config`, since the vault's mint was checked against `Config` when it was created.

## Entry points

Nineteen handlers. "Authority" is what the program actually checks, not who is expected to
call.

| Handler | Authority | Moves money | Notes |
| --- | --- | --- | --- |
| `initialize_config` | signer equals compiled-in `INITIAL_ADMIN` | no | one-shot, front-run proof |
| `propose_config_authority` / `accept_config_authority` | `has_one` current / pending | no | two-step |
| `register_marketplace` | any signer, pays rent | no | creates `bond_vault` |
| `update_marketplace` | `has_one = authority` | no | stamps `signer_rotated_at` on a real key change |
| `propose_marketplace_authority` / `accept_marketplace_authority` | `has_one` current / pending | no | two-step |
| `initialize_stake` | seller signs | no | creates vault only |
| `add_stake` | seller signs | in | conservation asserted |
| `withdraw_stake` | seller signs | out | gated on `staked - amount >= committed` |
| `grant_permit` | seller signs | no | freezes window and bond rate |
| `increase_permit` | seller signs, `has_one` | no | increase-only by API shape |
| `revoke_permit` | seller signs, `has_one` | no | stamps `revoked_at`, frees nothing |
| `release_permit` | permissionless | no | needs window elapsed and `open_disputes == 0` |
| `release_permit_early` | seller **and** marketplace authority | no | skips the wait, not the wind-down |
| `raise_dispute` | buyer signs **and** marketplace-signed receipt | in (bond) | the widest surface |
| `resolve_dispute` | `marketplace.arbiter` | out (payout, bond) | every account chained off the record |
| `expire_dispute` | permissionless | out (bond) | the anti-freeze lever |
| `close_dispute` | permissionless | no | deletes the record, rent to buyer |

Three accounts are `UncheckedAccount`: two rent destinations bound by `has_one`
(`release_permit`'s seller, `close_dispute`'s buyer) and the Instructions sysvar, whose
address is checked inside the helper that reads it.

## Where money moves

Five handlers touch tokens. Three of them reload the vault afterwards and require
`stake_vault.amount == stake.staked` at runtime: `add_stake`, `withdraw_stake`,
`resolve_dispute`. The other two, `raise_dispute` and `expire_dispute`, move only a bond and
never touch the seller's vault, so that assertion is inexpressible in them; both say so in a
comment at the point where a reader would otherwise wonder.

The bond pool is the one balance with no onchain ledger to check against, because its ledger
is the set of open dispute records and the program cannot enumerate accounts. The test
harness asserts it after every instruction instead: each `bond_vault` equals the sum of
`bond` across that marketplace's Open records, and `open_disputes` equals the count of them.

## The three functions that carry the program

**`ed25519::verify_signed_receipt`** (`src/ed25519.rs`). Confirms the instruction immediately
before the current one is a canonical single-signature Ed25519 verification of a message that
deserialises as a receipt, and returns the receipt with the key that signed it. In order: the
sysvar's address, the current index and that the current instruction belongs to this program,
the neighbour's program ID and empty account list, an exact data length, the signature count
and padding byte, all four byte offsets against constants, and all three instruction indices
against the Ed25519 instruction itself. Offsets are pinned to constants before anything is
sliced and every slice goes through `get`, so an out-of-range read produces an error rather
than a panic.

The check that matters most is the last one. The precompile reads `u16::MAX` as "this
instruction's own data" and any other value as an absolute index into the transaction's
top-level instructions, so an attacker can have it verify their own throwaway signature while
those indices point at an instruction they also control, leaving the handler to read an
attacker-chosen key and message from bytes nobody signed. Validating the byte offsets alone
does not catch it. `test_introspection_rejects_crossed_indices` builds exactly that
transaction with fully canonical offsets, and then shows the same instruction succeeding on
its own to prove this program's check is the only thing in the way.

**`raise_dispute`** (`src/instructions/raise_dispute.rs`). The checks execute in the order
DESIGN-v2.md numbers them, confirmed by reading the handler top to bottom: signature
verification, then the signing key against the marketplace's current or rotated-away signer,
then domain, program ID, chain tag, receipt expiry and the permit's frozen complaint window,
then every party named in the receipt against the accounts passed, then the revocation rule,
then the claim against the order's value, then the record's creation as the replay guard, then
the counters and the bond. Fourteen accounts, all data accounts boxed to stay inside the 4KB
stack frame.

**`resolve_dispute`** (`src/instructions/resolve_dispute.rs`). `status == Open` is the first
check, because without it a never-closed record is resolvable repeatedly, draining other
buyers' bonds out of the shared pool and underflowing `open_disputes` past the release gate.
Upheld pays `min(claim, permit remaining, staked)` and drops `committed` and `staked`
together through `SellerStake::slash`, which is the pairing that keeps `committed <= staked`
true across a slash. Rejected moves the bond into the seller's vault, the only path where a
seller's free balance grows without a deposit.

## Controls in place

| Control | Where | Note |
| --- | --- | --- |
| Checked arithmetic | 29 `checked_*` call sites | no `saturating_*` on any balance; the only one in the program is on a byte offset that is bounds-checked immediately |
| Basis-point maths | `bond_for` | casts to `u128`, multiplies before dividing, rounds the bond up so a small claim cannot buy a free complaint |
| Account binding | every handler | seeds re-derived from stored fields, `has_one` for live keys, no account left unbound |
| Mint binding | every token handler | bound to the vault being moved, `transfer_checked` throughout |
| Conservation | `add_stake`, `withdraw_stake`, `resolve_dispute` | asserted at runtime, not only in tests |
| Replay | `DisputeRecord` PDA | `init` failure is the guard; deletion waits for the receipt to age out |
| Domain separation | receipt `domain`, `program_id`, `chain_id` | a signature from another context cannot be read as a receipt |
| Events | 19 `emit_cpi!` events | buyer history is derived from these, so `emit_cpi!` rather than `emit!` |

Coverage as of Phase 3: 114 tests passing with no warnings, every one of the nineteen
handlers exercised, and every one of the 33 error variants asserted by name somewhere in the
suite. `raise_dispute` measures 953 bytes of the 1,232-byte transaction limit and 42,229 of
200,000 compute units.

## Where to look hardest

1. **The Ed25519 introspection module.** Highest consequence in the program: a weakness
   forges marketplace receipts with no key compromise and defeats every other protection at
   once. It is also the most heavily tested, which is not the same as being right.
2. **The shared bond pool.** One vault per marketplace holds every live bond, and its
   solvency rests entirely on the discipline that only the amount recorded on a record ever
   moves. Nothing onchain can check the pool against its records.
3. **Concurrency against one permit.** Two complaints raised before either resolves both pass
   the raise-time checks against the same undecremented balance; the cap holds because
   resolution recomputes the remainder. Worth re-deriving for any new path that reads
   `max_slashable - slashed`.
4. **Deployment facts that the code cannot enforce.** `INITIAL_ADMIN` is a compiled-in
   pubkey, `collateral_mint` and `chain_id` are unchangeable after `initialize_config`, and
   the upgrade authority is whatever the deploy set. A wrong value in any of them is a
   redeploy, not a fix.
5. **The absence of CI.** Nothing prevents a stale `.so` from being tested against, which is
   the failure mode TESTING.md calls out as biting hardest exactly here.

## Open questions for the next reader

- Is `bond_bps = 0` intended to be a legal marketplace setting? `validate_marketplace_settings`
  bounds only the ceiling, so a marketplace can charge no bond at all, which removes the one
  cost a frivolous complaint carries. Deliberate or an unstated floor?
- Should `raise_dispute` be reachable through CPI? Phase 3 decided no and enforces it, on the
  strength of TESTING.md naming a CPI-wrapper test and audit finding TS-25. The design doc's
  numbered check list does not mention it, and the restriction costs future composability.
- Who holds the `INITIAL_ADMIN` key, and is the deployed program's upgrade authority already
  the Squads multisig the README claims? Neither is answerable from the source.
- The event log is the only durable record of a closed dispute. No indexer exists yet, and
  `getProgramAccounts` with unindexed `memcmp` is the only alternative. Who runs it?
