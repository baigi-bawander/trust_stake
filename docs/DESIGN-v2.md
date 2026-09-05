# TrustStake v2: from prototype to protocol

*Note, 2026-08-25: this is a design document, written before the v2 rebuild it specifies. It
records intent, decisions and predictions made at the time; some of its forward-looking
estimates differ from what was actually built. For current facts, see README.md,
docs/TESTING.md and CLAUDE.md.*

## What TrustStake is

A seller locks USDC into an account the program controls. For each marketplace they sell
on, they sign a permit capping what that marketplace may take from it. When a buyer is
scammed, the marketplace decides the complaint and the program moves money from the
seller's collateral to the buyer, up to the permit's cap and never past it.

One pot of collateral backs a seller across every marketplace they work on, and the loss
record travels with them. That is the whole product.

**TrustStake is for trades where at least one side cannot be escrowed.** You can escrow
bitcoin. You cannot escrow a used phone, a bank transfer, a bag of cash, or a delivered
service. Binance escrows the crypto leg of a P2P trade and *still* requires cash merchants
to post a deposit, because the cash leg cannot be escrowed. That gap is where this sits.

## Where the code is today

*Note, 2026-08-25: this section describes the state of the code before this document's
rebuild, which is what motivated writing this document in the first place. The rebuild is
now complete, and the program ID below runs v2, not the prototype described here. See
README.md and docs/TESTING.md for the current account model and test count.*

A working Anchor prototype on devnet (`3Vc6M8Az9h2GtDmqqhQKURqTKKygNfekQq7PoJris6V2`):
three accounts, four instruction handlers, 3/3 LiteSVM tests passing. The Rust is clean.
The mechanism is not a product:

- `Config` is a singleton (`seeds = [b"config"]`), so one arbiter judges every dispute on
  earth, forever.
- Anyone can file a claim against any seller for free. No proof a purchase happened.
- `create_stake` uses `init`, so a seller slashed to zero can never re-stake. Their account
  is permanently dead.
- `disputes_lost` has no denominator, nothing emits events, and arithmetic is unchecked.

This plan rebuilds it as a multi-tenant protocol with portable seller collateral.

**Scope for this build:** the program and its tests. The devnet demo exercises two
marketplaces with different settings, because one marketplace cannot demonstrate
portability. A frontend is out of scope and is not part of this document's deliverable.

---

## The design, and why

| # | Decision | Why |
| --- | --- | --- |
| 1 | **Shared collateral, delegated slashing.** One global `SellerStake` per seller. Marketplaces register as tenants. The seller signs a `SlashPermit` capping what each may take. | The only shape that delivers portable cross-marketplace reputation. Per-marketplace stakes re-implement Binance's merchant deposit N times, and force a seller to lock N deposits to sell in N places. |
| 2 | **The marketplace is trusted, up to the seller's cap, and the document says so.** It signs the receipts and appoints the arbiter, so it can draw the full permit at will, in two transactions, without breaking a rule. | This is true whether or not it is written down, so it gets written down. A permit is a bounded, revocable, publicly recorded line of credit the seller extends to a marketplace. Receipts and bonds constrain strangers and chancers, not the party that issues receipts and judges complaints. Claiming otherwise is the fastest way for a reviewer to lose trust in the rest of the design. |
| 3 | **The cap is the whole protection.** `committed <= staked`, checked at grant time and at withdrawal time, and preserved on a slash by lowering both figures together. No leverage, no rate limit, no second mechanism. | Following from decision 2, one sentence is completely true: *you can never lose more than the number you set.* A list of partial protections reads as stronger than a single real one and is worth less. The seller chooses the number, the program enforces it, and nothing else claims to help. |
| 4 | **Disputes require a marketplace-signed order receipt.** | Gives the program a reason to believe a purchase happened, caps the claim at the real order amount, and makes a stranger unable to touch a seller's collateral. It also bounds spam far better than any bond does: filing 100 complaints requires 100 real purchases. |
| 5 | **Receipts live offchain, verified onchain only when disputed**, via the Ed25519 precompile plus instruction introspection. | Zero cost and zero transactions per normal order. A marketplace integrates by signing a struct in its own backend: no wallet, no SOL, no transaction pipeline. Integration cost is what kills infrastructure products. It also lets a buyer file without the marketplace's cooperation, which is what makes an abandoned complaint publicly visible (decision 7). |
| 6 | **USDC collateral, with the mint pinned by address in `Config`.** Token code is written against `anchor_spl::token_interface` and uses `transfer_checked`, so it runs unchanged against either token program. | Decision 4 introduced fiat-denominated claims; mixing those with SOL collateral forces a price oracle. The risk to close is a mint carrying a transfer fee, which would make the recorded `staked` disagree with the vault's real balance. Pinning one mint by address closes that completely, since one address is one fee policy, and it is stronger than pinning the token program while leaving the door open to a Token Extensions stablecoin later. |
| 7 | **Complaints are scoped to the permit, and expire.** An open complaint freezes only the collateral committed to that one marketplace. Once raised, a complaint undecided after 30 days may be expired by anyone: nobody is paid, the bond returns in full, the freeze lifts, and the **marketplace** takes a permanent public mark. That 30-day clock starts at filing, not at the order, and a complaint may be filed as late as the end of the complaint window, so a seller's real worst case, from order to a guaranteed-lifted freeze, is the complaint window plus 30 days, up to 60. | All three audit reviewers found the previous version independently. A global open-complaint counter meant the smallest, most forgotten marketplace a seller ever signed up with could freeze their entire collateral forever, for the price of one filing. Expiry closes the liveness hole that appears with no attacker at all: a marketplace that shuts down, loses a key, or ignores its queue. Expiring in the buyer's favour was rejected because it hands every genuine buyer a free option to keep the goods and reclaim the money whenever a marketplace is slow. |
| 8 | **Two-speed exit, and no withdrawal cooldown.** Collateral above `committed` withdraws immediately. Committed collateral frees only by revoking a permit and waiting out that permit's complaint window, with a cooperative fast path that still enforces a minimum wait of `CLOCK_SKEW_TOLERANCE_SECONDS` after revocation. | Collateral above `committed` is claimable by nobody, so holding it protects no one. Committed collateral is exactly what backs outstanding orders, and releasing it before the complaint window closes is a clean exit scam requiring no race. Splitting the two makes the separate cooldown redundant: revocation plus one complaint window is the only timer the system needs. The fast path's own minimum wait is not optional either: without it, a revoke-release-regrant cycle can complete in minutes, and `raise_dispute` check 7's clock-skew allowance would then accept a receipt from the just-closed era against the freshly granted one. |
| 9 | **Buyer posts a refundable bond** (default 10%, marketplace-set, protocol ceiling 20%). Upheld returns it with the payout; rejected forfeits it to the seller. Complaint records are deleted and their rent refunded once the receipt is too old to reuse. | The bond's real job is narrow and worth having: it stops a buyer who received the goods from filing anyway on the chance of a refund. The rent refund matters more than it sounds. Permanent records made filing cost roughly $0.35 on top of the bond, which on a $20 order is most of a 12% haircut just to raise a hand, and small orders are the entire target market. A receipt cannot be reused once its complaint window has passed, so the record has no job left after that and can go. |
| 10 | **A permit's money terms are frozen when the seller signs it; the marketplace's keys stay live.** Cap, complaint window and bond rate are copied onto the permit and never change. The receipt signer and arbiter are read live from the marketplace account. | Freezing the money terms stops a marketplace rewriting the deal after the fact, for example stretching the complaint window to trap a departing seller. Freezing the *keys* would break ordinary operations: a staff change would strand every pending complaint and force every seller to re-sign. Under decision 2 the marketplace can already sign any receipt and rule any way it likes, so changing which key does that grants it nothing new. |
| 11 | **A marketplace is identified by an immutable ID, not by its authority key.** Keys become ordinary fields, changeable through a two-step transfer. | Seeding the PDA by the authority key makes key rotation structurally impossible: a lost or stolen key forces re-registration, which discards the dispute history and orphans every seller's permit, since each permit names the old account. Two of the three reviews flagged this independently. |
| 12 | **The complaint window is per-marketplace, between 2 and 30 days**, chosen at registration and frozen onto each permit. | A cash-trading marketplace where disputes surface in minutes and a shipped-goods marketplace where a parcel takes a week need genuinely different numbers, and forcing both to one value is worse for both. The floor stops a marketplace setting a window so short that complaints are impossible; the ceiling stops one trapping a seller's collateral indefinitely. |
| 13 | **No daily slashing cap, and no protocol revenue.** | A cap does not bound theft, it delays it: the thief writes the receipt, so the thief picks the amount, and a per-day limit costs them one day during which nobody has a lever to pull. Meanwhile it shortchanges the one honest buyer whose claim exceeds the daily allowance. Every payout already emits an event, so the cap adds no detection either. Taking a cut of forfeited bonds was rejected on the same principle: the position worth protecting is that this program ships rules and never decides where anyone's money goes. |
| 14 | **Build the whole architecture; demo two marketplaces.** | Portability is the differentiator, and a single-marketplace demo cannot show it: what a reviewer sees is a seller deposit system, which Binance has had for years. Two marketplaces are the minimum that demonstrates portability, permit-scoped freezing, and per-marketplace settings. Three would be noise. |

