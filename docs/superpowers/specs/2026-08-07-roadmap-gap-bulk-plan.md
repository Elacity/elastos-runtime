# Roadmap Gap Bulk Plan — closing every sub-100% item vs the 0.7-dev roadmap

**Date:** 2026-08-07
**Baseline:** `feat/dkms-esp-port` @ `e2cc4229`, measured by the roadmap coverage report at `docs/superpowers/specs/2026-08-07-roadmap-coverage-baseline.md` — that report is this plan's baseline: percentages below are its functional/aligned figures, and progress on any section should be re-measured against it.
**How to read this:** one section per roadmap item, ordered as in the roadmap. Each has: what already exists on the baseline branch, the remaining workstreams (concrete enough to cut into feature-branch tasks), test anchors, dependencies, and **open questions** for anything whose implementation is not yet settled — those need a decision (or a brainstorm) before the workstream is cut into tasks. Nothing here re-plans work that is done.

Cross-cutting near-term hygiene (from the merge ledger, independent of any roadmap item):
- `scripts/home-camofox-smoke.mjs`: 10 stale query-form `home_token` references (extraction broken on Documents/System/Inbox flows) + 2 inert query probes in `capsules/{ddrm-viewer,elacity-player}/index.html`.
- Wire `capsules/home/browser/*.test.mjs` (27 tests incl. spend-prompt) into the justfile/CI.
- `browser_reconciliation` flaky family: symptoms point at a shared timing budget, not individual bad tests — give the module deterministic waits. New flaky `room::test_chat_room_transport_joins_bootstrap_peer_after_topic_already_joined` same treatment.
- Comment rot: stale `required_action_for` references in `carrier_bridge.rs`; ~15 bare-filename doc citations in Rust doc comments now pointing at pre-hub filenames.

---

## Section 1 — Foundation

### 1.1 `feat/shell-ui-esp` — 30% functional / 15% aligned

**Exists:** commerce slice (owned-asset open flow, spend-confirmation dialog, Library viewers, wallet-metamask polish) brokered under Home's existing authority; ESP fact/verb machinery on main (`esp_binding.rs`, `gateway_esp.rs`, typed schemas like `elastos.capsules.catalog/v1`).

**Remaining workstreams:**
1. Define the fact/verb schemas for the commerce + viewer conversation plane: `market.buy`, `create.mint`, `viewer.open/close` as verbs; catalog/ownership/session state as facts. Schema-first, versioned (`/v1`).
2. Migrate the marketplace↔Home bridge from the bespoke 5-key postMessage shape to the ESP verb envelope; the spend-confirmation dialog becomes the consent step of the verb, not a custom flow.
3. Migrate main's own wallet/system REST surfaces to the same verbs (shared debt — this item owns it).
4. Converge the `/api/market/*` auth posture (buy is Home-token-only, siblings accept capsule tokens) as part of the verb migration.

**Test anchors:** verb-schema conformance tests mirroring G3b's shape; re-run the `dkms_rail` e2e through the verb surface; spend-prompt click-driven e2e lands here naturally.
**Depends on:** nothing hard; unlocks aligned-% convergence for 2.3/2.4.

**Open questions:**
- [ ] Verb transport: stay on postMessage-to-Home, or route verbs over the gateway (SSE/stream) so remote shells work identically?
- [ ] One global verb namespace vs per-app namespaces with capability-scoped visibility?
- [ ] Is spend confirmation a generic `consent` verb (reusable for clipboard/egress/mandates) or money-specific?

### 1.2 `feat/elastos-carrier-security` — 35%

**Exists:** invoke-path authority (op→action pre-dispatch, G-ID fail-closed caller identity, refused claims fail closed, identity-keyed teardown), egress audit handoff pieces.

**Remaining workstreams:**
1. Transport authentication: peer identity binding on Carrier connections (who is on the other end, cryptographically).
2. Bounds: per-peer/per-capsule budgets (rate, bandwidth, concurrent streams) enforced at the transport layer.
3. Lifecycle ownership: Runtime owns connection open/close/reap; no capsule-held transport handles that outlive their session.
4. Audit handoff: transport events into the audit chain with the same shape as invoke audits.

