# Browser contract

Runtime owns Browser compatibility, principal and operator authority, provider
selection, transport, and session lifecycle. Engine providers own page semantics
and rendering. Exit providers own authorized website connections. Browser UI and
operator adapters use the same resources for local and remote execution.

The executable wire definitions shared by Runtime and the Engine provider live
in [browser_protocol.rs](../elastos/crates/elastos-common/src/browser_protocol.rs).
The [Browser architecture](BROWSER_CAPSULE.md) describes provider implementation,
the [support matrix](BROWSER_SUPPORT.md) describes qualification, and
[TASKS.md](../TASKS.md#browser-maturity-workstream) records remaining work.

## Contract identity and ownership

There are three existing version boundaries. Their versions have different
meanings and must be checked at the boundary they identify.

| Boundary | Identity | Enforcement |
| --- | --- | --- |
| Capsule package | `elastos.capsule/v1`, `elastos.runtime-projection/v1`; interface `elastos.browser.page` version `0.5.0` | Runtime admits the package and its declared affordances. These identifiers describe the current package interface, not a release readiness claim. |
| Runtime web projection | `elastos.browser.runtime/v1` summary and operation-specific result schemas | A verified Browser launch grant binds each request to principal, session and launched Browser instance. The HTTP routes are a Runtime adapter for those resources. |
| Runtime to Engine | Provider `browser-engine-adapter`, protocol `2.1` | Runtime validates the provider inventory before profile preparation, lifecycle reservation, streams or launch. Engine results and cleanup records retain exact provider and protocol bindings. |

The Engine version policy is exact compatibility with the declared protocol.
Missing, older, newer, or malformed versions produce
`incompatible_engine_protocol`. A future minor version requires an explicit
compatibility decision and tests before it is admitted. Package version,
Engine protocol version, and installed binary version cannot substitute for
each other.

A capsule package is an immutable admitted artifact. A running Browser instance
is an authority-bound execution with a lifecycle. A profile is mutable state
owned by a principal. Installing the same package on another compatible Runtime
and invoking an already running remote provider are distinct operations. The
calling capsule's resource contract is the same in both cases.

## Resources and typed operations

The Browser capsule already declares these resources. Their current web adapter
and exact operation inputs are described here; this document does not create a
second dispatch path.

| Resource | Operations and input | Result/ownership |
| --- | --- | --- |
| `elastos://browser/page` | Open with `BrowserOpenRequest`: URL, optional reason, selected Engine/Exit IDs, Browser instance, viewport, display mode, isolation guarantee, and async preference. Navigate and input use the exact acquired page. | Open result, pending open handle, page status, and terminal outcome. Runtime derives principal, session, profile and provider route from authority. |
| `elastos://browser/display` | Attach to an acquired page; signaling uses `BrowserWebrtcSignalRequest` with display attachment, offer/answer/candidate/end-of-candidates, channel and bounded signaling data. | A page-bound `elastos.browser.display-session/v1`; WebRTC product media and authorized input. |
| `elastos://browser/exit` | Select an admitted Exit offer for the next launch. Current open input carries `remote_exit_id`; omission selects the local Exit policy. | The launch lifecycle records the exact Exit identity. Net validates destination policy and Exit owns stream execution. |
| `elastos://browser/profile` | Inspect the projected profile facts and explicitly reset an idle profile. | Principal-owned profile state, separate from package and page lifetime. Reset requires the owning authority and closed pages. |
| `elastos://browser/wallet-bridge` | Request a website operation through the current page's origin and Runtime authority. | Wallet/Inbox owns account and signing approval. Browser receives the permitted outcome. |
| Runtime session/open ownership | Observe pending open; heartbeat an acquired page; close with `BrowserPageCloseRequest`, exact cleanup ID and Browser instance. | Runtime owns acquisition, renewal, cancellation settlement, close, and orphan cleanup. |

An operator request cannot contain a principal, provider route, profile disk
path, remote connect ticket, or arbitrary authority descriptor. The shared
request types reject undeclared fields. Browser grants and existing resource
checks apply to GUI and programmatic callers alike. Production delegated agent
identities, semantic automation, and Playwright/Camofox/Camoufox adapters remain
the separate B04 delivery gate; an authenticated HTTP client alone does not
establish those capabilities.

Current HTTP adaptation:

| Operation | Route | Wire contract |
| --- | --- | --- |
| Discover current resource state | `GET /api/apps/browser/summary` | `elastos.browser.runtime/v1`, scoped Engine inventory, Net/Exit state and session projection |
| Open | `POST /api/apps/browser/open` | `BrowserOpenRequest`; `elastos.browser.open-result/v1`, `open-accepted/v1`, or `open-error/v1` |
| Observe acquisition | `GET /api/apps/browser/open/:open_id` | `elastos.browser.open-status/v1`, bound to the verified launch owner |
| Observe page | `GET /api/apps/browser/pages/:page_id/status` | `elastos.browser.page-status/v1` |
| Observe diagnostics | `GET /api/apps/browser/pages/:page_id/diagnostics` | Bounded, authorized diagnostics for that page |
| Discover page inspection | `GET /api/apps/browser/pages/:page_id/inspect` | Optional `elastos.browser.inspect-capabilities/v1` |
| Inspect the Engine document | `POST /api/apps/browser/pages/:page_id/inspect` | `elastos.browser.inspect-request/v1`: schema, limit and cursor; `elastos.browser.inspect-result/v1` |
| Renew viewer activity | `POST /api/apps/browser/pages/:page_id/heartbeat` | Exact page ownership and Runtime lease handling |
| Input | `POST /api/apps/browser/pages/:page_id/input` | `BrowserInputRequest`; the Engine validates its typed input-event schema |
| Signaling | `POST /api/apps/browser/pages/:page_id/webrtc` | `BrowserWebrtcSignalRequest`; Runtime validates type, bounds and page binding |
| Close | `POST /api/apps/browser/pages/:page_id/close` | `BrowserPageCloseRequest` with `elastos.browser.close-request/v2` and the Runtime cleanup handle |
| Reset profile | `POST /api/apps/browser/profile/reset` | Explicit principal-bound destructive operation |

Page inspection projects native Engine accessibility roles, names, descriptions
and values from the top document. Each result binds a document generation and
snapshot ID. Opaque node references map to private Engine nodes; navigation,
replacement, close and a 30-second expiry invalidate the snapshot. The provider
limits collection to 512 nodes, 128 KiB and 1.5 seconds; each response contains
at most 64 nodes and 32 KiB. Native reply size and expansion work are bounded.
Runtime checks the exact launch owner before dispatch and before returning
content. This read preserves the page, media and input lifetimes. Typed actions,
delegation, child frames and operator-adapter conformance remain B04 successors.

The private `BrowserProfileDescriptor` includes the host-adapter disk binding.
Runtime sends that descriptor only to its selected Engine adapter. Public
profile projection keeps the authorized resource identity and protection facts.
Remote profile migration requires its own granted transfer and compatible
format; sending a local filesystem path to an arbitrary peer is not migration.

## Engine capability discovery and selection

`BrowserEngineInventory` carries provider identity, protocol version, status,
adapter count, authority proofs, and at most 64 adapter entries. Each entry
declares a safe ID, Engine implementation label, default marker, backing
substrate label, display modes, isolation guarantees and Runtime-only network
mode. Inventory count, unique IDs, exactly one configured default, explicit
authority proofs, bounded capability lists, and supported enum values are
validated before selection. Private provider fields are excluded from the
public typed inventory.

`configured` means a provider has configuration. Installed artifact readiness,
available capacity, permission to use a service, and installed-product
certification are separate facts. Before preparing a profile or reserving launch
effects, Runtime requests `readiness` for a compatible Engine. The result uses
`elastos.browser.engine-readiness/v1`, binds the selected adapter ID, and carries
`ready` or `unavailable` with a typed reason. An explicit selection retains its
identity; automatic selection tries the compatible default, then other compatible
entries in the admitted inventory. A failed adapter prewarm retains the inventory
so Runtime can evaluate the other entries.

The VM control service exposes the same readiness report through its private
`GET /readiness` operation. Its host check verifies the rootfs receipt, kernel and
initrd identities, actual host architecture, and usable KVM or entitled VZ support.
It caches a successful result only while file identity, size and nanosecond change
timestamps match. Changed files require a fresh check. Host inspection is bounded
at eight seconds, and Runtime bounds candidate readiness requests at twelve
seconds in total. Installation and repair still need atomic admitted artifact
sets; service discovery and installed qualification retain their own gates.

Protocol 2.1 requires this readiness operation. Runtime rejects older Engine
protocols before launch effects. Close active 2.0 sessions before installing the
new Runtime/Engine pair; durable cleanup bindings keep their exact protocol
version. Normal remote Runtime service admission remains B08 work. Legacy
operator tunnel launchers report `readiness_unsupported` until they implement a
verified remote-host result.

An explicit Engine selection either satisfies the requested display and
isolation requirement or returns a typed rejection. Automatic selection first
tries the configured default when it satisfies the requirement, then the first
compatible entry in the Runtime-admitted inventory. It preserves the requested
isolation level. The same algorithm applies to a local or approved remote
adapter; placement does not grant additional authority.

Valid display/guarantee pairs are WebRTC with microVM isolation, WebRTC with an
operator remote-browser boundary, and an explicitly declared native surface
with a policy-WebView boundary. Native surfaces are provider capabilities that
require their own qualification. The Browser product display remains WebRTC.
Diagnostic and image-polling display modes are rejected.

## Errors, events, deadlines and cancellation

The web Runtime adapter reports `elastos.browser.viewer-capabilities/v1` with
WebRTC transport features, reported receive-codec MIME types and an eligibility
result. Its `viewer_unavailable` and `unsupported_viewer_display_mode` errors
carry `stage: viewer_compatibility` and a terminal pre-effect open outcome. This
check runs before closing the previous page or dispatching a new open. Codec
queries can be unreported; actual SDP negotiation and media proof determine
interoperability. Headless and native operators retain their own display
capability requirements behind the same Runtime page contract.

Compatibility errors are stable typed codes:
`incompatible_engine_protocol`, `invalid_engine_status`, `engine_unavailable`,
`engine_not_found`, `incompatible_engine_capabilities`, and
`no_compatible_engine`. Readiness rejection uses `engine_not_ready`, a typed
`reason`, and `stage: engine_readiness`; it retains the same terminal pre-effect
outcome. Browser UI maps those reasons to preparation, repair, update, connection,
or host-selection instructions. Compatibility failures carry `stage: engine_compatibility` and
an `elastos.browser.open-outcome/v1` stating that page, VM and stream effects
were not acquired. Browser UI explains version and capability remedies; agents
can use the same code and outcome. A cleanup-pending outcome retains priority
over compatibility wording after an effect has been acquired.

Runtime binds lifecycle observations to the exact launch generation. Existing
observations include acquisition progress, active page, navigation, retirement,
failure, acquired effects, pending cleanup and terminal settlement. Page IDs and
cleanup IDs are scoped by the verified launch owner, not globally usable bearer
authority. Existing progress is a status projection; a subscription/event API
and semantic operator waits require B04/B06 implementation and tests.

Runtime summary carries the requested Engine and Exit choices on a recoverable
page as `service_selection` (`elastos.browser.service-selection/v1`). The
`engine_id` and `exit_id` fields identify the original choices; empty values mean
Automatic Engine and local Exit. Runtime keeps these values separate from the
resolved adapter and redacted routing labels, through launch, durable ownership,
reconciliation and cleanup transfer. Legacy ownership can omit this field; the
viewer preserves cleanup authority while recovery of those choices is unavailable.

The session summary's `fresh_start_allowed` result describes the verified
principal and Browser window scope. Runtime checks pending open jobs, launching
and active sessions, reconciliation, cleanup and capacity. A null recoverable
page alone is insufficient to start another page. Unavailable or missing scope
evidence keeps startup pending. Fresh open still passes the existing Runtime
admission checks. Viewer reload adopts an active page and connects its display;
viewer unload ends that document's observations while Runtime retains ownership.

### Fresh viewer attachment

Runtime advertises `engine_adapter.display_attach_supported: true` when its web
adapter accepts fresh display attachment. The retained Engine display also needs
a `display_generation` of `display:` followed by 32 lowercase hexadecimal digits.
Browser checks both facts. Initial signaling remains compatible with existing
2.1 Engines; the optional generation alone does not enable a new Runtime request.

A new viewer uses the existing page signaling route with `type: display_attach`,
a 32-digit lowercase hexadecimal `request_id`, and the expected
`display_generation`. Runtime normalizes this as
`elastos.browser.display-attach-request/v1`. Attachment carries no channel, SDP
or candidate. The existing page grant authorizes it. Runtime and the adapter
retain one attempt; matching retries join or replay that request before checking
the current generation. A different request receives `display_attach_busy`
while the original request remains pending with an uncertain outcome.

A successful `elastos.browser.display-attach-result/v1` has exactly seven fields:
`schema`, `page_id`, `request_id`, `previous_display_generation`, a fresh
`display_generation`, `initial_offer`, and `audio_offer`. Each offer uses
`elastos.browser.webrtc-offer/v1`. Runtime updates these display fields inside
its retained authority. Page, profile, VM, service choices, transport streams
and cleanup generation keep their existing owners. The Engine retires the old
video/audio signaling pair and creates a fresh pair for that same page.

After attachment starts, each answer, candidate and end-of-candidates message
carries its display generation, and the acknowledgment echoes it. Engine and
Runtime reject stale generations. Viewer callbacks also retain their exact
peer identity so a late response cannot affect a later connection. Close keeps
priority over late attachment completion.

Recovery summary exposes an optional `display_attachment` object with schema
`elastos.browser.display-attachment/v1`, `state` (`pending`, `ready` or `failed`),
`request_id`, `previous_display_generation` and optional `error_code`. A new
viewer reconciles a pending attempt with that exact request identity. This
bounded in-process receipt does not qualify Runtime restart recovery; restart
continues through durable page ownership and cleanup reconciliation.

The typed errors `display_attach_busy`, `display_generation_mismatch` and
`display_owner_changed` use HTTP 409; `display_attach_unsupported` uses 501;
`display_attach_failed` and `display_attach_uncertain` use 503. Guest control,
VM proxy and Engine adapter preserve this allowlisted code across the existing
route. The Engine bounds paired offer preparation to four seconds; the adapter
bounds the control exchange to five seconds. Adapter timeout retains the same
uncertain request for reconciliation. Engine preparation timeout caches terminal
`display_attach_failed` for that request. These operation limits are separate from the B06
five-second user recovery gate and its installed evidence.

Provider timeouts bound individual calls. The current implementation has a
five-minute stale-heartbeat threshold, a fifteen-minute retained open-job TTL,
and bounded launch reconciliation calls. These are implementation limits in
[gateway_browser_sessions.rs](../elastos/crates/elastos-server/src/api/gateway_browser_sessions.rs)
and [gateway_browser.rs](../elastos/crates/elastos-server/src/api/gateway_browser.rs),
not the proposed short remote-authorization lease or responsiveness goals.

Every operation needs an explicit result or a retained reconciliation owner.
Losing a response cannot establish that a launch or website action did not
happen. Close uses exact Runtime cleanup ownership and converges on a terminal
receipt. A cancelled or timed-out dispatch retains cleanup responsibility until
its outcome is known. B06 owns remaining lifecycle and cancellation delivery;
B14 owns bounded revocation during a network partition.

## Qualification

Contract tests cover unknown versions, malformed inventories, authority
substitution, display/isolation mismatch, explicit selection, automatic
selection and placement-independent decisions. Route tests must additionally
prove that rejected compatibility never allocates a page or stream and never
dispatches a profile or Engine launch. Provider tests decode actual provider
status through the same shared Runtime types.

Installed evidence must bind the exact source, package, binaries, device roles,
viewer, codecs and topology. A contract test does not certify a device. The
[support matrix](BROWSER_SUPPORT.md) and B01-B16 acceptance gates retain those
separate obligations.
