# State

Last updated: 2026-05-25 UTC

Product state and open truths for the ElastOS runtime on this branch.
For open work, see [TASKS.md](TASKS.md).
For direction, see [ROADMAP.md](ROADMAP.md).

## What works

- Signed install -> setup -> Home as the default front door.
- Native P2P chat over Carrier with Ed25519 message signing and verification; local history persistence now requires a principal-scoped launch context instead of shared `Users/self`.
- Same-host native ↔ WASM chat interop on shared runtime (proven 2026-03-30).
- Sovereign room control with DID-backed invite/accept flow and hosted `chat-room` access through the explicit operator lane.
- WASM and microVM capsule execution with capability-gated provider access.
- Signed release, update, and publish pipeline (Carrier-first, explicit web bootstrap/override only).
- Operator-only remote node status, room control, and trusted-source update control over Carrier via `elastos node ...`.
- Content sharing, local site hosting, site publish/activate/rollback.
- Device DID identity (`did:key`, Ed25519) with encrypted key storage for node/Carrier signing; passkey principals remain the local account roots.
- Agent capsule with signed gossip and verified-only AI responses.
- Current Home browser-hosted adapter backed by the internal `home` capsule:
  - truthfully declared as a WASM capsule
  - static runtime-owned browser-hosted adapter under `/apps/home/`
  - first System slice backed by runtime-owned summary + validated launch APIs
  - same-origin iframe attachment for browser-capable apps
  - first-class Inbox, Documents, and Library app surfaces with app-scoped launch tokens
  - a first visible `browser` capsule shell for testing the Browser/Net/Exit product shape; it declares Runtime wallet/network capability intent and calls `/api/apps/browser/open`, where Runtime owns the internal Net/Exit/Engine handoff. With an explicit operator-configured Browser Engine Adapter and Exit backend, it can render public HTTPS pages while still blocking private/LAN targets by default. Browser wallet/account discovery and selected signing/transaction intents are Runtime-mediated, not direct wallet authority. The visible Browser still does not claim final cross-platform support, final native/microVM isolation, or accepted product audio/UX until the remaining Browser/Net/Exit gates pass as defined in [docs/BROWSER_CAPSULE.md](docs/BROWSER_CAPSULE.md) and [docs/BROWSER_PROVIDER_BAKEOFF.md](docs/BROWSER_PROVIDER_BAKEOFF.md)
  - `net-provider` is the first Browser/Net provider boundary: it validates requests, blocks LAN/private targets, and returns an explicit exit handoff instead of using direct host networking
  - `exit-provider` defines the internal Browser egress contract and fails closed until a local, Carrier-routed, privacy, paid, or enterprise backend is configured; constrained `http_fetch` and stream-session reservation are available only through `ELASTOS_EXIT_PROVIDER_CONFIG` host allowlists and remain separate from the browser byte transport. Configured stream backends can now return private `elastos.adapter-ipc/v1` engine descriptors and private `elastos.exit.relay-ipc/v1` Exit relay descriptors
  - `browser-engine-adapter` defines the internal Browser Engine Adapter contract and fails closed until `ELASTOS_BROWSER_ENGINE_ADAPTER_CONFIG` and attached `adapter_ipc` byte transport exist; it validates private `elastos.adapter-ipc/v1` descriptors passed by Runtime, exposes screenshot/input operations only through Runtime-scoped page routes, and native adapter kinds must go through an operator-approved `elastos.browser.engine.supervisor-result/v1` launch proof before returning a page receipt
  - `browser-engine-supervisor` is the first Linux host helper for native browser engines: it validates `ELASTOS_BROWSER_ENGINE_SUPERVISOR_CONFIG`, starts the configured engine under `linux_new_netns`, passes only stream/IPC/target/URL environment to the child, and returns an explicit `native_surface` display session for local launcher/mobile-style product hosts. `scripts/browser-native-supervisor-smoke.sh` is the target-host proof for this boundary: direct TCP, DNS, and HTTP must fail inside the engine namespace while Runtime Unix stream-bridge traffic still works; this container cannot complete that proof because `CLONE_NEWNET` is not permitted
  - `browser-stream-bridge` is the first Linux local byte-transport helper: it forwards between a private engine Unix socket and a Runtime-owned Unix stream socket, with no TCP, DNS, HTTP, wallet, chain, or raw host-network path; `browser-engine-supervisor` can launch it before the native engine when the internal descriptor carries `runtime_stream_path`
  - `browser-local-exit` is the first server-side Browser Exit relay: it reads `ELASTOS_BROWSER_LOCAL_EXIT_CONFIG`, accepts typed Runtime relay-open handshakes on a private Unix socket, dials only operator-approved public targets, supports a `"*"` public-web policy for browser use, and blocks private resolved IPs by default
  - `browser-playwright-engine` is the first renderable server/headless proof: it launches Playwright Chromium through the Browser Engine Adapter contract, routes page requests through the Runtime proxy and `browser-local-exit`, returns a WebRTC proof surface backed by CDP screencast, exposes constrained EIP-1193 account/chain discovery, and routes `personal_sign` into typed Wallet/Inbox approval requests that resolve only after managed or connector signature completion. Playwright is diagnostic/test infrastructure, not the product browser runtime
  - the current hosted Selkies/GStreamer service is a managed baseline for hosted Browser proof, not final product completion. Live Browser now launches one isolated Selkies Runtime Exit target per Browser page through `scripts/browser-per-launch-selkies-supervisor.mjs`; each page owns its returned control socket and page operations fail closed without that page-scoped session. It exposes a `product_compositor` WebRTC display session with audio/video/datachannel input and `direct_network=false`, and has passed controlled media, wallet bridge, and Glide-connect gates. It is still not accepted as the final Browser because stable arbitrary YouTube/audio behavior, manual UX acceptance, principal-scoped Browser profile persistence, and cross-platform native/host adapters are still open
  - Gateway allocates private Browser Runtime stream socket paths under the short host temp directory `elastos-browser-streams/` for internal Browser Engine launch requests, relays them only to private Exit relay Unix sockets when configured, closes fail-closed otherwise, strips `adapter_ipc` and `relay_ipc` from Browser UI responses, and does not pass `relay_ipc` to Browser Engine Adapter
  - Documents publish/unpublish through the runtime documents provider, not a direct capsule or gateway IPFS path
  - passkey-first unlock: first passkey becomes admin, later passkeys become guests with separate `localhost://Users/<principal-root>` roots, and System controls new guest enrollment
  - unsigned Home opens to a standard, non-user desktop that encourages passkey sign-in without exposing user identity, wallpaper, browser state, runtime state, or notifications
  - passkey prompt follows the PC2 login visual language while staying Runtime-native: centered dark card, ElastOS branding, concise copy, no wallet dependency
  - passkey prompt stays mounted through status checking and signed boot; session refresh accepts the HttpOnly `home-session` cookie rather than requiring JS-held authority
  - Home has an explicit sign-out control that revokes the current proof-bound session and clears the HttpOnly `home-session` cookie
  - signed Home browser sessions refresh through `/api/auth/sessions/refresh` instead of relying on stale launch tokens
  - signed Home/System agent testing now has a loopback WebAuthn virtual-authenticator proof in `scripts/home-passkey-virtual-auth-smoke.mjs`; it uses real browser passkey ceremonies, refuses remote Home mutation by default, launches System with an app-scoped token, and revokes the disposable test passkey
  - open-window session restore is bound to a browser-context id in site storage and de-dupes targets, so clearing cookies/site data creates a fresh context instead of replaying stale server-side windows
  - Home browser layout/session/recent-target state is stored under the active principal's `localhost://Users/<principal-root>/.AppData/ElastOS/Home/` area and uses the protected principal-root object envelope when that root has verified protection
  - Home appearance state is also active-principal-owned: System writes wallpaper and overlay preferences under `localhost://Users/<principal-root>/.AppData/ElastOS/Home/Appearance/`, not shared runtime state
  - passkey management is runtime-enforced: admins can manage guest passkeys, guests can only see/remove their own passkey, and the last admin cannot be removed while guest passkeys remain
  - guest enrollment is self-registration: System only lets admins open/close enrollment, and guests create their own passkey/principal from Home
  - `/api/auth/recovery/status`, `/api/auth/recovery/create`, `/api/auth/recovery/export`, and `/api/auth/recovery/import` are proof-bound to the active principal; Recovery Kit creation generates a per-principal data key, wraps it to a recovery phrase, stores a runtime-encrypted downloadable archive, and import/export validate principal/root binding with signed audit
  - viewer/content storage paths that still use the capsule-facing `localhost://Users/self` convention are resolved through the signed Home launch-token principal before hitting disk and use the protected principal-root object envelope when that root has verified protection
  - Documents provider requests are bound to the signed Home launch-token principal, so each passkey principal gets a separate `localhost://Users/<principal-root>/Documents/` working-copy area; protected roots reject plaintext document-body reads instead of silently migrating
