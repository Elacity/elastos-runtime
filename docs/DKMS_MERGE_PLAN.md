# dKMS Foundation → ESP Main: Merge & Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the dKMS/dDRM/marketplace foundation (currently on `review/dkms-foundation`, built pre-ESP) onto ESP-era `main`, fully working, with unit + integration tests green as the acceptance anchor.

**Architecture:** Abort the stalled 49-conflict merge (verified: zero manual resolution invested — every conflicted file still carries raw markers). Rebase `review/dkms-foundation` onto `main` (`--onto main 847086be`), which isolates ~all hard resolution into one commit (`43ff0533`, 17 Rust files) and replays 8 of 12 commits clean. Then run three hardening phases — Rust integration, UI port, CI/test anchoring — each gated by tests before `main` sees anything.

**Tech Stack:** Rust (cargo workspace `elastos/` v0.6.0 + out-of-workspace capsule crates), `just` task runner, vanilla-JS browser capsules, GitHub Actions CI.

**Companion document:** `docs/DKMS_MERGE_ANALYSIS.md` (branch analysis; per-file conflict classification referenced throughout as "the analysis").

## Global Constraints

- **Never resurrect files main deleted:** `elastos-compute/src/providers/wasm.rs`, `capsules/home/browser/shell.js`, `capsules/chat-wasm/**`, `capsules/chat-room/chat-room.wasm`. All 4 DU conflicts resolve to main's deletion; dkms edits are re-expressed in main's replacement structure.
- **Main's conventions win wherever both sides built the same thing:** manifest schema (`runtime_abi`/`execution`/`projections`/`interfaces` with `resource`+`operation`), `provider_operation_action()` for op→action, `gateway_passkey_step_up` for freshness, typed `RuntimeCapsuleAffordanceBinding` catalog dispatch.
- **dKMS security properties are non-negotiable keeps:** `PINNED_SUB_NAMES` first-writer-wins pinning, G-ID (`session.vm_id`) fail-closed caller identity, viewer sessions fail closed, SameSite cookie token delivery for money surfaces, dKMS Release Invariant (release build of `dkms-authority` succeeds with default features, **fails** with `legacy-receipt-authz`).
- **Test gate before every `git rebase --continue` and every commit:** minimum `just check` (touched crates) + targeted tests; full `just test` at phase boundaries.
- **Commit style:** repo conventional format (`feat(scope): …`, `fix:`, `docs:` — see `git log`). No AI co-author lines.
- **Branch:** all work on `feat/dkms-esp-port`. `review/dkms-foundation` and `backup/dkms-foundation` are never rewritten. Landing target: `upstream/0.7-dev` (currently == `main` == `temp/0.7-merge` @ `d358dedb`), then PR → `main`.
- **Working directory:** repo root `/Users/maciz/www/ela.city/elastos-runtime` unless stated.

---

## Phase 0 — Preflight, abort, baseline

### Task 0.1: Snapshot, abort the stalled merge, commit the analysis docs

**Files:**
- Create (already on disk, untracked): `docs/DKMS_MERGE_ANALYSIS.md`, `docs/DKMS_MERGE_PLAN.md`
- No tracked files modified.

- [ ] **Step 1: Snapshot the conflicted state for forensics (cheap insurance)**

```bash
git diff > /private/tmp/claude-501/-Users-maciz-www-ela-city-elastos-runtime/40ee393b-5881-4dee-a845-3570616d065e/scratchpad/aborted-merge-conflicts.diff
git status --porcelain > /private/tmp/claude-501/-Users-maciz-www-ela-city-elastos-runtime/40ee393b-5881-4dee-a845-3570616d065e/scratchpad/aborted-merge-status.txt
```

- [ ] **Step 2: Abort the merge**

```bash
git merge --abort
git status
```
Expected: `On branch temp/0.7-merge`, working tree clean except the two untracked `docs/DKMS_MERGE_*.md` files (untracked files survive the abort). HEAD remains `d358dedb`.

- [ ] **Step 3: Commit the analysis + plan to `upstream/0.7-dev` (the work anchor)**

```bash
git switch upstream/0.7-dev
git add docs/DKMS_MERGE_ANALYSIS.md docs/DKMS_MERGE_PLAN.md
git commit -m "docs: dKMS x ESP merge analysis and implementation plan"
```

### Task 0.2: Establish the green baseline on main

A baseline proves later failures are ours, not inherited.

