# Capsule Inspector — testing guide

How to set up an environment and manually test the Capsule Inspector on the
`feat/capsule-inspector` branch. Pair this with `docs/CAPSULE_INSPECTOR.md` (the
design/security model) and `docs/INSPECT_DDRM_MERGE_NOTES.md` (the parallel-branch
integration plan).

- **Branch:** `feat/capsule-inspector`
- **Setup role (Cursor agent / operator):** build, run the test suites, stand up
  the runtime, and produce the access steps below.
- **Manual role (human):** click through the UI and run the security checks.

> Keep the branch isolated: do **not** merge or pull in
> `feat/ddrm-hardening-and-creator-parity` (the two branches run in parallel),
> and do not push or open a PR while testing.

## What you're testing

A read-only, capability-gated, object-centered view of every capsule — its
identity/provenance, its declared powers (affordances + provider `authority`),
and its audit trail — plus a metadata-driven **gate preview**: given a provider
operation, it shows the exact capability tuple (resources + actions) and audit
events a call would require, **without executing anything**. It re-secures Self's
live-object / mirror experience under ElastOS's zero-ambient-authority model.

Reachable through both product transports:

| Transport | Entry | Gate |
| --- | --- | --- |
| Browser UI | `POST /api/provider/inspect/<op>` + `x-elastos-home-token` | gateway allow-list → System operator only |
| Capsule / agent | carrier_bridge `carrier_invoke` | capability token → resource/action |

Ops: `capsules` (list), `capsule` (detail), `plan` (gate preview) are read-only.
The write op `revoke` is **not** exposed through the browser proxy.

## Intentionally NOT built yet (do not file these as bugs)

- **Preview only** — there is no human-approval loop and no invoke
  *dispatch*/execution. Nothing actually runs or mutates.
- **`granted_capabilities` is empty by design** — the audit event schema carries
  no resource/action, so the observed-grant list is left empty rather than
  fabricated (see CAPSULE_INSPECTOR.md, "observed, not enumerated").
- **`revoke` must 404 via the browser** — mutation stays on the capability-gated
  carrier/admin path.
- **Sample-data fallback** — when no live source/token is present, the browser UI
  renders bundled sample data. Watch the badge: `live` vs `sample data`.

## Setup (operator)

```bash
git fetch origin
git checkout feat/capsule-inspector        # expect HEAD f8afb2f (or later)

# 1. Build the workspace (binary -> elastos/target/release/elastos)
cargo build --workspace --release

# 2. Test the touched crates
cargo test -p elastos-runtime              # inspect/ scope + invoke/ planner + conformance
cargo test -p elastos-server               # inspect_provider, carrier e2e, gateway inspect

# 3. Lint (a few PRE-EXISTING warnings in request_handler.rs /
#    gateway_capsule_catalog.rs are known/out of scope — flag only NEW ones)
cargo clippy -p elastos-runtime -p elastos-server
```

Relevant tests to confirm green:

- `elastos-runtime`: `inspect::tests::*` (scope, fail-closed),
  `invoke::tests::*` (affordance + provider-op gate, split-block union),
  `tests/inspect_conformance.rs` (a SelfOnly caller cannot read another capsule).
- `elastos-server`: `inspect_provider::tests::*` (projection, no-leak, provenance,
  attestation, gate preview), `carrier_bridge` inspect e2e + the merge tripwire
  `carrier_inspect_ops_match_canonical_action_contract`, and
  `api/gateway_tests/inspect.rs` (token required, System-only, `revoke` 404).

## Two ways to test

### A. Fast UI smoke test — sample mode (no runtime, no token)

The UI is static. Serve the folder and open it; the live API call fails and it
falls back to sample data (expected — badge shows `sample data`).

```bash
cd capsules/capsule-inspector/browser
python3 -m http.server 8099
# open http://127.0.0.1:8099/
```

This exercises the entire UI/UX: list, detail glass box, provenance card, the
**Custody** panel (spend + audit + intent, painted from the shared ESP projection),
and the "preview gate" buttons (which compute the gate locally from the sample
authority). The frontend now lives in `browser/` (matching the `browser` capsule
convention) and loads `inspector.js` as an ES module, so it must be served over
HTTP — opening `index.html` via `file://` will not load the module.

