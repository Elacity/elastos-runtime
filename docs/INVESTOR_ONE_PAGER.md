# Elacity — One Pager

## The one line
**The runtime for selling anything digital — including AI models and data — where
the buyer can use it but never gets the file, the key, or the weights.**

## The problem
Two things the AI era needs and today's stack cannot provide:
1. **Enforced authority, not logs.** As agents act on our behalf, "we logged that
   the agent did X" is not enough — you need to *prevent* what a fooled agent
   shouldn't do, and *prove* what it was allowed to touch.
2. **Sell AI models & data without giving them away.** A model owner who ships
   weights has lost them. There is no clean way to monetize a model, a dataset, or
   a likeness while keeping control.

## What we built
A **capability-security kernel + decentralized post-quantum DRM**:
- **Encrypt once, mint rights on-chain.** Any file becomes a tradeable access
  token on Base with creator-set price, royalties, and resale rules.
- **Open inside a containment boundary.** Ten viewers across five tiers ship
  today; the buyer sees pixels/frames, never the bytes or the key.
- **No single party holds the key.** A 2-of-3 threshold key-management quorum
  (real distributed key generation) releases keys only against an on-chain
  entitlement — post-quantum (ML-KEM-768 + ML-DSA-65) at the core.
- **Tamper-evident receipts.** A hash-chained, signed audit trail proves the
  custody chain — the kind of record a regulator or insurer can rely on.

## The moat
The defensible intersection — **runtime execute-containment × on-chain rights ×
threshold key custody** — lets us do the one thing incumbent DRM structurally
cannot: **sell a runnable model or game that executes for the buyer while the
binary and weights never leave the sandbox.** That is not a feature bolted on; it
*is* the capsule architecture.

## How it makes money
- **Marketplace fee** on every primary sale (protocol cut, read live on-chain).
- **Resale royalties** — the protocol re-clips a cut on every secondary trade;
  creators earn a recurring reseller royalty.
- **Key-release toll** — every open requires a quorum key release: a meterable
  per-open / subscription line.
- **AI-model & data licensing (the wedge)** — decrypt-to-inference and
  training-rights licensing of models, datasets, likeness and voice, with
  on-chain royalties.

## Where we are (honest)
- **Security & cryptography audit-grade** (8/10 both): fail-closed capability
  enforcement and a post-quantum threshold-DRM core, independently reviewed.
- **Marketplace built end-to-end** against live Base contracts (buy / list /
  resell / royalties).
- **Pre-revenue on the new stack**, by discipline — we publish no vanity metrics.
- **Next milestones:** first public receipt-backed sale, and the flagship Tier-3
  "runnable model, weights never leave" demo.

## The ask
Fund the two milestones that convert audited technology into a category:
one credible, on-chain, receipt-backed sale — and the Tier-3 demo that only the
capsule model can deliver.

*(Internal note: technical diligence should read `docs/AUDIT_2026-07-03.md` for
the honest grade card, the trust-boundary caveats, and the roadmap.)*
