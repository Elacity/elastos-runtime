# Changelog

All notable changes to the public ElastOS Runtime repository.

## [Unreleased]

### Added
- The unpublished 0.7 source integrates the reviewed protected-content
  lifecycle with collaboration, Home/platform, Wallet, GBA, model-provider,
  Assistant, Library, Marketplace, and player UIUX. The Runtime-owned
  protected-content path remains inactive until installed proof and cutover.
- A bounded read-only protected-content audit verifies source and installed
  artifact parity, private provider declarations, profile and role facts, and
  operator prerequisites. Its redacted receipt reports readiness for active
  proof separately from product readiness.
- Full source-home setup installs one stable Runtime under the platform data
  root and writes an owner-only receipt that binds source identity, Runtime,
  components, capsule metadata, platform, and installation time.
- macOS and Linux source-home restart helpers validate the stable installation
  receipt and exact prior process before replacement. They own one PID file,
  one bounded principal-root rollback, and one atomic restart receipt.
- People now reports Profile readiness explicitly. Passkey registration and
  Profile setup are separate: a valid passkey remains valid when Profile setup
  is not ready, and People directs the person to System Recovery rather than
  hiding protection or Recovery changes behind a profile save.

### Fixed
- Renaming yourself no longer looks like becoming someone else. A person's
  identity is their Profile DID and a rename only advances the revision, but
  the context check compared whole signed Profile documents, so editing your
  own name read as a different authority and your Home began refusing
  incoming messages from contacts who had done nothing. The check now asks
  the question that matters — same Profile DID, and no rolled-back revision
  replaying an older truth — and a newer revision of the same DID is
  accepted as what it is: the same person, saying something new about
  themselves.
- A running Home announces the people who live on it. Presence was
  published by a browser tab on a timer, once per tab, and disappeared when
  the tab closed, so two Homes that were both running showed each other
  Offline. Everything the announcement carries is durable — the principal
  record and its signed Profile — and the Runtime now publishes it for
  every Profile on the Home. The announcement still has a lifetime and is
  still refreshed on the protocol's cadence, but once per Home rather than
  per open tab, and whether or not anyone is looking.
- A running Home receives its owner's messages. Delivery contexts existed
  only while a signed-in browser session held one, so a Home with nobody
  looking at it refused messages from contacts it had already accepted:
  the sender saw "Sending" for minutes, and the queue drained only when the
  recipient opened Chat, which made a retry loop look like the transport.
  The Runtime now registers each Profile on this Home at startup, so
  acceptance rests on the durable relationship and a verified envelope
  rather than on a browser being open. Sending still requires the person's
  session, because sending acts on their behalf. Measured on two Homes with
  no tab open on the receiver: the recipient Runtime accepted it and the send
  settled in six seconds.
- Carrier and direct-message failures keep their cause. Both sides of the
  peer call discarded the underlying error, so a message that would not
  send reported only that it had not sent; the reason now reaches the log.
- Chat windows use the standard window bar. The unified-sidebar chrome hides
  the bar because the app's own left sidebar stands in for it, and Chat was
  opted in while its sidebar was still unbuilt, leaving the window controls
  floating over bare content. That chrome is now opt-in by its class alone,
  and the lone "Shared room" tab no longer renders as a full-width button:
  a selector appears only once a direct conversation exists to switch to.
- Profile creation now fails clearly when the principal root is not protected.
  People opens System Recovery for that prerequisite; Profile setup itself does
  not establish protection, mint Recovery material, or weaken a valid passkey.
- Home state directories are created owner-only. The shell writes into a
  person's root at its first mount, `create_dir_all` follows the process
  umask, and protected writes then refuse the world-readable parent the
  Runtime itself had just made — a Home that had drawn its desktop could
  never save anything protected. Creation now narrows to owner-only inside
  the data directory, and never reaches above it. The check that refuses a
  loose parent at write time is unchanged.
- App windows are visible again. `.window-frame` starts transparent so a
  capsule never flashes white before it paints, and its rule promised a
  fade-in on load with a fail-open behind it, but nothing ever restored the
  opacity: every window since the shell redesign painted correct, fully
  styled, interactive content into an invisible frame, so the desktop, the
  chrome and the title drew while the app never appeared. Load now reveals
  the frame and a 1.5s fail-open reveals it regardless. The launch skeleton
  had the same shape of bug — its rule says it is hidden the moment the
  iframe reveals, and nothing hid it — so a grey square pulsed over every
  app forever; it now hides on that same signal. The passkey smoke's app
  matrix counted DOM nodes, which is markup and not paint, and passed on
  eight invisible windows; it now requires each window to become visible
  within the reveal's own budget, proven by deleting the rule and watching
  it fail.
- A Home session the Runtime will not renew sends a person to the passkey
  gate instead of a signed-looking desktop of empty windows. Boot swallowed
  the refresh failure and trusted a summary that can still read as signed,
  which left no way back in.
- Session refresh rotates the Home cookie while open tabs keep the prior
  mint, so a divergent header/cookie pair is no longer a conflict when both
  tokens verify to the same session authority. Genuine conflicts still fail,
  now as 403 so a client bounces to the gate rather than erroring.
- A first Recovery Kit can be created on a Home that has already rendered
  its desktop. Activation demanded a clean principal root while the offline
  migration path demanded protection that could not exist yet, so any Home
  that had drawn the shell was locked out of protection — and with it
  Profile authority and all of People. Activation now migrates the surviving
  plaintext under its own guard, backing the originals up first.
- Direct messages send under the authority a person's session actually
  carries. The check required the grant to name `chat-room`, but passkey
  sign-in only ever mints grants naming Home and System, so no real person
  could send; the API tests passed only because their fixture fabricated
  grants the product never issues.
- A first run no longer republishes presence it knows will be refused. The
  Runtime answers 409 until a Profile exists, and the heartbeat now stops on
  that answer instead of filling the console every interval.

### Changed
- Product inventory exposes one Chat app. The Runtime-backed `chat-room` app is
  the only Chat entry in manifests and supported release profiles. The
  obsolete terminal Chat and Agent implementations are no longer packaged as
  apps; their retained source is only pending explicit operator-CLI extraction
  or removal and is not a compatibility path.
- Collaboration messages now identify people as Profiles and recipients as
  Profiles or conversations. Runtime derives routing from the currently signed
  Profile instead of putting endpoint identity in the product message. Endpoint
  acceptance receipts remain endpoint receipts, so Chat shows `Sent` rather
  than claiming the person received or read the message. For direct messages,
  contact revocations, and Profile updates, the receiving Runtime now derives
  the remote endpoint from the authenticated Carrier connection. It verifies
  that route independently from the Profile-authorized application signer, and
  rejects caller-supplied or substituted endpoint metadata before persistence.
- Shared-room Chat and presence now carry one strict Profile-authenticated
  payload: `{ product, signed_profile }`. Runtime request binding covers that
  exact envelope, admission verifies the Profile's endpoint and scoped signer
  grants, and product code receives the verified Profile plus only its own
  inner payload. Names and presence therefore come from signed Profile truth,
  not device labels or fields repeated inside Chat and presence payloads.
- The Services peer contact object is named as what it holds. The released
  `people-contacts.json` (schema `elastos.people.contacts-state/v1`) carried
  Services peer data under a People name; it is now
  `services-peer-contacts.json` with schema
  `elastos.services.peer-contacts-state/v1`, and an installed host's object
  migrates whole — name and schema together, through the protected-object
  path since the encryption binds the object URI — on first touch. The
  capsule-facing read-model audit the task list called for came back clean:
  device identity serializes to the System diagnostics surface only, remote
  endpoint DIDs are serialization-skipped, offer routes are app navigation,
  and the one topology field (`source_peer`) is the System trusted-source
  panel's by design. Offline reach limits are now stated in one place for
  all three transports: direct messages end terminal and visible, the shared
  room's gossip buffer bounds catch-up, and Profile updates bridge at most
  the 8-revision announcement ring before failing closed to a fresh
  approval.
- Every request/response collaboration retry now runs on one delivery
  engine. `collaboration_delivery.rs` owns the bounded pass — attempt the
  whole plan, settle only what the selected peer endpoint acknowledged, report the
  first failure without letting one unreachable peer starve the rest — and
  direct messages, contact revocations, and Profile update announcements all
  run on it instead of three hand-rolled loops. What happens at an envelope's
  end of life is now a declared per-source decision pinned beside the code
  that implements it: terminal and visibly expired for messages, re-mint the
  exact signed fact for revocations, regenerate from durable truth for
  Profile announcements. Artifacts stay in the store their authority owns,
  and the shared-room outbox remains the one stated difference (broadcast
  with asynchronous receipts). Revocation delivery failures now propagate to
  the sync scheduler instead of being silently swallowed, so failed passes
  back off like every other delivery. The encrypted mailbox lands on this
  layer instead of adding a fifth loop.

### Added
- Recovery now restores the collaboration identity. The Full Recovery Bundle
  carries the signed People identity — the Profile signing seed with its
  retained revision ring, and the signed contact store — alongside the root
  kit and Wallet keys. Importing it on a genuinely fresh machine keeps the
  same Profile DID, restores the contacts, and authorizes the new device by
  minting the next signed revision through the normal Profile authority path,
  which the existing update-delivery chain then announces to every accepted
  contact: relationships survive the machine. The import response and the
  System recovery UI report the People outcome honestly and never claim a
  complete account restore when the identity did not come back.

### Changed
- Direct conversations are text-only by declared decision. Attachments need
  object handling, retention, and a delivery path of their own — designed on
  the unified delivery layer together with the encrypted mailbox, not as
  another ad-hoc transport — so until then the attach control in a direct
  conversation is visibly disabled and says why, instead of silently missing,
  and the product doc scopes attachments to shared conversations.

