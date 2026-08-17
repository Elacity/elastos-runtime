# PDR — ElastOS, the Sovereign Computer (Flint & Bella)

The definitive product spec, from council swarm `w878qm5jv` (9-expert audit+frame → spine →
4-lens adversarial pressure-test → PDR). The runtime expert audited the LIVE tree at
`/Users/sash/code/elastos-runtime` (branch `feat/ddrm-hardening-and-creator-parity`) — the
file:line citations below are real and actionable by Cursor. Builds on `FLINT_KEEP_CONCEPT.md` +
`FLINT_SHELL_VISION.md`. Honest by construction; every claim is graded shipped vs to-build.

> NOTE ON TREES: my prior adoption-wedge commits + these docs live on the `flint` tree; the code
> findings here reference the founder's `elastos-runtime` tree. The two should be reconciled
> (e.g. `elastos mcp serve` was built on `flint` but is NOT in the `elastos-runtime` tree the
> agents audited). Treat the file:line citations as pointing at `elastos-runtime`.

## Positioning (the honest reframe)
ElastOS = the Sovereign Computer. Consumer surface: the agents **Flint** (boy) + **Bella** (girl).
Company/protocol: Elacity. It is **NOT** "an AI operating system" and **NOT** a genius-on-your-desk.
It is **a consent layer and a cryptographic flight recorder for AI agents**: no actor — not you,
not your local agent, not an external AI over MCP — ever touches a resource, a key, or money
without passing the same five-beat gate and leaving a signed, tamper-evident receipt. Sold
free-and-sovereign to people; sold as the verifiable agent-containment black box to enterprise.

## What is already real (audited in tree, ~9/10)
- Capability plane: scoped/expiring/revocable ed25519 tokens through a real grant-evaluation
  pipeline (`capability/policy.rs`); 12-check validate on the dDRM viewer plane.
- ed25519 hash-chained receipt log (`primitives/audit.rs`: SHA-256 over domain/seq/prev_hash/event,
  per-record signature, `verify_chain`, an ML-DSA agility tag reserved).
- Dual-tier isolation: wasmtime (StoreLimits, epoch-deadline interrupt, preopened-dir FIFO
  carriers, no inherited socket) + a full crosvm MicroVM lifecycle that is **default-no-internet**.
- dDRM rights plane (uncopyable): 2-of-3 Shamir dKMS with per-node on-chain rights re-check that
  fails closed; transcript-welded replay-proof decrypt; key USED-not-owned.
- Local LLM is further along than assumed: `llama-provider` already spawns + health-polls +
  restarts a `llama-server` child over line-delimited JSON. The agent capsule already drives
  resources via `carrier_invoke` with a `cap_token`.

## The two honest corrections this PDR refuses to paper over
1. **The halo is currently a LIE.** `AffordanceRisk` is SELF-DECLARED by the capsule
   (`manifest.rs:199`); the `RuleCheck` enum (`capability/policy.rs:357`) has nine variants and
   **none is egress, destination, or reach**; "reversibility" exists only as privacy fingerprints.
   So today the two-channel halo would invent its verdict, and a malicious capsule could declare
   Payment as Read. **NOW item zero: make risk/reach a CORE-COMPUTED verdict** derived from what the
   token actually does (invokes `buy_authority`, releases a CEK, opens egress) so the picture
   genuinely cannot lie. This is the honest-manifestation moat made real rather than asserted.
2. **No per-capsule network EGRESS control exists.** Gating files/actions while a capsule opens
   arbitrary sockets is not containment — the top exfiltration vector for a 24/7 box that ingests
   devices and trades box-to-box. **Build egress-as-capability FIRST** (add an Egress/Destination
   variant to `RuleCheck`, default-deny, per-destination allowlist, enforce at the WASM FIFO
   boundary + the crosvm network path, render into the halo, emit every outbound to the chain).
   ADD-only over the existing default-no-internet posture.

