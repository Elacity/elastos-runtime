# ElastOS / Flint — Master Roadmap & Status

The single living map of this effort. Updated at every wedge. Sits above `KNOWN_GAPS.md`
(detailed gap registry) and the agent memory (cross-session). Branch: `flint` (shared
infrastructure, never pushed). `flint-shell` is reserved for the unique Flint front-end.

**Mission:** ElastOS = the open, capability-secured world-computer runtime (shared infra,
works with any shell, the protocol + 5% fee is the moat). **Flint** = the unique front-end
experience (the killer app) that sits on top, later, on `flint-shell`.

**Status at a glance (2026-06-26):** substrate DONE; intent compiler DONE; security
audited + hardened to ~9/10 (AUD-1..5 closed); Astrid/UniCity research DONE (validation +
adopt-list); now executing the ADOPTION WEDGES before the shell. Wedges 1 (`elastos mcp serve`)
+ 2 (typed dataflow binding) — the council's two pre-shell priorities — DONE. Wedges
3-5 are decision-coupled (ai-backend track / design-fit) → founder's steer.

---

## ✅ DONE

**Phase 0 — Substrate (the agent five-beat loop, live in prod).** perceive (inspect /
typed affordances) → plan (invoke gate preview) → consent (approval::decide, fail-closed,
shell-mediated) → act (carrier dispatch, capability-gated, exact-action) → audit (ed25519
signed tamper-evident hash chain). Capability identity unified on `vm-{name}` (G-ID). dDRM
crown jewels merged. All loop gaps G1/G2/G3/G4/G8 closed → enforced invariants. Dual
WASM + MicroVM(crosvm) isolation. Tag `flint-substrate-v1`.

**Phase 1 — Intent compiler (shipped, shell-agnostic).** `compile` / `compile_sequence`
(ordered multi-step) / `discover` (find which capsule offers a goal across a set,
fail-closed Ambiguous) / `compile_sequence_discovered`. Full serde I/O contract (plans +
errors as JSON). System-scoped gateway `discover` op (carrier-locked to Admin).

**Phase 2 — Security audit + hardening (7/10 → ~9/10).** Six-specialist 0.01% audit
(`wu4y6lvzb`), every finding independently refuted. ZERO capsule-exploitable findings.
Closed: **AUD-1** author-signature launch gate (fail-closed-when-configured) + canonical
signing form; **AUD-2** gateway audit fails closed (no memory-log downgrade); **AUD-3**
revocation fail-closed; **AUD-4 plane-(b)** verify-on-read with anti-spoof DID pin;
**AUD-5** bare `scheme://*` super-wildcard refused at the grant chokepoints. Tags
`flint-audit-hardened-v1`, `flint-secured-v1`.

**Phase 3 — Competitive research (Astrid/UniCity, Joshua Bouw).** Verdict: independent
convergence = validation of our model. We lead on the shipped composition engine, dual
isolation, signed per-effect audit, the carrier, dDRM, tighter caps, the audit. They lead
on the MCP bridge, dataflow binding, COW workspace, shipped metering, ecosystem. License
cleared (dual MIT/Apache; rmcp Apache-2.0; clean-room). See `reference_unicity-astrid-comparison`.

---

## 🔭 CURRENT TRACK — adoption wedges (shared-infra on `flint`, BEFORE the shell)

Each = a clean-room PATTERN adoption (no code copied), built on what we shipped, judged
against runtime/carrier principles (no ambient authority, capability-gated, fail-closed,
carrier-for-off-box).

