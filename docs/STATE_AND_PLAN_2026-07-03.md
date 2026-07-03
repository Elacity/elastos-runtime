# State of the System + Master Plan (2026-07-03)

An eight-seat 0.01% assessment — professionals, an innovator, a visionary, an
advisor, and the four stakeholder voices — reconciled into one grade, one
architecture verdict, one UX verdict, the vision, and a single ordered plan.

## Grade today — **6.5 / 10** ("world-class core, unfinished edges, unproven market")

| Dimension | Grade | Verdict |
|---|---|---|
| Capability Security | **8** | Genuinely fail-closed enforcement spine |
| Cryptography & DRM | **8** | Post-quantum hybrid seal + threshold quorum, sound |
| Architecture | **7** | Clean primitives; the *trusted core is inverted* |
| Code Quality | **7** | Excellent ratchet culture; two god-files, one now-typed money path |
| Product & Monetization | **6** | Real moat, but it lives in the pre-release runtime; no traction |
| Performance | **5→** | The fsync ceiling is now **measured** (~789 durable emits/s); group-commit is the lever |
| User Experience | **5** | One beautiful room; three design systems; the money path shouts "Web3" |
| Vision / GTM | *strong* | A real category to create, entered on already-built primitives |

**The shape:** an **8 on the hard core** (security, crypto) and a **5 on the
edges** (UX, and the two *unbuilt ends* — the buy-UI finish and Tier-3 execute).
The expensive, risky middle is done; the cheap, visible ends are not. That is an
unusually good position — you buy the grade up with edge work, not core rewrites.

## Architecture verdict — right primitives, not-yet-optimal ordering

**Is it the best/most-optimised/right way?** The *primitives* are — the capability
token (consent = trade = audit in one fail-closed object), the provider/carrier
seam, the signed hash-chained audit, the threshold DRM. These are best-in-class.

**The structure is not yet optimally ordered.** The #1 debt: the **trusted core is
inverted** — `elastos-server` is ~170k LOC holding app/service logic (`content.rs`
13k, `library.rs` 7.6k) *inside* the trusted boundary, while the real TCB
(`elastos-runtime`) is ~24k. This directly weakens the "small enough to trust"
argument the whole security thesis rests on. Second: the **reach/"halo"
enforcement model is built but wired nowhere** — so "enforce, not log" is currently
*self-declared*, not core-computed. Third: god-files and a hardcoded
`RESERVED_SUB_NAMES` couple the provider taxonomy to the core.

**The right ordering** (this is the key call): do **not** restructure now. Ship the
wedge first to earn the right to exist; pay down the trusted-core inversion
(ADR-0001) as the deliberate "make-it-best" investment *after* revenue — **except**
wire the halo + unstub the act path sooner, because those make the security *claim
true*, which is load-bearing for the pitch and the enterprise sale.

## UX verdict — a powerful, honest prototype with one finished room

**How it feels:** for ~30 seconds the marketplace storefront convinces you a normal
person could use this — turquoise-and-gold glass, real skeleton loaders,
focus-trapped modals, keyboard-operable cards, and radical honesty (a `live/demo`
pill, no fake progress %). Then the buy sheet renders a raw `unsigned_tx` JSON
blob and the asset page announces KID / ERC1155 / tokenId / tx hashes, and you
remember engineers built it and want you to see the plumbing. The "never say Web3"
ambition and the product are at war.

- **Positives:** 0.01%-tier *real* accessibility in the storefront; clean
  information architecture; honesty-as-UX; passkey (not seed-phrase) auth; genuine
  motion craft.
- **Drawbacks:** three design systems across storefront/creator/home; the money
  path leaks Web3 at every step; `curl | bash` + CLI install disqualifies the
  target user; the creator flow reads like an internal tool; the open/playback
  payoff moment is the least-designed screen; the signed-fact consent UI (the
  conceptual heart) is spec-only.
- **UX wedges:** hide the plumbing behind a "details for nerds" disclosure *(low)*;
  unify to one design system + gate drift *(med)*; real installer → passkey *(high)*;
  managed-wallet + fiat on-ramp as the default buy *(high)*; design the
  open/playback moment *(med)*.

## The vision — **Sealed-Rights Commerce**

A licensing rail where the unit of trade is a **revocable, enforced key bound to an
on-chain right**, not a copyable file. The NFT era sold receipts without locks;
this binds the token to the lock.

- **The wedge:** premium creators and talent being AI-cloned — voice actors,
  likeness rights-holders, high-end photographers/3D artists — who sell *access to*
  a likeness/voice opened inside a containment boundary (buyer sees frames, never
  the file/key/weights), with **training rights a separate grant defaulting to
  FALSE**, at **2% vs platforms' 30–55%**. Runs on already-built, smoke-tested
  primitives (the open→rights→key→decrypt loop is ~99% end-to-end).
- **Why now:** AI cloning made "sell it but keep control" survival, not
  convenience; 2025–26 law (NO FAKES, EU AI Act Art. 12/14) turned consent +
  provenance from nice-to-have into a budgeted mandate — logs no longer suffice,
  enforcement is required.
- **3-year:** the neutral clearing-and-enforcement rail for AI-era IP; the sealed
  catalog becomes the consented-provenance registry AI buyers must clear against;
  the per-open key-release becomes both the toll and the liability artifact; Tier-3
  makes it the one place you can sell a runnable model whose weights never leave.

## The single ordered master plan (by leverage)

**NOW — done this session:** MKT-1 money-path fix (red-teamed) · `MintError`
typing · first benchmark (perf estimate → measured) · the audit + one-pager.

**Q1 — earn the right to exist (the one bet):**
1. **Finish the draft buy-UI gate → close ONE named-creator, receipt-backed sale.**
   The sole remaining gate is UI wiring, not new crypto. This is the first metric.
2. **Hide the plumbing** (UX wedge #1, low effort) — a normal buyer sees price,
   "you'll own this," Confirm — not a serialized transaction.
3. **Managed-wallet "Buy with balance" default + fiat on-ramp** — or the money path
   stays expert-only.

**Q2 — reframe into the category (the centerpiece):**
4. **Ship ONE Tier-3 "runnable model, weights never leave the capsule" demo** — the
   move that converts "another 2%-vs-45% marketplace" into a category only the
   capsule model can serve.
5. **Wire the reach/egress model + unstub the user-approval act path** — makes
   "enforce, not log" *true*; the technical-credibility spine for the pitch.
6. **Unify the design system** across creator + viewers; gate drift in CI.
7. **Pin + KAT-test the PQ crates** (`ml-dsa`/`ml-kem`) against FIPS 203/204 — the
   real crypto-critical dependency.

**Q3+ — scale + make "the best" true:**
8. **Audit group-commit** (now that ~789/s is measured) — lift the durable ceiling.
9. **Diversify dKMS nodes out of one trust domain** + recover-proof↔reseal-AAD
   binding + external audit-chain anchoring — make "decentralized" honest.
10. **Pay down the trusted-core inversion** (ADR-0001: extract god-files, shrink the
    TCB) — restore "small enough to trust."
11. **Carrier peer authentication** (G-CARRIER-PEER).

## The one strategic bet

Close **one on-chain, receipt-backed likeness/voice sale on the already-built loop
this quarter**, and spend that credibility to build the **Tier-3 demo**. Every seat
— product, vision, architecture, security — converges on this exact sequence.
