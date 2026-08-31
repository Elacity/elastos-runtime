# State

Last updated: 2026-08-31 UTC

This file records public-safe current truth for released 0.6.0 and active
development work. Private operator paths, credentials, target identities, and
volatile proof logs remain outside the repository.

## Release Posture

- A fresh fetch records `main` at `d358dedb` as the released 0.6 source and
  `origin/upstream/0.7-dev` at `e481b153` as the current 0.7 integration line.
- The local 0.7.0 release-preparation commit on top of `e481b153` sets the
  coordinated workspace version to `0.7.0`, gives every capsule manifest
  changed since 0.6.0 a minor bump (new capsules keep their initial manifest
  versions), cuts the `0.7.0` changelog entry, and refreshes
  `elastos/Cargo.lock`. Installed artifacts report `0.7.0` only after the
  checked publish flow stamps `ELASTOS_RELEASE_VERSION`; unstamped source
  builds report `0.7.0-dev`.
- Published feature evidence is
  `origin/feat/protected-content-runtime-lifecycle@854d9dc9`,
  `origin/feat/protected-content-uiux-reconstruction` (PR39),
  `origin/feat/0.7-uiux-candidate@8b547590`,
  `origin/feat/dkms-esp-port@27d85c6f`, and
  `origin/feat/0.7-product-documentation@74cd4e42`.
- The published protected-content stack is contracts `0c56c56a`, custody
  `2f844cef`, key reconstruction `467a6c03`, custody provider `1b7fa732`, Wallet
  rights `c9e82e75`, Runtime `a8ac6dc8`, and rights `3627da01`. Every tip is an
  ancestor of the published lifecycle and is already present in the active
  integration. The latest published protected-branch repairs need no new
  extraction.
- PR39 includes the Home audit fixes, the named principal-root write policy,
  checkout-bound test fixtures and the privacy-reviewed audit workbook at
  `d06d64f3`. All seven CI jobs pass on that revision, including macOS,
  both Linux source-home targets and the release build. New local inclusions
  require CI on their exact revision; installed product acceptance stays separate.
- The local source includes the CPU watcher optimization from `e4d897f6`.
  Unchanged executable metadata skips binary hashing; a changed stamp triggers
  a streamed digest. Focused tests cover idle ticks, replacement and deletion.
  Installed CPU measurement remains open.
- The source-merge scope preserves reviewed history and includes the exact
  PR43 commit `58ebfb23`. PR39 then feeds `upstream/0.7-dev`, which feeds
  `main`; fetching refs establishes the current heads before each step. A
  merge commit preserves the original commits rather than squashing them.
  Main integration, release publication and installed cutover are separate
  actions. The deferred scope remains explicit in [TASKS.md](TASKS.md).
- The published audit changes include Home and Terminal repairs, create-only
  Library writes, document close protection, Assistant and model-init repairs,
  declared-content icons, private diagnostics, socket-root protection, and Mac
  restart safety.
  The history review preserved the product tree and incorporated the published
  CI/setup changes. Installed acceptance still belongs to exact artifact
  receipts and recorded GUI outcomes.
- Upstream `90bbe15b` records Irzhy's verified Base 8453 evidence and changes
  Chain provider source, tests and protected-content documentation.
  The branch preserves the published
  protected-content repairs and includes that evidence once. It also retains
  upstream collaboration and Browser local-exit orphan cleanup.
- Irzhy's updated PR43 repair, `58ebfb23`, is included in the local source.
  Runtime adopts fully completed mint records after a lost intent completion
  mark. Ambiguous or partial records fail closed; partial custody cleanup and
  installed mint/restart acceptance remain open. The current named-policy
  helper already resolves the auth lint warning; the updated PR43 drops its
  earlier lint suppression.
- The protected-content source path remains inactive. Installed proof and one
  atomic cutover remain open.
- Commits `3026992b`, `ed7a8bfc`, and `7f6e47f9` provide portable listing
  publication and import, buyer purchase, and buyer open, read, and close
  without creator Runtime mint state. The package binds the public custody
  identity to Chain-committed metadata and uses one immutable listing
  projection on each Runtime.
- Commits `ba7f6cea` and `84569da5` complete exact buyer Runtime rights admission
  and the two-Runtime source journey. Runtime A keeps provisioning authority for
  the real process-backed 2-of-3 custody nodes. Runtime B imports the listing,
  buys it, and completes open, read, and close with its own Profile, Wallet,
  device identity, state, and signed release operation.
