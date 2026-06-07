# WCI Executive Weekly Report

Week ending: 2026-06-05

Branch: `review/library-release`

## Runtime Update

This week’s Library/Explorer work moved the previous PC2-aligned file-manager
slice much further into the ElastOS Runtime / WCI model. The important outcome
is still not a direct PC2 transplant; it is PC2-familiar Explorer UX with
Runtime-owned principal authority, provider-owned object/content state,
Carrier-mediated off-box effects, and no app-visible raw Kubo/IPFS, host path,
cloud credential, wallet, chain, or peer authority.

Release readiness is close but not finished. The code and alignment gates are
green, live smokes have passed, and the implementation is coherent, but the
worktree still contains a broad uncommitted release diff. Final release still
needs coherent commit slicing, a normal Chrome-profile human pass, coordinated
version selection, and publish with an operator release key.

Progress:

- Runtime Library now behaves much closer to PC2 Explorer while staying on
  ElastOS rails: places/sidebar, breadcrumbs, grid/list views, sorting,
  inline create/rename, double-click open, context menus, upload/download,
  drag/drop move/copy, Desktop projection, Trash/restore/delete,
  publish/share/status/repair, properties, preview/open, viewer handoff, SSE
  refresh, and browser Back/Forward takeover are all provider-mediated.
- The PC2 baseline was checked directly from `Elacity/pc2.net` `main` at
  `a0a910158bd67666a6d3ea2a775ce09005ba7ae7`; key files inspected included
  `UIItem.js`, `UIDesktop.js`, `open_item.js`,
  `refresh_item_container.js`, and the PC2 filesystem/file APIs.
- Runtime intentionally does not copy PC2’s authority model. PC2 shortcuts such
  as username/wallet path rewriting, `/null` path fallbacks, signed file URL
  shortcuts, broad global socket/session assumptions, direct GUI filesystem
  access, and app-visible IPFS/gateway paths remain rejected.
- Library is now an app capsule that owns UI only. Runtime injects the signed
  principal. Mutable files/folders/Desktop/Documents/Public/Trash are mediated
  by canonical `object-provider` on the `object` Runtime scheme; browser calls
  use `/api/provider/object/*`. Published content goes through
  `content-provider`; Kubo/IPFS stays behind the provider layer.
- Home Desktop now projects `localhost://Users/<principal>/Desktop` through
  `object-provider`. Home owns icon placement/opening only, and delegates
  Desktop object actions such as Properties and Download back to Library through
  signed object-action launches.
- Library `Open in New Window` now works for sidebar places and folders through
  signed `library -> library` Home open-target messages instead of weakening
  Home’s source-gated message policy.
- Documents can open and save a concrete Library object through the
  `/api/viewers/documents/library-object` viewer route. Library lists installed
  viewer capsules only, including Documents for text/markdown/PDF and GBA
  Emulator for `.gba`, `.gb`, and `.gbc` objects.
- Browser-native `prompt`, `alert`, and `confirm` are no longer used for
  Library object actions. Share/create/rename flows are in-app and
  provider-backed.
- PC2-style menu behavior has been improved: sidebar blank space suppresses the
  browser menu, sidebar place items expose `Open` and `Open in New Window`,
  item menus include working nested groups such as `Open With`, `Sort By`,
  `View`, and `New`, and dead visible controls are hidden.
- The double-click/rename bug was closed. Selected names now open on
  double-click in grid and list views; rename is explicit through context menu
  or F2; active rename editors do not also open the object.
- Keyboard parity improved: Shift-click range selection, Enter-open for
  selected objects, and Shift-F10/ContextMenu-key selected-object menus are
  covered in list view.
- Published/blocked/trash badges were moved into stable Explorer layout
  positions instead of awkward icon overlays. Published state appears under the
  filename in icon view and in the name column in list view.
- Empty folder states are centered and clearer. Spaces empty state explains
  that mounted provider-backed spaces appear only when a resolver is available.
