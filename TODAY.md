# Today - Library / Explorer Release State

Date: 2026-06-06

Goal: keep Library as a stable, PC2-familiar Explorer running on ElastOS
Runtime provider rails. Library must be testable by a human in Home and by an
operator through route/provider/UI checks.

## Non-Negotiables

- Library is an app capsule. It owns UI only.
- `object-provider` is the canonical mutable principal object provider. It owns
  folders, files, Desktop/Documents, revisions, Trash, encrypted
  principal-root storage, and Library object events. Runtime registers only the
  `object` provider scheme, and browser calls use `/api/provider/object/*`.
- `content-provider` is the Carrier-backed published-content authority. It owns
  immutable content identity, CIDs, publish/fetch, status, repair, replication,
  and availability receipts. It should not own Explorer UI, folder names,
  Desktop, Trash, or local rename/move semantics.
- `ipfs-provider`/Kubo is only the current low-level local content backend for
  CID creation, pinning, and fetch. It is not the final capsule-facing content
  delivery model and must stay system-only behind `content-provider`.
- The fully aligned target is a Carrier/provider content-delivery plane:
  `content-provider` owns content policy and receipts, Carrier coordinates peer
  discovery/transport, availability providers handle replication/repair, and
  apps see one access surface regardless of whether bytes are local, cached, or
  fetched from the network.
- `webspace-provider` exposes mounted/discoverable spaces. It is a resolver
  surface over object/content spaces, not the principal-root object graph; mutable
  mounts/forks may also materialize local WebSpace objects in provider-owned
  object/head tables until external resolver sync workers take over.
- `localhost://WebSpaces/<mount>/...` is the local mounted view, shown to users
  as **Spaces** in Library. Provider targets such as `google://drive/...` are
  resolver-private, and `elastos://content/*` is the provider-independent
  published/shared content identity after import, publish, or fork.
- Runtime injects the signed principal and mediates Library operations.
- Library must not call raw host filesystem, Kubo/IPFS, Elacity APIs, wallet,
  chain, network, Carrier peer, provider SDK, or broad `localhost://Users/*`.
- PC2 is the UX reference, not the authority model.
- Every visible menu/action must work now or be hidden.

## Canonical Remaining Product Plan

This is the single source of truth for the repeated gap lists. The old internal
track labels have been retired. The branch should now describe work by product
area and authority boundary, not by numbered release buckets.

### First-Principles Audit Rules

- Runtime owns identity injection, capability routing, provider invocation, and
  audit. Apps never receive raw provider SDKs, host paths, Carrier tickets,
  Kubo/IPFS handles, keys, wallet authority, or foreign principal roots.
- `object-provider` owns mutable principal-root objects; `content-provider`
  owns published immutable content identity, CIDs, availability, and
  Carrier-backed delivery receipts; `webspace-provider` owns mounted resolver
  views and metadata/fork heads.
- Paths such as `localhost://WebSpaces/<mount>/...` are user-comfort views.
  Provider targets and CIDs remain resolver/content identities behind Runtime
  and provider mediation.
- A product area closes only when the user-visible behavior and the
  provider/runtime authority boundary both have tests. Documentation or receipt
  metadata alone is not enough.

### Complete In This Branch

- Object-provider migration is complete. `object-provider` is the canonical
  package, manifest, Runtime `object` scheme, and `/api/provider/object/*`
  browser route. Retired mutable-object provider compatibility fallback paths
  should not return.
- Provider-to-provider invocation is foundation-complete. Runtime can invoke
  service providers locally and over Carrier provider transport without exposing
  raw transport/backend authority to apps.
- Provider streaming is complete for this branch. `ProviderTransfer::Stream` is
  a validated JSON/base64 chunk envelope, Runtime stream sessions provide
  read/cancel/progress flow control, `content-provider` fetch drains provider
  chunks through that path, and Library downloads return chunked HTTP body
  streams with backpressure/cancel receipt metadata.
- Recipient-scoped sharing and protected-content receipt-chain proof are
  complete for this branch. Library has public-link and recipient-scoped share
  UX, `shared_access`, `Check My Access`, Runtime recipient-proof injection,
  readiness receipts, and receipt-bound drm/rights/key/decrypt provider
  contracts. This does not claim production encrypted payload generation,
  production dDRM policy reads, or production dKMS.
- Spaces/WebSpace foundation and live byte sync are complete for this branch.
  Runtime has persistent mounts, resolver adapter registry/health, object heads,
  cache/sync/fork receipts, local materialization, adapter
  `metadata_index`/`read_bytes`/`write_bytes` contracts, provider-to-provider
  adapter invocation, durable byte-cache status, Documents viewer handoff,
  resolver availability hints, installed `operator-drive-adapter`, redacted
  endpoint status/receipts, deterministic local adapter storage, and a
  filesystem-backed operator endpoint proof.
- Multi-peer availability proof foundation is complete for this branch. Carrier
  provider invocation can prove remote replicas for supported exact/manifest
  paths; receipts include peer-selection, quota, accounting, abuse-control,
  repair-worker, storage-market posture, and capped remote proof metadata.
- Storage accounting and quota admission are complete for this branch. Signed
  availability receipts project into a durable per-principal ledger; publish,
  exact import, and manifest-object import can enforce principal storage quota
  before bytes enter the local content backend.
- Cross-provider admission and repair policy are complete for this branch.
  `content-provider` exposes signed provider-only admission receipts, Carrier
  verifies remote admission before moving bytes or DAG repair data, and
  arbitrary DAG repair uses the Runtime-only block-graph provider path instead
  of unsafe exact-byte fallback.
- Operator availability/status surfaces are complete for this branch. Operators
  can inspect provider-wide or per-CID content status, storage accounting,
  quota, repair graph, repair workers, peer proof, storage-market admission
  posture, settlement posture, external repair-fleet posture, alerting posture,
  peer reputation posture, and configured endpoint-quorum exchanges through
  Runtime provider invocation.
- Archive support is complete for enabled families in this branch. ZIP, tar,
  tar.gz, and tgz download/list/preview/selective extract are provider-owned;
  generic non-tar/non-zip families are detected and policy-gated; Archive
  Manager exposes safe browse/import/extract UX and WebSpace archive policy
  without viewer-side extraction or raw provider access.

### Remaining Product Tracks

- Production multi-peer availability and storage markets require real external
  production infrastructure and remain outside this branch scope. Required proof
  includes production provider-network quota federation, production abuse/ban
  ledgers, production peer reputation and revocation exchange, production
  storage-market pricing/SLA/settlement/escrow, repair-fleet SLA/settlement,
  and production operator dashboard/subscription infrastructure. Do not close
  this with another local endpoint shim, receipt-only schema, policy/status
  surface, or documentation-only claim.
- Future generic archive work should be a format-specific dependency and
  release-policy approval. Do not reopen the completed ZIP/tar behavior unless
  a real regression is found.
- Production encrypted-content backends remain a Trusted content/access-rights
  follow-up: real encrypted payload generation, production dDRM policy reads,
  production dKMS/key release, and production decrypt/render backends.

### Release Decision

- This Library release can be considered feature-complete for the current branch
  scope: Library Explorer UX, the object-provider capsule/API boundary,
  provider invocation and streaming, recipient-scoped sharing proof,
  Spaces/WebSpace foundation,
  archive manager for enabled families, and branch-local multi-peer
  availability proof/status surfaces.
- Remaining work before publishing is release validation: human Chrome-profile
  testing, live signed Home/Library smoke with a real session, release notes,
  version selection, and operator-key publication.
- Do not keep renaming the gaps. Future deferrals should map to production
  multi-peer/storage-market infrastructure, format-specific archive dependency
  approval, or production Trusted content/access-rights backends.

### Completion Audit 2026-06-06

- Objective checked: complete the protected-recipient and availability/storage
  proof work in this file and run a full entropy/alignment pass.
- Branch-local protected-recipient and availability/storage proof work is
  verified complete for this branch:
  protected recipient receipt-chain tests, content status/dashboard tests,
  repair-worker tests, Carrier replication/admission/block-graph tests,
  storage-accounting/quota/admission tests, standalone `availability-provider`
  metadata/fanout tests, configured storage-market endpoint-quorum admission tests,
  configured federated quota-ledger endpoint-quorum exchange tests,
  configured federated abuse-control endpoint-quorum exchange tests,
  configured external repair-fleet endpoint-quorum dispatch tests, operator alert receipt/sink
  tests, configured federated operator alert-exchange tests, configured Carrier peer-attestation endpoint-quorum exchange tests,
  `cargo check`, `cargo clippy`, WCI alignment, Home entropy, and whitespace
  checks pass.
- Full entropy sweep on 2026-06-06 verified the active protected-recipient and
  availability/storage truth surfaces:
  `git diff --check`, `node scripts/home-entropy-check.mjs`,
  `bash scripts/check-wci-alignment.sh`, and stale-marker searches for retired
  mutable-object provider package naming plus old numbered gap phrases pass. Remaining
  `WebSpaces` strings are intentional internal URI namespace strings behind the
  user-facing `Spaces` label; unrelated `legacy`/`fallback` hits are migration,
  protocol-mode, generated emulator, or explicit fail-closed repair-path
  contexts outside the current completion claim.
- Production multi-peer availability and storage markets are now explicitly
  marked as a `TASKS.md` blocker, not branch-local release work.
  It still requires real external federation/market/repair/alerting
  infrastructure: production independent provider-network quota-ledger
  federation beyond the configured bounded endpoint quorum, storage-market
  pricing/SLA/settlement beyond the configured admission gate, repair-fleet
  worker attestation/SLA/settlement beyond configured dispatch quorum, network
  abuse ledgers/banlists beyond the configured bounded abuse-control endpoint
  quorum, and production federated operator dashboards/UI beyond the configured
  alert-exchange endpoint plus production peer reputation trust policy beyond
  the configured Carrier peer-attestation endpoint quorum.
