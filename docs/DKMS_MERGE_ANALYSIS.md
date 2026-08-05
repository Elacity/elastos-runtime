# dKMS Foundation × ESP — Merge Analysis & Strategy

**Date:** 2026-08-05
**Branches:** `main` @ `d358dedb` ⇐ `review/dkms-foundation` @ `94b0f63a` (merge-base `847086be`, 2026-07-02)
**State analyzed:** in-progress merge on `temp/0.7-merge`, 49 unmerged paths
**Method:** three parallel deep-dives — Rust core architecture, UI/capsule layer, system topology (with strategy experiments in a throwaway clone; the live repo was never mutated).

---

## Executive summary

**Verdict: do NOT start over. The dKMS foundation survives ESP.**

ESP invalidated exactly **one** substrate assumption dkms was built on — WASI-preview1 capsule
execution (`WasmProvider` was deleted; main rejects WASI capsule materialization). dKMS's rails
happen not to stand on it: its provider capsules (`content-market`, `act-emitter`) are
**microvm**-type, its boot providers (`encrypt`/`publish`/`media`) are in-process, and every
gateway auth helper its ~15k additive server lines import (`require_home_token_context`,
`issue_home_launch_token_with_context`, `spawn_carrier_bridge`) **still exists on main**.
The alarming −39k lines on main are concentrated in browser tests, `wasm.rs`, and the
`inspect_provider`/`auth_gateway` restructures — none load-bearing for dkms.

- **~80% of dkms ports cleanly** (all-new crates/modules, clean cherry-picks proven empirically).
- **4 genuine redesign hotspots** (days of careful auth-sensitive work, not a rewrite):
  `runtime.rs` wiring, `carrier_bridge.rs` dispatch spine, `gateway_home_token.rs`
  passkey-freshness unification, `gateway_capsule_catalog.rs` typed bindings.
- **Recommended mechanics: abort the current merge and rebase `review/dkms-foundation` onto
  `main`** — it converts one 49-file wall into 4 stops totaling ~25 conflict instances, with all
  hard work isolated in a single commit resolved with full per-commit context.
  *Exception:* if substantial conflict resolution has already been invested in the current
  `temp/0.7-merge` index, finishing the merge is acceptable — audit what's already staged before
  aborting.

---

## 1. The two branches, architecturally

### main since base — "ESP" (58 commits, 612 files, +145,725/−39,121)

A *shell-protocol + execution-substrate* rewrite:

- **ESP v0 protocol** (`docs/ESP_V0.md`, `elastos/esp/*.ts`, `elastos-server/src/esp_binding.rs`,
  `api/gateway_esp.rs`): shells are projection/consent surfaces only; the Runtime owns all
  authority; typed fact/verb schemas.
- **Execution substrate replaced:** `elastos-compute/src/providers/wasm.rs` (WASI-preview1
  `WasmProvider`, 1,226 lines) deleted, replaced by `providers/component.rs`
  (WASM component model + ElastOS Bus, `elastos/wit/elastos-bus-v1.wit`). Commit `75597bf7`
  makes the runtime reject source-WASI capsule materialization outright.
- **Auth/token surface rewritten:** `auth.rs` +4,935 (signed anchored audit chain),
  `gateway_home_token.rs` +1,238 (projection launch tokens, `RuntimeWalletAuthority`, DID-bound
  tokens), new `gateway_passkey_step_up.rs` (2,335 lines), new crate `elastos-wallet-contract`.
- **Capsule catalog restructured** into typed `RuntimeCapsuleAffordanceBinding` enum dispatch
  (module split `gateway_capsule_catalog/{bindings,read_model,contract_audit}.rs`).
- **Shell restructured:** desktop GUI moved `capsules/home` → new `capsules/home-gui`;
  `home/browser/shell.js` deleted (logic → `home-shell-host.js`); `capsules/system` rewritten;
  Rust shims stripped from home/system/wallet-metamask (capsules became pure web-projections);
  WASI app shims removed (`chat-wasm` capsule deleted, `chat-room.wasm` deleted).

### review/dkms-foundation since base — the rails (12 commits, 516 files, +212,721/−1,966)

A *vertical product rail* on the pre-ESP substrate — **2/3 pure addition** (345 added files, 0 deletions):

