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
- ✅ **2b — intent channel LIVE** — `intent_proof_summary` exposed through the `AuditSource` trait (fail-honest `None`
  default), a top-level `intent_proof` field projected on the capsule detail (keyed `vm-{name}`), threaded into
  `homeCustodyView`'s 3rd arg. Absent/clean/flagged all paint honestly. (Receipt: `2b` commit; gated server 908/908.)
- ✅ **2c — standing-grant dispatch (the milestone) LANDED** — `StandingGrantStore` (fail-closed issue/revoke) +
  `dispatch_standing_act` routes a self-declared agent act through `run_intent_gate`. Proven end-to-end from a real
  `CapabilityToken`: derive → issue → dispatch (runs, matched) → **revoke → same dispatch denied, act never runs**
  (the autonomy kill switch). Gated `cargo test -p elastos-runtime --lib capability::intent` → 29/29. (Commits `fe9211f`, `19e3e9e`, + this.)
- ⬜ Then: confirm the inspector shows **live** runtime custody (DID / trust / manifest / required-vs-granted-vs-denied
  caps / audit chain), not sample-data — the last piece is Cursor's live-on-Mac confirm.

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
- ✅ **5b-inspector (= Tier 2b)** — intent channel LIVE end-to-end · ✅ **4b (= Tier 2c)** — standing-grant dispatch, kill switch proven.

## Current focus
**Tier 2 is COMPLETE (2a + 2b + 2c all landed & gated).** The remaining Tier-2 item is not code — it's **Cursor's
live-on-Mac confirm**: reload the shell → open the inspector → screenshot the three-channel Custody card showing LIVE
runtime custody (not sample data). After that, **open the flint→main PR** (step 1; nothing blocks it now).
> **2a** custody panel mounted (`fd1336a`) · **2b** intent channel LIVE end-to-end (`98d6eea`) · **2c** standing-grant
> dispatch + kill switch (`fe9211f`, `19e3e9e`, + this). The runtime enforcement loop is now closed: an agent can run
> unsupervised under a standing grant AND be halted by revoking its token (the gate re-reads the grant each dispatch,
> so revocation denies every not-yet-started act). Gated: elastos-runtime
> intent 29/29, elastos-server 908/908, ESP 89/89.
>
> **Standing-grant API (post-2c, in progress):** ✅ `StandingGrantService` (the store+audit+key seam, signed by the
> manager's own key) · ✅ shell-only `POST /api/standing-grants/issue` (mints a real token → derives the standing
> envelope) and `/revoke` (the kill switch), fail-closed, behind the same shell-only middleware as grant/deny · ✅
> shell-only `POST /api/standing-grants/preview` — a SIDE-EFFECT-FREE dry-run: authenticate a signed
> `IntentDeclarationV1` (`verify_self`) then report the envelope verdict, recording nothing and running no act ·
> ⬜ the side-effecting **dispatch/act route** (an agent actually running an act over HTTP — needs the
> affordance-invocation wiring decision) — held for a design call. Gated: server 911/911, runtime intent 34/34.