- Identity layers are intentionally separate: local handles are display labels, global names require a future EID/DID-chain registry claim, `did:key` is the device/node DID, `did:elastos`/EID is a linked global account path, and CIDs identify immutable content graphs.
- The generic capsule-kernel bridge maps `localhost://Users/self` through an explicit principal context when present, rejects explicit foreign `localhost://Users/<root>` access before approval prompts, routes protected in-runtime `Users/self` read/write calls through the runtime principal-root object envelope, and rejects capability requests or `carrier_invoke` calls when principal context or the protected bridge is missing. Home-backed runtime launches now forward the signed launch-token principal into WASM `BridgePipes`; shell/supervisor microVM launches can now validate a signed app-scoped launch grant and pass the verified principal into `BridgeContext`; attached/remote bridge storage and native CLI user-root storage remain fail-closed until they have the same explicit protected storage bridge.
- `chain-provider` exposes typed production chain status/proofs, EVM transaction prepare/broadcast, rights reads, sync health, and System-gated persistent node-lifecycle status without returning backend RPC URLs or node ports to capsules.
- Wallet approval is owned by Wallet/Inbox, not System: Wallet creates passkey-managed accounts and typed approval requests, Home surfaces Inbox attention, Inbox can review/reject wallet requests, and built-in signing routes back to Wallet for a fresh passkey ceremony before `wallet-provider` signs or mutates key material. Runtime records signed audit for request, approval, rejection, account deletion, and recovery-key access.
- Wallet UX now treats MetaMask, Bitcoin wallets, WalletConnect, and the passkey-managed wallet as approval methods under one Wallet mental model. The visible Wallet capsule creates passkey-managed accounts, shows all linked accounts, reads native ESC/Base/BTC balances through typed `chain-provider` balance calls, selects defaults, reviews approvals, renames accounts, deletes/removes accounts after fresh passkey confirmation, generates receive QR codes, exports/imports per-account Wallet recovery keys after fresh passkey confirmation, and opens connector capsules only as approval-method handoffs. Internally, external wallet connectors are still split by real authority: `wallet-metamask` links and signs EVM requests through MetaMask, while Wallet's Bitcoin approval method links Bitcoin accounts through a BIP-322 connector handoff. `wallet-walletconnect` has a dormant source capsule, an exact-version adapter build script, and a smoke-tested operator config path, but it stays hidden and unroutable until operator-pinned WalletConnect config and a local Reown/AppKit adapter hash are present. Ledger is not visible until implemented, and UniSat is not advertised from hosted Browser environments where the extension cannot exist. `wallet-provider` owns proof bindings, approvals, receipts, and audit; connector capsules own only wallet-specific UX and handoff. Connector account and request lists are scoped to the active connector capsule.
- Alignment gates now block ordinary app/viewer/content capsules from referencing raw wallet, chain, node, RPC, WalletConnect, MetaMask, or blockchain-provider authority. System remains the runtime-owned account-policy and diagnostics surface; wallet accounts and wallet approval review belong to Wallet/Inbox.

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
- `scripts/protected-content-provider-contract-smoke.sh` — protected-content provider boundary proof over the real DRM/rights/key/decrypt JSON line protocols.
- `cargo test -p elastos-server test_wallet_approval_journey_creates_request_reviews_in_inbox_and_signs -- --nocapture` — System -> Home badge -> Inbox -> wallet-provider managed-signature approval journey with signed audit.

