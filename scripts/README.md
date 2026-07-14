# Scripts

The `scripts/` tree is organized around one rule:

- the `scripts/` root contains top-level commands a developer or operator may run directly
- subdirectories contain lower-level support tooling

Not every root script is a stable end-user entrypoint. Some root scripts are
explicit proof, smoke, audit, or release helpers and should be documented as
such.

## Root Entry Points

Top-level directly-invoked entrypoints stay at the root:

- `agent.sh` — run the agent capsule
- `build.sh` — build runtime and capsules
- `chat.sh` — launch the chat demo
- `install.sh` — signed installer
- `home-demo-local.sh` — prepare and launch the local source-based Home demo in a clean temp home
- `chat-demo-local.sh` — prepare and launch the local Chat demo against a clean local runtime
- `build-chat-room-ui.sh` — build the browser Chat Room UI bundle used by Chat smokes
- `publish-release.sh` — low-level release publisher
- `setup-crosvm.sh` — install runtime VM prerequisites
- `setup-source-home.sh` — build/provision the source-home runtime and generated helper scripts
- `share-demo.sh` — share project docs/content
- `vendor-walletconnect-adapter.sh` — refresh the pinned local WalletConnect adapter asset

If a script is something a human is expected to type from docs, it belongs here.

## Root Proof Helpers

Proof, smoke, and audit helpers also currently live at the root. Common examples:

- `command-smoke.sh`
- `auth-wallet-focus-smoke.sh`
- `wallet-product-safety-smoke.sh`
- `installed-command-audit.sh`
- `installed-provider-verify.sh`
- `local-carrier-chat-smoke.sh`
- `local-carrier-setup-smoke.sh`
- `home-frontdoor-smoke.sh`
- `system-camofox-smoke.sh`
- `chat-room-gateway-camofox-smoke.sh`
- `chat-room-session-reuse-camofox-smoke.sh`
- `chat-room-guest-identity-camofox-smoke.sh`
- `chat-room-runtime-activity-smoke.sh`
- `browser-session-capacity-smoke.sh`
- `browser-abi-provider-contract-smoke.sh`
- `public-install-identity-smoke.sh`
- `public-install-operator-smoke.sh`
- `public-install-home-frontdoor-smoke.sh`
- `public-root-site-smoke.sh`
- `public-user-journey-smoke.sh`
- `home-live-smoke.sh`
- `installed-home-chat-reuse-smoke.sh`
- `installed-native-chat-smoke.sh`
- `protected-content-provider-contract-smoke.sh`
- `publisher-bootstrap-integrity-smoke.sh`
- `recovery-kit-live-smoke.sh`
- `people-conversations-local-smoke.sh`
- `browser-vm-target-refresh.sh`
- `linux-source-home-restart.sh`
- `source-home-capsule-inventory-smoke.py`
- `capsule-live-parity-smoke.py`

These are review and release helpers, not automatically part of the stable
end-user command contract. The `public-install-*.sh` helpers can target a
published candidate gateway by setting `ELASTOS_PUBLISHER_GATEWAY=<url>`. They
use the stamped trusted-source transports by default. During 0.5.0 baseline
review, `ELASTOS_BIN_OVERRIDE=<path-to-branch-elastos>` swaps in the current
branch binary only when the selected gateway serves a 0.5.0-compatible manifest
with the current `home` setup profile and checksummed artifacts. The helpers pin
the installer-selected components manifest so source checkout metadata cannot
leak into the installed-path smoke. Use `scripts/local-carrier-setup-smoke.sh`
for source/local Carrier setup proof before a candidate gateway exists. Set
`ELASTOS_PUBLIC_INSTALL_FORCE_RELAY_ONLY=1` when you intentionally want to
turn the public install proof into a stricter publisher relay-health check.
`recovery-kit-live-smoke.sh` requires `ELASTOS_HOME_TOKEN` from a signed
browser session and is non-mutating unless create/import flags are set.
`people-conversations-local-smoke.sh` is a local review wrapper for the People
and Chat conversation slice. It composes the Home entropy guard with the narrow
Rust/WASM tests for join-object creation, launch-query preservation, profile
cards, People contact projection, and Chat invite-query decoding.
`browser-vm-target-refresh.sh` is the renewable drift-repair path for
already-provisioned Browser VM targets. It refreshes installed helper scripts
and guest script artifacts from the reviewed source checkout without pretending
those helpers are standalone `components.json` release components.
`linux-source-home-restart.sh` is the Linux source-home gateway restart/proof
path for targets such as Jetson. Run it after full source-home setup replaces
the gateway binary so the live front door comes back with a hash-bound Home and
Services receipt.
`source-home-capsule-inventory-smoke.py` proves source-home finalization removes
only managed inactive capsules, preserves user state and unmanaged capsules,
and stamps installed capsule metadata after all other manifest writers finish.
`capsule-live-parity-smoke.py` is the final localhost artifact check: every
active catalog entry must have one matching installed source tree, and every
launchable browser projection must match the bytes served by the gateway.
`auth-wallet-focus-smoke.sh` runs the current passkey, Recovery Kit,
capsule-bridge principal storage, principal-launch, System managed-wallet route,
wallet approval, managed-wallet, BTC, typed chain proof/prepare/broadcast,
chain sync-health, node lifecycle, entropy, and alignment checks as one
repeatable branch gate.
`wallet-product-safety-smoke.sh` is the product-level Wallet release-safety
gate: MetaMask multi-account link/remove, passkey-gated delete and recovery-key
routes, WalletConnect pinned-config gating, hidden Ledger UI, and no hosted
Browser UniSat injection path.
`check-first-party-wasi-gate.mjs` reports every first-party capsule with WASI
Preview 1 evidence, allows only explicit non-product fixture classifications,
and fails on any first-party product WASI usage.
`check-elastos-bus-wit.mjs` verifies the minimal `elastos:bus@v1` WIT world
and fails if it grows raw filesystem, raw network, environment authority,
gateway URL, provider-selection, or WASI import surfaces.