| Commit | Content | Touches core? |
|---|---|---|
| `02c2dbc4` | CENC cipher core + PQ-hybrid threshold envelope + media packaging (`cenc-core`, `ddrm-envelope`, `ddrm-media`) | No — pure add |
| `43ff0533` | **de-Flinted runtime + server foundation** (114 files, +35k, 84 modified in `elastos/crates`) | **YES — ~all the entanglement lives here** |
| `b3787429` | dKMS key custody + on-chain rights rail | No (capsules only) |
| `7a9e6a8c` | dDRM content-protection rail (encrypt/decrypt providers, viewers) | No |
| `37380d52` | Marketplace publish→index→buy→acquire rail | No — pure add |
| `6303906c` | Substrate glue: capsule shell/manifest wiring (50 modified) | Collides with main's shell rewrite |
| `b800dbdc`–`caea2886` | Docs, harness, CI, dead-code removal (small) | Trivial |
| `94b0f63a` | Viewer session lifecycle (sweeper wired into `gateway_server.rs`) | Yes, small |

New elastos-server modules (all dkms-only, ~15k lines): `api/{market_reads, media_authority,
mint_authority, object_authority, rights_authority, trade_authority, buy_authority, viewer_open,
viewer_media, viewer_object, wallet_signer, session_lifecycle, session_bounds, owned_ledger,
creator, gateway_marketplace, access_grant, chain_tx, content_index, capsule_watchdog}.rs`, plus
`egress_audit.rs`, `carrier_service.rs`, `net_validation.rs`, `notifications.rs`. Also additive:
`elastos-runtime` `capability/receipt.rs` + conformance/fuzz tests, `elastos-common`
`{reach,canonical_hash,localhost}.rs`, `elastos-crosvm` `egress_{audit,firewall}.rs`.

None of the new dkms capsule crates are members of the `elastos/Cargo.toml` workspace — they
carry their own lockfiles and don't collide with main's workspace restructuring at all.

---

## 2. Convergence points (both sides built the same thing)

These are *agreements of intent*, resolved by adopting main's mechanism and folding dkms deltas in:

1. **Operation→action capability enforcement** — main's
   `provider_operation_action(scheme, op)` ≡ dkms's `required_action_for(op)` (dkms "PRE-AUDIT #3").
   Adopt main's (more general); fold in dkms-only schemes
   (`encrypt/publish/media/market/key/decrypt/drm/rights/chain`).
2. **Passkey freshness for sensitive verbs** — main's `gateway_passkey_step_up.rs` (2,335 lines,
   wired to recovery/wallet) vs dkms's `require_fresh_passkey_home_token`. **Pick main's
   mechanism**; re-bind dkms's money-verb call sites to it. Keep dkms's app-scoped SameSite
   cookie delivery (orthogonal, genuinely better than URL-borne `?home_token=`).
3. **Domain-separated signature verification** — main `verify_domain_separated_signature`
   vs dkms `domain_separated_verify`. Trivial; keep both or unify.
4. **Capability request plumbing** — main added `create_request_with_reason`; dkms added
   `create_request_with_capsule`/`create_affordance_request` via shared `create_request_inner`.
   Compatible: union by adding `reason` to `create_request_inner`.
5. **Reserved sub-provider names** (`provider/registry.rs`) — both extended
   `RESERVED_SUB_NAMES`. Union-merge; **keep dkms's `PINNED_SUB_NAMES` first-writer-wins** —
   a real security property for the escrow/key spine.
6. **Startup hooks** (`gateway_server.rs`) — main's browser lifecycle reconciler + dkms's
   `session_lifecycle::spawn_sweeper()` are distinct concerns. Keep both. (No true session
   duplication: main's sessions are auth-cookie sessions; dkms's are viewer decrypt sessions.)
7. **Manifest `interfaces`** — both invented one. Main's ESP-era convention
   (`runtime_abi`, `execution`, `projections`, `resource`/`operation` methods) is what the
   runtime now validates: **take main everywhere**; propose dkms's typed
   `input_schema`/`output_schema` upstream as an addition if wanted.

---

## 3. Divergence points (the real cost)

### Rust core — 4 redesign hotspots