- No more local-only status schema should be added to close that gap. Resume it
  only when real external production infrastructure is available for testable
  execution.

## Current State

- `library` launches from Home and uses `/api/provider/object/:op` to reach the
  Runtime object provider.
- `object-provider` is currently the provider capsule for principal-root
  storage, object lifecycle, object events, WebSpaces resolver roots, and
  share/status projection. A Runtime provider-to-provider invocation envelope
  now exists and `content-provider` uses it for internal provider effects; the
  provider plane now has explicit local and Carrier transports plus a bounded
  provider stream envelope, so
  Library content-backed operations remain explicitly Runtime/provider mediated.
- Target provider split is: `library` app -> Runtime -> mutable object provider
  (`object-provider` package on the canonical `object` Runtime scheme) ->
  Carrier-backed `content-provider` for published content and availability.
- Ontology priority: keep `object-provider` as the principal-root object
  authority and `content-provider` as the published-content identity,
  availability, and Carrier-backed delivery authority. The provider naming work
  is closed for this branch: the object provider has one package, one manifest,
  one Runtime scheme, and one browser provider route.
- `content-provider` now treats `carrier_announced` as an auditable
  availability state. When no external availability provider is configured,
  Runtime registers a built-in Carrier availability provider that signs and
  announces published CIDs on deterministic Carrier topics instead of exposing
  raw Kubo/IPFS/peer authority to apps.
- `content-provider` now owns the fetch decision too: it tries the local
  CID backend first, then asks the internal availability provider for the same
  CID/path if the local cache misses. Apps still use one `elastos://content/*`
  surface.
- Carrier now has an internal `content_fetch` byte operation on its file ALPN:
  connected Runtime peers can request CID/path bytes, and the serving Runtime
  reads from its local `ipfs-provider` through the provider registry. This is
  Runtime infrastructure, not app-visible Carrier or Kubo authority, and is now
  the narrow compatibility/bootstrap path rather than the availability fetch
  policy path.
- Carrier availability announcements now carry signed internal fetch
  descriptors. On local cache miss, `content-provider` can ask the built-in
  availability provider to verify matching announcements and fetch CID/path
  bytes through generic Carrier `provider_invoke` to the remote `content`
  service provider without exposing tickets, peer handles, or Kubo/IPFS to the
  app. That Carrier availability fetch now requests `transfer: "stream"` and
  decodes the validated provider stream envelope back into bytes for the
  availability response.
- Availability receipts now carry explicit `peer_selection`, `quota`, and
  `repair_worker` metadata. Local-only publishes honestly report
  `single_local` with no live multi-peer proof; configured availability
  providers may pass through richer Carrier/supernode policy metadata.
- Built-in Carrier availability now records an enforced replica-count quota
  verdict in availability receipts: effective max replicas, used replicas,
  `within_quota`/`at_quota`/`requirements_exceed_quota`, and explicit
  impossible-requirement state. Signed content receipts now also carry
  `elastos.content.accounting/v1` local accounting metadata with observed
  file/byte counts and replica-byte estimates when the provider operation
  exposes them. Signed receipts also carry
  `elastos.content.abuse-controls/v1`; the Carrier path records candidate
  limits, attempted remote provider invocations, failure counts, and whether
  the local attempt cap throttled candidates. This is bounded provider-plane
  enforcement and local receipt accounting/guardrails, not a full external
  storage-accounting market.
- Built-in Carrier availability also scores signed candidate announcements
  deterministically from local metadata plus bounded local success/failure
  history before provider invocation. Runtime startup loads and persists that
  local peer reputation under system content state. Remote peer-selection
  receipts include redacted score, selection reason, and local reputation
  reason, while federated peer reputation remains a production policy slice.
- `content-provider` now persists an auditable
  `elastos.content.repair-task/v1` ledger under Runtime system content state.
  Publish/ensure/repair/unpublish record local-only, queued, healthy, and
  retired task states, `status` includes the latest task, and provider-only
  `repair_worker` now requires a Runtime provider invocation envelope, so app
  capsules cannot invoke autonomous repair directly. It can retry queued CIDs
  through the same local pin plus availability-provider ensure path and returns
  explicit local quota/abuse-control guardrail receipts for run limits, attempt
  budgets, and failure throttling. Operators can trigger it with
  `elastos content repair-worker`, which routes through Runtime
  provider-to-provider invocation instead of raw provider JSON. Servers can
  also enable the same bounded loop with
  `ELASTOS_CONTENT_REPAIR_SCHEDULER=true`; it is opt-in, minimum-interval
  guarded, and uses the same run limit, attempt budget, and failure budget
  controls as the manual worker. With the built-in Carrier availability
  provider, that retry path now uses signed Carrier availability announcements
  to select remote peers, keeps at least one remote provider invocation in the
  candidate budget when a live multi-peer proof is required and quota permits
  it, preflights the remote peer's `content/admission` contract before moving
  bytes, invokes the remote peer's `content/ensure`, verifies the same CID with
  remote `content/status`, falls back first to bounded
  `content/import_object` reconstruction for manifest-backed objects and then
  to bounded `content/import_exact` byte push for file-like CIDs when remote
  pin cannot fetch the object, records `network_available` only when an
  independent remote provider proves a live pinned replica, verifies and
  summarizes the remote content provider's signed availability receipt when
  present, including safe peer-selection replica counts plus capped redacted
  score/reason/local-reputation rows with explicit cap/truncation metadata,
  quota, repair-worker, and accounting plus abuse-control posture, and emits
  explicit peer-selection/quota/repair-worker metadata without leaking Carrier
  connect tickets. This is the first autonomous cross-peer repair/replication
  proof path; production peer markets, production arbitrary-DAG repair
  scheduling/fleet execution beyond the current block-graph proof path,
  production scheduling policy beyond the current opt-in bounded loop, network
  abuse policy beyond local provider-invocation guardrails, federated peer
  scoring beyond durable local reputation, richer remote/multi-peer dashboards
  beyond the capped provider status rows, production storage-market admission
  across independent provider networks, and production independent
  provider-network quota-ledger federation beyond the configured bounded
  endpoint quorum still remain separate product
  slices. The
  provider-owned no-CID `content/status` dashboard already summarizes the
  current signed receipt, storage-accounting, and repair-task ledgers,
  including quota verdict, per-principal files/bytes/replica estimates, local
  accounting, abuse controls, live proof, remote replica, verified remote
  receipt counters, and capped recent remote-replica proof rows with redacted
  peer-selection score/reason and local-runtime reputation fields.
- Runtime provider-to-provider bounded byte transfers now apply requested
  ranges to provider `data.data` base64 payloads and validate progress
  `expected_bytes` when supplied. Provider-to-provider calls now inject a typed
  `_runtime_invocation` envelope (`elastos.provider.invocation/v1`) into the
  target provider request and mirror its source/target/op capability string in
  transfer receipts, so target providers can audit Runtime-mediated local
  provider-plane authority. `content-provider` fetch now propagates requested
  `range`/`progress` contracts to both local IPFS and availability-provider
  fallback reads and returns the provider transfer receipt with the fetch
  response. Carrier provider invocation now routes through an explicit
  `carrier-provider-plane` transport: the Runtime registry requires a registered
  Carrier invoker, attaches the same invocation envelope, suppresses raw connect
  tickets from app-visible receipts, and the remote Carrier ALPN accepts a
  generic `provider_invoke` operation only for service-provider targets
  (`content`, `availability`, `rights`, `key`, `decrypt`, `drm`) instead of raw
  backends like `ipfs` or `localhost`. `ProviderTransfer::Stream` now uses a
  validated `elastos.provider.stream/v1` base64-chunk envelope with range and
  expected-byte validation. Runtime can open that transfer as a
  `ProviderStreamSession` with read-next backpressure, progress events, and
  cancel; `content-provider` fetch uses that session path for both local IPFS
  and availability-provider fallback reads, and Library object downloads return
  chunked HTTP body streams with explicit backpressure/cancel receipts. Source
  providers cannot spoof
  Runtime-only invocation/transfer metadata; requests that predeclare
  `_runtime_invocation` or `_runtime_transfer` fail closed before reaching the
  target provider.
- Principal roots, list, read/download, write/upload, mkdir, rename, move,
  copy, trash, restore, permanent delete, publish, unpublish, repair, share,
  status, provider-owned `.tar.gz`/`.zip` folder archive download,
  provider-owned `.tar.gz`/`.zip` selected-object archive download,
  provider-owned same-folder `Compress to ZIP` object creation for files,
  folders, and same-folder selections,
  `.tar`/`.tar.gz`/`.tgz`/`.zip` extraction, and typed events exist in the provider/gateway
  path.
- Share now records typed policy metadata. Library exposes an in-app share
  policy dialog for public-link and recipient-scoped sharing; supplied
  recipients create `elastos.library.share-grant/v1` records on the published
  object. The provider now has a recipient-scoped `shared_access` gate that
  fails closed for recipients without an active grant, records allowed and
  denied access decisions, validates optional Runtime recipient-proof context
  (`elastos.library.recipient-proof/v1`) for recipient-scoped opens, and
  returns explicit shared-open/key-release receipts for authorized recipients
  without exposing raw content-provider, Carrier, Kubo/IPFS, or host authority
  to Library. Gateway requests strip app-supplied recipient proof and inject a
  Runtime launch-grant proof only when the requested recipient equals the signed
  session principal and the session carries an active passkey proof binding.
  Library now exposes a `Check My Access` action for shared published objects
  that asks `object-provider` for the signed principal's `shared_access`
  receipt and renders the access decision, open contract, and key-release
  posture instead of making users infer remote access from raw share metadata.
