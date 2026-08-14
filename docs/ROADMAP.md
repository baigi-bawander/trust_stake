# TrustStake — grant & hackathon roadmap

Research date: 2026-08-10. Written after reading what Solana Foundation, Superteam,
and Colosseum actually publish about how they evaluate. Revisit the funding calendar
if this is more than ~2 months stale.

## The headline finding

**Technical completeness is not what's blocking this project. Evidence of use is.**

Superteam's grant listings publish an explicit heuristic ladder:

> Proof-of-Work > Potential
> Active Links > Concepts/Ideas
> Product Feedback > Market Thesis
> Unique actionable insights > Opinions from X
> Working Product > MVP >>> PRDs > Pitch Decks

and state plainly what they reject: *"Applications that only have a good idea, a basic
landing page, or a pitch deck without working links/active users are not an ideal fit."*

Colosseum's winners post describes what judges scored: *"execution speed, insight,
founder-market fit, prioritization, and overall talent to potentially build an enduring
crypto startup."* Superteam Canada's guide to winning Colosseum reduces it to "working
demo > perfect architecture," "deployed, not localhost," "clear problem > impressive tech."

Nobody in this list asks for an audit, a DAO, or formal verification at this stage.
So the audit-readiness / staked-juror-arbitration branch of the original question is
real engineering but the wrong thing to spend the next two months on.

Two technical gaps made the product unusable by a stranger: **you cannot get your money
back, and there is nothing to click.** The first is resolved in the v2 design; see
[DESIGN-v2.md](DESIGN-v2.md), decision 8. The second is still open and is the subject of
1.1 below.

## Reframe the pitch first (costs nothing, changes everything)

Binance P2P already runs this exact mechanism at scale: merchants post a security
deposit, it is frozen while they trade, Binance deducts from it when a dispute is
decided against them, and it is returned within 14 business days if their record is
clean. That is not a competitor problem — it is the strongest validation the model has.
Cite it.

The differentiator is what a centralized platform structurally cannot offer:

- Binance's deposit is **custodial** — Binance holds it and decides alone.
- It is **platform-locked** — it buys you nothing on OLX, Daraz, or a Solana marketplace.
- It is **erasable** — leave the platform and the record leaves with you.

TrustStake's actual product is **portable seller collateral with a loss record the
seller cannot leave behind.** Stake once; any marketplace can read the stake and the
`disputes_lost` count before letting you list. That is a one-sentence pitch, it is
Solana-specific (only worth doing if reading the record is free), and it survives the
"why not just use a database" question. The current README's framing — trust for P2P
marketplaces generally — is true but doesn't say why it can't be a platform feature.

## Priority 1 — make it usable (this is the whole game)

Withdrawal and USDC collateral both moved into the program rewrite and are specified in
[DESIGN-v2.md](DESIGN-v2.md), decisions 6 and 8. What remains on this roadmap is the part
the program cannot provide.

### 1.1 A deployed frontend with a public URL
"Active Links > Concepts/Ideas" is a literal published criterion, and Colosseum's
guidance is that judges must understand the project in 60 seconds. This is the single
highest-leverage item once the program is built: it converts every existing piece of work
into something an evaluator can experience instead of read about.

Build one screen first, not four: **a read-only seller lookup.** Paste an address, see the
collateral, each marketplace's permit listed separately, the free balance, and both the
seller's and each marketplace's record. It needs no wallet connection, no transaction
building and no user funds, which removes most of the work and nearly every way a Solana
frontend goes wrong. It is also the screen used on every transaction rather than
occasionally, and the only one that shows portability in a single picture.

It must list permits separately and never show a combined "backed by" figure. A seller with
$300 staked and a $150 permit at this marketplace is backed by $150 here, and showing $300
is false in the direction that matters.

Staking, filing and resolving are better shown as a recorded walkthrough of
`examples/devnet_demo.rs` with real transaction links, which satisfies Colosseum's
technical-demo requirement without building a transactional UI.

## Priority 2 — credibility, cheap (a few days total)

### 2.1 Squads multisig on the protocol authority and the upgrade authority
v2 moves arbitration to each marketplace's own arbiter key, so there is no longer a global
arbiter to point a multisig at. What remains worth doing, and is still a deployment change
rather than an architecture project, is pointing the `Config` authority and the program's
upgrade authority at a Squads multisig, and saying so in the README. It also improves the
legal posture below.

### 2.2 Emit a Solana Attestation Service attestation on `resolve_dispute`
SAS is the Foundation's own credential primitive, live on mainnet since May 2025.
Writing the outcome as an attestation means any Solana app can read a seller's record
without integrating TrustStake's program at all. That is precisely the Solana Foundation
grant criterion — *"a significant open-source contribution"* — and precisely what
Colosseum's Public Good Award ($10k, won this year by Zoneless) is for. Small work,
disproportionate narrative payoff.

### 2.3 Threat model + invariant tests, not an audit
No grant program at this stage funds or expects a pre-traction audit. What reads as
rigor for free: a written threat model (what a malicious seller / buyer / arbiter can
each attempt, and what stops them), plus fuzz or invariant tests asserting that staked
lamports are conserved and the PDA never drops below rent-exemption. Costs days, not
dollars, and answers the security question an evaluator would actually raise.