### Added
- Home now tells you when someone wants to connect or has messaged you. A
  pending contact request appears in the durable notification store — one
  projection read by the Home badge, the toast, and Inbox alike — and is
  pruned the moment the signed request is decided, revoked, or expired. A
  verified incoming direct message mints one live notification per
  conversation, named by the sender's signed Profile presentation, resolved
  by reading the conversation in Chat and resurfaced by new mail. Inbox stays
  the only Accept/Decline surface: notifications point at it and never carry
  the decision.
- Signed Profile update delivery now works between runtimes. A rename (or an
  authorized-device change) travels to accepted contacts as an exact bounded
  signed revision chain over the dedicated `collaboration-profile` Runtime
  provider, and the receiving store applies it only under the strict rules:
  an already accepted Profile DID, the next exact revision bridged by the
  exact previous signed-envelope hash, a delivery envelope signed by an
  application signer the new head authorizes, and delivery from the endpoint
  the new head names — rollback, gaps, conflicts, mixed profiles, and
  unauthorized signers or endpoints fail closed. Announcement is a pure function of the
  local head and the accepted contacts, so a restart simply re-announces and
  the receiver treats it as an idempotent replay. The two-runtime proof
  surfaced a real wire gap: the Carrier peer provider plane's allow-list had
  never admitted the profile provider, so no announcement had ever been
  deliverable; it now is.
- Shared-room names now come from signed Profile truth. Every configured-room
  participant and message row with a verified member device is named from the
  polling principal's own signed Profile, an accepted (or retained removed)
  contact head, or a room membership profile card — and a verified device none
  of those name renders as an explicit "Unverified device" row instead of a
  stored `Device {hex}` label, a presence self-claim, or an invented
  "Conversation member N" placeholder. Presence heartbeats stay liveness-only,
  durable conversation history is untouched, and the plain (unconfigured) room
  keeps its server-stamped home-session and guest names.
- Bilateral signed contact removal, with the complete People relationship
  states. Removing a contact mints a pair-scoped signed revocation: removal is
  immediate locally, the Runtime durably retries the exact envelope until the
  peer's device acknowledges it (re-minting only after an envelope's own
  lifetime), and the peer's side verifies it against the pair's accepted
  authority. Both sides keep the relationship visible in a removed state —
  named by the retained signed Profile head — instead of letting it vanish.
  Removed permits reading only: history stays readable under an explicitly
  declared policy, sending and receiving stop on both sides, and a fresh
  request through Inbox reopens the pair. People now also projects `requested`
  and `declined` from the signed request and decision chains, and the store
  binds retained Profile heads to known relationships only.
- People shows whether an accepted contact is reachable right now. The signal
  is the existing presence heartbeat projected onto the contact list — Online
  now / Offline when presence has an answer, nothing when it has no basis —
  and it stays out of the realtime signature so heartbeat flaps do not emit
  people.changed events.
- The system bar carries the focused app's name and that app's own menus. Apps
  declare them with a postMessage manifest and receive each chosen command
  back; an app that declares none gets File with Close Window. Manifests are
  data, not authority: the host takes one only from an app frame, binds it to
  that frame's launch token, and never accepts a window id from the sender.
- Spotlight, opened with Cmd/Ctrl+Space, searches open windows, installed apps
  and desktop objects together. It reads only what the shell already holds, and
  opens objects through the same capability guard the desktop uses.
- The shell keyboard layer: Alt+Tab window switching, Cmd+` cycling, Cmd+W /
  Cmd+M close and minimize, Cmd+Alt+arrow snapping, F3 for Mission Control,
  Ctrl+arrows to switch Spaces, and Cmd+/ for a shortcuts overlay that
  documents exactly what exists. Quick Look arrives with it: Space on a
  selected desktop icon opens a frameless preview of glyph and metadata —
  never object bytes, which stay behind viewer authority — and Open routes
  through the same guarded path the desktop uses. The layer sleeps whenever
  Home GUI is not the mounted shell.
- Stages, Desktop Spaces and Mission Control. The green button gives an app its
  own fullscreen Space, extra Desktops can be added, and Mission Control lays
  the Spaces out as live thumbs over the open windows — click to preview, click
  again to enter, drag to reorder. Sessions restore the Space you were on.
  Presentation only: a Space switch mints no authority.
- Capsules declare their own app icon. A capsule names an icon directory in its
  manifest, the Runtime resolves it to that capsule's own asset routes, and the
  shell renders what it is handed instead of keeping a central icon table keyed
  by capsule name. Capsules that declare none get the shell's generic glyph.
  People, Chat Room and Inbox ship their own icons.
- One canonical source for the shared UI tokens, theme runtime and accent
  picker, stamped into each participating capsule by `just vendor-ui` and gated
  against drift in `just verify`. Per-capsule origins make the copies necessary;
  the gate makes them identical.

### Changed
- The local device DID reaches exactly one browser surface: System, the
  runtime-inspection page. The Home and People read models and the People
  profile-update response no longer serialize it — no other surface consumed
  it, and device identity stays out of app-facing projections.
- The People contact read model drops its dead `route` field. Nothing read it,
  the Profile path always left it empty, and route strings do not belong in
  capsule read models.
- The Home GUI shell takes the redesigned look for the surfaces it already has:
  desktop, dock, windows, launcher, toolbar and notification toast. It loads the
  shared token sheet, so it follows theme and accent like the app capsules.
- People, Chat Room and Inbox take every colour from the shared token sheet
  rather than their own palettes, so all three follow theme and accent. Only the
  colour source moved; their layouts are unchanged.
- The shared theme runtime owns no browser-profile storage. It keeps an
  in-memory view and accepts a persistence adapter that only a host installs,
  which lets the opaque Home GUI take the shared tokens without violating the
  rule that capsule frames do not own that storage.
- People's sidebar behaves as the tabs it looks like: sections switch and the
  page title names the visible one, replacing a scroll between two
  always-visible sections.

### Fixed
- A contact that never acknowledges can no longer wedge the direct message
  store. Abandoned records — expired with no receipt — are terminal for
  retention: settled pairs prune first, abandoned records yield next, and only
  a store made entirely of live unexpired messages refuses a write as honest
  backpressure.
- A direct message abandoned at its 24-hour TTL now reads `expired` instead of
  `pending`, and the Chat UI shows "Not delivered" instead of "Sending". The
  Runtime already stopped retrying at expiry; the read model and UI now say so.
  A settled receipt still wins: an acknowledged message stays Sent even
  when read after the TTL.
- Home summary reads are side-effect free. Reading the Home summary no longer
  materializes or rewrites the shared-room store or the notification store; a
  read now answers from an in-memory expiry view and persists only a real
  change.
- Chat Room text on an accent fill uses the accent's own ink colour instead of
  hardcoded near-white, which was unreadable on the yellow and graphite
  accents.
- People no longer presents a contact state it does not recognise as
  "connected". The Runtime emits two relationships today; anything else now
  renders as an unknown state, so a build that has not learned a new state
  cannot tell someone they are still connected to a person they may not be.
- The Home GUI dock draws its Apps launcher glyph again. It had been pointed at
  an icon file the shell does not ship.

## [0.6.0] - 2026-07-31

0.6.0 combines the reviewed ESP line with the Wallet, Recovery, and Browser
continuation. These changes are not part of the 0.5.0 release history.

### Changed
- Added the ESP v0 descriptor, Runtime-derived capsule/interface catalog, and
  fail-closed shell selection for the allowlisted Home GUI and Home CLI shells.
- Split `/apps/home/` into a neutral host, trusted graphical shell modules, and
  an isolated Runtime-owned Home CLI PTY while keeping both shells on the same
  catalog, intent, approval, and provider facts.
- Replaced first-party WASI product entrypoints with explicit Runtime
  projections and added a test-only `elastos.component/v1` conformance fixture
  for the `elastos:bus@v1` authority path.
- Added locked, isolated, path-remapped Component artifact builds and made
  setup and release packaging use that same build path.
- Restored the GBA viewer/content relationship as a portable browser projection
  and tightened Browser audio/session diagnostics without adding a fallback
  rendering path.
- Added canonical capsule authoring templates, manifest/interface checks, and
  active-product inventory gates.
- Extracted People from Home into a standalone first-party app capsule with an
  app-scoped launch token while keeping profile, discovery, requests, contacts,
  removal, and Chat handoff Runtime-mediated.
- Added signed Home launch-token v4 authority, the typed Wallet Bus v2.3
  boundary, durable exact-intent passkey step-up, managed Wallet recovery, and
  Runtime-owned transaction effects that reconcile without rebroadcasting.
- Added the macOS VZ Browser product path with Runtime-only networking,
  WebRTC display, explicit Browser Engine and Exit selection, principal-scoped
  profile ownership, and durable page/VM cleanup ownership.
- Added a Runtime-mediated injected-wallet bridge. Deterministic localhost proof
  confirmed `window.ethereum`, EIP-6963 discovery, the Runtime Wallet binding,
  and one `eth_requestAccounts` call creating one pending account-access request.

### Fixed
- Bound `browser-local-exit` to the lifetime of the Runtime that launched it via
  a held-open stdin pipe, so it no longer survives as an orphan when the Runtime
  is SIGKILLed, aborts on panic, or leaves through `std::process::exit` (the
  installed-binary supersession watch takes that path on every rebuild). Helper
  teardown is now scoped by inode identity to the relay socket it bound, and the
  Runtime refuses to replace a relay socket a live helper is still serving
  instead of stranding it on an unlinked socket.
- Fixed Home launch classification so browser projections are attached as
  authorized web surfaces instead of being sent to a WASM compute provider.
- Bound fresh passkey authority to one app, operation, and request payload;
  made Inspector approval claims atomic; and made persisted audit records
  Runtime-signed and hash-chained.
- Isolated shell and app frames with opaque browser sandboxes, removed
  ambient-cookie token minting, trusted Host-header callbacks, and manifest-only
  shell admission.
- Made passkey session refresh atomically revoke its predecessor, restricted
  first-owner enrollment to local Runtime access, and removed unauthenticated
  capsule bootstrap plus obsolete standalone Recovery Kit routes.
- Rejected archive links and host-path escapes during capsule install and
  browser asset serving.
- Bound Browser Wallet reads and approvals, exact EVM signing outcomes, managed
  approval authority, and recovery state to the accepted Wallet contracts.
- Made Browser close and authority renewal acknowledged and retryable, retained
  exact Runtime cleanup ownership across interruption, gated the deterministic
  close handshake in `just verify`, and removed a delayed terminal Home window
  after the exact close result.

### Known limitations
- Browser is included in 0.6.0, but is not claimed fully reliable. The accepted
  localhost proof covers launch, display, navigation, and injected account
  access; it is not a general product-readiness claim for every target or site.
- Browser restart remains intermittent: one observed restart failed and the
  next opened. The `ela.city` login did not survive the restart, and current
  Browser performance is slow.
- Browser profile storage remains principal-owned and reset-scoped, but is not
  yet protected or Recovery Kit-recoverable.

### Deferred
- Carrier reconciliation and physical multi-node evidence are deferred to 0.7.
- The shell UI redesign and extended AI UI work are deferred unless reviewed as
  independent post-release changes; they are not part of this 0.6.0 code tree.

## [0.5.0] - Unreleased

0.5.0 was the baseline before the 0.6.0 release. It brought the Mac, Jetson,
and server work into one line while keeping Browser readiness claims tied to
target-device media, audio, input, and installed-path proof.

Notes from the unpublished intermediate patch line are folded into this entry
because no separate patch release was published.

### Added
- Added the first Services app so local services start disabled, can be enabled
  intentionally, and remote services from trusted people are shown separately.
- Added opt-in People discovery, `elastos://peer/invite?...` pairing, trusted
  people, display names, and the 1:1 chat path while moving service management
  out of People.