## Home-branch reality

- `home` remains the internal capsule ID; the visible product surface is Home.
- Home is passkey-fronted by default. Wallet proofs are linked to an existing passkey-backed runtime principal and do not mint Home sessions by themselves.
- The current source proof is a runtime-owned Home browser-hosted adapter:
  - `capsules/home/capsule.json` now declares a WASM capsule, not a microVM
  - `capsules/home/browser/` serves Home assets
  - `/api/apps/home/summary` exposes a signed-in Home authority view or a standard unsigned desktop snapshot, plus an explicit app/object catalog
  - `/api/apps/system/summary` is the System summary and requires a System launch token
  - `/api/apps/home/launch` validates Home launch targets and mints app-scoped launch tokens
  - Home attaches browser-capable apps through same-origin iframes
- `system` is the canonical first-party app ID for the visible System surface; there is no alternate System route or package identity.
- Documents publish/unpublish is provider-plane only:
  - Documents and Library use `/api/provider/documents/...` with Home/app-scoped authority
  - the gateway injects the signed Home launch-token principal into provider calls, and the provider rejects cross-principal document access
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
- Browser architecture is coherent enough to preserve, but the Browser/audio objective is not complete. `scripts/browser-objective-audit.mjs` currently passes best-path, no-fallback, and planning checks, but fails product audio proof and hash-bound manual UX evidence. `scripts/browser-provider-decision-report.mjs` summarizes supplied `hosted_bakeoff` and `native_preflight` artifacts with explicit rejected-artifact blockers; when supplied artifacts are accepted, it clears unrelated live-host blockers and routes to preserving the accepted artifacts. `scripts/browser-provider-runbook.mjs` accepts the same hosted/native/manual proof artifacts so operator guidance is generated from the actual evidence instead of stale live-status prose.
- The current live gateway now exposes the Browser Session Manager receipt `elastos.browser.session-capacity/v1`; public Home-token smokes have proved two independent Browser pages, heartbeat hold, page-scoped Selkies control, `direct_network=false`, clean close, and zero remaining principal sessions. This is a bounded Browser lifecycle proof, not a normal-browser-equivalent product claim.
- The live Runtime Browser can open the known `ela.city` protected-content route through `scripts/browser-ela-city-protected-content-open-smoke.sh` and cleanly release the session. A funded live purchase/playback path has also succeeded for the current 0.3.0 branch, including operator proof transaction `0x7a69f2269e283268abf32f6129098e29ba5f0972d12eb7814ce10e4c64058cc5` on ESC. This is release evidence for the current protected-content user journey, not proof that production dDRM, dKMS, decrypt/render providers, or arbitrary protected-content assets are complete.
- Docker/Selkies is only `managed_baseline_not_final_product`. The old always-on Selkies service was single-session; active pages are a serialization blocker in that model, so it has been disabled on the live host. The live path now starts a per-launch target and routes page operations through the launch result's page-scoped control socket. Do not keep tuning Selkies as product work unless a measured provider gate closes an explicit blocker. On 2026-05-13, after closing the stale active page, `scripts/browser-hosted-provider-bakeoff.sh --candidate selkies --adapter-config /tmp/elastos-browser-selkies-live/browser-engine-adapter.json --cdp-endpoint http://127.0.0.1:39593 --artifact-out /tmp/elastos-browser-bakeoff/selkies-hosted-bakeoff.json` passed the hosted candidate gate: product compositor, audio track, video track, datachannel input, 10s media hold, 1280x720+ quality floor, Runtime/provider navigation, Runtime-mediated wallet bridge, Glide connect, and `direct_network=false`. It still failed product acceptance because the fixed YouTube stress loaded and decoded some media bytes but stayed paused at 0.933s. A patched control-bridge bake-off at `/tmp/elastos-browser-bakeoff/selkies-patched-hosted-bakeoff.json` then made datachannel display coordinates explicit and exercised click plus keyboard activation; candidate gates still passed, YouTube advanced to 2.750s with audio/video bytes decoded, but still ended paused before stable playback. Stable arbitrary YouTube playback and manual UX evidence remain open.
- This host is not a product native-browser proof target because it lacks a real host compositor/display, host audio service, and working network namespace support for the native preflight. Prove native Chromium/CEF on a real launcher/desktop/Jetson target instead.
- Kasm Workspaces, BrowserBox, or KasmVNC cannot replace Selkies until an operator provisions a durable candidate control socket plus the required credentials/license; generated placeholder configs report `operator_control_socket not provisioned` instead of exposing temporary socket paths. A candidate still must pass `scripts/browser-hosted-provider-bakeoff.sh` plus hash-bound manual UX evidence before acceptance.
- System currently shows Account policy, Appearance, Recovery Kit controls, and Advanced runtime/network diagnostics; it does not yet expose the fuller `elastos://` provider/object/capability discovery contract.
- Protected-content providers are contract-testable and fail closed, but production dDRM, dKMS, and decrypt/render backends are not wired yet.
- Chain-provider node-lifecycle status is persistent and typed. Start/stop/restart are available only for loopback nodes with an explicit operator-approved supervisor config; remote/public RPC networks remain status-only.
- Protected principal roots now encrypt Documents working copies, Home browser state, viewer/content storage, and in-runtime capsule-kernel `Users/self` read/write calls through the runtime/provider plane; Recovery Kit downloads can be password-packaged; verified Recovery Kit import can reassign an orphaned root to the active passkey with a reissued session and signed audit.
- Guest confidentiality is materially stronger for protected roots, but public guest hosting still needs installed-path recovery proof, WebAuthn PRF/DID-backed protectors, and broader provider adoption before claiming full operator-private storage.
- Shell/supervisor microVM launches now have first-party launch plumbing to pass a verified launch-grant principal into `BridgeContext`, while provider-role launches remain outside user scope. Attached/native CLI capsule storage still needs a protected principal bridge before history/state persistence is re-enabled for `Users/self`-style current-user aliases.
- The next target-lane proof is: run one manual installed `Home -> System -> app/object -> Home` acceptance loop and decide the first non-browser attach contract.
- The branch should keep deleting Home-side donor and KVM assumptions instead of layering extra compatibility branches around them.
- The default Home path must remain a KVM-independent browser-hosted adapter so macOS and Windows stay in scope without pretending to offer Linux parity.

## Support boundary

- Linux is the truthful full-runtime baseline (x86_64 and aarch64).
- macOS is not yet a truthful full runtime target on this branch.
- The Home direction is being redirected so the default Home path does not depend on KVM or donor backend semantics; that is the condition for macOS to become a first-class front-door target later without faking Linux parity.
- Home is the intended front door but not fully boring on every target machine yet.
