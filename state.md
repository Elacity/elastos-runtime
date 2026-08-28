# State

Last updated: 2026-08-28 UTC

This file records public-safe current truth for released 0.6.0, published review
lines, and active unpublished work. Historical
local proof logs, private SSH aliases, tunnel ports, operator usernames, key
paths, and target backup paths are intentionally not tracked in the public
repository.

## Release Posture

- `main` at `d358dedb` is the released 0.6.0 line.
- The published collaboration review stack is
  `origin/review/collaboration-foundation` ->
  `origin/review/collaboration-product-integration` ->
  `origin/review/collaboration-candidate`. Each review branch depends on its
  parent; installed and live claims remain separate target evidence, not branch
  truth.
- Bottom-up review of the stacked chain is COMPLETE (2026-08-28): all ten
  checkpoints (#27-#36) reviewed, fixed on their branches, CI-green on the
  shared upstream, and integrated on the local `temp/0.7-merge` line together
  with the base reconciliation (the `docs/elastos-system-map` merge and the
  browser-local-exit orphan-reaping hotfix). The provisional-retirement guard
  stayed green at every checkpoint. Merge of the chain into `main` remains the
  chain owner's operation.
- `codex/post-0.6-consolidation` remains an unpublished local integration line
  for additional collaboration and UI work. It has no upstream and is not
  installed or published product truth.
- `feat/protected-content-contracts` is a published source-only contract
  branch, stacked for review on
  `origin/review/collaboration-product-integration`. It adds the
  canonical `elastos-protected-content-contracts` crate, the related
  documentation, and the shared strict DID/Carrier codec correction required by
  that contract surface. It does not integrate Runtime orchestration, provider
  replacement, custody, threshold reconstruction, recipient encryption proof,
  decryption, playback, installation, or deployment.
- `origin/feat/protected-content-custody` is the published source-only child
  branch
  stacked on `origin/feat/protected-content-contracts`. It adds the
  `elastos-protected-content-custody` crate plus custody-envelope
  provisioning and recipient-sealed node release. It also includes the typed
  EVM rights-policy and evidence contracts, Profile-signed recipient-key
  authorization, signed custody epochs, the Runtime-to-release-node operation
  envelope, and one owner-only node-local durable dual-key replay store. That
  store privately gates release, persists the exact encrypted node
  contribution, and replays only that result after restart.
- `origin/feat/protected-content-key-reconstruction` is the published
  source-only child branch stacked on
  `origin/feat/protected-content-custody`. It adds authenticated release
  reconstruction inside the decrypt boundary for new content. Its current
  reviewed behavior rejects invalid X25519 contract key bytes before HPKE use,
  requires exactly the bound released threshold, and checks a manifest-bound
  CEK commitment after reconstruction to detect a wrong reconstructed key. A
  durable local claim without a stored result fails closed and currently
  requires a fresh Runtime release operation; there is no operation-resume
  journal for that state.
- `origin/feat/protected-content-custody-provider` is the historical published
  source-only protected-content branch at `f7cd6c3d`. Its ancestry includes
  provider protocol, authenticated payload sealing, local decrypt-output,
  object-bound custody-pool policy, one-node provisioning authority, expected
  Runtime issuer pinning, owner-only durable node-share storage, and the
  unregistered `capsules/custody-provider` process. It proves exact
  object/pool/epoch/committee binding, one selected node and one sealed-share
  provisioning record, local-node validation, exact duplicate/conflict/restart
  behavior, signed-rights-gated release, exact encrypted contribution replay,
  bounded provider frames, redacted diagnostics, and clean shutdown. It exposes
  no CEK, raw share, topology, Carrier authority, route, host, IP address,
  port, path, or credential. It is not registered, installed, deployed, or
  product-current. The old provisional `key-provider` remains the only active
  registered product key/custody path until a later atomic Runtime cutover.
- `origin/feat/protected-content-wallet-rights` at `2c69d0c2` is the published
  source-only child branch that adds a dedicated Wallet operation for
  protected-content rights. It signs the exact canonical `RightsRequestV1`
  bytes for the selected active EVM account through the existing verified
  Wallet invocation context. It does not add Runtime, Library, provider
  registration, Carrier, or product UI behavior.
- `origin/feat/protected-content-runtime` at `b00bfeeb` is the published
  source-only child branch that adds private durable Runtime release state
  and typed internal coordination over the Wallet-rights, rights-provider, and
  custody-provider contract types. It persists before provider effects, records
  effect-started state, treats ambiguous post-dispatch outcomes as durable
  nonterminal state, replays only exact stored terminal results, and prevents
  caller-selected provider or topology input. It is not wired into
  `elastos-server` product routes, Library, viewer output, or the installed
  provisional path.
- `origin/feat/protected-content-rights` at `43a83e5b` is the published
  source-only child branch that adds typed chain-rights evidence and the
  typed rights evaluator. Evidence acquisition is bound to the exact Runtime
  release operation, verifies live chain id, uses an exact canonical block
  hash/number binding, binds contract/method selector, has bounded freshness,
  redacts upstream failures, and does not accept caller-supplied rights facts.
  It remains source-only and unregistered.
- The current `feat/protected-content-runtime-lifecycle` branch is stacked on
  published `origin/feat/protected-content-runtime-lifecycle` at `34465959`,
  which in turn is stacked on published
  `origin/feat/protected-content-rights` at `43a83e5b`. The inactive
  Runtime-owned mint -> availability -> creator mint/list -> buy -> open ->
  play -> close path is complete in source on the current branch:
  Runtime-owned mint durability, fresh pre-buy availability, verified creator
  mint/list binding, Runtime-owned buy with finalized access corroboration,
  durable viewer lifecycle, and the inactive combined mint -> buy -> open ->
  play -> close proof. Later closeout commits keep Base read-path truth and
  docs current without changing installed-product scope. The atomic cutover
  has not started.
- `CustodyEnvelopeV1` is current source-only inactive Runtime
  open/provisioning material stored owner-only at
  `protected-content/runtime-open/{mint}/envelope.bin`, not public asset
  metadata. It is separate from the identity-only mint journal and from public
  metadata, capsules cannot read it, and Runtime cannot open the node-sealed
  shares inside it. Each selected custody provider persists only its own raw
  share. Public metadata contains no shares; it contains bounded identities,
  threshold/epoch/pool facts, CEK commitment, and signatures only.
- Raw CEK and private-key JSON vectors in historical protected-content work are
  deterministic test fixtures only. Product operations, responses, logs, public
  metadata, and durable product state must not contain raw CEKs.
- PR #15 / `feat/dkms-esp-port` remains public, unmerged source evidence only:
  keep its threshold crypto, node-local custody, recipient-sealed
  contributions, CEK commitment, lifecycle scenarios, and fail-closed negative
  tests as research; reimplement per-node durable shard storage,
  DKG/rotation/re-share/revocation, pool/governance policy, provider roles, and
  Runtime-open scenarios at the canonical boundary; reject its public
  aggregated `shares[]` metadata, capsule-owned authority, raw CEK operations,
  `rail_shim`/reference fallbacks, old `drm-provider` orchestration, direct
  topology in capsules or contracts, static authorization fallbacks, and
  standalone harness as a product route. Its generated producer-smoke
  `escrow.json` is historical dev evidence only because it aggregates wrapped
  shares. The producer smoke writes and reloads `cek_commitment_b64`; the older
  Creator path carried the missing-commitment writer/reloader inconsistency.
- Current protected-content source proof is complete for the inactive
  Runtime-owned mint -> availability -> creator mint/list -> buy -> open ->
  play -> close path on `feat/protected-content-runtime-lifecycle`, but it is
  still not installed or product-complete.
  Runtime tests prove durable 2-of-3 custody provisioning, exact pre-buy
  signed availability rechecks, Runtime-owned creator mint/list settlement,
  immutable listing bind, Runtime-owned buy with finalized multi-source access
  corroboration, and durable viewer lifecycle/cleanup. The typed combined proof
  now drives the existing gateway Runtime seams with two distinct principals and
  accounts, one shared ProviderRegistry, real protect/custody/decrypt provider
  processes, typed Wallet approval/effect replay, creator mint -> listing ->
  buyer purchase -> finalized access corroboration -> open -> 2-of-3 release ->
  decrypt init/segment read -> exact close, with zero unresolved Runtime or
  provider state. The proof still uses deterministic test Wallet/Chain/content
  fixtures for non-installed authority surfaces, so it is source proof rather
  than installed/live product evidence. Lower-level Runtime lifecycle and
  decrypt-provider process tests still separately prove PQ-hybrid contribution
  reconstruction, exact CENC media reads, close replay, process restart, and
  old-handle absence. Separate Runtime restart/replay tests prove persisted
  terminal replay and retained nonterminal state after effect start. The
  decrypt provider generates each operation-scoped recipient key and retains
  its secret behind an opaque handle; Runtime receives only the public
  key/identity, and a Profile signature authorizes that exact key. No Profile
  seed enters Runtime, custody, or decrypt-provider contracts. The combined
  inactive proof and its supporting lower-layer tests cover wrong-recipient and
  wrong-object/media-binding rejection, exact durable release replay from the
  same Runtime journal, explicit provider unregister/absence cleanup, and zero
  unresolved release state. Restart/crash/cleanup remain owned by the
  lower-layer process and journal tests:
  `capsules/custody-provider/tests/process.rs::custody_provider_process_provisions_releases_replays_after_restart_and_shuts_down`,
  `capsules/protected-content-decrypt-provider/tests/process.rs::process_prepare_open_read_close_replay_and_restart_absence_flow`,
  `elastos/crates/elastos-protected-content-runtime/src/journal.rs::durable_state_replays_only_persisted_terminal_result`,
  `elastos/crates/elastos-protected-content-runtime/src/coordinator.rs::runtime_coordination_replays_terminal_without_dispatch`,
  `elastos/crates/elastos-protected-content-runtime/src/mint.rs::restart_after_effect_started_stays_nonterminal`,
  and `elastos/crates/elastos-protected-content-runtime/src/mint.rs::custody_provisioned_replays_without_redispatch`.
  Focused Profile-signing, Wallet binding, Chain evidence, and the integrated
  deterministic process path already cover the source seams they own. Current
  source still pins one local Runtime device issuer; multi-Runtime issuer
  admission is not yet present. The new protected-content path remains
  inactive, not installed, not cut over, and not product-ready.
- Released 0.6 and the published collaboration review stack retain the older
  provisional `elastos_common::protected_content` DTOs plus fail-closed
  `drm-provider`, `rights-provider`, `key-provider`, and `decrypt-provider`
  capsules. That old DRM/provider surface remains installed and source-visible
  only until an atomic Runtime cutover replaces it. It does not consume or
  prove the new v1 contract, and the new protected-content product path is not
  yet connected or usable. Installed-target truth requires separate target
  evidence.
- An independent branch-local source/contract review of the published
  `origin/feat/protected-content-contracts` branch completed with no code
  findings after the strict DID codec and Carrier codec consolidation. This is
  not an external cryptographic audit or production security approval.
- An independent AI/model review found the invalid-X25519 acceptance,
  released-threshold mismatch, and missing reconstructed-key commitment check
  now corrected on the custody branch. That review is useful source
  review evidence, not a professional external cryptographic audit.
- The collaboration review stack adds Runtime-backed People/Chat collaboration
  and selected shell UI work. The source boundary is complete for review:
  Profile authority, Runtime lifecycle, Carrier routing, People/Chat
  projections, and the strict fixture-owned two-Runtime acceptance all pass.
  Normal localhost and public seed installation remain separate product gates.
- The first normal cross-Runtime Chat send on the installed candidate aborted
  inside the old Iroh 0.96.1 `iroh-quinn` transport. The source candidate now
  uses one coordinated Carrier generation: Iroh 1.0.2, iroh-gossip 0.101.0,
  mDNS 0.4.0, and distributed-topic-tracker 0.3.5. Focused Carrier,
  collaboration, and two-node source tests pass. Localhost artifact parity and
  machine Browser open/connect/close/zero-residue proof now exist; public-seed
  retesting and manual Browser visible video/input usability remain open.
- The Runtime implements the WASM Component Model path through
  `elastos.component/v1` and the Runtime-mediated `elastos:bus@v1` authority
  contract. The conformance fixture and authoring template exercise it; all 18
  shipped first-party UI Apps still use `elastos.runtime-projection/v1` web
  projections. WASI Preview 1 is rejected at product capsule admission.
- Source/review proof must cite concrete reusable commands: `git diff --check`,
  `node scripts/home-entropy-check.mjs`, `node scripts/browser-entropy-check.mjs`,
  `bash scripts/check-wci-alignment.sh`, `just candidate-command-audit`, and the
  touched-surface Rust/capsule tests.
- Component ABI proof also includes
  `node scripts/check-first-party-wasi-gate.mjs`, which reports every
  first-party WASI Preview 1 capsule finding and fails on unclassified product
  WASI usage.
- ElastOS Bus v1 deliberately omits streams because no shared
  capability/audit/lifecycle implementation exists yet. Its checked-in WIT hash
  is `7a026e0a641c8c04214576dc85a677e0b52c9f02866d231119f9a3ba609d49e2`;
  the conformance fixture and Component authoring template are bound to that
  hash. No shipped first-party product Component proves adoption end to end.
- [docs/CAPSULE_AUTHORING.md](docs/CAPSULE_AUTHORING.md),
  [templates/capsules](templates/capsules), and `elastos init` are the canonical
  capsule authoring paths. Their manifests are validated by repository gates;
  generated product capsules have no WASI, ambient environment, preopen,
  socket, FIFO, or direct provider authority.
- People is a standalone first-party web-projection capsule under
  `capsules/people`. Home launches `/apps/people/` with a People-scoped token;
  People owns its UI and calls only its Runtime-mediated app routes.
- Release publishing validates every discovered capsule manifest and rejects
  missing descriptions, missing authors, and scaffold placeholder authors.
- Target proof is operator-supplied. It must name target roles and exact command
  lines without committing private host aliases, SSH keys, tunnel ports, local
  data roots, or other private operator paths.
- Private proof logs stay outside this repo. Public release notes should claim
  only proof that can be reproduced from the reviewed source or from
  explicitly supplied target evidence.

## Capsule Execution Truth

- [docs/CAPSULE_MODEL.md](docs/CAPSULE_MODEL.md#isolation-boundary)
  defines the cross-branch isolated-execution contract. It is a 0.6
  architecture requirement introduced by 0.6, not proof that every first-party
  app is already a Component.
- The ESP branch proves a useful substrate slice: the Component runner and
  conformance fixture use no linked WASI, environment, filesystem preopen,
  FIFO, raw socket, or gateway authority, and every guest effect is linked
  through `elastos:bus@v1`. This is contract proof, not first-party product-App
  adoption.
- The Component runner is bounded today, but not yet by each manifest's declared
  resources: it uses a fixed 128 MiB memory ceiling and fixed fuel budget.
  `component/v1` is a bounded activation contract and cannot cancel or stop an
  activation already running.
- Component identity context is not complete: principal may be launch-bound,
  but the current Bus host reports the capsule id as the session id and has no
  device binding. Principal, proof binding, device, capsule, launch grant, and
  session therefore are not yet independently proven end to end.
- Whole-bundle publisher verification, signed interface compatibility,
  complete WebSpace state portability, and cross-node historical
  re-instantiation remain open. A valid manifest or checked-in Component is not
  yet proof of an independently durable Digital Capsule.
- Browser and Home web surfaces remain host projections and adapters. Their
  routes, frames, cookies, and placement are not the capsule ABI or authority,
  and the repo must not claim that every visible first-party app is already a
  self-contained executable Component.
- Verified on `b07160cf` on 2026-08-16: the generic
  `POST /api/provider/:scheme/:op` route remains a live host adapter used by
  current web projections and control surfaces. It is not the Component ABI or
  a capsule contract, and new capsule code must not treat it as one.
- Runtime auth audit history is currently SHA-256-linked and Ed25519-signed with
  retained-chain anchoring. BLAKE3 is not the canonical audit hash in this
  branch; any future algorithm change requires an explicit schema version,
  algorithm identifier, golden vectors, and a signed transition anchor rather
  than rewriting retained history.

## Authority And Wallet Truth

- Home authority uses signed `elastos.home.launch-token/v4` envelopes. Runtime
  validation binds resource, actors, principal, proof, grant, session,
  lifetime, and non-delegatability; callers cannot supply Wallet authority.
- Wallet Bus v2.3 is the typed Runtime/Wallet Provider boundary. Wallet Provider
  owns keys, accounts, proofs, approval execution, and validated outcomes;
  Runtime owns launch authorization, orchestration, durable effects, and the
  private provider adapter.
- Passkey step-up is durable, one-shot, and bound to the original launch,
  operation, and canonical request digest. Managed recovery verifies the exact
  Wallet set and root reassignment before returning terminal success.
- Runtime creates durable transaction-effect state before dispatch and
  reconciles recorded or uncertain Chain outcomes without rebroadcasting.
- Browser account access is an explicit Wallet approval. The injected provider
  can request accounts, but the page receives no selected address until the
  Runtime-mediated request is approved through the trusted review path.

## Consequence-aware effect truth

- The manifest schema includes `AffordanceRisk::Actuator`, and Runtime maps it
  to the `execute` capability action. The generic catalog invocation path still
  rejects actuator, payment, rights, and privileged affordances because its
  explicit user-approval dispatch is not enabled.
- No shipped capsule manifest declares `actuator`. This branch has no general
  sensor-observation envelope, installed physical-actuator provider proof, or
  hard real-time safety claim.
- Wallet transactions have durable effect IDs and uncertain-outcome
  reconciliation. Browser launch has bounded `DidNotAct` reconciliation, and
  remote service contracts forbid blind retry after uncertain dispatch. These
  are provider-specific proofs, not a shipped universal effect state machine.
- [Consequence-aware effects](docs/CONSEQUENCE_AWARE_EFFECTS.md) defines the
  shared target contract. Physical effects still require an operation-specific
  provider, destination admission, local interlock proof, truthful settlement,
  and installed-target evidence before any readiness claim.

## Proof Path Ledger

- Installed operator/update proof path: `public-install-operator-smoke.sh`.
- DID/profile proof path: `public-install-identity-smoke.sh` or
  `local-identity-profile-smoke.sh`, depending on target role.
- Public Linux runtime portability proof path:
  `audit-linux-runtime-portability.sh`.
- Provisional protected-content provider retirement guard:
  `protected-content-provider-contract-smoke.sh`. It does not verify the
  canonical v1 custody or Runtime path.

## Browser Truth

- Browser is included in 0.6.0 as a bounded Runtime Browser, not as a fully
  reliable general-purpose Browser claim.
- On the installed collaboration candidate at localhost, accepted machine proof
  now covers Browser launch, TURN/media-relay connection, Runtime-mediated
  traffic, exact terminal close, and zero remaining ownership, stream, and
  reconciliation files for that page/session.
- That localhost machine proof does not yet prove human-visible decoded video,
  Browser text input, scrolling, or audio. Manual Browser usability remains
  open.
- Accepted localhost evidence covers the installed macOS VZ candidate's launch,
  decoded display, navigation through Runtime-only networking, and injected
  provider availability.
- Deterministic proof confirmed `window.ethereum`, one EIP-6963 provider,
  `isElastOS=true`, `isMetaMask=true`, the Runtime Wallet binding, chain `0x14`,
  and exactly one `eth_requestAccounts` handoff producing one pending Wallet
  account-access approval.
- One failed Browser restart followed by a successful open, lost `ela.city`
  login state across restart, and slow performance remain explicit post-0.6
  follow-ups.
- Runtime owns Browser launch settlement and exact cleanup obligations. The
  close path acknowledges authority renewal, binds close to the exact Browser
  instance, and keeps nonterminal cleanup ownership durable.
- Browser profile state is principal-owned and reset-scoped, but it still lacks
  protected/recoverable Browser profile storage.
- Browser VM Chromium profile disks are principal-owned and reset-scoped, but they are not protected principal-root envelopes or Recovery Kit-packaged state yet.
- Browser profile receipts must continue to report
  `storage_posture=principal_owned_reset_scoped_unprotected`.
- Principal-root object protection exists for selected Home/runtime state; this does not include Browser VM Chromium profile disks yet.
- Product-readiness claims remain gated on target-specific objective audit and
  matching manual UX evidence; inclusion in 0.6.0 does not waive that gate.

## Browser Provider Evidence

- Browser architecture is coherent enough to preserve, but the objective still
  fails product audio proof and hash-bound manual UX evidence.
- Verified on `b07160cf` on 2026-08-16: Browser projection code still selects
  `display_mode`, preferring `webrtc_remote_display` when the selected engine
  advertises it. This is an open authority-placement gap. The target contract
  has Browser request a display capability while Runtime selects the display
  path and engine adapter.
- Docker/Selkies is only `managed_baseline_not_final_product`; the hosted Selkies/GStreamer service is a managed baseline, not accepted as the final Browser.
- The hosted baseline is single-session; active pages are a serialization blocker.
- This server is not a product native-browser proof target because it lacks a real host compositor/display, host audio service, and working network namespace support.
- Verified on the public seed on 2026-08-16: `test -e /dev/kvm` returned 1. The
  seed is a bootstrap and gateway host and may consume a remote Browser Engine;
  it is not a person's Home or a local crosvm/KVM Browser target.
- Kasm Workspaces, BrowserBox, or KasmVNC cannot replace Selkies until the
  operator_control_socket not provisioned blocker and their operator evidence
  requirements are cleared.
- `scripts/browser-provider-decision-report.mjs` summarizes supplied `hosted_bakeoff` and `native_preflight` artifacts and keeps generated placeholder configs out of operator instructions.
- `scripts/browser-provider-runbook.mjs` is read-only guidance. Its operator guidance is generated from the actual evidence and should not be treated as a deployment action.
- Current Browser runbooks must keep the stop condition visible: do not keep
  tuning the running Selkies baseline as product architecture.

## Mac VM Proof Boundaries

- The Mac VM acceptance chain recomputes the receipt SHA-256 from the receipt path and rejects auth setup receipts generated after the machine proof.
- The virtual-auth Browser setup path must drive the virtual-auth Browser open viewport by default.
- Profile reset proof must preserve `removed_profile_disk=true`.
- The virtual-auth credential store remains an owner-only local file.
- The handoff exits non-zero until the headed auth setup receipt is bound.
- `scripts/mac-source-home-restart.sh` remains the source-home restart/proof
  helper for macOS target evidence.

## GBA Capsule Truth

- `gba-emulator` is a conditional viewer capsule, not an always-on native
  provider. It carries one browser-targeted mGBA JS/WASM engine and loads that
  engine only after Runtime supplies compatible uCity or Library `.gba`
  content.
- ROM and save bytes cross authenticated Runtime viewer routes. Save state is
  scoped to the launch principal; the engine has no Runtime WASI adapter,
  preopens, environment, socket, FIFO, or direct-network authority.
- `scripts/normalize-gba-engine-imports.mjs` deterministically converts the
  exact pinned upstream Emscripten import label into the capsule-local
  `capsule.local.memfs.v1` boundary. The product artifact imports only that
  local module and its bundled Emscripten environment.
- `scripts/gba-demo-smoke.sh` proves manifest, Runtime route, authorization,
  storage, input-map, and artifact invariants.
  `scripts/gba-opaque-frame-browser-smoke.sh` proves the same capsule assets in
  disposable Chromium: opaque `Origin: null` topology, parent DOM denial,
  changing nonzero framebuffer writes, trusted keyboard input, nonzero emulator
  audio output, on-screen controls, save/reload persistence, and process cleanup.
- Installed macOS proof covers uCity and Library `.gba` launch, moving frames,
  keyboard/on-screen input, user-enabled audio, save/reload, source-installed-
  served artifact parity, and view cleanup. GBA remains outside the default
  profile and is installed only by explicit `demo` or `full` profiles.

## System Truth

- System has no generic Storage or pseudo-WebSpace inventory section. Files,
  documents, and provider-backed storage remain in their owning apps; System
  keeps real account, appearance, shell, security, source, app/service, and
  device controls.
- Apps and background services come from the Runtime capsule catalog. Privileged
  identity, permission, verification, and approval details remain behind the
  explicit technical inspection surface rather than ordinary app discovery.

## Home Shell Truth

- `/apps/home/` remains the Home front door. The current internal shell model is
  `home-shell-host` for host lifecycle, `home-gui` for the desktop projection,
  and `home-cli` for the command projection.
- `home` remains the installed host/front-door bridge id for `/apps/home/`;
  it is not selectable. `home-gui` and `home-cli` are sibling shell capsules on
  capsule-specific browser origins. They share the same Runtime facts, launch
  validation, lifecycle, sign-out, and explicit shell-switch authority; GUI
  owns windows while CLI owns the Runtime PTY. Visible product language remains
  `Home`.
- `home-cli` replaces the obsolete `esp-shell` capsule as the selectable
  terminal shell. It is a shell-role capsule with no provider authority.
- `home-cli` is a terminal shell over Runtime Home summary, capsule catalog,
  interface, ESP, service, approval, Browser, wallet, people, gate, and audit
  facts. Home CLI TUI actions write a structured Home `intent.json`; the Home
  owner process handles visible app opens and declared runtime-policy affordance
  invocation through Runtime/Home authority. User-approval and high-risk methods
  stay blocked; `home-cli` does not directly call providers or System routes.
- Core first-party manifests now declare typed affordance descriptors across
  app, viewer, shell, connector, content, and provider surfaces. Home-facing
  descriptors cover `home` host facts, `home-gui`, `home-cli`, `browser`,
  `wallet`, wallet connectors, `inbox`, `services`, `system`, `library`, `documents`,
  `archive-manager`, `chat-room`, `chat` terminal, `agent`,
  `marketplace`, `gba-emulator`, and `gba-ucity`; provider-role capsules now
  project authority metadata for service-plane inspection. These descriptors
  are projected as facts for shells and System; Runtime gates, approval, launch
  tokens, providers, and audit remain authoritative.
- `/api/capsules/catalog` now derives `elastos.capsule.projection/v1` for each
  capsule: web, CLI, facts, affordances, gates, audit/mirror, and
  Carrier/service readiness. `home-cli inspect <capsule>` renders these
  Runtime-derived facts instead of guessing from shell-local UI code.
- Machine proof now includes
  `first_party_capsules_have_complete_projection_contract`, which checks the
  first-party development capsule set through the Runtime catalog read model,
  verifies every capsule has the seven shell-facing projection surfaces, and
  confirms `/api/capsules/interfaces` stays count-aligned with catalog facts.
- The browser `home-cli` is now terminal-only in the product path: it autostarts
  a capsule-local xterm.js Runtime PTY stream, focuses the terminal, and keeps
  browser-side command projection out of the product entirely.
- The browser `home-cli` now has a Runtime-owned PTY terminal contract:
  start/events/input/resize/close routes are launch-token gated, event delivery
  uses a scoped stream ticket instead of a Home token, and the capsule renders
  PTY bytes with xterm.js while Runtime owns the process, PTY, dimensions,
  input, resize, and lifecycle. The Home CLI TUI accepts keyboard navigation plus
  SGR mouse wheel movement and tab-row clicks through that PTY.
- Home CLI source is split by existing responsibility into Runtime I/O, line
  views, TUI state, rendering, and view models while preserving one Rust module
  and one snapshot/intent contract. The split is behavior-neutral and does not
  add a framework or command registry.
- `elastos home` / `home-cli` now consumes the shared
  `capsules/home-cli/browser/commands.json` command contract. Home CLI line mode
  embeds it, but first-run help is split into Tabs, Controls, Advanced, and
  Debug. The default path shows only the five user-facing tab commands
  (`home`, `inbox`, `people`, `apps`, `system`) plus controls; `mywebsite`,
  `wallet`, `exits`, `invoke`, `debug`, projection details, raw PTY/xterm
  details, and security wording are hidden until `help advanced`, `help debug`,
  or command-specific help. `home-cli` reads Runtime-derived
  catalog/interface/service facts from the Runtime-owned Home snapshot.
  Low-risk `invoke` still writes a structured Home intent; the Home owner
  process mints a non-delegatable launch token and dispatches through
  `/api/capsules/interfaces/invoke`. Machine proof covers Browser Exit
  service-offer filtering and the serialized Home CLI invoke intent payload;
  user/high-risk methods still fail closed before dispatch.
- Home CLI People now matches the Home GUI People model: profile, contacts,
  pending requests, discovery, and add/remove/message commands stay in the
  default view; room policy, guest sessions, schema/model/source fields, invite
  internals, and transport facts move to `debug people`. Contact message actions
  are available only when Runtime People facts expose a message-capable Home app
  route such as `/apps/chat-room/`; visible contact names and handles resolve to
  the same contact ids as the GUI model, and route-backed actions can say
  `Chat with <person>` without claiming a separate direct-thread transport.
- Home CLI System now keeps the default view to shell switch readiness,
  human session status, trusted source/update policy, and a compact details
  pointer. Services, roots, peers, capsule counts, launch-token/auth wording,
  DID-heavy identity, and detailed diagnostics live under `system source`,
  `system identity`, `system diagnostics`, or explicit `debug ...` topics.
  `system shell home-gui` is ready from browser Runtime PTY mode and unavailable
  from native terminal mode, where no browser root shell is mounted to switch.
- Machine proof covers the signed virtual-passkey System picker switch to
  `home-cli`, full-viewport CLI root mount, isolated `home-gui` root launch with
  no desktop GUI markup or code in the neutral host document, remembered
  alternate-shell first-paint suppression before the Runtime summary round trip,
  stale `home` first-summary suppression before Runtime ensure settles the
  selected shell, no-hint neutral resolving through Runtime ensure before
  selecting `home-cli`, neutral host boot masking until the selected shell,
  auth, or recovery surface is visible, retirement of the previous root shell,
  host-owned neutral auth gating with no
  stale desktop behind the passkey prompt, root-shell-owned window session
  restore, child-intent rejection for wrong-origin, wrong-token, host-route, and
  `home-gui` launch attempts, failed signed switchback recovery without
  mounting `home-gui`, return to `home-gui`, and Home CLI Apps showing
  GUI-only Browser/GBA targets read-only without implicit launch.
  Dynamic `capsule-*` actions come only from the canonical catalog's available
  CLI projection. Browser, GBA, Wallet, and other GUI-only capsules remain
  facts or explicit `open-gui:<target>` actions; Home CLI no longer reloads
  manifests to invent a separate launch matrix.
- Source gates now also assert that GUI chrome projection stays behind
  `home-gui`: the host no longer imports GUI chrome/surface modules or runs
  identity, Inbox badge, Wallet toast, or toolbar clock projection while an
  alternate shell owns the root.
- The Home host no longer queries or mutates desktop/taskbar/launcher DOM nodes
  or GUI window registries. It marks shell lifecycle state and routes validated
  launch-token intents; `home-gui` owns GUI node creation, binding, rendering,
  and window state inside its isolated root frame.
- Home Host summary handling does not require GUI DOM or GUI layout state.
  Desktop layout, browser-window session state, glyphs, and GUI surface state
  live in `capsules/home-gui/browser/shell-core.js`. A normal Home CLI action
  stays in CLI; opening a GUI-only target requires an explicit, launch-token-
  gated `switch shell and open` intent.
- The Home shell implementation, source gates, and machine smokes pass for the
  current working tree. The origin-isolation change requires a fresh
  commit-bound operator pass covering passkey sign-in, System switching to
  `home-cli`, CLI ownership of the full viewport, no desktop first-paint or
  hidden GUI bleed-through, hard reload into the selected shell, and return to
  `home-gui` without a passkey loop before merge readiness can be claimed.
- `scripts/home-shell-objective-audit.mjs` remains the fail-closed completion
  audit. Manual evidence is commit-bound and intentionally not stored in the
  repository; any later Home shell behavior change requires a new or re-reviewed
  report against the exact candidate commit.

## Collaboration Truth

- Collaboration represents a person through a signed Profile DID. The human
  actor, local principal, signed Profile, and Device DID remain separate. The
  Profile authorizes Carrier endpoint DIDs for routing and scoped signer DIDs
  for application authorship. Neither role is a display name, contact key, or
  browser-visible selector.
- Runtime owns identity derivation, protected state, and Carrier/provider
  mediation. Capsules receive bounded read models and opaque selectors only.
- `chat-room` is the sole Chat product in manifests, demos, build, and
  supported release profiles. The old terminal `capsules/chat` and
  `capsules/agent` source trees are retired and cannot return as a raw-peer
  compatibility path.
- Chat and People use Runtime-mediated collaboration services selected by a
  signed network profile. Runtime owns their registration, workers, shutdown,
  and the long-lived Carrier endpoint. The seed and profile signer are
  bootstrap and configuration authority only, never person, contact, or
  message authority.
- Collaboration messages identify the sender Profile and either a Profile or
  conversation recipient. Runtime routing and Carrier transport no longer
  replace those product identities. A generic acceptance receipt proves only
  that the named endpoint durably accepted the exact envelope; it is not a
  person delivery or read receipt. For direct messages, contact revocations,
  and Profile updates, the receiving Runtime derives the source endpoint from
  the authenticated Carrier connection and checks it independently from the
  Profile-authorized application signer. Shared-room gossip still requires its
  signer to be endpoint-authorized because that admission path does not yet
  receive an equivalent authenticated transport-source fact.
- Peer-DID provider routing resolves through the long-lived Runtime Carrier
  endpoint. The resolved endpoint identity must match the requested DID, and a
  route with no verified peer reports the peer as unavailable before any
  provider effect.
- Verified on `b07160cf` on 2026-08-16: a same-endpoint Peer DID stays on the
  Carrier provider plane and enters authenticated admission through the local
  registry without a network dial. Direct messages settle the same signed
  envelope and signed acceptance receipt used by the remote contract. This was
  implemented by `8dd54706`; it is not an open gap on this review line.
- Discovery is explicit, opt-in, bounded, and temporary. Accepted contacts are
  derived from signed request and decision chains, and Inbox is the only
  Accept/Decline authority surface.
- Direct conversations are permitted only between accepted Profile contacts.
  The conversation ID is derived from the two Profile DIDs and the network ID
  and is a selector, not authority.
- Direct delivery is durable on the sender and point-to-point with no relay.
  `send_text_with_context` persists the signed envelope before the first
  delivery attempt, and `retry_pending` re-delivers on the 15-second sync
  cadence until the envelope's 24-hour TTL expires, surviving sender restarts
  (`durable_pending_restarts_with_the_exact_envelope_and_settles_once`). The
  recipient does not need to be online at send time; it needs to become
  reachable within the TTL while the sender's Runtime is running. The real gap
  is a sender that goes offline before the recipient returns: there is no
  third-party store-and-forward, and an expired envelope is abandoned and
  reads `expired`, never `pending`. The seed never sees message plaintext.
  The shared room has the matching reach limit: gossip topic buffers are
  in-memory on whichever peer holds them, so a peer offline past that
  buffer's retention, or across a restart of the holding peer, misses that
  interval; whatever arrives is ingested durably. Profile update catch-up is
  bounded by the 8-revision announcement ring. A contact further behind
  fails closed with an explicit refusal and needs a fresh approval.
- A Profile authorizes exactly one device today. The signed document supports
  several, but the product path always writes the current local device, so
  Profile update delivery covers renaming and carrying a data root to another
  machine rather than pairing a second concurrent device.
- Recovery restores the collaboration identity. The Full Recovery Bundle
  carries the Profile signing seed, its retained revision ring, and the signed
  contact store; a fresh-machine import keeps the Profile DID, authorizes the
  new device through the normal signed-revision path, and accepted contacts
  learn the rebound endpoint from one announcement. An import whose identity
  restore fails is reported incomplete and never claims a complete account
  recovery.
- The collaboration path on `origin/review/collaboration-candidate` at
  `46e51a77` is published for review but is not released or deployed product
  truth. Its disposable, fixture-owned two-Runtime product journey passed on
  exact source-built artifacts. The current candidate is installed on
  normal localhost with source/installed artifact parity, HTTP 200, accepted
  People/Chat/Inbox/Clipboard/restart evidence, and machine Browser
  open/connect/close/zero-residue proof, but its one-Runtime product acceptance
  is not complete because manual Browser visible video/input usability remains
  open. The public seed has not been updated to this candidate and is not
  matching product evidence.
- Bilateral signed contact removal is implemented with the complete People
  states: a pair-scoped signed revocation delivered over the direct channel
  with durable retry, visible removed state on both sides, retained heads as
  the signed name source, and history readable under the declared policy.
- Shared-room attribution is implemented: configured-room rows are named from
  signed Profile truth (own Profile authority, accepted and retained heads,
  membership profile cards) or rendered explicitly unverified; presence is
  liveness-only and durable history is untouched.
- Signed Profile update delivery is implemented: renames travel as an exact
  bounded signed chain over the dedicated `collaboration-profile` provider,
  apply under strict next-revision and chain-hash rules, and re-announce
  idempotently after restart. The two-runtime proof surfaced and fixed a real
  wire gap: the Carrier peer provider plane had never admitted the profile
  provider. Design and boundaries live in
  [docs/COLLABORATION_HANDOFF.md](docs/COLLABORATION_HANDOFF.md); the
  strict fixture-owned installed two-runtime acceptance now passes: Recovery,
  distinct Runtime/Profile evidence, opt-in Discovery, exact Inbox approval,
  direct messages both ways, rename, bilateral removal, re-add, shared-room
  continuity, restart continuity, Clipboard, narrow UI, and final People/Chat
  identity scans all passed on two fresh loopback Homes.

## Remote Carrier Exit Evidence

- Operator evidence must reject local redacted artifact hash mismatches.
- Operator evidence must reject local redacted artifacts that still contain private route material.
- Operator evidence must reject stale or route-mismatched hash-bound route-readiness reports.
- Operator evidence must reject stale local installed artifact readiness reports.
- Operator evidence must reject missing route principals.
- Operator evidence must reject local Browser machine-proof artifacts that do not cite the reviewed route target or target host.
- Operator evidence must reject weak evidence that does not cite the reviewed source/exit runtime DIDs and endpoints.
- Operator evidence must reject weak evidence that does not cite the reviewed principal/grant/target/Carrier stream/cleanup route nouns.
- Remote Carrier Exit readiness must remain hash-bound remote route readiness.
- Public-live update planning must stage candidate binaries in a server-side candidate directory before explicit install approval.

## Public Install Truth

- Public-install branch-binary smokes must pin the installer-selected components manifest.
- Public-install branch-binary smokes prevent source checkout `components.json` from leaking into installed-path proof.
- Public-install branch-binary smokes fail if the selected gateway lacks the current `home` setup profile.
- Branch-override public smokes require a staged or published 0.6.0-compatible
  manifest with the current `home` profile and checksummed artifacts.
- Source/local Carrier setup proof stays in `scripts/local-carrier-setup-smoke.sh`.
- Public install proof for this candidate requires a staged or published
  0.6.0-compatible manifest with the current `home` profile and checksummed
  artifacts.
- Set `ELASTOS_PUBLIC_INSTALL_FORCE_RELAY_ONLY=1` only when the publisher relay
  path itself is under review.
- Final public installed-path proof waits for publishing the 0.6.0
  binary/artifact set.

## Open Blockers

- Product Browser completion is not claimed.
- Manual installed-device checks on Mac and Linux/aarch64 targets are still
  required before release handoff.
