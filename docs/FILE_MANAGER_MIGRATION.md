# Explorer Migration Plan

> PC2 baseline: `Elacity/pc2.net` `main`
> `a0a910158bd67666a6d3ea2a775ce09005ba7ae7` (`v1.3.0`).

This is the contract for bringing PC2's Explorer experience onto ElastOS
Runtime rails. The product goal is PC2-familiar file management. The security
goal is no PC2 authority shortcut.

## Decision

Use the existing `library` capsule as the first Explorer surface. Do not add
a second competing file app unless a later product decision explicitly renames
the surface. The visible UI should be a PC2-style Explorer; the Runtime
contract should be a typed object provider.

## Current Branch State

Implemented in this branch:

- Typed object provider rail exposed to the browser through the canonical
  `/api/provider/object/:op` route.
- Standalone `object-provider` capsule process for principal-root Library
  object storage and event authority; the `library` app remains UI-only and
  Runtime injects the signed principal before forwarding provider operations.
- Principal-scoped roots, list, stat, read/download, write/upload, mkdir,
  rename, trash, restore, delete-permanently, status, publish, unpublish,
  repair, share, provider-owned `.tar.gz`/`.zip` folder archive download,
  provider-owned `.tar.gz`/`.zip` selected-object archive download,
  provider-owned same-folder `Compress to ZIP` object creation for files,
  folders, and same-folder selections,
  `.tar`/`.tar.gz`/`.tgz`/`.zip` extraction, and typed object events.
- SSE event stream for active Library windows, with app-side refresh only when
  an event touches the current folder.
- Provider tests for token scope, principal isolation, traversal rejection,
  encrypted-root writes/reads, object lifecycle, content-provider publish
  handoff, no-content fail-closed behavior, share requiring an active published
  object, unpublish/repair status transitions, and typed event filtering.
- PC2-style Explorer UI with Places, breadcrumbs, grid/list views, upload, new
  folder, text document, inline draft naming, inline rename, context actions,
  file download, folder archive download, `Download as ZIP` for folders,
  selected-object archive download, `Download Selected as ZIP`,
  provider-backed `Compress to ZIP` and `Compress Selected to ZIP`,
  `Extract Here` for `.tar`/`.tar.gz`/`.tgz`/`.zip` archives, publish, share, unpublish,
  status, repair, public-link share receipt, signed-principal `Check My Access`
  receipt UX for shared published objects, properties, recoverable Trash
  actions for `.Trash` objects, and Chat Room attach mode.
- PC2-style first-level context-menu groups for installed viewers (`Open With`),
  folder sorting (`Sort By`), view mode (`View`), and creation (`New`). The app
  hides unsupported actions instead of shipping inert menu rows.
- PC2-style `Open in New Window` for sidebar places and active folder items,
  routed through Home's signed `library -> library` open-target policy instead
  of a PC2 global shell/window shortcut.
- Per-file upload progress in the UI, a raw Runtime upload route
  (`PUT /api/provider/object/upload`) for small browser file uploads, and
  Runtime upload sessions (`start` / bounded `chunk` / `finish` / `cancel`) for
  large uploads. User file uploads no longer travel through JSON/base64 `write`;
  text/editor writes still use the JSON provider operation intentionally.
- Raw download route (`GET /api/provider/object/download/raw`) for principal-root
  file/folder archive downloads, selected-object archive downloads, and
  WebSpace resolver/materialized files. Directory and selected-object archive
  downloads default to `.tar.gz` and accept explicit `archive=zip`. Library
  browser downloads no longer travel through JSON/base64 `download`; the raw
  route supports single HTTP byte-range responses. Preview/read paths still use
  the typed JSON provider operation intentionally.
- Raw upload/download routes emit Runtime transfer receipts (`schema`,
  `request_id`, `op`, `transport`, `status`, byte counts, and optional range
  metadata). Chunked large uploads add `elastos.object.upload-session/v1` /
  `http-chunk-session` receipt metadata so operator smokes can prove bounded
  browser chunks crossed the Runtime/audit boundary before the final
  object-provider commit.
- Multi-select with context-menu batch publish, unpublish, trash, restore,
  delete, cut, and copy through the same provider operations.
- PC2 Explorer visual parity pass: compact PC2 sidebar favorites, PC2
  SVG icon assets, icon-tile grid with PC2 dimensions, details-list
  columns/headers, navbar back/forward/up controls, PC2-like selection states,
  footer density, context-menu density, and F2 rename.