- [ ] **Step 1: Build + test main's workspace**

```bash
git switch main
just build && just test
```
Expected: PASS. If anything fails here, STOP — record it in the plan doc as a pre-existing failure and exclude it from later gates.

- [ ] **Step 2: Record baseline**

```bash
just test 2>&1 | tail -5 > /private/tmp/claude-501/-Users-maciz-www-ela-city-elastos-runtime/40ee393b-5881-4dee-a845-3570616d065e/scratchpad/baseline-main.txt
```

---

## Phase 1 — The rebase spine

### Task 1.1: Start the rebase

**Files:** none modified yet (git plumbing only).

- [ ] **Step 1: Create the work branch at the dkms tip**

```bash
git switch -c feat/dkms-esp-port review/dkms-foundation
```

- [ ] **Step 2: Rebase onto main, cutting off the pre-ESP base**

```bash
git rebase --onto main 847086be
```
Expected: `02c2dbc4` (CENC core) applies clean; first stop at **`43ff0533`** with ~17 conflicted files. Conflict-stop counts below are from a clone experiment and approximate — resolve whatever actually stops by the per-file recipes; classify any unexpected file against the analysis §2/§3 tables.

### Task 1.2: Resolve Stop A (`43ff0533` — the de-Flinted foundation, 17 Rust files)

This is the only hard stop. Resolve file groups in the order below; the goal at this stop is **"main's architecture + dkms's additions seated minimally, compiling, existing tests pass"** — deliberate unification work is deferred to Phase 2 tasks.

**Files (all Modify, conflict resolution):**
- `elastos/crates/elastos-compute/src/providers/wasm.rs` (DU)
- `elastos/crates/elastos-server/src/{runtime.rs, carrier_bridge.rs, provider_resource.rs, lib.rs, crypto.rs, auth.rs, server_infra.rs}`
- `elastos/crates/elastos-server/src/api/{gateway.rs, gateway_server.rs, gateway_home_token.rs, gateway_home_runtime.rs, gateway_capsule_catalog.rs, auth_gateway.rs, gateway_browser_stream.rs, handlers/provider.rs}`
- `elastos/crates/elastos-runtime/src/{capability/pending.rs, provider/registry.rs}`

**Interfaces (what Phase 2+ relies on from this resolution):**
- `provider_resource::provider_operation_action(scheme, op) -> Option<Action>` is the single op→action authority, covering dkms schemes `encrypt/publish/media/market/key/decrypt/drm/rights/chain`.
- `registry::PINNED_SUB_NAMES` + `is_reserved_sub_name()` exist alongside main's `RESERVED_SUB_NAMES`.
- `capability::pending::create_request_inner(requester_capsule_id, binding, reason)` carries both main's `reason` and dkms's capsule identity.
- dkms's additive server modules (`session_lifecycle.rs`, `viewer_*.rs`, `*_authority.rs`, `gateway_marketplace.rs`, `egress_audit.rs`, …) compile against main's `gateway_home_token` helpers `issue_home_launch_token_with_context` / `require_home_token_context` (verified present on main).

- [ ] **Step 1: Kill the dead substrate**

```bash
git rm elastos/crates/elastos-compute/src/providers/wasm.rs
```
`runtime.rs`: take main's side (component wiring, `ComponentProvider::set_bus_hostcall`). From dkms's side, salvage only the hostcall *body* (manifest-capability lookup) — move it inside main's component hostcall closure. Drop dkms's `wasm_provider` + stdio-bridge wiring entirely.

- [ ] **Step 2: `carrier_bridge.rs` — re-seat dispatch arms (biggest single file)**

Keep main's spine: `authorize_and_dispatch_carrier_invoke` + `handle_component_carrier_request`. From dkms's `carrier_invoke_dispatch`, transplant only the *dispatch arms* for the dDRM/marketplace/wallet/chain schemes into main's match structure, routed through `provider_operation_action` (Step 3). Drop dkms's WASM-pipe bridge variant (its transport was deleted with `WasmProvider`). Keep dkms's PRE-AUDIT #3 enforcement call-sites, re-expressed against main's authorize path.

- [ ] **Step 3: op→action convergence — `provider_resource.rs` + `api/handlers/provider.rs`**