- Playback reconstructs from the authenticated release operation, verified
  signed epoch, released contributions and terminal receipt, recipient
  possession, and public CEK commitment. Provisioning still uses
  `CustodyEnvelopeV1`; Runtime stores no playback copy of the custody envelope.
- The combined protected-content gateway proof uses private Runtime targets for
  protect, media, custody, and decrypt. Carrier supplies authenticated endpoint
  transport before the Runtime-selected custody target handles the request.
  Public provider projection excludes these targets.

## Branch Hygiene

- Local UIUX subgroup branches are extraction scaffolding already contained in
  the published UIUX candidate and active integration. They need no separate
  publication.
- Local accepted protected-content labels whose tips are ancestors of the
  published lifecycle or upstream need no separate publication.
- Retained donor branches and dirty worktrees remain under the operator
  ledger's preservation rules. The August review carried useful content,
  Recovery/Profile, Windows and operator documentation into this candidate.
  Older Assistant and migration donors retain explicit deferred tasks.
  Preserve unique history and original dirty files until their owners approve
  cleanup; published source does not make every older hunk equivalent.

## Integrated Source Truth

Runtime retains `tracing`. Irzhy postponed the replacement `elastos-logger`
on August 30; its absence is an intentional decision, not an omitted release
feature. Its useful VM-payload privacy repair is included independently in
`74ed3bc9` and has a log-capture regression test.

The older July Carrier branch contains framing, deadline and protocol work
that needs an adapted integration: the current incoming request handler still
has an unbounded line read, while the donor's whole protocol would disable
provider invocation used by the current protected-content path. This remains
an explicit source-integration decision in [TASKS.md](TASKS.md). Inclusion of
all retained work requires a behavior-level comparison, not only commit counts.

Older Assistant attachment, knowledge/search/citation and advanced Studio
implementations remain retained donors for the open work in [TASKS.md](TASKS.md).
The PR15 legacy-auth migration also remains separate because it replaces
unchained audit history. Current signed-checkpoint policy owns compatibility;
retaining those donors does not mean their behavior is in the candidate.

The reviewed content-distribution, Recovery/Profile and WSL-first documents
are included. The catalog currently projects installed capsules; signed network
discovery, Home Get and model-content packaging remain planned work. WSL
packaging and native Windows support also remain unproved product targets.

Runtime owns authenticated principal and session authority, capability
admission, provider selection, lifecycle, durable operation identity, Wallet
and Chain coordination, audit, and settlement. Providers own their typed
operation semantics. Carrier transports only Runtime-selected endpoint traffic.
Capsules own presentation and app behavior and receive bounded read models and
opaque selectors only.

The integrated source includes these durable facts:

- Collaboration identifies people by Profile DID and uses endpoint DIDs for
  private Runtime routing. People and Chat consume typed Runtime projections.
- Home owns shell chrome, launch framing, focus, fullscreen, clipboard
  mediation, notifications, and sign-out. Capsules own their content and icons.
- Wallet owns accounts, approvals, signatures, and transaction effects. Runtime
  binds protected-content creator and buyer operations to the verified Wallet
  account and configured Chain authority.
- Assistant is a capsule. Runtime selects one model offer through
  `ProviderRegistry`, and `model-provider` owns model execution. Assistant
  renders bounded, sanitized markdown and math from typed output.
- Library prepares, protects, lists, and opens video through typed Runtime
  operations. Marketplace reads immutable listing projections and submits only
  the mint identity for buy or open. `elacity-player` owns video presentation.
- Runtime keeps the canonical protect, media, custody, and protected-content
  decrypt providers on reserved private targets. Public capsule lookup,
  interface projection, and provider routes exclude these targets.
- Protected publication requests exactly three replicas. The repair task keeps
  the same requirement. Purchase and open both require fresh signed
  availability bound to the exact protected object.
- Protected Chain configuration is an owner-only
  `protected-content/chain-provider.json` file. It contains one versioned
  `protected_content_network`; Runtime supplies its operation issuer
  separately. Node-local rights evaluation uses 2-5 explicit private RPC
  sources and requires two exact agreeing finalized results.
- `scripts/protected-content-installed-static-audit.py` reads installed
  artifacts and emits
  `elastos.protected-content.installed-static-audit/v1`. It reports source and
  static artifact failures, operator configuration prerequisites, and active
  installed proof prerequisites separately. `ready_for_active_proof` is a
  static admission result, not product readiness.
- Full `scripts/setup-source-home.sh` installs one stable Runtime at the
  platform data root under `bin/elastos`. It writes the owner-only
  `receipts/source-home-installation.json` receipt after components, native
  providers, capsule trees, and source-home capsule metadata are final. The
  receipt binds source commit/tree/clean state and exact artifact hashes.
  Setup requires at least 10% free space on both source and data volumes before
  builds. Its private install stage is removed on success, copy failure and
  installer failure, as verified by the isolated installation smoke. Source
  setup and Browser target refresh each retain one default VM backup set.
