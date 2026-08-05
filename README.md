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

- Collateral is real money, locked in a program-owned account.
- A slash moves that money to the wronged buyer automatically.
- The incident count lives on the seller's stake account. Walking away means
  abandoning the collateral and starting from zero — which is the point.

Solana specifically, because a trust layer only works if checking and updating
it is effectively free. At sub-cent fees this can run on every transaction
rather than a sampled few.

## How it works

Three accounts, four instructions.

| Account | Seeds | Holds |
| --- | --- | --- |
| `Config` | `["config"]` | The arbiter authorized to resolve disputes |
| `SellerStake` | `["stake", seller]` | Staked lamports + permanent `disputes_lost` count |
| `Dispute` | `["dispute", seller, buyer]` | An open claim and its amount |

| Instruction | Signer | Effect |
| --- | --- | --- |
| `initialize_config` | arbiter | Sets the dispute arbiter. Runs once. |
| `create_stake(amount)` | seller | Locks `amount` lamports as collateral |
| `raise_dispute(claim)` | buyer | Opens a claim, capped at the seller's stake |
| `resolve_dispute(uphold)` | arbiter | If upheld: slashes to the buyer, increments `disputes_lost` |

The stake PDA holds the collateral directly on top of its rent-exempt balance,
so a slash only ever moves staked lamports and never risks the account itself.

## Build and test

```bash
anchor build
cargo test
```

Tests run against [LiteSVM](https://github.com/LiteSVM/litesvm) in-process and
cover the three paths that matter: an upheld dispute slashes the stake, a
rejected dispute leaves it untouched, and a non-arbiter cannot resolve
anything.

## Deploy

```bash
solana config set --url devnet
anchor deploy
```

## Prototype scope

This is a hackathon prototype, and these are deliberate omissions rather than
oversights:

- **The arbiter is a single key.** A real deployment needs a multisig, a DAO
  vote, or staked jurors — a single arbiter is a trusted party, which is
  exactly what the design otherwise avoids.
- **Collateral is native SOL.** USDC is the sensible asset for real sellers;
  SOL keeps the prototype free of token-account plumbing.
- **One open dispute per buyer/seller pair**, and no appeal, expiry, or
  partial-refund flow.
- **No unstaking instruction.** A seller cannot yet withdraw collateral after a
  clean run.

## Credit

The problem statement comes from ONDC India, published on
[Superteam's idea board](https://superteam.fun/build/ideas/reputation-based-slashing).
The design and implementation here are original.
