# Debugging Policy

Use this file for stable debugging guidance only. Do not use it as a running
work log.

## Workflow

1. Reproduce the failure with a concrete command, URL, or user journey.
2. Write the smallest falsifiable hypothesis.
3. Run one experiment that can prove or disprove that hypothesis.
4. Record durable findings in the issue, PR, commit message, `state.md`,
   `TASKS.md`, or the relevant `docs/` file after verification.

## Verification

- Prefer product-level smoke tests for Home, app launch, Browser, Wallet,
  pairing, sharing, and mobile/touch behavior.
- Prefer focused tests for the authority boundary being changed.
- Run `node scripts/home-entropy-check.mjs` after UI, naming, routing, token,
  People, Services, or ontology changes.
- Run `node scripts/browser-entropy-check.mjs` after Browser, Browser Engine,
  Exit, WebRTC, VM, or wallet-bridge changes.
- Run `git diff --check` and Rust/JS syntax checks before handing off.
- Keep historical transcripts out of active docs. Git history already preserves
  them.

## Where Notes Belong

- Temporary debugging notes belong in the active issue, review thread, or an
  untracked local note.
- Release truth belongs in `state.md`, `TASKS.md`, `elastos/CHANGELOG.md`, or a
  receipt under the documented proof path.
- Durable architecture guidance belongs in `docs/`.
- Stable local-debugging invariants belong here.

## Logging

The gateway builds its log filter in `elastos/crates/elastos-server/src/main.rs`
from the standard `RUST_LOG` environment variable on top of a hard `elastos=info`
baseline. `RUST_LOG` takes tracing directives (`target=level`, comma-separated);
the most specific matching target wins, so you can tune one subsystem without
touching the rest.

```bash
# More detail from one area:
RUST_LOG="elastos_server::api=debug"

# Silence one noisy target while keeping every other WARN:
RUST_LOG="elastos::provider::latency=error"

# Combine directives:
RUST_LOG="elastos::provider::latency=error,elastos_server::api=debug"
```

Some targets exist specifically so they can be tuned this way. Example:
`elastos::provider::latency` is a tripwire that WARNs whenever a provider round
trip exceeds 150ms. Once a principal root is protected, object-provider event
reads legitimately cost ~200-300ms (the data key is deliberately re-derived per
read — no caching, by security policy), so the tripwire fires on every Home
realtime poll. Filtering that target to `error` is the intended way to quiet
it; do not raise the threshold or cache the key.

Set `RUST_LOG` in the shell that launches the gateway, or for VS Code debugging
in the `options.env` of the gateway bring-up task in `.vscode/tasks.json` (the
gateway passes its env down to every provider it execs).

## Provider Bridge Invariant

Provider request/response pipes must be cancellation-safe. If an HTTP caller
times out or disconnects after a provider request is written, the bridge must
still drain exactly one response line before the next request can read from the
provider. Otherwise a later request can consume stale provider output from an
earlier request.

## Viewer Session Lifecycle Invariant

A viewer session (object or media) pins a gateway-spawned authority subprocess
chain (for a quorum open: `ddrm-media-authority --quorum` → `decrypt-provider`
+ `key-provider`). Those processes outliving the viewer is a bug in one of the
three release paths, not expected behavior: (1) the explicit close route
(`POST /api/viewers/:viewer/{object,media}/:session/close`), fired by the
viewer capsule on `pagehide` and by the Home shell on window close; (2) the
60s `session_lifecycle` sweeper, which releases expired sessions on the clock;
(3) the store's lazy sweep on lookup/admission. When debugging lingering
`ddrm-media-authority` / `decrypt-provider` processes, check those paths in
that order before suspecting the reap itself — session drops must also honor
the deferred-drop contract (reaps run after the store lock releases, never
under it). New session kinds must register with the `SessionLifecycle` trait
in `api/session_lifecycle.rs`; a registered-kinds test pins the registry.

## Provider Deploy Invariant

When a standalone provider changes, deployed Home or app assets are not enough.
Rebuild the provider binary, install it under the active
`XDG_DATA_HOME/elastos/bin`, update `components.json` with the new sha256 and
size when publishing, and restart the gateway so the provider process is
respawned.

## Source-Home Artifact Invariant

Source-home sync must install the artifact declared by each capsule's execution
contract. Runtime projections need their browser tree, WASM Components need
the stamped component artifact and WIT hash, and provider or content
capsules need their declared provider or data artifacts. Do not synthesize a
missing WASI entrypoint as a compatibility fallback. If the installed artifact
does not match the manifest, treat the install as incomplete and verify the
built and installed hashes before debugging Home launch behavior.

## Browser Display Invariant

The Browser product display path is WebRTC remote display through the Runtime
Browser Engine Adapter contract. `runtime_frame`, `diagnostic_frame`,
screenshot, and image-polling routes are not product fallbacks. Debug VM launch,
adapter IPC, WebRTC signaling/media, and Runtime-scoped input receipts instead.

## Browser Engine Selection Invariant

Browser Engine selection must be explicit at the Browser UI/API boundary.
Non-KVM hosts consume a trusted Browser Engine adapter or remote engine service;
they must not silently fall back to local non-isolated browser execution.

## Browser VM Target Invariant

The generated `browser-vm-selkies-start` bridge config and installed
`browser-vm-guest-control-bridge` binary/script are one versioned pair. Target
refresh and preflight must fail when the rootfs bridge does not support
`elastos.browser.vm-guest-control-bridge.config/v1`,
`control_socket_ready_timeout_ms`, and `control_request_timeout_ms`.

## People Discovery Invariant

People discovery is opt-in, short-lived, and principal-rooted. UI and API
summaries must derive visible state from the discovery expiry, not just a
persisted boolean. Expired discovery must not publish presence until explicitly
enabled again.

## Service Selection Invariant

Services state is user-facing service selection, not ambient provider authority.
Local service enablement and remote service subscriptions must stay separated
from provider grants until a reviewed grant-management API exists.

## Authority Envelope Invariant

Capability tokens belong in the Runtime/provider envelope. Provider JSON bodies
must not carry duplicate token fields unless a provider explicitly validates
that field as part of a reviewed protocol.

## Active Work Logs

Do not add running debug notes or host transcripts to this file. Promote only
stable invariants here after the relevant code and tests are updated.

## Browser Address Navigation Invariant

Address-bar navigation may use in-place Browser control while the active page is
healthy. If a scheme, host, or port change fails through page control, or the
active page is already a Chromium error URL, Browser must recover through
`/api/apps/browser/open` so Runtime reserves a fresh Exit route and the Browser
gets a fresh display session instead of reusing stale page state.

## Services Access Request Status Invariant

Services access request validation and delivery failure are separate API
classes. A selected remote service offer that is not tied to a connected People
record is a deterministic request-state error and must return `400`. A request
that passes local validation but cannot be delivered through the service-access
transport remains `503`. Keep both focused tests in the gate:
`test_home_summary_reports_people_contacts_from_accepted_conversation_members`
for the `400` path and
`test_services_remote_exit_request_local_only_does_not_save_requested_state` for
the `503` path.

## Browser Profile Storage Posture Smoke Invariant

Browser profile reset receipts must machine-declare the current non-protected
posture: `storage_posture=principal_owned_reset_scoped_unprotected`,
`protected_storage=false`, `encrypted=false`, and `recoverable=false`.
Manual-review packet smokes must keep draft reports fail-closed with `ok=false`
and must assert the current validator requirement for at least one hash-bound
redacted Mac VM screen recording artifact. Do not replace that with a generic
visual-evidence substring or weaken the no-human-evidence gate.