- Added service registry flows for enabling local services, requesting access to
  services from trusted people, and approving remote Browser Exit use.
- Added Browser Engine and Exit selection in Browser so non-KVM hosts can serve
  the Browser UI and use approved remote engines instead of pretending to be
  local VM providers.
- Added Browser Session Manager capacity receipts, page ownership checks,
  heartbeats, stale-session cleanup, and fail-closed page control.
- Added Mac VZ and Linux crosvm Browser VM target support, including WebRTC-only
  display, VM artifact preflight, target refresh, runtime relay setup, active
  target audits, and remote VM launcher support.
- Added Runtime-owned Browser relay helpers for WebRTC and private stream
  wiring, including TURN setup when `turnserver` is available.
- Added Browser profile-disk descriptors and principal-owned reset handling,
  with the storage truth documented as reset-scoped but not yet encrypted or
  recoverable.
- Added Inbox fresh-passkey approval for built-in Wallet requests while keeping
  Wallet as the review surface for wallet authority.
- Added the Inspector approved-provider dispatch surface with fresh passkey
  approval and typed provider invocation boundaries.
- Added public install and release proof helpers for component checksums,
  publisher bootstrap integrity, source-home setup, source-home restart, and
  installed-path smoke testing.
- Added the public `AGENTS.md` operator contract and onboarding docs for review,
  setup, target Browser proof, People conversations, and Inspector testing.

### Changed
- Reworked Home authority boundaries around passkey principals, launch tokens,
  localhost `Users/self` mapping, protected content providers, Browser page
  ownership, and sanitized provider responses.
- Removed Browser `runtime_frame`, `diagnostic_frame`, screenshot, and
  image-polling fallbacks from the product path. Browser launches now use
  WebRTC remote display only.
- Clarified the server role on non-KVM hosts: serve Home and Browser UI, then
  delegate Browser execution to approved remote engines when needed.
- Hardened Browser VM preflight and setup so stale Unix sockets no longer count
  as launch-ready and gateway-only hosts keep remote VM wiring instead of being
  overwritten with unusable local crosvm defaults.
- Hardened Browser Wallet and dapp bridge behavior so Wallet reads, prepare,
  signing, and broadcast stay Runtime-mediated and reviewed through
  Wallet/Inbox.
- Cleaned the System, People, and Services app surfaces so first-party app
  navigation is quieter, People focuses on people, and Services focuses on
  manageable services.
- Aligned the default publish/setup profile with the current Home surface,
  including System, People, Services, Browser, Wallet, Documents, Library,
  Marketplace, Archive Manager, and Inbox.
- Tightened release publishing so `elastos publish-release` uses the explicit
  `home` profile by default, demo-only capsules stay behind the `demo` profile,
  and first-party artifacts require sha256 or sha512 checksums before signing or
  serving.
- Hardened public-install smokes so they pin the installer-selected components
  manifest and fail unless the selected gateway serves a 0.5.0-compatible
  `home` profile with checksummed artifacts.
- Hardened trusted-source Carrier bootstrap stamping so installer refresh and
  release publishing require one publisher-scoped ticket/node pair.
- Removed private staging, reconciliation, and local-machine proof wrappers from
  the public script surface. Public proof now names reusable gates and explicit
  target checks instead.
- Updated release, install, Mac, Browser VM, People, Services, Inspector, and
  architecture docs so proof claims distinguish source gates, installed-path
  checks, target-device proof, and Browser product acceptance.

### Fixed
- Hardened Home app launch materialization so release-installed app bundles can
  be refreshed from signed package metadata, stale bundles missing their
  declared entrypoint are detected before launch, and source/dev launches remain
  local when no release package identity is present.
- Added macOS ARM64 setup metadata and Home runtime transport fixes so managed
  Home can reuse a live local runtime, route WASM carrier calls through the host
  runtime when FIFO bridge transport is unavailable, and avoid Linux `/proc`
  process-liveness assumptions.
- Cleaned the capsule catalog count construction so strict workspace clippy
  gates pass with `-D warnings`.
- Fixed Browser VM supervisor autostart smoke cleanup so fake launchers are
  reaped after each run instead of leaving orphaned `node -` processes.
- Fixed public install identity smoke permissions so the documented installed
  DID/profile proof reaches the publisher check instead of failing locally with
  `Permission denied`.
- Fixed public installs to refresh trusted-source metadata from the live
  Publisher Carrier bootstrap route instead of relying on stale publish-time
  peer tickets.

### Verification Status
- Source/review proof should cite concrete commands: `git diff --check`,
  `node scripts/home-entropy-check.mjs`,
  `node scripts/browser-entropy-check.mjs`,
  `bash scripts/check-wci-alignment.sh`, `just candidate-command-audit`, and
  touched Rust or capsule tests.
- Auth, wallet, chain-authority, Inspector, Services, People, and Browser wallet
  bridge smokes are part of the required review proof set.
- Target Browser VM parity proof remains operator-supplied. Use
  `scripts/jetson-browser-runtime-audit.mjs` with explicit target host, user,
  data directory, and source checkout arguments.
- Public install proof still requires a staged or published 0.5.0-compatible
  manifest with the current `home` profile and checksummed artifacts, followed
  by `scripts/public-install-identity-smoke.sh` and
  `scripts/public-install-home-frontdoor-smoke.sh`.
- Product Browser completion is not claimed until
  `scripts/browser-objective-audit.mjs` has accepted product media proof plus
  hash-bound manual UX evidence.
- Manual installed-device checks on Mac and Jetson are still required before a
  release handoff: `elastos setup`, open Home, visit first-party apps, and
  return Home cleanly on the installed path.

## [0.4.0] - 2026-06-09

### Added
- Added a provider-backed Library release slice: desktop-familiar
  places, breadcrumbs, grid/list views, inline create/rename, drag/drop,
  preview/open, upload/download, folder and selected-object archive download,
  provider-created ZIP objects, safe `.tar`/`.tar.gz`/`.tgz`/`.zip`
  extraction, publish/share/status/repair, Trash, browser Back/Forward
  takeover, and desktop-style context menus now run through Runtime-scoped Library
  provider authority.
- Added `object-provider` as the mutable principal-root object provider for
  Library files/folders, Desktop/Documents/Public roots, protected-root object
  envelopes, object events, legacy plaintext auto-protection, and WebSpace
  resolver routing. Runtime registers the canonical `object` provider scheme,
  and browser calls use `/api/provider/object/*`.
- Added Library-to-Documents viewer handoff and Home Desktop projection so
  concrete Library objects can open in Documents and Desktop file mutations stay
  provider-owned while Home only displays and launches them.
- Added the current Spaces/WebSpace contract for Library:
  `localhost://WebSpaces/*` is the local mounted resolver view shown as
  **Spaces**, provider targets such as `cloud://drive/...` remain
  resolver-private, read-only mounts hide mutable actions, mutable mounts/forks
  can materialize provider-owned objects, and `elastos://content/*` remains the
  provider-independent published/shared content surface.
- Added the first Carrier-backed content availability proof path for Library
  publish/status/repair: `content-provider` signs availability receipts with
  peer-selection, quota, repair-worker, accounting, and abuse-control metadata;
  built-in Carrier availability can invoke remote service providers for
  `content/ensure`, `content/status`, manifest-backed object import, and
  exact-CID byte fallback without exposing raw Carrier tickets, Kubo/IPFS, or
  peer handles to apps.
- Added Runtime provider-to-provider invocation foundations, including typed
  invocation envelopes, transfer receipts, range/progress propagation, a
  validated bounded `ProviderTransfer::Stream` base64-chunk envelope, and
  service-provider-only Carrier `provider_invoke` transport.