Take main's `provider_operation_action(scheme, op)`; extend its tables with dkms's schemes/operations from `required_action_for` (`encrypt`, `publish`, `media`, `market`, `key`, `decrypt`, `drm`, `rights`, `chain`). Delete `required_action_for`. In `handlers/provider.rs`, take main's handler, then port dkms's G-ID fail-closed check (canonical caller identity from `session.vm_id`) on top.

- [ ] **Step 4: Capability plane unions — `pending.rs`, `registry.rs`**

`pending.rs`: keep both sides' constructors (`create_request_with_reason`, `create_request_with_capsule`, `create_affordance_request`) delegating to one `create_request_inner` extended with `reason`. `registry.rs`: union both `RESERVED_SUB_NAMES` lists; keep dkms's `PINNED_SUB_NAMES` + `is_reserved_sub_name`.

- [ ] **Step 5: Token/gateway files — minimal seat, defer redesign**

`gateway_home_token.rs`: take main's projection-token mechanism wholesale; re-add dkms's SameSite cookie delivery functions (`secure`-flag threading) and `require_fresh_passkey_home_token` *compiling but unused if necessary* (Phase 2 rebinds call sites to main's step-up). `gateway.rs` / `gateway_home_runtime.rs`: take main's route construction; re-point dkms imports at main's renamed token API. `gateway_capsule_catalog.rs`: take main's typed `RuntimeCapsuleAffordanceBinding` dispatch; re-express dkms's `secure` threading in it; drop dkms's string-tuple edits and invoke-result shape change.

- [ ] **Step 6: Trivial unions**

`lib.rs`: keep both `pub mod esp_binding;` and `pub mod egress_audit;`. `crypto.rs`: keep both verifiers (`verify_domain_separated_signature`, `domain_separated_verify`). `auth.rs`: take main's audit chain; re-add dkms's small constants/helpers. `gateway_server.rs`: keep both startup hooks (browser lifecycle reconciler + `session_lifecycle::spawn_sweeper()` — sweeper module arrives in a later commit; if unresolved at this stop, gate the call behind the module's introduction, i.e. leave main's side only and let `94b0f63a` re-add it). `auth_gateway.rs`, `gateway_browser_stream.rs`, `server_infra.rs`: test-region unions — keep both sides' tests/helpers.

- [ ] **Step 7: Gate, then continue**

```bash
grep -rn '^<<<<<<<' elastos/ capsules/ && echo "MARKERS REMAIN — fix before continuing"
just check && just check crate=elastos-runtime && just check crate=elastos-compute
just test-crate elastos-runtime && just test-crate elastos-server
git add -A && git rebase --continue   # keep the original commit message
```
Expected: build green; pre-existing tests pass. New dkms tests referencing later commits' modules may not exist yet — that's fine.

### Task 1.3: Resolve remaining stops (B: `b800dbdc` docs ×4, C: `e599c4db` ×2, D: `94b0f63a` ×2, plus any shell-glue stops from `6303906c`)

**Files:** as git stops. Resolution policy per category:

- [ ] **Step 1: Docs stops (`README.md`, `docs/README.md`, `docs/CAPSULE_INSPECTOR.md`, `docs/INSPECTOR_TESTING.md`, `docs/ESP_V0.md` AA):** take main's structure, append dkms's dKMS/dDRM sections where they don't overlap. `docs/ESP_V0.md`: main's version is canonical — discard dkms's copy unless it contains dKMS-specific sections, which move to `docs/` dkms docs.
- [ ] **Step 2: Shell-glue stops (`6303906c` — `capsules/home/**`, `home-gui/**`, `system/**`, manifests):** take main for **every** shell file and **every** `capsule.json` (per analysis §2.7/§3). Do NOT port UI features mid-rebase — Phase 3 re-applies the 4 real UI pieces deliberately. If `capsules/home/browser/shell.js` reappears as DU: keep deleted. Cache-buster-only dkms edits: discard.
- [ ] **Step 3: `e599c4db` (dead-code removal) stop:** if the code it deletes was already resolved away in Stop A, resolve to "already deleted" (`git rebase --skip` if the commit becomes empty).
- [ ] **Step 4: `94b0f63a` (session lifecycle) stop:** keep both startup hooks in `gateway_server.rs` (reconciler + `spawn_sweeper()`); take main's surrounding structure in `gateway.rs`.
- [ ] **Step 5: After EVERY stop:**

```bash
just check && git add -A && git rebase --continue
```

### Task 1.4: Post-rebase full gate

- [ ] **Step 1: Workspace build + full test**

```bash
just build && just test
```
Expected: PASS, including dkms suites now present: `elastos-server/tests/c4_egress_spine.rs`, `tests/ddrm_verdicts.rs`, `api/gateway_tests/marketplace.rs`, `elastos-runtime/tests/capability_conformance.rs`, `capability_token_fuzz.rs`. Fix forward any failure before proceeding (each fix = own commit, `fix(scope): …`).

- [ ] **Step 2: Out-of-workspace capsule crates**

```bash
for c in cenc-core ddrm-envelope ddrm-media content-market act-emitter chain-provider wallet-provider availability-provider; do
  cargo test --manifest-path capsules/$c/Cargo.toml || echo "FAIL: $c";
done
```
Expected: all pass (these were additive; failures indicate API drift from Stop A decisions — fix forward).

- [ ] **Step 3: Commit any fix-forward work; push nothing yet.**

---

## Phase 2 — Rust integration hardening (TDD; one commit per task)

### Task 2.1: Rebind money-verb freshness to main's passkey step-up

**Files:**
- Modify: `elastos/crates/elastos-server/src/api/gateway_home_token.rs` (remove `require_fresh_passkey_home_token`), call sites in `api/{buy_authority.rs, mint_authority.rs, trade_authority.rs, wallet_signer.rs}` (wherever `require_fresh_passkey_home_token` is invoked — locate with `git grep -n require_fresh_passkey_home_token`)
- Test: `elastos/crates/elastos-server/src/api/gateway_tests/marketplace.rs`

**Interfaces:**
- Consumes: main's `gateway_passkey_step_up` public API (read the module first; use the same guard main's recovery/wallet routes use).
- Produces: every money verb (`/api/market/buy`, mint, trade-approval, wallet-sign) rejects stale-auth sessions with the same status main's step-up returns elsewhere.