- Library UI supports places/sidebar, breadcrumbs, grid/list, upload, new
  folder, text document, inline rename, drag/drop upload/move/copy,
  preview/open, file download, folder archive download, `Download as ZIP`,
  selected-object archive download, `Download Selected as ZIP`,
  provider-backed `Compress to ZIP` and `Compress Selected to ZIP`,
  `Extract Here` for `.tar`/`.tar.gz`/`.tgz`/`.zip` archives,
  publish/unpublish/share, public-link share receipt, signed-principal access
  check, status/repair detail, trash/restore/delete,
  properties, sort, show hidden, SSE refresh, and browser Back/Forward takeover.
- Home projects `localhost://Users/<principal>/Desktop` through
  `object-provider`; Home owns desktop placement/opening only and delegates
  Desktop file/folder `Properties` and `Download` back into Library with a
  signed `objectUri/action` launch instead of duplicating object-provider
  authority in the shell.
- Documents can open and save a concrete Library object through
  `/api/viewers/documents/library-object`.
- Legacy plaintext objects in the protected principal root are auto-protected
  by `object-provider` on first access.
- Browser-native `prompt`, `alert`, and `confirm` are not used for Library
  object actions.
- Context menus now use PC2-style first-level groups: `Open With` for installed
  viewers, `Sort By`, `View`, and `New`. Byte-bearing files expose
  `Copy Content CID`; published objects additionally expose `Copy Published
  Link` instead of IPFS-branded UI copy.
- Sidebar right-click follows PC2 Explorer: sidebar chrome/title/blank space
  suppresses the browser-native context menu, while sidebar place items expose
  a small Library menu with `Open` and `Open in New Window`.
- Home now explicitly authorizes signed `library -> library` open-target
  messages, so Library sidebar `Open in New Window` can open another Library
  window without weakening the source-gated Home message policy.
- Folder item context menus now also expose `Open in New Window` for active
  directories, matching PC2 Explorer while still routing through Home's signed
  `library -> library` open-target policy.
- Sidebar right-click suppression now covers the whole sidebar, including the
  Favorites title and empty sidebar area, so the browser-native context menu is
  not exposed there.
- Sidebar active state now chooses the most specific matching root, so Home
  does not remain selected while Desktop/Documents/Public/Spaces are active.
- Sidebar places now support PC2-style drag reorder as a local user preference:
  provider roots remain provider-owned, while `library.sidebarOrder` persists
  the visible order by root ID and survives reload.
- Published/blocked/trash badges are static Explorer layout elements, not icon
  overlays. Published state appears below the filename in icon view and inside
  the name column in list view.
- Empty folder states are centered in the content pane, and Spaces empty
  state explains that mounted provider-backed spaces appear only when a
  WebSpace resolver is available. Read-only resolver mounts stay explicitly
  read-only; mutable WebSpace mounts expose provider-backed write/mkdir/delete
  affordances only when the WebSpace provider marks the current handle writable.
- Library now treats Spaces as mounted/indexed resolver views in the UI:
  `localhost://WebSpaces` lists mounts such as `Elastos` and indexed external
  spaces such as `Google`, and traversal like
  `localhost://WebSpaces/Google/Drive/Project X/file.pdf` remains read-only
  while exposing resolver metadata and raw Runtime download/open affordances.
- Library now also honors writable WebSpace metadata: mutable mounts/forks can
  create folders, upload/write files, read materialized bytes, and permanently
  delete local materialized objects through Runtime -> `webspace-provider`
  without exposing raw resolver targets or host paths.
- `webspace-provider` now has a persistent mount table under Runtime data,
  typed mount/unmount/list/index operations, a persistent resolver index table,
  a persistent local object table for mutable materialized WebSpace objects,
  provider-owned refresh/cache/sync lifecycle receipts for resolver metadata
  and fork heads, metadata health reports for mounted-no-index,
  metadata-ready, and dirty-head states, provider-backed `write`/`mkdir`/`delete`
  for non-readonly user mounts, and CLI support for
  `elastos webspace mounts|mount|unmount|index|health|refresh|head|cache|cache-status|sync|sync-status|fork`. Built-in
  `Elastos` remains reserved; custom mounts such as
  `localhost://WebSpaces/Google/...` map indexed local handles to resolver
  targets such as `google://drive/...` without exposing provider credentials or
  transport handles to Library.
- Async SVG hydration has been removed from folder rendering. Icons are stable,
  the sidebar stays mounted, visible/sorted objects are cached, and hot-path
  renders are measured.
- Library caches successful folder listings, prefetches root folders after
  initial load, and uses cached listings for navigation/back/forward while
  refreshing through the provider. Mutating provider operations clear the cache
  so authority remains provider-owned.
- Large-folder rendering now uses PC2-style keyed item reuse plus chunked
  first paint: unchanged item DOM nodes are reused by URI/signature, the first
  visible chunk paints immediately, and remaining rows append across animation
  frames.
- Upload progress rendering is frame-coalesced, so local file read progress does
  not repaint the upload panel for every browser `FileReader` progress event.
- `capsules/library/index.html` is a static shell. Active Library modules are
  `library.css`, `src/app.js`, `src/actions.js`, `src/api.js`,
  `src/dialog.js`, `src/editor.js`, `src/events.js`, `src/menu.js`,
  `src/model.js`, `src/navigation.js`, `src/preview.js`,
  `src/realtime.js`, `src/render.js`, `src/selection.js`, `src/state.js`,
  and `src/uploads.js`.
- `src/editor.js` owns inline create/rename behavior: draft object insertion,
  rename input lifecycle, Enter/Escape/blur handling, and provider-backed
  commit callbacks injected from `src/app.js`. It does not own provider
  routing, raw storage, content, Carrier, Kubo, network, wallet, chain, or host
  filesystem authority.
- `src/events.js` owns Library UI event binding for places, breadcrumbs,
  toolbar buttons, content click/double-click/context menu, drag/drop,
  keyboard shortcuts, browser history popstate, and unload cleanup. It receives
  all Runtime/provider actions by injection and has no direct provider/backend
  authority.
- `src/state.js` owns in-memory Library state initialization, perf counters,
  visible-object filtering/sorting cache, folder-listing signatures, folder
  cache writes, and the mutating-provider op set used to clear stale local
  caches. It has no provider, storage backend, Carrier, Kubo, network, or host
  filesystem authority.
- `src/render.js` owns the content render hot path: empty state, grid/list row
  construction, list headers, footer counts, view toggles, keyed item DOM reuse,
  chunked large-folder rendering, and first-paint telemetry. `src/app.js` keeps
  provider orchestration and event wiring.
- `src/preview.js` owns preview reads and blob URL lifecycle for text, image,
  video, audio, and PDF previews. Preview bytes still come through the Runtime
  object provider; the preview module has no direct storage authority.
- `src/realtime.js` owns Library SSE/EventSource lifecycle, reconnect timers,
  current-folder event matching, and debounced refresh scheduling. It only calls
  the injected `loadCurrentFolder` path and has no raw provider/backend
  authority.
- `src/actions.js` owns provider-backed user/object actions: open/viewer
  handoff, upload/download, publish/share/status/repair, trash/restore/delete,
  clipboard paste/move/copy, and text/folder creation. It receives Runtime
  provider and Home/viewer helpers by injection and does not gain raw storage,
  content, network, Carrier, Kubo, or host filesystem authority.
- Home service-worker registration is intentionally disabled for now, and the
  deployed service worker self-unregisters while clearing old `elastos-home-*`
  caches. This prevents stale browser-profile cache state from masking Runtime
  shell/module deployments during active development.

## PC2 Code Comparison

- Checked `Elacity/pc2.net` `main` at
  `a0a910158bd67666a6d3ea2a775ce09005ba7ae7`, matching the recorded PC2
  baseline in `docs/FILE_MANAGER_MIGRATION.md`.
- PC2 UI reference files inspected: `src/gui/src/UI/UIItem.js`,
  `src/gui/src/UI/UIDesktop.js`, `src/gui/src/helpers/open_item.js`,
  `src/gui/src/helpers/refresh_item_container.js`,
  `pc2-node/src/api/filesystem.ts`, and `pc2-node/src/api/file.ts`.
- Runtime Library now matches the core PC2 Explorer affordances: item anatomy
  with icon/name/details/badges, inline name editor, double-click open,
  Desktop as an item container, context menu/taphold shape, sort/view controls,
  Shift-click range selection, Enter-open of selected objects, keyboard context
  menu open, upload, drag/drop move/copy, Trash/restore/delete,
  publish/share/status, properties, folder/sidebar `Open in New Window`,
  selected-object archive download, `.tar`/`.tar.gz`/`.tgz`/`.zip` extraction,
  and viewer handoff.
- Runtime Library intentionally rejects PC2 authority shortcuts found in the
  code: username-to-wallet path rewriting, `/null` path fallbacks,
  signed file URL shortcuts, broad global socket/session assumptions, direct
  GUI access to filesystem APIs, app-visible IPFS/gateway paths, and wallet
  address roots as filesystem truth.
- Current Runtime alignment is therefore product-level PC2 UX on ElastOS rails,
  not a PC2 backend transplant: `library` stays UI-only; Runtime injects the
  principal; `object-provider` owns mutable principal-root object state;
  `content-provider` owns published content; Kubo/IPFS remains behind
  `content-provider`; Home only projects Desktop through provider summary.