- Added Runtime-native provider stream sessions on top of
  `ProviderTransfer::Stream`: providers can be opened as read/cancel sessions
  with live progress events and read-next backpressure, `content-provider`
  fetch consumes local IPFS and availability fallback reads through that
  session path, and Library object downloads now return chunked HTTP body
  streams with backpressure/cancel transfer receipts.
- Added the WebSpace federation slice: `operator-drive-adapter` now has an
  operator-private endpoint backend contract with redacted config/status,
  provider-to-provider invocation boundaries, durable cache/viewer handoff
  coverage, mutable fork write-back, and a filesystem-backed endpoint proof for
  metadata traversal, byte reads, and mutable writes.
- Added recipient-scoped Library share-grant records, `shared_access` checks,
  access-decision/shared-open receipts, and fail-closed key-release policy
  handling as the current local/provider-mediated sharing slice.
- Added the protected recipient receipt-chain proof: Library can publish a
  non-production `protected_content_fixture` sealed-object descriptor through
  `content-provider`, record recipient-scoped key-release grants, bind
  `shared_access` to Runtime recipient proof plus launch `session_id`, invoke
  DRM/rights/key/decrypt providers, return a viewer-scoped protected-open
  contract, and fail closed when protected-content providers are absent without
  exposing raw CEKs, plaintext, wallet, chain, Kubo, host filesystem, or
  provider credentials to apps.
- Added durable storage accounting: `content-provider` projects
  signed availability receipts into a persistent per-principal ledger, exposes
  storage-accounting summaries and no-settlement storage-market posture through
  `content/status`, and preserves original publisher identity when unpublish is
  called without explicit principal metadata.
- Added principal storage-quota admission for content publish/import:
  `availability_requirements.max_storage_bytes_per_principal` is enforced from
  the durable content accounting ledger before local content-backend writes and
  recorded as `principal_storage_quota` posture in receipts/status.
- Added bounded cross-provider content admission preflight: `content-provider`
  exposes provider-only signed `elastos.content.admission/v1` receipts, and
  Carrier invokes and verifies remote `content/admission` before
  `content/ensure`, exact/object import, or block-graph repair transfer so
  unsigned admission, payload mismatches, and quota rejections fail closed before
  bytes move.
- Added an optional storage-market endpoint-quorum admission gate for `content/admission`:
  `ELASTOS_CONTENT_STORAGE_MARKET_ADMISSION_*` can point content-provider at one
  operator-owned admission endpoint or a bounded configured endpoint set with an
  explicit quorum, and accepted/rejected market decisions are normalized into
  the signed admission receipt before Carrier moves bytes or DAG repair data.
  Credentials are redacted from status, and endpoint or quorum failure rejects
  admission fail-closed.
- Added optional external repair-fleet dispatch for the Runtime-gated
  `content repair-worker`: `ELASTOS_CONTENT_EXTERNAL_REPAIR_FLEET_*` can point
  content-provider at one operator-owned dispatch endpoint or a bounded endpoint
  set with an explicit quorum, due tasks are sent as
  `elastos.content.external-repair-fleet.dispatch-request/v1`, replies are
  normalized into dispatch receipts with endpoint receipts plus quorum counters,
  and credentials are redacted while local provider verification still decides
  final availability.
- Added bounded Carrier repair-graph policy receipts: current cross-peer repair
  explicitly supports object-manifest and exact-byte import fallbacks while
  refusing arbitrary IPLD DAG fallback unless the Runtime-only block-graph
  provider ABI is available.
- Added the block-graph provider path: Runtime reserves
  `elastos://block-graph/*`, the build/release surfaces include a
  `content-block-graph-provider` contract capsule, startup registers the
  verified provider when installed, and Carrier routes arbitrary `ipld_dag`
  repair through local `export_graph` plus remote `import_graph` provider-plane
  invocations. The provider uses the `ipfs-provider` Kubo coord file to export
  and import bounded DAG CAR bytes, pins imported roots, and fails closed when
  Kubo/provider setup is absent.
- Added `elastos content status` for operator inspection of provider-wide or
  per-CID availability, storage-accounting, quota, repair, peer-proof, and
  no-settlement storage-market status through Runtime provider invocation.
- Added `elastos.content.repair-fleet/v1` status receipts to content dashboard
  and repair-worker runs so operators can inspect the current single-runtime
  provider-owned repair coordinator/worker policy, ledger-based due scheduling,
  task pressure, and explicit non-production external-fleet/settlement posture.
- Added `elastos.content.network-abuse-policy/v1` status receipts to content
  dashboard, per-CID status, and repair-worker runs so operators can inspect
  provider-owned local guardrails, Carrier candidate caps, admission preflight
  posture, repair-worker budgets, configured abuse-control endpoint-quorum
  exchange posture, and explicit non-production network-wide
  throttles/banlists/abuse-ledger posture.
- Added `elastos.content.operator-dashboard/v1` to provider-wide content status
  so operators can inspect provider-local storage pressure, top principals,
  replica-byte estimates, quota-exceeded records, fleet-history attempts, recent
  repair rows, live-proof counts, and explicit non-production federation posture.
- Added `elastos.carrier.peer-reputation/v1` policy/status metadata to Carrier
  peer selection and content status so local Runtime success/failure scoring is
  visible while signed cross-runtime reputation remains explicitly not
  configured.
- Added `elastos.carrier.peer-attestation-exchange-policy/v1` metadata to
  Carrier peer selection, redacted remote receipt summaries, content proof
  summaries, and operator dashboard surfaces so signed availability
  announcements, verified remote content receipts, remote provider proofs, and
  local Runtime reputation are distinguished from unconfigured signed
  cross-runtime reputation receipts, third-party attestations, trust-policy
  exchange, and revocation.
- Added opt-in Carrier peer-attestation exchange: when
  `ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_*` is configured,
  `carrier-availability` posts a signed
  `elastos.carrier.peer-attestation.exchange-request/v1` with redacted remote
  proof summaries to one operator-owned exchange endpoint or a bounded endpoint
  set with an explicit quorum, requires accepted endpoint responses to include
  signed `elastos.carrier.peer-attestation.exchange-receipt/v1` receipts,
  verifies receipts before marking configured quorum accepted, records endpoint
  receipts plus quorum counters, and keeps connect tickets plus endpoint
  credentials out of app-visible proof surfaces.
- Added `elastos.content.storage-settlement-policy/v1` metadata to local,
  Carrier, ledger, per-CID, and provider-wide storage-market status so pricing,
  escrow, payment settlement, SLA enforcement, storage-market admission, and
  cross-provider escrow are explicit non-production policy state.
- Added `elastos.content.storage-market-admission-policy/v1` metadata to local,
  Carrier, ledger, per-CID, provider-wide, and operator-dashboard
  storage-market surfaces so local quota admission and remote
  `content/admission` preflight are distinguished from unconfigured production
  provider-admission networks, offer receipts, price discovery, SLA admission,
  and economic abuse controls.
- Preserved proof metadata through the standalone `availability-provider`
  capsule so configured external availability targets can report
  `storage_market`, `repair_graph`, and `abuse_controls` on the same
  Runtime-validated contract as built-in Carrier availability, with explicit
  no-market / target-report-only defaults when a target omits them; configured
  target fanout can now satisfy min-replica/live-proof requirements without
  bypassing max-replica quota.
- Added `elastos.content.federated-quota-ledger-policy/v1` metadata to local,
  principal storage-quota, Carrier quota, remote receipt, per-CID, provider-wide
  status, and operator dashboard surfaces so local per-principal ledgers and
  signed remote admission receipt exchange are distinguished from configured
  signed federated quota-ledger endpoint-quorum exchange and production
  quota-receipt exchange.
- Added opt-in federated quota-ledger exchange for remote admission preflight:
  `content/admission` can post a signed
  `elastos.content.federated-quota-ledger.exchange-request/v1` to one
  operator-configured endpoint or a bounded endpoint set with an explicit
  quorum, require signed
  `elastos.content.federated-quota-ledger.exchange-receipt/v1` receipts for
  accepted endpoints, record endpoint receipts and quorum counters in the signed
  admission receipt, and reject fail-closed on configured quorum failure,
  malformed signed receipt, timeout, or transport failure without exposing
  endpoint credentials.
- Added opt-in federated abuse-control exchange for remote admission preflight:
  `content/admission` can post a signed
  `elastos.content.federated-abuse-control.exchange-request/v1` to one
  operator-configured endpoint or a bounded endpoint set with an explicit
  quorum before quota-ledger, storage-market, byte-transfer, or repair-graph
  movement, require signed
  `elastos.content.federated-abuse-control.exchange-receipt/v1` receipts for
  accepted endpoints, record endpoint receipts and quorum counters in the signed
  admission receipt, and reject fail-closed on configured quorum failure,
  malformed signed receipt, timeout, transport failure, or receipt verification
  failure without exposing endpoint credentials.
- Added `elastos.content.external-repair-fleet-policy/v1` metadata to
  provider-wide status, repair-worker runs, and operator dashboard surfaces so
  the local provider-owned repair worker is distinguished from unconfigured
  external coordinators, volunteer/supernode workers, cross-provider queues,
  worker attestations, settlement, and repair SLAs.
- Added `elastos.content.federated-operator-alerting-policy/v1` metadata to
  provider-wide status and operator dashboard surfaces so provider-local status
  JSON, storage pressure, repair-task pressure, live-proof counters, and
  remote-receipt counters plus configured provider-local alert sink and
  configured federated alert-exchange posture are distinguished from production
  cross-provider dashboards, peer-health subscriptions, fleet-wide SLA policy,
  and operator UI.