1. **`runtime.rs` + `elastos-compute/src/providers/wasm.rs`** — dkms's execution-substrate work
   is dead: main has no `WasmProvider` and rejects WASI capsules. Drop dkms's `wasm.rs` additions
   (epoch-interruption `stop()`, watchdog); re-host the useful hostcall body
   (manifest-capability lookup) inside `ComponentProvider`'s hostcall closure. Microvm capsules
   unaffected.
2. **`carrier_bridge.rs`** — two divergent rewrites of the authorization/dispatch spine
   (main `authorize_and_dispatch_carrier_invoke` + component path; dkms
   `carrier_invoke_dispatch` + dDRM/market/wallet/chain arms + a WASM-pipe bridge main deleted).
   Biggest hand-merge: re-seat dkms's dispatch arms inside main's structure; drop the pipe bridge.
3. **`gateway_home_token.rs`** (+ `gateway.rs`, `gateway_home_runtime.rs`) — token issuance
   and launch-route construction diverged (main: projection tokens + `executable_actor`; dkms:
   `delivery` + secure cookie). Auth-critical, needs deliberate unification per §2.2.
4. **`gateway_capsule_catalog.rs`** — dkms's edits thread through string-tuple dispatch main
   replaced with a typed enum + module split. Small in lines; must be re-expressed, not merged.

### Delete/modify conflicts — all resolve toward main's deletions

`chat-room/chat-room.wasm`, `chat-wasm/capsule.json`, `home/browser/shell.js`,
`compute/providers/wasm.rs`: main deliberately deleted these paths
(`b2fea63d` WASI-shim removal, `1ac5512e` shell split). **Never resurrect them** — re-express
dkms's edits in main's replacement structure.

### UI layer — main rewrote, dkms hooked

Main's shell delta on `home`/`home-gui`/`system` is ~11,400 insertions / 5,475 deletions with
renames; dkms's is 664 insertions of which ~450 are cache-buster/copy noise. The real dkms UI
value is four portable pieces:

1. **Owned-asset open flow** in `shell-windows.js` (+474): guarded `openTarget` branch for
   `elacity-player`/`ddrm-viewer` — in-flight dedupe, staged loading window,
   `POST /api/viewers/open`, buy-then-retry via `POST /api/market/buy`, session release on window
   close. Ports mechanically: main's rewritten `home-gui/browser/shell-windows.js` kept the exact
   primitives it needs (`openTarget`, `createWindow`, `browserWindowSpec`, `focusWindow`).
2. **Loading-window CSS** (+143, self-contained) → paste into `home-gui/browser/style.css`.
3. **Two allowlist entries** (`ddrm-viewer`, `elacity-player`) → `home-shell-host.js`
   (`SHELL_MESSAGE_OPEN_TARGET_SOURCES.library`, the map moved there from deleted `shell.js`).
4. **wallet-metamask robustness** (+~100): 5s approval auto-poll (Creator mint UX), EIP-6963
   re-discovery, interaction-busy guard → re-implement on main's rewritten wallet-metamask.

Non-conflicting and clean: dkms's Library wiring (`library/browser/src/*`, +370 — "open with
viewer" menu) merges as-is.

### New dkms UI capsules — portable as-is

`creator/`, `elacity-player/`, `ddrm-viewer/`, `content-market/` are fully self-contained (own
dir + manifest, no shared shell imports, gateway-HTTP-only, fail closed without a session).
They drop onto main unchanged **iff** the dkms server routes land (`/api/viewers/open`,
per-viewer media/object session routes, `/api/market/buy`, `/api/apps/creator/*`). Each needs a
manifest-only update to main's web-projection schema.

---

## 4. Merge topology (measured)

**The 49 unmerged paths:** 44 UU (content), 4 DU (delete/modify), 1 AA (`docs/ESP_V0.md`).

| Area | Count | Real logic? |
|---|---|---|
| `elastos-server/src` (incl. `api/`) | 15 | **Yes — the hard core** |
| `elastos-runtime` / `elastos-compute` | 3 | Yes |
| Capsule Rust (`wallet-provider`) | 1 | Small |
| Capsule JS/HTML shell glue | 13 | Medium (main restructured; dkms wired old shell) |
| `capsule.json` manifests | 7 | Trivial — take main |
| Binary (`chat-room.wasm`) | 1 | None — take main's deletion |
| Docs | 5 | None |
| Scripts | 4 | Low |