- Drag/drop move between folders/places through the typed provider `move`
  operation with revision checks and object events.
- Drag/drop copy between folders/places through the typed provider `copy`
  operation with revision checks and object events. Copy is explicit via
  Alt-drop or context-menu Copy/Paste.
- Browser-native Back/Forward is mapped onto Explorer folder history. The first
  Library history entry is guarded so browser Back does not accidentally leave
  the capsule.
- Large folder rendering now avoids full synchronous repaint: Library reuses
  unchanged item DOM nodes by URI/signature and paints large folders in chunks
  so first content appears before all rows are appended.
- Library rendering is split into `src/render.js` so the large-folder hot path,
  keyed DOM reuse, footer/view-state sync, and first-paint telemetry stay
  isolated from provider orchestration and object actions.
- Library preview handling is split into `src/preview.js` so provider-backed
  preview reads and blob URL lifecycle stay isolated from app orchestration.
- Library inline create/rename behavior is split into `src/editor.js` so draft
  object insertion, rename input lifecycle, Enter/Escape/blur handling, and
  provider-backed commit callbacks stay isolated from app orchestration. The
  module receives provider and refresh helpers by injection and does not own
  raw storage or content authority.
- Library object actions are split into `src/actions.js` so open/viewer handoff,
  upload/download, publish/share/status/repair, trash/restore/delete,
  clipboard paste/move/copy, and create actions are separated from the app
  event/render orchestrator. Authority still flows only through injected
  Runtime provider/Home helpers.
- Library state/cache ownership is split into `src/state.js` so in-memory UI
  state, visible-object filtering/sorting, folder-listing signatures, and
  stale-cache invalidation metadata are separated from provider orchestration.
  This module has no backend, Carrier, Kubo/IPFS, network, wallet, or host
  filesystem authority.
- Library UI event binding is split into `src/events.js` so places,
  breadcrumbs, toolbar clicks, content selection/open/context-menu behavior,
  drag/drop, keyboard shortcuts, browser history popstate, and unload cleanup
  are separated from provider orchestration. The module receives actions and
  navigation helpers by injection and does not call the provider directly.
- Library realtime refresh is split into `src/realtime.js` so SSE/EventSource
  lifecycle, reconnect timers, current-folder event matching, and debounced
  refresh scheduling stay isolated from the app shell. The module only calls
  the injected folder refresh path and does not own provider/backend authority.
- PC2 treats the desktop as an item container. Runtime mirrors that without
  giving Home filesystem authority: Home summary projects
  `localhost://Users/<principal>/Desktop` through `object-provider`, and Home
  only owns desktop placement/opening. File mutations remain in Library.
- Viewer routing only exposes installed viewer capsules from Runtime capsule
  manifests, with properties/details fallback when no verified viewer exists.
- Archive-like files advertise the installed `archive-manager` viewer. Archive
  Manager uses Runtime viewer routes for stat, supported-family entry listing,
  bounded safe entry preview, destination roots, and selected extraction. It
  never extracts unsupported bytes or touches host storage.
- Documents viewer routing can open and save a concrete Library object through
  `/api/viewers/documents/library-object`; the viewer route checks that
  Documents is an installed viewer for the object and keeps raw Library
  provider access scoped to the Library capsule.
- Legacy plaintext objects left in the protected principal root are
  auto-protected by `object-provider` on first access so old dev files regain
  normal capabilities without weakening the protected-root rule.
- Published object status/repair is exposed through the context menu and
  availability detail dialog, including content CID, status, publish/share
  timestamps, and provider receipts.
- Spaces are visible as resolver roots with explicit provider capability
  metadata. The Library UI must make readonly resolver mounts explicit: sidebar
  Spaces are navigation-only where appropriate, mutable actions are
  hidden/fail-closed for `readonly` handles, and an empty resolver state
  explains that mounted provider-backed spaces appear only when a WebSpace
  resolver is available. Mutable WebSpace mounts/forks may expose New/Upload/
  Delete only when the WebSpace provider marks the current handle writable.
  `localhost://WebSpaces/<mount>/...` is the Library-visible mounted handle;
  provider targets such as `google://drive/...` or backing
  `elastos://content/*` identities stay behind resolver/provider authority.
