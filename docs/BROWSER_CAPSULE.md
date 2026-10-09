# Browser Capsule Architecture

> Architecture target, not current shipped behavior. For current proof level see
> [GitHub issues](https://github.com/Elacity/elastos-runtime/issues).

The shared wire definitions and version rules are in
[Browser contract](BROWSER_PROTOCOL.md). Device roles, resource requirements and
qualification gates are in [Browser support](BROWSER_SUPPORT.md).

## Capsule delivery and service selection

Browser uses the common [Digital Capsule model](CAPSULE_MODEL.md). Its entry
point and UI can remain one app capsule. Engine and Exit implementations are
provider capsules. Each installable package has a complete signed artifact
closure, including its declared dependencies and compatible platform payloads.
Native Engine helpers and the VM image belong to that verified closure; Runtime
owns admission onto the supported host substrate. A library may be a verified
member of a package without becoming another independently managed capsule.

The Browser entry declares required interfaces and dependencies. Runtime resolves
them through the trusted catalog to exact artifact identities, verifies and
admits locally needed packages, and binds approved running services. Dependency
names alone do not identify acceptable bytes. Service selection and mutable
profiles stay outside the shared immutable Browser artifact.

Admission also defines update and removal. An active instance retains its exact
verified package and dependencies. A replacement activates atomically. Removal
waits for consumers to release ownership; a failed close retains that ownership
for reconciliation. These rules belong to the common capsule path.

[Content availability](CONTENT_AVAILABILITY.md) owns retention and retrieval.
Multiple nodes can retain the same capsule CID. Their availability receipts are
separate from offers to execute an Engine or provide Exit service. A retained
Engine package remains useful while its original execution host is offline, but
another host can run it only if that host has a compatible substrate and grants
the required authority. Pinning bytes does not promise running capacity.

For local execution, the selected Runtime acquires and admits the Engine closure
on demand. For remote execution, the provider Runtime owns that preparation;
the consumer acquires only its local dependencies and attaches to the approved
service. Both paths use the same Browser entry and Runtime contracts. Content
and availability providers select the byte source; Carrier carries off-box
traffic. Browser does not implement its own downloader or call IPFS directly.

The installer supplies a small verified bootstrap set with enough Content and
Carrier support to acquire other capsules, plus trusted catalog roots and an
upgrade path. The installer and Runtime distribution paths must agree on artifact
identity and admission. They must not require developer source-home files.

Delivery proof includes fresh acquisition from each declared availability node
with the other source offline, verified reuse, interrupted acquisition and
tamper rejection. Execution-service unavailability is tested separately. Browser
proof then distinguishes local Engine admission from remote service use without a local
Engine image. Runtime preserves user state and reports unavailable content or
services as separate outcomes. This section defines the target contract; current
installation and replication evidence belongs in the owning GitHub issue.

## Decision

ElastOS needs a real browser capsule, but the first production target should not
be a full browser engine compiled to WASM.

The correct first target is a **Browser Engine Adapter**:

```text
Browser UI capsule
  -> Browser Engine Adapter
  -> Runtime Net Provider
  -> Carrier/Exit Provider
  -> selected network
```

The product rule is **no fallbacks**. A Browser launch selects exactly one
declared display mode, and failure to start that mode fails closed. Screenshot
or image-frame polling is not a Browser display mode. Proof tooling may collect
diagnostics out-of-band, but it cannot masquerade as the selected display
surface.

The current `browser` capsule is a Runtime Browser proof. It can declare wallet
and network capability intent, request `elastos://net/*`, open as an ElastOS Home
window, and render public HTTPS pages through the Browser Engine Adapter
when the operator configures a public-web Exit backend. The normal hosted path
uses a Selkies/GStreamer WebRTC product-compositor baseline with Runtime-scoped
signaling, audio/video tracks, datachannel input, Browser command routing, and
`direct_network=false`. Playwright/CDP collectors are test instrumentation only:
they do not define a Browser display route and cannot replace the selected
display mode.
The Browser UI reports the engine's actual URL/title after launch and input,
uses the Home window's current viewport, supports resize, wheel, click, paste,
basic keyboard input, address navigation, back, forward, reload, and keeps
Browser UI history aligned with engine navigation.
Exit backends may use narrow host allowlists or `"*"` for public web, plus
explicit scheme/protocol and port allowlists. Private IP/LAN targets remain
blocked by default. This baseline is still not an accepted product browser:
stable arbitrary YouTube/audio behavior, subjective UX quality, per-user or
per-page hosted session isolation, native scrolling/text-selection quality,
WalletConnect wallet mode, and cross-platform support still require the native
or hosted-provider gates below.

## Core Invariant

The browser is a dangerous capsule, but still a capsule:

- no ambient host internet
- no raw sockets
- no raw DNS
- no raw wallet, chain, node, IPFS, file, or host API
- no Runtime API exposed directly to web pages
- no browser extension as the foundation
- no app-level WalletConnect or MetaMask SDK authority

All off-box network effects must use the runtime capability plane:

```text
web page
  -> browser engine sandbox
  -> browser capsule bridge
  -> runtime capability
  -> elastos://net/*
  -> Carrier/Exit provider
```

Same-machine adapter plumbing may use IPC, vsock, stdio, or loopback between the
host engine and the runtime. That plumbing is not the capsule contract. From the
capsule and web page point of view, there is still no ambient internet; there is
only a runtime-mediated network stream. Browser launch uses the same resource
discipline internally: `elastos://net/stream` validates the destination,
`elastos://exit/open_stream` performs the approved egress handoff, and
`elastos://browser-engine/launch` binds the stream to the selected engine
adapter.

## Browser / Net / Exit ABI

The browser capsule should target these contracts:

```text
elastos://net/resolve
elastos://net/connect
elastos://net/stream
elastos://net/http
```

`elastos://net/http` is optional and constrained. General browsing should prefer
a stream relay where the browser engine owns TLS. HTTP-fetch proxying is useful
for controlled content, caching, diagnostics, or compatibility, but it makes the
runtime or exit provider a content proxy and should not be the default browser
path.

Wallet account, proof, approval, signing, Recovery, and transaction operations
are not capsule-visible provider resources. The generic Wallet provider surface
contains only read-only `elastos://wallet/meta/status`. Browser product routes
validate signed launch-token v4 authority and invoke typed
`WalletProviderOperationV2` requests through the private Runtime-local Wallet
Bus 2.3 adapter. Chain reads and transaction effects remain typed
`chain-provider` operations.

Exit providers should be pluggable:

```text
local-runtime-exit
remote-carrier-exit
privacy-exit
paid-exit
enterprise-policy-exit
```

The default policy must block LAN/private IP ranges unless a user or operator
explicitly grants them.

Remote Carrier exits are configured as `remote_carrier_exits` on the internal
`exit-provider`. A remote exit must name the remote Runtime DID/service,
private `connect_ticket`, stable operator `grant_id`, allowlisted principals,
allowlisted public hosts/schemes/ports, a global active-stream quota, and
optionally a per-principal active-stream quota plus `expires_at` grant lifetime. Expired
grants remain visible as expired provider status for diagnostics, but cannot be
discovered, quoted, or opened. `discover_remote_carrier_exits` returns a typed,
principal-scoped `elastos.exit.remote-carrier.discovery/v1` list of
permitted Carrier stream exits with the grant id, policy/accounting, and without
leaking the allowed-principal list. `quote` and `open_stream` return typed
`elastos.exit.remote-carrier.quote/v1` /
`elastos.exit.remote-carrier-session/v1` receipts with
`byte_transport=carrier_stream` and the grant id; they do not expose Unix
sockets, host paths, TCP sockets, private route tickets, or direct network
authority to the Browser capsule.

## Browser Engine Adapter ABI

The Browser Engine Adapter is an internal provider boundary:

```text
browser capsule
  -> Runtime summary / launch grant
  -> /api/apps/browser/open
  -> elastos://net/stream
  -> Runtime-owned Exit handoff
  -> Browser Engine Adapter
  -> attached elastos.exit.stream-session/v1 byte transport
```

The first `browser-engine-adapter` implementation is deliberately fail-closed.
It defines `status`, `launch`, `attach_stream`, `page_status`, `diagnostics`,
`input`, `webrtc_signal`, and `close_page`. It only accepts a stream-session
receipt when `byte_transport` is `adapter_ipc`; `not_attached` receipts are rejected.
Native adapter kinds such as CEF or Chromium-in-microVM launch only through an
operator-approved supervisor command. Runtime sends the supervisor a typed
`elastos.browser.engine.launch-request/v1` payload in
`ELASTOS_BROWSER_ENGINE_REQUEST`; the supervisor must return
`elastos.browser.engine.supervisor-result/v1` and must report
`runtime_net_only`, `direct_network=false`, and `wallet_injection=false`.
Without that proof, native launch fails closed.
Every configured adapter must also declare supported `display_modes`.
The product adapter declares `webrtc_remote_display`. Retained test adapters can
declare `native_surface`; each mode has an explicit contract and its own proof.

The product Engine executes Chromium and Selkies inside a per-launch Linux VM.
Host-native Chromium namespace launchers and Playwright collectors are test
instrumentation. Normal release and setup supply the VM Engine closure. The
adapter's `test-engine` build feature enables the retained proof engines only
for isolated tests; production configuration accepts `chromium_microvm`.

## Cross-Platform VM Browser Target

The Browser has one Engine: Chromium, Selkies and the Runtime network wrapper
inside a per-launch Linux guest. One immutable ARM64 image set supplies both
Apple Silicon Mac through Apple Virtualization and Linux ARM64 through
crosvm/KVM. Hosts that cannot execute that guest, including Linux x86-64, use an
explicitly approved remote Engine through the same Runtime contracts. Each host
adapter has its own qualification; the guest bytes and recipe input identity are
shared.

The product path is:

```text
Browser UI capsule
  -> /api/apps/browser/open
  -> Runtime capability + Exit stream receipt
  -> Browser Engine Adapter
  -> browser-vm-product / chromium_microvm supervisor
  -> per-launch Browser VM target
```

`scripts/browser-vm-engine-supervisor.mjs` is the substrate-neutral contract
point for that target. It accepts only `chromium_microvm`,
`webrtc_remote_display`, `runtime_net_only`, `direct_network=false`, and
`wallet_injection=false`. It delegates to a VM-resident Browser control service
at `ELASTOS_BROWSER_VM_CONTROL_SOCKET` and otherwise fails closed with
`scripts/browser-vm-engine-preflight.sh`-compatible diagnostics.

VM Browser display is also a Runtime boundary. A VM target must report
`media_transport: "runtime_relay"` in its display session; direct VM/LAN ICE
candidates are not an acceptable product path. The host substrate launcher is
responsible for bridging VM-local display/control traffic to Runtime-scoped
Browser routes.

The same Browser app and Browser Engine Adapter config shape is used on Linux
and macOS. The substrate below the VM supervisor differs:

- Linux ARM64 product substrate: crosvm/KVM with the admitted crosvm helper, the image-set kernel, and a
  Browser VM rootfs containing Chromium, the Browser control service, and the
  Runtime/Exit bridge.
- macOS product substrate: Apple Virtualization.framework, reusing the proven
  `elastos-vz` lessons from `sash/local-test-v030`, with the same VM-resident
  Browser control service contract.

The existing Selkies/Docker and Apple-container seams remain proof or staging
adapters only. They are useful for testing the Browser Engine Adapter, WebRTC,
wallet bridge, and profile behavior, but they are not the final ElastOS Browser
isolation boundary.

The VM guest boundary is now a separate contract: the rootfs must include
`browser-vm-runtime-relay`, which exposes a VM-local Unix Exit socket to
`browser-native-proxy-engine` and forwards those bytes to a host Runtime bridge
over an explicit VM transport. That relay is still a target-image/substrate
piece; it is not a Browser UI feature and not a direct-network escape hatch.
`scripts/build/stage-browser-vm-target.sh` assembles that guest contract from
explicit binaries and runs `scripts/browser-vm-target-preflight.sh`; it does
not install Chromium or claim a bootable target without real guest artifacts.
Gateway hosts without compatible virtualization select an approved remote Engine
through Runtime's Services controls. Carrier transports the selected Engine's
control and media below the same principal and lifecycle authority.

The test-only byte-transport helper lives at `elastos/tools/browser-stream-bridge`. It
reads `ELASTOS_BROWSER_STREAM_BRIDGE_CONFIG`, binds the private
`adapter_ipc_path` Unix socket for the engine side, connects to a Runtime-owned
`runtime_stream_path` Unix socket, and forwards bytes between them. It is not an
exit provider and contains no TCP, DNS, HTTP, wallet, chain, or storage access.
The supervisor can start this helper before launching the engine when its
operator config declares a `stream_bridge` program and the private `adapter_ipc`
descriptor includes `runtime_stream_path`. Gateway now allocates that private
`runtime_stream_path` in the short host temp directory
`elastos-browser-streams/` for each validated
`elastos.exit.stream-session/v1` receipt. The short path is intentional because
Unix domain socket paths have tight platform limits. Gateway binds a one-shot
fail-closed Unix listener at that path and calls the Browser Engine Adapter. If
the Exit backend also returns a private `elastos.exit.relay-ipc/v1` Unix socket
descriptor, Gateway relays bytes between the Runtime socket and that Exit
socket; otherwise it accepts and closes fail-closed. `adapter_ipc` and
`relay_ipc` are stripped from Browser UI responses. Product Browser engines
receive only `adapter_ipc.runtime_stream_path`; the runtime stream client sends
the typed Exit relay-open handshake, and Gateway forwards only that bounded
first line to the private `relay_ipc` socket before relaying bytes.

The guest uses `elastos/tools/browser-native-proxy-engine` as its network
wrapper. Its loopback proxy forwards Chromium destination streams through the
guest Runtime relay and selected Exit. The wrapper source remains a guest build
input. The retired host `browser-engine-supervisor` namespace path is absent
from normal delivery.

The retained native/hosted and Playwright smokes are isolated test tools. Their
receipts prove the tested contracts and identify their test engine build; they
provide no product-engine acceptance. The production media path is Selkies in
the Linux guest, bound to Runtime signaling, audio/video and input routes.

Runtime owns Browser helper lifetime through a pipe with one writer. Local Exit,
guest network proxy and stream bridge helpers receive the reader on stdin with
`ELASTOS_BROWSER_LOCAL_EXIT_PARENT_EOF=1`. EOF starts their cleanup after Runtime
stop or crash. VM launchers forward the verified reader identified by
`ELASTOS_UPDATE_PARENT_PIPE` across short-lived launch commands. The shared
`elastos-common::process_lifetime` module verifies that descriptor's identity.
Helpers remove only the socket inode they bound. Runtime retains Engine and
profile ownership until exact cleanup proves that native writers have stopped.

### Display Session ABI

The real Browser surface negotiates an explicit display session:

```json
{
  "schema": "elastos.browser.display-session/v1",
  "session_id": "display:...",
  "mode": "webrtc_remote_display | native_surface",
  "input": "datachannel | native_ipc | runtime_route",
  "offerer": "browser | engine",
  "initial_offer": {
    "schema": "elastos.browser.webrtc-offer/v1",
    "type": "offer",
    "sdp": "..."
  },
  "display_backend": "native_compositor_webrtc",
  "backend_class": "product_compositor",
  "audio": true,
  "video": true,
  "network_mode": "runtime_net_only",
  "direct_network": false
}
```

The production VM Engine returns `webrtc_remote_display` for local and remote
viewers. Retained native adapter experiments use `native_surface` in test-only builds.
Each Browser Engine Adapter must declare its supported display modes in operator
config. Runtime passes exactly the requested mode to the adapter, and the
adapter must return the same mode in `display_session`; mismatches fail closed.
The adapter must also return explicit `view.width` / `view.height` and
`display_session.width` / `display_session.height` geometry. WebRTC product
compositor streams may use a different encoded stream size only when the stream
and Runtime view preserve the same aspect ratio, so fixed-stream baselines
cannot silently stretch input coordinates or page pixels.
For WebRTC sessions, audio is allowed only when the adapter identifies a real
product compositor backend such as `display_backend=native_compositor_webrtc`
and `backend_class=product_compositor`. A proof backend such as
`cdp_screencast_i420` / `proof_surface` must advertise `audio: false`.
`offerer` defaults to `browser` for the current proof path. Selkies/GStreamer
uses `offerer=engine` because its host creates the WebRTC offer and the browser
client answers; those sessions must include `initial_offer`.

Required adapter invariants:

- no ambient host internet
- no raw sockets exposed to app capsules
- no browser-engine provider route exposed to ordinary apps
- Browser UI uses the high-level Browser open route, not raw provider routes
- Browser UI fails closed if the selected display session cannot start
- Browser UI does not downgrade from `webrtc_remote_display` or `native_surface`
  to image polling
- Browser UI never receives raw `adapter_ipc` endpoint descriptors
- no raw wallet injection into web pages
- page launch binds principal, URL, stream session, reason, and audit context
- configured adapters must declare `network_mode = runtime_net_only`

The public source-home Browser launch path is WebRTC-only inside the per-launch
Browser VM with Runtime-owned Exit/control/wallet boundaries. Non-WebRTC display
polling is not a source-home product fallback and must not hide WebRTC/Carrier
relay failures.

Runtime signaling is part of the display contract, not an app-visible browser
engine API. For `webrtc_remote_display`, Browser UI creates a local offer,
posts it to the display session's Runtime-scoped `signaling_url`, receives an
`elastos.browser.webrtc-answer/v1`, and then sends ICE candidates separately as
`elastos.browser.webrtc-candidate/v1` plus
`elastos.browser.webrtc-end-of-candidates/v1`. Gateway forwards each message as
the `browser-engine` operation `webrtc_signal` and validates the answer or ack
schema before returning it to the Browser UI. The adapter must declare
`webrtc_remote_display`, and use a
`/api/apps/browser/pages/.../webrtc` signaling URL. Hosted proof adapters
currently return `input=runtime_route` for broad SDP interoperability;
`input=datachannel` remains an optional optimized mode. The display session may
carry operator-approved `ice_servers` for WebRTC traversal, but the Browser UI
does not choose STUN/TURN infrastructure and does not receive wallet objects,
chain RPC, node RPC, raw sockets, or host network authority through this route.

For `offerer=engine`, the flow is inverted for Selkies-style servers:

```text
engine display_session.initial_offer
  -> Browser setRemoteDescription(offer)
  -> Browser createAnswer()
  -> POST /api/apps/browser/pages/:page_id/webrtc { type: "answer", sdp }
  -> Engine returns elastos.browser.webrtc-signal-ack/v1
  -> ICE continues over candidate/end_of_candidates messages
```

Browser UI supports both roles without fallback. The display session declares
the role; the UI follows that one role and fails closed on mismatched signaling.

The test-only hosted sender is implemented in `browser-playwright-engine`: it uses
Playwright Chromium behind a loopback Runtime HTTP CONNECT proxy, and that proxy
opens target TCP streams only through `browser-local-exit`. Normal browsing does
not use Playwright request interception or `route.fulfill`; Chromium owns TLS,
HTTP, cache, cookies, service workers, WebSockets, and page lifecycle. The
current display backend still captures compositor frames with CDP
`Page.startScreencast`, decodes JPEG, converts RGBA to I420, and sends that as a
WebRTC video track. The display session and page status must report
`display_backend=cdp_screencast_i420` and `backend_class=proof_surface` so this
proof cannot be mistaken for the final compositor path. It receives input
through the Runtime-scoped WebRTC data channel when available. This removes the
HTTP image-blob product path, but it is still a hosted proof: it must advertise
`audio: false` until real audio capture is implemented. Gateway/provider
validation rejects `proof_surface` audio claims, so YouTube/audio readiness must
come from a product compositor adapter, not from stretching the CDP proof.
Native/compositor quality remains for the dedicated CEF/microVM,
remote-compositor, or native-surface adapters.

For product-compositor WebRTC sessions, the Browser UI starts the remote video
muted so autoplay can begin, then unlocks audio on the first Browser user
gesture. It reports the unlock state in status/debug metrics so reviewers can
distinguish a missing audio track from muted autoplay policy. A provider sending
audio is necessary but not sufficient if the host browser blocks audible
autoplay.

Media verification uses two gates:

- `scripts/browser-youtube-acceptance-smoke.sh` proves the hosted Runtime proxy
  path can load an accepted YouTube fixture and that Chromium decodes video and
  audio bytes while staying `direct_network=false`.
- `scripts/browser-native-youtube-smoke.sh` proves the native proxy engine path
  can launch a Chrome/Chromium-family browser behind `browser-local-exit` and
  decode video and audio bytes without Playwright launching the browser.

Those gates prove media capability for the selected Exit and fixture. They are
not product audio acceptance by themselves. The Browser objective remains open
until the hosted-provider bake-off or native preflight provides an accepted
machine artifact and hash-bound manual UX evidence. Arbitrary YouTube URLs can
still require a trusted operator profile or approved Exit with better
reputation.

The Playwright daemon is reused only when its status reports the same config
fingerprint and the requested display mode. Stale daemons from an older
diagnostic-only build must be replaced before launch; otherwise a Browser
window could open successfully and then fail when `/webrtc` signaling reaches an
old control surface.

### Hosted Product Control Service

The hosted product adapter is split into two pieces:

```text
browser-engine-adapter
  -> scripts/browser-per-launch-selkies-supervisor.mjs
  -> per-launch Selkies Runtime Exit target
  -> page-scoped Unix control socket
  -> compositor service
```

The live hosted path must not use an always-on shared control socket. The
per-launch supervisor starts `scripts/browser-selkies-runtime-exit-target.sh`
under a unique session directory, waits for that target's control socket, then
delegates the actual page open to the strict hosted-product supervisor. The
Browser Engine Adapter stores `page_id -> control_socket_path` and rejects
status/input/diagnostics/WebRTC operations when no page-scoped control session
exists.

The bundled hosted-product supervisor bridge is intentionally small. It reads
`ELASTOS_BROWSER_ENGINE_REQUEST`, posts the request to the configured
`ELASTOS_BROWSER_HOSTED_PRODUCT_CONTROL_SOCKET`, validates the returned
`product_compositor` display session, and prints the typed supervisor result.
It does not launch Chromium itself, perform WebRTC signaling itself, synthesize
audio/video, or fall back to CDP frames.

The compositor control service behind the socket must implement:

```text
POST /pages
GET  /pages/{page_id}/status
POST /pages/{page_id}/webrtc
POST /pages/{page_id}/input
POST /pages/{page_id}/close
POST /shutdown
```

`POST /pages` receives:

```json
{
  "schema": "elastos.browser.hosted-product.open/v1",
  "launch_request": {
    "schema": "elastos.browser.engine.launch-request/v1",
    "engine": "selkies_gstreamer | hosted_remote_browser",
    "display_mode": "webrtc_remote_display",
    "network_mode": "runtime_net_only",
    "direct_network": false
  },
  "requirements": {
    "backend_class": "product_compositor",
    "video": true
  }
}
```

It must return `elastos.browser.engine.supervisor-result/v1` with
`display_session.backend_class=product_compositor`,
`display_session.video=true`, and a Runtime-scoped
`/api/apps/browser/pages/.../webrtc` signaling URL. Fresh VM artifacts install
and start the PipeWire/Pulse audio stack by default, so product sessions should
normally expose `display_session.audio=true` plus `display_session.audio_offer`.
If an older or constrained image lacks audio dependencies, the Browser VM
product launch must fail closed until the target image is refreshed or rebuilt.
The service must launch or attach to an isolated browser/compositor session and
route browser networking through Runtime Exit policy.

For a Selkies/GStreamer implementation, this service is the translation layer
between the ElastOS Browser ABI and Selkies' native process/signaling model. It
owns the operator-specific details: container/process lifecycle, X11/Wayland,
PulseAudio/PipeWire, encoder settings, ICE/TURN configuration, and CDP or
equivalent browser control for navigation and wallet injection. Those details
must not leak into Browser UI or ordinary capsules.

`scripts/browser-selkies-control-service.mjs` is the first concrete bridge for
that service boundary. It listens on the configured Unix control socket, connects
to an operator-run Selkies WebSocket signaling endpoint, performs Selkies'
`HELLO client` / `SESSION server` handshake, returns the engine-created
audio/video SDP offer as `display_session.initial_offer`, and forwards the
Browser answer/candidates back to Selkies. It also requires a private
loopback-only `browser_control.kind=cdp_http` endpoint so `POST /pages` actually
navigates a controlled browser page before returning a receipt; without this,
the service fails closed instead of streaming an unrelated desktop. It also
validates configured `ice_servers` and copies them into the typed display
session so the Browser client can use explicit STUN/TURN/TURNS policy without
owning raw network authority. `scripts/browser-selkies-control-service-smoke.sh`
proves that translation, CDP navigation, and ICE server propagation work against
fake Selkies/CDP peers. It does not install, launch, or supervise the real
Selkies/GStreamer compositor yet; the operator service must still provide that
process/container, audio device, GPU or software encoder policy, CDP endpoint,
and Runtime Exit wiring.

The public `gst-py-example` Selkies-GStreamer image is useful research material,
but it is not a drop-in ElastOS product adapter. A local check showed its
exposed service speaks a legacy/numeric signaling flow, while this bridge
targets the current Selkies WebRTC service model (`HELLO client` / `SESSION
server`). Product deployment should use a controlled Selkies image/process that
exposes the expected signaling protocol, private CDP browser control, audio
capture, and Runtime Exit proxying.

`scripts/browser-selkies-target-preflight.sh` is the operator gate for such a
target. It does not launch Selkies or Chromium; given an already-running Selkies
WebSocket endpoint, private loopback CDP endpoint and explicit
`--runtime-fetch-proxy-url`, it starts the ElastOS
control bridge and runs the hosted product-display preflight through
`browser-engine-adapter`. Passing this gate means the target can return a typed
`product_compositor` session with declared audio/video and engine-offer signaling,
then exact generation-bound terminal cleanup and adapter Shutdown. The control
service validates the existing Runtime proxy field. Fake signaling proves the
fixture contract; media quality, real Exit routing, direct-network denial and
long-session durability remain target tests. The current-wheel and real-Chromium
wrappers require `ELASTOS_BROWSER_RUNTIME_FETCH_PROXY_URL` before starting their
target work.

Real Selkies targets often protect the signaling endpoint with Basic auth. The
preflight must be given those credentials explicitly with
`--selkies-basic-auth-user` and `--selkies-basic-auth-password`; disabling target
auth just to pass a Browser preflight is not acceptable for hosted deployments.
Operators that need traversal beyond direct host UDP must pass `--ice-server`
one or more times, plus `--ice-username` and `--ice-credential` for TURN. Those
values become `display_session.ice_servers`; they are not an app capability and
must be supplied from operator configuration, not capsule code.

`scripts/browser-selkies-current-wheel-smoke.sh` is a heavier Docker-based
decision gate. It layers the current `selkies` Python wheel onto the research
Selkies image, starts Xvfb/PipeWire/PipeWire-Pulse plus the current Selkies
WebRTC service, then runs `browser-selkies-target-preflight.sh` against it with
authenticated signaling. Passing it proves the current Selkies service can
produce the single engine-offer audio/video WebRTC session required by the
Browser ABI. It still uses a fake CDP endpoint, so it is a signaling/media
contract proof, not the final real-Chromium/Runtime-Exit deployment proof.

`scripts/browser-selkies-real-chromium-smoke.sh` extends that gate by running a
real Chromium binary on the same Xvfb display, exposing a private loopback CDP
endpoint, and then running `browser-selkies-target-preflight.sh` against that
real browser control endpoint. This proves the hosted product path can control a
real browser page while Selkies returns the required audio/video
`product_compositor` display session. The smoke blocks normal DNS in Chromium so
it does not become a raw internet path; the remaining product proof is wiring
Chromium through Runtime Exit instead of that denial-only network posture.

`scripts/browser-selkies-runtime-exit-smoke.sh` is the current best hosted
Browser proof. It starts `browser-local-exit`, launches real Chromium inside the
Selkies target through `browser-native-proxy-engine`, verifies a real
`https://example.com/` page load through Runtime Exit, and then runs the same
authenticated Selkies product-compositor preflight. This is the first hosted
path that has all required foundations in one test: real browser control,
Runtime-mediated networking, no raw DNS path, engine-offer WebRTC, video, and
browser audio. Its verification path now uses
`scripts/browser-hosted-product-webrtc-smoke.sh`, which answers the
engine-created Selkies offer with a real WebRTC client and requires audio track,
video track, datachannel input, connected ICE, and the declared
`input_protocol=selkies_v1` contract. Pass `--hold-ms <milliseconds>` to keep
the negotiated session alive after the initial proof and fail if ICE/WebRTC
disconnects during that hold. Silent pages prove an audio track, not audio
payload bytes; decoded audio bytes are required only with `--require-media` on a
controlled media fixture. Browser UI translates pointer, wheel, key, and
text input events to Selkies datachannel commands only for this product adapter;
generic Browser capsules still speak the Runtime Browser ABI, not raw host input
APIs. Browser commands such as address navigation, back, forward, and reload
remain Runtime/provider input calls; the Selkies control service applies them
over private CDP and returns the current page state with `direct_network=false`.
The product raster is fixed at 1920x1080 with DPR 1. Home resizes only the
viewer using `object-fit: contain`, and maps input through decoded-video
coordinates. Home CSS dimensions never resize the guest compositor or page
raster.
Browser status polling consumes that page state, including `can_go_back` and
`can_go_forward`, so visible navigation follows Chromium's actual history
instead of a UI-side shadow model. Address-bar submits in an active hosted
remote-display session use `browser_command: navigate` against the existing
engine page; they do not close and reopen the compositor session.
The Browser UI pauses page-status polling while the user is actively editing the
address bar, so engine status cannot overwrite a typed destination or steal the
navigation flow; submit and Escape explicitly clear that edit lock.
Debug mode (`?debug=1` or `?metrics=1`) also exposes WebRTC receive stats and
rendered video-element counters such as decoded frames, dropped frames, received
bytes, packet loss, and RTT. The hosted product WebRTC smoke emits the same
quality stats so product tuning has measurable evidence instead of subjective
"slow" reports. A
controlled MP4 media gate now proves decoded video and audio bytes through the
hosted product compositor. Remaining product hardening is long-session quality,
Glide wallet flow in this hosted product mode, per-user/per-page session
isolation, and TURN/operator configuration. YouTube remains a stress gate rather
than the canonical audio-pipeline proof because direct embeds can fail without
valid referrer identity and watch pages can decode initial media but pause due
site/profile/ad behavior; arbitrary YouTube URLs can still require an approved
Exit route or trusted operator profile.

`scripts/browser-selkies-runtime-exit-target.sh` is the canonical target
launcher used by the per-launch supervisor. It starts the local Exit relay,
starts the Selkies/Chromium Docker target, starts the ElastOS Selkies control
service, writes target diagnostics, and stays in the foreground until `/shutdown`
or process termination cleans up the target. The smoke above is now only a `--verify
--cleanup-after-verify` wrapper around this launcher, so the tested path and the
launched path do not drift. The launcher defaults to a product H.264
profile (`x264enc`, fixed 1920x1080 stream/page raster at DPR 1, 30 fps,
16 Mbps) and exposes `--selkies-encoder`, `--selkies-framerate`,
`--selkies-video-bitrate`, and `--selkies-h264-crf` for codec tuning. Raster
size and resolution mode are not operator configuration.
Helper Rust binaries are built into a shared
`${XDG_CACHE_HOME:-$HOME/.cache}/elastos/browser-selkies-cargo-target` cache by
default, not into each session directory. Per-launch Browser startup must not
rebuild helper binaries for every Browser window.
Chromium state is no longer held in the target container's `/tmp`. The
per-launch supervisor derives a persistent profile directory from the signed
principal and passes it to the target via `--profile-dir`; the target mounts
that directory at `/var/lib/elastos-browser-profile` and locks
`.elastos-profile.lock` for the lifetime of the session. A second concurrent
session for the same profile fails explicitly instead of racing Chromium's
single-writer user-data directory. This is the current hosted-provider bridge
toward principal-owned Browser state; the final protected object root remains
`localhost://Users/<principal>/BrowserProfiles/...` or an equivalent encrypted
provider-owned root.
The invariant is one fixed 1920x1080 compositor, capture surface, and page
raster at DPR 1. The earlier dynamic CDP viewport path is retired because a
smaller emulated viewport left blank right/bottom regions inside the encoded
frame. Browser window resize changes only the contained viewer; decoded frame
progress and decoded-video coordinate mapping must remain stable.
`browser-engine-adapter` accepts supervisor timeouts up to 300 seconds because
per-launch hosted session supervisors may need more than the old 30-second proof
limit. A configured adapter-level `control_socket_path` is no longer sufficient
for product Browser operations; page operations must use the control socket
returned by the launch result for that specific page.

Each Browser profile is owned by one VM session so the principal-owned profile
disk is mounted writable in exactly one place. That VM can host multiple Browser page
sessions through `/pages` and `/pages/:id/*`; opening a second Browser page
must not recycle or kill the first page. Operators can inspect a launched target
state through `GET /status` on the Selkies control socket. It returns
`elastos.browser.selkies-control.status/v1` with `active_pages`, `page_ids`,
`single_session`, `single_vm_session=true`, and `direct_network=false`; this is
an observability endpoint, not an app-visible Browser capability.

Failed opens must not poison the single-session target. If the Selkies
controller WebSocket is created but launch later fails, the control bridge
closes that partial page before returning the error, records the close, and
applies a short bounded cooldown before the next open so Selkies can re-register
its server peer. The smoke intentionally fails the first post-CDP Selkies
session, verifies the failed WebSocket is closed, verifies `/status` still
reports zero active pages, and then proves the next open succeeds.

For development, the launcher can assemble a disposable Selkies target from the
upstream Selkies build image and `gst-py-example` base image. Production should
not install OS/Python dependencies at session start. Build the controlled target
image first:

```bash
scripts/browser-selkies-operator-image-build.sh
scripts/browser-selkies-runtime-exit-target.sh \
  --out-dir "$ELASTOS_DATA_DIR/browser-selkies" \
  --target-image elastos/browser-selkies-runtime-target:dev \
  --ice-server stun:stun.l.google.com:19302 \
  --selkies-encoder x264enc \
  --selkies-framerate 60 \
  --selkies-video-bitrate 16
```

The Dockerfile lives at `deploy/browser-selkies-runtime-target/Dockerfile`. It
pins the dependency shape for the current Selkies wheel, X11/PipeWire runtime,
Chromium library requirements, Selkies runtime helpers such as `xclip`, and a
known-good PixelFlux screen-capture version for the Ubuntu 24.04 base. The build
asserts the PixelFlux capture module imports so a `vaMapBuffer2`/libva mismatch
cannot ship silently. The actual Chromium binary and Runtime proxy binary remain
mounted by the operator launcher. This keeps the Browser ABI stable and avoids
a slow, mutable bootstrap path in production.

The Selkies control bridge must also preserve the native WebSocket protocol. In
particular, client-to-server pong frames are masked and preserve the server ping
payload; otherwise Selkies can establish media and later tear it down when the
signaling keepalive fails. `scripts/browser-selkies-control-service-smoke.sh`
contains a regression for this ping/pong behavior, and
`scripts/browser-hosted-product-webrtc-smoke.sh --hold-ms 60000 --require-media`
is the live long-session gate for controlled media/audio stability. That smoke
also enforces a quality floor for controlled media runs: rendered video must be
at least 1280x720 by default, audio/video bytes and decoded frames must be
present, reported FPS must stay at or above 24 when available, and dropped
frames must stay under the configured ratio.

The old `scripts/system/elastos-browser-selkies.service` wrapper is an operator
diagnostic target only. It must not be used as the live Browser product path
because it creates an always-on shared hosted session. Live Runtime config
should point `browser-engine-adapter` at
`scripts/browser-per-launch-selkies-supervisor.mjs` and let each Browser launch
create and clean up its own target.
`scripts/browser-per-launch-selkies-supervisor-smoke.sh` is the regression gate
for this invariant. It starts two hosted Browser launches concurrently under a
service-style `HOME`, requires different page IDs, different page-scoped control
sockets, different isolation directories, and different persistent profile
directories with non-reversible `profile-<sha256>` names for different
principals, verifies each control socket owns exactly its returned page, then
shuts both targets down through `/shutdown`.

Hosted operators should set `ELASTOS_BROWSER_SELKIES_BROWSER_PROGRAM` to an
explicit executable Chromium/Chrome path. Do not rely on Playwright's
HOME-derived browser cache discovery under systemd, because the gateway service
may use a dedicated `HOME` that does not contain the developer user's browser
cache. Hosted operators may override the profile root with
`ELASTOS_BROWSER_PROFILE_ROOT`; otherwise installed source-home uses the runtime
data root's `browser-profiles` directory, and source runs derive the root from
`XDG_DATA_HOME`/`HOME`.

## Source-Home VM and Remote Browser Config

`scripts/setup-source-home.sh` writes explicit Browser provider config on both
Linux and Mac through `scripts/browser-source-home-config.mjs`; source-home
must not rely on the old ambient `contract_proof` default.

The source-home Browser config is VM-only. It writes adapter id
`browser-vm-product`, kind `chromium_microvm`, display mode preference
`["webrtc_remote_display"]`, and supervisor
`${DATA_DIR}/bin/browser-vm-engine-supervisor`. It also writes a stable
`ELASTOS_BROWSER_VM_CONTROL_SOCKET` so the supervisor can auto-start the VM
control service instead of hanging on an implicit or missing socket.

It does not expose a hosted-proof, host-browser, or Apple-container Browser
engine selector. Standalone hosted/Selkies scripts may remain in the repo as
protocol or operator smokes, but `scripts/setup-source-home.sh` does not install
or select them as source-home Browser runtime dependencies.

On macOS, product parity is the same VM contract through
`browser-vz-engine-supervisor`: same rootfs target, same Runtime-owned Exit
relay, same guest control bridge, and same WebRTC/datachannel display/input
surface. Source-home config defaults the VM control launcher to
`${DATA_DIR}/bin/browser-vz-engine-supervisor`. If the VM artifacts or VZ
supervisor are missing, Browser must fail closed instead of falling back to a
host or container browser.
Mac source-home Browser config loads Runtime-owned TURN credentials from
`$HOME/runtime-turn/turn-credentials.env` or
`${DATA_DIR}/runtime-turn/turn-credentials.env` when explicit operator ICE
environment variables are not already set. The generated adapter must carry the
same relay-only ICE and media relay values that the running Runtime TURN service
uses.

## TLS Model

Preferred:

```text
Browser engine owns TLS.
Exit provider relays encrypted TCP/QUIC streams.
```

This means an exit provider may see destination metadata, timing, size, and
possibly SNI unless ECH is used, but it does not see page contents.

Avoid as the general browser model:

```text
Exit provider fetches HTTPS and returns HTML/assets.
```

That can be useful for narrow provider functions, but it turns the provider into
a trusted content intermediary and weakens the browser security boundary.

## Wallet Bridge

For dapps, the browser engine adapter may expose an EIP-1193-compatible provider
to web pages, but Wallet authority must terminate in Runtime's private typed
Wallet Bus adapter.

```text
web page window.ethereum request
  -> Browser Engine Adapter origin check
  -> verified Browser launch authority
  -> typed WalletProviderOperationV2 or chain-provider operation
  -> Wallet/Inbox approval only for signing or transaction effects
  -> selected signer or chain provider
  -> signed audit/result or read receipt
```

The web page must never receive private keys, raw wallet RPC, raw node RPC,
connector SDK objects, Runtime tokens, or provider credentials. The bridge must
bind origin, account, chain, principal, session, browser profile, action, nonce,
expiry, and audit event.

Chain selection is Runtime-owned. The Browser Engine Adapter must not choose a
chain from the website host, URL, connector brand, or dapp-specific rule. Runtime
builds the wallet bridge from the signed Home principal's linked Wallet provider
state: `browser_connect` default when present, then the Wallet
`transaction_intent` default, then the first linked EVM account for that
principal. If the principal has no EVM account, `eth_requestAccounts` fails
closed with a no-account error instead of returning an empty connected result.
In the VM Browser path, the wallet bridge does not depend on page-visible HTTP
to the Runtime gateway: dapp CSP can block it, and guest loopback is not the
host Runtime. The Browser control service installs a CDP Runtime binding for
typed wallet bridge requests; only the control service sees the Home token and
Runtime wallet endpoint URLs, while signing and transaction effects stay behind
Wallet/Inbox approval.

Browser profile state must follow the same authority boundary. Cookies,
localStorage, IndexedDB, service workers, bookmarks, and history are principal
profile state and should be rooted under
`localhost://Users/<principal>/BrowserProfiles/<profile>/...`, not in a shared
container profile. Principal isolation alone is not a
claim that Chromium cookies or localStorage are protected principal-root objects
or Recovery Kit state.

For the VM Browser path, the concrete storage boundary is a Runtime-owned
profile disk. Runtime derives a non-reversible SHA-256 profile key from the
signed Browser launch principal, attaches `<profile-key>.ext4` as the VM data
disk, and requires the guest to mount it before Chromium starts. The Browser
capsule does not receive a host path, disk path, cookie jar, or reset handle. It
can only request the
high-level reset route, `POST /api/apps/browser/profile/reset`, with its Browser
launch token. Runtime refuses reset while that principal has live Browser pages
and deletes only the matching profile disk.

Profile storage boundary: the Browser VM profile disk is
principal-owned and reset-scoped, but it is not yet a protected principal-root
object envelope and is not yet exported/imported by Recovery Kit. The Browser
capsule and web pages still never receive host paths or profile keys, but
cookies, localStorage, IndexedDB, service workers, bookmarks, and history must
not be described as encrypted/recoverable until the Browser profile-store or
protected-disk contract lands with tests. Runtime Browser profile descriptors
and reset receipts must declare
`storage_posture=principal_owned_reset_scoped_unprotected`,
`protected_storage=false`, `encrypted=false`, and `recoverable=false`.

The product Browser control service exposes a constrained `window.ethereum` for
account and chain discovery and converts `personal_sign` / `eth_sign` into
`browser_personal_sign` Wallet/Inbox approval requests. EIP-712 requests
(`eth_signTypedData`, `eth_signTypedData_v3`, and `eth_signTypedData_v4`) use a
separate `browser_typed_data_sign` approval intent so typed-data prompts remain
visible and auditable. It reports Runtime wallet accounts and supports chain
switching across available EVM accounts. The Playwright proof remains a
diagnostic/account-chain/personal-sign surface unless it explicitly implements
additional methods. Read-only dapp calls such as
`eth_blockNumber`, `eth_getBalance`, `eth_getTransactionByHash`, and
`eth_getTransactionReceipt` do not become Inbox approvals because they are not
wallet authority. They route through the Browser wallet-read gateway into typed
`chain-provider` resources for the principal's selected EVM chain and return
only the JSON-RPC result shape to the page. Managed `eth_sendTransaction` is
converted into a typed transaction flow: Gateway
validates the Browser request, `chain-provider` prepares the unsigned intent,
Wallet/Inbox approves and signs, and `chain-provider` broadcasts. Web pages
receive only the final transaction hash. Runtime submits the Wallet approval as
a typed `RequestApproval` through the private Wallet Bus; the approval resource
shown to the user remains the chain effect:
`elastos://chain/<network>/broadcast_transaction`. External EVM accounts use
connector handoff after Wallet/Inbox approval. For injected MetaMask/Brave, the
opaque connector capsule sends only the exact approval request id through the
closed Home wallet-effect message. Runtime validates matching launch-token v4
Home and connector authorities and returns a typed handoff to the trusted
top-level Home host; Home performs only the fixed provider effect and completes
the existing Wallet Bus request. The connector frame receives status, not the
transaction, signing message, provider object, signature, or transaction hash.
Configured WalletConnect retains its connector-owned adapter path. Raw signing,
raw transaction broadcast, connector SDK objects, and private keys remain
unavailable to web pages. Approved message and typed-data signature requests
resolve back to the page only after Wallet/Inbox approval and managed or
connector signature completion.

`scripts/browser-wallet-bridge-smoke.sh` is the browser-level wallet proof. It
launches an actual page through Runtime proxy/local Exit, calls the injected
EIP-1193 provider, verifies `eth_requestAccounts`, verifies the initial ESC
chain (`0x14`), switches to Base (`0x2105`), and verifies the selected account
tracks the active chain. This complements connector-level transaction smokes; it
does not expose connector SDKs or raw wallet RPC to the page.

`scripts/wallet-connector-transaction-smoke.mjs` is the connector-level
authority proof. It checks that MetaMask/Brave and UniSat frames remain opaque,
carry their launch token through the Home message, and cannot submit arbitrary
methods, messages, or transaction payloads. The Home shell bridge smoke covers
exact WindowProxy/origin/token/connector binding, bounded replay and
single-flight behavior, exact EIP-6963 selection, and no extra connector
window. WalletConnect is checked separately on its unchanged configured path.

`scripts/browser-glide-wallet-smoke.sh` is the dapp compatibility proof for the
current objective. It opens `https://glidefinance.io/` through the Browser
engine, clicks Glide's Connect Wallet flow, selects the Metamask-compatible
injected provider, and verifies Glide renders the connected ESC account while
the Browser path reports `direct_network=false`.

`scripts/browser-hosted-product-wallet-smoke.sh` is the hosted-product bridge
proof. It launches through the Selkies/GStreamer Browser Engine Adapter, uses
the operator-private CDP endpoint only to inspect the controlled remote page,
and verifies the page received a constrained Runtime-mediated EIP-1193 provider:
`eth_requestAccounts` starts on ESC (`0x14`), `wallet_switchEthereumChain`
switches to Base (`0x2105`), and signing is routed through Runtime
Wallet/Inbox approval before the page promise resolves. This is a bridge proof,
not a substitute for the full hosted Glide acceptance flow.

`scripts/browser-hosted-product-glide-wallet-smoke.sh` is the hosted-product
dapp compatibility proof. It opens `https://glidefinance.io/` through the live
Selkies/GStreamer adapter, clicks Glide's Connect Wallet flow, selects the
injected MetaMask-compatible provider, and verifies Glide renders the connected
ESC account while the adapter reports `direct_network=false`.

WalletConnect follows the same rule. Mode A, where ElastOS connects to an
external wallet, lives behind the wallet provider. Mode B, where external dapps
connect to ElastOS as a wallet, must route every request through runtime approval
and audit.

## Host adapters and remote viewers

The production Browser has one VM Engine behind the Browser/Net/Exit ABI.

| Host | Engine placement | Boundary |
|---|---|---|
| Apple Silicon Mac | Linux ARM64 guest through Apple Virtualization | Per-launch guest, Runtime-owned vsock streams and lifecycle. |
| Jetson ARM64 Linux | The same Linux ARM64 guest through crosvm/KVM | Per-launch guest, Runtime-owned network device and authorized Exit. |
| Linux x86-64 or a host without compatible virtualization | Approved remote Engine | Runtime authenticates the selected Engine and carries its control/media through Carrier. |
| Other viewers | Approved remote Engine, after viewer qualification | The same WebRTC, principal, origin and lifecycle contracts. |

An x86-64 guest can use the same recipe after its target qualification. Native
WebView, CEF and Playwright experiments retain test-only ownership. Device
support follows installed evidence for the stated host adapter and viewer.

## Native Desktop Shell And Packaging

Loading ElastOS "like KDE or GNOME" is possible as a product direction, but it is
not implemented as a native desktop shell today. The current front door is a
Runtime-owned Home web surface served by the gateway and opened from `elastos`.
That path is intentionally KVM-independent and can run on macOS when the native
providers and source-home setup are available, but it is still a web-hosted Home
surface, not a native compositor or OS session manager.

A true native ElastOS desktop would be a separate host shell/launcher layer. It
would own native windows, tray/menu integration, app focus, system notifications,
and a secure display attachment contract while still routing app authority
through Runtime capabilities. That shell could later host Browser
`native_surface` sessions, but the Browser engine remains behind the same
Browser/Net/Exit ABI and must still prove network denial, profile isolation,
wallet mediation, media, and cleanup.

A macOS `.dmg` is feasible as distribution packaging after the macOS source-home
path is stable, but it is not present in this repo as a finished release
artifact. A `.dmg` would need to bundle or install the `elastos` binary,
provider binaries, Home/capsule assets, a launch wrapper or `.app`, passkey/RP
origin policy, update policy, and operator config without changing the Runtime
authority model. Packaging Home into a `.dmg` does not by itself prove native
Browser support.

Cosmopolitan Libc is not the Browser answer. It can produce portable C/C++
executables and may be worth researching for small helper tools, but it does not
solve Chromium/WebView embedding, GPU/audio, microVM/container isolation, Rust
workspace packaging, macOS app signing/notarization, or Runtime/Carrier
capabilities. Treat it as runtime-framework research, not as a Browser engine
or `.dmg` strategy.

`scripts/browser-hosted-provider-candidate-smoke.sh` is the provider comparison
gate. It runs the product-display contract, Runtime/provider navigation proof,
WebRTC media/audio quality gate, Runtime-mediated wallet bridge proof, and Glide
connect-wallet proof against one `browser-engine-adapter.json`. The navigation
proof is `scripts/browser-hosted-product-navigation-smoke.sh`: it verifies
address navigation, back, forward, and reload with `direct_network=false`. A
hosted provider is not a replacement candidate until it passes that script or an
equivalent stricter gate. The current live Selkies target passed the earlier
full gate with product compositor WebRTC, audio/video tracks, datachannel input,
a 60-second controlled media hold, the quality gate, Runtime-mediated wallet
bridge, Glide connect-wallet flow, and `direct_network=false`; rerun the
expanded gate after navigation/control-service changes.

The expanded gate has now passed against the live Selkies target with
Runtime/provider address navigation, back, forward, and reload. That makes
Selkies a valid baseline for the Browser ABI, not the final UX answer. The same
target still fails product-compositor YouTube stress tests: embeds can return
YouTube `Error 153`, and normal watch pages can decode initial media but pause
around ad/sign-in/profile flows. Before investing more in Selkies-specific
tuning, run BrowserBox and KasmVNC/Workspaces through the same
`hosted_remote_browser` candidate gate and a manual UX check for typing,
scrolling, media playback, and perceived latency.

The concrete comparison plan is in
[`docs/BROWSER_PROVIDER_BAKEOFF.md`](BROWSER_PROVIDER_BAKEOFF.md). The machine
gate is `scripts/browser-hosted-provider-bakeoff.sh`; it wraps the hosted
provider candidate gate and product-compositor YouTube stress gate, then leaves
manual UX review explicit.

Servo and full WASM/WASI browser engines are research paths. They fit the
long-term capsule ideal, but they are not the shortest route to a working,
modern, wallet-capable browser.

## Delivery and verification order

Runtime admits the signed Engine image and its matching host helpers, then
prepares the owner's Engine and Exit selection through normal setup. Mac Apple
Virtualization and Linux ARM64 crosvm run the same guest. Other hosts select an
approved remote Engine through Runtime and Carrier. The guest input hash permits
reuse while its recipe, locked dependencies and build options remain identical.

The first installed journey covers Home sign-in, a decoded WebRTC page,
navigate/type/scroll/click/reload, Close and profile reopen. Repeat that journey
after a normal update. Runtime owns the separate mutable profile and process
lifecycle; the Engine owns execution, rendering and website TLS. Exit owns the
authorized destination streams. The guest network boundary confines all website
traffic to that Runtime route.

Wallet follows the installed journeys on both target machines. Its origin-bound
EIP-1193 bridge sends requests to Runtime and Wallet/Inbox for explicit approval.
Broader media, recovery, device and workload qualification retains its complete
acceptance criteria in the owning Browser issues. Playwright and the retired
host-native namespace tools remain test fixtures behind the adapter's explicit
`test-engine` feature.

## Verification Matrix

Before calling the browser capsule real:

- A test proves a browser capsule without `elastos://net/*` capability cannot
  open any off-box destination.
- A test proves browser DNS does not bypass the Runtime Net provider.
- A test proves browser HTTP(S) streams use the selected exit provider.
- A target VM proof confirms that the guest sends website TCP, DNS and HTTP
  through Runtime Net/Exit, with the host adapter enforcing the guest boundary.
- A test proves LAN/private IPs are blocked by default.
- A test proves wallet injection is origin-bound and request-scoped.
- A test proves dapp wallet requests appear in Wallet/Inbox before any
  signing effect.
- A test proves rejecting a wallet request returns a rejection to the page and
  records audit.
- A test proves approving a request returns only the scoped result, not wallet
  RPC, node RPC, private keys, or connector SDK state.
- A test proves browser profile, cookies, bookmarks, downloads, and history are
  rooted under the active principal and either use a protected principal-root or
  encrypted provider-owned storage contract with Recovery Kit coverage, or the
  product explicitly marks Browser VM profile disks excluded from protected
  storage and Recovery Kit claims.
- A test proves the visible Browser surface receives decoded WebRTC media and
  continuous input under the selected Engine contract.
- A test proves product Browser launch fails closed when the selected display
  session is unavailable.
- Manual smoke covers Linux x86_64, Jetson aarch64, Windows, macOS, and Android
  before any platform is advertised as supported.

## Red Lines

- Do not call iframe-rendered external web content or a normal host tab the final
  browser engine.
- Do not implement fallback display modes. A selected display mode either starts
  or fails closed.
- Do not allow browser engines to keep ambient host network access while
  claiming Runtime-only networking.
- Do not make HTTP-fetch proxying the default for arbitrary browsing.
- Do not let web pages call Runtime APIs directly.
- Do not put MetaMask, WalletConnect, chain RPC, node RPC, or IPFS SDK authority
  inside ordinary app capsules.
- Do not claim Android/macOS/Windows support until the adapter has platform
  proof, not just shared JavaScript UI.

## Research Notes

Primary documentation checked while defining this plan:

- CEF is a Chromium-based embedding framework and documents platform-specific
  application layouts for Windows, Linux, and macOS.
- WebView2 lets a Windows host app observe and customize web resource requests,
  but that is not by itself an OS-level no-network proof.
- Android WebView provides `WebViewClient.shouldInterceptRequest`, but mobile
  host policy is still required for a strong boundary.
- Apple `WKURLSchemeHandler` handles custom URL schemes, which is useful for
  controlled resources but not enough by itself for arbitrary `https` isolation.
- WPE WebKit targets embedded and low-power Linux devices and may be useful for
  Jetson-class appliance proof, but dapp compatibility still needs testing.
- Servo is an embeddable Rust browser engine directionally aligned with ElastOS,
  but it is still an R&D engine target for this project.
- Selkies-GStreamer is the strongest open-source hosted-display precedent for
  this project: it is a Linux-native WebRTC remote desktop stack built around
  GStreamer capture/encoding of screen and audio, designed for self-hosted,
  container, Kubernetes, and cloud/HPC deployments.
- Kasm/KasmVNC proves production browser/container isolation, but it should be
  treated as a reference or possible backend behind `hosted_remote_browser`, not
  the Browser ABI itself. Do not treat the standalone `kasmweb/chromium` image
  as an ElastOS audio proof by itself: Kasm's Docker image docs state that
  audio, uploads, downloads, and microphone passthrough are only available when
  using Kasm Workspaces orchestration. A Kasm spike must therefore prove the
  Workspaces/orchestrated path or an explicitly equivalent audio pipeline.
- AWS AppStream proves HTML5 app/browser streaming at managed-service scale,
  including audio-capable sessions, but its trust and operator model are not an
  ElastOS capsule/provider boundary.
- Cloudflare Browser Isolation proves remote browser isolation as a production
  category, but its proprietary Network Vector Rendering service is not the
  self-hosted Runtime/Carrier implementation target.
- BrowserBox is another production-oriented remote browser isolation precedent,
  but its commercial licensing makes it a candidate backend/reference point
  rather than a default open Runtime dependency.
- Guacamole/noVNC are mature browser-access remoting systems, but they are not
  the first choice for smooth browser video/audio because VNC/RDP-style display
  transport is the wrong performance target.
