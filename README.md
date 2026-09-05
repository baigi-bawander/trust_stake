# TrustStake

Staked reputation for peer-to-peer marketplaces, on Solana.

Sellers lock collateral before they list. Buyers can file a claim against that
collateral. If an arbiter upholds the claim, the collateral is slashed and paid
to the buyer, and the loss is recorded on the seller's account permanently.

## The problem

On peer-to-peer marketplaces — OLX, Facebook Marketplace groups, WhatsApp
selling — a buyer has no reliable way to tell whether a seller will actually
deliver. Reviews can be faked or deleted, new sellers have no history at all,
and a seller who scams someone can abandon the account and start again with a
clean profile. Nothing is at stake, so nothing backs the promise.

This is not hypothetical. ONDC — India's government-backed open commerce
network, projected at $80B+ of commerce across 15M+ sellers — 
[published this exact gap as an open problem](https://superteam.fun/build/ideas/reputation-based-slashing),
and its own ecosystem research still lists reliable dispute resolution as
unsolved.

## Why a blockchain helps here

The useful property is not "decentralization" in the abstract — it is that the
seller's incentive is enforced by something the marketplace cannot quietly
reverse and the seller cannot delete:

- Collateral is real money, locked in a program-controlled vault.
- A slash moves that money to the wronged buyer automatically.
- The incident count lives on the seller's stake account — both how many
  disputes it has faced and how many it has lost. Walking away means
  abandoning the collateral and starting from zero — which is the point.

Solana specifically, because a trust layer only works if checking and updating
it is effectively free. At sub-cent fees this can run on every transaction
rather than a sampled few.

## How it works

5 account types, 19 instructions, grouped by what they act on: config/admin,
marketplace, stake, permit, dispute.

**Accounts**

- `Config` — `["config", "v2"]`. One per deployment: the protocol admin, the
  collateral mint pinned forever at `initialize_config`, and a chain-ID tag
  checked against every receipt.
- `Marketplace` — `["market", "v2", marketplace_id]`. One per tenant: its
  receipt-signing key, its arbiter, and the complaint window and bond rate it
  freezes onto every permit it grants. Owns a `bond_vault` token account
  (`["bonds", "v2", marketplace]`) that holds live dispute bonds.
- `SellerStake` — `["stake", "v2", seller]`. One per seller, shared across
  every marketplace they sell on: how much is staked, how much is committed
  against open permits, and the seller's `disputes_total` and `disputes_lost`
  counts. Owns a `stake_vault` token account (`["vault", "v2", seller]`) that
  holds the seller's actual collateral.
- `SlashPermit` — `["permit", "v2", seller, marketplace]`. A seller's
  bounded, revocable line of credit to one marketplace: a maximum slashable
  amount, the amount already slashed, and the complaint window/bond rate
  frozen at the moment it was granted.
- `DisputeRecord` — `["dispute", "v2", marketplace, seller, order_id]`. One
  open or resolved complaint against one order: the claim, the bond
  deposited, and its expiry and closing timestamps. Its address is also the
  replay guard — a second complaint against the same order collides with the
  same PDA.

**Instructions**

- **Config/admin** — sets the protocol's collateral mint and admin key once,
  with a two-step authority handoff after that: `initialize_config`,
  `propose_config_authority`, `accept_config_authority`.
- **Marketplace** — registers a tenant and lets its authority update settings,
  with the same two-step handoff: `register_marketplace`,
  `update_marketplace`, `propose_marketplace_authority`,
  `accept_marketplace_authority`.
- **Stake** — creates a seller's shared collateral account and moves tokens in
  and out of it: `initialize_stake`, `add_stake`, `withdraw_stake` (gated so a
  withdrawal can never drop staked collateral below what's committed to open
  permits).
- **Permit** — grants, raises, revokes and releases a seller's bounded
  exposure to one marketplace: `grant_permit`, `increase_permit`,
  `revoke_permit`, `release_permit`, `release_permit_early`.
- **Dispute** — opens a complaint backed by a marketplace-signed receipt,
  verified by the Ed25519 precompile in the same transaction rather than by
  the program trusting a bare claim: `raise_dispute`. That marketplace's own
  arbiter decides it: `resolve_dispute`. If nobody decides, `expire_dispute`
  frees the seller 30 days after the complaint is filed — as late as the
  complaint window plus 30 days (up to 60) after the order itself, since a
  receipt stays filable for the whole window; once a decided dispute's
  receipt is too old to reuse, `close_dispute` reclaims its rent.

Collateral lives in a separate token vault (`stake_vault`), not on the
`SellerStake` account itself, so a slash only ever moves vault tokens and
never touches the account's own rent-exempt balance.

## Documentation

- [`docs/DESIGN-v2.md`](docs/DESIGN-v2.md) — the full design: the account model, every
  instruction handler, the offchain receipt format, integration requirements for a
  marketplace, and the honest limitations. Read this before integrating a marketplace or
  questioning a design decision above.
- [`docs/TESTING.md`](docs/TESTING.md) — the test plan behind the 152-test suite, plus the
  live devnet transaction signatures proving it runs onchain and not only in LiteSVM. Read
  this before adding a test or trusting a "this should already be covered" claim.
- [`docs/SECURITY-CONTEXT.md`](docs/SECURITY-CONTEXT.md) — architectural reconnaissance
  written for whoever audits the program next: what each control defends against and where
  the weak spots are, without reporting findings of its own. Read this before starting a
  security review.
- [`docs/AUDIT-v2-FINDINGS.md`](docs/AUDIT-v2-FINDINGS.md) — 57 findings from three
  independent adversarial reviews of the design, run before any code was written. Read this
  to see what a design-only review caught and why some of the account model exists.
- [`docs/IMPLEMENTATION-FINDINGS.md`](docs/IMPLEMENTATION-FINDINGS.md) — findings from a
  sharp-edge sweep over the built program, each one reproduced against the compiled program
  rather than inferred from reading. Read this for what building the design turned up that
  reviewing it on paper did not.

## Build and test

```bash
anchor build
cd programs/truststake && OPENSSL_NO_VENDOR=1 cargo test
```

`anchor build` has to run first: tests load the compiled
`target/deploy/truststake.so` through LiteSVM rather than compiling the
program natively, so a handler change needs a rebuild before the tests see
it. `OPENSSL_NO_VENDOR=1` works around a vendored-OpenSSL build failure on a
clock-skew check in this environment.

152 tests run against [LiteSVM](https://github.com/LiteSVM/litesvm) in-process, organized
by who's attacking: a malicious seller trying to withdraw committed collateral or evade a
slash, a malicious buyer forging or replaying a receipt, a malicious marketplace trying to
exceed its own permit or freeze a seller forever, an outsider attempting PDA substitution,
and the Ed25519 signature-introspection check itself (the single most exploitable surface in
the design). A shared invariant check runs after every instruction handler in every test —
vault balances match their ledgers, `committed <= staked` always holds, and no collateral is
created or destroyed anywhere in the suite. A separate 28-test suite,
`OPENSSL_NO_VENDOR=1 cargo test --example devnet_demo --features devnet_demo`, covers the
devnet demo script's own idempotency-guard decisions instead of the program.

v2 is deployed on devnet at `3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2`; see
[docs/TESTING.md](docs/TESTING.md) for the full test plan and the live devnet transaction
signatures proving it runs onchain, not only in LiteSVM.

## Deploy

```bash
solana config set --url devnet
solana program deploy --program-id 3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2 \
  target/deploy/truststake.so
```

Never `anchor deploy` — it defaults to `--program-keypair target/deploy/truststake-keypair.json`,
which deploys a brand-new program at a different address and spends devnet SOL without
touching the program actually live at the address above. The command shown upgrades that
program in place, authorised by `--upgrade-authority` (defaults to the configured keypair,
`solana config get`).

The upgrade fails if the new binary no longer fits the program account's current
allocation. Compare the size of `target/deploy/truststake.so` against the `Data Length`
reported by `solana program show <program-id> --url devnet`; if the binary is larger, extend
the account by the difference first, with headroom so the next build doesn't need this again:

```bash
solana program extend 3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2 <difference-plus-headroom>
```

A program account can never shrink, so that rent is permanent. Deploying costs
real devnet SOL — program rent-exemption runs roughly 6,960 lamports per byte of the compiled
`.so` — so check `solana balance --url devnet` first. If a deploy fails partway, check
`solana program show --buffers --url devnet` before retrying; there may be a paid-for buffer
account worth resuming from instead of paying rent again from scratch.

The IDL is a separate step, after any deploy:

```bash
anchor idl upgrade 3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2 \
  -f target/idl/truststake.json --provider.cluster devnet
```

Never `anchor idl init` — the IDL account already exists, and `init` fails against an account
that's already allocated. **This upgrade command has failed on every attempt since
2026-08-31**: the public devnet RPC has silently truncated the uploaded buffer rather than
rejecting it outright, three times in a row (see CLAUDE.md's IDL entry for the full
diagnosis and the abandoned buffers it left behind). The IDL currently published dates from
2026-08-24 and is safe to keep using in the meantime — verified by fetching and decompressing
it and diffing against `target/idl/truststake.json`: every instruction, account and shared
error code matches, and it differs only by lacking names for two error codes added since and
one stale doc string, none of which changes how anything decodes.

## Current tradeoffs

Some of what's below reads like a gap. It's a deliberate design choice, not an oversight —
see [docs/DESIGN-v2.md](docs/DESIGN-v2.md) for the complete list and the reasoning behind
each:

- **No independent arbitration.** Each marketplace's own arbiter judges its own disputes, up
  to the seller's permit cap. The arbiter is only ever checked as "is this the expected
  signer," never assumed to be a wallet, so a future jury program can occupy that slot later
  with no forced migration.
- **No buyer reputation stored onchain.** A buyer identity costs nothing to abandon, so buyer
  history is computed from onchain events instead of stored in an account.
- **No KYC or identity data, ever.** Everything written to Solana is public and permanent;
  that verification stays inside a marketplace's own systems.
- **No leverage multiplier.** Backing above a seller's collateral is only safe once
  reputation is expensive to fabricate, which it is not yet.
- **A permit caps total damage across every buyer on one marketplace, not per-buyer
  coverage.** Keeping order volume in line with a seller's live permit is the marketplace's
  job, published as an integration requirement.
- **A marketplace can fabricate a seller's dispute counters in both directions**, since it
  signs its own receipts and judges its own disputes — `disputes_total` and `disputes_lost`
  are marketplace-reported, not attack-resistant on their own.
- **No appeals, no partial refunds, no protocol fee, no frontend.**

## Credit

The problem statement comes from ONDC India, published on
[Superteam's idea board](https://superteam.fun/build/ideas/reputation-based-slashing).
The design and implementation here are original.