## Verified Locally

- `node scripts/library-menu-smoke.mjs` now covers PC2-style nested
  `Sort By`, `View`, `New`, and `Open With` context-menu groups, suppressed
  sidebar right-click, single active sidebar place, centered empty states, and
  mounted/indexed Spaces traversal/read-only behavior through
  `Google/Drive/Project X/file.pdf` and `Elastos/content/<cid>` handles. It
  also asserts Published badge placement in
  icon/list views and proves framed Library sidebar and folder item
  `Open in New Window` emit signed Home `open-target` requests for `library`.
  Grid, list, and framed selected-name double-clicks are covered so open wins;
  rename remains explicit through context menu or F2. Active rename-editor
  double-clicks now follow PC2's guard and cannot also open/read the item.
  Shift-click range selection, Enter-to-open every selected object, and
  Shift-F10/ContextMenu-key selected-object menus are covered in list view so
  keyboard and range-selection affordances stay PC2-familiar without bypassing
  Runtime provider rails.
  Multi-select `Download Selected` is covered through the raw Runtime download
  route with repeated selected object URIs, and folder/selection
  `Download as ZIP` actions are covered through the same raw route with
  `archive=zip`. Provider-backed `Compress to ZIP` and
  `Compress Selected to ZIP` are covered through the `compress_archive`
  operation and create normal protected Library ZIP objects. `Extract Here` is covered for
  `.tar`/`.tar.gz`/`.zip` archives through the provider-owned
  `extract_archive` operation. The smoke also covers public-link and
  recipient-scoped share dialog flows without browser-native prompts.
- `cargo test --manifest-path elastos/Cargo.toml -p elastos-server gateway_tests::library`
  now also covers raw ZIP archive downloads for folders/selections, unsupported
  archive-format rejection, provider-created ZIP objects for folders/selections,
  WebSpace resolver metadata projection,
  recipient-scoped Library share-grant records, and GBA Emulator viewer
  discovery for ROM objects.
- `cargo test --manifest-path capsules/webspace-provider/Cargo.toml` covers
  WebSpace provider resolver metadata in list/stat responses.
- `node scripts/library-performance-smoke.mjs`
- `env -u ELASTOS_HOME_TOKEN -u ELASTOS_HOME_COOKIE -u ELASTOS_COOKIE -u ELASTOS_HOME_COOKIE_JAR -u ELASTOS_COOKIE_JAR scripts/library-live-smoke.sh`
  verifies public Library shell/module deployment and skips the signed provider
  path cleanly when no signed browser session is available.
- `ELASTOS_HOME_COOKIE=<signed-session> scripts/library-live-smoke.sh` passed
  against the live gateway after the realtime split deployment: Home launch
  minted a Library token, then roots, Public write, publish, status, share,
  trash, and cleanup all succeeded. Latest smoke CID:
  `QmZP7gu9fTs1XVtr3unHK9CzPgMBTyc9sJy24xnqkdCX3W`.
- `node scripts/home-entropy-check.mjs`
- `git diff --check -- capsules/library elastos/crates/elastos-server/src/library.rs elastos/crates/elastos-server/src/api/gateway_tests/library.rs scripts/home-entropy-check.mjs scripts/library-menu-smoke.mjs scripts/library-performance-smoke.mjs TODAY.md TASKS.md docs/FILE_MANAGER_MIGRATION.md`
- `cargo test --manifest-path elastos/Cargo.toml -p elastos-server gateway_tests::library`
- `cargo test --manifest-path elastos/Cargo.toml -p elastos-server test_home_summary_reports_identity_and_launch_targets`
- `cargo test --manifest-path elastos/Cargo.toml -p elastos-server content::tests::content_publish_accepts_carrier_announced_availability`
- `cargo test --manifest-path elastos/Cargo.toml -p elastos-server content_admission -- --nocapture`
- `cargo test --manifest-path elastos/Cargo.toml -p elastos-server federated_abuse_control -- --nocapture`
- `cargo test --manifest-path elastos/Cargo.toml -p elastos-server content_repair_worker -- --nocapture`
- `cargo test --manifest-path elastos/Cargo.toml -p elastos-server content_command_ -- --nocapture`
- `cargo test --manifest-path elastos/Cargo.toml -p elastos-server content_repair_scheduler -- --nocapture`
- `cargo test --manifest-path elastos/Cargo.toml -p elastos-server content_status_ -- --nocapture`
- `cargo test --manifest-path elastos/Cargo.toml -p elastos-server carrier::tests::test_`
- `cargo check --manifest-path elastos/Cargo.toml -p elastos-server`
- `cargo clippy --manifest-path elastos/Cargo.toml -p elastos-server --tests -- -D warnings`
- `bash scripts/check-wci-alignment.sh`
- Live `/apps/library/` serves the split Library shell, `library.css`, and
  `src/app.js` from the current branch assets.
- Live gateway restarted from
  `/home/wau/.local/share/elastos-public-gateway-live/elastos`; the previous
  live pass verified the installed `object-provider` checksum. Rerun live setup
  after publishing the canonical `object-provider` asset.
- Live `https://elastos.elacitylabs.com/apps/home/` and
  `https://elastos.elacitylabs.com/apps/library/` return 200 from the
  installed gateway. The deployed Library shell is the split HTML shell and the
  deployed dialog module contains the public-link share receipt path.
- Live `https://elastos.elacitylabs.com/apps/library/library.css`,
  `/apps/library/src/app.js`, `/apps/library/src/api.js`, and
  `/apps/library/src/menu.js` return 200 from the installed gateway.
- Live `/apps/library/src/app.js` now serves the cached/prefetch build
  containing `folderCache`, `folderCacheHits`, and `scheduleRootPrefetch`.
- Live `/apps/library/src/render.js` now serves the native-feel render build
  containing `objectNodeCache`, `renderContentChunks`, `LARGE_RENDER_THRESHOLD`,
  `INITIAL_RENDER_LIMIT`, and `initialRenderedCount`.
- Live `/apps/library/src/actions.js` now serves the action-boundary build
  containing `createLibraryActions`, `publishObject`, `pasteClipboardTo`,
  `uploadFiles`, and `copyText`.
- Live `/apps/library/src/editor.js` now serves the editor-boundary build
  containing `createLibraryEditor`, `startCreateObject`, `startRename`, and
  `startNameEdit`.
- Live `/apps/library/src/events.js` now serves the event-boundary build
  containing `bindLibraryEvents`, content/places event handlers, and
  drag/drop type detection.
- Live `/apps/library/src/realtime.js` now serves the realtime-boundary build
  containing `createLibraryRealtime`, `EventSource` lifecycle, current-folder
  event matching, and unload cleanup through an injected stop hook.
- Live `/apps/library/src/state.js` now serves the state/cache-boundary build
  containing `createLibraryState`, `visibleObjectsForState`,
  `cacheFolderListing`, `MUTATING_PROVIDER_OPS`, and upload render telemetry.
- Live `/apps/library/src/uploads.js` now serves the frame-coalesced upload
  progress build containing `scheduleUploadRender`, `uploadRenderCount`, and
  `uploadRenderScheduledCount`.
- Live `/apps/library/src/menu.js` and `library.css` now serve the nested
  submenu build containing `menu-submenu`, parent-scoped submenu open state,
  and left/right viewport flipping.
- Live smoke now checks served `src/app.js`, `src/events.js`, and
  `src/render.js` for the active-root, sidebar right-click suppression, and
  explicit Spaces empty-state fixes before running signed provider checks.
- Live Library publish backend is restored: the live runtime data root now has
  Kubo v0.40.1 at `xdg-data/elastos/bin/kubo`, `ipfs-provider` finds it on
  startup, direct IPFS add returns a CID, and live content publish/fetch returns
  the published smoke file.
- Exact live Object provider route smoke passes with a synthetic operator
  principal: write, publish, status, share, and delete cleanup all succeeded.
- Artificial-delay performance measurement with 1000 files and 80 ms provider
  list delay shows prefetched Desktop navigation around 75 ms, cached root
  navigation hitting twice, and the remaining large-folder cost dominated by
  roughly 60 ms DOM render time.
- `node scripts/library-performance-smoke.mjs` now asserts large folders render
  in chunks, root navigation hits the prefetched folder cache, and returning to
  a large folder reuses existing item DOM nodes. It also covers upload progress
  render coalescing during a provider-backed write.
- `scripts/home-live-smoke.sh` verifies live Home asset versioning, disabled
  service-worker registration, cleanup-worker behavior, module availability,
  and optional signed summary when a browser session cookie is supplied.

## Release Plan For Today

Release objective: publish a clean Library/Explorer-focused release candidate
after the remaining human/operator gates pass. Default version direction is the
next patch/minor release after 0.3.1; choose the exact version only after the
release branch scope is fixed.

### In Scope

- `library` app capsule: PC2-familiar Explorer UI, split static shell/modules,
  nested context menus, sidebar behavior, grid/list, inline create/rename,
  drag/drop, preview/open, upload/download, publish/share/status/repair,
  trash/restore/delete, browser Back/Forward takeover, and performance caching.
- `object-provider`: principal-root object authority for files, folders,
  Desktop/Documents/Public, revisions, Trash, encrypted protected-root storage,
  object events, legacy plaintext auto-protection, and WebSpace resolver
  routing/bridge support.
- Home/desktop projection: Home reads Desktop through `object-provider`; Home
  owns placement/opening only, uses object-aware keyboard open/context-menu
  handling for Desktop files and folders, delegates object actions back into
  Library, and service-worker registration remains disabled while the cleanup
  worker removes stale development caches.