## Priority 3 — the real bottleneck: evidence

Everything above is still supply-side. The grant rubric weights *"Product Feedback >
Market Thesis"* and rejects working products with no users. Two things to run in
parallel with the engineering:

- **One design-partner marketplace.** Not a signed contract — a named operator who has
  seen the demo and said what would have to be true for them to try it. See the legal
  note before choosing who.
- **Ten real conversations** with P2P sellers and buyers, written up. What would they
  stake? What's the collateral level where they'd trust a stranger? What breaks?
  This is the cheapest item on the list and the one most likely to be skipped.

## Explicitly deprioritized

DAO or staked-juror arbitration, appeals, expiry, partial refunds, a security audit,
mainnet deployment, and any token design. Each is defensible eventually; none of them
moves an evaluator in the next two months, and the README already frames them as
deliberate scope rather than gaps.

## Legal constraint (real, and it shapes GTM)

Pakistan's **Virtual Assets Act, 2026** made PVARA a permanent licensing authority.
Unlicensed virtual-asset service provision carries fines up to PKR 50 million and up to
five years' imprisonment; unauthorized virtual-asset offerings or promotion carry a
separate penalty. PVARA is currently issuing No Objection Certificates with the full
licensing framework still to come.

Practical implications, in rough order of importance:

1. **Do not make Pakistani retail P2P sellers the first customer.** A consumer-facing
   product that takes Pakistani sellers' money and pays it to Pakistani buyers is the
   fact pattern this law is aimed at. Target crypto-native marketplaces where users
   already hold assets and no fiat on-ramp is involved.
2. **Keep it structurally non-custodial and say so.** Collateral sits in a program-owned
   PDA. Point the arbiter multisig at *the marketplace operator*, not at you — you ship
   the program, you do not decide where anyone's money goes.
3. **Get local counsel before any mainnet deployment touching Pakistani users.** Not
   before a devnet demo, a grant application, or a hackathon submission.

This is also a genuine pitch asset rather than only a risk: "regulated market, so the
protocol is non-custodial and the operator holds arbitration" is a more sophisticated
answer than most hackathon submissions give.

## Funding calendar

**Verified: there is no Superteam Pakistan grant.** As of 2026-08-10 the Superteam
grants API lists 46 open grants across 21 regions — India, UK, Nigeria, UAE, Turkey,
Ukraine, Kazakhstan and others — and **none for Pakistan**. The chapter exists (it ran
an orientation) but has no funding listing. Do not build the plan around it; do ask in
the Pakistan chapter where Pakistani builders are meant to route applications.

Actual paths, best first:

| Path | Amount | Timing | Fit |
| --- | --- | --- | --- |
| **Colosseum Eternal** | $25k Eternal Award (2×/yr) + $250k pre-seed accelerator | 4-week sprint, start any time | **Best fit.** Designed for already-built startups; Colosseum's own copy points existing projects here rather than at hackathons. Requires weekly video updates, then product description, GitHub repo, pitch deck, technical walkthrough. |
| **Colosseum fall hackathon** | $30k grand / $10k top-20 / $10k public good | Sept 28 – Nov 2, 2026 | Strong, with a caveat — confirm in the FAQ whether a pre-existing project qualifies. Requires GitHub repo, ≤3-min pitch video, technical demo video. |
| **Solana Foundation grants** | Milestone-based, varies | Rolling, ~1 week triage / ~3 weeks to decision | Good. Frame as public good: open source, free, plus the SAS contribution. Needs a milestone budget with real numbers. |
| **Agentic Engineering grant** | $200 USDG, Global, open | Apply now | Small but strictly positive: 50% up front, 50% on shipping a live working Solana product. Cheap proof-of-work, and the payout condition is exactly the frontend you need to build anyway. |
| Superteam Earn bounties | varies | ongoing | Not funding for TrustStake — proof-of-work for the grant application. The rubric weights "bounties won/participated in." |

### Suggested sequence

The fall hackathon is ~7 weeks out, which is the natural anchor.

- **Now:** apply for the $200 Agentic Engineering grant; reframe README around portable
  collateral; start the ten user conversations.
- **Mid-Aug → mid-Sept:** run a Colosseum Eternal 4-week sprint delivering the v2 program
  build, then Priority 1 (the lookup page), with weekly updates. The sprint deadline is the forcing
  function; the weekly updates are also visibility with judges.
- **Mid-Sept:** Priority 2 (multisig, SAS, threat model). Decide hackathon-vs-Eternal
  based on the FAQ answer on pre-existing projects.
- **Sept 28 – Nov 2:** hackathon if eligible. Either way, the Solana Foundation
  application goes in once there's a live URL and at least one design partner.

## Open questions to resolve

- Does Colosseum's fall hackathon accept pre-existing projects, or must the work be
  built in the window? Load-bearing for the sequence above; answer is in their FAQ.
- Where do Pakistani builders route grant applications given no Pakistan listing —
  Solana Foundation direct, or an adjacent chapter?
- Which marketplace is the design partner? Until that has a name, Priority 3 is stalled.
