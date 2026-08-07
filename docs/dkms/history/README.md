# history — convergence-era working notes

These are **historical documents**, kept deliberately. They are the working notes,
design decisions, audits and plans produced while the dKMS / dDRM / commerce rails were
being built (the "convergence" period — bringing the Elacity / PC2 product into the
ElastOS runtime as a Rust engine), plus the superseded marketplace planning docs.

**Read them for *why*, never for *where are we now*.** Most are day-logged snapshots:
they carry status lines, sprint counts and "next step" sections that were true on the day
they were written and are not maintained. The current, code-checked picture lives one
directory up:

- [../README.md](../README.md) — the onboarding entry
- [../ARCHITECTURE.md](../ARCHITECTURE.md) — the current architecture
- [../SECURITY_MODEL.md](../SECURITY_MODEL.md) — the current security model
- [../COMMERCE.md](../COMMERCE.md) — the current commerce rail
- [../MEDIA_PIPELINE.md](../MEDIA_PIPELINE.md), [../VIEWER_SESSIONS.md](../VIEWER_SESSIONS.md)

Where one of these pages disagrees with the code on this branch, **the code wins** — and
so does the hub page above, which was written against the code.

What they are worth keeping for: the *rationale*. Why the decrypt boundary is shaped the
way it is, why the CEK is minted in-boundary rather than handed in, what PC2 did and what
we deliberately did not port, why the marketplace mints nothing and plays nothing. That
reasoning is expensive to reconstruct and is not repeated in the current pages.

---

## Convergence — architecture and rationale

| Document | What it is |
|---|---|
| [CONVERGENCE_PLAYBOOK.md](CONVERGENCE_PLAYBOOK.md) | The North Star alignment doc for the whole multi-month convergence effort — first principles, working method, the durable "how". |
| [PRODUCT_VISION.md](PRODUCT_VISION.md) | The companion "why/what": product vision and PRD, the macro picture behind the engineering. |
| [SYSTEM_ARCHITECTURE_MAP.md](SYSTEM_ARCHITECTURE_MAP.md) | Whole-system map of the PC2/Elacity content journey (creator → publish → market → purchase → key → decrypt → playback) against what the runtime had, with the target architecture. |
| [CONVERGENCE_AUDIT.md](CONVERGENCE_AUDIT.md) | The honest bird's-eye audit: what was built, what was missing, and the roadmap to the full loop for media *and* non-media assets. |
| [STRATEGIC_ROADMAP.md](STRATEGIC_ROADMAP.md) | Sequencing and timing across the two portals (Create, Market), the legacy Lit path, and the distributed key rail. |

## dDRM — the boundary decisions

| Document | What it is |
|---|---|
| [DDRM_ENCRYPT_INVARIANT.md](DDRM_ENCRYPT_INVARIANT.md) | Invariant #1 on the encrypt side: the CEK is minted **inside** the boundary. States the original gap, the closing work, and the target contract. Still cited from `capsules/encrypt-provider` and `scripts/ddrm-drift-check.sh` as the reconcile-me list. |
| [DDRM_DECRYPT_RAIL.md](DDRM_DECRYPT_RAIL.md) | The decrypt-rail design decision: an ElastOS-native contract rather than Lit/Chipotle, with sealed material pushed *in* and no outbound key fetch (Option A). The reason `decrypt-provider` looks the way it does. |
| [PC2_PLAYER_ALIGNMENT.md](PC2_PLAYER_ALIGNMENT.md) | How the PC2 players and the runtime's viewer seam line up — `elacity-player`, the runtime media routes, and `decrypt-provider stream_segment`. |
| [DDRM_STATUS.md](DDRM_STATUS.md) | The rolling status / review package across sprints. Purely a snapshot log — the most day-stamped file here. |
| [DEV_MODE_GUARD_SPEC.md](DEV_MODE_GUARD_SPEC.md) | The spec that fenced the insecure dev modes out of production builds. Implemented — the `dev-modes` feature and `enforce_release_build_rights_safety` are its output. Still referenced from `docs/SECURITY_AUDIT.md` and `docs/THREAT_MODEL.md`. |

## Onboarding notes from the convergence period

| Document | What it is |
|---|---|
| [HANDOVER.md](HANDOVER.md) | The long-form handover written for a fresh context window: mission, state, reading order. Superseded as an entry point by [../README.md](../README.md); kept for the narrative of how the work unfolded. |
| [NEW_AGENT_BRIEF.md](NEW_AGENT_BRIEF.md) | The zero-blind-spots brief for a new agent — mission, repo geography (this repo *and* the PC2 reference repo), visual maps, working method. |

## Commerce — superseded planning

| Document | What it is |
|---|---|
| [COMMERCE_PLAN.md](COMMERCE_PLAN.md) | The original "marketplace in the runtime" council plan. Its scope was later corrected — the marketplace is buy/trade **only** — but the architecture, contract and UX reasoning stand. |
| [COMMERCE_UI_STRATEGY.md](COMMERCE_UI_STRATEGY.md) | The 2026-06-24 council decision on whether to use `elacity-web` (the production dApp) as the runtime marketplace UI, and why the answer went the way it did. |
| [COMMERCE_CONTRACTS_LEGACY_ABI.md](COMMERCE_CONTRACTS_LEGACY_ABI.md) | The legacy marketplace contract/ABI reference, preserved verbatim from a retired branch. The current, live-verified reference is [../COMMERCE_CONTRACTS.md](../COMMERCE_CONTRACTS.md). |

---

**Not kept.** Three convergence-era files were dropped in this consolidation rather than
relocated: `ELASTOS_ARCHITECTURE_VISUAL.md` (its diagrams are now
[../diagrams/](../diagrams/), rendered and referenced from
[../ARCHITECTURE.md](../ARCHITECTURE.md)), `PUSH_PLAN.md` and `V040_COORDINATION.md`
(both week-tactical coordination notes with nothing durable in them). The marketplace
`SCOPE.md`, `ROADMAP.md`, `README.md`, the `PHASE*` chunk plans and the `layouts.html`
mock were folded into [../COMMERCE.md](../COMMERCE.md).
