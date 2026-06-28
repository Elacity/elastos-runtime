# Ready for Cursor — the local/KVM lane checklist

Everything that can be built in the cloud is done and pushed to
`claude/keep-consent-architecture-0fz0ll`. This file is the clean pickup list for
when you're at a machine with **Cursor + `/dev/kvm` + `CAP_NET_ADMIN`** (a real
Linux host with hardware virtualization and network-admin rights). Each item is
fully designed; only the hardware-dependent build + tests remain.

## Before you start (verify the branch is green in ~2 min)

```bash
cd elastos
cargo test  -p elastos-server -p elastos-runtime -p elastos-common --lib
cargo clippy -p elastos-server -p elastos-runtime --lib --tests -- -D warnings
cargo fmt -p elastos-server -p elastos-runtime -- --check
bash ../scripts/check-wci-alignment.sh
```
All four should pass. (Server lib ~876 tests, runtime ~345, common ~95.)

> **Known env-dependent failures (NOT regressions):** the cloud sandbox cannot run
> a few integration tests — ~9 failures in the **browser-engine** path and some
> **checksum/VM** tests that need a real Linux host. They pass on your machine.
> So a *full* `cargo test` (not just `--lib`) may show those; cross-check any
> failure against `docs/ROADMAP.md` (LOCAL/CURSOR section) before treating it as a
> real break. The `--lib` suites above are the clean in-cloud signal.

## 1. W1b — the real egress firewall  ← highest leverage

**Design:** `docs/W1B_EGRESS_FIREWALL.md` (complete — read it first).
**Why it matters:** converts the reach *model* (already built + tested) into
packet-level *enforcement* — the literal "we can prove the agent stayed in its
lane" proof (EU AI Act containment). This is the single biggest remaining piece.

Build steps (all specified in the design doc):
- A per-TAP `nftables` chain derived from the capsule's `EgressReach`
  (`None`=drop-all, `Allowlisted`=allow-resolved-only, `Open`=allow+tagged-hot),
  installed right after the TAP hook at `supervisor.rs:1115` (`guest_network`).
- An `EgressDenied` signed audit event on the flight recorder for every drop.
- Teardown-on-reap (keyed by `vm-<name>` — mirror the BUG-2/3 leak discipline).
- The DNS proxy (host-scoped allowlist vs IP filter) — proxy primary, IP-set interim.
- The 5 tests in the design doc, incl. the **compromised-guest backstop** (a guest
  bypassing the provider SDK is still dropped by the kernel chain).

## 2. BUG-2 / BUG-3 / BUG-7 — VM-lifecycle leaks

**Spec:** `docs/KNOWN_GAPS.md` ("Performance + bugs" section).
These need a real crosvm boot to test, so they're KVM-lane:
- **BUG-2:** per-launch `-carrier.sock` + detached bridge accept-loop leaked on
  teardown → store + abort the `JoinHandle`, remove the sock.
- **BUG-3:** boot-failure orphan: overlay + sockets + task leaked when `vm.start()`
  errors before the running-map insert → cleanup on the error path.
- **BUG-7:** reap treats the Carrier backend as unconditionally alive → real liveness.
(BUG-1, BUG-5, BUG-6, BUG-8 are already CLOSED in-cloud; BUG-4 mechanism is closed
with 3 real-provider migrations.)

## 3. W5b — the visual Svelte projection shell  (browser lane, not KVM)

**Spec:** `docs/ESP_SHELL_UI.md` (complete — 8 component contracts, each mapped to a
proven `elastos/esp/*.ts` projection + its schema tag, under the no-authority-in-the-
view invariant). The headless model + logic are all built and tested in-cloud;
only the live Svelte paint + a visual snapshot test remain.

## The standing discipline (keep it)

- One gated chunk at a time; commit only when `cargo test` + `clippy -D warnings` +
  `rustfmt --check` + the alignment ratchet are green.
- Verify-first: read the real code before changing an enforcement path; never
  loosen a capability action without confirming the op's true behavior.
- Fail-closed; name every deferral with a ratchet or a privilege requirement.
- The G3b conformance ratchet (`all_provider_manifests_preview_actions_match_verb_map_or_tracked`)
  will fail the build if any preview/enforce drift sneaks in — keep it green.

## The COMPLETE remaining registry (no blind spots)

The three items above (W1b, BUG-2/3/7, W5b) are the **highest-leverage** pickups,
but they are NOT the whole list. The authoritative, complete sources of truth are
**`docs/ROADMAP.md`** (the "DEFERRED / TRACKED" + "LOCAL / CURSOR" sections) and
**`docs/KNOWN_GAPS.md`** (the gap registry, each with a close-criterion or a
`#[ignore]`d ratchet test). Everything else still open, so nothing surprises you:

- **AUD-1 ACTIVATION (production trust):** the author-signature launch gate is
  wired + fail-closed-when-configured, but inert until you generate an author key
  (`trust_cmd`) and set `trusted_keys` in config, then re-sign the capsules. Until
  then only operator sha256 pinning protects launch. (Local/Cursor — founder config.)
- **Carrier-service author gate** — AUD-1 residual (host-binary launch path needs a
  distinct entrypoint-hash design to avoid false-denies).