- Library was split from a monolithic static page into active modules:
  `app.js`, `actions.js`, `api.js`, `dialog.js`, `editor.js`, `events.js`,
  `menu.js`, `model.js`, `navigation.js`, `preview.js`, `realtime.js`,
  `render.js`, `selection.js`, `state.js`, and `uploads.js`.
- Performance was improved beyond the prior week: async SVG hydration churn was
  removed, folder listings are cached, root folders are prefetched, visible and
  sorted objects are cached, large folders use keyed DOM node reuse, first paint
  is chunked, upload progress rendering is frame-coalesced, and object
  downloads now return chunked HTTP body streams with explicit
  backpressure/cancel transfer receipts.
- Library Properties, availability status, and share dialogs now lead with
  SmartWeb object identity and provider-owned availability summaries instead of
  raw backend diagnostics. Raw CIDs remain available as technical/copyable
  details.
- Archive support advanced from basic download into provider-owned object
  workflows: folder and selected-object ZIP downloads, `Compress to ZIP`,
  `Compress Selected to ZIP`, and safe `.tar`, `.tar.gz`, `.tgz`, and `.zip`
  extraction now exist with unsafe-entry rejection.
- Public-link and recipient-scoped share flows exist in Library. The provider
  records typed share policy metadata, `elastos.library.share-grant/v1`
  records, `shared_access` decisions, denied-access audit, shared-open
  receipts, and fail-closed key-release policy receipts.
- Legacy plaintext objects left in a protected principal root are auto-protected
  by `object-provider` on first access, preserving old development data
  without weakening the protected-root rule.
- Spaces is now the user-facing name for mounted WebSpace views. The Runtime
  handle remains `localhost://WebSpaces/<mount>/...`; resolver-private targets
  such as `google://drive/...` stay behind provider authority.
- `webspace-provider` now has persistent mount, resolver-index, object-head,
  and local materialized object tables. It also exposes provider-owned
  health, refresh, cache, sync, fork, write, mkdir, and delete receipts.
- Library now treats Spaces as mounted resolver views, not ordinary
  principal-root folders. Read-only mounts hide mutable actions; mutable
  mounts/forks can materialize provider-owned local objects only when the
  WebSpace provider marks the handle writable.
- `elastos webspace` CLI support now covers the current lifecycle surface:
  mounts, mount, unmount, index, health, refresh, head, cache, cache-status,
  sync, sync-status, and fork.
- `content-provider` now owns the content publish/fetch/status/repair decision
  above Kubo/IPFS. Apps still use one `elastos://content/*` surface whether
  bytes are local, cached, or fetched through availability providers.
- Built-in Carrier availability now treats `carrier_announced` as an auditable
  state. It signs and announces published CIDs on deterministic Carrier topics
  without exposing peer handles, connect tickets, or raw Kubo/IPFS authority to
  apps.
- Availability receipts now carry explicit peer-selection, quota,
  repair-worker, local accounting, and abuse-control metadata. Local-only
  publishes honestly report `single_local` with no live multi-peer proof.
- Carrier availability now scores signed candidate announcements using local
  metadata plus durable local runtime success/failure reputation. Remote
  receipts expose redacted score/reason/local-reputation rows, not raw transport
  authority.
- The first autonomous cross-peer repair/replication proof path exists:
  Carrier can select a remote provider, invoke and verify signed remote
  `content/admission` before moving bytes, invoke remote `content/ensure`,
  verify the same CID with remote `content/status`, and record
  `network_available` only when an independent remote provider proves a live
  pinned replica.
- Remote repair now has fail-closed fallbacks: `content/import_object` can
  reconstruct manifest-backed objects, and `content/import_exact` can push
  file-like exact-CID bytes when remote pin cannot fetch the object.
- The live-proof candidate selection bug was fixed. When live multi-peer proof
  is required and quota permits it, Carrier keeps at least one remote provider
  invocation in the candidate budget even if local replicas already satisfy the
  minimum count.