- Library object summaries expose typed WebSpace resolver metadata so mounted
  handles can show mount, provider, resolver state, read-only/access-policy
  state, and resolved target URI without exposing raw provider credentials or
  transport handles to the app.
- `webspace-provider` now persists mount and resolver-index tables plus
  provider-owned object-head/cache/sync/fork metadata and a local materialized
  object table for mutable WebSpace objects, exposes typed
  `mounts`/`mount`/`unmount`/`index`/`health`/`refresh`/`head`/`cache`/`cache_status`/`sync`/`sync_status`/`fork`/`write`/`mkdir`/`delete`
  operations, and has CLI support for the lifecycle/read-only resolver verbs through
  `elastos webspace mounts|mount|unmount|index|health|refresh|head|cache|cache-status|sync|sync-status|fork`.
  Custom mounts map `localhost://WebSpaces/<Moniker>/...` to resolver-private
  targets such as `google://drive/...`; resolver indexes can expose discovered
  child handles, health reports mounted-no-index/metadata-ready/dirty-head
  state, refresh can replace resolver metadata, and cache/sync can advance
  provider-owned metadata/fork heads without exposing provider credentials.
  Mutable mounts/forks can materialize local provider-owned bytes and folders;
  live external traversal, provider streaming, and remote sync workers remain
  separate provider responsibilities.
- WebSpace adapters also expose safe liveness receipts through `check_adapter`.
  Health can now distinguish configured/unchecked, connected/unverified,
  healthy, stale, unavailable, and disabled resolver adapters while keeping
  endpoint credentials redacted. This is an operator/provider readiness signal,
  not a claim that Runtime can traverse external bytes yet.
- The Runtime provider registry has a provider-to-provider invocation envelope.
  `content-provider` uses it for internal IPFS/availability effects, while the
  envelope now validates byte-range/progress receipt metadata and applies
  byte-range slicing to provider `data.data` base64 payloads for bounded
  `ProviderTransfer::Bytes` calls. `ProviderTransfer::Stream` now uses a
  validated `elastos.provider.stream/v1` base64-chunk envelope with the same
  range/progress contract, and Runtime opens it as a stream session with
  read-next backpressure, progress events, and cancel support.
- Published-object sharing now has a recipient-scoped `shared_access` gate for
  recorded Library share grants plus persisted key-release receipts. Library
  exposes an in-app share policy dialog for public-link and recipient-scoped
  grants, status/share dialogs show grant and key-release state, and
  authorized recipient checks return explicit access-decision and shared-open
  contracts only when Runtime recipient-proof state matches the requested
  recipient. Gateway requests strip app-supplied recipient proof and inject
  launch-grant proof only for the signed session principal when the session is
  passkey-proof-bound. Unauthorized recipients and recipient-scoped requests
  without Runtime proof fail closed and are audited. The protected-content
  provider contracts now bind key release to an allowed rights-decision receipt
  and bind decrypt/render to a typed key-provider release receipt. The branch
  also includes a non-production `protected_content_fixture` path that publishes
  a sealed-object descriptor, records recipient-scoped key-release grants,
  invokes DRM/rights/key/decrypt fixture providers, and returns a viewer-scoped
  protected-open contract without exposing raw keys or plaintext. Production
  encrypted payload generation, real dDRM policy reads, and production dKMS
  remain future Trusted content work.
- Content availability receipts now include peer-selection, quota, and repair
  worker metadata. Local-only receipts state that there is no live multi-peer
  proof; configured availability providers may pass through richer policy
  metadata.

Still pending:

- Richer type-aware previews and chooser-style Open With selection once more
  viewer capsules are available. Current installed-viewer handoff covers
  Documents for text/markdown/PDF and GBA Emulator for `.gba`, `.gb`, and
  `.gbc` ROM objects.
- Production protected-content backends and richer policy UX beyond the current
  fixture-backed receipt chain: approved dDRM rights reads, production dKMS key
  release, real encrypted payload generation, and production decrypt/render
  backends.
- Generic non-tar/non-zip archive extraction beyond current provider-owned
  `.tar.gz`/`.zip` folder and selected-object archive downloads, same-folder ZIP
  object creation, `.tar`/`.tar.gz`/`.tgz`/`.zip` browsing/preview/extraction,
  and WebSpace archive import/write-back policy. Policy-gated generic archive
  families are recognized and can open Archive Manager, but their entry
  browsing/importing remains disabled until dependency/release-policy review.