- **AUD-4 plane-(a) / G8 verify-on-read** — PARTLY LANDED in-cloud: `AuditLog::with_file_verified`
  now opens a file-backed log AND walks the existing hash+signature chain, failing closed (server
  startup aborts) on any tamper, so you can't append onto a laundered history. `server_infra` opts
  in via the `ELASTOS_AUDIT_LOG_PATH` env (durable EU AI Act custody mode); default stays memory.
  **Cursor TODO:** (a) set `ELASTOS_AUDIT_LOG_PATH=<data_dir>/audit/audit.log`, restart, confirm the
  "Durable audit log enabled (verified-on-open)" log line, then hand-tamper a record and confirm the
  next start REFUSES; (b) **tail-truncation is now CLOSED in-cloud** — a `<log>.head-anchor` sibling
  records the committed head seq each emit, and verify-on-open refuses when fewer records verify than
  the anchor promised (hand-test: `tail -n -1` the log, confirm the next start REFUSES with
  "tail-truncated"); the residual is only the *off-box / co-signed* anchor for a full-disk attacker
  who rewrites both files; (c) no LIVE read path (inspector / W7 export) re-runs `verify_chain`
  mid-session yet — startup-only.
- **G8 / G8b (capability plane)** — deny/approve/revoke (request- AND token-level) + affordance-use
  are now all fail-closed-signed. Remaining: ordinary grant/use stay best-effort by design (the
  per-validate hot path; needs the group-commit rewrite below before adding an fsync).
- **G1b / G2b serve-wiring** — the live `serve` path attaches only `AuthAuditSource`
  / registers capsules without verifying; thread the grant source + `verified_signer`
  (blocked on confirming the content-hash domain for the MicroVM artifact).
- **Performance (speed 5/10):** the free reflink/COW rootfs-overlay win
  (`supervisor.rs:1152`), then the audit group-commit rewrite (MEASURE first; never
  coalesce a custody record or cache a revocation/expiry/use-count check).
- **G3b dangerous tail (13, deliberately LOCKED):** drm `open`, encrypt `seal`,
  wallet signing/approval/secret-export, key `release`, decrypt, chain broadcast,
  object `share` — each needs a per-op security decision; the conformance ratchet
  keeps them visible. Do NOT bulk-loosen.
- **Helper-gated DidNotAct** — the last `import_exact` validations (shared-helper
  contract change). Low value.

## Beyond W0-W7 — the macro vision's NEXT band (product roadmap, not bugs)

W0-W7 is the **foundation** of the Sovereign Computer / KEEP, not the whole product.
The PDR ("NEXT" band, see `docs/PDR_SOVEREIGN_COMPUTER.md` + `docs/ROADMAP.md`) is
the next product arc, none of it started here:
- **Dual-receipt PLATFORM self-attestation** — the platform co-signs its own
  accountability at gate time (get a legal-admissibility opinion first).
- **Act-over-MCP (the write path)** — agents *doing*, not just reading, through the
  one consent gate + meter. This is the natural debit site for the spend meter
  (`carrier_bridge` `send_raw` dispatch, where the single-use token is consumed).
- **The spend meter** — bounds an agent's AI/resource spend (adoption wedge #4).
  **MECHANISM + ACT-PATH WIRING LANDED in-cloud `W3b-turn`:** `primitives::spend::SpendMeter`
  (atomic fail-closed `try_debit`, no-op `refund`, `ensure_budget` first-touch; the
  `concurrent_debits_never_overspend` test proves 64 racing debits never exceed the budget) is now
  WIRED into the carrier `carrier_invoke` dispatch: it debits the capsule's budget (keyed on the
  canonical capsule id) BEFORE `send_raw`, refuses fail-closed with `budget_exhausted` + refunds the
  single-use token when the budget is gone, and refunds the spend on the same `NoProvider`/`DidNotAct`
  no-op branches as the token. Signed `SpendDebit`/`BudgetExhausted` audit events on Plane A. Enabled
  by `ELASTOS_DEFAULT_SPEND_BUDGET` (unset ⇒ unmetered; `0` ⇒ hard-stop). Proven by
  `carrier_act_is_refused_when_spend_budget_is_exhausted` + `did_not_act_refunds_the_spend_debit`.
  **Cursor TODO / residual:** (a) only the **serve** act path carries the policy — the **microVM
  supervisor** (`vm-{name}`) and **WASM** carrier sites are `spend_policy: None` (documented
  follow-up; wire them from infra the same way); (b) cost is a flat **1 unit per act** (bounds the
  NUMBER of acts) — provider-reported variable cost (real AI tokens) is the next refinement; (c) the
  consent/affordance dispatch path (`gateway_capsule_catalog`) is not yet metered; (d) per-capsule
  budgets are a flat default — a real per-principal/quota policy + a top-up path is a product decision.
- **Free-text NL → intent** (adoption wedge #3) — the AI-backend "brain" track.
- **The marketplace of shells** — multiple untrusted shells over the ESP protocol.

## When ready to merge

Open a PR from `claude/keep-consent-architecture-0fz0ll`. The campaign is ~42
commits: the W0–W7 ESP core, the consent + signed-receipt flow, the EU AI Act
audit artifact, the security/bug hardening (AUD-1..5 + BUG-1/4/5/6/8), the BUG-4
`DidNotAct` refund mechanism on 3 real providers, and the G3b universal
preview==enforce conformance pin (drift can no longer hide).