- `content-provider` persists an `elastos.content.repair-task/v1` ledger.
  Publish/ensure/repair/unpublish record local-only, queued, healthy, and
  retired task states; status includes the latest task.
- A Runtime-provider-only repair worker exists. Operators can trigger it with
  `elastos content repair-worker`, and servers can enable an opt-in bounded
  scheduler with `ELASTOS_CONTENT_REPAIR_SCHEDULER=true`.
- Provider status and repair-worker runs now expose
  `elastos.content.repair-fleet/v1` receipts for the current single-runtime
  repair fleet: `content-provider` is the coordinator/local worker, scheduling
  is ledger-based, task pressure is inspectable, and external repair fleets,
  storage-market admission, and settlement are explicitly not configured.
- Provider status, per-CID status, and repair-worker runs now also expose
  `elastos.content.network-abuse-policy/v1` receipts. These tie signed
  abuse-control receipts to one provider-owned policy surface: Runtime
  invocation is required, Carrier candidate caps and remote admission preflight
  are local guardrails, repair-worker attempt/failure budgets are visible, and a
  configured federated abuse-control endpoint quorum can enforce signed external
  admission policy before bytes or repair data move. Production network-wide
  throttles, banlists, and abuse ledgers remain explicitly not configured.
- Provider-wide status now also exposes
  `elastos.content.operator-dashboard/v1`, a derived operator view over signed
  receipts, repair-task history, and storage-accounting ledgers. It reports
  storage pressure, top principals by active content bytes, replica-byte
  estimates, quota-exceeded records, fleet-history attempts, recent repair rows,
  live-proof counts, and explicit non-production federation posture.
- Carrier peer selection and content status now expose
  `elastos.carrier.peer-reputation/v1` policy metadata. The current policy is
  honest local Runtime success/failure history only; status aggregates
  local-history-applied/not-reported/federated-policy counts and still reports
  signed cross-runtime reputation as not configured.
- Carrier peer selection, redacted remote receipt summaries, content proof
  summaries, and the operator dashboard now expose
  `elastos.carrier.peer-attestation-exchange-policy/v1`. The current policy
  records signed availability announcements, verified remote content receipts,
  remote provider proofs, and local Runtime reputation as present. The branch
  can also call one configured Carrier peer-attestation endpoint quorum with a
  signed `elastos.carrier.peer-attestation.exchange-request/v1`, verifies
  returned signed `elastos.carrier.peer-attestation.exchange-receipt/v1`
  receipts, and records endpoint receipts plus quorum counters before marking
  exchange accepted. Third-party attestations, revocation, and production
  fleet-wide reputation policy remain explicitly not configured.
- Local, Carrier, ledger, per-CID, and provider-wide storage-market status now
  expose `elastos.content.storage-settlement-policy/v1`. Pricing, escrow,
  payment settlement, SLA enforcement, storage-market admission, and
  cross-provider escrow are explicit `not_configured` policy status instead of
  vague missing product behavior.
- Local, Carrier, ledger, per-CID, provider-wide, and operator-dashboard
  storage-market surfaces now expose
  `elastos.content.storage-market-admission-policy/v1`. Local quota admission
  and signed remote `content/admission` preflight are recorded as current
  proof-path admission; configured storage-market endpoint-quorum admission can
  now enforce one operator-owned admission endpoint or a bounded endpoint set
  with explicit quorum, while production offer receipts, price discovery, SLA
  admission, and economic abuse controls are explicitly not configured.
- Local quota receipts, principal storage-quota receipts, Carrier quota
  receipts, remote receipt summaries, per-CID status, provider-wide status, and
  the operator dashboard now expose
  `elastos.content.federated-quota-ledger-policy/v1`. The policy records local
  per-principal accounting, signed remote `content/admission` preflight, and
  configured signed federated quota-ledger endpoint-quorum exchange as present
  when configured. Production independent provider-network quota-ledger
  federation and production storage-admission networks remain explicitly not
  configured.