- Automated live external resolver adapters, external resolver byte traversal,
  byte cache/sync workers, mutable fork byte materialization, and
  external cloud/provider adapters beyond the current persisted
  mount/index/object-head/health/refresh/cache/sync/fork metadata receipts.
- Live multi-peer replication proof, enforced quotas, repair workers,
  peer-selection policy, and abuse controls beyond the receipt metadata.

## Source Of Truth

PC2 is the design and behavior reference for the first pass:

- `src/gui/src/UI/UIItem.js`
- `src/gui/src/UI/UIDesktop.js`
- `src/gui/src/IPC.js`
- `pc2-node/src/api/filesystem.ts`
- `pc2-node/src/api/file.ts`
- `pc2-node/src/api/storage.ts`

Use these files to match layout, object affordances, and interaction behavior.
Do not copy the Puter/PC2 authority model or broad-session assumptions.
Reimplement the behavior on Runtime contracts. Static PC2 icon assets may be
copied with provenance to preserve Explorer visual parity; PC2 runtime modules,
Puter authority code, and generated app bundles must not be transplanted.

## PC2 UX Parity Checklist

The Runtime Explorer should keep these PC2 behaviors unless they conflict
with ElastOS authority:

- Places/sidebar navigation for user roots and mounted spaces.
- Breadcrumb/current-folder state.
- Toolbar actions for upload, new folder, view mode, sort, and refresh when a
  manual refresh is useful.
- Grid and list views.
- PC2 item anatomy: icon or thumbnail, badge stack, divider, display name,
  hidden name editor, and list-mode attrs.
- List columns for modified time, size, and type.
- Sort by name, modified, size, and type.
- File/folder badges for shared/published/public/availability state.
- Type-aware icons and previews.
- Empty-folder state.
- Loading and error states that do not erase the current folder context.
- Upload by button and drag/drop.
- Upload/download progress.
- Explicit inline rename from context menu or F2, with Enter to commit, Escape
  to cancel, and blur to commit. Selected-name clicks must remain selection/open
  affordances so double-click open is reliable. Active rename-editor clicks,
  double-clicks, and context menus must stay inside the editor and not bubble
  into item open handling.
- Double-click/tap open.
- Context menu/taphold.
- Keyboard open: Enter opens every selected object through the same Runtime
  viewer/provider path as double-click.
- Keyboard context menu: ContextMenu key or Shift-F10 opens the selected-object
  menu, or the folder background menu when no object is selected.
- Background context menu for folder-level actions.
- Multi-select where the Runtime operation supports it.
- Shift-click range selection in the visible grid/list order, anchored to the
  last explicit selection.
- Multi-select archive download where all selected objects are downloadable
  principal-root objects in the same folder.
- `Extract Here` for provider-owned `.tar`/`.tar.gz`/`.tgz`/`.zip` archives.
- Drag/drop move and copy only when backed by Runtime operations.
- Delete moves recoverable objects to Trash before permanent delete.
- Create, rename, and destructive confirmation flows use in-app Explorer UI,
  not browser-native `prompt`, `alert`, or `confirm` dialogs.
- Properties/details view with name, URI, type, size, created/modified, current
  file-byte content CID, published CID when available, availability receipt, and
  object revision.
- Footer/status text for item count, selection count, and active operation
  progress where PC2 shows equivalent feedback.

Initial context menu actions:

- Open
- Open in New Window, only for folders/directories
- Open With -> viewer rows, when external viewers are available
- Download
- Download as ZIP, only for downloadable principal-root folders
- Download Selected, only for multi-select downloadable principal-root objects
- Download Selected as ZIP, only for multi-select downloadable principal-root objects
- Compress to ZIP, only for provider-compressible principal-root objects
- Compress Selected to ZIP, only for same-folder provider-compressible selections
- Extract Here, only for `.tar`/`.tar.gz`/`.tgz`/`.zip` archives
- Publish / Unpublish
- Share
- Copy content CID, for byte-bearing files
- Copy published link, only when published
- Cut / Copy / Paste Into Folder
- Delete
- Restore, only inside Trash
- Delete Permanently, only inside Trash and behind confirmation
- Rename
- Properties

Initial folder/background actions:

- Sort By -> Name / Date Modified / Type / Size / Ascending / Descending
- View -> Icons / Details
- Refresh
- Show Hidden
- New -> Folder / Text Document
- Paste, only after a valid Runtime clipboard move/copy exists
- Upload Here
- Properties