| # | Wedge | Status | Notes |
|---|-------|--------|-------|
| **1** | `elastos mcp serve` (MCP bridge, read surface) | **✅ DONE `6c524baf`** | Clean-room (open MCP spec, hand-rolled over serde_json, ZERO new deps). Read tools capsules/capsule/plan/intent; every tools/call routes through the carrier's `handle_request` VERBATIM (one canonical gate); bridge holds a scoped time-boxed `mcp-bridge` token; discover Admin-locked + excluded; effects deferred. 6 earned tests (gate runs, empty token denied). server 837 pass. |
| **2** | Typed dataflow binding (`compile_pipeline`) | **✅ DONE `11b017b5`** | Caller-declared port edges, compiler-validated against COMO typed schemas, fail-closed (out-of-range / backward / untyped / missing pointer-or-field / type-mismatch); affordance-only; shell-agnostic. Strict superset of compile_sequence. 9 earned tests; runtime 336. Honest: shipped outputs opaque (mechanism is the value). Deferred (named in-code): duplicate-target-port + required-input completeness + runtime value-passing. |
| **3** | NL→StructuredIntent provider capsule | **DECISION-COUPLED** | Needs an LLM provider capsule + the deferred ai-backend track ([[project_runtime-ai-backend-strategy]]). Not a clean pure-runtime wedge; founder's steer. |
| **4** | Dual-scope meter (reservation/refund + per-principal CPU/mem caps) | **DECISION-COUPLED** | = the deferred ai-provider meter; coupled to the ai-backend track (it meters ai-provider spend). Substantial; founder's steer. |
| **5** | COW workspace VFS + manifest hardening | **NEEDS DESIGN-FIT** | COW overlay is real; the manifest "typed imports" hardening may not map (we declare interfaces/authority, not Astrid-style imports). Map before building. |

**Then → Phase 4: the Flint shell** (on `flint-shell`) — the unique front-end experience.
Needs the founder's product direction (the first thin slice of the experience). Sits on a
runtime that is interoperable (MCP), can execute pipelines (dataflow), and is bounded (meter).

---

## 🗂️ DEFERRED / TRACKED (do not forget)

- **BUG-1..8** — confirmed VM-lifecycle leak/zombie cluster (reap trusts kill(pid,0);
  carrier-sock + JoinHandle leak; boot-failure orphan; bounded carrier read; lost-grant
  poll; next_cid overflow; carrier liveness; Once-refund). Need a crosvm/KVM test env →
  **good local/Cursor candidates.** Detail in `KNOWN_GAPS.md`.
- **Performance** (speed 5/10): reflink rootfs overlay (FREE win, VM-path, local/Cursor);
  audit group-commit (the runtime-wide throughput ceiling — MEASURE FIRST; security-
  sensitive: never coalesce a custody record / cache a revocation).
- **AUD-4 plane-(a)** — `verify_chain` on startup (needs `AuditLog::with_file` switch +
  a persisted head-anchor vs tail-truncation). Wave 2.
- **Carrier-service author gate** — AUD-1 residual (host-binary artifact model, separate design).
- **G-CIE** — ACCEPTED (the grant root is the trusted shell; the principled alternative is
  role-based capability tiering = a future initiative, not a band-aid).
- **AUD-1 ACTIVATION** — the gate ships inert; production trust roots are config the
  founder/Cursor sets (`trust_cmd` generate key → config `trusted_keys` → sign capsules).

## 💻 LOCAL / CURSOR (founder's device — VM env + operational)

1. Activate AUD-1: generate the author key via `trust_cmd`, add the pubkey to config
   `trusted_keys`, re-sign the shipped capsules (flips the gate to enforcing).
2. BUG-1..8 (the VM-lifecycle cluster — needs KVM/crosvm to validate; specced in `KNOWN_GAPS.md`).
3. The reflink rootfs quick-win (perf; platform-specific COW).
4. Run the full suite locally (the 9 sandbox env-fails — browser-engine + checksums —
   should be green on a real machine).
5. Optional: push `flint` for a remote backup (we never push from here by rule).

---

## ⚙️ OPERATING CADENCE (how the 0.01% team works)

- **Per wedge:** design swarm (map → design → adversarial verify) → implement the smallest
  honest slice → gate on `just test-crate <crate>` + `cargo fmt -- --check` → commit ONE
  clean slice on `flint` → update `KNOWN_GAPS.md` + this `ROADMAP.md` + memory.
- **Never push. Never another branch** (`flint-shell` is the reserved front-end line).
- **Adoptions are clean-room:** patterns not code; verify each dependency's license is permissive.
- **Security-touching wedges** (enforcement path, identity, audit, the gateway gate) get
  extra adversarial verification and a focused pass — never rushed.
- **Checkpoints (tags):** `flint-substrate-v1` → `flint-audited-v1` → `flint-audit-hardened-v1`
  → `flint-secured-v1` (and onward per milestone).
- **Honesty bar:** earned tests, no fabricated trust/durability, fail-closed, docs/code/tests
  agree; the swarm is empowered to return decision-needed → stop + surface, never force.