- Added opt-in operator alert delivery for content availability: provider-wide
  `content/status` can emit a durable
  `elastos.content.operator-alert.receipt/v1` outbox entry, post an
  `elastos.content.operator-alert/v1` payload to one HTTPS or loopback sink
  when `ELASTOS_CONTENT_OPERATOR_ALERT_*` is configured, and deliver a typed
  `elastos.content.federated-operator-alert.exchange-request/v1` to one
  operator-owned exchange endpoint when
  `ELASTOS_CONTENT_FEDERATED_OPERATOR_ALERT_EXCHANGE_*` is configured. The
  normalized
  `elastos.content.federated-operator-alert.exchange-receipt/v1` is recorded
  beside the provider-local sink result without exposing operator credentials to
  apps or status JSON.
- Added runtime authority primitives for proof-bound authentication: principals, proof bindings, SIWE challenges, session grants, and audit events.
- Added EVM SIWE challenge, verify, and revoke gateway routes that bind verified wallet proofs to runtime principals and issue scoped Home/System launch grants.
- Added `chain-provider` as the first blockchain provider capsule: typed `elastos://chain/*` access for Essentials-compatible Elastos networks without exposing raw RPC URLs to app capsules.
- Added wallet approval approve/complete states, Inbox review entries, WebConnect-style handoff metadata, and signature receipt hashes without exposing wallet signatures to app capsules.
- Added passkey-controlled built-in EVM wallet creation through Wallet, with provider-owned encrypted key storage and managed signing only after Wallet/Inbox review plus fresh passkey confirmation in Wallet.
- Added fresh passkey-bound gates for built-in wallet signatures, Wallet send, account deletion, and Wallet recovery-key export/import so long-lived app launch tokens cannot execute managed-wallet authority by themselves.
- Bound managed-wallet private-key envelopes to principal/account/chain/address metadata using AES-GCM AAD and principal-derived storage keys, with tamper tests for cross-principal/chain metadata changes.
- Removed internal wallet storage object paths from wallet-provider init/status responses; consumers now see only configured booleans and counts.
- Hardened `wallet-provider` request parsing so hidden signing authority, connector wallet objects, and extra wallet fields fail at decode time.
- Added a dedicated `wallet-metamask` connector capsule for MetaMask SIWE linking and external wallet approval completion, keeping browser wallet authority out of System.
- Added connector-bound external wallet links and approval completion so EVM wallet proofs and signatures can only be finished by the dedicated connector capsule that owns the account.
- Generalized external wallet connector approval/account routes behind an allowlisted connector-capsule path, keeping MetaMask working while preventing unknown connector capsules from receiving wallet handoffs.
- Added PC2 convergence documentation and provider-resource tests that translate PC2 wallet bridge method classes into Runtime wallet/chain scopes while rejecting raw EIP-1193 methods as provider operations.
- Added typed chain proof, EVM transaction prepare, signed transaction broadcast, and node lifecycle status scopes to `chain-provider`.
- Hardened `chain-provider` request parsing so hidden raw transaction and node RPC fields fail at decode time before provider logic runs.
- Added signable EIP-155 legacy transaction intents from `chain-provider` and built-in wallet transaction signing after Runtime approval, while external wallet transaction signing remains connector-bound.
- Added ERC-1271 smart-account SIWE proof verification through a typed `chain-provider` proof followed by wallet-provider challenge consumption, keeping smart-account proof checks out of app capsules.
- Added typed Bitcoin BIP-322 simple proof challenge/verification for Bitcoin mainnet native P2WPKH addresses, with wrong-message and unsupported-script paths failing closed behind `elastos://wallet/proof/bip322/*`.
- Added connector-token-scoped BTC wallet-link routes for BIP-322 challenge/verify so a connector capsule can bind a Bitcoin proof to the existing passkey principal without minting a Home session or exposing raw wallet/node authority.
- Added manual BIP-322 proof-link handoff inside the visible `wallet` capsule, keeping MetaMask EVM-only until a documented Bitcoin dapp signing API exists.
- Added a dormant `wallet-walletconnect` connector capsule UI that stays hidden/unroutable until operator-pinned WalletConnect config and a local Reown/AppKit adapter hash are present.
- Added a WalletConnect connector config utility and smoke proof that copy a local reviewed adapter into the runtime data dir and pin its sha256 before the connector can launch.
- Added an exact-version WalletConnect adapter build script for producing the local Reown/AppKit adapter bundle used by the connector gate.
- Added an entropy guard proving WalletConnect requires an explicit operator Project ID and local SDK hash pinning, with no repository or environment default Project ID.
- Added a visible Wallet surface that replaces the old Bitcoin-first wallet app with accounts, native ESC/Base/BTC balance reads through `chain-provider`, default-account selection, approval review, and connector handoffs as approval methods.
- Added the first visible `browser` capsule shell with explicit `elastos://wallet/*` and `elastos://net/*` capability intent, a Glide default URL for testing, and honest cross-origin wallet-injection boundaries until the native/webview or microVM Browser/Net/Exit adapter exists.
- Added `net-provider` as the first fail-closed Browser/Net boundary: it validates Browser requests, blocks LAN/private targets, rejects hidden raw authority fields, and returns an explicit `exit_unavailable` handoff instead of touching host networking itself.
- Added `exit-provider` as the internal Browser egress contract with fail-closed `quote`, `open_stream`, `close_stream`, and `http_fetch` operations for future local, Carrier-routed, privacy, paid, or enterprise exit backends.
- Added the first constrained Browser egress proofs: `/api/provider/net/http` and `/api/provider/net/stream` now validate through `net-provider` and then hand off internally to operator-configured `exit-provider` `http_fetch` or `stream_relay` backends with host allowlists, body limits where bytes are returned, and private-target blocking by default.
- Moved the visible Browser preview request path from HTTP fetch to stream-session reservation so the UI exercises the intended browser path and reports `byte_transport: not_attached` until a Browser Engine Adapter exists.
- Added `browser-engine-adapter` as the internal Browser Engine Adapter contract with fail-closed `status`, `launch`, `attach_stream`, and `close_page` operations; it requires explicit operator config and attached `adapter_ipc` byte transport before any page launch can succeed.
- Added `/api/apps/browser/open` as the high-level Browser product route that performs Runtime-owned Net validation, Exit stream reservation, and Browser Engine Adapter launch without exposing raw Exit or Browser Engine provider routes to ordinary capsules.
- Added Browser Session Manager proof surfaces: launch reservations, per-principal/total capacity receipts, page heartbeats, stale active-page cleanup, and gateway-level smokes that prove concurrent Home-launched Browser page accounting closes without leaving capacity behind.
- Added a protected-content Browser reachability smoke for the known `ela.city` item route. This is intentionally scoped to route/session cleanup and does not claim purchase, key release, decrypt, or playback readiness.
- Added typed `elastos.adapter-ipc/v1` descriptors for configured Browser stream backends and stripped those private endpoint descriptors from Browser UI responses while passing them to the internal Browser Engine Adapter.
- Added typed `elastos.exit.relay-ipc/v1` descriptors for private Exit relay sockets; Gateway uses them only internally to relay bytes from the Runtime-owned Browser stream socket to an operator/Carrier exit daemon, strips them from Browser UI, and never passes them to the Browser Engine Adapter.
- Added the Browser Engine native supervisor handshake: Runtime sends `elastos.browser.engine.launch-request/v1` through `ELASTOS_BROWSER_ENGINE_REQUEST`, and native adapters only launch when the supervisor returns a validated `elastos.browser.engine.supervisor-result/v1` with runtime-net-only, no-direct-network, and no-wallet-injection proofs.
- Added `browser-engine-supervisor` as the first Linux host helper for native browser engines: it validates operator config, starts the configured engine under `linux_new_netns`, and passes only stream/IPC/target/URL environment to the child process.
- Added `browser-stream-bridge` as the first Linux local byte-transport helper for Browser Engine work: it forwards between a private engine Unix socket and a Runtime-owned Unix stream socket without TCP, DNS, HTTP, wallet, chain, or raw host-network authority; the supervisor can launch it before the native engine through `ELASTOS_BROWSER_STREAM_BRIDGE_CONFIG`.
- Added Runtime-owned Browser stream socket path allocation: Gateway injects a private `Runtime/BrowserStreams/*.sock` path into the internal Browser Engine launch descriptor, binds it as a local Unix listener, closes fail-closed when no private Exit relay exists, and keeps `adapter_ipc`/`relay_ipc` hidden from Browser UI responses.
- Added `browser-local-exit` as the first server-side Browser Exit relay: Runtime sends a typed `elastos.exit.relay-open/v1` handshake to its private Unix socket, and the helper dials only operator-allowlisted public targets while blocking private resolved IPs by default.
- Changed Home launch so the Browser capsule opens as an ElastOS window instead of escaping into a host browser tab.
- Added persistent typed node-lifecycle state to `chain-provider`, including reload coverage that does not persist or return raw node RPC URLs.
- Added proof-bound Recovery Kit export/import request routes that validate principal/root binding, require active Home/System authority, and record signed audit denials.
- Added validated principal-root protection state so recovery status can report verified recovery protection without allowing cross-principal protection records to bleed across users.
- Added proof-bound Recovery Kit creation/import/export: the runtime generates a per-principal data key, wraps it to a recovery phrase, stores a runtime-encrypted downloadable archive plus protector metadata, and verifies encrypted root descriptors on import.
- Added optional password-packaged Recovery Kit downloads and password-verified package import so users can protect the offline kit file without giving apps raw recovery authority.
- Added public route-level Recovery Kit coverage for create, status, password-packaged export, wrong-password rejection, and verified import through `/api/auth/recovery/*`.
- Added contract-level WebAuthn PRF recovery-protector validation so PRF protectors must use `webauthn-prf` envelopes and cannot carry Recovery Kit archives.
- Hardened runtime WebAuthn response parsing so hidden extension payloads such as `clientExtensionResults` are rejected until client-side PRF wrapping exists.
- Added contract-level DID recovery-protector validation so DID protectors must identify a `did:key` or `did:elastos` subject, use DID-bound envelope metadata, and cannot masquerade as downloadable Recovery Kits.
- Hardened principal-root protection and Recovery Kit contracts so hidden or unknown nested fields fail during decode instead of being accepted and ignored.
- Added a typed `did-provider` recovery-proof verification operation for `did:key` subjects that binds the proof to principal, root, protector, data-key ID, nonce, and expiry.
- Hardened `did-provider` request parsing so hidden fields on typed DID recovery and chat-signing requests are rejected instead of being accepted and ignored.
- Hardened all `op`-tagged provider request contracts to reject unknown top-level fields, covering Documents, localhost, IPFS, availability, protected-content, AI, WebSpace, tunnel, and provider bridge operation envelopes.
- Hardened protected-content object and request contracts so sealed objects, key envelopes, key-release requests, decrypt-session requests, DRM open requests, rights checks, and availability ensure calls reject hidden nested authority fields at decode time.
- Hardened browser-facing gateway request bodies so Home state, System settings, wallet approvals, Home launches, Inbox actions, chat access, room messages, and upload starts reject hidden authority fields at decode time.
- Hardened capability API request bodies so request, grant, deny, revoke-all, and audit query inputs reject hidden authority fields at decode time.
- Wired Recovery Kit import to consume typed DID recovery proofs through `did-provider` only when they match an existing DID recovery protector on the recovered root, preserving that protector without claiming DID-only recovery.
- Added System smoke coverage for Recovery Kit password-protected download and import controls so the user-visible recovery path stays tested.
- Added Home smoke coverage for Recovery Kit controls inside the Home-launched System frame.
- Added `scripts/recovery-kit-live-smoke.sh` so signed browser sessions can prove live Recovery Kit status/download/import without silent mutation; it accepts a copied token, Cookie header, or cookie jar from the signed Home session.
- Fixed the live Recovery Kit smoke script to validate the current `elastos.principal.root-recovery.status/v1` schema before export/import.
- Added `scripts/auth-wallet-focus-smoke.sh` as a repeatable gate for the branch's passkey, recovery, recovery-protector contract, capsule-bridge principal storage, principal-launch, System managed-wallet route, wallet, BTC, typed chain proof/prepare/broadcast, chain sync-health, node lifecycle, entropy, and alignment checks.
- Extended the auth/wallet focus smoke with MetaMask connector and Wallet Bitcoin-proof journey filters.
- Added Home Chat Room launch coverage to the auth/wallet focus gate so Home-to-runtime launches must use signed `launch_grant` authority and never raw `principal_id`.
- Added explicit Recovery Kit root reassignment: System can import a verified kit for an existing principal root, rebind the active passkey to that root, reissue Home/System session tokens, restore included built-in Wallet keys, and record signed audit.
- Hardened Recovery Kit root reassignment so recovery replaces old passkey-root bindings, revokes their sessions, and public handler coverage proves the response rotates the Home session cookie after reassignment.
- Hardened System Recovery Kit import so recovering an account requires an explicit in-surface `Recover account` action instead of silent reassignment.
- Added runtime-enforced principal-root object encryption for protected roots: Documents working copies, Home browser state, and viewer/content storage now write AES-256-GCM envelopes bound to principal, root, data-key ID, and object URI, and reject plaintext reads for protected roots.
- Fixed protected-root Home loading so old plaintext browser-state files are treated as untrusted UI state and reset to a clean default instead of causing a 500 or being accepted as data.
- Hardened the legacy generic `/api/localhost/*` storage handler so `Users/*` roots fail closed without a runtime principal-scoped provider route.
- Updated the Home CLI `Users` root descriptor to show principal-root storage instead of the obsolete shared `Users/self` alias.
- Removed stale shared `Users/self` examples from generic capability/provider-resource tests so only principal-scoped bridge paths keep that alias.
- Removed the stale shared `Users/self` path from VM provider path-shaping tests.
- Added an entropy guard that only allows `Users/self` in approved scoped-alias code, capsule manifests, and regression tests.
- Renamed the System account display field from Handle to Name so local profile copy is not confused with global name-claim semantics.
- Added `scripts/protected-home-state-smoke.sh` to prove the protected Home browser-state reset regression and live unsigned Home summary path before handing browser-visible changes back for testing.
- Added passkey revocation policy coverage proving admins can revoke guest passkeys without revoking their own session while guest-to-admin and last-admin removal still fail closed.
- Clarified WebAuthn RP authority handling so Host-derived same-origin requests are not described as a fallback around malformed browser origins.
- Added admin-controlled guest passkey promotion through a runtime auth route and System action, with guest self-promotion rejected.
- Added admin-controlled admin passkey demotion through a runtime auth route and System action, while rejecting self-demotion and last-admin demotion.
- Removed the unsigned Home unlock copy flicker so the passkey card starts with the final sign-in message instead of a temporary checking message.
- Simplified Home sign-in so the browser passkey chooser opens automatically, sign-in no longer shows the passkey-name field or leaked form label, and guest creation is a separate flow.
- Fixed Home passkey cancellation so dismissed browser passkey prompts show a clean `No passkey selected` state with `Create guest account` still available.
- Fixed Home/System appearance state so wallpaper and overlay preferences are stored under the active passkey principal root instead of a shared runtime bucket.
- Clarified Wallet UI language around Accounts and Approval methods so MetaMask, Bitcoin, WalletConnect, and passkey-managed signing read as connector-backed approval paths under one Wallet model instead of separate wallet products.
- Cleaned Wallet Approval Methods so linked accounts are removable from Wallet after fresh passkey confirmation, MetaMask can add another account, WalletConnect remains disabled without pinned operator config, Ledger stays hidden until implemented, and UniSat is not advertised as available from hosted Browser environments.
- Added principal-scoped WASM launch plumbing: Home-backed runtime launches forward the signed launch-token principal into WASM bridge pipes so capsule-facing `localhost://Users/self` can resolve through the runtime principal root instead of a shared alias.
- Routed protected in-runtime capsule-kernel `localhost://Users/self` read/write calls through the runtime principal-root object envelope when a Home launch principal is present, and made attached/remote bridge user-root storage fail closed until it has the same protected bridge, preventing protected user-root state from falling through to generic localhost-provider storage.
- Added route-level rejection tests for `/api/capsules` raw principal injection so supplied `principal_id` values fail closed before reaching the runtime bridge.
- Replaced raw `/api/capsules` principal injection from Home with a signed, app-scoped `launch_grant`; raw `principal_id` launch authority is now rejected even when it is paired with a grant.
- Tightened managed Home/chat runtime policy so capability grants target principal-root storage, while the capsule bridge rejects explicit foreign `localhost://Users/<root>` requests before approval prompts.
- Fixed managed runtime-backed Home launches so Chat Room and other app capsules can validate signed principal launch grants instead of failing with `principal launch grant unavailable`.
- Hardened the shell/supervisor microVM launch path so it accepts the same signed app-scoped `launch_grant`, rejects raw `principal_id`/`home_token` authority and authority-shaped config, passes the verified principal into the microVM Carrier bridge, and refuses provider-role user scope.
- Added System-gateway coverage for `chain-provider` node lifecycle status so lifecycle checks use the same launch-token and capability-resource path as other System chain diagnostics.
- Added operator-approved loopback node supervisor control to `chain-provider` for start/stop/restart without returning raw node URLs, ports, command paths, or process handles.
- Added System node lifecycle controls that appear only when `chain-provider` reports `control_available=true`; remote/public RPC networks stay status-only.
- Added wallet approval journey coverage: provider-created typed signature requests appear in Wallet/Inbox, approval executes managed signing, and completion records signed audit without exposing wallet authority to apps.
- Added provider-backed default wallet routing: System selects the principal's default account, and typed signature requests must name `chain_namespace + intent` before the wallet-provider resolves a default or verifies an explicit same-chain account.
- Added anti-drift checks that block ordinary app/viewer/content capsules from referencing raw wallet, chain, node, RPC, WalletConnect, MetaMask, or blockchain-provider authority directly.
- Added shared protected-content schemas and a fail-closed `drm-provider` contract for `elastos://drm/meta/status` and `elastos://drm/open`.
- Added `rights-provider` as the typed, fail-closed protected-content policy boundary for access, subscription, stream, and download questions.
- Added `key-provider` as the typed, fail-closed protected-content key-release boundary for PQ-hybrid dKMS requests.
- Added `decrypt-provider` as the typed, fail-closed protected-content decrypt/render session boundary.
- Added a canonical `drm-provider.status.required_sequence` for protected-content open orchestration before backend wiring.
- Added Runtime-owned release receipt and audit steps to the protected-content open sequence.
- Added the same machine-readable required sequence and runtime events to fail-closed `drm-provider.open` responses.
- Added `scripts/protected-content-provider-contract-smoke.sh` to exercise protected-content provider capsules through their real JSON line protocol.
- Added `scripts/installed-provider-verify.sh` so installed provider binaries can be checked against the installed `components.json` before live browser testing.
- Added alignment checks and release-story documentation so the protected-content provider journey proof stays visible in `TASKS.md`, `state.md`, and the runtime repo checklist.
- Added algorithm metadata to protected-content key envelopes so sealed objects can declare cipher, signature, KEM, and share-scheme choices for PQ-hybrid dKMS work.
- Enforced protected-content key-envelope algorithm allowlists for AES-256/ChaCha20 payload encryption, hybrid X25519 + ML-KEM share wrapping, and classical + PQ signatures.