Some root `.mjs` files are script-local harnesses for same-named smoke wrappers
or read-only report generators. Keep those at the root only when they are
directly invoked by public smoke commands or by documented release gates;
private target-maintenance commands should stay outside this repo.

Browser proof helpers are intentionally explicit because Browser is still a
proof surface, not a completed product browser. The commonly referenced gates
include:

- `browser-wallet-bridge-smoke.sh`
- `browser-glide-wallet-smoke.sh`
- `browser-per-launch-selkies-supervisor-smoke.sh`
- `browser-session-capacity-smoke.sh`
- `browser-vm-engine-supervisor-autostart-smoke.sh`
- `browser-home-session-smoke.sh`
- `browser-ela-city-protected-content-open-smoke.sh`
- `HOME_URL=http://localhost:8090/apps/home/ HOME_VIRTUAL_AUTH_BROWSER=1 HOME_VIRTUAL_AUTH_BROWSER_OPEN=1 node scripts/home-passkey-virtual-auth-smoke.mjs`
- `browser-objective-audit.mjs`
- `browser-provider-decision-report.mjs`
- `browser-provider-runbook.mjs`

The Browser operator service wrappers live under `scripts/system/`:

- `elastos-browser-selkies.env.example`
- `elastos-browser-selkies.service`
- `elastos-browser-selkies.sh`

Those files are durable diagnostic/operator packaging for the hosted Browser
proof baseline. They should not be installed as the live product Browser path,
because that creates an always-on shared hosted session. Live Browser config
should use `scripts/browser-per-launch-selkies-supervisor.mjs` so each Browser
launch gets a separate target/control socket and cleans up through `/shutdown`.
Use `scripts/browser-per-launch-selkies-supervisor-smoke.sh` to prove two
independent hosted Browser launches can run concurrently with separate
page-scoped control sockets under a service-style HOME.
Use `HOME_VIRTUAL_AUTH_BROWSER_OPEN=1` on
`home-passkey-virtual-auth-smoke.mjs` to prove the real Home-token path can
open and close a Browser page through the live gateway.
Use `HOME_VIRTUAL_AUTH_BROWSER_OPEN_CONCURRENT=2` and
`HOME_VIRTUAL_AUTH_BROWSER_OPEN_HOLD_MS=30000` with the same smoke to prove
multiple Runtime Browser pages can stay alive, receive heartbeats, appear in
the Browser session-capacity receipt, and close without leaving capacity behind.
Use `HOME_VIRTUAL_AUTH_BROWSER_OPEN=0 HOME_VIRTUAL_AUTH_BROWSER_SUMMARY=1` on
`home-passkey-virtual-auth-smoke.mjs` when you only need to verify that a
running gateway exposes `elastos.browser.session-capacity/v1` without opening a
heavyweight Browser engine session.
Use `scripts/browser-session-capacity-smoke.sh` to prove the active Browser
capacity path. By default it opens one Browser page, holds heartbeats for
30 seconds, verifies an additional open fails closed with structured
`browser_capacity_unavailable`, then closes the page and checks the Browser
session-capacity receipt returns to baseline. Override
`HOME_VIRTUAL_AUTH_BROWSER_OPEN_CONCURRENT` only for a provider that truthfully
supports more active pages than the current Mac VM single-page substrate.
`scripts/browser-home-session-smoke.sh` is the named release gate wrapper for
that proof. Override `HOME_URL`, `HOME_VIRTUAL_AUTH_BROWSER_OPEN_CONCURRENT`,
and `HOME_VIRTUAL_AUTH_BROWSER_OPEN_HOLD_MS` when testing another runtime.
The smoke intentionally fails if the running gateway does not expose
`elastos.browser.session-capacity/v1`; that means the local/public service is
older than the current Browser Session Manager code.
Use `scripts/browser-ela-city-protected-content-open-smoke.sh` to prove the
live Runtime Browser can open the known `ela.city` protected-content page
through a disposable Home passkey and close the page cleanly. This is a Browser
reachability and session-cleanup proof only; it does not prove purchase,
license, key release, or dDRM playback success.

Internal target-maintenance wrappers that depend on private SSH aliases,
reverse tunnels, local usernames, or host-specific data roots should not live in
the public repo. Keep those commands in local operator notes and leave reusable
repo scripts parameterized through explicit environment variables or CLI flags.

## Support Subdirectories

- `build/` — lower-level build helpers
  - `build-rootfs.sh`
  - `build-vm-smoke-rootfs.sh`
  - `build-llama-server.sh`
  - `clean.sh`
- `fetch/` — asset/tool fetchers
  - `fetch-cloudflared.sh`
  - `fetch-model.sh`
- `lib/` — shared shell helpers sourced by top-level proof and release scripts
  - `runtime-cleanup.sh`
- `system/` — explicit operator service wrappers that may be installed by an
  operator, but are not public CLI commands
  - `elastos-browser-selkies.env.example`
  - `elastos-browser-selkies.service`
  - `elastos-browser-selkies.sh`
- `dev/` — development/operator-only helpers that are not part of the public
  command contract
  - `sign-elastos-vz/`

Any deeper deployment or helper assets should stay out of the public root-script story unless they are part of the shipped public contract.

## Design Rules

- One canonical path per operation.
- Root scripts should be obvious, stable entrypoints.
- Root repo launchers use the repo binary by default.
- Installed runtime mode must be explicit (`--installed`) where supported.
- Support scripts should be grouped by job, not historical accident.
- If a script is internal, its path should make that obvious.
