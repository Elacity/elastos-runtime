# ElastOS / Flint — Master Status & Goal

The single consolidated view: the mission, the three *different* definitions of "done," what's
built vs mounted vs future, and the current focus. Cross-checked between the runtime work (this
branch) and the shell/macOS work (Cursor). Canonical backlog detail lives in `TASKS.md`; the
per-wedge history in `ROADMAP.md`; the prover/verifier loop in `INTENT_PROOF_LOOP.md`.

## Mission
**KEEP / the Sovereign Computer** — the consent layer + cryptographic flight recorder for AI
agents. It *enforces* what an agent may do (object-capability tokens + kernel egress firewall +
per-agent spend meter, all fail-closed) and *records* everything on a tamper-evident, ed25519-
signed audit chain that self-verifies. **Funded wedge:** enterprise EU AI Act (Art 12/14) agent-
containment audit.

## Three definitions of "done" — DO NOT conflate
| Goal | Needs | Distance |
|---|---|---|
| **A. See & merge THIS branch** (the security engine + ESP library) | Tier 2 | **Closest — days** |
| **B. Polished macOS consumer shell** | Tier 1 | Separate track; macOS-native is PARKED (PR #3) |
| **C. Ship the whole product** | Tier 3 | The full roadmap — months |
> The 3-tier list below is the **whole company's backlog**, not this branch's gap. This branch's
> gap is **Tier 2 only**, and even that is optional for a backend/security-only PR.

## Merge-readiness (Goal A)
- **`flint` = `main` (0.4.0) + 386 commits ahead, 0 behind — a CLEAN PR / fast-forward.** (An earlier
  "disjoint histories" scare was a *shallow-clone artifact*, since corrected.)
- Full-workspace `cargo test --workspace --lib` **green**; **no `todo!()`/`FIXME` stubs** in the crates.
- **No open PR yet → opening one is literally step 1** (best after the Tier 2 mount so there's something to click).
- Known **non-regressions** to allowlist for reviewers: 6 Mac `SUN_LEN` (PR #3), ~3 macOS-arch checksum, ENV-1 aarch64 clippy.
- Review bar (from PR #3): Linux CI green · hunk-level review of `carrier_bridge.rs`/`supervisor.rs`/`vm_provider.rs` · reviewer Anders.

## Tier 1 — the macOS shell (CORRECTED — functional today)
- ✅ **macOS shell WORKS today** — providers run as **native arm64 subprocesses** via `ELASTOS_<NAME>_BIN`;
  one command: `ELASTOS_DKMS_CARRIER=1 scripts/dev/run-creator-gateway.sh`. **No VZ, no Linux box.** (Receipt: `5eb3dee`.)
- 🟡 **Durable item shrinks to packaging:** publish **arm64 platform entries in the signed manifest** so it
  works without the dev override. Release hygiene, not a porting project.
- ⬜ **Apple VZ = isolation-hardening only** (on Linux these are microVMs; on macOS native subprocesses). **PARKED**
  (PR #3, Anders 2026-05-29) — do **not** un-park.
- 🟡 Managed-runtime / host-lock ergonomics (restart-on-fingerprint / one-host-per-data-dir) — smoother.

## Tier 2 — surface THIS branch's work into the shell (the real gap)
- ✅ **2a — Custody panel MOUNTED into `capsule-inspector`** (spend + audit paint LIVE; intent reads `absent`).
  Shared `custodyDisplayRows` contract; drift-guarded projection copy; gated 89/89 + headless render. (Receipt: `fd1336a`.)
- 🔵 **2b — 5b-inspector:** expose `intent_proof_summary` through the `AuditSource` trait + project an `intent_proof`
  field + ESP data path → the intent channel goes from `absent` to LIVE. **← NEXT**
- ⬜ **2c — 4b standing-grant dispatch (the milestone):** issue/revoke standing capability envelopes and route
  self-declared agent acts through `run_intent_gate` — "an agent runs unsupervised under the loop."
- ⬜ Then: confirm the inspector shows **live** runtime custody (DID / trust / manifest / required-vs-granted-vs-denied
  caps / audit chain), not sample-data.

## Tier 3 — product backlog (TASKS.md is canonical, §Now = strict priority)
- **Auth/identity:** proof-bound non-delegatable sessions, principal roots replacing `Users/self`, passkey recovery/
  reassignment UX, agent principals, WebAuthn RP policy (prod HTTPS vs local).
- **Wallet/blockchain:** WalletConnect connector capsule, recovery-kit semantics, keep chain/RPC authority out of ordinary capsules.
- **Home/System contract:** Library browsing, runtime health, capability prompts, "Apps vs capsules" naming, `setup --profile demo` = what Home advertises.
- **Browser provider (the big one):** native/isolated engine, Net/Exit contract, media/audio acceptance gate.
- **Release/install coherence, truth-surface anti-drift, protected-content/dDRM, operator/audit hardening.**

## Intent-proof loop ledger (`INTENT_PROOF_LOOP.md`)
- ✅ **ch1–5 + 5b-runtime** — signed records, fail-closed verifier matrix, on-chain emit, `from_token`, `run_intent_gate`,
  ESP `intentProofView`, presence-aware `AuditLog::intent_proof_summary`. All gated.
- ⬜ **5b-inspector (= Tier 2b)** · ⬜ **4b (= Tier 2c)**.

## Current focus
**Tier 2b** — take the intent channel from `absent` to LIVE: expose `AuditLog::intent_proof_summary` through the
`AuditSource` trait, project an `intent_proof` field on the inspector detail, and thread it into the ESP
`homeCustodyView`. The panel + display contract already render it (2a); this is the runtime data path behind it.
> **2a done (`fd1336a`):** custody panel mounted in `capsule-inspector` — spend + audit paint LIVE from the runtime
> projection, intent reads `absent`; both shells share the tested `custodyDisplayRows` contract (drift-guarded copy).
> **Awaiting Cursor's live confirm on the Mac** (reload shell → open inspector → screenshot the Custody card).