- Documents viewer handoff: double-click/open for text/markdown-like objects
  launches Documents with the concrete Library object, not just the app shell.
- Content availability first slice: publish/share/status uses
  `content-provider` and `elastos://content/*`; Kubo/IPFS remains a local
  system backend; Carrier announcements and internal fetch descriptors are
  infrastructure, not app-visible peer or Kubo authority. Availability receipts
  include peer-selection/quota/repair-worker metadata. Availability-provider
  `network_available` and `carrier_announced` claims are now validated against
  requested replica/quota/live-proof requirements before they can become signed
  availability receipts; under-proven claims are recorded as `repair_needed`.
  The built-in Carrier availability provider can now turn signed remote
  announcements into live remote replica proof by invoking remote
  `content/ensure` and `content/status` over the Carrier provider plane, with
  a fail-closed `content/import_object` fallback for manifest-backed objects
  and `content/import_exact` fallback for file-like exact-CID byte push when
  remote pin cannot fetch the object. Repair-only announcements do not
  advertise fetch routes and cannot become replication candidates.
  Local-only receipts explicitly state that live multi-peer proof is not
  present.
- Spaces/WebSpace contract: Library shows `localhost://WebSpaces/<mount>/...` as a
  local mounted resolver view; provider targets such as `google://drive/...`
  remain resolver-private; `elastos://content/*` remains the
  provider-independent content identity. Read-only resolver mounts expose
  open/list/read/download/properties only. Mutable mounts/forks can materialize
  local WebSpace objects through provider `write`/`mkdir`/`delete`, persist them
  in `objects.json`, and expose `owner-writable` access-policy metadata without
  making mounted WebSpace views ordinary principal-root folders. Library object summaries
  now carry typed WebSpace resolver metadata
  (`elastos.library.webspace-object/v1`) with mount, provider, resolver state,
  read-only/access-policy state, and resolved target URI when the resolver
  provides one. `webspace-provider` list/stat responses now emit that resolver
  metadata directly; persistent mount, index, head, and object tables now allow
  resolver-discovered and locally materialized children to survive provider
  restarts.
- Viewer handoff now covers installed Documents for text/markdown/PDF and the
  installed GBA Emulator for `.gba`, `.gb`, and `.gbc` objects. Library still
  does not invent viewer authority; it only lists installed viewer capsules.
- Library Properties, availability status, and share receipts now lead with a
  SmartWeb object identity plus provider-owned availability summary instead of
  raw backend diagnostics; content IDs remain available as technical/copyable
  details.
- Library object identity is split deliberately: every readable local file has
  a current immutable raw-byte `content_cid` for its mutable object head, while
  public `elastos://` links use `published_cid` only after `content-provider`
  publish creates a signed published-content record and availability receipt.
  This keeps all files CID-based without treating private local objects as
  already-published content.
- Library now has one explicit Public-vs-Published rule: `Public` is placement
  metadata under the principal's Library root, while `published_cid` is the
  only public content-link truth. Moving or copying into `Public` does not
  silently publish; publishing creates the content-provider receipt and public
  `elastos://...` link. Published objects appear in `Public` only when the user
  also places/projects them there.
- Local mutable storage is still standard Runtime/provider-owned object
  storage, not "everything is IPFS." `object-provider` owns private mutable
  file/folder bytes and object heads; `content_cid` is the current byte identity
  for those private objects; `published_cid` is the separate public
  content-provider identity once availability receipts exist. Private files are
  SmartWeb object heads, while published files become globally addressable
  SmartWeb content records.
- `webspace-provider` now has a persistent adapter registry
  (`adapters.json`) plus `adapters`, `register_adapter`, and
  `unregister_adapter` provider ops and matching `elastos webspace` CLI
  commands. Health now reports configured/connected adapter counts, redacted
  adapter endpoint metadata, and per-mount adapter state (`not_registered`,
  `configured`, `connected`, `unavailable`, or `disabled`) instead of implying
  all external resolvers are anonymous unavailable mounts.
- `webspace-provider` now also has a safe adapter liveness receipt path:
  `check_adapter` records `ok`/`failed`/`skipped`/`unknown` checks with
  redacted health summaries, stale-check metadata, checked-adapter counts, and
  matching `elastos webspace check-adapter` CLI support. This is resolver
  readiness and operator health state only; it does not expose credentials or
  claim live external byte traversal.
- Library share/status/properties dialogs now surface a remote-access policy
  summary: public-link versus recipient-scoped, Runtime recipient-proof
  requirement, key-release status, and the current provider gate
  (`object-provider shared_access`). Backend share receipts now include
  `elastos.library.remote-access-policy/v1` so the UI can explain what is
  enforced now and what still needs drm/rights/key/decrypt providers.
- Protected-content provider contracts are explicit and fail closed:
  `drm-provider` advertises protected-content open orchestration,
  `rights-provider` advertises typed rights-decision authority only,
  `key-provider` advertises key-release receipts without raw CEK exposure, and
  `decrypt-provider` advertises viewer-scoped decrypt/render sessions without
  broad plaintext/filesystem authority. The key-release request contract now
  requires an allowed `elastos.rights.decision.receipt/v1` bound to the same
  principal/session/object/action, and the decrypt-session contract now requires
  a typed `elastos.release.receipt/v1` from `key-provider` bound to that same
  principal/session/object/action. Object-provider share/open receipts now
  carry `elastos.library.protected-content-provider-requirements/v1` so
  recipient-scoped sharing clearly names the drm/rights/key/decrypt chain required
  for future encrypted published payloads.
- Object-provider status/share/shared-access responses now also carry
  `elastos.library.protected-content-provider-status/v1` provider-chain
  readiness. Library status/share dialogs surface that readiness, and the share
  dialog shows encrypted-recipient sharing as a disabled fail-closed option
  until drm/rights/key/decrypt providers are configured and encrypted publish mode
  exists.
- Provider transfer receipts now include `elastos.provider.transfer-abi/v1`
  metadata. `ProviderTransfer::Stream` advertises Runtime stream-session mode,
  read-next backpressure, live progress events, and cancel support.
- Carrier availability receipts now include `elastos.content.storage-market/v1`
  policy metadata. The current mode is `carrier_provider_receipts` with
  `settlement: not_configured`, making live multi-peer proof distinct from
  production storage-market settlement.
- Library Properties now includes an archive support matrix for archive-like
  objects, showing implemented ZIP/tar/tar.gz download/extract behavior and the
  generic archive families still dependency/policy gated.
- Object metadata now recognizes generic non-tar/non-zip archive families such
  as `.7z`, `.rar`, `.tar.xz`, `.tar.bz2`, `.tar.zst`, `.xz`, `.bz2`, `.zst`,
  `.lz4`, and plain `.gz` as policy-gated archives. Library shows them with
  archive icon/properties context, advertises the installed `archive-manager`
  viewer, and exposes `Archive Support` so users can open safe policy
  inspection instead of unsafe extraction. Extraction remains disabled until
  dependency and release-policy review is complete.

### Not In Scope For This Release

- Frozen Hey Social work. Current branch history contains Hey work, but the
  Library release branch should exclude new Hey changes unless explicitly
  re-scoped.
- Full PC2 desktop/window/taskbar/socket behavior, app suggestions, thumbnails,
  and taskbar integrations. This release ports the Explorer file-management
  subset, not the whole PC2 shell.
- Production multi-peer availability and storage markets are not complete in
  this release because they require real external production infrastructure.
  The release ships Library Explorer UX, the object-provider capsule/API
  boundary, provider invocation and streaming, recipient-scoped sharing proof,
  Spaces/WebSpace foundation, archive manager for enabled families, and
  branch-local availability proof/status surfaces. Production dDRM/dKMS remains
  a Trusted content/access-rights follow-up track.
- AI Chat, dDRM, Elacity Marketplace, Mac VZ, and new Browser provider work.

### Release Blockers

- Worktree/commit hygiene: the Library-related release slices must stay
  isolated from frozen Hey files and unrelated dirty work. Do not tag until the
  current uncommitted Library/WebSpace/content-provider release changes are
  committed as coherent reviewable slices on `review/library-release`.