Also: the User-approval ACT path is STUBBED (`gateway_capsule_catalog.rs:336` returns "not enabled
yet"); unstubbing it so ONE real human-gated act completes end-to-end is the single biggest unlock.

## Validated tech verdicts
- **RUST + WASM + MicroVM: VALIDATED** — for TRUST + PROVABILITY, not as a universal UX-speed claim.
  Memory-safety + capability-confinement is the right substrate for a gate whose product IS its
  trustworthiness. Sell PROVABILITY, never raw speed. The shell must NOT be Rust-native.
- **GODOT: REJECTED** (the founder's bias suspicion was correct). A talk-to-it sovereign computer is
  a conversation + consent cards + a standing-authority sidebar + windowed results = a text-heavy,
  streaming, **web-native** surface. Build the shell on TypeScript + a thin reactive renderer
  (lean Svelte for bundle/snappiness; React fine — the renderer is not the moat). Home is ALREADY
  browser-hosted in tree. The shell is a read-only PROJECTION of typed runtime facts over a local
  IPC/websocket bridge; it must NEVER render anything not backed by a real receipt, so it ships
  BEHIND the unstubbed act path. Reserve a spatial engine ONLY for a LATER capsule-city delight.
- **Local AI: downgraded to a defensible 2026 spec** — local = REFLEX + ROUTER + private-data RAG
  (3-8B consumer / 14-32B prosumer / 70B appliance, 4-8bit); frontier-over-MCP is the BRAIN; hybrid
  by default. The PLAN step is a DETERMINISTIC hand-written grounded composer (NO model in the loop)
  in NOW; the model touches PLAN only in NEXT, behind the meter, gated by a measured escalation-rate
  ceiling (an open EVAL problem, not "wiring").
- **P2P (iroh carrier): sound + load-bearing** (already transports dKMS Shamir shares). Box-to-box is
  feasible as transport but NOT autonomous commerce until the spend-cap meter + egress exist;
  residential NAT/offline ⇒ async store-and-forward with the chain as the always-on settlement party.
- **The agentic IDE: research-track only.** "Generated code is inert until signed" secures the
  OUTPUT, not the BUILD ENVIRONMENT — which needs egress for deps, a writable toolchain, and
  long-lived compute, each a hole in the posture that earns the audit grade. The graveyard of
  CI-sandbox guest-escape + dependency-confusion CVEs lives here. Defer hard behind egress, ephemeral
  per-build rootfs, a pinned vetted dependency mirror, and a crosvm exploit-mitigation review.
- **The dual-receipt trust-vs-latency fork:** the self-attestation is signed under the same runtime
  key a live-compromised box controls (audit.rs's own header admits this). Split it: Tier-A (instant
  local ed25519 co-sign on the critical path, <16ms, the vault feel) + Tier-B (async batched
  Merkle-anchoring of chain heads to Base, off the critical path = the group-commit perf win). Get a
  BLOCKING legal opinion on Art-12/14 admissibility before building the enterprise appliance.

## The capability map (ordered by how it compounds)
- LAYER 1 — INTELLIGENCE FROM DEVICES (daily habit + the private-data moat): gated/budgeted
  ingestion of calendar/location/smart-device data → a daily brief in the Review; a local 14-32B
  model can BEAT a frontier model because it holds context the cloud never sees. Sell sovereign
  AUTHORITY, not sovereign privacy — the halo goes hot if data leaves the box. Needs egress first.
- LAYER 2 — CREATOR dDRM PACKAGING (uncopyable, two-sided supply): point the agent at a media file →
  deterministic metadata extraction (NOT "deciphering value"; auto-appraisal is LATER, model-gated)
  → author a typed signed RIGHTS DESCRIPTOR (opType, territory, exclusivity, duration,
  derivativeAllowed, royalty splits to DIDs, a DISTINCT `trainAllowed` defaulting FALSE) → two-channel
  gate → mint to Base → pin → list. The agent ASSEMBLES + DEFENSIVELY REVIEWS (flags missing splits,
  train-allowed-without-confirm, scope-exceeds-upstream); it never silently values. Terms bind into
  the decrypt transcript = enforced at key-release, not merely displayed.
- LAYER 3 — RIGHTS TRADING (the multibillion rail): inventory = a scoped/expiring/revocable
  capability token over a dDRM capsule carrying a signed policy (split_table, price_schedule,
  rights_class). Three pricing primitives ≈ 90% of demand: buy-to-access, subscription (auto-renew +
  periodic reach budget), pay-per-use (metered on receipts, BATCH-settled so sub-cent survives fees).
  Flat 5% protocol fee in the split table (below App-Store 30%). Stablecoin-settled, token-OPTIONAL.
  Agent-as-buyer NOW within a human-set Grant-Garden budget; agent-as-seller + autonomous A2A LATER.
- LAYER 4 — PORTFOLIO-TOKEN SERVICES (north-star illustration, NOT near-term): **the canonical
  "bought a music catalogue, train a model, sell generation" example does NOT close** and must be
  CUT from all external materials — training rights are negotiated per-master/per-composition (not
  click-bought from clean indie supply), a competitive model is 8-figure capex, and output copyright
  is unsettled. dDRM enforces the license TERM; it cannot create the license MARKET or pay the
  training bill. `trainAllowed` is a SEPARATE, default-FALSE, separately-priced, dual-receipted grant.
  The rights economy closes as Layer 3 metered access, never as Layer 4 agent-trains-and-resells.
- LAYER 5 — BUILD-ON-DEMAND IDE (flagship the consent architecture exists to make safe; LATER,
  research-gated): a crosvm capsule whose capability set EXCLUDES signing/runtime-control/cross-
  principal authority; its only outputs are creator-scoped source + a new UNSIGNED capsule that
  cannot run until a human passes the publish-and-sign gate. Structural for the OUTPUT; the hard part
  is the BUILD ENVIRONMENT (above).
- ORDERING: 1 → 2 → 3 is the compounding spine, all buildable on shipped primitives. 4 and 5 are
  amplifiers that switch on only once liquidity, legal frames, and the hardened jail exist.

## Flint & Bella (the hard invariant)
A persistent, warm, sovereign IDENTITY you talk to — but a ZERO-STANDING-AUTHORITY NARRATOR that
ROUTES to the best brain (local for reflex/routing/RAG; frontier over MCP for hard reasoning) and
settles every consequential proposal into a witnessable five-beat card. Warmth lives in HOW it
explains, never in WHAT it may do (affective trust is exactly the automation-bias the gate defeats).
A capsule with a distinct principal + short-lived grants; every act passes the same gate + mints the
same receipt as if you did it; aggregate-reach budgets + a true all-stop mean no silent authority.
"Talk to it from anywhere" = a DID-authenticated thin-client relay to the always-on box (not the
model on your phone); v1 scopes "anywhere" to same-LAN or a cloud-hosted box and says so. The persona
ships LATER, OFF public copy until NL behind the meter is live.

## Realistic phasing
- NOW (0-6mo; the only success criterion = "did the flywheel turn once"): (0) core-computed
  risk/reach verdict; (1) egress-as-capability; (2) unstub the User-approval act path; (3) ONE hero
  ACT — a live ela.city dDRM cross-identity open (`api/viewer_open.rs`), key used-not-owned, co-signed
  dual receipt, with a benign always-completing fallback; (4) the thin-slice WEB shell rendering the
  two-channel object + dual receipt from THAT real act (nothing unbacked); (5) dual receipt + the
  Grant Garden with a TRUE all-stop wired to real revocation; (6) answer the 6 founder decisions
  first, take the reflink/COW perf win, fix the VM-lifecycle bug cluster.
- NEXT (6-15mo, funded by enterprise revenue): the METER (spend-cap + signed per-call audit +
  DID-bound output) as a thin wrapper, with a local-model GO/NO-GO eval gate BEFORE any
  model-in-the-plan-loop work; act-over-MCP (build `elastos mcp serve` in this tree); free-text NL
  turns on HERE behind the meter; Venice-x402 as first remote cartridge; inference-output-born-as-dDRM;
  the creator wizard; the enterprise dual-receipt audit appliance as first paid SKU.
- LATER (15-30+mo, research-gated, narrative-only until true): woven typed pipelines; capped
  autonomous box-to-box; hub directory → Kademlia-over-iroh; the DERIVE/trainAllowed type; confidential
  local inference; the agentic IDE; decentralized/attested dKMS; the branded hardware box; the persona.
- INVARIANT: each band ships a lovable artifact before the next unlocks; never let a later item
  consume a NOW engineer.

## The multibillion opportunity + moat
Be THE CONSENT-AND-CONTAINMENT LAYER FOR THE AI-AGENT ECONOMY — "Stripe for the agent economy's
consent-and-proof layer." Wedge NOW = ENTERPRISE AGENT-CONTAINMENT AUDIT (a net-new category under EU
AI Act Art 12/14, NIST AI RMF, SOC2-for-agents; the buyer has a board mandate and no answer to "prove
what your agent did"; the dual receipt is the exact artifact regulators + insurers force). NEXT =
creator-rights monetization via dDRM (a $25B+ licensing market on unenforced PDFs, plus the AI-training
provenance mandate, sellable to BOTH sellers and buyers). LATER = the sovereign-computer/box-to-box
economy. MODEL: consumer free-and-sovereign (zero subsidy; the dream that pulls users + supply, NOT a
funded GTM); enterprise pays for the containment appliance (the only near-term revenue); flat 5%
protocol fee (a cost center until GMV exists — say so). MOAT: the CAPABILITY TOKEN is triple-duty —
consent = trade = audit — fusing enforce + meter + prove in one substrate (governance SaaS can prove
not enforce; a lab logs its own homework; a cloud locks you in). REFRAME to survive the real threat
(platform-absorption by the labs whose MCP you depend on + the clouds where agents run): the NEUTRAL,
PORTABLE, MODEL-AGNOSTIC, CLOUD-AGNOSTIC proof layer the platforms structurally won't be (the
Switzerland argument). Lead enterprise sales with "independent + portable," not "sovereign computer."
Plus: "the product is the router, not the GPU"; dDRM keys-used-never-owned (no competitor analog,
extends to AI-inference-output with zero new crypto); receipts as a switching cost.

## Partners + GTM (revenue before strategic dependency)
(1) A Big-4 AI-assurance channel FIRST (reaches the regulated buyer you can't cold; lacks your
evidence primitive). (2) A regulated EU-AI-Act design-partner enterprise (reference deployment +
validates dual-receipt admissibility — a BLOCKING legal opinion gates the appliance). (3) Anthropic/MCP
— make `elastos mcp serve` the canonical SAFE WRITE PATH, propose receipt-tagging + gate-preview as a
standard; manage as COOPETITION (your #1 partner is your #1 absorption risk). (4) Base/Coinbase (USDC,
x402, audit-chain anchoring — load-bearing if self-attestation is ruled insufficient). (5) Venice/NEAR/
Phala (first attested external-inference counterparty, Base-native). (6) Indie creators who own 100% of
their rights + C2PA/Content-Authenticity alignment. (7) UniCity/Astrid — reconnect at STANDARDS level,
avoid product collision. DISCIPLINE: pick ONE company for 18 months (the enterprise containment audit);
marketplace/box/persona are optionality funded by enterprise revenue — the shared substrate is R&D
efficiency, NOT a shared GTM. The two-market trap is boil-the-ocean through the back door.

## The flywheel (v1 success = "did it turn once")
One creator seals one real dDRM asset with a rights descriptor → a second identity's agent opens it
under the gate, key USED-not-owned → both hold a co-signed dual receipt → the seller sees a revenue
event → the Review surfaces it. Create → gate → consume → attested receipt → revenue. Four arms on ONE
primitive (consent = trade = audit): data gravity (weakest, no funded consumer engine — narrative);
supply → liquidity → reputation; proof → trust → bounded autonomy → more proof; model-agnostic ride
(the brain is a swappable capsule, every model gain accrues free). White-hat closure: the engagement
vector (the warm Review, the satisfying seal) IS the safety vector. HONEST CAVEAT: arms 1-2 have no
funded engine; enterprise revenue funds the company; the flywheel is the product vision, NOT the raise.

## Where to focus NOW (the one compounding thing)
Turn the shipped, audited gate into a daily-usable thin-slice shell whose every consequential act
produces an exportable dual receipt — and make the halo TRUTHFUL. Two moves (integration, not
invention): (1) MAKE THE HALO HONEST + THE ACT REAL (core-compute risk/reach + egress-as-capability +
unstub the User-approval path); (2) TURN THE FLYWHEEL ONCE (the loop above). Defer everything else.
Discipline: sell provability not speed; lead with the enterprise containment audit to fund the company;
keep Flint a zero-authority narrator; reject Godot for web; defuse the access-vs-training and
autonomous-spend landmines before they anchor the roadmap; never ship a claim ("it talks," "autonomous
revenue," "powerful local AI," "fully decentralized," "the picture cannot lie") until it is literally true.

## Top risks
1. Does a real human FEEL the weighted gate as PROTECTIVE not friction? Test the heavy one-way seal
   first — if the gate doesn't earn trust, nothing downstream matters.
2. Is the co-signed dual receipt ADMISSIBLE (signed under a key a compromised box controls)? Blocking
   legal opinion + Base anchoring fallback before the appliance.
3. PLATFORM ABSORPTION (the real strategic risk) — mitigation is the Switzerland framing, unproven
   until a regulated buyer accepts it over a hyperscaler's bundled checkbox.
4. Egress-as-capability buildable cleanly across both tiers with a trustworthy halo? Build first, red-team.
5. Is a local 7-32B model good enough as router/composer/RAG, or does everything escalate to frontier?
   GO/NO-GO eval ceiling BEFORE model-in-the-plan-loop; NOW = deterministic composer.
6. ACCESS-vs-TRAINING — retain a derivative-rights legal partner before any training copy; DERIVE as a
   separate default-FALSE SKU; cut the music-catalogue example now.
7. Two-sided cold-start — don't model A2A revenue until creator supply is dense.
8. The agentic IDE's escape surface is the BUILD ENVIRONMENT — its own research track + threat-model doc.
9. 2-of-3 operator-curated dKMS = two colluding operators reconstruct the CEK + buyer-metadata leak;
   DISCLOSE to creators before packaging; phase decentralization explicitly.

## Open decisions for the founder
1. Answer the 6 decisions in `FLINT_SHELL_VISION.md` FIRST (a single founder session this week).
2. ONE COMPANY FOR 18 MONTHS: commit (or decline) the enterprise agent-containment audit as the sole
   funded GTM, marketplace/box/persona as deferred optionality. The highest-leverage strategic choice.
3. THE HERO ACT: confirm a live ela.city dDRM cross-identity open is reachable end-to-end with the
   operator dKMS backend, or accept the benign fallback as the v1 demo.
4. COPY DISCIPLINE: ratify that the five forbidden claims never appear until literally true, and cut
   the music-catalogue / Layer-4 example from all decks + creator conversations now.
5. DUAL-RECEIPT LEGAL GATE: authorize the blocking admissibility opinion + the Base-anchoring fallback
   before the appliance.
6. FLINT vs BELLA at launch: one branded agent or both; persona OFF the critical path + OUT of copy.
7. HARDWARE TIMING: confirm hardware is a LATER premium SKU (software on NUC/cloud-VM/Apple-Silicon/
   NVIDIA now); no silicon before the flywheel pulls.
8. SCOPE OF "ANYWHERE": v1 says "on your LAN / cloud-hosted box" (honest today) vs waiting to claim
   "from anywhere" until the DID-relay with NAT-traversal exists.

Pressure-test verdicts: all four lenses = SHARPEN (none serious-gap), folded in. Hardest problems:
local-model planning reliability (→ deterministic composer in NOW); platform-feature-risk (→ sell the
wedge on its own merits, not the flywheel); the agentic-IDE build environment; the reversibility/reach
verdict being fictional today (→ core-compute it first). Source: swarm `w878qm5jv`.