**Test anchors:** authenticated-peer refusal tests; budget-exhaustion tests; teardown-reaps-connections tests.
**Open questions:**
- [ ] Identity binding primitive: DID-derived session keys, or reuse the node-identity work from dKMS custody (2.2)?
- [ ] Boundary with 1.3: does authentication live here (transport) or in the signed protocol envelope (or both, defense-in-depth)?

### 1.3 `feat/elastos-carrier-protocol` — 0%

**Exists:** nothing on-branch.
**Remaining workstreams:** design + implement one signed, versioned, replay-resistant Runtime-to-Runtime request/receipt protocol above Carrier: envelope (signature, version, nonce/expiry), request/receipt pairing, replay windows, error taxonomy.
**Test anchors:** replay-attack tests, version-skew tests, receipt-verification round-trips.
**Depends on:** 1.2 (or at least its identity decision).
**Open questions:**
- [ ] Envelope format (CBOR/COSE vs JSON + domain-separated sigs — the crypto helpers exist in `crypto.rs`)?
- [ ] Replay resistance: per-peer monotonic counters vs nonce cache windows — and who persists them?
- [ ] Receipt semantics: transport-delivery receipts, or application-outcome receipts (mandate-core and agent-budget both want the latter)?

### 1.4 `feat/elastos-collaboration-provider` — ~0%

**Exists:** chat capsules on raw Carrier gossip; incidental manifest updates only.
**Remaining workstreams:** typed message/room/collaboration provider contracts (schema-carrying manifests exist as a pattern now); migrate Chat, then Agent, off raw gossip; room membership/authz model.
**Test anchors:** contract conformance per provider; migration parity tests (old gossip vs typed path produce same room state).
**Depends on:** 1.3 for cross-node rooms; local-only rooms could start earlier.
**Open questions:**
- [ ] Room state ownership: provider-local, CRDT-replicated, or chain-anchored for public rooms?
- [ ] Message persistence + retention policy (and its interaction with the availability contract, 1.5)?

### 1.5 `feat/elastos-content-availability` — 25%

**Exists:** the commerce rail exercises pin/fetch (acquire pins into Library), `content_index`, availability-provider, pinned `availability`/`peer` sub-routes.

**Remaining workstreams:**
1. Define the single availability contract: fetch, pin, provide, cache, repair, availability-report — one provider interface.
2. Migrate consumers one at a time: Library pinning → viewer media fetches → publish pipeline → commerce acquire.
3. Repair + re-provide loop (background), with audited decisions.

**Test anchors:** contract conformance suite; consumer-migration parity tests; kill-a-provider repair test.
**Open questions:**
- [ ] Pinset ownership: principal-scoped pinsets vs capsule-scoped (Library vs viewer caches have different lifetimes)?
- [ ] Cache eviction policy and who arbitrates disk budgets (ties to 3.1 resource limits)?
- [ ] Is IPFS the only backend at v1, or is the contract proven with ≥2 backends from the start (webspace-interop pressure)?

### 1.6 `feat/elastos-carrier-core` — 0%

