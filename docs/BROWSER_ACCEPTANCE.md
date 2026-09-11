# Browser maturity goals and acceptance contract

This document defines the work needed for a stable Browser experience for people
and agents. It gives implementation instructions and acceptance criteria. The
checkboxes in [TASKS.md](../TASKS.md#browser-maturity-workstream) are the canonical
work status. These criteria are proposed release requirements, not claims that
the current product meets them.

## Product contract

A person or agent opens Browser, selects an approved profile and services, and
uses a page. Runtime makes the selected capsules work on the available devices.
The operator uses the same actions when Engine or Exit moves to another Runtime.
Browser Engine renders and operates websites. Browser Exit provides website
egress through Runtime Net authority. Browser UI presents the session.

**Local = nonlocal means equal contracts, authority, lifecycle, and observable
results.** Remote execution also has transport latency, availability, cost, and
trust properties. Runtime handles these properties and exposes useful service
state. It preserves the operator's approved Engine, Exit, profile, and spending
policy when a connection changes.

Ownership follows [PRINCIPLES.md](../PRINCIPLES.md):

| Owner | Responsibility |
| --- | --- |
| Browser UI and operator adapters | User interaction, semantic inspection, commands, progress, and recovery choices through a versioned Browser contract. |
| Runtime | Principal, delegation, capabilities, service discovery and selection, compatible artifact installation, host adaptation, session lifecycle, resource limits, and audit. |
| Engine provider | Browser implementation, page semantics, rendering, input, automation protocol translation, and profile operations within its authority. |
| Exit provider | Authorized website connections, DNS behavior, stream lifecycle, and usage accounting. |
| Carrier | Authenticated transport between Runtime endpoints selected by Runtime. |
| Wallet and other trusted providers | Their own approvals and protected operations. A website or browser operator can request an operation; its provider decides whether it is authorized. |

Keep the trusted Runtime core small. Reuse the existing provider registry,
capability checks, lifecycle records, and transport. Browser-specific rendering
and automation stay in providers and adapters. Repair the current working Engine
in small steps before considering an engine replacement.

Capsule package, running instance, and mutable profile are separate resources.
Portable packages can contain compatible platform payloads selected by Runtime.
Remote service use invokes an instance on another Runtime. Both paths need proof;
one successful remote session does not prove package portability.

## Evidence that sets the initial priorities

The initial analysis found these repair targets. Its exact source revision is
recorded in the [Browser maturity workstream](../TASKS.md#browser-maturity-workstream).

- Artifact setup can report success with required Engine artifacts missing.
  Home can offer an Engine from configuration or binary presence before launch
  readiness is established.
- Continuous wheel input uses a trailing delay that can withhold events until
  the gesture stops. Horizontal input also needs a regression case.
- A temporary heartbeat or status request failure can close the display and
  trigger page cleanup before reconnection has a chance to recover.
- Shared synchronous provider request handling can delay unrelated session
  operations behind an Engine launch.
- Remote Engine setup still needs operator transport machinery, while the
  user service-request path handles Exit. Engine needs the same Runtime service
  model.

These findings set the order below. They do not establish the cause of another
person's failure without that installation's diagnostic receipt. The inspected
Brave session failed before an active Browser page; it supplied startup evidence,
not media or usability acceptance evidence.

## Execution order

B01-B16 define release acceptance areas. Current execution slices, owners and
deadline checkpoints live in [TASKS.md](../TASKS.md#browser-maturity-workstream).
The accepted B01 contract and bounded B04 ownership prerequisites unblock their
consumers while device and operator qualification remain open. Run local
installation/usability, operator capability
and independent Engine/Exit placement work in parallel where their contracts
and resources allow it. B03 diagnostics and B14 authority checks apply to each
slice; B12 profiles, B13 workflows and B15 update/repair stay in the delivery
queue. Reserve time for B11 sustained-use proof before B16 final review.
Every repair returns to the nearest installed product journey. Full acceptance
below remains unchanged when an execution slice passes.

The numbers below are proposed engineering targets. Record a baseline on named
devices before changing them. Any target change needs an explicit rationale in
the acceptance contract; a failed run remains a failed run against its recorded
target.

<a id="b01"></a>
## B01. One Browser contract and an explicit support matrix

**Owner:** Runtime contracts and release engineering.

**Instructions:** Specify session, page, profile, display, operator, Engine, and
Exit resources. Define typed requests, events, errors, deadlines, cancellation,
version negotiation, and capability discovery. Keep host launch details and
provider transport private to Runtime adapters. Adapt existing contracts where
possible. Certify viewer, Engine host, and Exit host as separate device roles.
Start with the existing macOS VZ and Linux/crosvm target paths. Evaluate Intel
Mac, Windows, and mobile roles explicitly before advertising support.

**Acceptance:**

- A published matrix names OS and architecture, minimum resources, host
  virtualization requirements, outer browser versions, codecs, and supported
  roles. Every supported row links to an installed-product test receipt.
- Runtime detects capabilities and installs the matching signed payload or
  selects an already approved remote service. An unsupported local Engine host
  can still serve as a supported viewer when a suitable remote Engine exists.
- The same Browser capsule package and operator contract run across certified
  hosts. OS switches, executable paths, SSH commands, and VZ/crosvm details are
  confined to Runtime/provider adapters.
- Supported version combinations pass contract tests. An incompatible peer
  produces a typed compatibility error before allocating a page or transferring
  profile state.

<a id="b02"></a>
## B02. A fresh installation opens a usable Browser

**Owner:** Runtime installation and Engine host adapters.

**Instructions:** Install a verified, compatible artifact set atomically. Include
the Engine, host helpers, VM image where required, configuration, and launch
permissions. Make readiness one authoritative result shared by setup, Home,
service publication, and launch. Model installation, download, verification,
readiness, and failure separately. Offer a direct repair action for repairable
conditions.

**Acceptance:**

- On each certified Engine host, a fresh supported installation opens the
  controlled test page with the documented normal setup steps and no manual
  paths, copied artifacts, environment variables, or terminal repair.
- Tests cover an empty data root, each missing/corrupt artifact, wrong
  architecture, denied virtualization access, interrupted download, and low
  disk space. Incomplete preparation reports incomplete status and a useful
  remedy; it never becomes a ready Engine offer.
- A ready remote Engine works from a viewer installation that has no local
  Engine binary or VM image. Runtime installs only dependencies needed for the
  selected roles.
- After installation, built, installed, and served artifacts match their
  receipt hashes. First launch has visible progress; cancellation cleans up
  partial work. A restart after interruption resumes or repairs deterministically.

<a id="b03"></a>
## B03. Every failure has an actionable diagnosis

**Owner:** Runtime lifecycle and Browser diagnostics.

**Instructions:** Trace installation, service resolution, authorization, Engine
acquisition, page creation, signaling, first frame, input acknowledgment, Exit
connection, and close under one operation/session identity. Preserve the failure
stage and cause behind a short user message. Provide the same structured result
to agents. Make a redacted diagnostic report available from the error view.

**Acceptance:**

- Injected missing-artifact, denied-grant, unavailable-peer, version mismatch,
  launch timeout, media failure, and Exit failure cases each produce the correct
  stage, stable error code, retryability, and recovery action.
- One exported report identifies Runtime/Engine/capsule versions, artifact
  integrity, detected device capabilities, selected service identities, and
  stage timings. A maintainer can distinguish setup, Engine, transport, display,
  and website failures without requesting a terminal transcript.
- Metrics distinguish first installation from cold launch, warm launch,
  website load, and reconnection. Collection uses bounded storage and excludes
  credentials, page contents, and browsing history by default.

<a id="b04"></a>
## B04. People and agents have equal Browser capabilities

**Owner:** Browser contract, operator adapters, and Runtime delegation.

**Instructions:** Expose a tool-neutral operator contract on the actual Engine
page. Provide semantic inspection, actions, events, and waits. Build adapters
for native Runtime clients, Playwright, and the Camofox agent API; evaluate and
certify a Camoufox/Firefox provider path for its compatible clients. Reuse one
page/session lifecycle and permission model. An agent inspecting the outer
video element is not inspecting the website inside the Engine.

Ship the smallest complete operator surface against the current Engine first.
Additional Engine implementations are separately certified additions. Keep the
public operator contract independent of Playwright, Camofox, and Camoufox so
another conforming tool can use it without becoming a Runtime dependency.

Define these core operations: attach/list/open/close pages; navigate and history;
inspect bounded DOM/accessibility snapshots; locate by role, name, label, and
text; select frames; click, type, select, scroll, drag, and keyboard input;
wait for meaningful page events; capture authorized screenshots; handle dialogs,
popups, uploads, and downloads; request trusted approvals; detach and hand off.
Advertise optional operations such as page script evaluation and network
inspection with their required authority.

**Acceptance:**

- A shared workflow suite passes through the human UI, a native agent client,
  the Playwright adapter, and the declared Camofox/Camoufox integration paths.
  It covers navigation, forms, frames, popups, scrolling, files, approval,
  reconnect, and close. Each result records client, adapter, and Engine versions.
- A human hands an existing page to an authorized agent and takes control back.
  Page identity, profile, cookies, selected services, and pending work remain
  consistent. An explicit control lease orders writers; readers need inspection
  authority. Revocation stops that operator's new actions.
- Headless operation works without an open viewer. A permitted viewer can attach
  later to the same session. Detaching an operator and closing a page have
  distinct, tested lifecycle effects.
- Element references carry page/document generations. Navigation invalidates
  stale references; stale clicks fail clearly. Snapshots have bounds and
  pagination. Events and condition waits replace fixed sleeps in the test suite.
- Equal grants yield equal permission decisions for equivalent human and agent
  operations. Agents use delegated identities with action, profile, duration,
  destination, and quota limits. Approval requests reach the trusted approval
  surface and return structured pending/approved/denied/expired results.
- Adapter compatibility is versioned and explicit. Unsupported protocol methods
  return a capability error. A new conforming operator can complete the example
  workflow using documented APIs without changing Runtime core.
- Inspection and screenshots retain their capability checks. Runtime-scoped
  automation keeps provider debug endpoints private, and product viewing
  continues to use WebRTC. Effectful actions with an uncertain outcome return
  that uncertainty and a reconciliation handle instead of blind replay.

**Compatibility basis:** Playwright distinguishes its own connection protocol
from Chromium-only CDP, which has lower fidelity. Its native connection requires
compatible client/server major and minor versions. Choose and test the adapter
protocol before promising API coverage.
[Playwright BrowserType](https://playwright.dev/docs/api/class-browsertype).
Camoufox documents a Firefox Playwright remote server and marks that server
experimental; this is a separate provider/version qualification task.
[Camoufox remote server](https://camoufox.com/python/remote-server/).
Camofox wraps Camoufox with a REST API, including accessibility snapshots and
element references. Preserve these useful operator semantics while mapping
authority and lifecycle to Runtime.
[Camofox project documentation](https://github.com/jo-inc/camofox-browser/blob/master/README.md).

<a id="b05"></a>
## B05. Input and navigation feel like a browser

**Owner:** Browser input and Engine page adapter.

**Instructions:** Replace trailing gesture delay with bounded frame/time-based
batching. Preserve horizontal and vertical movement, units, accumulated distance,
and ordering. Apply backpressure by combining replaceable motion while retaining
keys, button transitions, and other discrete actions. Handle focus, composition,
clipboard grants, viewport scaling, and touch through the same page contract.

**Acceptance:**

- The continuous-wheel regression emits movement during the gesture, with
  p95 dispatch delay at most 32 ms. Vertical, horizontal, diagonal, pixel/line
  units, small deltas, and long gestures preserve expected movement.
- Keyboard modifiers, held keys, focus loss, text composition, Unicode paste,
  selection, drag, touch, back/forward, reload, stop, and address submission pass
  the manual and automated corpus on each applicable viewer.
- Input ordering survives congestion without stuck keys, stuck buttons, or an
  unbounded queue. Disconnected input has an explicit outcome and stale input
  is discarded on reconnection.
- Input-to-visible-response meets the B11 budget on a controlled page. Agent
  semantic actions produce the same page results as equivalent human actions.

<a id="b06"></a>
## B06. Sessions recover from normal interruptions

**Owner:** Runtime session lifecycle and Engine/display adapters.

**Instructions:** Use one authoritative session state machine with bounded
acquisition, active, degraded, reconnecting, closing, and terminal states. Make
status observations distinct from close commands. Separate viewer connectivity
from Engine ownership. Use leases, bounded retries with jitter, cancellation,
and explicit terminal causes. Reuse Runtime lifecycle records.

**Acceptance:**

- One failed heartbeat or status fetch preserves a valid page and starts the
  appropriate recovery path. The current cleanup regression is covered.
- A five-second network interruption and a viewer reload restore the same page
  within five seconds of healthy transport, with the same profile and service
  selection. Tests cover Wi-Fi changes, background tabs, sleep/wake, Engine
  crash, provider restart, and Runtime restart with documented recovery outcomes.
- Longer interruptions respect the authorization lease in B14. Recovery after
  expiry reacquires authority; page execution and network effects follow the
  expired state while the peer is unreachable.
- Concurrent retry, cancel, and late launch completion create at most one
  acquired session. Repeated close converges on one terminal result. Engine
  loss gives a clear restore/reopen choice for recoverable profile state.
- Every nonterminal state has a deadline or an active lease. A website action
  with an uncertain result is reconciled before an operator retries it.

<a id="b07"></a>
## B07. One slow session cannot block the others

**Owner:** Runtime provider bridge and Engine scheduling.

**Instructions:** Move long launches to bounded jobs or otherwise separate them
from short control operations. Retain request/response correlation and safe
cancellation draining. Schedule within declared per-host limits for sessions,
memory, CPU, disk, sockets, and queues. Give close and revocation priority.

**Acceptance:**

- While one launch is deliberately stalled, another active session can receive
  input, status, signaling, and close. Short control responses meet p95 250 ms
  locally or transport RTT plus 250 ms remotely under the normal test profile.
- Cancellation and late responses cannot cross session or principal boundaries.
  Tests include overlapping launches, malformed responses, timeout, and provider
  restart; removing a lock alone is insufficient proof.
- At advertised capacity the resource budget holds. An excess request receives
  a typed busy/queued result with a deadline and cancellation path.
- One hundred open/close cycles return session-owned processes, streams, ports,
  and leases to baseline. Any warm pool has explicit count, memory, and idle
  limits. Orphan reclamation is tested after client and provider failure.

<a id="b08"></a>
## B08. Remote Engine is an ordinary Runtime service

**Owner:** Runtime provider discovery, grants, and transport.

**Instructions:** Give Engine the standard discover, offer, request, approve,
select, invoke, revoke, and close flow. Runtime resolves endpoints and carries
control/media through approved transport. Keep host launchers inside the serving
Runtime. Publish verified readiness, capacity, protocol features, and service
terms. Extend the existing service model instead of adding Browser-specific
peer infrastructure.

**Acceptance:**

- Two fresh Runtimes with independent identities complete Engine sharing through
  normal product controls. The consumer opens and operates a page without SSH,
  copied credentials, manual ports, provider URLs, or local VM installation.
- Same-device, LAN, and supported WAN/NAT paths pass discovery, approval,
  launch, input, media, reconnect, revocation, and cleanup tests.
- Unavailable, full, incompatible, expired, and revoked Engine offers have
  distinct results. Discovery does not grant use; service selection binds the
  session to the approved provider identity.
- Human and agent clients use the same service-selection contract. Remote
  Engine cost and profile exposure remain within the approved policy.

<a id="b09"></a>
## B09. Exit has identical local and remote network semantics

**Owner:** Runtime Net and Exit providers.

**Instructions:** Route website traffic and DNS through the selected authorized
Exit contract. Define behavior for redirects, subresources, workers, WebSockets,
streaming, address families, destination restrictions, and connection closure.
Qualify any advertised UDP/QUIC or website WebRTC support separately. Treat the
Browser display transport and website networking as separate authorities.

**Acceptance:**

- A controlled destination observes the selected Exit identity/address for page,
  subresource, worker, and WebSocket traffic in every B10 topology. DNS capture
  confirms the declared resolver policy.
- Negative tests cover redirects, DNS rebinding, private/loopback destinations,
  direct host sockets, and automation-client proxy/network options. Runtime
  enforces the same granted destination rules on local and remote operations.
- Exit loss pauses/fails affected requests clearly. Retries retain the approved
  Exit; any change follows the operator's selection policy. The Engine's host
  network cannot silently become the replacement Exit.
- TLS certificate errors, stream backpressure, cancellation, idle timeout, and
  half-close behavior match the contract. Revocation closes affected authority
  within B14's bound and cleanup leaves no owned stream behind.

<a id="b10"></a>
## B10. Prove independent placement of UI, Engine, and Exit

**Owner:** Runtime integration and Browser service selection.

**Instructions:** Use the topology matrix below as a required test parameter,
including separate Runtime identities on the same device. Make Engine and Exit
independently selectable by people and agents. Define "this device" relative to
the controlling Runtime in the UI, with explicit service identities in receipts.
Evaluate preapproved defaults before asking an operator to configure anything.

| Controlling Runtime | Engine | Exit | Required case |
| --- | --- | --- | --- |
| A | A | A | Entirely local |
| A | B | A | Remote Engine returning through the consumer's Exit |
| A | A | B | Local Engine with remote Exit |
| A | B | B | Engine and Exit together on a remote Runtime |
| A | B | C | Engine and Exit on independent remote Runtimes |

**Acceptance:**

- All five placements pass the same navigation, input, media, operator, network,
  approval, recovery, and close suite. Run physical LAN and WAN cases, including
  relay-required connectivity; loopback-only proof is insufficient.
- The same installed Browser capsule uses all placements without code edits or
  per-topology configuration files. The viewer can also connect to A from a
  separate supported device/browser.
- Stopping B or C identifies the failed service correctly. Engine and Exit
  grants remain independent, and one service cannot substitute its own authority
  for the other.
- Default selection is automatic within existing trust, locality, profile, and
  cost policy. A change outside that policy requests approval. The operator can
  see and change the selected services without transport details.

<a id="b11"></a>
## B11. Meet measured responsiveness and media budgets

**Owner:** Engine rendering, display transport, and Runtime host adapters.

**Instructions:** Profile before tuning. Measure launch stages, encode/decode,
frame delivery, input, network use, CPU, memory, and idle power. Adapt resolution,
frame rate, codec, and bitrate to viewport, host capability, and transport.
Evaluate hardware acceleration within the supported isolation model. Keep
WebRTC as the product display path.

**Proposed acceptance budgets:**

| Measurement | Local target | Normal WAN target |
| --- | --- | --- |
| Cold launch to first usable frame, dependencies installed | p95 <= 5 s | p95 <= 7 s |
| Warm launch to first usable frame | p95 <= 1 s | p95 <= 2 s |
| Input to visible response on a controlled page | p95 <= 100 ms | p95 <= measured control RTT + 100 ms |
| Recovery after the five-second interruption in B06 | <= 5 s after transport is healthy | Same |
| Continuous test motion | Delivered rate >= 95% of negotiated target; lost/dropped frames < 1% | Same under the normal network profile |

Normal LAN: RTT at most 10 ms. Normal WAN: RTT at most 100 ms, jitter at most
20 ms, loss at most 1%, available bandwidth at least 10 Mb/s. Record both A-B
and B-C paths where used; Engine display and website egress are separate timing
components. A stress profile uses 300 ms RTT, 5% loss, and 2 Mb/s and tests
bounded degradation and recovery instead of normal-profile speed.

**Acceptance:**

- Publish measurements for every certified target at its declared resolution,
  frame rate, concurrent-session capacity, and workload. Use at least 100 cold
  and 100 warm launches for the latency distribution. Installation downloads
  and website server delays have their own measurements.
- Thirty minutes of controlled video/audio and interaction meet the negotiated
  budgets. During active motion, unexplained freezes stay below 250 ms; audio
  interruptions stay below 100 ms and A/V offset stays within 100 ms.
- An eight-hour mixed workload stays within declared memory/CPU/disk limits,
  maintains responsive controls, and returns transient allocations after close.
  Idle and hidden sessions use bounded resources and recover correctly.
- Measurement uses monotonic clocks or calibrated end-to-end probes; timings
  from different machines have a stated synchronization/error method. Save
  machine-readable results and matching manual UX evidence.
- Text remains readable, resize and device scale remain correct, and adaptation
  recovers after congestion. Screenshot/image polling does not substitute for
  accepted product media.

<a id="b12"></a>
## B12. Profiles and user state survive safely

**Owner:** Engine profile provider and Runtime storage authority.

**Instructions:** Separate profile state from executable artifacts and active
sessions. Define ownership, encryption, retention, single-writer rules,
checkpoint/restore, deletion, and supported migration. Support an explicit
ephemeral mode. Moving execution to another provider follows the profile's
approved placement policy; live migration is a separate capability.

**Acceptance:**

- Cookies, history/settings where advertised, local storage, and IndexedDB
  survive ordinary restarts and supported upgrades. Crash tests preserve the
  last committed checkpoint and identify any unrecoverable state clearly.
- Two principals cannot inspect or modify each other's profiles. Concurrent
  writers are serialized or rejected. Agent attachment uses the granted profile
  instead of silently creating a second logged-out session.
- Supported export/import or checkpoint transfer is authenticated, encrypted,
  version-checked, and verified by a restored-session test. Remote placement
  requires the profile's grant. Incompatible Engine formats have an explicit
  supported migration boundary rather than a claim of universal portability.
- Ephemeral sessions remove their profile state on close. Deletion and retention
  cover remote copies, backups, and traces with verifiable completion or a
  visible pending state for an unavailable provider.

<a id="b13"></a>
## B13. Complete daily browser workflows and accessibility

**Owner:** Browser UI, Engine page adapter, and trusted feature providers.

**Instructions:** Maintain a controlled web compatibility corpus plus a small
representative external-site corpus. Cover tab/window lifecycle, forms, login,
files, media, permissions, and wallet/dapp use. Provide a semantic accessibility
bridge for the actual Engine page along with keyboard focus and visible state.
Use Runtime object grants for file transfer and trusted surfaces for sensitive
approvals.

**Acceptance:**

- Redirects, SPAs, frames, popups, cookies, service workers, IndexedDB, downloads,
  uploads, and WebSockets complete through every supported Engine/Exit placement.
  Files arrive at the selected operator's authorized storage and large transfers
  have progress, bounds, cancellation, and integrity checks.
- Audio/video playback, mute, autoplay prompts, fullscreen, resize, zoom, and
  clipboard behavior pass on the supported viewer matrix. Camera, microphone,
  passkeys, DRM, and other optional features are advertised only for tested
  combinations with the necessary provider and platform support.
- Keyboard-only and screen-reader users can operate Browser chrome and supported
  page controls. Focus, accessible names, text entry, zoom, and touch behavior
  pass manual checks. Streaming video alone does not satisfy accessibility.
- Human-to-agent handoff completes a login/approval workflow in the same page.
  Wallet/dapp connect, signing approval, rejection, account change, disconnect,
  and revocation use the trusted Wallet path and the correct page origin.

<a id="b14"></a>
## B14. Preserve authority, privacy, and bounded revocation

**Owner:** Runtime security, Engine isolation, and provider enforcement.

**Instructions:** Bind requests to principal, operator, session, page/profile,
selected Engine/Exit, and scoped capabilities. Authenticate Runtime peers and
encrypt transport. Maintain Engine isolation and review inner browser sandbox
configuration. Explain that an authorized remote Engine processes page content
and any profile state sent to it. Apply minimal diagnostic retention.

**Acceptance:**

- Adversarial tests reject cross-principal page IDs, replayed/expired grants,
  unauthorized inspection, provider substitution, raw debug access, and adapter
  options that bypass Net or storage authority. Website text cannot grant new
  operator permissions or approve a Wallet action.
- Revocation blocks new dispatch at the issuing Runtime immediately. A reachable
  provider acknowledges and enforces it within two seconds under the normal
  network profile. An unreachable provider loses active authority within a
  renewable lease of at most 30 seconds; the enforcing Runtime owns that timer.
  Tests exercise both paths, including browser execution and network effects.
- Suspend, clock changes, partition, and process restart preserve expiry rules.
  Long-lived pages can preserve recoverable private state while expired actions
  remain stopped. Reconnection obtains fresh authority before continuation.
- Audit links meaningful operations to the correct operator and authority
  decision. Credentials, cookies, tokens, typed secrets, and page snapshots are
  absent from default logs; authorized traces have explicit bounds and deletion.
- Isolation and network tests pass with automation enabled as well as disabled.
  Any security exception has a named owner, scope, and release decision.

<a id="b15"></a>
## B15. Updates and repair preserve a working installation

**Owner:** Runtime distribution and Browser maintenance.

**Instructions:** Publish signed, reproducible component manifests that bind
Runtime compatibility, capsule payloads, Engine, helpers, and VM images. Apply
updates atomically with staged health checks. Define safe profile schema
migration and recovery. Assign an owner and response deadline for upstream
browser security updates. Integrate standard OS packaging after the underlying
installation is proven.

**Acceptance:**

- Clean install, upgrade from each supported prior release, interrupted update,
  failed health check, and repair all pass on certified hosts. Running artifacts
  match one compatible manifest after each terminal outcome.
- Profile and identity state survive successful updates. A failed migration
  retains a verified recoverable state; rollback occurs only across a proven
  compatible schema boundary. Retained rollback artifacts have a bounded size
  and explicit expiry/cleanup condition.
- Upstream Engine/CVE monitoring has a maintainer, documented severity-based
  deadlines, and a tested update or feature-disable path. An unsupported browser
  version cannot remain advertised as a healthy service indefinitely.
- Packaging tests cover launcher paths, permissions, origin/passkey policy,
  uninstall behavior, and update delivery. A package format alone does not mark
  an OS or Engine role supported.

<a id="b16"></a>
## B16. Release only from repeatable product evidence

**Owner:** Browser release engineering and target maintainers.

**Instructions:** Turn the initial reproductions into regression tests. Add
installed-product tests, operator conformance, topology tests, fault injection,
and performance runs around the existing Browser gates. Keep behavioral proof
separate from source/text checks. Test on independently provisioned machines,
including a fresh Mac installation of the reported failing source version.

**Acceptance:**

- Every supported device-role row passes B01-B15 criteria that apply to it.
  Missing hardware or remote peers are recorded as unverified coverage; those
  rows wait for evidence before support is advertised.
- The release suite includes 100 lifecycle cycles, cold/warm launch samples,
  the media run, eight-hour soak, all five placements, human/agent conformance,
  capacity limits, mixed-version cases, and network/process fault recovery.
  Required deterministic cases have zero failures. Retried failures remain in
  the record; a clean rerun alone does not explain them.
- Each receipt binds source commit/tree, built and installed hashes, capsule and
  Engine image manifests, OS/device, operator versions, topology, network
  conditions, exact commands, results, and manual UX/media evidence. Public
  records omit private operator paths and credentials.
- The basic repository gates and relevant Browser entropy/smoke gates pass.
  `scripts/browser-objective-audit.mjs` passes with accepted product media and a
  matching manual UX report. Update obsolete audit checks only when behavioral
  evidence still enforces the product requirement.
- A second maintainer can install and repeat the launch, browse, operator
  handoff, remote Engine/Exit, and close workflow from the release instructions.
  The release states exactly which roles, features, and versions passed.

## Completion record for each goal

Close a goal in TASKS only after its review includes the implementing commits,
contract changes, exact acceptance commands, target receipts, measured results,
remaining limitations, and evidence location. Use focused implementation slices
with one authority owner per concern. Existing branch and publication rules in
[AGENTS.md](../AGENTS.md) still apply.
