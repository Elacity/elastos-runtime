# North Star for Flint

*Audited by a cross-industry 0.01% panel — local-AI, agentic-product, creator-economy,
tokenized-markets, securities/IP law, and sovereign-systems — each instructed to
push back with a better answer, not applaud. This is the vision laid back, corrected,
and grounded in first principles.*

## The one sentence

**Flint is the trust layer for agentic commerce: say what you want, seal it once,
and an agent turns what you own into income and action under authority you can watch
and revoke.** A broker that works *for* you — not a twin that *is* you.

## The fundamental goal (5-Whys to bedrock)

The stated goal — *"turn data into capital for everyone; let no one be left behind."*
Run it down:

1. *Why Flint?* → so a person can turn what they own into income and action without
   being robbed of the keys, the rights, or the margin.
2. *Why can't they today?* → platforms that hold the keys take the rights and the
   margin; agents that could act run on ambient tokens no one can scope or revoke.
3. *Why does it matter now?* → AI cloning made loss-of-control existential (your
   voice/face used without consent), and 2025–26 law made *enforcement* mandatory,
   not optional.
4. *Why is this the only place it's solvable?* → because the asset only ever opens
   inside a containment boundary bound to an on-chain, **revocable** right —
   enforcement, not a takedown prayer.
5. *Why does that generalize to "not left behind"?* → because one primitive — a
   revocable, enforced, receipted key over an owned asset — is the unit that lets a
   non-technical person safely sell access, license training, or delegate to an
   agent, under authority they can *see and revoke*.

**Corrected bedrock:** the goal is **not** "everyone's data becomes capital" (most
data is worth ~$0 — value comes from *demand*, not from protection). It is: **give
people enforceable control over what they own in the AI economy, so those whose
assets have value keep control and capture it — through an agent they trust because
they can see and revoke exactly what it did.** Sell the *lock* first; the store,
the twin, and the market are upside stacked on that one non-negotiable.

## The two shells (this part holds)

- **ElastOS** — free, open-source, classic desktop OS shell on the runtime.
- **Flint** — a paid (~$20, dDRM-owned) capsule you flip to: the agentic environment,
  same runtime underneath. The ESP protocol makes shells pluggable — which opens a
  **shell marketplace** (Flint is the first paid one, not the last).

This is sound: dDRM is already the metering rail, so "own a $20 shell" and "sell
access to a service" are the *same* entitlement primitive. **Caveat (funding):** a
one-time $20 fee cannot fund continuous inference or a mesh others subscribe to —
price the *agent's* ongoing work (usage/subscription), not just the shell license.

## What Flint IS — and is NOT (yet)

**IS:** an **intent composer/broker** grounded in reachable capabilities — your own
`docs/FLINT_SHELL_VISION.md` already nailed it: *"an intent STREAM, not a chatbot…
a command COMPOSER that cannot propose a hallucinated action."* Keep that. Plus a
marketplace **terminal** (browse/buy/subscribe/download) and a **local runner**
(assets open in a window, sandboxed).

**IS NOT (yet), and don't pitch as if it is:**
- a **chatbot twin that *is* you** and that strangers subscribe to — a trust/liability
  tarpit and the least-built part;
- a **frontier model running at home** — frontier is defined by max compute; home
  will always trail;
- a **public yield market** — that's an unregistered security until gated.

## The five corrections (pushback → better answer)

1. **"Run frontier AI at home" → "Frontier is rented; YOUR model is owned."** A 2026
   home box runs a strong quantized 20–70B, not GPT-5-class. The "digital twin" is
   **RAG over your DRM-licensed catalog + a light LoRA/voice adapter + long-lived
   memory**, and hard tasks **hybrid-route to a frontier API *through the DRM
   boundary*** so raw data never leaves. Local wins on **sovereignty, privacy, and
   the marginal cost of *your* tokens — never on raw capability.** Sell that.

2. **"A twin that IS you, negotiating, subscribed-to" → a capability broker + a
   revocable power-of-attorney.** Lead with *"say what you want; it assembles a
   scoped, revocable, receipted plan; you seal it once at the gate; it runs locally
   and hands you a signed receipt."* Each autonomous negotiation is itself a scoped
   capability with a spending cap and an audit log — opt-in, per-deal, shipped later.
   **Sell the seal, not the sorcery.**

