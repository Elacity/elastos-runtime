# State

Last updated: 2026-04-27 UTC

Product state and open truths for the ElastOS runtime on this branch.
For open work, see [TASKS.md](TASKS.md).
For direction, see [ROADMAP.md](ROADMAP.md).

## What works

- Signed install -> setup -> Home as the default front door.
- Native P2P chat over Carrier with Ed25519 message signing and verification.
- Same-host native ↔ WASM chat interop on shared runtime (proven 2026-03-30).
- Sovereign room control with DID-backed invite/accept flow and hosted `chat-room` access through the explicit operator lane.
- WASM and microVM capsule execution with capability-gated provider access.
- Signed release, update, and publish pipeline (Carrier-first, explicit web bootstrap/override only).
- Operator-only remote node status, room control, and trusted-source update control over Carrier via `elastos node ...`.
- Content sharing, local site hosting, site publish/activate/rollback.
- DID-backed identity (did:key, Ed25519) with encrypted key storage.
- Agent capsule with signed gossip and verified-only AI responses.
- Current Home browser-hosted adapter backed by the internal `home` capsule:
  - truthfully declared as a WASM capsule
  - static runtime-owned browser-hosted adapter under `/apps/home/`
  - first System slice backed by runtime-owned summary + validated launch APIs
  - same-origin iframe attachment for browser-capable apps
  - first-class Inbox, Documents, and Library app surfaces with app-scoped launch tokens
  - Documents publish/unpublish through the runtime documents provider, not a direct capsule or gateway IPFS path

## What is proven

- `just verify` — source-line gate: alignment, clean-home setup, command smoke, candidate command audit, fmt, clippy, and tests.
- `just verify-release` — release-trust gate: `just verify` plus the PTY Home frontdoor smoke.
- `scripts/shared-runtime-gossip-proof.sh` — bidirectional gossip delivery on shared runtime.
- `scripts/chat-wasm-native-interop-smoke.sh` — native ↔ WASM end-to-end.
- `scripts/chat-wasm-local-smoke.sh` — local WASM chat.
- `cargo test -p elastos-server --lib operator_control::tests::test_two_node_operator_status -- --ignored --exact --nocapture` — local two-runtime operator Carrier proof.
- `cargo test -p elastos-server --lib api::gateway::tests::test_room_service_cross_runtime_presence_syncs_join_and_leave -- --exact --nocapture` — local two-runtime room presence proof.
- `cargo test -p elastos-server --lib api::gateway::tests::test_room_service_cross_runtime_room_syncs_over_carrier -- --exact --nocapture` — local two-runtime room message-sync proof.
- `cargo test -p elastos-server --lib api::gateway::tests::test_room_service_cross_runtime_attachment_syncs_over_carrier -- --exact --nocapture` — local two-runtime room attachment-sync proof.
- `scripts/public-install-identity-smoke.sh` — installed-path DID/profile acceptance path.
- `scripts/public-install-operator-smoke.sh` — installed-path operator-node status/update acceptance path.
- `scripts/public-install-home-frontdoor-smoke.sh` — installed-path Home frontdoor acceptance path.
- installed update and portability concerns are covered by the current public-install acceptance helpers, rerunning those helpers against a published gateway via `ELASTOS_PUBLISHER_GATEWAY=<published-url>`, `scripts/audit-linux-runtime-portability.sh`, and `just verify-release`.
- `cargo test -p elastos-server home --lib -- --nocapture` — source-line proof for the static `/apps/home/` Home surface, System summary, and validated Home app launch.
- `cargo test -p elastos-server resolves_browser_surface_for_non_data_capsule --lib -- --nocapture` — generic capsule `browser/` surface coverage.
- `scripts/system-camofox-smoke.sh` — public System browser-hosted acceptance path.
- `scripts/home-camofox-smoke.sh` — Home browser-hosted acceptance path, including desktop/taskbar/window flows, Inbox, Documents, Library, GBA, and refresh session restore.
- `scripts/chat-room-session-reuse-camofox-smoke.sh` — same-browser `chat-room` reuse between Home and direct `/apps/chat-room/`.
- `scripts/chat-room-guest-identity-camofox-smoke.sh` — separate-browser `chat-room` guest identity remains distinct from the Home user.
- `scripts/public-user-journey-smoke.sh` — current public root + System + Home + hosted chat acceptance wrapper.