- `content/admission` can now enforce an operator-configured federated
  abuse-control exchange before quota-ledger, storage-market, byte-transfer, or
  repair-graph movement. The request is signed as
  `elastos.content.federated-abuse-control.exchange-request/v1`; accepted
  responses must include a verified
  `elastos.content.federated-abuse-control.exchange-receipt/v1`, and rejection
  or endpoint failure rejects admission fail-closed without exposing endpoint
  credentials.
- Provider-wide status, repair-worker runs, and the operator dashboard now expose
  `elastos.content.external-repair-fleet-policy/v1`. The policy records the
  provider-owned local repair worker/scheduler as present, while external
  coordinators, volunteer/supernode workers, cross-provider repair queues,
  worker attestations, fleet settlement, and repair SLAs are explicitly not
  configured.
- Provider-wide status and the operator dashboard now expose
  `elastos.content.federated-operator-alerting-policy/v1`. The policy records
  provider-local status JSON, storage-pressure signals, repair-task pressure,
  live-proof counters, remote-receipt counters, and configured provider-local
  alert sink plus configured federated alert-exchange posture as present.
  Operators can request a durable `elastos.content.operator-alert.receipt/v1`,
  optional `elastos.content.operator-alert/v1` webhook delivery to one
  configured sink, and optional
  `elastos.content.federated-operator-alert.exchange-request/v1` delivery to
  one configured operator-owned exchange endpoint. Cross-provider dashboards,
  peer-health subscriptions, fleet-wide SLA policy, and operator UI remain
  explicitly not configured.
- Provider-to-provider Runtime invocation now has a typed
  `elastos.provider.invocation/v1` envelope. Target providers can audit
  Runtime-mediated source/target/op capability metadata.
- The provider plane now has explicit local and Carrier transports. Carrier
  `provider_invoke` is service-provider-only and rejects raw backend targets
  such as `ipfs` or `localhost`.
- Provider transfer receipts now carry range/progress metadata. Bounded
  byte-range slicing works for base64 provider payloads.
- `ProviderTransfer::Stream` now uses a validated
  `elastos.provider.stream/v1` base64-chunk envelope. `content-provider` fetch
  opens it as a Runtime-owned stream session for local IPFS and
  availability-provider fallback reads, with read-next backpressure, live
  progress events, and cancel support.
- Source providers cannot spoof Runtime-owned `_runtime_invocation` or
  `_runtime_transfer` fields; predeclared fields fail closed before reaching the
  target provider.
- The availability dashboard now summarizes signed receipt and repair-task
  ledgers, including quota verdicts, accounting counters, abuse controls,
  remote replica proof rows, verified remote receipt counters, and explicit
  truncation metadata for capped rows.
- `content-provider` now exposes provider-only
  signed `elastos.content.admission/v1` preflight receipts. Carrier records
  accepted admission in peer-selection metadata and stops before
  `content/ensure`, exact/object import, or block-graph import when a remote
  peer omits/forges the receipt or rejects projected quota.
- Spaces/WebSpace now has a persistent resolver adapter registry and operator
  CLI. Health reports adapter registration/connection state and redacts
  endpoint credentials, which makes external resolver readiness visible without
  claiming live byte traversal.
- Spaces/WebSpace adapter health now has a safe liveness receipt path:
  `check_adapter` records redacted `ok`/`failed`/`skipped`/`unknown` checks,
  stale-check metadata, checked-adapter counts, and CLI support without
  exposing credentials or claiming external byte sync.
- Spaces/WebSpace now also has a tested adapter invocation/cache contract:
  Runtime invokes resolver adapters through provider-to-provider
  `metadata_index` and `read_bytes`, and `webspace-provider` can materialize
  adapter bytes into a clean, non-dirty cache head. This is a fake-adapter
  proof of the contract, not a claim that external operator endpoints are
  production-ready.