Unsupported PC2 actions must be hidden or shown with a clear not-yet-available
state only when useful. Do not ship dead menu items.

## Intentional Divergence From PC2

Do not port these PC2/Puter-era behaviors:

- Wallet-address roots as filesystem truth.
- `/null` path fallbacks.
- Username path aliases as authority.
- Bearer-token file shortcuts as app authority.
- Direct app calls to Kubo, IPFS Cluster, Elacity APIs, host paths, or broad
  `localhost://Users/*`.
- Socket/global-state assumptions that bypass Runtime capability checks.
- App-visible IPFS pinning, node credentials, wallet RPC, chain RPC, or network
  HTTP authority.

The Explorer is an app capsule. Dangerous authority stays in providers. The
current split is `library` for UI and `object-provider` for principal-root
object authority.

## Runtime Contract

Add a typed Library/Object provider contract before UI replacement. The app
calls this contract only; it never reaches raw storage or content backends.

## SmartWeb File Storage And CID Model

Local mutable storage is owned by `object-provider`. Files, folders,
Desktop/Documents/Public, Trash, revisions, protected principal-root storage,
and object events are ordinary mutable Library object state mediated by Runtime.
They are not exposed to apps as host filesystem paths, Kubo handles, Carrier
tickets, or raw provider SDKs.

CID is content identity, not a storage-location guarantee. Every readable local
file gets a current immutable raw-byte `content_cid` for the bytes at that
mutable object head. That proves what the current bytes are; it does not mean
the file has been published, replicated, or made globally fetchable.

Private files are SmartWeb object heads: a mutable `localhost://...` object URI
with provider-owned metadata, revision, capabilities, and current
`content_cid`. Published files are separate content-provider records: only after
publish does the object receive a `published_cid`, a public `elastos://...`
link, and content-provider availability/repair/replication receipts.

`Public` is a Library placement/projection root, not a second publish pipeline.
Putting an object under `Public` makes the user's intent visible in Library, but
it does not silently create a public network link. Public network access is
controlled by the explicit `publish` action and the resulting content-provider
receipt. Published objects do not automatically appear in `Public`; they appear
there only if the user also places or copies the object into that root.

The practical split is:

- `object-provider`: local mutable file/object storage and UI lifecycle
  authority.
- `content-provider`: immutable published content identity, public CIDs,
  availability receipts, and Carrier-backed delivery policy.
- `webspace-provider`: mounted resolver Spaces and provider-owned cache/sync/fork
  heads.

Minimum object record:

```json
{
  "schema": "elastos.library.object/v1",
  "uri": "localhost://Users/<principal-root>/Documents/example.txt",
  "name": "example.txt",
  "kind": "file",
  "mime": "text/plain",
  "size": 1234,
  "created_at": 1770000000,
  "modified_at": 1770000000,
  "revision": "rev:...",
  "viewer": "text-viewer",
  "thumbnail_uri": null,
  "availability": "local-only",
  "content_cid": "bafkrei...",
  "published_cid": null,
  "published": false,
  "shared": false,
  "capabilities": ["open", "rename", "download", "publish", "trash"]
}
```

Minimum provider operations:

- `list`
- `stat`
- `read`
- `stream`
- `download`
- `write` / `upload`
- `mkdir`
- `rename`
- `move`
- `copy`
- `trash`
- `restore`
- `delete_permanently`
- `publish`
- `unpublish`
- `share`
- `status`
- `repair`
- `events`

Every mutating operation must accept and return a revision or receipt so stale
UI state cannot silently overwrite newer state.

Large reads and media opens must use range/stream semantics through the provider
contract. The app must not synthesize host URLs or bypass the Runtime stream
path to make media playback work.

## Object Roots

Initial places:

- Home
- Documents
- Pictures
- Videos
- Downloads
- Public
- Spaces

The root labels can match PC2's familiar model, but the backing URIs must be
principal-rooted or WebSpace-mounted Runtime objects.

## Provider Mapping

- Explorer/Library UI: the `library` app capsule. It owns layout,
  interactions, viewer handoff, and PC2-familiar UX only.
- Local mutable object graph: canonical `object-provider` package in this
  branch, exposed through the canonical `object` Runtime provider scheme. It
  owns folders, files, Desktop/Documents, revisions, Trash, encrypted
  principal-root storage, and object events.