3. **"Data→capital for everyone / LimeWire" → "Stripe for your likeness," aimed at
   the already-cloned.** Target creators with *provable existing demand and an active
   theft problem* — mid-tier **voice actors / VO artists / narrators / VTubers**,
   cloned today, technically literate, organized in guilds. Message: *"stop others
   turning your likeness into their capital — then license it on your terms."* Drop
   "LimeWire" (it signals piracy and unpaid work to the pros you're courting).
   **Sell a lock before a store.**

4. **"Non-correlated royalty hedge, sold to funds" → it's a *derivative of GMV*.** A
   new creator's on-chain royalty is 100% dependent on *your* marketplace's traffic —
   *perfectly correlated with your own DAU*, the opposite of a hedge. Build it **after
   ~12 months of real metered revenue**, and then sell **diversified index tokens**
   (top-N by trailing on-chain revenue) with a designated market-maker — **not 10,000
   illiquid dust tokens.** Seed liquidity from creators financing themselves + one
   anchor crypto-native fund. Not first.

5. **"Sovereign 24/7 nodes others subscribe to" → blocked on two keystones, not on
   Tier-3.** Isolation is real (wasmtime + crosvm, hardware-verified) and Carrier
   transport is real (iroh/pkarr). But (a) the Carrier inbound plane has **no peer
   auth** (`G-CARRIER-PEER`) so it's locked *read-only* — it **cannot carry
   "subscribe to my agent"** today; and (b) **"buy a model and run it, weights never
   leave" needs a TEE that is entirely a design doc** (plain crosvm has no attestation).
   **"Sovereign" today honestly means self-hosted, single-trust-domain, unattested.**

## The honest capability ladder (say this at each step)

- **Now:** a **local 20–70B agent** doing RAG over your DRM-licensed catalog, tool-use,
  and packaging/minting — private, owned, personalized; hard tasks routed to frontier
  through the boundary. Never say "frontier" or "trained your own model."
- **~18mo:** overnight **LoRA "twin adapter"**; agent-to-agent negotiation between
  users' local agents; **RAG-gated subscription access** to your curated catalog.
- **~3yr:** local crosses the *2024-frontier* line; genuine (small) 24/7 serving —
  *only* once the mesh does failover so no single home box is the SLA, and only once
  nodes are **attested**.

## What must be true to be legitimate (build the gate, don't bolt it on)

*Substance beats form: you can't tokenize out of a security or contract out of a
personality right.*

1. Royalty tokens = **accredited/institutional only**, real exemption (Reg D 506(c) /
   Reg S), **on-chain transfer restrictions** (permissioned tokens), KYC/AML — a
   non-verified wallet is *technically* impossible, not just forbidden by T&Cs.
2. Bundles via **SPV + registered/exempt adviser**; the platform is *infrastructure*,
   never the pool manager or yield-promoter. Kill retail "buy yield" framing.
3. Train-rights = **default-FALSE, purpose-scoped, revocable, identity-verified**;
   block minor/coerced/estate-invalid grants; retain GDPR/BIPA-grade consent records.
   *(This grant is the single strongest thing already designed — keep it granular.)*
4. **Revocation propagates downstream** — withdrawal disables live twin/agent instances
   and quarantines derived models, with an audit trail proving it. *(You can revoke a
   license on-chain but you cannot un-train a model — close this or "enforceable
   revocation" is cosmetic.)*
5. Likeness/voice twins carry **separate, specific, NO-FAKES-compliant written
   consent** — never a generic ToS click.
6. **Three business lines kept legally separate** (content marketplace / royalty
   securities / data-&-agent services) so a securities problem can't collapse the
   runtime; geofence by jurisdiction.

## Blind spots we are tracking

- **Demand has no motion.** The vision is 90% supply-side; buyers won't just appear.
  → *Concierge the first 100 transactions by hand; seed demand from the legal-clearing
  side (brands/AI labs that must source consented likeness).*
- **24/7 residential-node economics + liveness.** An always-on box holding a 70B to
  serve near-zero requests is a bad cost structure, and a home node dark half the day
  is a settlement record, not a service. → *Model watts × utilization × price before
  promising it; lean on the mesh for failover.*
- **Revocation ≠ un-training** (above).
- **One-time $20 can't fund continuous inference** → price the agent's ongoing work.
- **Royalty token as a public security** → the gate above, or an enforcement action.

## Revised build sequence (the one change from the prior plan)

The prior roadmap parked **Carrier peer authentication** at "LATER (item 11)." The
panel's strongest structural finding: it's a **keystone, not a cleanup** — the literal
precondition for *both* the agent-subscription economy *and* attested permissionless
nodes. **Promote it.**

1. **Close one receipt-backed likeness/voice sale** on the built loop (the gate is
   buy-UI, not crypto) + **hide the plumbing** in the buy sheet.  *[the first metric]*
2. **Carrier peer authentication (G-CARRIER-PEER)** — verify the iroh node-id against
   an allowlist + signed request envelope + inject a verified principal. *Unlocks the
   whole mesh pillar.*  *(promoted from 11 → 2)*
3. **Ship the Tier-3 "runnable model, weights never leave" demo** — the category bet.
4. **Wire the reach/act enforcement** — makes "enforce, not log" true.
5. **Pin the PQ crates** (`ml-dsa`/`ml-kem`); **unify the design system**.
6. **TEE attestation** for permissionless nodes; **diversify the dKMS quorum** out of
   one trust domain — the two things that make "sovereign" *true*.
7. Royalty index market — **only after ~12 months of real GMV**, fully compliance-gated.
8. Pay down the trusted-core inversion (ADR-0001) — restore "small enough to trust."

## The one strategic bet (every seat converged here)

**Close one on-chain, receipt-backed likeness/voice sale this quarter on the
already-built loop — then spend that credibility to authenticate the mesh
(G-CARRIER-PEER) and build the Tier-3 demo.** Protection first, then the store, then
the agent economy, then — much later, gated — the market. That is the sequence that
turns "another marketplace" into **the trust layer for agentic commerce.**