### B. Live mode — runtime + System token

```bash
elastos serve            # API binds ~http://127.0.0.1:8090
```

The browser `inspect` read ops require the **System** capsule's signed
home-launch token in the `x-elastos-home-token` header.

> **Frontier / known gap:** obtaining a real System home-launch token and the
> exact `/apps/<name>/` route the inspector UI is served from in a live `serve`
> may not be fully wired on this branch yet. If you cannot obtain a System token
> or reach the UI live, **document the blocker and fall back to sample mode (A)
> for UI testing** — the security checks below can still be run against
> `elastos serve` with curl once a token is available. (In tests, tokens are
> minted via `issue_home_launch_token(data_dir, SYSTEM_CAPSULE_ID)`; the operator
> task is to reproduce that for a live run, or report what's missing.)

## Manual checklist (human)

UI (sample mode is enough for all of these):

- [ ] Capsule list loads; selecting one shows the detail "glass box."
- [ ] **Provenance card** shows a trust level (`signed` / `content-addressed` /
      `unsigned`), a signature **fingerprint** (short hex), and an audit
      **attested** count / signer DID.
- [ ] A provider capsule (one that declares `authority`) shows its powers, each
      with a **"preview gate"** button. Clicking it reveals the required
      **resources + actions + audit events** — and nothing executes.
- [ ] **Custody panel** shows three independent channels: **Spend** (Unmetered /
      Within budget / Near limit / Budget exhausted), **Audit chain** (No durable
      chain / Chain verified / Chain tampered), and **Agent intents** (No
      agent-intent custody / Intents within grant / Intents flagged — now LIVE from
      the runtime's per-capsule intent-proof tally, Tier 2b). A verified chain sitting
      beside an exhausted budget and a flagged intent must show ALL THREE honestly
      (no green-over-bad, and each channel is independent); e.g. in sample mode the
      `capsule-inspector` row shows `Budget exhausted` + `Chain tampered` +
      `Intents flagged` (1 denied · 0 diverged · 1 undelivered) side by side, while
      `wallet-provider` shows a present-and-clean `Intents within grant` and
      `chat-room` shows `No agent-intent custody` (absent — never a false clean).
- [ ] **No** raw signature, bearer token, or mutation handle appears anywhere in
      the UI (Principle #16).

Security (live mode, curl — substitute `$TOKEN` with a System home-launch token):

```bash
B=http://127.0.0.1:8090

# No token -> 403
curl -s -o /dev/null -w '%{http_code}\n' -X POST $B/api/provider/inspect/capsules \
  -H 'content-type: application/json' -d '{}'

# System token -> 200 + lists capsules
curl -s -X POST $B/api/provider/inspect/capsules \
  -H "x-elastos-home-token: $TOKEN" -H 'content-type: application/json' -d '{}'

# Gate preview (no effect) -> shows resources/actions a call would require
curl -s -X POST $B/api/provider/inspect/plan \
  -H "x-elastos-home-token: $TOKEN" -H 'content-type: application/json' \
  -d '{"id":"<capsule-id>","operation":"<op>"}'

# Write op revoke -> 404 (not browser-reachable) EVEN with a System token
curl -s -o /dev/null -w '%{http_code}\n' -X POST $B/api/provider/inspect/revoke \
  -H "x-elastos-home-token: $TOKEN" -H 'content-type: application/json' \
  -d '{"token_id":"00000000000000000000000000000000"}'
```

- [ ] no token → `403`
- [ ] non-System app token → not `200`
- [ ] System token → `200`, lists capsules
- [ ] `plan` → returns the gate (`resources`, `capability_actions`, `audit_events`),
      with `kind: "operation"` for provider ops; nothing runs
- [ ] `revoke` → `404`
- [ ] no response body contains a raw signature / token / handle

## Reporting

Capture: build result, test + clippy output, the URLs + token steps (or the
blocker), and any UI/security deviation from the checklist. Anything intentionally
not built (see above) is not a defect.