- `scripts/mac-source-home-restart.sh` and
  `scripts/linux-source-home-restart.sh` select only that stable Runtime.
  Each owns one exact PID file, stops only the identity-bound prior Runtime,
  retains at most one bounded principal-root rollback, and writes an owner-only
  restart receipt. Mac default mode uses the existing installation; `--init`
  also requires current clean source and artifact parity. Mac dry-run validates
  without stopping the Runtime, including with `--down`. Runtime owns provider
  shutdown.
- The macOS replacement-restart path is proven in its fixture on this host,
  including a live prior Runtime after atomic binary replacement. Linux dry-run
  and fixture proof is source evidence; active `/proc`, listener, and binary
  replacement behavior still requires Linux target evidence.

## Protected-content Contract Truth

KID and `EncryptedContentIdentityV1` are separate identities. The bytes16 CENC
KID is the deployed AuthorityGateway access key. The full encrypted-content
identity binds the protected object and media contract.

The active local branch preserves the verified deployed read behavior:

- `AuthorityGateway.hasAccessByContentId(address,bytes16) -> bool` owns the
  access read.
- `CentralStorage.ipReference(bytes16)` resolves the KID for that read.
- An unknown KID reverts with `UnboundContentId(bytes16)`; a bound KID without
  access returns `false`.

`origin/upstream/0.7-dev@90bbe15b` records Irzhy's deployed Base 8453 probe
evidence, already included in this branch:

- `CentralStorage.bindIP(bytes16,address,uint256)` accepts acknowledged
  contracts only and is called by `AssetFactory.registerNewAsset`.
- Native `AuthorityGateway.buyAccess` uses selector `0xf7580ad9`.
- ERC20 `AuthorityGateway.buyAccess` uses selector `0x0ede2294`; Wallet approval
  targets each operative `paymentProcessor()`.
- EventHub emits mint events.
- Upstream records bound-KID allow, deny, and unbound evidence.

The exact funded buy receipt and event remain installed proof items. Deployed
`View` and `Download` still map to one boolean Chain access result, so signed
Runtime policy owns the action distinction until contract evidence defines it.

The canonical source path keeps Runtime journals limited to identities, state,
receipts, and settlement. Protect, custody, and decrypt providers keep clear
media, ciphertext staging, CEKs, and shares inside their private process
boundaries. Each custody node owns one independent share and its node-local
rights check. Runtime and capsules do not receive private provider, storage,
Chain, RPC, or Carrier topology.

## PR15 Extraction Ledger

PR #15 / `feat/dkms-esp-port` is source evidence, not a merge target. The
integrated source adapted these useful parts to the typed Runtime path:

- `6d2e9083`: player/viewer behavior and Library-open UX now use typed Runtime
  launch, read, and close operations.
- `c5aed9db`: Creator UX now appears as the Library protect-and-list flow.
- `57974479`: the grant journey maps to current Profile, session, Wallet, and
  Runtime authority.
- `ffea5998`: useful Create, mint, and open failure cases are covered by the
  current typed paths.
- `e148218b`: applicable CI lessons remain in the focused source and platform
  gates.

Current video opens in `elacity-player`. Document and 3D viewers remain later
typed-viewer scope. External cryptographic review remains open before public
dKMS or production confidentiality claims. Global listing discovery and public
custody governance remain later work. The shared listing link, portable import,
buyer Runtime rights admission, and exact two-Runtime 2-of-3 source journey are
complete. Installed proof and the atomic authority cutover remain open.

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
- Browser launch, TURN/media-relay connection, Runtime-mediated traffic, exact
  terminal close, and zero-residue behavior require fresh target evidence for
  the exact integrated commit. Human-visible video, input, scrolling, and audio
  remain manual proof gates.
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
- Hosted tooling supports per-launch targets. Each engine/control service keeps
  its declared page capacity; concurrent product sessions require their own
  installed capacity and cleanup proof.
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
- The GBA source catalog contains exactly `gba-nonogram` and `gba-ucity`.
  Shared visual tokens remain inside the GBA capsule. GBA stays outside the
  default profile and belongs only to explicit `demo` or `full` profiles.
  Installation, target media, input, save/reload, and cleanup proof remain
  separate gates.

## Model And Assistant Truth

