# ElastOS / Flint — Master Roadmap & Status

The single living map of this effort. Updated at every wedge. Sits above `KNOWN_GAPS.md` (detailed
gap registry), the strategy docs (below), and the agent memory (cross-session). **Canonical branch:
`flint`** (= `feat/capsule-inspector` 100% + `ddrm` merged + all our docs/wedges/hardening; pushed to
`origin/flint`). Build everything here.

**Mission:** ElastOS = the **Sovereign Computer** — *the consent layer and cryptographic flight
recorder for AI agents* (NOT "an AI OS"). The product is **KEEP** (the glass delegation desk): you and
your agents become one accountable actor, same gate, same signed receipts (identity collapse). Every
UI pixel is a read-only projection of real crypto — the un-fakeable moat. *The future is not
artificial intelligence, it is artificial integrity.* The funded wedge is the enterprise
agent-containment audit (EU AI Act Art 12/14).

**Status at a glance (2026-06-27):** substrate + intent compiler DONE; security audited + hardened
~9/10 (AUD-1..5); Astrid research DONE; adoption wedges 1-2 (MCP-serve + dataflow) DONE; the PRODUCT +
STRATEGY resolved (KEEP, the PDR, the shell vision, the ESP protocol, the narrative); now executing the
**ESP build wedges W0-W7** toward the shell — **W2 (consent act path) IN PROGRESS (~40%)**.

---

## 🧭 THE THREAD (the three layers, so nothing is lost)
- **WHY — the product + moat:** KEEP / the Sovereign Computer. Docs: `FLINT_KEEP_CONCEPT.md`,
  `PDR_SOVEREIGN_COMPUTER.md`, `FLINT_SHELL_VISION.md`, `NARRATIVE.md`.
- **WHAT — the build plan:** the **ESP build wedges W0-W7** on `flint` (the runtime substrate the
  shell projects over). Doc: `ESP_SHELL_PROTOCOL.md`. W2 detail: `W2_CONSENT_PLAN.md`.
- **HOW — the method:** the orchestrator + loop (design swarm → gated chunks → close-the-loop →
  retro), per the `elastos-runtime` `CLAUDE.md` contract. Self-optimizing via
  `~/Desktop/Claude-Orchestrator/LESSONS.md` (4 lessons banked this campaign).

---

## ✅ DONE

**Phase 0 — Substrate (the agent five-beat loop, live in prod).** perceive (inspect / typed
affordances) → plan (gate preview) → consent (approval, fail-closed) → act (carrier dispatch,
capability-gated) → audit (ed25519 signed tamper-evident chain). Capability identity unified on
`vm-{name}` (G-ID). dDRM merged. Loop gaps G1/G2/G3/G4/G8 closed. Dual WASM + MicroVM isolation.
Tag `flint-substrate-v1`.

**Phase 1 — Intent compiler (shipped).** `compile` / `compile_sequence` / `discover` /
`compile_sequence_discovered` + `compile_pipeline`. Full serde I/O contract. System-scoped `discover`.

**Phase 2 — Security audit + hardening (7/10 → ~9/10).** Six-specialist audit (`wu4y6lvzb`); zero
capsule-exploitable findings. Closed AUD-1 (author-signature launch gate + canonical form), AUD-2
(gateway audit fail-closed), AUD-3 (revocation fail-closed), AUD-4 plane-(b) (verify-on-read + DID
pin), AUD-5 (no bare `scheme://*`). Tags `flint-audit-hardened-v1`, `flint-secured-v1`.

**Phase 3 — Competitive research (Astrid/UniCity).** Independent convergence = validation. License
cleared (dual MIT/Apache; rmcp Apache-2.0; clean-room). See `reference_unicity-astrid-comparison`.

**Phase 4 — Adoption wedges 1-2 (the two pre-shell priorities, clean-room from Astrid PATTERNS):**
- **Adoption #1 — `elastos mcp serve` ✅ (`6c524baf`).** Clean-room MCP bridge (open spec hand-rolled,
  zero new deps); read tools through the carrier's one canonical gate; scoped time-boxed token;
  discover Admin-locked; effects deferred. 6 tests; server 837 pass.
- **Adoption #2 — typed dataflow binding ✅ (`11b017b5`).** `compile_pipeline` — caller-declared port
  edges, compiler-validated, fail-closed; the composition engine that is our lead. 9 tests; runtime 336.