### Changed
- Upgraded Home launch tokens to carry principal, session, proof-binding, grant, expiry, and non-delegation context, with active-session validation for proof-bound tokens.
- Aligned principles, roadmap, capsule model, and Carrier docs around the local-and-off-box Carrier plane and blockchain quadrant authority model.
- Added an explicit `blockchain` setup profile and release/build support for `chain-provider` while keeping it out of the default Home profile.
- Enforced carrier-only authority for ordinary app/viewer/content capsule manifests and added repo-wide alignment checks for forbidden manifest authority.
- Moved the headless agent capsule from direct runtime HTTP calls to the guest Carrier-kernel SDK and added an alignment check against direct host-route usage in ordinary Rust/WASM app capsules.
- Made the capsule bridge reject raw runtime-control requests with an explicit `not_capsule_kernel_abi` error so app capsules stay on capability/`carrier_invoke` calls.
- Removed raw shell/runtime-control, direct storage, direct provider routing, and direct capsule-message helpers from the guest SDK so capsule code only sees capability requests and `carrier_invoke`.
- Moved first-party chat, agent, and Home CLI capsules from `provider_call(scheme, op)` to the URI-based `carrier_invoke(uri, operation)` capsule-kernel ABI.
- Documented the older `elastos-runtime` handler as an internal shell/control protocol, not the public guest SDK, and added a test that `carrier_invoke` stays on the Carrier bridge.
- Added browser-host-adapter proof that attached WASM capsules cannot use HTTP bridge routes for raw runtime control.
- Required provider capsule manifests to declare their owned `provides` namespace, including the WebSpaces resolver.
- Required provider capsule manifests to declare provider-authority metadata with reason strings, capability schemas, operation lists, and expected audit events.
- Added a built-in `content` provider seam above `ipfs-provider` and moved Documents publish/unpublish onto that availability contract with honest local availability status.
- Made `elastos://ipfs/*` a system-only backend at the capability request surface so ordinary capsules must use `elastos://content/*`.
- Added signed local availability receipts for content publish/unpublish and exposed the latest receipt through `elastos://content/status`.
- Moved site publish/activate CID creation onto the `elastos://content/*` path while keeping `ipfs-provider` as the low-level backend.
- Moved `elastos share`, `elastos shares` channel-head updates, and `elastos attest` provenance writes onto the `elastos://content/*` path.
- Added `elastos://content/fetch` for CID/path reads and moved provenance verify/read helpers onto the content provider path.
- Routed `/s/<cid>/...` gateway file reads through `elastos://content/fetch` instead of raw `ipfs-provider` requests.
- Moved share metadata and channel-head reads onto `elastos://content/fetch`.
- Rejected ordinary app/viewer/content manifest capabilities for system-only backend namespaces such as `elastos://ipfs/*`, Kubo, IPFS Cluster, Elacity SDK, and runtime SystemServices storage.
- Added `elastos://content/repair` so the content provider can re-pin a CID locally or record a signed `repair_needed` receipt.
- Added `elastos://content/ensure` as the idempotent availability operation and made content status reject invalid CIDs instead of returning ambiguous unknown state.
- Added a registered availability-provider seam so content publish/ensure can return `network_available` or `repair_needed` when a runtime-owned replication provider verifies network availability.
- Added an `availability-provider` capsule that forwards `elastos://content/ensure` availability requests to explicitly configured Elacity/supernode-compatible targets without hardcoded public service assumptions.
- Removed stale raw-IPFS helper paths from the main command dispatcher and added an alignment guard so command materialization stays on `elastos://content/*`.
- Added deterministic `_elastos_object.json` manifests to directory publishes, giving Documents, shares, and sites a common IPLD-compatible object shape.
- Extended content object manifests with deterministic CID links plus `release` and `sealed` object kinds for release manifests and protected-content descriptors.
- Made `sealed` content object publishes fail closed unless they include `sealed.json`, payload/rights/availability/provenance links, and approved protected-content key-envelope algorithms.
- Made content directory publishes sort package entries and reject duplicate paths or unknown object kinds before bytes reach the IPFS backend.
- Made Documents unpublish receipts preserve the document owner DID instead of falling back to the runtime device DID.
- Tightened ordinary capsule manifests so apps, viewers, and content cannot declare external host dependencies, provider implementation overrides, or microVM HTTP ports.
- Unified provider capability-resource derivation across the HTTP host adapter and Carrier bridge so local and capsule-kernel calls fail closed against the same resource contract.
- Removed duplicate wallet-provider transaction prepare/broadcast declarations; typed transaction prepare/broadcast belongs to `chain-provider`, while `wallet-provider` owns approval and signing receipts.
- Narrowed `elastos://content/*` capability-resource derivation to documented publish/fetch/status/ensure/repair/unpublish operations instead of a broad content wildcard.
- Made capsule-kernel capability requests fail closed for unsupported schemes and system-only backends such as raw gateway/IPFS/Kubo/Elacity namespaces before they can create user approval prompts.
- Routed ordinary `elastos publish` capsule directory uploads through `elastos://content/*`, while leaving large MicroVM rootfs streaming on the existing explicit operator path.
- Routed `elastos open elastos://<cid>` data-capsule materialization through `elastos://content/*` and verified `_elastos_object.json` file size/hash metadata before serving.
- Routed `elastos run --cid` and `elastos serve --cid` materialization through `elastos://content/*` instead of the raw IPFS bridge.
- Routed supervisor-installed capsule artifact downloads through `elastos://content/fetch` instead of direct `ipfs` sub-provider calls.
- Routed public gateway installer publishing through `elastos://content/publish` instead of direct `ipfs` sub-provider calls.
- Added an operator `elastos content publish-object` path and release-object sidecars so public releases can expose IPLD-compatible manifest links without breaking raw installer CIDs.
- Taught release/update bookkeeping to validate and display signed `release_object_cid` values while preserving raw `latest_release_cid` installer compatibility.
- Taught `elastos open elastos://<release-object-cid>` to open release objects as verified metadata summaries and made CID materialization reject release objects as non-launchable content graphs.
- Added alignment gates so CID run/serve and public gateway publish paths cannot reintroduce raw IPFS materialization.
- Tightened passkey/WebAuthn ceremonies to require user verification in browser options and reject authenticator data without the user-verification flag.
- Added a first-class `passkey_webauthn` runtime proof-binding model so passkeys can bind principals without becoming wallet or DID replacements.
- Changed successful WebAuthn registration/authentication to return verified credential facts for runtime proof-binding issuance.
- Bound successful passkey registration/authentication into runtime auth state by upserting a passkey proof binding and issuing a short-lived Home/System session grant.
- Clarified WebAuthn RP/origin derivation so localhost development uses `http://localhost` while hosted Home uses its HTTPS origin.
- Added browser-gateway passkey register/sign-in endpoints and System passkey controls that issue Home/System launch grants without wallet-first UX.
- Promoted passkeys into the default Home entry path so fresh browsers see a Home unlock surface instead of receiving an automatic local session cookie; successful passkey registration/sign-in sets the same refresh-safe HttpOnly Home session cookie.
- Made passkey the Home front door authority: the first passkey on a runtime becomes admin, later passkeys become guest principals with their own `localhost://Users/<principal-root>` area, guest creation defaults off, and System admin controls new guest enrollment without revoking existing guests.
- Made guest passkey creation nameable and same-authenticator friendly by omitting `excludeCredentials` for new runtime principals while preserving duplicate-prevention for legacy backup-passkey registration.
- Made first Home passkey creation nameable, derived the visible handle from the active passkey principal, and documented orphaned user-root recovery semantics.
- Removed the proof-less System handle path, scoped guest passkey lists to the current guest principal, and made passkey revocation runtime-enforced so guests cannot remove admin passkeys.
- Removed admin-created guest passkeys from System; admins now only open or close guest enrollment, while guests self-register their own passkey and principal from Home when enrollment is enabled.
- Added principal-root protection and Recovery Kit contracts plus a proof-bound recovery status route that stays honest about unencrypted or unprotected roots.
- Scoped viewer/content storage such as GBA save states through the signed Home launch-token principal instead of writing under a shared literal `localhost://Users/self` directory.
- Scoped Documents provider working copies through the signed Home launch-token principal and rejected cross-principal document operations at the provider boundary.
- Removed the global notification-to-native-chat relay that wrote room events into shared `localhost://Users/self` state without an active principal.
- Made the generic capsule-kernel bridge map `localhost://Users/self` through an explicit principal context, while rejecting capability requests and carrier invokes when that context is missing.
- Made unsigned Home load as a standard non-user desktop that prompts for passkey sign-in while keeping runtime ensure, app launch, and browser state writes capability-gated.
- Aligned the Home passkey prompt with the PC2 login surface: centered dark card, ElastOS branding, amber action, and concise passkey copy without wallet-first dependencies.
- Simplified Home sign-in copy around data, apps, and desktop access and tightened toolbar spacing so the sign-out control remains fully visible.
- Simplified System into Account, Appearance, and collapsed Advanced areas so routine passkey, guest, and wallpaper settings are not mixed with runtime diagnostics.
- Renamed the visible System Advanced DID field to Device identity and clarified that passkey principals, `did:key`, `did:elastos`/EID, handles, CIDs, and IPLD object graphs are separate identity layers.
- Signed runtime audit events with the runtime DID key before persisting them in auth state.
- Kept the Home passkey card stable through status checking and signed boot so registration/sign-in does not flash through intermediate desktop states.
- Wired Home to refresh proof-bound passkey sessions through the runtime session-refresh route after signed boot and during long-lived desktops.
- Fixed proof-bound session refresh to accept the browser's HttpOnly `home-session` cookie, preventing successful passkey sign-in from falling back into the unlock prompt.
- Added explicit Home sign-out that revokes the current proof-bound session grant, clears the HttpOnly `home-session` cookie, and reloads into the unsigned passkey prompt.
- Bound Home open-window session restore to a browser-context id in site storage and de-duped restored targets so clearing browser site data cannot replay stale server-side System windows after sign-in.
- Moved Home browser layout/session/recent-target state into the active principal's `localhost://Users/<principal-root>/.AppData/ElastOS/Home/` area instead of a shared system bucket.
- Made browser-gateway passkey routes load identity state lazily and fail closed without creating identity material during unrelated reads.
- Made WebAuthn RP derivation fail closed for malformed or insecure browser origins and documented hosted Home, localhost, PWA, and future WebView adapter boundaries.
- Added proof-bound passkey list, credential revoke, and session-refresh routes for Home/System without exposing passkey controls to app capsules.
- Expanded fail-closed passkey coverage for replayed or expired challenges, wrong origin/RP, missing user verification, counter regression, missing grants, and revoked proof bindings.
- Bound multiple passkeys for the same local identity to one runtime principal and required an existing Home/System grant before adding backup passkeys.
- Replaced capsule-facing arbitrary DID `sign(data)` with a typed chat-message signing intent so the DID provider no longer exposes generic private-key signing to app surfaces.
- Defined the wallet-provider contract and expanded the chain-provider capability schema before adding blockchain write/broadcast UI or provider behavior.
- Added wallet-provider capability-resource mapping so future `elastos://wallet/*` calls are scoped and fail closed instead of falling through to broad provider wildcards.
- Added the first `wallet-provider` capsule slice for linked-account metadata under runtime-managed storage, with proof, signing, and transaction operations failing closed until approval/proof enforcement exists.
- Extracted shared auth primitives into `elastos-auth` and added wallet-provider SIWE challenge/verify support with single-use proof challenges while keeping signing and transactions fail-closed.
- Routed browser EVM wallet login through `wallet-provider` proof challenge/verify and linked verified accounts through the provider before Runtime issues scoped Home/System grants.
- Added wallet-provider typed signing approval requests so `request_signature` records pending, principal-scoped approval state instead of exposing arbitrary signing or wallet RPC.
- Added System wallet approval review/reject APIs and UI so pending typed wallet requests can be inspected without giving apps direct wallet authority.
- Added a typed `chain-provider` rights-read seam for `has_access_by_content_id` that validates protected-content inputs and fails closed until rights ABI configuration exists.
- Added configurable `chain-provider` rights-method ABI support for `hasAccessByContentId(string,address,string) -> bool`, scoped to approved contracts/selectors and still without arbitrary RPC passthrough.