**Exists:** nothing (by design — it's the shrink step).
**Remaining workstreams:** after 1.4 + 1.5 extractions, reduce Carrier to authenticated transport, framing, routing, budgets, lifecycle, audit handoff; delete the vacated surface.
**Test anchors:** the 1.2 suite keeps passing against the shrunk core; grep-guard that extracted domains don't reappear.
**Depends on:** 1.2, 1.4, 1.5. No open questions until then.

---

## Section 2 — Content creation and playback

### 2.1 `feat/elastos-webspace-interop` — 10%

**Exists:** `webspace-provider` correctly bound to the pinned `webspace` slot via `localhost_delegated_scheme` (reference shape for delegated mounts).

**Remaining workstreams:**
1. Space-mount provider interface: mount/unmount/list/stat over replaceable backends (local, cloud, IPFS, DID-addressed, friend).
2. Credential isolation: backend credentials never reach capsules; provider holds them behind the capability plane.
3. Friend spaces: authenticated remote mounts.

**Test anchors:** two-backend conformance (local + one remote); credential-leak negative tests (grep-guard + runtime).
**Depends on:** 1.3 for friend spaces; 1.5 for cache/pin semantics of remote content.
**Open questions:**
- [ ] Are backend credentials dKMS-custodied (threshold-escrowed like CEKs) or locally sealed per node?
- [ ] Mount namespace: more delegated `localhost://` subtrees vs first-class `webspace://` URIs in capsule manifests?

### 2.2 `feat/elastos-dkms-custody` — 65%

**Exists:** threshold custody core (`dkms-authority`, `dkms-keygen`, PQ-hybrid envelopes, escrow rail), release fail-closed invariant CI-gated, audit chains.

**Remaining workstreams (the "re-prove" half):**
1. Degraded recovery: explicit code paths + runbooks for quorum-member loss; recovery drills as tests.
2. Fault-tolerance proofs: node churn/partition simulation suite (kill members mid-escrow, mid-recover).
3. Node identity lifecycle: enrollment, key rotation, revocation of custody nodes.
4. Operator evidence: auditable custody receipts surfaced somewhere inspectable.

**Test anchors:** kill-quorum-member e2e; rotation-under-load test; evidence-chain verification test.
**Depends on:** 1.3 for cross-node receipts; 4.4 (inspector) as the natural evidence surface.
**Open questions:**
- [ ] Trust root for node enrollment: chain-anchored registry, operator allowlist, or DID web-of-trust?
- [ ] Threshold parameters policy: fixed at seal-time forever, or re-shareable (proactive secret sharing) when membership changes?
- [ ] Where does operator evidence live: inspector facts (4.4), a dedicated audit page, or chain anchors?

### 2.3 `feat/elastos-protected-content` — 85% / 75% aligned

**Exists:** the branch's core — full provider chain, CENC, viewers, session lifecycle, e2e anchor.

**Remaining workstreams:**
1. **Subject-resolution rewire** (the gap between dev-lane and chain-mode): thread `RequiredHomeLaunchToken` through the 7 remaining `resolve_subject_address` call sites (2 in `viewer_open.rs`, 5 in `gateway_marketplace.rs`), widen `runtime_wallet_data` visibility (`gateway_wallet_accounts.rs`), keep the raw-wallet-dispatch denylist at its current ledger (then shrink it to zero: `creator.rs` accounts ×2, `wallet_signer.rs` create_managed_account). Un-deads the 4 parked `viewer_open.rs` fns. Prerequisites are enumerated in the PARKED block at `viewer_open.rs`.
2. Real decrypt-binary chain in CI: a lane that builds the 5 binaries (`ddrm-media-authority` → `decrypt-provider` → `key-provider` chain) and runs one true decrypt e2e.
3. `image-viewer` for plain Library images (the self-clearing `KNOWN_UNSHIPPED_VIEWER_IDS` test will flag when shipped).
4. ESP verb migration of the viewer surfaces (owned by 1.1; parity tests live here).

**Test anchors:** extend `dkms_rail` with a chain-mode lane once (1) lands; the decrypt-chain e2e; existing fail-closed suite stays green throughout.
**Open questions:**
- [ ] CI cost/placement of the decrypt-binary lane: nightly vs per-PR, and does it need Linux-only hardware features?
- [ ] Live-chain test strategy: mock chain in CI forever, or a testnet lane (`live-chain` feature is currently dev-modes-gated by design)?

### 2.4 `feat/elastos-content-commerce` — 85% / 65% aligned

**Exists:** full publish→index→buy→acquire rail, Wallet/Chain integration, marketplace capsules, e2e anchor.

**Remaining workstreams:**
1. `/api/market/search` over HTTP: break the `OnceLock<PathBuf>` process-global (inject the data dir) so the route is order-independent and testable; then cover it.
2. Create-portal browser caller for the `create.mint` broker rail (re-add `creator` to `MONEY_VERB_APP_SOURCES` — one line when the UI lands, per the in-code note).
3. Discover/UX depth: search facets, listings pagination, storefront polish.
4. ESP verb migration + market-route auth convergence (owned by 1.1).

**Test anchors:** HTTP search tests; Creator mint click-through e2e (with 1.1's consent verb).
**Open questions:**
- [ ] When order routes (`sell/withdraw/approve` — currently unsigned-tx builders) gain any server-side signing, they must join the step-up set: decide now whether to pre-emptively gate them to prevent drift.
- [ ] Search index ownership: keep in-process `content_index`, or move behind the availability contract (1.5)?

---

## Section 3 — Hardening

### 3.1 `fix/component-runtime-hardening` — 25%

**Exists:** manifest-capability ceilings fail-closed in the hostcall, denial classification, session bounds, capsule watchdog.

**Remaining workstreams:**
1. Manifest schema for per-activation resource declarations: memory, fuel, deadline, instance count.
2. Enforcement in `ComponentProvider`: wasmtime fuel metering + epoch deadlines (the dropped `WasmProvider` watchdog work reincarnated for the component model), memory limits per store, instance caps.
3. Truthful limit-exceeded errors reaching guests as their own class (not `Internal` — the classification plumbing from 1280af7f is the pattern).

**Test anchors:** per-limit exhaustion tests (fuel, memory, deadline, instances) proving both refusal and non-poisoning of neighbors.
**Open questions:**
- [ ] Fuel vs epoch-only: is deterministic fuel metering worth its overhead, or are epoch deadlines sufficient for v1?
- [ ] Limits schema defaults: opt-in per manifest vs Runtime-imposed defaults with manifest overrides?

### 3.2 `feat/elastos-capsule-trust` — 20%

**Exists:** `resolve_verified_signer` (advisory, honest-`None`), capability receipts, integrity-verified manifests assumption.

**Remaining workstreams:**
1. Make signature verification enforcing at materialization (currently `trusted_keys` empty by default = gate inert).
2. Publisher trust chain + dependency closure verification (a bundle's deps are also verified).
3. Cross-node re-instantiation receipts (pairs with 1.3 receipts).
4. Interface-compat verification: declared interfaces vs actual exports (bus-conformance machinery is the seed).

**Test anchors:** unsigned/tampered-bundle refusal e2e; dependency-substitution attack test.
**Open questions:**
- [ ] Rollout without breaking dev: dev-modes bypass (fail-open in dev, fail-closed in release — the `dkms-authority` compile-gate pattern), or a warn-then-enforce migration window?
- [ ] Trust root distribution: chain-anchored publisher registry vs operator-configured allowlists?

### 3.3 `feat/elastos-runtime-lifecycle` — 30%

**Exists:** identity-keyed teardown, viewer-session sweeper, boot-bind hard-fail, truthful refusal statuses.

**Remaining workstreams:**
1. Restart + cancellation: supervised restart with clean slate, cooperative cancellation tokens for in-flight work.
2. Fix the known gap: a refused carrier claim leaves an unreapable capsule (`CapsuleBackend::Carrier(None)` treated as alive).
3. `seal_boot_providers()`: make the boot-pin guard structural (mark-on-registration / boot-completion seal) instead of temporal+single-file-literal (parked ledger item).
4. Truthful status surface (running/degraded/reaping) and, later, Runtime-owned streams.

**Test anchors:** restart-preserves-invariants tests; refused-claim-reap test; pin-guard test that catches an out-of-file boot registration.
**Open questions:**
- [ ] Runtime-owned streams: which consumer drives the design first — viewer media, collaboration (1.4), or agent output?

### 3.4 `feat/elastos-remote-access` — 0%

**Exists:** nothing on-branch.
**Remaining workstreams:** ela.city domain setup (DNS, TLS), tunnel lifecycle + revocation, rate limiting, passkey-origin migration.
**Test anchors:** tunnel revocation e2e; rate-limit tests; origin-migration credential tests.
**Open questions:**
- [ ] Tunnel substrate: Carrier-native tunnel vs conventional (WireGuard/FRP-style) sidecar?
- [ ] **Passkey-origin migration is security-critical and unsolved**: WebAuthn credentials are origin-bound — moving a Home from localhost to `<name>.ela.city` re-binds identity. Dual-origin enrollment window? Recovery-flow-based rebind? Needs its own brainstorm before any implementation.
- [ ] Rate limiting scope: per-IP at the edge vs per-principal at the gateway (or both)?

---

## Section 4 — Later follow-ups

### 4.1 `feat/elastos-mandate-core` — 15%

**Exists:** the step-up token shape (signed, scoped, expiring, intent-bound, single-use) as a narrow precursor; `access_grant.rs`; the `walletPersonalSign` deferral explicitly waiting on this.

**Remaining workstreams:**
1. Mandate schema: signed, scoped, expiring, **agent-bound**, revocable delegated authority; local-only first (remote waits for 1.3 per the roadmap).
2. Server-side delegation challenges replacing `walletPersonalSign`'s direct `window.ethereum` access (the documented proper fix).
3. Revocation index + enforcement at consume time.

**Test anchors:** mandate lifecycle e2e (issue→exercise→expire/revoke→refuse); the signing-oracle negative test (a mandate must never become raw signing).
**Open questions:**
- [ ] Unify with step-up tokens (one authority-token family with different scopes/lifetimes) or keep separate primitives?
- [ ] Agent binding: to a capsule identity (G-ID), a DID, or a running-instance handle?

### 4.2 `feat/elastos-agent-budget` — 5%

**Exists:** single-use grant refund semantics as a distant cousin.
**Remaining workstreams:** reserve/commit/release/receipt accounting for delegated spending; per-mandate budgets; receipts feeding the audit chain.
**Depends on:** 4.1 (budgets attach to mandates).
**Open questions:**
- [ ] Unit of account: native token only, or multi-asset with oracle pricing?
- [ ] The step-up burn-vs-recover ruling (buy/mint burn tokens; wallet-send recovers via effect ledger): agent spending needs the effect-ledger shape — build it here or generalize wallet-send's?

### 4.3 `feat/elastos-egress-policy` — 45% (pre-seeded)

**Exists:** `egress_audit.rs` + `egress_firewall.rs` (crosvm), `c4_egress_spine` suite, `net`/`exit` sub-routes pinned.

**Remaining workstreams:**
1. Default-deny rollout for all microVMs (firewall exists; make deny the default posture with manifest-declared allowances).
2. Policy surface: how allowances are declared (manifest capabilities extension) and decided (Runtime policy + user consent).
3. Audited-decision UX: egress grants/refusals visible (inspector, 4.4).

**Test anchors:** extend `c4_egress_spine` with default-deny + allowance tests; policy-drift grep-guard.
**Open questions:**
- [ ] Policy granularity: per-host, per-CIDR, per-service-class ("chain RPC", "IPFS gateways")?
- [ ] Consent moment: install-time (manifest review) vs first-use prompts (the 1.1 consent verb) vs both?

### 4.4 `feat/elastos-capsule-inspector` — 10%

**Exists:** inspector docs reconciled to the live self-tier routing (currently doc-pinned only).
**Remaining workstreams:** the read-only ESP-facts renderer (web-projection capsule consuming catalog/audit/session facts — never owning domain logic); restore test coverage for the self-tier routing (the dkms-era affordance tests shipped on Flint and were dropped — re-express them against the current tree); surface custody operator evidence (2.2) and egress decisions (4.3).
**Test anchors:** read-only-ness guard (inspector can render but never invoke mutating verbs); routing-tier tests.
**Open questions:**
- [ ] Facts source: does the inspector read the ESP fact schemas directly (1.1) — making it the first pure-facts consumer and a forcing function for that design?

---

## Suggested sequencing (dependency-honest, not a commitment)

1. **Now / with the merge:** cross-cutting hygiene; 2.3-1 subject-resolution rewire (unblocks chain-mode); 2.4-1 search fix.
2. **Next:** 1.1 verb schemas (unlocks aligned-% convergence + the consent verb that 4.3 and 3.4 want); 3.3-2/3 lifecycle fixes (small, known); 4.3 default-deny (cheapest big win, 45% done).
3. **Then:** 2.2 re-proving; 3.1 resource limits; 3.2 trust enforcement (needs its rollout decision first).
4. **Later, as scheduled:** 1.2→1.3→1.4/1.5→1.6 chain; 2.1; 3.4 (after its passkey-origin brainstorm); 4.1→4.2.