## Home-branch reality

- `home` remains the internal capsule ID; the visible product surface is Home.
- The current source proof is a runtime-owned Home browser-hosted adapter:
  - `capsules/home/capsule.json` now declares a WASM capsule, not a microVM
  - `capsules/home/browser/` serves Home assets
  - `/api/apps/home/summary` exposes local device identity, Home authority, runtime status, Inbox state, and an explicit app/object catalog
  - `/api/apps/system/summary` is the System summary and requires a System launch token
  - `/api/apps/home/launch` validates Home launch targets and mints app-scoped launch tokens
  - Home attaches browser-capable apps through same-origin iframes
- `system` is the canonical first-party app ID for the visible System surface; there is no alternate System route or package identity.
- Documents publish/unpublish is provider-plane only:
  - Documents and Library use `/api/provider/documents/...` with Home/app-scoped authority
  - the documents provider calls the registered `ipfs` provider for pin/unpin work
  - public CID reads use cached content or the runtime provider registry and fail closed when the registry is unavailable
- The intended Home architecture on this branch is now single-path:
  - runtime-owned Home contract
  - first-party System, Inbox, Documents, and Library apps
  - one truthful `Home -> System -> app/object -> Home` loop
- `chat-room` is now one capsule identity with one shared web surface:
  - inside Home, Home launches it through runtime orchestration rights
  - outside in a normal browser, the same surface runs under browser-session capability policy
  - browser/session authority differs; product identity and room UI do not

## Open truths

- The main blocker is target-machine Home boringness, not missing features.
- Home is more honest than the earlier public line, but some installed-path surfaces are still secondary rather than boring.
- Hosted room setup currently spans `setup --profile demo` plus the explicit operator lane, and that split is still too implicit.
- Installed target-machine proof for the full `elastos -> Home -> app -> Home` path is still a manual acceptance item.
- GBA is locally promising but not yet earned as a public default path; unsupported mobile/WebView engines fail fast when threaded WebAssembly is unavailable.
- Home currently proves hosting, routing, launch authority, windowing, Inbox, Documents, Library, Chat Room, and GBA flows, but still needs installed-path boringness.
- The current target lane is still source-line proof, not installed-path proof.
- System currently shows identity, runtime state, storage counts, and runtime activity; it does not yet expose the fuller runtime/object/capability contract.
- The next target-lane proof is: run one manual installed `Home -> System -> app/object -> Home` acceptance loop and decide the first non-browser attach contract.
- The branch should keep deleting Home-side donor and KVM assumptions instead of layering extra compatibility branches around them.
- The default Home path must remain a KVM-independent browser-hosted adapter so macOS and Windows stay in scope without pretending to offer Linux parity.

## Support boundary

- Linux is the truthful full-runtime baseline (x86_64 and aarch64).
- macOS is not yet a truthful full runtime target on this branch. The
  daemon code compiles and runs natively; `WASM` and `data` capsules run
  natively; `type: microvm` capsules require Linux (KVM + `crosvm`) and
  fail closed on Mac with a clear "not available for darwin-arm64"
  message. See [docs/MAC.md](docs/MAC.md) for the full status, the
  honest trust delta, and the named path to first-class support.
- The named project that makes Mac a first-class ElastOS host with
  identical microVM isolation is the **Apple Virtualization.framework
  backend** — [docs/vz-backend/PLAN.md](docs/vz-backend/PLAN.md). It
  adds `elastos-vz` as a sibling crate to
  [`elastos-crosvm`](elastos/crates/elastos-crosvm/) without touching
  Linux code.
- The Home direction is being redirected so the default Home path does not depend on KVM or donor backend semantics; that is the condition for macOS to become a first-class front-door target later without faking Linux parity.
- Home is the intended front door but not fully boring on every target machine yet.