- `model-provider` is the single active model execution path. Runtime registers
  it as a verified native provider and authorizes only the typed
  `offers_list`, `runs_create`, `runs_get`, `runs_events`, and `runs_cancel`
  operations.
- The operator-owned model-provider config lives under the Runtime data root at
  `providers/model-provider/config.json`. Runtime validates the fixed path and
  file security, passes only the raw top-level offers value through Init, and
  keeps backend URLs, credentials, and adapter details below the provider
  boundary.
- A missing installed components manifest or model-provider entry leaves the
  provider unconfigured and unavailable. Runtime does not select a fallback.
- Missing model-provider config is an honest zero-offer state: Runtime may
  start/register the provider with no offers, writes no config file, and
  Assistant shows that no model offers are available.
- `model-provider` now accepts the Runtime Init envelope fields
  `base_path`, `allowed_paths`, `read_only`, `encryption_key`, and `extra`
  without weakening strict unknown-field handling. The zero-offer stdio Init
  test passes with the Runtime envelope in source tests.
- Assistant is a standalone first-party capsule. Chat, Build, and Studio use
  only typed Runtime model resources and the protected Assistant workspace;
  transcript copy goes only through the trusted Home Clipboard path.
- Assistant model messages render a self-contained safe markdown subset with
  escaped HTML, inert links, headings/lists/blockquotes/tables, fenced and
  inline code, and inline/display math through vendored KaTeX 0.18.3. Focused
  source proof lives in `scripts/assistant-shell-smoke.mjs`. The Home audit
  records observed UI behavior separately; configured model-run and advanced
  workflow acceptance remain open.

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
  `archive-manager`, `chat-room`, `assistant`,
  `marketplace`, `gba-emulator`, `gba-ucity`, and `gba-nonogram`; provider-role capsules now
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
- The Home shell source gates assert these ownership rules. The
  origin-isolation change requires a fresh
  commit-bound operator pass covering passkey sign-in, System switching to
  `home-cli`, CLI ownership of the full viewport, no desktop first-paint or
  hidden GUI bleed-through, hard reload into the selected shell, and return to
  `home-gui` without a passkey loop before merge readiness can be claimed.
- `scripts/home-shell-objective-audit.mjs` remains the fail-closed completion
  audit. Manual evidence is commit-bound and intentionally not stored in the
  repository; any later Home shell behavior change requires a new or re-reviewed
  report against the exact reviewed commit.
- Home first-run onboarding now honors the existing `settings=security`
  deep-link, focuses the recovery action when verified readiness becomes
  available, and refreshes Home summary state after Recovery Kit export. People
  setup prefills the suggested first Profile name as editable text, preserves
  unfocused edits across refresh, and still requires explicit create or
  confirm.
- Declared content icons stay capsule-owned and serve only manifest-declared
  icon variants. Nested content entrypoints resolve icons from their matching
  serving root, and declared icon requests reject ROM bytes, traversal, and
  symlinked targets.
- Fresh desktop placement seeds only visible targets on first run. Saved hidden
  target positions remain intact after later reloads.
- Current source proof for the onboarding slice is focused and local:
  `recovery_readiness_change_emits_home_summary_event_only`,
  `test_recovery_readiness_and_first_profile_gate_share_one_recovery_rule`,
  `scripts/people-discovery-smoke.mjs`, and
  `scripts/home-shell-regression-smoke.mjs` pass. The Home audit keeps installed
  outcomes separate from those source tests. Empty-machine recovery coverage
  for a first kit that predates the later random Profile key remains open.
  Manual GUI acceptance still requires the exact installed artifact.
- A fresh Recovery Kit export is now truthful about included People identity.
  Source still needs a separate repair for empty-machine recovery when the first
  kit predates the later random Profile key.

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
  implemented by `8dd54706`; it was not an open gap in that snapshot.
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
- Historical verification at `b07160cf` records a disposable, fixture-owned
  two-Runtime collaboration journey. Current localhost and public-seed product
  claims require fresh artifact and target evidence for the exact integrated
  commit.
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
  strict fixture-owned two-Runtime source proof covers Recovery,
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
- Public install proof for an integrated candidate requires a staged or published
  0.6.0-compatible manifest with the current `home` profile and checksummed
  artifacts.
- Set `ELASTOS_PUBLIC_INSTALL_FORCE_RELAY_ONLY=1` only when the publisher relay
  path itself is under review.
- Integrated 0.7 installed-path proof waits for one reviewed source tree and
  matching stable installation receipts on each target role.

## Open Blockers

- Product Browser completion is not claimed.
- Manual installed-device checks on Mac and Linux/aarch64 targets are still
  required before release handoff.