- (Adoption #3 NL→intent + #4 meter → the deferred AI-backend track; #5 COW/manifest → later. See DEFERRED.)

**Phase 5 — Product + strategy RESOLVED (council swarms `w6hzmym8l` / `w878qm5jv` / `wgqlt5u74`).**
- **KEEP** (`FLINT_KEEP_CONCEPT.md`) — the product: the two-channel object (real vs reach), the dual
  receipt (platform signs its own accountability), the refraction toggle, NO score.
- **The PDR** (`PDR_SOVEREIGN_COMPUTER.md`) — the definitive spec, audited against the live tree:
  consent-layer + flight-recorder positioning; Rust/WASM YES, Godot NO→web; local-AI realistic; the
  enterprise wedge; the honest now/next/later phasing.
- **The shell vision** (`FLINT_SHELL_VISION.md`) — the glass delegation desk; 3 objects + 2 properties.
- **The ESP protocol + wedge plan** (`ESP_SHELL_PROTOCOL.md`) — runtime-first; shells = untrusted
  clients over a 2-plane bridge, gate-in-core; the W0-W7 plan; the marketplace-of-shells future.
- **The narrative** (`NARRATIVE.md`).

---

## 🔭 CURRENT TRACK — the ESP build wedges (on `flint`)

The PDR/ESP plan: build the honest substrate first, then the shell as a read-only projection over ESP.

| Wedge | What it is | Status |
|---|---|---|
| **W0** | Core-derived reach (the honest halo) | not started |
| **W1** | Egress-as-capability | not started |
| **W2** | **Unstub the consent act path** | **IN PROGRESS ~40%** (below) |
| **W3** | De-hardcode "the shell" → "a shell" + rename `consent-broker` | not started |
| **W4** | Write ESP v0 (protocol doc + TS types) | not started |
| **W5** | The v1 Svelte projection shell + the hero dDRM act | not started |
| **W6** | The refraction toggle + shell-picker | not started |
| **W7** | Export the receipt chain as the EU AI Act audit artifact (the flywheel) | not started |

W0/W1 make the halo *truthful*; W2 makes consent *real*; W3/W4 open the *modular* shell; W5/W6 build
the *first* shell; W7 *turns the flywheel once*. (W0/W1 are the PDR's "the halo is a lie + no egress
control" fixes — the precondition for the honest-manifestation moat.)

**W2 sub-status (`W2_CONSENT_PLAN.md`):** steps 3 (binding fields), 4a (request-half receiver), 4b
(gateway consent path: shared canonical hash + descriptor + `InvocationGate` + the token-incapable 202
POST seam) DONE + gated green; parked on `origin/w2-consent-source`. REMAINING: **re-integrate onto
flint's hardened code** (W2 was built on the older `ddrm` base — collides with G-ID /
`create_request_with_capsule`), then step 5 (SSE — already surfaces by construction), **6 (the grant
READS the binding — the enforcement crux)**, 7 (validate-and-consume endpoint), 8 (`ValidatedAffordanceGrant`
witness = compiler-enforced dispatch gate), 9 (signed BLOCKING audit + receipt), 10 (journey test),
11 (alignment + docs). Steps 6-8 = the security crux (binding actually enforced).

**Immediate next (in the cloud, on `flint`):** re-integrate W2 → step 6 → finish W2 → W3 → … → W7.

---

## 🌿 BRANCH STRUCTURE (resolved 2026-06-27 — was tangled across two clones)
- **`flint`** — CANONICAL. capsule-inspector (100%) + ddrm merged + all 89 commits of our work
  (docs/wedges/hardening + the W2 plan). On `origin/flint`. **Build everything here.**
- **`feat/ddrm-hardening-and-creator-parity`** — the founder's + Cursor's dDRM + creator-parity line;
  restored to `97bcd3689` (creator WIP uncommitted local). Separate; do not put our work here.
- **`origin/w2-consent-source`** (`369497502`) — the 3 verified W2 commits, parked for re-integration.
- Lesson banked: confirm the canonical line BEFORE building — an audited "live tree" can be a stale base.

---

## 🗂️ DEFERRED / TRACKED (do not forget)
- **Adoption wedges 3-5 (original Astrid leg):** #3 NL→intent capsule + #4 dual-scope meter → the
  deferred **AI-backend track** ([[project_runtime-ai-backend-strategy]] — Venice/Bittensor, the
  agent's brain; the meter bounds AI spend). #5 COW workspace + manifest hardening → later (design-fit).
- **BUG-1..8** — VM-lifecycle leak/zombie cluster (needs crosvm/KVM → local/Cursor). `KNOWN_GAPS.md`.
- **Performance** (speed 5/10): reflink rootfs overlay (free win, local/Cursor); audit group-commit
  (measure first; never coalesce a custody record / cache a revocation).
- **AUD-4 plane-(a)** — `verify_chain` on startup (Wave 2). **Carrier-service author gate** — AUD-1
  residual. **G-CIE** — ACCEPTED. **AUD-1 ACTIVATION** — production trust roots = config (founder/Cursor).
- **PDR NEXT-band (beyond W0-W7 v1):** the dual-receipt PLATFORM self-attestation (co-sign at gate
  time; legal admissibility opinion first); act-over-MCP (the write path); the meter; free-text NL.

## 💻 LOCAL / CURSOR (founder's device — VM env + operational)
1. Activate AUD-1 (generate author key via `trust_cmd` → config `trusted_keys` → re-sign capsules).
2. BUG-1..8 (the VM-lifecycle cluster — needs KVM/crosvm; specced in `KNOWN_GAPS.md`).
3. The reflink rootfs quick-win (perf; platform-specific COW).
4. Run the full suite locally / in the cloud (the 9 sandbox env-fails — browser-engine + checksums —
   pass on a real Linux machine).
5. Continue the creator-parity line independently on `feat/ddrm-hardening-and-creator-parity`.

---

## ⚙️ OPERATING CADENCE (how the 0.01% team works)
- **Per wedge/chunk:** design swarm (map → design → adversarial verify) → implement the smallest
  honest slice → gate on the real `just` checks (`check` / `test-crate` / `fmt` / `lint` / `verify` /
  `alignment-check`) → close the loop (what changed · gates · open · next step) → update this
  `ROADMAP.md` + `KNOWN_GAPS.md` + memory → retro (bank any lesson).
- **Push discipline:** push `flint` (the canonical line) for cloud access + backup; NEVER commit/push
  to the founder's `ddrm`/shared branches without explicit ask; force-pushes need explicit OK.
- **Adoptions are clean-room:** patterns not code; verify each dependency's license is permissive.
- **Security-touching wedges** (enforcement path, identity, audit, the gate) get extra adversarial
  verification and a focused pass — never rushed.
- **Honesty bar:** earned tests, no fabricated trust/durability, fail-closed, docs/code/tests agree;
  the type system as proof; the swarm may return decision-needed → stop + surface, never force.
- **Checkpoints (tags):** `flint-substrate-v1` → `flint-audited-v1` → `flint-audit-hardened-v1` →
  `flint-secured-v1` → `flint-preshell-v1` (and onward per milestone).