- Spaces/WebSpace now has a second operator-style provider fixture beyond the
  fake Google adapter. It proves provider-selected adapter reads, durable
  cached status after materialization, cached second reads, and viewer handoff
  for adapter-backed markdown content through the installed Documents viewer.
- Library/Runtime now exposes a Spaces-only `sync` operation that invokes the
  resolver adapter, persists bytes into the provider-owned WebSpace cache, and
  returns a byte-sync receipt without returning file bytes to the caller. A
  later read uses the cached bytes.
- Share receipts and Library properties now expose a remote-access policy
  summary. Recipient-scoped sharing is provider-gated by `shared_access` and
  Runtime recipient proof. Library also exposes `Check My Access` for shared
  published objects, which asks `object-provider` for the signed principal's
  shared-access receipt and renders the access decision, open contract, and
  key-release posture. The branch now includes a non-production
  `protected_content_fixture` path that publishes a sealed-object descriptor,
  records recipient-scoped key-release grants, invokes DRM/rights/key/decrypt
  fixture providers, returns a viewer-scoped protected-open contract, and fails
  closed when those providers are absent.
- DRM/rights/key/decrypt provider capsules now advertise explicit protected-content
  contracts. DRM is protected-content open orchestration, rights is typed
  ACL/rights decisions only, key is typed key-release receipts without raw CEK
  exposure, and decrypt is viewer-scoped decrypt/render sessions without broad
  plaintext or filesystem authority. Key release now requires an allowed
  rights-decision receipt bound to the same principal/session/object/action, and
  decrypt/render now requires a typed key-provider release receipt bound to that
  same request context.
  Object-provider share/open receipts now reference those provider
  requirements so protected recipient sharing has a clear receipt chain.
- Object-provider status/share/shared-access responses now expose
  protected-content provider-chain readiness. Library status/share dialogs show
  that readiness. Production encrypted-recipient payload generation, live dDRM
  policy reads, and production dKMS remain separate Trusted content follow-ups.
- Provider transfer receipts now include an explicit transfer ABI. Stream mode
  advertises Runtime stream-session semantics: read-next backpressure, live
  progress events, and cancel support above the validated JSON/base64 chunk
  envelope.
- Carrier availability now reports storage-market policy separately from live
  multi-peer proof. Current replication is receipt/proof based and explicitly
  marks `settlement: not_configured`.
- Library Properties now includes an archive support matrix for implemented
  ZIP/tar/tar.gz download/extract behavior and remaining generic archive
  policy/dependency gaps.
- Generic archive families such as `.7z`, `.rar`, `.tar.xz`, `.tar.bz2`,
  `.tar.zst`, `.xz`, `.bz2`, `.zst`, `.lz4`, and plain `.gz` are now
  recognized as policy-gated archives in object metadata and Library
  properties. Policy-gated archives also expose an `Archive Support` context
  action so users can see the dependency/release-policy reason instead of a
  missing action, while extraction remains disabled.
- Release and planning docs were updated to keep the ontology explicit:
  `object-provider` is the current mutable principal-root object provider,
  `content-provider` owns published content, and Kubo/IPFS is only a low-level
  backend.
- The changelog now describes the Library, Spaces/WebSpace, content
  availability, provider invocation/streaming, Home Desktop, Documents handoff,
  archive support, sharing foundation, and known non-goals without claiming full
  WebSpace federation, remote ACL/key-release, dDRM, or production storage
  markets.
- Live Library publish/share was restored and smoke-tested through a signed
  Home session. Public Home/Library routes return 200, public Library live
  smoke passes, signed Home live smoke passed, and signed Library live smoke
  passed through roots, Public write/upload, archive extraction, publish,
  status, share, and cleanup.