**Overlap matrix:** 106 files changed by both sides (17% of main's 612, 21% of dkms's 516);
git auto-merged 57 of them — only 49 conflict. Concentration: `elastos-server` (32),
docs (6), `capsules/home/browser` (5), scripts (4).

**Strategy experiments** (throwaway clone):

| Strategy | Measured result |
|---|---|
| **A. Finish current merge** | 49 conflicted files in one sitting; resolution decisions hidden in one +212k merge commit |
| **B. Rebase `--onto main 847086be`** | **4 stops / ~25 conflict-file instances**: `43ff0533` → 17 (exactly the Rust core set), `b800dbdc` → 4 (docs), `e599c4db` → 2, `94b0f63a` → 2. **8 of 12 commits replay clean.** |
| **C. Cherry-pick rails onto fresh branch** | `02c2dbc4`, `b3787429`, `7a9e6a8c`, `37380d52` all apply **clean (0 conflicts)**; but `43ff0533` must then be hand-ported without 3-way help, `6303906c` → 22 conflicts, `94b0f63a` → 11 |
| `-X theirs`/`-X ours` | Both collapse to 4 conflicts (the DU cases) — proves the 44 UU are line-level resolvable, but would silently pick wrong sides. Measurement only; rejected. |

---

## 5. Integrity findings

- **Cargo workspace: safe.** Main is v0.6.0 + `crates/elastos-wallet-contract` member; dkms only
  added `[profile]` tuning. Auto-merges. dKMS capsule crates are out-of-workspace.
- **CI:** dkms adds `review/**` gating, `just verify-ci`, a dDRM Capsule Gate
  (`just verify-capsules`), and a **dKMS Release Invariant** job (release build of
  `dkms-authority` must succeed with defaults and *fail closed* with `legacy-receipt-authz`).
  Additive, but the `just` targets must be validated against main's justfile before landing.
- **Binaries on the dkms branch:** `chat-room.wasm` must die (main deleted its shim); DejaVu
  fonts + doc PNGs are legit assets.
- **Post-merge verification to schedule:** dkms's `market`/`object` provider routing and the
  reserved/pinned sub-name invariant re-verified against main's component-provider registration
  path (`supervisor.rs` is now the only `spawn_carrier_bridge` caller on main).

---

## 6. Recommended strategy

**Primary: Strategy B — abort the in-progress merge, rebase `review/dkms-foundation` onto `main`.**

Why: one 49-file wall becomes 4 sittings (~25 conflict instances); all hard work isolates into
`43ff0533` (17 Rust files) resolved with full per-commit intent; 8 of 12 commits replay clean;
history stays reviewable; the ESP-restructured shell/manifest glue is re-expressed
commit-by-commit rather than untangled inside merge soup.

**Before aborting:** audit the current `temp/0.7-merge` index. Many files are already staged —
if substantial resolution work is already done there, sunk cost may favor finishing the merge
(Strategy A) instead. Both A and B are viable; C (fresh branch + cherry-pick + hand-port) costs
≥ B and is only preferable if the goal is to audit-away Flint remnants during the port.

**Fixed decisions regardless of strategy:**

1. All 4 DU conflicts resolve to **main's deletions**; re-express dkms edits in main's structure.
2. **Take main's shell wholesale**; re-apply the 4 dkms UI pieces (§3) by hand — est. ~1 focused
   day incl. re-testing open/close/session-release against main's window lifecycle.
3. **Take main's manifest schema** everywhere; update the 4 new dkms capsule manifests to it.
4. Unify passkey freshness on main's `gateway_passkey_step_up`; keep dkms's SameSite cookie
   delivery; keep dkms's `PINNED_SUB_NAMES` pinning; keep both startup hooks.
5. Drop all dkms `WasmProvider` work; port the hostcall capability lookup to `ComponentProvider`.
6. Validate dkms CI `just` targets against main's justfile before landing.

**Effort estimate:** ~80% ports cleanly or auto-merges; the 4 Rust hotspots are days of careful
auth-sensitive work; UI re-application ~1 day; manifests/docs trivial. Re-implementation from
scratch was rejected: it would rebuild the identical additive modules (~35k lines) for no gain.