- [ ] **Step 1: Write the failing test** — in `gateway_tests/marketplace.rs`, using the existing harness (`support_runtime.rs` / `support_providers.rs` — mirror how `wallet/managed_approvals.rs` builds an authenticated session):

```rust
#[tokio::test]
async fn market_buy_requires_fresh_passkey_step_up() {
    let ctx = support_runtime::gateway_ctx_with_session().await; // adapt to harness
    // session authenticated but WITHOUT recent passkey step-up
    let resp = ctx.post_json("/api/market/buy", serde_json::json!({"listing": "test-listing-1"})).await;
    assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED,
        "buy without step-up must be rejected");
    let body = resp.json::<serde_json::Value>().await;
    assert_eq!(body["error"], "passkey_step_up_required");
}
```
(Adapt helper names to the actual harness; the assertion contract — 401 + step-up error code on stale auth — is the requirement.)

- [ ] **Step 2: Run: `just test-crate elastos-server market_buy_requires_fresh` — expect FAIL** (dkms's own freshness check either passes it or the guard is absent).
- [ ] **Step 3: Replace `require_fresh_passkey_home_token` at each call site with main's step-up guard; delete the dkms function.**
- [ ] **Step 4: Run the test — expect PASS. Run `just test-crate elastos-server` — no regressions.**
- [ ] **Step 5: Commit: `feat(gateway): unify money-verb freshness on passkey step-up`**

### Task 2.2: Verify SameSite cookie token delivery end-to-end

**Files:**
- Modify (only if test exposes gaps): `api/{gateway_home_token.rs, gateway_home_runtime.rs}`
- Test: `elastos/crates/elastos-server/src/api/gateway_tests/home_system.rs`

- [ ] **Step 1: Write the failing/characterizing test:**

```rust
#[tokio::test]
async fn app_launch_delivers_token_via_samesite_cookie_not_url() {
    let ctx = support_runtime::gateway_ctx_with_session().await;
    let resp = ctx.get("/api/apps/creator/launch").await; // adapt to actual launch route
    let set_cookie = resp.headers().get_all(axum::http::header::SET_COOKIE);
    assert!(set_cookie.iter().any(|c| {
        let s = c.to_str().unwrap();
        s.contains("SameSite=Strict") || s.contains("SameSite=Lax")
    }), "launch token must arrive as SameSite cookie");
    let location_or_body = resp.text().await;
    assert!(!location_or_body.contains("home_token="), "no URL-borne tokens on money surfaces");
}
```
- [ ] **Step 2: Run — if PASS immediately, keep it as a regression anchor and skip Step 3.**
- [ ] **Step 3: If FAIL: re-thread the `secure`/cookie path from dkms's delivery into main's launch-route construction (`gateway_home_runtime.rs`).**
- [ ] **Step 4: `just test-crate elastos-server` — green.**
- [ ] **Step 5: Commit: `feat(gateway): app-scoped SameSite cookie delivery for launch tokens`**

### Task 2.3: Pinned sub-provider invariant vs main's component registration path

**Files:**
- Modify (if gap found): `elastos/crates/elastos-server/src/supervisor.rs` (now the only `spawn_carrier_bridge` caller on main), `elastos-runtime/src/provider/registry.rs`
- Test: `elastos/crates/elastos-runtime/tests/capability_conformance.rs`

- [ ] **Step 1: Write the failing test:**

```rust
#[test]
fn pinned_sub_name_is_first_writer_wins_across_registration_paths() {
    let registry = ProviderRegistry::default(); // adapt constructor
    registry.register_sub_provider("key", "escrow", provider_a()).unwrap();
    let err = registry.register_sub_provider("key", "escrow", provider_b()).unwrap_err();
    assert!(matches!(err, RegistryError::PinnedNameConflict { .. }));
}
```
- [ ] **Step 2: Run — expect FAIL or PASS-with-gap** (the risk from analysis §5: main's component-provider registration may bypass the pinning check).
- [ ] **Step 3: Route ALL registration paths (incl. the supervisor/component path) through `is_reserved_sub_name` + pinning enforcement.**
- [ ] **Step 4: `just test-crate elastos-runtime && just test-crate elastos-server` — green.**
- [ ] **Step 5: Commit: `fix(runtime): enforce pinned sub-name invariant on component registration path`**

### Task 2.4: Manifest schema migration for the 4 new capsules

**Files:**
- Modify: `capsules/creator/capsule.json`, `capsules/elacity-player/capsule.json`, `capsules/ddrm-viewer/capsule.json`, `capsules/content-market/capsule.json`
- Test: `elastos/crates/elastos-server/src/api/gateway_tests/library.rs` (catalog validation)

- [ ] **Step 1: Write the failing test — catalog must load and validate all four:**

```rust
#[tokio::test]
async fn dkms_capsules_validate_against_esp_catalog() {
    let ctx = support_runtime::gateway_ctx_with_session().await;
    let catalog = ctx.get_json("/api/capsules/catalog").await; // adapt route
    for name in ["creator", "elacity-player", "ddrm-viewer", "content-market"] {
        let entry = catalog.as_array().unwrap().iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("{name} missing from catalog"));
        assert!(entry["invalid"].is_null(), "{name} failed manifest validation");
    }
}
```
- [ ] **Step 2: Run — expect FAIL (old dkms schema: `interfaces@0.1.0` with `input_schema`, `authority` block).**
- [ ] **Step 3: Migrate each manifest to main's convention** — copy the shape from `main:capsules/chat/capsule.json` (web-projection: `runtime_abi: "elastos.runtime-projection/v1"`, `execution: "web-projection"`, `projections`, `capabilities`, `interfaces` methods with `resource` + `operation`) for creator/player/viewer; from a main microvm capsule (e.g. `capsules/chain-provider/capsule.json`) for content-market. Translate content-market's `authority.capabilities` into main's `capabilities` array; keep `audit_events` only if main's schema accepts unknown keys, else move to docs.
- [ ] **Step 4: Run — PASS. Also `just alignment-check`.**
- [ ] **Step 5: Commit: `feat(capsules): migrate dkms capsule manifests to ESP projection schema`**

### Task 2.5: Hostcall capability-lookup parity in ComponentProvider

**Files:**
- Modify: `elastos/crates/elastos-compute/src/providers/component.rs` (only if Task 1.2 Step 1 seated it incompletely)
- Test: `elastos/crates/elastos-runtime/tests/capability_conformance.rs`

- [ ] **Step 1: Write the failing test:** a component invoking a bus call for a capability its manifest does not declare must be denied:

```rust
#[test]
fn component_hostcall_denies_undeclared_capability() {
    // build a component ctx whose manifest declares only ["elastos://did/*"]
    let result = hostcall_authorize(&manifest, "elastos://key/escrow", Action::Invoke);
    assert!(result.is_err(), "undeclared capability must fail closed");
}
```
- [ ] **Step 2: Run — FAIL if the salvaged dkms lookup body wasn't fully wired.**
- [ ] **Step 3: Complete the wiring inside `set_bus_hostcall`'s closure.**
- [ ] **Step 4: Run — PASS; `just bus-conformance` — PASS.**
- [ ] **Step 5: Commit: `feat(compute): manifest-capability enforcement in component hostcall`**

---

## Phase 3 — UI port (take-main shell + re-apply the 4 dkms pieces)

Reference for every step: the dkms originals via `git show review/dkms-foundation:<path>`.

### Task 3.1: Shell open-target allowlist + loading CSS

**Files:**
- Modify: `capsules/home/browser/home-shell-host.js` (the `SHELL_MESSAGE_OPEN_TARGET_SOURCES` map, ~line 68), `capsules/home-gui/browser/style.css`

- [ ] **Step 1:** Add `"ddrm-viewer"` and `"elacity-player"` to `SHELL_MESSAGE_OPEN_TARGET_SOURCES.library` in `home-shell-host.js`.
- [ ] **Step 2:** Append dkms's loading-window CSS block (+143 lines: `.window-loading`, `.window-loading-stages` staged checklist) from `git show review/dkms-foundation:capsules/home/browser/style.css` into `capsules/home-gui/browser/style.css`. It is self-contained and uses CSS vars main also defines.
- [ ] **Step 3:** Verify: `node scripts/home-entropy-check.mjs` (main's version) and `just home-frontdoor-smoke` — both green.
- [ ] **Step 4:** Commit: `feat(shell): allow ddrm-viewer and elacity-player open targets + loading-window styles`

### Task 3.2: Owned-asset open flow in `shell-windows.js`

**Files:**
- Modify: `capsules/home-gui/browser/shell-windows.js`
- Source: `git show review/dkms-foundation:capsules/home/browser/shell-windows.js` (the +474 block)

**Interfaces:**
- Consumes (must exist server-side from the rebase): `POST /api/viewers/open`, `POST /api/market/buy`, `POST /api/viewers/<viewer>/<kind>/<session>/close`.
- Consumes (main's shell primitives, all confirmed present): `openTarget()` (~line 968), `createWindow()`, `browserWindowSpec()`, `focusWindow()`.

- [ ] **Step 1:** Port the guarded early-return branch in `openTarget` for `elacity-player`/`ddrm-viewer` and the ~8 standalone functions (in-flight dedupe, staged loading window, `launchOwnedFromLibrary` → `/api/viewers/open`, buy-then-retry via `/api/market/buy`, viewer-session release wired into `removeWindowEntries` on window close). Mechanical port — same primitives, new file body.
- [ ] **Step 2:** Bump the capsule cache-buster query strings the way main does for shell edits.
- [ ] **Step 3:** Manual smoke (`just home-demo-local`): open an owned asset from Library → staged loading window appears → player/viewer window opens → closing the window fires the session-close POST (verify in server log). Buy-then-retry path: attempt open of a non-owned listing → buy prompt → retry succeeds.
- [ ] **Step 4:** Commit: `feat(shell): owned-asset open flow — viewer sessions, staged loading, buy-then-retry`

### Task 3.3: wallet-metamask robustness re-implementation

**Files:**
- Modify: `capsules/wallet-metamask/browser/wallet-metamask.js` (main's rewritten version)
- Source behavior: `git show review/dkms-foundation:capsules/wallet-metamask/browser/wallet-metamask.js` (`startApprovalAutoRefresh`, `ensureWalletDiscovery`)

- [ ] **Step 1:** Re-implement on main's file: (a) 5s approval auto-poll so a mint tx queued by Creator appears without reopening Wallet; (b) EIP-6963 provider re-discovery with timeout; (c) interaction-busy guard preventing double-submission.
- [ ] **Step 2:** Manual smoke: queue a mint from Creator (Task 3.2 flow) with Wallet open → approval appears within 5s.
- [ ] **Step 3:** Commit: `feat(wallet-metamask): approval auto-poll, EIP-6963 rediscovery, busy guard`

### Task 3.4: Library copy + verify non-conflicting wiring landed

**Files:**
- Verify (should already be present from rebase): `capsules/library/browser/src/{api,app,menu,render,tags}.js`, `library.css` (dkms's +370 "open with viewer" wiring merged clean)
- Modify: `capsules/library/browser/index.html` (optional one-word footer copy)

- [ ] **Step 1:** `git grep -n "open with" capsules/library/browser/src/` — confirm the viewer menu wiring exists post-rebase; if the rebase dropped it (shell-glue take-main sweep), restore from `git show review/dkms-foundation:capsules/library/browser/src/menu.js` et al.
- [ ] **Step 2:** Manual smoke via Task 3.2's flow (Library → open with viewer).
- [ ] **Step 3:** Commit if anything changed: `fix(library): restore open-with-viewer wiring`

---

## Phase 4 — CI & the test anchor

### Task 4.1: Port and validate dkms CI gates

**Files:**
- Modify: `.github/workflows/ci.yml`, `justfile`
- Reference: `git show review/dkms-foundation:justfile` (`verify-ci`, `verify-capsules`), `git show review/dkms-foundation:.github/workflows/ci.yml`

- [ ] **Step 1:** The rebase should have brought `verify-ci` + `verify-capsules` just targets and the CI jobs (`review/**` gating, dDRM Capsule Gate, dKMS Release Invariant). Diff against the dkms branch to confirm nothing was dropped; re-add gaps.
- [ ] **Step 2:** Run each locally:

```bash
just verify-ci
just verify-capsules
cargo build --release --manifest-path capsules/dkms-authority/Cargo.toml            # must SUCCEED
cargo build --release --manifest-path capsules/dkms-authority/Cargo.toml --features legacy-receipt-authz && echo "INVARIANT BROKEN" || echo "fail-closed OK"
```
Expected: first three succeed; the `legacy-receipt-authz` build **fails** (that is the invariant).
- [ ] **Step 3:** Commit: `ci: dkms verification gates on ESP main`

### Task 4.2: End-to-end commerce+viewer rail integration test (the anchor)

**Files:**
- Create: `elastos/crates/elastos-server/tests/dkms_rail_e2e.rs`
- Harness: reuse `src/api/gateway_tests/{support_runtime,support_providers}.rs` patterns (in-process boot providers `encrypt`/`publish`/`media` register at server boot per dkms's `server_infra.rs`)

- [ ] **Step 1: Write the test — full rail, publish → index → buy → acquire → open → stream → close → sweep:**

```rust
//! E2E anchor: the dkms rail must work end-to-end on the ESP substrate.
#[tokio::test]
async fn publish_buy_open_view_close_rail() {
    let ctx = support::boot_gateway_with_providers().await; // encrypt/publish/media in-process

    // 1. Creator publishes a protected asset
    let published = ctx.post_json("/api/apps/creator/prepare-mint", asset_fixture()).await.expect_ok();
    let listing = ctx.await_indexed(&published).await;          // content_index picks it up

    // 2. Buyer (fresh session) cannot open before buying — fail closed
    let buyer = ctx.new_session().await;
    let denied = buyer.post_json("/api/viewers/open", open_req(&listing)).await;
    assert_eq!(denied.status(), 403);

    // 3. Buy (with passkey step-up satisfied), then acquire
    buyer.satisfy_step_up().await;
    buyer.post_json("/api/market/buy", buy_req(&listing)).await.expect_ok();

    // 4. Open now succeeds and yields a viewer session
    let session = buyer.post_json("/api/viewers/open", open_req(&listing)).await.expect_ok();
    let sid = session["session"].as_str().unwrap();

    // 5. Media route serves decryptable segments only with the session
    let seg = buyer.get(&format!("/api/viewers/elacity-player/media/{sid}/init")).await;
    assert_eq!(seg.status(), 200);
    let noauth = ctx.new_session().await.get(&format!("/api/viewers/elacity-player/media/{sid}/init")).await;
    assert_eq!(noauth.status(), 403, "session is bearer-bound, fails closed");

    // 6. Explicit close releases; media route dies with the session
    buyer.post_json(&format!("/api/viewers/elacity-player/media/{sid}/close"), serde_json::json!({})).await.expect_ok();
    let dead = buyer.get(&format!("/api/viewers/elacity-player/media/{sid}/init")).await;
    assert_eq!(dead.status(), 403);

    // 7. Sweeper reaps an abandoned session past its bound
    let s2 = buyer.post_json("/api/viewers/open", open_req(&listing)).await.expect_ok();
    ctx.advance_session_clock_past_bound().await;              // hook session_bounds
    ctx.run_sweeper_once().await;                              // session_lifecycle
    let swept = buyer.get(&format!("/api/viewers/elacity-player/media/{}/init", s2["session"].as_str().unwrap())).await;
    assert_eq!(swept.status(), 403);
}
```
(Helper names adapt to the actual harness; the numbered contract is the requirement. Split into 2–3 tests if the harness favors it.)
- [ ] **Step 2:** Run: `just test-crate elastos-server dkms_rail` — iterate to green. Every failure here is a real integration defect from the port; fix forward in the responsible module, one commit each.
- [ ] **Step 3:** Commit: `test(server): dkms rail end-to-end anchor on ESP substrate`

### Task 4.3: Full-matrix verification

- [ ] **Step 1:** The whole board, in order:

```bash
just fmt && just lint
just build && just test                                  # workspace incl. gateway_tests, c4_egress_spine, ddrm_verdicts
for c in cenc-core ddrm-envelope ddrm-media content-market act-emitter chain-provider wallet-provider availability-provider dkms-authority dkms-keygen; do cargo test --manifest-path capsules/$c/Cargo.toml || echo "FAIL: $c"; done
just bus-conformance && just alignment-check && just capsule-templates
just verify-ci && just verify-capsules && just verify-release
```
Expected: everything green (modulo pre-existing failures recorded in Task 0.2).
- [ ] **Step 2:** Fuzz/conformance long-runs: `just test-crate elastos-runtime capability_token_fuzz` (respect its default iteration budget).
- [ ] **Step 3:** Commit any final fixes; tag the branch state: `git tag dkms-esp-port-verified`.

---

## Phase 5 — Landing

### Task 5.1: Land on `upstream/0.7-dev`, then PR to `main`

- [ ] **Step 1:** Merge (no rewrite of the reviewed history):

```bash
git switch upstream/0.7-dev
git merge --no-ff feat/dkms-esp-port -m "merge: dKMS foundation rebased onto ESP main"
just build && just test
```
- [ ] **Step 2:** Push `feat/dkms-esp-port`; CI must be green including the dKMS Release Invariant job.
- [ ] **Step 3:** Open PR → `main` (via `gh pr create`), body summarizing: rebase strategy, the 6 fixed decisions (analysis §6), test anchors added. **Stop for human review — do not merge the PR.**
- [ ] **Step 4:** After PR merge, cleanup: delete the scratch clone in the session scratchpad; keep `review/dkms-foundation` + `backup/*` until the release ships.

---

## Rollback & risk register

| Risk | Mitigation |
|---|---|
| Stop A resolution goes sideways | `git rebase --abort` returns to `review/dkms-foundation` tip; nothing else touched. Original branch + `backup/dkms-foundation` are never rewritten. |
| Hidden dkms↔ESP API drift beyond the 4 hotspots | Surfaces as compile errors at Task 1.4 Step 2 (out-of-workspace crates) — fix forward with the analysis's §2 convergence table as the tie-breaker (main's mechanism wins). |
| Sweeper/bounds not testable via public API | Add a `#[cfg(test)]` hook in `session_lifecycle.rs`/`session_bounds.rs` rather than sleeping in tests. |
| `just` targets referencing Flint-era paths | `just verify-capsules` failures at Task 4.1 name the stale paths; prune Flint-only entries (we ship dKMS, not Flint). |
| Shell regression from Task 3.2 port | The flow is a guarded early-return branch — feature-scoped; `home-frontdoor-smoke` + entropy check + manual smoke gate it. |

## Acceptance checklist (the definition of "properly working")

- [ ] `just build`, `just test` green on `feat/dkms-esp-port` (workspace).
- [ ] All out-of-workspace capsule crate suites green (incl. `cenc-core` parity, `ddrm-media` av_pipeline, `wallet-provider` suite).
- [ ] `dkms_rail_e2e` green: publish → index → buy (step-up enforced) → acquire → viewer open → session-bound streaming → close → sweeper reap, all fail-closed at each unauthorized step.
- [ ] dKMS Release Invariant holds (release build OK; `legacy-receipt-authz` build fails).
- [ ] `bus-conformance`, `alignment-check`, `verify-ci`, `verify-capsules`, `verify-release` green.
- [ ] Manual shell smoke: Library → open owned asset → play/view → close releases session; buy-then-retry works; Creator mint appears in Wallet within 5s.
- [ ] CI green on the pushed branch; PR open against `main` awaiting human review.
