# State

Last updated: 2026-07-31 UTC

This file records public-safe current truth for the 0.5.0 baseline, the 0.6.0
release candidate, and active feature branches. Historical
local proof logs, private SSH aliases, tunnel ports, operator usernames, key
paths, worktree paths, and target backup paths are intentionally not tracked in
the public repository.

## Release Posture

- `main` is the 0.5.0 baseline. Active feature branches must state whether they
  are preserving 0.5.0 behavior or intentionally moving the product architecture.
- `fix/elastos-shell-protocol-browser-wallet-consolidation` is the published
  0.6.0 review line. It is based on `feat/elastos-shell-protocol`, which
  descends from `upstream/0.6-dev`; it includes the reviewed ESP, Wallet,
  Recovery, and Browser continuation and keeps the accepted implementation
  history intact. It is not merged to `main`.
- Carrier reconciliation, shell UI redesign, and extended AI UI work are not
  included in 0.6.0. Carrier moves to 0.7; the UI work requires an independent
  compatibility and product review before a later release.
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
  data roots, or local worktree paths.
- Private proof logs stay outside this repo. Public release notes should claim
  only proof that can be reproduced from the reviewed source or from
  explicitly supplied target evidence.

## Capsule Execution Truth

- [docs/CAPSULE_MODEL.md](docs/CAPSULE_MODEL.md#isolation-boundary)
  defines the cross-branch isolated-execution contract. It is a 0.6
  architecture requirement, not an additional product claim for the 0.5.0
  `main` line.
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

## Proof Path Ledger

- Installed operator/update proof path: `public-install-operator-smoke.sh`.
- DID/profile proof path: `public-install-identity-smoke.sh` or
  `local-identity-profile-smoke.sh`, depending on target role.
- Public Linux runtime portability proof path:
  `audit-linux-runtime-portability.sh`.
- Protected-content provider journey proof path:
  `protected-content-provider-contract-smoke.sh`.

## Browser Truth

- Browser is included in 0.6.0 as a bounded Runtime Browser, not as a fully
  reliable general-purpose Browser claim.
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
- Docker/Selkies is only `managed_baseline_not_final_product`; the hosted Selkies/GStreamer service is a managed baseline, not accepted as the final Browser.
- The hosted baseline is single-session; active pages are a serialization blocker.
- This server is not a product native-browser proof target because it lacks a real host compositor/display, host audio service, and working network namespace support.
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

## Canonical Journey Proofs

These reusable proof paths carried over from the flint baseline and remain the
named acceptance commands for their journeys (the alignment gate pins them here
so the ledger and the proofs cannot drift apart):

- `scripts/local-identity-profile-smoke.sh` / `scripts/public-install-identity-smoke.sh`
  — DID/profile proof path (source-local and installed-path).
- `scripts/public-install-operator-smoke.sh` — installed-path operator-node
  status/update acceptance path.
- `scripts/audit-linux-runtime-portability.sh` — explicit public Linux runtime
  portability proof (with `just verify-release` as the release-trust gate).
- `scripts/protected-content-provider-contract-smoke.sh` — protected-content
  provider boundary proof over the real DRM/rights/key/decrypt JSON line
  protocols.

## Open Blockers

- Product Browser completion is not claimed.
- Manual installed-device checks on Mac and Linux/aarch64 targets are still
  required before release handoff.
