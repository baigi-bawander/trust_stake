# Security Policy

## Status

TrustStake is a devnet-only prototype. No user funds are at risk: the collateral mint on devnet is a test mint created for this deployment, not real USDC, and the program has never been deployed to mainnet.

## Scope

In scope: the TrustStake program at `3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2` on devnet, and the source under `programs/truststake/src/`. Out of scope: the `cpi_wrapper` test fixture, `examples/devnet_demo.rs`, and anything about the devnet test mint's own authority.

## Internal review

No external audit exists. What has been done instead: three independent design-stage reviews, run blind to each other before any code was written; three per-phase security reviews of the built program; and two dedicated passes over the dispute lifecycle, one of them adversarial, executing real exploit probes against the compiled program in LiteSVM. Together these produced 61 findings, and six cross-phase bugs were found and fixed afterward, none of them visible to any single-phase review. 152 tests pass today. All 24 Critical and High findings marked fixed, plus 3 marked superseded, were independently re-verified against program source on 2026-09-10.

None of this is a substitute for an external audit. An audit should start with `src/ed25519.rs` (the most exploitable surface in the design), cross-phase interaction between the collateral-lifecycle and dispute handlers (where all six bugs above were found), and PDA seed coverage. See [docs/AUDIT.md](docs/AUDIT.md) for the full findings and scope.

## Reporting a vulnerability

Report privately through GitHub's Security tab rather than a public issue:

1. Go to the repository's **Security** tab.
2. Click **Report a vulnerability** to open a private GitHub Security Advisory.
3. Describe the issue, the affected instruction handler(s) or account(s), and, if possible, a test case that reproduces it.

Non-sensitive issues (build failures, documentation errors, test flakiness) can go through a normal GitHub issue instead.

## No bounty program

There is no bug bounty. This ties directly to the status above: a devnet prototype holding no real funds has nothing to fund a bounty with. This would be revisited before any mainnet launch.

## Design invariants, not bugs

A few things look like gaps and are deliberate:

- `Marketplace.bond_bps` has a ceiling but no floor. Zero is legal on purpose: a floor would price out honest buyers with small claims.
- `Marketplace` keeps a single `prev_receipt_signer` slot, not a ring. A second key rotation inside one complaint window overwrites the first; a cooldown was considered and rejected because it would block the emergency the rotation mechanism exists to handle.
- The vault conservation check is `>=`, not `==`. Exact equality let anyone brick a seller's vault by donating one token unit into it; the dangerous direction, a vault holding less than its ledger, is still caught.

See [CLAUDE.md](CLAUDE.md), "Deliberate tradeoffs on v2, not bugs," for the rest.

## Depth

- [docs/SECURITY-CONTEXT.md](docs/SECURITY-CONTEXT.md): architectural reconnaissance written for whoever audits the program next.
- [docs/AUDIT.md](docs/AUDIT.md): every finding, its severity, and its status.