*Note, 2026-09-05: a third marketplace, SwiftMarket, was added to the devnet demo on
2026-08-31 (`examples/devnet_demo.rs` stage 1), registered at a 30-day window and a 0 bps
bond -- the far end of the legal range in both dimensions, next to CashDesk's 2-day/1,000 bps
and PixelBazaar's 7-day/300 bps. This does not overturn decision 14: portability was already
demonstrated by two marketplaces sharing one stake, and a third adds nothing to that
argument. What it does add is live coverage of `MAX_COMPLAINT_WINDOW_SECONDS` and the legal
0 bps bond rate (CLAUDE.md's deliberate-tradeoffs entry on `bond_bps` having a ceiling but no
floor), neither of which the original two marketplaces' settings reached. A third
marketplace added for its settings, not for a third data point on portability, turned out not
to be the noise decision 14 anticipated.*

---

## What this design deliberately does not do

Each of these is a decision with a reason, not an oversight.

- **No independent arbitration.** The marketplace judges its own disputes. Building a court
  means answering who the judges are, who pays them, and what stops them being bribed, and
  that is larger than everything else here combined. The door is kept open: the arbiter is
  only ever checked as "is this the expected signer", never assumed to be a wallet, so a
  future jury program's PDA can occupy the slot with no change to this program. Because
  terms are frozen per permit (decision 10), that migration can happen one seller at a time
  with no flag day.
- **No buyer reputation stored onchain.** A buyer identity costs nothing to abandon, so a
  bad mark is escaped by making a new wallet, and storing it would cost account slots in the
  two tightest transactions. Every resolution emits an event carrying the buyer and the
  outcome, so a buyer's history is computable today offchain and can be stored later with
  the full history rebuilt from those events.
- **No identity or KYC data, ever.** Every byte written to Solana is public and permanent.
  A national ID number stored here is published to the world with no way to retract it, and
  hashing does not save it: a 13-digit number is exhaustively searchable in seconds.
  Marketplaces verify their own users in their own databases, where the legal duty already
  sits. If onchain identity is wanted later, the correct shape is an attestation from a
  third-party issuer carrying a yes/no flag and never the underlying data.
- **No leverage multiplier.** A clean record earning more backing than collateral is only
  safe once reputation is expensive to fabricate, and it currently is not. Turning it on
  today converts a cheaply faked record into real uncollateralised exposure.
- **No appeals, no partial refunds, no protocol fee, no frontend in this build.**

---

## Honest limitations, stated rather than papered over

- **A permit caps total damage, not per-buyer coverage.** Forty buyers scammed on one
  marketplace share one permit, first come first served. The honest sentence is *"$150 is
  recoverable from this seller in total, shared across everyone who complains"*, not *"this
  seller is backed by $150"*. Keeping order volume in line with the permit is the
  marketplace's job. Binance enforces exactly this on its own merchants: a store cannot
  trade more than its deposit amount in a single order. That rule is published as an
  integration requirement (see below).
- **A stolen receipt-signer key together with a stolen arbiter key means up to one complaint
  window of exposure.** Whoever holds the signing key chooses the date on the receipt, so
  forged receipts can be backdated and revoking a permit does not stop them. Neither key
  alone is enough to steal. Recovery is: rotate both keys, and tell sellers to revoke and
  re-sign.
- **Reputation attaches to a wallet, not a person.** A scammer can start over. The barrier
  is the cost of a fresh stake and a history of zero, not identity.
- **A marketplace can fabricate reputation in both directions** for the price of
  registering, since it signs its own receipts and judges its own disputes. The counters are
  only as trustworthy as the marketplaces reporting them.
- **USDC's issuer can freeze a token account.** A frozen vault stops both slashing and
  withdrawal. No liquid stablecoin lacks this property.
- **The complaint window ceiling of 30 days excludes** preorders, long-lead custom work, and
  warranty-style claims.
- **A seller who quits for good leaves their account rent behind.** There is no
  `close_stake`, so roughly half a cent stays locked in the stake account and its vault after
  the last token is withdrawn. Adding a handler to reclaim it is a few lines and is deferred
  rather than overlooked: it is worth less than the transaction fee to call it.
- **A marketplace that shuts down for good leaves its account rent behind too.** There is no
  `close_marketplace`, so a `Marketplace` and its `bond_vault` strand their rent exactly the
  way `SellerStake` and its `stake_vault` do above, once the marketplace stops taking new
  orders and its last dispute is closed. Same reasoning, same deferral: adding a handler to
  reclaim it is a few lines and is worth less than the transaction fee to call it.
- **A rotated receipt-signer key is remembered once, not as a ring.**
  `Marketplace.prev_receipt_signer` holds only the key rotated away from, so a marketplace
  that rotates twice inside one complaint window overwrites it, and a receipt genuinely
  signed by the *first* key becomes unfileable even though its window has not closed. Not
  fixed, on purpose: the obvious fix, refusing a second rotation until the window elapses,
  would block exactly the emergency this mechanism exists for, a marketplace discovering
  mid-incident that its replacement key is also compromised, forced to leave a
  known-compromised signer live rather than rotate again. If this is ever revisited, the
  right fix is a small ring of previous keys with their own timestamps, not a cooldown on
  rotation.
- **A frozen buyer token account can stall `expire_dispute`.** The bond can only return to
  `buyer_token_account`, so a buyer whose token account is frozen blocks the one handler
  that exists specifically to free a seller when a marketplace never decides. Judged low:
  `buyer_token_account` is bound only to `token::authority = dispute.buyer`, not to the
  buyer's associated token account, and `expire_dispute` is permissionless, so whoever calls
  it may supply any token account the buyer owns, including a fresh one created for the
  purpose. Only a mint that makes new accounts frozen by default defeats this.
- **`bond_bps` may legally be zero.** `validate_marketplace_settings` bounds only the
  ceiling; no protocol floor is imposed. The risk is not only that a marketplace can harm
  its own sellers this way: with a zero bond a buyer holding one genuine receipt files for
  free, and every filing increments `permit.open_disputes`, which `release_permit` requires
  to reach zero before a seller can exit. Enough zero-cost complaints, each needing to be
  individually resolved or expired, traps a seller who wants out. It is left legal because
  `bond_bps` is frozen onto the permit at grant time, so a seller reads the rate and can
  decline before taking on any risk, and because a marketplace already holds a larger lever
  in its own arbiter. The mitigation belongs in whatever interface shows a seller the terms
  before they grant a permit.
