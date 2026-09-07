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
| Runtime to Engine | Provider `browser-engine-adapter`, protocol `2.0` | Runtime validates the provider inventory before profile preparation, lifecycle reservation, streams or launch. Engine results and cleanup records retain exact provider and protocol bindings. |

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
| `elastos://browser/display` | Attach to an acquired page; signaling uses `BrowserWebrtcSignalRequest` with offer/answer/candidate/end-of-candidates, channel and bounded signaling data. | A page-bound `elastos.browser.display-session/v1`; WebRTC product media and authorized input. |
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
| Renew viewer activity | `POST /api/apps/browser/pages/:page_id/heartbeat` | Exact page ownership and Runtime lease handling |
| Input | `POST /api/apps/browser/pages/:page_id/input` | `BrowserInputRequest`; the Engine validates its typed input-event schema |
| Signaling | `POST /api/apps/browser/pages/:page_id/webrtc` | `BrowserWebrtcSignalRequest`; Runtime validates type, bounds and page binding |
| Close | `POST /api/apps/browser/pages/:page_id/close` | `BrowserPageCloseRequest` with `elastos.browser.close-request/v2` and the Runtime cleanup handle |
| Reset profile | `POST /api/apps/browser/profile/reset` | Explicit principal-bound destructive operation |

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
certification are separate facts. B02 supplies authoritative readiness; B08
supplies normal remote Engine service admission. Neither is inferred from a
configuration label.

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
`no_compatible_engine`. Open failures carry `stage: engine_compatibility` and
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