- Published content: Carrier-backed `content-provider`. It owns immutable
  content identity, CIDs, publish/fetch, status, repair, replication, and
  availability receipts. It must not own Explorer UI, folder names, Trash,
  local rename/move semantics, or app-visible Kubo/IPFS authority.
- Provider coordination: publish/unpublish/repair/status use
  `elastos://content/*` through Runtime/provider mediation. The first
  provider-to-provider request envelope exists for internal provider effects,
  with explicit local and Carrier provider-plane transports. Carrier
  `provider_invoke` stays Runtime-mediated and service-provider-only; the
  current `ProviderTransfer::Stream` envelope is bounded and validated, with a
  Runtime-owned stream-session read/cancel contract above it.
- Availability and replication: `availability-provider`.
- Mounted spaces: `webspace-provider`. It should return typed mount handles,
  backing target metadata, provider-owned object heads, metadata health and
  refresh/cache/sync/fork status, read-only/mutable capability flags, and availability
  hints without exposing raw provider credentials, cloud APIs, Kubo/IPFS, or
  Carrier transport authority to Library. Current mutable mounts can also
  materialize local WebSpace objects in a provider-owned object table; remote
  mutable sync still belongs to resolver/sync workers.
- Viewer selection: Runtime viewer registry and capsule manifests.
- Sharing policy: Runtime capability/share records, not a public path shortcut.

## Security And Audit Requirements

- Two principals must not read each other's roots.
- Path traversal, foreign roots, raw host paths, and broad
  `localhost://Users/*` must fail closed.
- Protected principal-root writes must use the protected storage helper or fail.
- Publish/share/delete-permanent must produce signed audit records.
- Local upload, mkdir, rename, move, trash, restore, and download must produce
  auditable object-operation receipts.
- App tokens must be scoped to the Explorer capsule and current principal.
- No operation may require app-visible Kubo/IPFS/Elacity credentials.

## Realtime Rule

Do not implement PC2-style global socket assumptions directly. The first slice
may refresh after local actions. The durable path is a typed Runtime event stream
for object changes, upload/download progress, publish status, availability
repair, and share changes. No aggressive polling.

## Implementation Slices

1. Provider contract and tests.
2. Explorer UI shell using the typed provider.
3. Upload/download/new-folder/rename/trash/properties.
4. Publish/unpublish/share/status plus provider-level repair support.
5. Viewer routing.
6. Spaces root visibility plus read-only/mutable policy.
7. Event stream and progress receipts.
8. Provider-to-provider content coordination for publish/unpublish/repair.

Each slice must be releasable and must not expose a dead visible control.

## Verification

Provider tests:

- list/stat/read a file in the current principal root
- reject foreign principal roots
- reject traversal and raw host paths
- write/upload through protected storage
- stream/download with HTTP range semantics through the Runtime route, and
  provider-streaming through the bounded provider chunk envelope
- rename with revision precondition
- move to Trash and restore
- delete permanently only from Trash
- publish returns a content receipt
- availability status returns a provider receipt
- app cannot call content/IPFS/host paths directly

UI/operator smoke:

- automated context-menu and core journey smoke:
  `node scripts/library-menu-smoke.mjs`
- signed live publish/share smoke when a Home token or browser cookie is
  available: `scripts/library-live-smoke.sh`
- open Explorer from Home
- browse each initial place
- upload one file by button
- upload one file by drag/drop
- create a folder from the toolbar or background context menu
- create a folder in Library -> Desktop and confirm Home desktop shows it as a
  desktop object without direct filesystem access from Home
- switch grid/list and sort by name, modified, size, and type
- rename inline with Enter, Escape, and blur paths
- open a file through viewer routing and verify the Home frame receives the
  target plus object URI
- download the file
- move it to Trash, restore it, then delete permanently with confirmation
- publish it and copy the CID
- share it and verify the share state changes
- verify another principal cannot read it unless shared
- verify no network-tab polling hammer while idle

Design parity smoke:

- compare the Runtime Explorer against the PC2 reference checklist above
- capture at least one desktop-size and one narrow-window screenshot or trace
  for the implemented slice
- if a PC2 behavior is missing, mark it unsupported with a reason or implement it
- if a PC2 behavior is intentionally changed, document the authority or platform
  reason in this file before shipping the slice