- Current verification gates are green after the object-provider no-fallback
  migration: `home-entropy-check`, WCI alignment, `git diff --check`,
  object-provider/Library gateway tests, content tests, Carrier tests, content
  command/scheduler/status tests, `cargo check`, `cargo clippy --tests -D
  warnings`, and the hard stale-marker sweep for retired object-provider
  fallback strings have all passed on the touched release surface.
- A follow-up bounded-product-slice gate also passed for WebSpace adapters,
  adapter byte-cache materialization, stream ABI receipts,
  Carrier storage-market metadata, recipient-scoped share receipts, Library
  dialog syntax, `cargo check`, `cargo clippy`, entropy, WCI alignment, and
  `git diff --check`.
- A protected-content/archive-policy gate also passed for drm/rights/key/decrypt
  provider contract tests, recipient-scoped share receipt requirements,
  generic archive policy-gate metadata, Library dialog/model syntax,
  `cargo check`, and `cargo clippy`.
- A protected-content provider-readiness UX gate also passed for recipient
  share/status readiness receipts, Library dialog syntax, entropy, WCI
  alignment, whitespace, `cargo check`, and `cargo clippy`.
- A WebSpace mutable resolver-sync gate also passed: operator-backed mutable
  WebSpace writes now sync back through adapter `write_bytes`, local mutable
  spaces fail closed when no resolver adapter exists, and resolver conflicts
  return explicit conflict receipts instead of pretending to sync. The gate
  covered focused sync tests, the full Library gateway suite, `cargo check`,
  entropy, and WCI alignment.
- A WebSpace resolver availability-hint gate also passed: adapter-cached and
  adapter-synced WebSpace objects now expose resolver-scope availability hints
  in object metadata and sync receipts. The hints are deliberately labeled as
  not SmartWeb content availability receipts, so they improve UI/operator
  clarity without claiming CID replication.
- An installed operator adapter gate also passed: `operator-drive-adapter` is
  now a real provider capsule with manifest/build/release metadata, Runtime
  startup registration, Runtime-only provider invocation, deterministic
  provider-owned local byte storage, read-only/conflict policy, and hidden
  credential-field rejection. This promotes the fixture contract into a shippable
  adapter package, while still not claiming external operator federation.
- An operator endpoint-backend gate also passed: `operator-drive-adapter` can be
  configured through Runtime operator config with an operator-private loopback
  HTTP endpoint, traverse metadata, read bytes, and write mutable forks through
  that endpoint. App-visible status/receipts redact endpoint URL and
  authorization, and the adapter does not forward the Runtime invocation
  envelope to the backend.
- A production-style operator endpoint proof also passed: the adapter can talk
  to a filesystem-backed operator endpoint that traverses real temp-dir
  metadata, reads bytes, writes mutable forks, and returns no endpoint
  credentials, host paths, or Runtime invocation envelope to app-visible
  receipts.

Remaining gaps:

- The branch is not release-clean yet. The worktree still contains a broad
  uncommitted Library/WebSpace/content-provider release diff that must be split
  into coherent, reviewable commits before tag/release.
- Final normal Chrome-profile testing is still required. The remaining human
  gate is perceived speed/native feel, no stale browser-profile cache behavior,
  and no visible Explorer regressions on
  `https://elastos.elacitylabs.com/apps/home/`.
- Production protected-content backends are not complete. The branch proves the
  Library/Runtime recipient-proof, rights, key-release, decrypt-session, and
  viewer-handoff receipt chain with a non-production fixture. Real encrypted
  payload production, approved dDRM rights backends, production dKMS/key
  release, and production decrypt/render backends remain deferred.
- Provider streaming is complete for this Library branch. Remaining transport
  work should be scoped only if it changes behavior beyond the tested Runtime
  stream-session and chunked object-download contracts.