### Removed
- Removed the generic `chain-provider` `call` operation so chain access stays on reviewed typed operations instead of arbitrary `eth_call` inputs.

### Fixed
- Hardened EVM auth challenges to derive origin from the runtime request, reject client-supplied origin fields, and verify the exact runtime-issued SIWE message.
- Made source-checkout `elastos setup --list` use the checkout `components.json` before any stale installed manifest, so developer runs show current Home/blockchain profiles.
- Rejected path-like document DIDs before joining document metadata paths.
- Narrowed viewer-bound object launch tokens so a token for one content capsule cannot enumerate the full viewer library.
- Required recovery status to distinguish verified recovery protectors from merely present root-protection metadata.
- Required approved wallet requests to expire before managed signing or external wallet completion can execute.
- Updated managed-wallet namespace errors so Bitcoin-capable wallet creation no longer reports stale EVM-only guidance.
- Rendered System passkey admin actions only after the runtime access role is loaded, so admins can promote guest passkeys without reloading System.
- Persisted an explicit empty Home browser-window session after the last window closes, preventing stale windows from reopening after refresh.

## [0.2.0] - 2026-04-29

### Added
- Added the Home browser shell capsule, `home-cli`, and the `elastos home` command path as the visible front door.
- Added first-party System, Inbox, Library, Documents, Chat Room, GBA Emulator, and uCity browser/content capsules to the shipped Home catalog.
- Added runtime-owned browser capsule routing, object/viewer launch foundations, and app-scoped launch tokens for Home-launched surfaces.
- Added Documents provider APIs for summary, create, get, save, save-as, publish, unpublish, delete, and immutable `elastos://<cid>` document opens.
- Added Library document browsing and Chat Room attachment flow through Home orchestration instead of browser file upload.
- Added Chat Room browser-session pairing, same-browser Home session reuse, guest identity separation, guest kick controls, and runtime member invite/block controls.
- Added System appearance controls for wallpaper, reset, overlay toggle, and overlay opacity.
- Added Home PWA metadata, mobile fullscreen support, touch-first desktop icon behavior, reversible desktop icons, and mobile-safe window behavior.
- Added GBA save-state persistence, mobile touch controls, keyboard mapping labels, fullscreen ratio handling, compact controls, and fail-fast unsupported-WebView detection.
- Added shared first-party design-system docs and `scripts/home-entropy-check.mjs` for UI, naming, authority, and stale-copy drift checks.