- Operator gate: rerun and keep passing `node scripts/home-entropy-check.mjs`,
  `node scripts/library-menu-smoke.mjs`, `node scripts/library-performance-smoke.mjs`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server gateway_tests::library`,
  targeted content/Carrier tests, `cargo check`, `cargo clippy`, `cargo fmt`,
  `bash scripts/check-wci-alignment.sh`, and `git diff --check`.
- Live gate: live Home and Library routes return 200, served Library assets
  contain the current split-module/WebSpace markers, public
  `scripts/library-live-smoke.sh` passes, and signed
  `ELASTOS_HOME_COOKIE=<signed-session> scripts/library-live-smoke.sh` passes
  when a human session cookie is available.
- Live upload gate: passed on 2026-06-06. Public Library `src/api.js` now calls
  only `/api/provider/object/*`, includes chunked large-file upload sessions
  (`/api/provider/object/upload/start`,
  `/api/provider/object/upload/:upload_id/chunk`, and
  `/api/provider/object/upload/:upload_id/finish`), and no longer serves the
  retired `/api/provider/library/upload` path. Remaining operator-sensitive
  check: the public edge proxy must allow the bounded chunk body size for
  `/api/provider/object/upload/:upload_id/chunk`; if a chunk is rejected before
  Runtime accepts it, Library must show the explicit
  "public gateway body-size limit" message, not raw nginx HTML.
- Human gate: normal Chrome profile signs in after the Home service-worker
  cleanup; Library feels native enough in that profile; no folder/view switch
  shows stale loading flicker or icon flashing.
- Release notes gate: changelog/release notes must describe the Library,
  content availability, Spaces/WebSpace, Home desktop, Documents handoff, and known
  non-goals without claiming production third-party WebSpace adapter ecosystem,
  production dDRM/dKMS, or production multi-peer storage-market readiness.

### Release File Scope

- Include Library/UI assets: `capsules/library/**`, `capsules/object-provider/**`,
  Library icons/CSS/modules, `capsules/library/capsule.json`, and the split
  shell `capsules/library/index.html`.
- Include Runtime/provider rails directly required by Library:
  `elastos/crates/elastos-server/src/library.rs`,
  `elastos/crates/elastos-server/src/api/gateway_tests/library.rs`,
  Library route wiring in gateway/viewer/provider-proxy files, provider
  registry wiring, support-provider test fixtures, and `components.json`
  entries for `library` and canonical `object-provider`.
- Include related user-facing integration: Documents viewer object handoff,
  Home Desktop projection, Home service-worker cleanup, Library live/perf/menu
  smoke scripts, `scripts/home-entropy-check.mjs`, WCI alignment updates,
  `scripts/publish-release.sh` support for canonical `object-provider`, and
  the Library/WebSpace/content availability docs.
- Include content availability only where it is required for Library
  publish/share/status: `content-provider` receipts, internal availability
  provider fallback, Carrier content-fetch/announcement support, and tests
  proving no app-visible Kubo/IPFS/peer authority.
- Exclude frozen Hey/social files unless explicitly re-scoped:
  `capsules/hey-social-rust/**`, `docs/HEY_CAPSULE_MIGRATION.md`,
  Hey-specific build tooling from the frozen branch,
  `elastos/crates/elastos-common/src/social_protocol.rs`, DID social-discovery
  additions, and social-only Carrier/gateway changes.
- Exclude debug/session artifacts unless they are intentionally turned into
  durable docs: `.gitignore` Hey WASM wildcard changes and `DEBUG.md` are not
  Library release material by default.

### Latest Gate Status

- Release worktree exists at
  `/home/wau/elastos-runtime-library-release` on branch
  `review/library-release`, based on local `main` at `6d4c385`.
- Post Archive closeout gate passed on 2026-06-06 19:48 UTC:
  `node scripts/library-menu-smoke.mjs`, `node
  scripts/library-performance-smoke.mjs`, `node
  scripts/home-entropy-check.mjs`, `bash scripts/check-wci-alignment.sh`,
  `git diff --check`, `cargo fmt --manifest-path elastos/Cargo.toml --all
  -- --check`, `cargo test --manifest-path elastos/Cargo.toml -p
  elastos-server gateway_tests::library -- --nocapture`, `cargo test
  --manifest-path elastos/Cargo.toml -p elastos-server archive_entries --
  --nocapture`, `cargo test --manifest-path elastos/Cargo.toml -p
  elastos-server selective -- --nocapture`, `cargo test --manifest-path
  elastos/Cargo.toml -p elastos-server webspace_archive -- --nocapture`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  content::tests::content_publish_accepts_carrier_announced_availability`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  content_admission -- --nocapture`, `cargo test --manifest-path
  elastos/Cargo.toml -p elastos-server federated_abuse_control --
  --nocapture`, `cargo test --manifest-path elastos/Cargo.toml -p
  elastos-server content_repair_worker -- --nocapture`, `cargo test
  --manifest-path elastos/Cargo.toml -p elastos-server content_command_ --
  --nocapture`, `cargo test --manifest-path elastos/Cargo.toml -p
  elastos-server content_repair_scheduler -- --nocapture`, `cargo test
  --manifest-path elastos/Cargo.toml -p elastos-server content_status_ --
  --nocapture`, `cargo test --manifest-path elastos/Cargo.toml -p
  elastos-server carrier::tests::test_`, `cargo check --manifest-path
  elastos/Cargo.toml -p elastos-server`, and `cargo clippy --manifest-path
  elastos/Cargo.toml -p elastos-server --tests -- -D warnings`. The
  performance smoke was corrected to use the canonical `/api/provider/object/*`
  fixture route with no legacy fallback, and Archive copy was made
  provider-neutral for WCI alignment.
- Post object-provider no-fallback gate passed on 2026-06-05 15:17 UTC in this
  worktree after freeing disk space: `cargo fmt --manifest-path
  elastos/Cargo.toml --all`, `cargo test --manifest-path elastos/Cargo.toml -p
  elastos-server gateway_tests::library`, `cargo test --manifest-path
  elastos/Cargo.toml -p elastos-server content_admission -- --nocapture`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  federated_abuse_control -- --nocapture`, `cargo test --manifest-path
  elastos/Cargo.toml -p elastos-server content_repair_worker -- --nocapture`,
  `cargo test --manifest-path elastos/Cargo.toml -p
  elastos-server content_command_ -- --nocapture`, `cargo test
  --manifest-path elastos/Cargo.toml -p elastos-server
  content_repair_scheduler -- --nocapture`, `cargo test --manifest-path
  elastos/Cargo.toml -p elastos-server content_status_ -- --nocapture`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  carrier::tests::test_`, `cargo check --manifest-path elastos/Cargo.toml -p
  elastos-server`, and `cargo clippy --manifest-path elastos/Cargo.toml -p
  elastos-server --tests -- -D warnings`.
- Latest lightweight gates remain green: `node scripts/home-entropy-check.mjs`,
  `bash scripts/check-wci-alignment.sh`, `git diff --check`, JSON/metadata
  checks for Runtime and `capsules/object-provider`, and the hard stale-marker
  sweep for retired object-provider fallback strings.
- Post signed remote admission receipt gate passed on 2026-06-06 UTC:
  `cargo fmt --manifest-path elastos/Cargo.toml -p elastos-server`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  content_admission_ -- --nocapture`, `cargo test --manifest-path
  elastos/Cargo.toml -p elastos-server carrier_replication -- --nocapture`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  test_carrier_availability_ensure_proves_remote_replica_via_provider_plane
  -- --nocapture`, and `cargo test --manifest-path elastos/Cargo.toml -p
  elastos-server
  test_carrier_availability_requires_remote_attempt_for_live_proof_when_min_met
  -- --nocapture`.
- Post signed-admission policy-surface regression gate passed on 2026-06-06 UTC:
  `cargo fmt --manifest-path elastos/Cargo.toml -p elastos-server`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  content_status_without_cid_returns_availability_dashboard -- --nocapture`, and
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  test_carrier_availability_ensure_proves_remote_replica_via_provider_plane --
  --nocapture`.
- Post configured storage-market endpoint-quorum admission gate passed on 2026-06-06 UTC:
  `cargo fmt --manifest-path elastos/Cargo.toml --all`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  storage_market_admission -- --nocapture`, `cargo test --manifest-path
  elastos/Cargo.toml -p elastos-server content_admission -- --nocapture`,
  including
  `content_admission_records_configured_storage_market_acceptance`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  content_admission_rejects_when_configured_storage_market_rejects`,
  `content_storage_market_admission_accepts_endpoint_quorum`,
  `content_storage_market_admission_rejects_endpoint_quorum_failure`, and
  `content_storage_market_admission_config`, `cargo test --manifest-path
  elastos/Cargo.toml -p elastos-server
  content_status_without_cid_returns_availability_dashboard -- --nocapture`,
  `cargo check --manifest-path elastos/Cargo.toml -p elastos-server`,
  `cargo clippy --manifest-path elastos/Cargo.toml -p elastos-server --tests
  -- -D warnings`, `git diff --check`,
  `node scripts/home-entropy-check.mjs`, and
  `bash scripts/check-wci-alignment.sh`.
- Post configured federated quota-ledger endpoint-quorum exchange gate passed on 2026-06-06 UTC:
  `cargo fmt --manifest-path elastos/Cargo.toml --all`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  federated_quota_ledger -- --nocapture`, `cargo test --manifest-path
  elastos/Cargo.toml -p elastos-server content_admission -- --nocapture`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  content_status_without_cid_returns_availability_dashboard -- --nocapture`,
  `cargo check --manifest-path elastos/Cargo.toml -p elastos-server`,
  `cargo clippy --manifest-path elastos/Cargo.toml -p elastos-server --tests
  -- -D warnings`, `git diff --check`, `node scripts/home-entropy-check.mjs`,
  and `bash scripts/check-wci-alignment.sh`.
- Post configured external repair-fleet dispatch gate passed on 2026-06-06 UTC:
  `cargo fmt --manifest-path elastos/Cargo.toml -p elastos-server`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  content_repair_worker_dispatches_configured_external_repair_fleet -- --nocapture`,
  and `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  content_external_repair_fleet_config -- --nocapture`.
- Post provider-local operator-alert sink gate passed on 2026-06-06 UTC:
  `cargo fmt --manifest-path elastos/Cargo.toml -p elastos-server`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  content_status_delivers_operator_alert_to_configured_loopback_sink -- --nocapture`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  content_status_without_cid_returns_availability_dashboard -- --nocapture`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  content_status_can_emit_operator_alert_receipt_without_sink -- --nocapture`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  content_operator_alert_sink_config -- --nocapture`, `cargo check
  --manifest-path elastos/Cargo.toml -p elastos-server`, and `cargo clippy
  --manifest-path elastos/Cargo.toml -p elastos-server --tests --
  -D warnings`.
- Post bounded-product-slice gate passed on 2026-06-05 15:46 UTC:
  `cargo test --manifest-path capsules/webspace-provider/Cargo.toml`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-runtime
  provider::registry::tests::test_provider_invocation_stream_normalizes_range_progress_transfer_receipt`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  content_fetch_stream_returns_provider_stream_payload`, `cargo test
  --manifest-path elastos/Cargo.toml -p elastos-server
  test_carrier_availability_ensure_proves_remote_replica_via_provider_plane`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  test_library_provider_records_recipient_scoped_share_grants`, `cargo test
  --manifest-path elastos/Cargo.toml -p elastos-server
  test_library_provider_rejects_key_release_policy_until_provider_exists`,
  `cargo check --manifest-path elastos/Cargo.toml -p elastos-server`,
  `cargo clippy --manifest-path elastos/Cargo.toml -p elastos-server --tests
  -- -D warnings`, `node --check capsules/library/src/dialog.js`, `node
  scripts/home-entropy-check.mjs`, `bash scripts/check-wci-alignment.sh`, and
  `git diff --check`.
- Post protected-content/archive-policy gate passed on 2026-06-05 16:09 UTC:
  `cargo test --manifest-path capsules/rights-provider/Cargo.toml`,
  `cargo test --manifest-path capsules/key-provider/Cargo.toml`,
  `cargo test --manifest-path capsules/decrypt-provider/Cargo.toml`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  test_library_provider_records_recipient_scoped_share_grants`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  test_library_provider_marks_generic_archive_families_policy_gated`,
  `node --check capsules/library/src/dialog.js`,
  `node --check capsules/library/src/model.js`,
  `cargo check --manifest-path elastos/Cargo.toml -p elastos-server`, and
  `cargo clippy --manifest-path elastos/Cargo.toml -p elastos-server --tests
  -- -D warnings`.
- Post protected-content provider-readiness UX gate passed on 2026-06-05
  17:10 UTC: `cargo fmt --manifest-path elastos/Cargo.toml --all`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  test_library_provider_records_recipient_scoped_share_grants`, `node --check
  capsules/library/src/dialog.js`, `node scripts/home-entropy-check.mjs`,
  `bash scripts/check-wci-alignment.sh`, `git diff --check`,
  `cargo check --manifest-path elastos/Cargo.toml -p elastos-server`, and
  `cargo clippy --manifest-path elastos/Cargo.toml -p elastos-server --tests
  -- -D warnings`.
- Post protected-content DRM-chain correction gate passed on 2026-06-05
  17:10 UTC: `cargo fmt --manifest-path elastos/Cargo.toml --all`,
  `cargo test --manifest-path capsules/drm-provider/Cargo.toml`,
  `bash scripts/protected-content-provider-contract-smoke.sh`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  test_library_provider_records_recipient_scoped_share_grants`, `cargo test
  --manifest-path elastos/Cargo.toml -p elastos-server
  test_library_provider_rejects_key_release_policy_until_provider_exists`,
  `node --check capsules/library/src/dialog.js`,
  `node scripts/home-entropy-check.mjs`,
  `bash scripts/check-wci-alignment.sh`, `git diff --check`,
  `cargo check --manifest-path elastos/Cargo.toml -p elastos-server`, and
  `cargo clippy --manifest-path elastos/Cargo.toml -p elastos-server --tests
  -- -D warnings`.
- Post protected recipient receipt-chain gate passed on 2026-06-05
  21:55 UTC: `cargo fmt --manifest-path elastos/Cargo.toml --all`,
  `cargo test --manifest-path capsules/drm-provider/Cargo.toml`,
  `cargo test --manifest-path capsules/rights-provider/Cargo.toml`,
  `cargo test --manifest-path capsules/key-provider/Cargo.toml`,
  `cargo test --manifest-path capsules/decrypt-provider/Cargo.toml`,
  `bash scripts/protected-content-provider-contract-smoke.sh`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  test_library_provider_runs_protected_content_receipt_chain_for_recipient
  -- --nocapture`, `cargo test --manifest-path elastos/Cargo.toml -p
  elastos-server
  test_library_protected_shared_access_fails_closed_without_providers
  -- --nocapture`, and `cargo test --manifest-path elastos/Cargo.toml -p
  elastos-server gateway_tests::library -- --nocapture`, `cargo check
  --manifest-path elastos/Cargo.toml -p elastos-server`, `cargo clippy
  --manifest-path elastos/Cargo.toml -p elastos-server --tests -- -D
  warnings`, `node scripts/home-entropy-check.mjs`, `bash
  scripts/check-wci-alignment.sh`, and `git diff --check` over the touched
  Library/protected-content/docs files.
- Post WebSpace adapter-health gate passed on 2026-06-05 17:10 UTC:
  `cargo fmt --manifest-path capsules/webspace-provider/Cargo.toml`,
  `cargo test --manifest-path capsules/webspace-provider/Cargo.toml`,
  `cargo check --manifest-path elastos/Cargo.toml -p elastos-server`, and
  `cargo clippy --manifest-path elastos/Cargo.toml -p elastos-server --tests
  -- -D warnings`.
- Post archive-support UX gate passed on 2026-06-05 17:10 UTC:
  `node --check capsules/library/src/app.js`, `node --check
  capsules/library/src/dialog.js`, plus stale protected-content wording and
  archive-support scans.
- Post WebSpace mutable resolver-sync gate passed on 2026-06-05 19:41 UTC:
  `cargo fmt --manifest-path elastos/Cargo.toml --all`, `cargo test
  --manifest-path elastos/Cargo.toml -p elastos-server
  test_library_gateway_syncs_operator_mutable_webspace_file_to_resolver`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  test_library_gateway_webspace_sync`, `cargo test --manifest-path
  elastos/Cargo.toml -p elastos-server
  test_library_gateway_mutates_writable_webspace_through_runtime_provider`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  gateway_tests::library`, `cargo check --manifest-path elastos/Cargo.toml -p
  elastos-server`, `node scripts/home-entropy-check.mjs`, and `bash
  scripts/check-wci-alignment.sh`.
- Post WebSpace resolver availability-hint gate passed on 2026-06-05 20:08
  UTC: `cargo fmt --manifest-path elastos/Cargo.toml --all`, `cargo test
  --manifest-path elastos/Cargo.toml -p elastos-server
  test_library_gateway_webspace_sync`, `cargo test --manifest-path
  elastos/Cargo.toml -p elastos-server
  test_library_gateway_syncs_operator_mutable_webspace_file_to_resolver`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  gateway_tests::library`, `cargo check --manifest-path elastos/Cargo.toml -p
  elastos-server`, `node scripts/home-entropy-check.mjs`, and `bash
  scripts/check-wci-alignment.sh`.
- Post installed operator WebSpace adapter gate passed on 2026-06-05 20:16
  UTC: `cargo fmt --manifest-path capsules/operator-drive-adapter/Cargo.toml`,
  `cargo test --manifest-path capsules/operator-drive-adapter/Cargo.toml`,
  `cargo clippy --manifest-path capsules/operator-drive-adapter/Cargo.toml
  --tests -- -D warnings`,
  `cargo check --manifest-path elastos/Cargo.toml -p elastos-server`, `cargo
  test --manifest-path elastos/Cargo.toml -p elastos-server
  test_library_gateway_webspace_sync`, `cargo test --manifest-path
  elastos/Cargo.toml -p elastos-server
  test_library_gateway_syncs_operator_mutable_webspace_file_to_resolver`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  gateway_tests::library`, `node scripts/home-entropy-check.mjs`, `bash
  scripts/check-wci-alignment.sh`, and `git diff --check`.
- Post operator WebSpace endpoint-backend gate passed on 2026-06-05 20:24 UTC:
  `cargo fmt --manifest-path capsules/operator-drive-adapter/Cargo.toml`,
  `cargo test --manifest-path capsules/operator-drive-adapter/Cargo.toml`,
  `cargo clippy --manifest-path capsules/operator-drive-adapter/Cargo.toml
  --tests -- -D warnings`, `cargo fmt --manifest-path elastos/Cargo.toml
  --all`, `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  operator_drive_adapter_config_prefers_explicit_json`, `cargo test
  --manifest-path elastos/Cargo.toml -p elastos-server gateway_tests::library`,
  `cargo check --manifest-path elastos/Cargo.toml -p elastos-server`, `cargo
  clippy --manifest-path elastos/Cargo.toml -p elastos-server --tests -- -D
  warnings`, `node scripts/home-entropy-check.mjs`, `bash
  scripts/check-wci-alignment.sh`, and `git diff --check`.
- Post WebSpace and provider-streaming closure gate passed on 2026-06-05 21:03 UTC:
  `cargo fmt --manifest-path capsules/operator-drive-adapter/Cargo.toml`,
  `cargo fmt --manifest-path elastos/Cargo.toml --all`, `cargo test
  --manifest-path capsules/operator-drive-adapter/Cargo.toml`, `cargo clippy
  --manifest-path capsules/operator-drive-adapter/Cargo.toml --tests --
  -D warnings`, `cargo test --manifest-path elastos/Cargo.toml -p
  elastos-server gateway_tests::library`, `cargo test --manifest-path
  elastos/Cargo.toml -p elastos-runtime provider::registry::tests::test_provider_`,
  `cargo test --manifest-path elastos/Cargo.toml -p elastos-server
  operator_drive_adapter_config_prefers_explicit_json`, `cargo test
  --manifest-path elastos/Cargo.toml -p elastos-server
  content_fetch_stream_returns_provider_stream_payload`, `cargo test
  --manifest-path elastos/Cargo.toml -p elastos-server
  content_fetch_stream_ranges_availability_provider_when_local_backend_misses`,
  `cargo check --manifest-path elastos/Cargo.toml -p elastos-runtime`, `cargo
  check --manifest-path elastos/Cargo.toml -p elastos-server`, `cargo clippy
  --manifest-path elastos/Cargo.toml -p elastos-runtime --tests -- -D
  warnings`, `cargo clippy --manifest-path elastos/Cargo.toml -p
  elastos-server --tests -- -D warnings`, `node
  scripts/home-entropy-check.mjs`, `bash scripts/check-wci-alignment.sh`, and
  `git diff --check`.
- Release-scope entropy check: no Hey/social implementation files or symbols
  remain in the clean release branch's modified runtime/test surface. Hey is
  mentioned only as an explicit frozen/excluded scope note.
- Latest bounded deferrals closed: selected-object archive download,
  provider-owned `.tar.gz`/`.zip` folder and selected-object archive downloads,
  provider-owned same-folder `.zip` object creation for single files, folders,
  and selected objects,
  provider-owned `.tar.gz`/`.tgz` extraction, provider-owned plain `.tar`
  extraction, provider-owned `.zip` extraction, and archive MIME classification.
  ZIP extraction dependency review is scoped to stable non-yanked `zip 2.4.2`
  with default features disabled and only the flate2-backed deflate path.
  Generic non-tar/non-zip archive extraction and import policy beyond current
  safe `.tar`/`.zip` extraction remain deferred. Archive now presents a
  simplified Archive flow: browse/search entries, preview safe files,
  extract selected/all files into Library, and keep release-policy/dDRM details
  collapsed behind secondary safety copy. WebSpace mutable resolver sync is now
  fixture-proven with adapter
  write-back, no-adapter fail-closed receipts, conflict receipts, and
  resolver-scope availability hints. The fixture contract is now also promoted
  into an installed `operator-drive-adapter` provider package with Runtime
  startup registration, release/build metadata, Runtime-only invocation
  enforcement, deterministic provider-owned local bytes, read-only/conflict
  policy, operator-private endpoint backend traversal/read/write, Runtime
  config loading, redacted endpoint status/receipts, and no
  credential/raw-backend exposure to apps. Unsupported archive families are now
  detected and labeled as policy-gated archives instead of being hidden as
  ordinary files.
- Latest product-deferral foundations closed: persistent WebSpace mount table
  and CLI, persistent WebSpace adapter registry and CLI, provider-owned
  WebSpace object heads/health/refresh/cache/sync/fork receipts, fake-adapter
  WebSpace `metadata_index`/`read_bytes` invocation contracts, clean
  non-dirty adapter byte-cache materialization in `webspace-provider`, Library
  Runtime reads that invoke connected resolver adapters through
  provider-to-provider `ProviderInvocation`, Runtime provider-to-provider
  invocation envelope with bounded byte range/progress handling,
  target-visible capability metadata, transfer ABI receipts, and
  transport-bearing transfer receipts including Carrier `provider_invoke`,
  `content-provider` fetch propagation of provider range/progress/stream
  transfer receipts,
  recipient-scoped Library `shared_access` with
  access-decision/shared-open/remote-access-policy receipts and denied-access
  audit, ZIP folder/selection archive download,
  ZIP extraction plus unsafe-entry fail-closed coverage, and availability
  receipt metadata plus requirements enforcement for peer-selection/quota/
  repair-worker/storage-market claims plus durable content repair-task
  ledger/worker pass.
  Network availability claims now also fail closed unless peer-selection
  metadata names a concrete mode or strategy.
- Post WebSpace adapter-byte cache gate passed on 2026-06-05: `cargo test
  --manifest-path capsules/webspace-provider/Cargo.toml
  cache_handle_materializes_adapter_bytes_without_dirty_sync_debt` and `cargo
  test --manifest-path elastos/Cargo.toml -p elastos-server
  test_library_gateway_reads_external_webspace_file_through_adapter_cache`.
- Post operator WebSpace fixture/viewer gate passed on 2026-06-05: `cargo test
  --manifest-path elastos/Cargo.toml -p elastos-server
  test_library_gateway_operator_webspace_adapter_caches_bytes_and_viewer`.
- Post WebSpace sync byte-cache gate passed on 2026-06-05: `cargo test
  --manifest-path elastos/Cargo.toml -p elastos-server
  test_library_gateway_webspace_sync_caches_adapter_bytes_without_foreground_read`.
- Live public Library assets pass `scripts/library-live-smoke.sh`.
- Post live object-provider deployment gate passed on 2026-06-06:
  `https://elastos.elacitylabs.com/api/provider/object/roots` now returns
  `403 missing home launch token` instead of `404 Gateway provider not found`,
  proving the public gateway has the canonical object-provider route. Public
  Library assets now serve the current chunked upload client
  (`CHUNKED_UPLOAD_TRANSPORT = "http-chunk-session"`,
  `/api/provider/object/upload/start`) and no longer serve the retired
  `/api/provider/library/upload` path. Public Properties assets now include the
  PC2-style `window-item-properties`, `item-props-tabview`, `Content ID`, and
  `Published CID` markers. The live process tree now has `object-provider` and
  no `library-provider`.
- Signed Home live smoke passes with a current Home session: live Home shell,
  module graph, cleanup service worker, and signed summary verified. Latest
  Home asset version: `home-20260603c`.
- Signed live Library smoke passes with a current Home session: Home launch
  minted a Library token, then roots, Public write/upload, provider-owned plain
  `.tar` extraction, publish, status, share, and cleanup all succeeded. Latest
  smoke CID: `QmW8h4rLgBvCMwxMGuVrEUagjJRkiny98h9Rk7YK6cbGCT`.
- Still needs final human proof before release: normal Chrome profile retest and
  perceived Library speed/native-feel pass on
  `https://elastos.elacitylabs.com/apps/home/`. This is intentionally left for
  the final manual testing pass.
- Current git hygiene status: branch `review/library-release` has the coherent
  Library release commit stack from `43a0b77` through the current branch head,
  but the worktree still contains uncommitted Library/WebSpace/content-provider
  release changes. Tag/release should happen only after those changes are
  committed, the same gates rerun, and the final human pass succeeds.

### Human Test Checklist

- Sign into `https://elastos.elacitylabs.com/apps/home/` with the existing
  passkey in a normal Chrome profile.
- Open Library from Home; verify Home, Desktop, Documents, Public, and
  Spaces sidebar selection is correct and no browser-native context menu
  appears on blank sidebar chrome.
- Create a folder on Desktop from Library and confirm it appears immediately in
  both Library Desktop and Home Desktop.
- Double-click folders and files, including already-selected names. Folders
  navigate, and text/markdown files open Documents with the concrete object
  instead of entering inline rename or only opening the Documents app shell.
  While rename is active, double-clicking the editor must not open the object.
- Right-click files/folders/sidebar places and verify only working menu items
  are visible; `Open`, `Open in New Window`, `Open With`, `Sort By`, `View`,
  `New`, Properties, publish/share/status/repair, trash/restore/delete, and
  rename behave as expected for the selected object/root.
- In list view, Shift-click selects a visible range, Enter opens every selected
  object through the same viewer/provider path as double-click, and Shift-F10 or
  the ContextMenu key opens the selected-object menu.
- Upload, download, folder/selection `Download as ZIP`, selected-object archive download,
  provider-backed `Compress to ZIP`, provider-backed `Compress Selected to ZIP`,
  `.tar`/`.tar.gz`/`.zip` extraction,
  rename, move, copy, drag/drop, create text document, move to Trash, restore,
  and permanent delete from the appropriate roots.
- Upload a large video-sized file. Library must use Runtime chunked upload
  sessions and commit through object-provider at `finish`; it must not send the
  whole file as one public `PUT`. If the operator keeps the public gateway chunk
  limit below the Runtime chunk size, Library must show the explicit
  `public gateway body-size limit` error instead of raw nginx HTML.
- Publish from Public, copy/share the published link, check status, repair, and
  unpublish. Published badges must appear in grid and list views without
  awkward icon overlays.
- Visit Spaces. Read-only mounts should be useful as mounted resolver
  surfaces with clear copy, no mutable actions, no raw `google://`, Kubo/IPFS,
  Carrier peer, or host authority exposed to the app. Mutable mounts/forks
  should expose New/Upload/Delete only when `metadata.readonly === false`.
- Use browser Back/Forward inside Library and confirm it navigates Library
  history rather than leaving Home or fighting the shell.

### Deferrals To Keep In TASKS.md

- Keep only canonical remaining product-track work in TASKS.md: production
  multi-peer/storage-market infrastructure, specific future generic archive
  dependency approvals, and production dDRM/dKMS under the Trusted
  content/access-rights section. Do not keep duplicate numbered gap wording.
- Do not describe the object-provider work as fully extracted. This branch has
  the standalone `object-provider` capsule/API boundary, `object` Runtime
  scheme, package/profile routing, and fail-closed standalone wrapper tests.
  The pure object-provider core still lives in `elastos-server::library`;
  extracting it into a smaller core crate is architecture/build-review cleanup,
  not a user-facing behavior fix.
- Do not track provider-to-provider Carrier invocation as a missing baseline.
  Runtime-native stream sessions are complete for this branch; future
  storage-market execution belongs to production infrastructure work.
- Start the next PC2 slices only after this Library release is clean: AI Chat,
  then dDRM and Elacity Marketplace.

### Remaining Execution Order

1. Complete the human Chrome-profile checklist above on
   `https://elastos.elacitylabs.com/apps/home/`.
2. Choose the coordinated release version intentionally after confirming the
   already-published `0.3.1` baseline. The version policy accepts dotted
   prereleases such as `0.3.2-rc.1` and rejects compact forms such as
   `0.3.2-rc1`.
3. Publish from `review/library-release` only with an operator release key:
   `scripts/publish-release.sh --version <version> --key <release.key>`.
   If the human pass fails, fix only the failing release blocker and rerun the
   same gates before publishing.