- **A marketplace can grind down `SellerStake.disputes_total` and `disputes_lost` against its
  own seller, and every other marketplace that seller sells on inherits the damage.** Both
  counters live on the one global `SellerStake` per seller (decision 1) and only ever
  increment: `disputes_total` in `raise_dispute`, `disputes_lost` in the upheld branch of
  `resolve_dispute`. Since `validate_marketplace_settings` bounds `bond_bps` only by
  `MAX_BOND_BPS`, with no floor, a marketplace running a zero-bond permit can raise a
  complaint from a throwaway buyer account against its own seller, using a receipt it signed
  itself, and reject it with its own arbiter: no collateral moves, no complaint is upheld, and
  the cost is transaction fees alone, roughly 15,000 lamports for the three signed
  transactions one full cycle takes (raise, resolve, and the eventual close that reclaims the
  record's rent). Because the account is global, every other marketplace that seller sells on
  reads the same degraded number, not only the one that filed. Left legal rather than fixed,
  for three reasons. There is no protocol-enforced minimum bond: a zero bond is a legitimate
  choice for some marketplaces, and a floor would also price out honest buyers with small
  claims. The counters were not moved onto `SlashPermit` either: the seller controls permit
  release, so a per-permit counter would let a seller reset their own record by revoking and
  re-granting, which is strictly worse than a counter a marketplace can pad. And the counters
  were never meant to be the trust source: `DisputeRaised`, `DisputeResolved` and
  `DisputeExpired` all carry the marketplace's pubkey, so per-marketplace dispute history is
  computable offchain, exactly as this document already relies on for buyer history. A
  consumer that wants attack-resistant seller reputation has to read events and weight by
  marketplace, not read `stake.disputes_total` on its own.
- **A receipt the marketplace signs after revocation but before release can still land inside
  the next era's clock-skew tolerance.** `release_permit_early`'s minimum wait
  (`revoked_at + CLOCK_SKEW_TOLERANCE_SECONDS`) is what keeps check 7 safe against a
  genuinely stale, pre-revocation receipt, but it does not and cannot protect against a
  receipt dated *after* `revoked_at`: that receipt is already rejected against era 1 by
  `ReceiptIssuedAfterRevocation`, yet its `issued_at` can still sit within
  `CLOCK_SKEW_TOLERANCE_SECONDS` of era 2's `granted_at`, so it can be filed against the new
  permit instead. This is not the cross-era replay the fix closes -- the receipt is for a
  real order, correctly dated, and simply happens to have been signed during the wind-down
  window. It only exists because the marketplace signed a receipt against a permit it had
  already revoked, which is an integration error on the marketplace's side, not an attack a
  buyer or seller can engineer: the buyer does not control when the marketplace signs, and
  the seller does not control when the marketplace ships the receipt. Left unfixed on
  purpose. Closing it would mean either dropping the clock-skew tolerance entirely, which
  starts rejecting honest buyers over ordinary clock drift, or teaching `raise_dispute` to
  distinguish "receipt dated near the boundary because of drift" from "receipt dated near the
  boundary because it was signed during wind-down," which check 7 cannot do from the receipt
  alone. A marketplace that stops signing receipts against a permit the instant it revokes it
  never triggers this at all.
- **Check 7's rejection of a stale-era receipt no longer depends on an unenforced inequality
  between two constants; it depends on a floor `release_permit` now takes at the point of
  use.** This entry originally described a live gap: `release_permit` forced only `now >=
  revoked_at + complaint_window`, and the argument that check 7 then rejects every
  stale-era receipt relied on the premise that `complaint_window` could never fall below
  `MIN_COMPLAINT_WINDOW_SECONDS`. That premise was false under grandfathering, the same
  species of bug `closable_after` had (previous entry): `grant_permit` copies
  `complaint_window` off the live `Marketplace` with no re-validation against today's bounds,
  and `update_marketplace` only re-validates a field the caller actually supplies
  (`state/marketplace.rs`), so a marketplace registered before a future increase to
  `MIN_COMPLAINT_WINDOW_SECONDS` could still be granting permits with a window below the new
  floor. Fixed the same way as `closable_after`: `release_permit` now waits
  `revoked_at + complaint_window.max(CLOCK_SKEW_TOLERANCE_SECONDS)`
  (`earliest_release`, `instructions/release_permit.rs`), so the permit closes no earlier
  than `revoked_at + CLOCK_SKEW_TOLERANCE_SECONDS` no matter how short its stored window is.
  `release_permit_early` was already safe by construction the same way -- its wait is
  `revoked_at + CLOCK_SKEW_TOLERANCE_SECONDS` outright, with no stored window in the
  calculation at all (see that handler's doc comment for the R/L/G derivation both paths
  share). So both release paths are now safe by construction rather than by an assumed
  relationship between constants. The compile-time assertion
  `CLOCK_SKEW_TOLERANCE_SECONDS <= MIN_COMPLAINT_WINDOW_SECONDS` (`constants.rs`) stays in
  place as defence in depth -- it costs nothing and a wide margin between the two constants
  is still worth keeping -- but it is no longer what makes check 7 correct. With this fix,
  every stored field a protocol constant is compared against (`bond_bps`, the previous entry's
  `complaint_window` inside `closable_after`, and now `complaint_window` inside the release
  wait) is bounded at the point of use rather than trusted to already sit inside the
  constant's range; that family of grandfathering-sensitive constant uses is now closed.
- **`release_permit_early` erases outstanding buyers from the public record rather than
  merely closing their window.** The cooperative fast path (decision 8, above) lets the
  seller and the marketplace authority jointly skip the ordinary complaint-window wait, down
  to `CLOCK_SKEW_TOLERANCE_SECONDS` after revocation. Every buyer who has not yet raised a
  dispute against that permit loses the ability to do so the moment it closes:
  `raise_dispute` needs a live permit account to file against. Unlike `expire_dispute`,
  which formally marks an already-open complaint `Abandoned` and leaves that record
  onchain, this path never lets the complaint get raised in the first place, so no
  `DisputeRaised` event is ever emitted for it. The event trail this design otherwise leans
  on for offchain, attack-resistant reputation (see the counter-fabrication entries above)
  simply has a gap where those orders should be, with nothing onchain marking that they
  existed at all. Left unrecorded until now; the mechanism itself stays as designed, since a
  seller and marketplace who both consent to an early close are exactly the actors the
  ordinary window protects a buyer from -- the gap is only in what gets published about the
  buyers still in flight when they do.
- **A tightened protocol bound does not retroactively apply to marketplaces registered under
  the old one.** `grant_permit` reads `complaint_window` and `bond_bps` straight off the
  stored `Marketplace` account and never re-checks them against
  `MIN_COMPLAINT_WINDOW_SECONDS`, `MAX_COMPLAINT_WINDOW_SECONDS` or `MAX_BOND_BPS` as they
  stand today; `validate_marketplace_settings` only ever runs once, at `register_marketplace`.
  If a future release moves either constant, a marketplace registered under the old bounds
  keeps granting permits at its grandfathered values, and because `update_marketplace`
  validates only whichever field the caller actually supplies, that marketplace can still
  raise its bond without ever being forced to bring its window inside the new range. Not
  fixed, on purpose: adding the bounds check to `grant_permit` instead would not remove the
  grandfathering, only move which handler enforces it, and would start locking a marketplace
  that registered in good faith out of granting any new permit at all the day the bound
  moves. Tightening a bound is a migration, not a constant edit, and needs its own plan for
  what happens to the marketplaces it leaves outside the new range.

  That grandfathering had a sharper consequence than a marketplace merely keeping its old
  settings: `raise_dispute` check 7's own replay guard leaned on the same bound holding
  everywhere, not just at the marketplace that registered under it. `closable_after` is what
  lets `close_dispute` reclaim a `DisputeRecord`'s rent once a complaint window has genuinely
  closed, and deriving it from the protocol ceiling `MAX_COMPLAINT_WINDOW_SECONDS` flat,
  rather than the live permit's own (possibly wider, grandfathered) `complaint_window`, meant
  a grandfathered marketplace running a window wider than that ceiling could have its
  `DisputeRecord` closed -- `close_dispute` is permissionless -- before that marketplace's
  own, longer window had actually elapsed. The freed PDA let the same still-valid receipt be
  filed again, and the same order slashed twice. This specific consequence is fixed:
  `closable_after` is now derived from `max(permit.complaint_window,
  MAX_COMPLAINT_WINDOW_SECONDS)` rather than the flat ceiling alone, so a grandfathered
  marketplace's wider window can no longer outlive the record that blocks its receipts from
  being replayed. The grandfathering itself is untouched and stays deliberate, for the
  reasons above.
- **The bond percentage a grandfathered marketplace stores is clamped where the buyer is
  charged, not where the permit is granted.** `grant_permit` copies `bond_bps` onto the
  permit verbatim, for the grandfathering reason in the entry above; `raise_dispute` then
  takes `min(permit.bond_bps, MAX_BOND_BPS)` when it computes the bond. Without that, a
  marketplace registered under a higher ceiling would keep granting permits at the old rate
  and every buyer filing there would have to post a larger bond than today's ceiling
  permits. The fix deliberately does not go in `grant_permit`: a bounds check there would
  lock a marketplace that registered in good faith out of granting any permit at all the day
  the constant moves, which is the outcome the grandfathering exists to avoid. Note that
  this clamps *down* where `closable_after` clamps *up*. The two are not inconsistent: each
  takes the direction that protects the party who cannot defend themselves. A buyer cannot
  negotiate the rate they are charged, so the bond takes the smaller of the two rates; a
  replay guard must outlive every filing window that could apply to it, so `closable_after`
  takes the longer of the two windows.
- **The collateral mint may carry no token extensions at all.** `initialize_config` reads the
  mint's extension list and rejects anything outside `ALLOWED_MINT_EXTENSIONS`, which is
  empty, so a Classic Token Program mint passes and every Token Extensions mint carrying any
  extension is refused: transfer fees, transfer hooks, permanent delegates, pausable mints,
  default-frozen accounts, confidential transfers, and interest-bearing mints alike. An
  allow-list rather than a list of banned extensions, so that extension types added to the
  Token Extensions Program in future are denied by default instead of admitted by default.
  Widening it later is a deliberate and safe change: `Config.collateral_mint` is pinned per
  deployment, so admitting a new extension only affects which mints a fresh deployment can
  pin, never the collateral an already-live deployment holds.

  The case that forces this is `TransferFeeConfig`. A fee-bearing mint skims a percentage in
  transit, so a buyer's bond would arrive in the bond vault short of the amount
  `raise_dispute` records on the `DisputeRecord`; `resolve_dispute` and `expire_dispute`
  would each then try to move the full recorded bond out of a vault holding less and revert
  permanently, leaving `open_disputes` stuck at 1 and the seller's committed collateral
  locked with it. Because there is no `update_config`, that is unrecoverable short of a
  program upgrade.

  What is checked is whether the mint *carries* an extension, never what that extension is
  set to today. A transfer fee can be created at zero basis points and raised later by the
  `transfer_fee_config_authority` through `SetTransferFee`, taking effect about two epochs
  on, so reading today's fee and accepting zero would promise nothing. Whether a mint carries
  an extension at all is fixed when the mint is created and can never change, which makes it
  the only durable thing to test.

  The more general fix, having each token-moving handler measure what actually arrived rather
  than trusting what it sent, was considered and deferred. It is disproportionate for a
  prototype: it means a balance read before and after every transfer in five handlers, on
  every path, for a hazard this deployment does not face at all. It is also not sufficient on
  its own, which is the stronger reason. Fees are one extension out of many, and the others
  break different things: a transfer hook runs third-party code inside every transfer, a
  permanent delegate can move collateral straight out of a vault with no handler involved,
  and a pausable mint can freeze slashing and withdrawal together. Conservation checks answer
  none of those. Supporting any single extension safely means reasoning through that
  extension against all five handlers, which is what an entry in `ALLOWED_MINT_EXTENSIONS`
  will mean when one is ever added.
- **`Config.authority` currently confers no powers.** Grepping every handler that loads
  `Config` shows it read for exactly two things: `propose_config_authority` and
  `accept_config_authority` read and write `config.authority`/`config.pending_authority`
  themselves, to run the two-step transfer; every other handler that loads `Config`
  (`initialize_stake`, `raise_dispute`) reads only `collateral_mint` or `chain_id`. There is
  no protocol-level admin action gated on `config.authority` -- no pause, no parameter
  change, no emergency lever -- so the two-step transfer machinery currently moves a key that
  does nothing once moved. That is not a bug in the transfer logic; it means the key is
  decorative until some future handler actually checks it, and whichever handler is first to
  do so inherits whatever has happened to that key in the meantime (see "Upgrade authority is
  a single ordinary keypair, not a multisig" under "Risks and open items" for the parallel
  concern on the *upgrade* authority, a separate key from this one).

---

## Integration requirements for a marketplace

Published as part of the interface, because the program cannot enforce them.

- **Do not sign receipts for more outstanding order value than the seller's remaining permit
  covers.** The program cannot count live orders, by design (decision 5). This is the same
  rule Binance applies to its own merchants.
- **Show buyers the permit at *this* marketplace, never the seller's total collateral.** A
  seller with $300 staked and a $150 permit here is backed by $150 here. Showing $300 is
  false in the direction that matters.
- **Order IDs must be unique per seller within a marketplace, forever.** A reused ID from the
  same seller collides with the existing complaint PDA and blocks the second complaint from
  ever being filed. Two different sellers on the same marketplace may reuse an order ID
  freely: the `DisputeRecord` seed includes the seller precisely so this does not collide.
  (Earlier versions of this rule said "unique within a marketplace," full stop, with no
  seller in the seed -- a footgun, because per-seller order numbering is the natural way to
  implement order IDs, and nothing about "unique within a marketplace" reads as forbidding
  it.)
- **Keep the receipt-signer key and the arbiter key separate**, and separate from the
  authority key. Compromise of any one alone cannot move money.

---

## Program conventions

House rules that apply to every file, collected here so they are decided once rather than
argued per handler. The prototype violates several of them, so Phase 1 is a rewrite rather
than an extension.

**Layout.** State types go in a `state/` folder, one file per account, rather than a single
`state.rs`; there are five of them and they will not stay small. Instruction handlers stay
one-per-file under `instructions/`, with the protocol-admin ones (`initialize_config` and
the authority-transfer pair) in `instructions/admin/`.

**Naming.** A `#[derive(Accounts)]` struct is named for what it *is*, not for what the
handler does, because a struct cannot do anything: `RaiseDisputeAccountConstraints`, not
`RaiseDispute`. All four of the prototype's structs need renaming.

**Space.** `space = Config::DISCRIMINATOR.len() + Config::INIT_SPACE`, with
`#[derive(InitSpace)]` on every account type. No arithmetic on literal byte counts anywhere;
the prototype's `8 + Dispute::INIT_SPACE` is exactly the pattern to remove. The version byte
and reserved padding are ordinary declared fields, so `InitSpace` accounts for them.

**Tokens.** `anchor_spl::token_interface` types (`InterfaceAccount<TokenAccount>`,
`InterfaceAccount<Mint>`, `Interface<TokenInterface>`) and `transfer_checked` for every
transfer, never raw `transfer`. `transfer_checked` carries the mint and decimals through the
call, so a wrong-mint or wrong-decimals account fails the transfer instead of silently
miscalculating. Every handler that moves tokens takes the mint account, and binds it against
`stake_vault.mint` rather than against `Config.collateral_mint`. The vault's mint was checked
against `Config` when `initialize_stake` created it, so the chain of trust already holds, and
binding through the vault means `add_stake`, `withdraw_stake` and `resolve_dispute` do not
need to carry `Config` at all. `raise_dispute` still carries it, because that is where
`chain_id` is checked. An unbound mint lets a caller substitute a different one.

**Amounts.** USDC has six decimals. Every figure in this document is written in dollars for
readability, and every figure in the program is in minor units: a $150 permit is
`150_000_000`. No hardcoded powers of ten; derive from the mint's own `decimals`.

**Arithmetic.** `checked_*` everywhere, `.ok_or(TrustStakeError::MathOverflow)?` to force the
error, never `saturating_*` on a balance. Basis-point maths casts to `u128` before
multiplying and narrows with `try_into()`. Multiply before dividing. **The bond rounds up**:
`claim * bond_bps` divided by `10_000`, rounded toward the buyer paying more, so a small
claim can never produce a zero bond through truncation.

**Ordering.** Checks, then effects, then interactions. State is written before the transfer
call, not after. Within a `#[account(...)]` attribute, `close` goes **last**: Anchor evaluates
constraints in order, so anything after `close` inspects an account that has already been
zeroed. This applies to `close_dispute` and to `release_permit`.

**Every account is bound to something.** Anchor's `Account<T>` checks only the owner and the
discriminator; it does not connect accounts to each other. In particular the `Marketplace`
account is validated by `seeds = [b"market", b"v2", marketplace.marketplace_id.as_ref()]`
against its own stored ID, so a permit or dispute PDA can never be derived from a marketplace
account the caller substituted.

**Conservation is asserted in the program, not only in tests.** Every handler that moves
tokens reloads the vault afterwards and requires `stake_vault.amount >= stake.staked`, not
exact equality: a token account accepts a transfer from anyone without its owner's consent,
and no handler recomputes `staked` from the vault, so exact equality let a stranger brick a
seller permanently by donating one minor unit into their vault -- every later handler that
moves tokens would then find the vault disagreeing with the ledger and refuse to run, with no
instruction able to fix it. `>=` still catches the dangerous direction, a vault caught *short*
of the ledger, which is what a drain (or, before it is rejected at registration, a
transfer-fee mint) looks like; a surplus is inert, since every payout is sized from the ledger
fields alone (`stake.staked`, `dispute.claim`, `dispute.bond`, ...) and never from
`stake_vault.amount`.

**Config validation is one named function.** `validate_marketplace_settings` checks the
window is inside 2 to 30 days and `bond_bps` is at or below 2,000, called from both
`register_marketplace` and `update_marketplace`. Settings are rejected at the point they are
accepted, not re-checked in every handler that reads them.

**Time** comes from `Clock::get()?`, and every deadline uses `unix_timestamp` rather than a
slot count. Slots are the tamper-resistant choice and are correct for oracle freshness, but
every deadline here is a promise to a human measured in days ("you have 14 days to
complain"), and slot timing drifts against wall-clock over that span. Validator timestamps
vary by seconds, which is irrelevant at this scale. Every comparison is written out in words
first and then coded to match that sentence, because inverted comparisons ship when tests use
a zero duration where both directions collapse to equality.

**Sysvar accounts are address-checked.** Any sysvar taken as an account is verified against
its known address. `raise_dispute` is the one that matters, and the reason is spelled out in
its own section.

**Presentation note.** This document uses markdown tables for the decision list, which the
house documentation style otherwise avoids. That format is deliberate here and carried over
from the previous version, because the decision-and-reasoning pairing is the point of the
document.

---

## Account model

Every account carries a `version` byte at a fixed offset immediately after the discriminator
plus reserved padding, so fields can be added later without migrating accounts that hold
real collateral. All seeds carry a `"v2"` component so v2 accounts cannot collide with the
deployed v1 layout.

**The claim that fields can be added later without migrating accounts is only true if every
new field is appended, never inserted.** Borsh serializes a struct positionally, by declared
field order, not by name. On a program that has already been deployed, adding a field anywhere
but immediately before `reserved` shifts the byte offset of every field declared after it. A
struct upgraded that way still compiles and still deserializes without error, because Borsh has
no schema to check against, just a byte count to consume. Concretely: if `revoked_at` shifted
eight bytes to the right, every existing permit would deserialize `revoked_at` from bytes that
used to hold `complaint_window`, `complaint_window` from bytes that used to hold `bond_bps`,
and so on down the struct. A live permit that was actually still open (`revoked_at == i64::MAX`)
would very likely read back a small, already-past `revoked_at` after the shift, so
`release_permit`'s window check would pass immediately: the collateral behind a permit the
seller never revoked becomes withdrawable, with no error, no panic, and no trace in the logs.
`SlashPermit.granted_at` was inserted mid-struct this way during the v2 build (fixed before
deployment, see the account layout below); the rule going forward is to always add the new
field as the last one before `reserved`, never between existing fields, once a program version
holding this struct has been deployed.

**`SEED_VERSION` does not move again once real collateral exists, either.** The
changelog below (item 14) explains why this deployment carries versioned seeds at all: moving
from `v1`'s unversioned seeds to `"v2"` was only safe because the v1 prototype held nothing of
value, which is also true today, before this branch deploys. Once a `SlashPermit`,
`SellerStake`, or any other PDA under `SEED_VERSION` is backing real collateral, bumping the
seed to `"v3"` would derive a different address for every one of those PDAs, silently
stranding whatever they hold: the program would start deriving and validating against
addresses nobody funded, while the old, still-funded accounts sit at their `"v2"` addresses
with no instruction able to reach them, because every constraint would derive the new seed.
The decision: `SEED_VERSION` may not change again after this deployment holds real collateral,
except through a dedicated migration instruction that signs for the *old* seed bytes
explicitly (deriving and validating against `"v2"`, not whatever `SEED_VERSION` has become) to
move funds out of the old PDA before anything starts deriving under a new one. A seed bump
with no such instruction is a stranding bug, not a version bump.

```
Config                    ["config", "v2"]
  version, bump           u8, u8
  authority               Pubkey     protocol admin (Squads multisig)
  pending_authority       Pubkey     two-step transfer; default = Pubkey::default()
  collateral_mint         Pubkey     USDC; the single protocol-wide mint, pinned by address
  chain_id                u8         devnet/mainnet tag, checked against every receipt
  reserved                [u8; 64]

Marketplace               ["market", "v2", marketplace_id]
  version, bump           u8, u8
  marketplace_id          [u8; 16]   client-chosen, immutable, the identity of the tenant
  authority               Pubkey     settings owner
  pending_authority       Pubkey
  receipt_signer          Pubkey     ed25519 key its backend signs receipts with
  prev_receipt_signer     Pubkey     valid for receipts issued before the rotation
  signer_rotated_at       i64
  arbiter                 Pubkey     resolves complaints
  complaint_window        i64        2..30 days; copied onto each new permit
  bond_bps                u16        <= 2_000; copied onto each new permit
  disputes_total          u32        \
  disputes_upheld         u32         > public track record
  disputes_abandoned      u32        /  the mark for never deciding
  total_slashed           u64
  reserved                [u8; 64]

SellerStake               ["stake", "v2", seller]        ONE per seller, global
  version, bump           u8, u8
  seller                  Pubkey
  staked                  u64        always equal to the vault's token balance
  committed               u64        sum of REMAINING allowance across active permits,
                                     that is sum of (max_slashable - slashed). Defining it
                                     as the sum of caps would break committed <= staked the
                                     first time anyone is slashed.
  disputes_total          u32        the denominator missing today
  disputes_lost           u32
  reserved                [u8; 64]

stake_vault               ["vault", "v2", seller]        token account, authority = SellerStake PDA
                                                         created by initialize_stake
bond_vault                ["bonds", "v2", marketplace]   token account, holds live bonds
                                                         created by register_marketplace

SlashPermit               ["permit", "v2", seller, marketplace]
  version, bump           u8, u8
  seller, marketplace     Pubkey
  max_slashable           u64        increase-only
  slashed                 u64
  open_disputes           u16        blocks release of THIS permit only
  revoked_at              i64        i64::MAX = active
  complaint_window        i64        frozen at grant
  bond_bps                u16        frozen at grant
  granted_at              i64        added after the fields above; stamped from Clock in
                                     grant_permit, never touched again; bounds which era a
                                     receipt must belong to (raise_dispute, check 7)
  reserved                [u8; 24]

DisputeRecord             ["dispute", "v2", marketplace, seller, order_id]
  version, bump           u8, u8
  marketplace, seller, buyer   Pubkey
  order_id                [u8; 32]
  claim                   u64
  bond                    u64        the amount actually deposited, not recomputed later
  created_at              i64
  expires_at              i64        created_at + DISPUTE_EXPIRY (30 days), protocol constant
  closable_after          i64        receipt.issued_at + max(permit.complaint_window,
                                     MAX_COMPLAINT_WINDOW_SECONDS)
  status                  u8         Open=1 | Upheld=2 | Rejected=3 | Abandoned=4
  reserved                [u8; 32]
```

`status` discriminants start at 1 so a zeroed account can never read as a valid state.

`revoked_at` uses `i64::MAX` for "active". A zero sentinel would make the release check
`now >= revoked_at + complaint_window` trivially true for every active permit, which is
instant unconditional withdrawal.

**`marketplace` in a seed means the `Marketplace` PDA's address, not its ID.** Only the
`Marketplace` account itself is keyed by the raw ID. Everything subordinate keys off the
address, because that is what `DisputeRecord` stores, and it is what lets `resolve_dispute`
chain every account off the dispute record with `has_one` instead of trusting the caller.
The address is derived from the immutable ID, so it survives every key rotation, which is
what decision 11 was for.

`Config` has no update handler. `collateral_mint` and `chain_id` are fixed at
initialization: changing the mint once vaults hold tokens would strand every balance, and
the chain tag is a deployment fact. Only the authority moves, and only through the two-step
transfer.

### The offchain receipt

A Borsh struct, never stored onchain, signed by `receipt_signer`:

```
domain            b"truststake:receipt:v1"   constant prefix; carries the version, so the
                                             struct does not also need a version field
program_id        Pubkey                     must equal crate::ID
chain_id          u8                         must equal Config.chain_id
marketplace_id    [u8; 16]
seller            Pubkey
buyer             Pubkey
order_id          [u8; 32]
amount            u64
issued_at         i64
expires_at        i64
```

An offchain signature that authorises spending has to commit to everything it authorises,
and the program has to rebuild the message it verifies from its own state rather than
trusting a client-supplied blob. That is what the field list above is for: the domain tag
and `program_id` bind it to this program, `chain_id` stops a devnet signature being replayed
on mainnet, and the rest names exactly which seller, which buyer, which order and how much.

**Every field is checked against the accounts actually passed into the instruction handler,
not merely against the signature.** A valid signature over a receipt naming a different
seller must not be usable against this seller.

**Single use** is enforced by the `DisputeRecord` PDA at
`["dispute", "v2", marketplace, seller, order_id]` rather than by a counter. A counter would
serialise receipt issuance, which decision 5 exists to avoid; the order ID is the natural
per-receipt nonce, and the record's existence is what makes reuse impossible. The seller is
part of the seed, not just the marketplace, because a receipt binds `(marketplace, seller,
buyer, order)`: without the seller, two sellers on one marketplace choosing the same order ID
collide on one `DisputeRecord` address, and the second buyer's genuine complaint is refused
until the first record's replay window has passed -- which, since both windows are measured
from the same protocol-wide `MAX_COMPLAINT_WINDOW_SECONDS`, tends to be exactly the moment the
second receipt has also aged out.

**The signature supplements the transaction's own authority check, it does not replace it.**
The buyer signs the transaction and the receipt names that same buyer, so both must agree.

---

## Instruction handlers

Replaces the current four in `programs/truststake/src/instructions/`, keeping the existing
one-file-per-handler layout and the `instructions.rs` module index.

**Protocol:** `initialize_config`, `propose_config_authority`, `accept_config_authority`

**Tenant:** `register_marketplace`, `update_marketplace`, `propose_marketplace_authority`,
`accept_marketplace_authority`

**Seller collateral:** `initialize_stake`, `add_stake`, `withdraw_stake`

**Opt-in:** `grant_permit`, `increase_permit`, `revoke_permit`, `release_permit`,
`release_permit_early`

**Disputes:** `raise_dispute`, `resolve_dispute`, `expire_dispute`, `close_dispute`

`initialize_stake` creates the account and vault only; `add_stake` moves collateral. That
split is what fixes the prototype's dead-account hole: a seller slashed to zero calls
`add_stake` and trades again.

`initialize_config` requires the signer to equal a constant compiled into the program, so a
freshly deployed program ID cannot have its config front-run. That constant is the deploy
wallet's pubkey and has to hold a real value from Phase 1 onward. It does not conflict with
pointing the protocol at a Squads multisig later: it gates only who may call this handler
once, and `Config.authority` moves afterwards through the two-step transfer.

`register_marketplace` creates the marketplace's `bond_vault` and the marketplace pays its
rent. Creating it lazily at the first dispute is not an option, because `init_if_needed` is
banned and `raise_dispute` must find the vault already there.

**The collateral mint on devnet cannot be real USDC.** Real devnet USDC exists but cannot be
minted, so the demo could never fund a test seller or buyer. Devnet pins a test mint created
by the deploy script with **6 decimals**, matching USDC so the figures read correctly and
every decimals-dependent path is exercised honestly. Mainnet would pin real USDC. There is no
`update_config`, so a deployment that pins the wrong mint cannot be repaired, only replaced.
The `World` harness sets up the same mint, which makes it a Phase 1 artifact that Phases 2
and 3 inherit.

`chain_id` is **1 on devnet and 2 on mainnet**. The program checks it in Phase 3 and the
demo backend signs receipts carrying it in Phase 4; a mismatch fails every dispute with an
error that reads like a signature problem.

`withdraw_stake` requires `staked - amount >= committed`, checked at withdrawal time.

`grant_permit` and `increase_permit` require the seller's signature (that is the entire
meaning of opting in) and require `committed + delta <= staked`. **The seller can raise a
cap but never lower it**: lowering it would let them shrink their exposure the moment a
complaint appears, paying a wronged buyer nothing. To lower a cap, revoke and grant a smaller
permit after the window. A slash reduces the remaining allowance, which is the program acting
on a real payout rather than the seller retreating, and is the one path that shrinks it.

`revoke_permit` stamps `revoked_at` and nothing else. It stops new receipts immediately but
frees no collateral; the permit's remaining allowance stays inside `committed` until release.

`release_permit` is permissionless and requires `now >= revoked_at + complaint_window` and
`open_disputes == 0`. It **subtracts the permit's remaining allowance from
`stake.committed`**, which is the entire point of releasing, then closes the permit account
and refunds its rent to the seller, who paid it. `release_permit_early` skips that
complaint-window wait when the seller and the marketplace authority both sign, and does the
same subtraction, but still requires `now >= revoked_at + CLOCK_SKEW_TOLERANCE_SECONDS`. That
minimum wait is not the complaint-window protection in miniature; it exists purely so that
`raise_dispute` check 7 cannot be replayed. A closed permit frees its PDA, and the seller may
grant a fresh one at the same address; the marketplace's and the seller's counters are what
carry the history, not the permit. Because a new grant can only follow a close (`grant_permit`
uses `init`), the earliest a fresh permit's `granted_at` can land is
`revoked_at + CLOCK_SKEW_TOLERANCE_SECONDS`, which is exactly the bound check 7 needs: every
receipt genuinely valid against the closed-out era was issued before `revoked_at`, and
therefore falls outside check 7's tolerance window against the new one. This wait reuses
`CLOCK_SKEW_TOLERANCE_SECONDS` rather than a constant of its own on purpose -- the two are one
guarantee split across two handlers, and giving them separate constants would let a future
edit to either one silently reopen the replay.

Every counter that has a denominator increments **when a dispute is raised**, not when it
resolves: `stake.disputes_total` and `marketplace.disputes_total` go up in `raise_dispute`.
Incrementing at resolution would leave abandoned disputes outside the total and make the
marketplace's abandonment rate uncomputable, which is the one number decision 7 relies on.
Upheld plus rejected plus abandoned plus still-open therefore equals total, always.

### `raise_dispute`, the one with real complexity

Submitted as a two-instruction transaction: `[Ed25519Program verify, raise_dispute]`. The
handler reads the other instruction through the Instructions sysvar and must verify, in this
order:

0. **The Instructions sysvar account is the real one**, its address checked against
   `sysvar::instructions::ID`. Everything below reads out of this account, so without this
   check an attacker supplies a fabricated account describing a transaction that never
   happened, and all nine remaining checks pass against data they wrote themselves. This is
   the same outcome as the crossed-indices attack, reached by an easier route, and it is the
   first thing the handler does.
1. The current index comes from `load_current_index_checked`, and the instruction at
   `current_index - 1` targets the native Ed25519 program. The position is derived, never
   assumed to be index 0.
2. **All three instruction indices in the Ed25519 header** (`signature_instruction_index`,
   `public_key_instruction_index`, `message_instruction_index`) point at that same
   instruction. Validating byte offsets alone is not enough: an attacker can otherwise have
   the precompile verify their own throwaway signature while this handler reads the pubkey
   and message from a different instruction they also control. Every offset is
   bounds-checked before slicing and the message length is asserted exactly. This is the
   most exploitable surface in the design; follow
   [GuidoDipietro/solana-ed25519-secp256k1-sig-verification](https://github.com/GuidoDipietro/solana-ed25519-secp256k1-sig-verification)
   rather than improvising.
3. The signing pubkey equals `marketplace.receipt_signer`, or equals
   `marketplace.prev_receipt_signer` when `signer_rotated_at != 0` and
   `receipt.issued_at < signer_rotated_at`. An honest key rotation must not silently void
   every outstanding buyer's claim. The `signer_rotated_at != 0` guard matters because a
   never-rotated marketplace holds `Pubkey::default()` in the previous slot, and that branch
   must be unreachable rather than merely unusable.
4. The receipt is deserialised **from the Ed25519 instruction's message bytes**, not passed
   again as an argument. Sending it twice wastes roughly 150 bytes of a 1,232-byte budget.
5. `receipt.domain` and `receipt.chain_id` match, `receipt.expires_at > now`, and
   `now < receipt.issued_at + permit.complaint_window`, using checked addition.
6. `receipt.marketplace_id == marketplace.marketplace_id`, `receipt.seller == stake.seller`,
   `receipt.buyer == buyer.key()` where `buyer` is the transaction signer, and
   `receipt.order_id` equals the seed the `DisputeRecord` is being created at. The receipt
   names the marketplace by its ID because that is what a backend knows; the accounts key off
   the marketplace's address, and the `Marketplace` account's self-validating seeds are what
   ties the two together.
7. The receipt was issued during THIS permit's own era, not some other one that used to
   occupy the address. Both release paths close the permit account and `grant_permit`
   re-inits at the same PDA, so a receipt is bound to the era that actually issued it:
   `receipt.issued_at >= granted_at - CLOCK_SKEW_TOLERANCE_SECONDS` (the same allowance
   the future-dating check above gives the marketplace's clock, applied at the other
   boundary -- rejecting a genuinely new order's receipt over a few seconds of drift costs
   a real buyer their complaint) and, at the other end, permit is active or was revoked
   with `receipt.issued_at < revoked_at` and is still inside the window. This bound is only
   safe on the ordinary `release_permit` path by construction, since a complaint window is
   always at least `MIN_COMPLAINT_WINDOW_SECONDS` (2 days), far longer than the tolerance.
   It is **not** safe on `release_permit_early` unless that handler enforces its own minimum
   wait of `CLOCK_SKEW_TOLERANCE_SECONDS` after revocation (decision 8): without it, the
   entire revoke-release-regrant cycle can complete inside the tolerance window, and a
   genuine, never-disputed receipt from the wound-down era can be filed and upheld against
   the freshly granted one.
8. `claim <= receipt.amount`. A claim larger than the permit's remaining balance is
   **accepted, not rejected**, and pays out whatever remains at resolution. Rejecting it
   would leave 39 of 40 scammed buyers unrecorded and make the seller's public loss count
   understate the fraud precisely when the fraud is worst.
9. `DisputeRecord` inits at `["dispute", "v2", marketplace, seller, order_id]`. Init failure
   is the replay guard.
10. The buyer transfers the bond, rounded up, into `bond_vault`; the amount actually
    deposited is recorded on the dispute. Then `permit.open_disputes += 1`,
    `stake.disputes_total += 1` and `marketplace.disputes_total += 1`.

### `resolve_dispute`

Signed by `marketplace.arbiter`. Requires `status == Open` as the first check: without it a
never-closed record is resolvable repeatedly, draining other buyers' bonds from the shared
vault and underflowing the open-complaint counter past the release gate.

Every account is chained off the `DisputeRecord` via `has_one`, never taken from
caller-supplied marketplace, permit or stake accounts. Otherwise anyone registers their own
marketplace, appoints themselves arbiter, and resolves someone else's complaint. Payout
destinations are bound to `dispute.buyer` and `dispute.seller` for the same reason.

Upheld: pay `min(claim, permit.max_slashable - permit.slashed, stake.staked)` from
`stake_vault` to the buyer, return the recorded bond amount to the buyer, increment
`permit.slashed`, `stake.disputes_lost`, `marketplace.disputes_upheld` and
`marketplace.total_slashed`. **Decrease `stake.committed` and `stake.staked` by the payout**,
which keeps them in step: the permit's remaining allowance and the collateral backing it
fall together, so `committed <= staked` survives a slash.

Rejected: the recorded bond moves from `bond_vault` into `stake_vault` and `staked` rises by
that amount. `committed` is untouched, so the seller's free balance grows by the bond.

Both paths: `permit.open_disputes -= 1` with checked subtraction, and `status` is set.

### `expire_dispute`

Permissionless once `now >= dispute.expires_at` and `status == Open`. Nobody is paid. The
bond returns in full to the buyer, `permit.open_disputes -= 1`, `status = Abandoned`, and
`marketplace.disputes_abandoned += 1`. Neither the seller's nor the buyer's record is
touched: the failure belongs to whoever was supposed to decide.

### `close_dispute`

Permissionless once `status != Open` and `now >= dispute.closable_after`. Closes the account
and refunds its rent to the buyer. Safe because a receipt cannot be used past
`issued_at + complaint_window`, so the replay guard has nothing left to guard. The permanent
record lives in the counters and the event log.

---

## Build order

**Phase 0: dependencies.** Confirm before anything else. Two of these decide whether later
phases can run at all, and the first is already true in the repo today.

- **The `solana-*` crate line is already split.** `cargo tree -d` reports duplicate major
  versions of `solana-address`, `solana-define-syscall`, `solana-hash`, `solana-pubkey` and
  `solana-system-interface`. Both sides come from inside `litesvm 0.10.0`'s own tree, so it
  compiles today, but `anchor-lang 1.1.2` resolves `solana-pubkey 3.0.0` while parts of that
  tree are on 4.x, and a `Pubkey` from one line is a different type to the compiler than a
  `Pubkey` from the other. **Every new direct dependency must be pinned to the 3.x line**, and
  `cargo tree -d` must be re-run after adding each one. This bites in Phase 3, where test code
  constructs an Ed25519 instruction and hands its types to Anchor APIs.
- `solana-instructions-sysvar` and `solana-sdk-ids`, both pinned `^3`, as direct
  dependencies. `anchor-lang` 1.1.2 does not re-export `load_instruction_at_checked`,
  `load_current_index_checked` or `ed25519_program::ID`.
- `litesvm = { version = "0.10.0", features = ["precompiles"] }`, so Ed25519 executes
  in-process. The feature does exist in 0.10.0, confirmed by the gate test below passing, so
  no LiteSVM version move is needed and the crate-line pins above hold.
- `anchor-spl = "=1.1.2"`, exactly matching the resolved `anchor-lang`, because 1.1.2
  tightened the inter-crate `anchor-*` pins. Set `default-features = false` selecting
  **`token`, `token_2022` and `associated_token`**, and extend `idl-build` to
  `["anchor-lang/idl-build", "anchor-spl/idl-build"]`; it currently names only `anchor-lang`,
  which fails the IDL build the moment token accounts appear.
  `token_2022` is required rather than chosen: anchor-spl 1.1.2's `idl_build.rs` references
  `crate::token_interface` with no `cfg` gate (only its `metadata` and `stake` entries are
  gated), so the `idl-build` feature does not compile unless both `token` and `token_2022`
  are on. Writing the program against `anchor_spl::token` instead would not avoid it, because
  the reference is inside anchor-spl rather than in our code. Leave
  `token_2022_extensions` off. Drop `token_2022` if the upstream packaging bug is fixed.
- **`Cargo.toml` declares `anchor-lang = "1.0.1"` but resolves 1.1.2, and the installed CLI
  is `anchor-cli 1.0.1`.** The CLI warns on a mismatch against the crate. Pin the declaration
  to `=1.1.2` and move the CLI to match, or pin both down to the 1.0.x line. Do not leave
  them disagreeing while also pinning `anchor-spl` exactly.
- Set `package_manager = "npm"` in `Anchor.toml`, which currently reads `yarn`.
- Measure the real `.so` size with a stub build. The prototype's four handlers come to
  194,872 bytes, which is about 1.37 SOL of rent-exemption, and a deploy needs the program
  account and a buffer at once, so peak is roughly double that. v2 carries seventeen handlers
  plus token CPIs and signature introspection, so expect the peak in the 5 to 7 SOL range
  against a devnet faucet capped at 2 SOL per request. Start accumulating devnet SOL well
  before Phase 4, and re-measure once the real binary exists at the end of Phase 3. Keeping
  the same program ID and versioning the PDA seeds makes Phase 4 an upgrade rather than a
  fresh deploy, so it pays the buffer plus the size delta instead of two full rent-exemptions.

The installed toolchain is otherwise sound: Ubuntu 24.04.4 with GLIBC 2.39 and Rust 1.89.0,
which meets `anchor-lang` 1.1.x's MSRV exactly. The installed Solana CLI is 4.0.2 while
Anchor 1.1.x is CI-tested against 3.1.10; the prototype builds and deploys today, so treat
that as a suspect only if `cargo build-sbf` starts failing.

**Phase 1: foundation.** New `state.rs`, `constants.rs`, expanded `error.rs`, new
`events.rs` using `emit_cpi!` rather than `emit!` (plain events truncate silently past the
10KB log limit, and buyer history is derived from events). `initialize_config`, the
authority-transfer pair, `register_marketplace`, `update_marketplace`, `initialize_stake`
with its vault, `add_stake`. Checked arithmetic and `u128` for basis-point maths throughout.

**Build the `World` test harness in this phase, not at the end.** Existing tests repeat ~80
lines of setup each, and every later phase needs a cheap way to reach a given state. Each
phase then adds its own slice of the matrix in Verification as it lands, so the suite is
always green rather than being written once at the finish.

**Phase 2: collateral lifecycle.** `grant_permit` and `increase_permit` with the
fully-backed check, `revoke_permit`, `release_permit`, `release_permit_early`,
`withdraw_stake` gated on `committed`.

**Phase 3: disputes.** Receipt struct, Ed25519 introspection helper, `raise_dispute`,
`resolve_dispute`, `expire_dispute`, `close_dispute`. Box the cold-path accounts in
`RaiseDisputeAccountConstraints` to stay inside the 4KB stack frame.

**Phase 4: proof and docs.** Close any remaining gaps in the test matrix, measure the real
`raise_dispute` transaction size, rewrite `examples/devnet_demo.rs` to walk two
marketplaces, redeploy, then update `README.md` and `CLAUDE.md`. The CLAUDE.md "Deliberate
simplifications" list and the README "Prototype scope" section both go stale the moment
Phase 2 lands, and CLAUDE.md explicitly asks for them to be updated together.

---

## Verification

Existing tests in `programs/truststake/tests/test_truststake.rs` repeat ~80 lines of setup
per test. The `World` harness (svm, keypairs, PDAs, one method per instruction handler)
lands in Phase 1, and each phase adds its own slice of the matrix below as it goes.

Run: `cd programs/truststake && cargo test`. Build: `anchor build` from the repo root, not
`cargo build`, because Solana programs need the SBF target.

**[TESTING.md](TESTING.md) is the full plan**: named tests grouped by who is attacking,
the invariants the harness asserts after every instruction handler, the property tests, the
transaction-size and compute measurements, and a table mapping every critical and high audit
finding to the test that keeps it fixed. Each phase's definition of done is the slice of
that file belonging to it.

Four areas carry most of the weight:

- **Invariants after every handler.** The vault balance equals `staked`, `committed` equals
  the sum of remaining permit allowances, and no USDC is created or destroyed by anything that is not
  a deposit or a withdrawal. The conservation check catches more than the rest combined.
- **The signature check.** A weakness here forges marketplace receipts with no key
  compromise, which defeats every other protection at once. Crossed instruction indices are
  the specific attack; validating byte offsets alone does not catch it.
- **The freeze.** A marketplace opens a complaint and never resolves it, and 30 days later
  anyone can expire it and the seller withdraws. This is the finding all three reviewers hit
  independently and the most important single test in the suite.
- **The cap.** Not that a marketplace cannot take the money, because decision 2 says it can,
  but that it cannot take one cent past the cap and cannot reach anything that is not its
  own.

Then `cargo run --example devnet_demo` for real devnet signatures. The demo walks:

> One seller, **$300** staked.
> **CashDesk** registers with a 2-day window; the seller grants it a **$150** permit.
> **PixelBazaar** registers with a 7-day window; the seller grants it a **$50** permit.
> CashDesk's backend signs a receipt; a buyer disputes with it; CashDesk's arbiter upholds.
> PixelBazaar's permit and the seller's free $100 are demonstrably untouched.
> The seller revokes CashDesk, waits out its window, and withdraws the remainder.

---

## Risks and open items

- **Transaction size is the ceiling, not compute.** Measured against the built program rather
  than estimated: `raise_dispute`'s transaction serialises to **953 bytes** of the
  1,232-byte limit, leaving 279 bytes of headroom, unchanged since Phase 3. Fourteen
  accounts and a 302-byte Ed25519 instruction carrying the 190-byte receipt account for most
  of it, including the two accounts `#[event_cpi]` adds to every emitting handler, an
  `event_authority` PDA and the program itself. Adding more than eight accounts, or a second
  signature, needs Address Lookup Tables rather than a dropped check. Compute is not a
  single figure: `dispute` is `init`ed with a bare `bump`, so Anchor searches for its
  canonical bump onchain, and one of that PDA's seeds is the seller's freshly generated
  pubkey, so the number of search attempts varies from one buyer/seller pair to the next.
  Phase 4 measured a range of 42,318 to 51,318 CU across repeated runs against the same
  build, comfortably inside the 200,000 default budget throughout.
  `test_raise_dispute_transaction_size` and `test_raise_dispute_compute` keep both figures
  honest.
- **No enumeration query path.** Point lookups by PDA are fast and free forever, but "all
  open complaints for marketplace X" needs `getProgramAccounts` with unindexed `memcmp`,
  which degrades as the program grows and which most RPC providers rate-limit. Anything that
  needs to scan requires an offchain indexer reading the event stream. Budget for it before
  building a UI.
- **No external reputation read.** Another program cannot read a seller's record without
  coupling to this program's exact Anchor and Borsh versions. A small `export_reputation`
  handler returning a frozen byte layout via `set_return_data` would fix that. Deferred, and
  worth doing before anyone integrates.
- **Per-marketplace write-lock contention.** One marketplace account and one bond vault
  serialise that marketplace's dispute throughput. Not a near-term problem; the fix is
  sharded counters.
- **Upgrade authority is a single ordinary keypair, not a multisig.** The deployed program's
  authority is `EE4skmuEcaL4ybktFhp7sUfr84to78KQKoNsAAu8L7jG`, the same key as
  `constants::INITIAL_ADMIN`, confirmed by `solana program show`. The README does not claim
  otherwise; nothing in it mentions upgrade authority, Squads, or a multisig. Moving the
  protocol authority and the upgrade authority to a Squads multisig, and saying so in the
  README, is a known future step, not yet done. Programs on Solana are normally upgradable
  so authors can ship fixes; the claim worth making once that move happens is that the
  deployed rules cannot be bypassed, not that the code is frozen.
- **Parameter defaults:** `bond_bps` 1000 (10%), protocol ceiling 2000 (20%);
  `complaint_window` 2 to 30 days, demo marketplaces at 2, 7 and 30 (the third also at the
  0 bps end of the legal bond range); `DISPUTE_EXPIRY` 30 days,
  fixed; no minimum stake.
- **Mainnet needs legal advice first**, particularly for marketplaces trading crypto against
  local cash. Not because this program does anything different, but because of who its
  customers would be. Paxful shut down permanently in November 2025 after pleading guilty to
  operating an unlicensed money transmitting business. Devnet, a demo, and a grant
  application carry none of that exposure.
- **Deferred deliberately:** frontend, SAS attestation mirroring, appeals, partial refunds,
  leverage mode, stored buyer reputation, `export_reputation`, marketplace registration
  bonds, and marketplace-signed sales-volume summaries (which would restore the onchain
  sales denominator lost in decision 5).

---

## Changes from the first version of this plan

Written for whoever compares this against the audit in
[AUDIT-v2-FINDINGS.md](AUDIT-v2-FINDINGS.md). Delete this section once the code is built.

1. **The framing changed first, and everything else followed.** The previous version
   presented receipts, the rate limit and the buyer bond as if they constrained the
   marketplace. They constrain strangers and chancers. Naming the marketplace as trusted up
   to the permit cap (decision 2) is what made the rest of the list decidable.
2. **The freeze bug is fixed** by scoping open complaints to the permit and adding
   permissionless expiry (decision 7). Expiry resolves to nobody rather than to the buyer,
   which the audit recommended; auto-upholding hands every genuine buyer a free option to
   keep the goods and reclaim the money whenever a marketplace is slow to answer.
3. **The unstake cooldown is deleted entirely** (decision 8). `pending_unstake` and
   `unstake_ready_at` are gone from `SellerStake`. Gating withdrawal on `committed` and
   releasing permits only after their window does the same job with one timer instead of two,
   and closes the exit scam the audit found in the 7-day-versus-14-day mismatch.
   `committed` is also redefined as the sum of *remaining* permit allowances rather than of
   caps, which is what the audit meant by TS-20. Defined as caps, a slash lowers `staked`
   without lowering `committed`, and a fully committed seller becomes unslashable and unable
   to withdraw ever again.
4. **The daily slash cap is removed** (decision 13), along with `total_permitted`,
   `slashed_today` and `slash_window_start`. It delays theft by a day without giving anyone
   a lever to pull in that day, and it shortchanges honest buyers with large claims.
5. **`leverage_bps` is removed** from `SellerStake`. A field permanently equal to 10,000 is
   a claim the program does not honour. The version byte and reserved padding provide the
   same forward compatibility without the untruth.
6. **Marketplaces gained an immutable ID** and two-step authority transfer, so keys can
   rotate without orphaning permits (decision 11).
7. **Permit terms are frozen at grant; keys stay live** (decision 10), with old receipts
   surviving a signer rotation so an honest key change does not void pending claims.
8. **Complaint records are now closable** with the rent refunded (decision 9). The previous
   plan kept them forever, which made filing cost roughly $0.35 on top of the bond and made
   small-order complaints uneconomic, in a product aimed at small orders.
9. **Over-cap claims are recorded rather than rejected**, so the public loss count reflects
   how many buyers were actually harmed.
10. **The complaint window became per-marketplace within a 2-to-30-day range**
    (decision 12), so cash-trading and shipped-goods marketplaces can both use the program.
11. **The demo covers two marketplaces, not one** (decision 14), because portability and
    permit-scoped freezing are invisible with one.
12. **The frontend is no longer described as out of scope**; a separate tracking document
    previously contradicted this file by calling it the highest-leverage remaining item.
13. **One correction to the previous risk list, and one risk it missed.** Compute was never
    the constraint; transaction size is. But the feasibility review's claim that LiteSVM
    0.10.0 supports precompiles behind a feature flag is unverified, and `cargo tree -d`
    shows the repo's `solana-*` crates are already split across major versions today. Phase 0
    exists to settle both before any dispute code is written.
14. **Versioned PDA seeds replace the "redeploy under a fresh program ID" plan.** That plan
    only worked because the prototype holds nothing of value; the same move against a funded
    deployment would strand seller collateral permanently.