- Production multi-peer storage markets are not complete, but durable
  accounting slice landed. Signed content availability receipts now project
  into a durable per-principal storage-accounting ledger, `content/status`
  exposes active/tracked objects, bytes, replica-byte estimates, quota posture,
  and no-settlement storage-market metadata, unpublish preserves the original
  publisher principal for retired accounting, and publish/import can enforce
  `max_storage_bytes_per_principal` before local content-backend writes.
  Carrier now also records bounded repair-graph policy and refuses to flatten
  requested arbitrary IPLD DAG repair into exact-byte fallback. The
  `content-block-graph-provider` path is now present: Runtime reserves
  `elastos://block-graph/*`, Carrier exports a local graph through
  `export_graph`, imports it on the remote peer through `import_graph`, and
  uses the `ipfs-provider` Kubo coordination file to move bounded DAG CAR bytes
  without exposing raw Kubo authority to apps. `elastos content status` gives
  operators the provider-wide or per-CID availability/storage status through Runtime provider
  invocation. Bounded remote content admission preflight is now also present:
  Carrier asks the remote content provider for a signed admission receipt before
  moving bytes or graph repair data. `content/admission` can now also enforce an
  operator-configured storage-market admission endpoint or bounded endpoint
  quorum, with accepted or rejected market decisions normalized into the signed
  admission receipt and endpoint/quorum failure rejecting admission fail-closed.
  Repair-fleet status receipts
  now make the current single-runtime coordinator/worker policy explicit, and
  the Runtime-gated repair worker can dispatch due tasks to a configured
  external repair-fleet endpoint quorum with normalized dispatch receipts while
  local provider verification still decides final availability.
  Network-abuse policy/status receipts make local guardrails and configured
  abuse-control endpoint-quorum exchange posture explicit without pretending
  production network-wide throttles or banlists exist. Provider-local operator
  dashboard depth now exposes storage pressure and fleet history from the
  existing ledgers.
  Availability/storage policy and status coverage is now complete for this
  branch. Remaining work is production execution: production independent provider-network
  quota-ledger federation beyond the configured bounded endpoint quorum,
  repair-fleet worker attestation/SLA/settlement beyond configured dispatch
  quorum, live market
  pricing/escrow/settlement beyond the configured admission gate, actual
  federated network throttles/banlists/abuse ledgers beyond the configured
  bounded abuse-control endpoint quorum, production peer
  reputation trust policy/third-party attestations/revocation beyond the
  configured Carrier peer-attestation endpoint quorum, durable remote
  peer-selection policy, cross-peer repair policy, and live federated
  dashboard/UI/peer-health
  subscriptions beyond the current provider-local dashboard plus configured
  alert-exchange endpoint.
- Object-provider capsule migration is now in place for package/manifest/profile
  selection, Runtime startup, and browser routing. The branch uses the canonical
  `object-provider` package with the `object` Runtime scheme and
  `/api/provider/object/*` route. The pure object-provider core still lives in
  `elastos-server::library`; extracting that into a smaller core crate is
  architecture/build-review cleanup, not a current Library behavior blocker.
- Archive support is good for this release slice, but richer generic archive
  UX, generic non-tar/non-zip archive extraction, archive-manager/import flows,
  and WebSpace selection archives remain deferred until resolver/archive policy
  is clearer. Unsupported archive families are now recognized and labeled as
  policy-gated rather than enabled.
- The next PC2 slices should stay out of this release until Library is clean:
  AI Chat, dDRM, Elacity Marketplace, Mac VZ, and broader Browser provider work.

Bottom line: Library is now substantially more complete than last week. It is
PC2-familiar for users, but the backend model is ElastOS-native: Runtime
principals and provider mediation own authority, `object-provider` owns mutable
local objects, `content-provider` owns published content and
availability, Carrier coordinates remote proof/repair, and Kubo/IPFS remains an
internal backend. The remaining work is mostly release hygiene, final human
testing, and keeping product deferrals mapped to plain product areas:
production multi-peer/storage-market infrastructure, production content-rights
backends, and format-specific archive dependency approvals.