### Changed
- Renamed the visible product front door from PC2 to Home and aligned setup profiles, proof scripts, docs, and CLI smoke tests around the Home naming.
- Replaced `md-viewer` with Documents and `room-browser` with Chat Room.
- Split setup profiles more explicitly between the core Home path, the broader demo surface, and the explicit operator lane.
- Hardened release proofing around clean-home setup, the PTY Home front door, `chat-room` packaging, source-local trusted-source checks, and Home/browser journey smoke coverage.
- Moved Home wallpaper and contrast overlay configuration into `System -> Appearance` backed by the runtime appearance store.
- Made Documents object-first: DID-backed document identity is the mutable object, `localhost://Users/self/Documents/...` is only the local working copy, and `elastos://<cid>` is the immutable published revision.
- Made Library content-first around documents and typed content instead of raw working-copy paths.
- Moved Inbox list rendering, read state, approval, denial, dismissal, and source-app open actions into the Inbox capsule; Home now owns only badge and launch.
- Aligned first-party capsule UI colors, spacing, mobile padding, and accessible controls with the shared light capsule token set.
- Aligned roadmap, principles, architecture, namespaces, and security docs around the four quadrants, object/capsule/space ontology, and capability-scoped Carrier/provider boundary.

### Fixed
- Unified main DID derivation with the device key and aligned local nickname persistence onto one shared codec.
- Removed stale live-host conflicts so managed Home/chat lanes and the explicit operator lane do not silently share one home.
- Cleaned up the public room naming so the shipped `chat-room` route, packaging, and proof tooling all agree.
- Moved Documents publish/unpublish IPFS-specific logic out of the gateway edge into the provider plane.
- Removed the gateway-owned IPFS provider bridge path; public CID reads now use cached content or the runtime provider registry and fail closed otherwise.
- Required Home authority before minting app launch tokens, app-scoped launch tokens for System and Inbox APIs, and browser-context-bound chat-room access polling.
- Redacted room bearer tokens from public/Home summaries and preferred the native Home room session over paired browser identity when both exist in one browser profile.
- Fixed browser Chat Room identity handling so messages from the native Home member no longer appear as the browser guest's own messages.
- Fixed Documents publish/unpublish behavior so unchanged content does not produce unnecessary new published revisions.
- Fixed document delete confirmation to use in-surface UI instead of browser alerts.
- Fixed Home window dragging so windows can move partially offscreen without jumping back when their title bar is clicked.
- Fixed desktop drag selection, desktop icon removal/re-add, mobile launcher focus, and maximized-window coverage over Home chrome.

## [0.1.2] - 2026-04-16

### Added
- Added device-backed local identity profile storage and shared DID-backed nickname handling across the CLI, did-provider, and PC2 surfaces.
- Added hosted browser-capsule foundation, the shipped `room-browser` asset set, and sovereign room invite/accept control with cross-runtime Carrier sync.
- Added explicit operator-lane setup, remote node control over Carrier, and release-line public-install/operator acceptance scripts.

### Changed
- Kept PC2 as the honest front door by surfacing room, chat, and identity flows with the current runtime and return-home contract.
- Split setup profiles more explicitly between the core PC2 path, the broader demo surface, and the explicit operator lane.
- Hardened release proofing around clean-home setup, the PTY PC2 front door, room-browser packaging, and source-local trusted-source checks.

### Fixed
- Unified main DID derivation with the device key and aligned local nickname persistence onto one shared codec.
- Removed stale live-host conflicts so managed PC2/chat lanes and the explicit operator lane do not silently share one home.
- Cleaned up the public naming around `room-browser` so the shipped browser route, packaging, and proof tooling all agree.

## [0.1.1] - 2026-03-31

### Fixed
- Removed the installer's undeclared `xxd` dependency from signature verification so minimal environments can install from the canonical gateway without extra packages.
- Pinned the documented and declared Rust toolchain to `1.89+` so fresh source builds match the actual compiler floor.
- Tightened PC2 home guidance and native chat runtime reuse so the public onboarding path stays coherent on WSL and Jetson.

## [0.1.0] - 2026-03-31

### Added
- Signed install, setup, and update flow with a canonical public onboarding path.
- Native Carrier chat with signed message verification, cross-host WSL ↔ Jetson proof, and same-host native ↔ WASM proof coverage.
- Capability-gated capsule execution across native runtime surfaces, WASM capsules, and microVM capsules.
- DID-backed identity, local sharing, site hosting/publish/activate/rollback, and agent capsule support.

### Changed
- The public repository starts fresh at `0.1.0`.
- `elastos chat` is native Carrier chat only; packaged chat surfaces launch through `elastos capsule ...`.
- The installer and first-run story are centered on `install.sh -> elastos setup -> elastos`.

### Removed
- Runtime/proof override residue including `ELASTOS_COMPONENTS_MANIFEST`, `ELASTOS_DEV_SEARCH`, `SkippedDevPath`, `InstalledBinaryVerification`, and `chat --mode ...`.

## Pre-public internal lineage

Earlier internal release candidates and development history existed before the public repository launch. They are intentionally not carried forward as the public release line.
