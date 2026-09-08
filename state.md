# State

Last updated: 2026-09-08 UTC

This file records public-safe current truth for released 0.7.0 and active
development work. Private operator paths, credentials, target identities, and
volatile proof logs remain outside the repository.

## Release Posture

- A fresh fetch records `origin/main` at `8ac18bec` as the released `v0.7.0`
  source and `origin/upstream/0.7.1-dev` at `c511b133` as the active
  integration line.
- Released `v0.7.0` already carries the coordinated workspace version,
  changelog, manifest bumps, and lock refresh. Installed artifacts report
  `0.7.0` only after the checked publish flow stamps
  `ELASTOS_RELEASE_VERSION`; unstamped source builds report `0.7.0-dev`.
- Published integration remains `origin/feat/0.7.1-integration@12d38266`.
  Local merge `91209988`, tree `16f8f020`, preserves the five reviewed groups:
  artifact verification `c95cf4c9`, local-engine lifecycle `7c4fc929`, hosted
  evidence `fcb8fc5e`, Responses and honest cancellation `0d768415`, and delivery
  documentation `ed275ba0`. Its other parent preserves auth ceremony binding
  `145fec2b`, secure credential persistence `eb25f747`, and durable first-owner
  enrollment `43b8f830`. Two syntax-only setup lint corrections are in the merge.
  Combined identity, auth, model, Home and bootstrap fixtures, strict targeted
  Clippy and formatting pass locally. Independent review of the combined source
  through integration code tip `6c6c2fab` is complete. Publication and remaining
  installed behavior acceptance stay open.
  Earlier `900d7e5c` remains the broad installed UIUX/protected-content proof.
  PR52, PR54 and PR55 work
  already present in the candidate must be compared by patch if upstream changes.
- The published protected-content stack is contracts `0c56c56a`, custody
  `2f844cef`, key reconstruction `467a6c03`, custody provider `1b7fa732`,
  Wallet rights `c9e82e75`, Runtime `a8ac6dc8`, and rights `3627da01`.
  Every tip is an ancestor of the published lifecycle and is already present
  in the active integration. The latest published protected-branch repairs
  need no new extraction.
- `main` and `origin/upstream/0.7.1-dev` already include the reviewed Home
  audit fixes, the named principal-root write policy, checkout-bound test
  fixtures, the privacy-reviewed audit workbook, the completed-mint adoption
  repair from `58ebfb23`, and the equivalent CPU watcher optimization at
  `8e53174f`. The reviewed donor `e4d897f6` is the source comparison, not an
  ancestor. The earlier candidate source passed formatting, alignment, Home
  and Browser entropy, Home shell, People discovery, the 26-case Browser close
  handshake, and Home Agent shell checks. Combined-merge proof is listed above;
  remote CI remains separate from local checks.
- `origin/upstream/0.7.1-dev` also carries Irzhy's verified Base 8453 probe
  evidence, shared build-artifact staging, upstream collaboration work, and
  Browser local-exit orphan cleanup. The protected-content source path remains
  inactive. Installed proof on isolated localhost, the seed and third custody
  node, and one atomic cutover remain open.
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

## Installed candidate proof

- The isolated localhost installation at `localhost:61380` has source commit
  `900d7e5c` and tree `c9a9effe`. This is installed acceptance for that local
  candidate. It is not seed, third-node, or cutover proof.
- The manual Brave journey opened and inspected System, Home launcher and
  windows, Profile and People, Chat, Inbox, Wallet, Marketplace, Services,
  Library, Archive, Documents, Player, standalone Assistant, Home Agent, GBA
  Emulator, Nonogram Advance, uCity, and Browser. Home fullscreen fills the
  workspace and returns to the saved layout.
- Both games render and their expected controls work. Browser opened
  `ela.city` through the VZ adapter and closed with no per-launch supervisor or
  TURN server residue. Startup took about 34.5 seconds. Transient TURN
  authentication failures occurred before allocation succeeded, so startup
  latency and those diagnostics need follow-up.
- Home Agent opens from the desktop and from Ask Assistant into the Home-owned
  Agent Space. This broad candidate installation had no configured model offer
  and reported that state directly. A later isolated macOS model proof is
  recorded under Model And Assistant Truth.
- People reports discovery as unavailable because the isolated Home has no
  collaboration configuration. The pending Wallet approval remained unchanged
  during the journey.
- Protected-content acceptance is blocked on one real three-node custody
  composition, private Chain and RPC configuration, three replicas, funded
  creator and buyer accounts, and installed two-Runtime proof. No mint, buy,
  playback, seed, third-node, or cutover claim follows from this localhost run.
- Recovery Kit protection and Profile creation worked in the isolated macOS
  model Home. The passkey, Recovery Kit protection, Profile, and Home Agent
  session survived a gateway restart from the same stable data root. Recovery
  Kit and Profile navigation remain slow and indirect and need a focused UX
  repair.
- Standalone Assistant and Home Agent are both installed pending an explicit
  product decision. The final default product should present one clear Agent
  surface. Multiple local Brave app copies can create duplicate Dock and
  recent-app entries; this is local operator hygiene, not product architecture.

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
are included. Current content-distribution facts and gaps are recorded below.
WSL packaging and native Windows support remain unproved product targets.

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
  Setup requires 16 GiB free on each source and data volume before builds. The
  operator process separately requires at least 10% free space on development
  and staging volumes. Setup removes its private install stage on success, copy
  failure, and installer failure, as verified by the isolated installation
  smoke. Source setup and Browser target refresh each retain one default VM
  backup set.
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

## Content Distribution Truth

- The content plane implements `elastos://content` publish, fetch, status,
  ensure, repair, and unpublish operations. The content provider owns that
  capsule-facing contract and uses the IPFS provider as a private low-level
  backend. Raw `elastos://ipfs` authority stays system-only.
- Local publication records signed availability receipts. Those receipts prove
  accepted local retention only; independent network replication needs its own
  evidence. Gateway CID reads serve verified content through the Runtime-owned
  route.
- The current Runtime catalog source projects installed capsules and optionally
  verifies one operator-pinned signed model catalog snapshot. The existing
  `components.json` config supplies a raw CIDv1/SHA-256 head and a nonempty
  trusted publisher set independently from the entry. The authenticated catalog
  projects the verified publisher DID, declared package CID, exact declared
  size and bounded model facts as `unprepared`, with installed and launchable
  both false. Same-name local files do not establish admission or readiness;
  malformed model trust leaves ordinary installed inventory available.
- The first-Qwen metadata profile and limits are in
  [Content capsule distribution](docs/CONTENT_CAPSULE_DISTRIBUTION.md#implemented-catalog-metadata-profile).
  Source tests pass 3 focused metadata tests, all 95 common tests, 6 model
  catalog tests and the final 25-test server capsule-catalog regression.
  Workspace/Chain formatting, Home/public-copy entropy and diff checks pass.
  These prove metadata consistency and publisher verification,
  including rejection paths; the composed preparation proof is recorded below.
  Installed Homes remain unchanged. Model-provider still consumes
  static private artifact/offer configuration, and Marketplace/System model
  selection remains future work. Model and Assistant Truth below records the
  existing Qwen operator bootstrap.
- The signed, complete-closure CID model path is required in the current
  closeout, alongside onboarding and window policy. People select or use a
  model; Runtime prepares the exact content behind that action. Settings local
  retention is separate from trust, admission and provider readiness. The plan is in
  [Content capsule distribution](docs/CONTENT_CAPSULE_DISTRIBUTION.md). CID owns
  byte identity, publisher signature owns the source claim, availability
  receipts own retention evidence, Runtime owns policy and atomic admission,
  and content and availability providers own backend selection and routes.
  Package identity remains separate from model service offers.
- Explicit `bounded_read: true` now carries a closed range through local
  Content to native IPFS, with a 64 KiB per-read cap and five-second total HTTP
  and body deadline. Native reads use the ready backend and its existing activity
  record, with redirects and proxy inheritance disabled. They perform no startup,
  pin, retry or fallback. Runtime checks the private CID/path/range receipt and
  exact byte count, consumes the range once, and removes that receipt from Bytes
  and Stream output. Remote bounded calls fail before dispatch.
- Complete `_elastos_object.json` reads use `max_bytes` up to 64 KiB, with
  Kubo length capped at one extra byte. EOF within the cap and an exact private
  CID/path/completed/length receipt are required. Runtime preserves whitespace
  and strips the receipt. Conflicting ranges or expected lengths fail, while
  unrelated provider operations retain their own `max_bytes` semantics.
  The stalled-body fixture observes the connection end
  after the deadline and a distinct next read succeeding; the existing bridge
  test preserves response association after caller cancellation.
- The candidate preserves reviewed onboarding, window/selection and
  conditional-save source checks alongside signed catalog metadata and bounded
  reads. Combined installed acceptance remains open.
- Runtime preparation connects the inventory to Content and typed catalog
  invocation, with native package identity and capacity verification. Source
  receipts cover the accepted final composite; intermediate whole-server trees
  were not tested separately. Analyser's latest
  regular run passes 25 tests, with the explicit process prerequisite ignored
  in that run and exercised separately. Registry, full-charge and aggregate-budget
  preflight pass; `Reserved` carries the full charge and exact-CID aliases remain
  single-charged. The seeded-cache 16 MiB
  lower-layer real-Kubo/native process test transfers the complete object index and ranged files, checks
  exact source bytes and digests, and reproduces the full package CID. Normal
  CLI import and streamed hashing agree, with unchanged backend allocation and
  successful fixture cleanup. Memory and disk observations are qualified
  samples and high-water marks; cold-network and Qwen proof remain open.
- Typed local Registry readiness/hash/capacity methods reject generic raw, provider-plane
  and Carrier calls before transmission. Readiness probes the bounded pinned
  version after the existing Kubo lifecycle runs. Descriptors are checked before
  effects and responses are sanitized. The hash helper is production code on
  supported platforms; ordinary operations retain their behavior. Current
  parent verification passes native 43 tests (one explicit prerequisite test
  ignored), the separately invoked real-Kubo process test and Registry 46/46.
  Earlier Content metadata 2/2, bounded reads 2/2 and content-fetch 11/11
  remain verified. Workspace/Chain/native formatting, Home/public-copy entropy
  and diff checks pass. The parent's final strict native Clippy all-targets
  rerun also passes.
- Private capacity observation checks the ready backend's actual repository
  and same-volume datastores, validates bounded numeric facts and preserves
  the 10% free-space floor. Backend directories allow current-owner `0755`
  while rejecting special or group/world write bits; staging remains `0700`.
  The real-Kubo process proof passes with repo mode `0755`, 264 calls and
  16,777,382 transferred bytes, with cleanup, child reap and EOF confirmed.
  This observation does not reserve capacity or establish peak usage.
- Separately invoked 1 MiB and 8 MiB process tests passed through the production preparation
  owner, real Content, Registry, native provider bridge and isolated offline
  Kubo. Fresh Use admitted 1,049,332 payload bytes plus a 717-byte index in
  426 ms, with 22 Content reads and 48 provider requests. Reopened-owner CID
  reuse made zero Content reads. Fixture cleanup and both child reaps passed.
  The 8 MiB run admitted 8,389,356 payload bytes plus a 717-byte index in
  2.066 seconds, with 133 Content reads and 270 provider requests. Reuse again
  made zero Content reads. Backend allocated bytes rose from 8,470,528 to
  8,474,624; sampled staging/admitted allocation was 8,605,696 bytes and the
  full reservation was 33,753,284 bytes. Cleanup and both child reaps passed.
  Both runs used signed synthetic packages seeded in offline local cache.
  Continuous peak-capacity and scaled cold-delivery proof remain open.
  One inventory worker retains accounting through drain and
  exact admission reconciliation. Shared Use/Keep/selection
  UI and cold exact-Qwen acceptance follow. Installed Homes remain unchanged.
- Runtime startup composes admitted content with configured
  operator offers after native IPFS registration. It rechecks the current signed
  catalog, complete stored closure, package CID and installed engine receipt.
  The offer ID binds the package, entrypoint/weights and engine receipt; the
  existing model provider includes that ID and its execution settings in the
  run binding. Its Runtime-owned profile is limited to the verified
  Darwin-arm64 engine settings. Startup and admission share one private config
  composer with canonical base and journal paths. Additive Init refresh keeps
  the same provider process, Registry slot and journal, with exact existing
  operator offers. The serialized coordinator checks and applies the config;
  active workers, cached engines and unresolved runs block additions. Exact
  replay is idempotent. Unknown outcomes retain their bindings through expiry
  and restart. Busy activation keeps the admitted artifact for retry without
  another transfer; admission and activation do not grant inference authority.
  Analyser's Rust 1.91 locked/offline checks with warnings denied pass 186 native
  library tests, 49 server model tests, 18 binary model-provider tests and 47
  Registry tests. The explicit same-process Init/refresh test passes in 0.17
  seconds with synthetic content and a fake engine; it does not run inference.
  The native suite leaves one real-Qwen prerequisite ignored, and the server
  suite leaves three explicit process prerequisites ignored. Earlier separate
  process receipts remain distinct. Formatting, Home/public-copy entropy and
  diff checks pass. Shared UI, retention eviction, scaled resource proof and
  cold exact-Qwen/installed acceptance remain open.
- Large-model publication needs a bounded operator/provider bootstrap path or a
  separately verified publisher repair. The current generic directory publisher
  reads whole files and builds a base64 JSON array. Cold-proof capacity must
  cover preparation reservation and any additional publisher backend copy;
  existing verified model files and the 10% free-space floor remain protected.

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

`origin/upstream/0.7.1-dev@c511b133` includes Irzhy's deployed Base 8453 probe
evidence from `90bbe15b`, already present in this branch:

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
  defines the standing cross-branch isolated-execution contract, not proof that
  every first-party app is already a Component.
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

- Registration source tests verify one-use ceremony binding to the exact RP
  and origin, and expected enrollment denials return HTTP 403. Credential saves
  use a stable file lock, encrypted-snapshot conflict checks and atomic
  replacement. Unix paths use directory descriptors and no-follow checks;
  fresh files are owner-only. Key creation shares the identity lock. Failed
  writes reload durable facts; post-replacement failure has indeterminate
  durability. Auth-state locking uses the standard cross-process file API.
  The non-Unix identity adapter preserves existing path/ACL behavior without
  claiming Unix path protection or directory durability. Windows acceptance
  remains open. Credential and principal writes have separate commit boundaries.
  The existing auth state now stores the owner operation before credential
  effects and reconciles its exact verified candidate after interruption.
  Fresh registration continues to reject existing credentials.
- Both registration routes share the durable first-owner operation. Verified
  loopback requests retain onboarding; public HTTPS requires a one-use local
  operator token bound to the exact origin, RP, Runtime and expiry. Runtime
  stores only its digest. The same claimant can resume verified enrollment
  after response loss or reload, preserving the saved name and one terminal
  grant. Source tests cover both routes, competing claimants, persistence
  boundaries, expired grants, counters and revoked credentials. Home's reload
  fixture passes. Installed operator-admitted HTTPS and recovery proof remain open.
- A fresh isolated macOS owner Home has a strict-start receipt for `8e6d298d`,
  tree `c7a24314`, and serves Home with HTTP 200. Its verified artifact set
  contains 132,141,405 bytes; existing identity, configuration and user data
  were excluded. Before enrollment, an unarmed public origin returned HTTP 403
  and malformed completion returned HTTP 422. Those checks preserved AuthState
  bytes, with registration, pending owner setup and guest registration all false.
  The user then completed Touch ID enrollment. Read-only installed records show
  one Admin principal, one existing user root, one owner-enrolled event, one
  consumed owner operation and one matching active terminal grant. An
  existing-install restart preserved owner, grant,
  credential, user-file and artifact hashes and returned HTTP 200. The upgrade
  reported zero roots and objects; it created no device-only protection. The
  old gateway and its direct providers exited. A healthy Home-managed service
  remains intentionally available with matching binary, policy and dependency
  fingerprints under the existing reuse contract. This is gateway cleanup,
  not an all-descendants-reaped claim. After that restart, the user saved a
  Recovery Kit and created a Profile. The exported bundle explicitly omitted
  People identity and preceded the Profile by 15 seconds. Root recovery is
  configured; that downloaded kit does not cover the later Profile.
  The owner installation now binds `abefc7ae`, tree `81d89d5a`. Runtime and
  object-provider built and installed hashes match. The five changed
  Home/System/People assets match source, installed, served and manifest hashes.
  Strict restart returned HTTP 200. Home reload replaced the previous managed
  service through
  the existing binary-fingerprint check; its old process tree exited.
  Owner/protection state, encrypted Profile, downloaded kit and private config
  match preflight. The Home browser-state file changed after the audit reload;
  semantic equality of that file is unverified. The model installation remains
  unchanged. Installed Home shows one Save Recovery Kit action, System shows
  Profile needs backup and focuses Save complete Recovery Kit, and People
  retains the existing Profile. Updated-kit export and Touch ID remain with
  the user. Installed Chat opening and operator-admitted HTTPS acceptance stay
  open; session lifetime stays subject to Runtime policy.
- Same-principal multi-passkey linking is separate follow-up work. Current
  auth records store role, root and name per proof binding, and counts count
  binding records. Canonical account ownership with separately revocable
  proofs and unique-principal counting requires an explicit existing-state
  migration/recovery review before linking. This is a source prerequisite,
  not a demonstrated live defect for current one-passkey accounts.
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

- Browser is included as a bounded Runtime Browser, not as a fully reliable
  general-purpose Browser claim.
- Browser launch, TURN/media-relay connection, Runtime-mediated traffic, exact
  terminal close, and zero-residue behavior require fresh target evidence for
  the exact integrated commit. Human-visible video, input, scrolling, and audio
  remain manual proof gates.
- Deterministic proof confirmed `window.ethereum`, one EIP-6963 provider,
  `isElastOS=true`, `isMetaMask=true`, the Runtime Wallet binding, chain `0x14`,
  and exactly one `eth_requestAccounts` handoff producing one pending Wallet
  account-access approval.
- One failed Browser restart followed by a successful open, lost `ela.city`
  login state across restart, and slow performance remain explicit follow-ups.
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
  matching manual UX evidence; source inclusion does not waive that gate.

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

## Documents Working-Copy Truth

- Runtime requires a read revision for working-copy saves and compares the
  principal, document, path, title and body under the existing principal-root
  mutation lock. Publication completion retains current local edits. The
  metadata and body remain separate files; an incomplete write reports an
  uncertain outcome and requires a fresh read before recovery.
- The editor draft preserves an observed created ID and revision before saving
  its body. An unknown create result keeps the draft and pauses automatic
  retries. The existing dialog offers checking Documents or explicitly creating
  a copy with a duplicate warning. Known-document conflicts pause autosave and
  use an exact read before explicit retry. Late results retain newer selections
  and edits.
- Parent Rust proof passes 21 Documents tests with one external-IPFS test
  ignored, nine gateway Documents tests and the shared conditional-write/GBA
  regressions. The 13 conflict and nine close-handshake Node tests pass, as do
  behavior and Home/public-copy entropy checks. Parent Brave proof passes at
  1280px and 390px, including visible Save in two pages, retained stored winner,
  losing draft and paused autosave. Workspace/Chain formatting and diff checks
  pass; fixture processes are cleaned. Installed Homes remain unchanged and
  installed conflict/recovery acceptance remains open.

## GBA Capsule Truth

- `gba-emulator` is a conditional viewer capsule, not an always-on native
  provider. It carries one browser-targeted mGBA JS/WASM engine and loads that
  engine only after Runtime supplies compatible uCity or Library `.gba`
  content.
- ROM and save bytes cross authenticated Runtime viewer routes. Save state is
  scoped to the launch principal; the engine has no Runtime WASI adapter,
  preopens, environment, socket, FIFO, or direct-network authority.
- The current save-conflict source requires an exact read revision or confirmed
  absence on viewer-storage writes. Runtime compares under the existing
  principal-root mutation lock and retains protected storage. GBA serializes
  saves per game and keeps live progress when a write conflicts or its result
  is uncertain. Check saved data reconciles the exact failed save; loading a
  different stored version requires explicit discard confirmation. Node18 and
  parent conditional-write3/storage-route1/GBA8 tests pass. Parent opaque-frame
  Brave proof covers ETag/CAS, changing pixels, trusted input, nonzero audio,
  save/state reload and fixture cleanup. Installed conflict/reconciliation and
  manual acceptance remain open.
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
- An installation without `providers/model-provider/config.json` reports an
  honest zero-offer state. The latest isolated macOS source Home configures one
  local Qwen3.5-9B Q4_K_M offer. The provider can also call OpenAI-compatible
  Chat Completions and Responses backends. Remote model service publication
  remains open.
- The grouped hosted Chat Completions evidence slice is at commit `fcb8fc5e`,
  tree `1e95de06`. Each OpenAI-compatible offer requires bounded public
  disclosure for the provider label, pinned requested selector, privacy and
  terms references, provider-enforced single dispatch without retry, and the
  operator assertion that upstream routing fallback is disabled. The API key,
  endpoint, headers, and topology stay private. Local Qwen offer and output
  JSON stay unchanged.
- Home Agent source keeps cancellation pending until Runtime supplies an outcome.
  Transport uncertainty with a known `providerRunId` offers Check status through
  `runs_get/events`; navigation detaches without cancellation or a new dispatch.
  Runtime-confirmed unknown settlement retains its completion time and permits a
  new turn. All twenty-one focused cancellation tests pass, covering denial,
  unknown settlement, completion races, stale polls, late acceptance and
  saved-workspace recovery. Home Agent keeps new text and attachments in the
  composer while the prior run remains unresolved. A lost or malformed create
  response retains a bounded request identity and an unknown outcome in the
  existing turn. HTTP 400 can arise from response projection after dispatch,
  so post-send errors do not prove refusal. Without a run ID, Home Agent blocks
  implicit resend and offers New chat rather than replay or Check status.
  The existing scheduled workspace save preserves this state once written;
  survival before that save completes is unproved. The composer clears copied text
  after Agent accepts it into the conversation or queue. An already-unresolved
  turn preserves unsent drafts; Runtime create acceptance is a separate tracked
  outcome. The Home Agent shell and cancellation tests are included in the CI
  source step; local checks pass. The workbook's MODEL-02 source note records
  this coverage while preserving its installed verdict. Installed interaction
  acceptance and remote CI proof remain open.
- The isolated installed Home reported a missing saved run as journal corruption;
  its exact deletion cause is unproved. Provider source returns `run_not_found`
  for a missing requested record in an intact private journal directory, including
  reads after retention cleanup and restart. Unsafe directories and corrupt records
  remain integrity failures. Home Agent source gives a New chat instruction while
  retaining the old run identity and unknown outcome. Installed recovery proof
  remains open.
- The provider-internal `open_ai_responses_text` source adapter and hosted
  cancellation correction are at commit `0d768415`, tree `96d45941`.
  The adapter posts only `model`,
  `input`, `stream: true`, `store: false`, and policy-derived
  `max_output_tokens`. It accepts text only from
  `response.output_text.delta`. Only an authoritative `response.completed`
  event with response status `completed` succeeds. The `response.failed`,
  `response.incomplete`, and `error` events produce bounded generic failures.
  Local Qwen request format stays unchanged. Cancellation semantics are recorded
  below.
- The provider stores hosted backend evidence once on the terminal event and
  replays it through `runs_get`. It reports the resolved model, token usage,
  and non-negative backend cost when the Chat Completions backend supplies
  valid facts. The Responses adapter reports valid model and token usage and
  keeps cost unknown. Other cases report explicit unknown facts. This is
  backend-reported evidence, not verified billing. Both hosted adapters reuse
  the existing worker, journal, cancellation, restart and replay, byte, time,
  and event limits, with one provider dispatch and zero provider retries.
  Hosted and managed-engine cancellation after possible dispatch records
  `settlement_unknown`: an HTTP stream close does not confirm backend stop.
  Deterministic fixtures keep backend work active after cancellation, then prove
  one terminal result, replay and restart persistence, and zero redispatch.
  Local completion and confirmed backend cancellation retain their terminal
  outcomes. Installed cancellation and backend-stop proof remain open.
  Deterministic focused fixtures prove the source adapters. The current
  candidate has passing formatting, Home entropy, public-copy entropy, and diff
  checks; full-suite and target proof remain separate evidence.
- A real hosted route still needs acceptance proof. Route-specific evidence
  must prove the fallback assertion, exact provider and model resolution,
  usage and cost behavior, one provider dispatch, zero provider retries,
  redaction, cancel and restart behavior, and cleanup for both the Chat
  Completions and Responses paths.
- The current source does not integrate the Codex SDK. Codex remains a later
  agent-execution adapter behind typed agent operations and explicit
  filesystem, network, tool, and approval grants. It is not a model offer.
- The current `components.json` bootstrap pins the Mac evaluation artifacts to
  immutable upstream publisher revisions and SHA-256 values: Qwen3.5-9B Q4_K_M
  is the stable candidate, PrismML Bonsai 8B Q1_0 is experimental, and llama.cpp
  `b10516` supplies the macOS arm64 engine bundle. The private operator offer
  selects the canonical installed path and digest. The current closeout must
  replace this setup-only product dependency with CID-addressed selection and
  Runtime preparation for that one Qwen package. Existing engine verification
  remains required on a fresh installation. Real catalog publisher trust,
  signed closure identity and availability deployment still need evidence;
  fixture keys and CIDs do not establish production readiness.
- The pinned Mac evaluation ran both candidates through llama.cpp. Bonsai
  passed and was lighter and faster in the three-prompt comparison. Qwen passed
  with thinking disabled and remains the stable-quality candidate. The local
  HTTP source fixture also proves macOS arm64 selection, checksum rejection,
  interrupted-download cleanup, and idempotent verified reuse. The Jetson CUDA
  source-build behavior is unchanged and reads the llama.cpp source revision
  from `components.json`; Jetson execution remains unproved.
- `model-provider` now accepts the Runtime Init envelope fields
  `base_path`, `allowed_paths`, `read_only`, `encryption_key`, and `extra`
  without weakening strict unknown-field handling. The zero-offer stdio Init
  test passes with the Runtime envelope in source tests.
- The local llama.cpp adapter keeps one engine manager behind the existing
  provider run journal. Init validates canonical operator-admitted paths and
  secure file metadata without hashing the model. Each actual child start or
  restart streams the configured engine and model hashes, starts at most one
  private loopback child per configured offer, and accepts readiness only when
  `/v1/models` returns its unpredictable alias. Source tests cover concurrent
  reuse, fresh bounded alias verification before reuse, one restart for a live
  but unhealthy child, failed-replacement cleanup, streaming, cancellation,
  crash restart, bounded health failure, graceful shutdown with bounded forced
  reap, manager Drop cleanup, idempotent shutdown, repeated-Init rejection, and
  public and journal redaction. Source process tests prove provider-loss guard
  cleanup after a provider hard kill. The installed direct and Runtime proofs
  also observed zero owned engine or guard residue after their shutdown and
  restart cases.
- Installed artifact parity is proved for candidate `8e6d298d`, tree `c7a24314`,
  in the existing isolated macOS model Home. Five Home Agent scripts carry the
  lost-create-acceptance correction. All thirty installed binaries retain their
  prior hashes; model-provider includes the earlier missing-run correction.
  Runtime and the other providers retain their verified `e496fe06`,
  tree `426a87d0`, build provenance. Model installed-provider verification passes;
  fourteen other provider binaries retain their captured hashes. Built and
  installed artifacts match at
  Runtime SHA-256
  `087c3d5d884fdeb519b71cbdc6d8952332222e8788e118cf9c6ede3bdd6f28cd` and
  model-provider SHA-256
  `af51cf994edf69618b17338650fe10882f2cb09fc6dadf7b18f9456c7d8be70c`.
  The five changed scripts match source, installation, manifest and served bytes.
  Strict restart and receipt/HTTP 200 checks passed on 2026-09-07.
- During the Agent update, device identity/TLS, private configuration, model
  weights, migration-backup files and all three protected user objects
  retained their captured hashes, sizes, modes and ownership. Separate passkey
  material, the downloaded Recovery Kit and live journals remain outside this
  byte-parity claim. Principal, proof, role and protected-root records match the
  pre-update semantic snapshot; sessions and audit records are volatile.
- On installed `6c6c2fab`, the user and an independent accessibility
  read observed a complete Qwen reply; the UI reported 14.1 seconds and 55
  tokens. Its journal record was no longer present at metadata inspection.
  Stop on a separate explicit run produced one dispatched journal event and
  one terminal `settlement_unknown` event. A later explicit submission completed
  in 111.2 seconds, within the 120-second limit, with 4,809 output characters,
  one terminal output event and no error. The journals support those two
  distinct run outcomes. Honest unknown settlement is an allowed cancellation
  outcome; actual backend stop was unobserved. A subsequent restart reaped the
  owned provider, engine and guard processes and pruned an already-expired
  terminal record. Unexpired result retention, installed draft preservation,
  lost-create-acceptance interaction and visual reload recovery remain open.
- The reviewed receipt convergence, end-to-end deadline, and hosted URL
  redaction corrections are complete in source and have fresh installed
  artifact parity proof.
- Assistant is a standalone first-party capsule with its own protected
  workspace. Its tested Chat, Build, and Studio behavior uses typed model
  offers and runs. Transcript copy goes only through the trusted Home
  Clipboard path.
- Home Agent is the Home-owned Agent face. Home GUI owns the Shelf transition
  to its composer, Agent Space, and `launchHomeTarget`. `home-agent` owns
  sessions, transcript, composer, and settings. Runtime owns the protected,
  revisioned workspace and model-proxy binding.
- Home Agent uses the typed `offers_list`, `runs_create`, `runs_events`, and
  `runs_cancel` operations. Its Home message contract is pinned. The source has
  one Shelf and one Home-owned Agent face, and the Agent room stays in the
  Space ring. Fixes `450db538` and `900d7e5c` keep activation, message routing,
  and saved layout upgrades on that canonical path without a duplicate generic
  window.
- Runtime accepts one opaque, bounded workspace envelope for this local
  contract. Typed document schemas remain future work when a cross-authority
  operation needs them.
- Assistant model messages render a self-contained safe markdown subset with
  escaped HTML, inert links, headings/lists/blockquotes/tables, fenced and
  inline code, and inline/display math through vendored KaTeX 0.18.3. Focused
  source proof lives in `scripts/assistant-shell-smoke.mjs`. The Home audit
  records observed UI behavior separately. Advanced standalone Assistant
  workflow acceptance remains open.
- Home Agent is the canonical Home-integrated Agent surface. Standalone
  Assistant remains installed while the product inventories and tests its
  distinct working behavior. It can remain as an explicitly scoped optional
  app, or its useful behavior can move before removal. Future Home Agent tools,
  Library reads, web search, Studio, Usage, and sampling controls require their
  typed Runtime operations first. Real hosted route acceptance, service
  publication and grants, and Jetson/Carrier proof remain open.

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
  name editing preserves unfocused drafts across refresh. Create enrollment
  initializes the Profile with the confirmed initial name.
- Declared content icons stay capsule-owned and serve only manifest-declared
  icon variants. Nested content entrypoints resolve icons from their matching
  serving root, and declared icon requests reject ROM bytes, traversal, and
  symlinked targets.
- Fresh desktop placement seeds only visible targets on first run. Saved hidden
  target positions remain intact after later reloads.
- The current onboarding backend binds Create or Recover
  intent to the passkey ceremony. Create records the explicit initial public
  name; Recover skips initial Profile creation. Source tests verify exact owner
  and guest retries, including restart after verified enrollment. Runtime
  retains authority over grants, guest policy and durable consent.
- An actual HTTP Recover enrollment followed by full import preserves the
  original Profile through the existing verified root and Profile/Wallet
  restoration paths. Successful recovered-root reassignment retires the
  previous initial Create-name consent; rejected reassignment preserves it.
  The full-recovery tests retain the existing partial Wallet recovery outcomes.
- Home source offers Create account and Recover account through native radio
  controls. Create passes the initial Profile name with the enrollment intent;
  Recover opens System's existing kit import control. Source fixtures verify
  exact retry after a lost completion response and preserve the current account
  on signed-in reload. System export waits for a ready Profile. Save and Later
  remain separate from enrollment, and files need a separate backup.
  Missing-Profile Recover and later sign-in open System Security/import from
  typed readiness. Home summary sync keeps the import form focused; explicit
  reminders and Chat guide the same recovery step. Unknown readiness checks
  System. Composed source tests cover cancellation, invalid import, repeated
  refresh, sign-in without a stored Recover flag, and ready-Profile Save/Later.
  Installed onboarding acceptance remains open.
- The current onboarding source passes all 70 auth gateway tests and all 15
  full-recovery tests on the same binary, plus strict server/identity Clippy
  for lib/tests under Rust 1.91.0 and `RUSTFLAGS=-D warnings`. Recovery passes
  under default umask; its fresh-machine fixture explicitly creates the same
  owner-only `0700` data root required by installation.
  Workspace/Chain formatting, Home/public-copy entropy and UI Node smokes pass.
  An isolated Brave fixture passes six HTTPS
  Create/Recover views at 1280x900, 390x844 and 320x568 using the actual enrollment
  markup, styles and auth modules. It checks the named dialog, native radio
  keys, 44px label targets, Tab and scroll access to inputs and the primary
  button, viewport bounds and zero page errors. The final UI change adds only
  friendly rejected-name copy and passes the Node smoke. Installed passkey/gateway
  acceptance remains open. Existing identities and installed recovery material
  are unchanged.
- The public registration decoder rejects missing/null intent and explicit
  legacy name fields, including null, before registration effects. Runtime
  checks Create names with the existing pure validators before owner-claim
  preparation and returns 422 for invalid input. Operational errors retain
  their existing handling. Original owner-first and optional internal
  dispatch remain in place, with native identity contracts unchanged.
- Source tests verify correction after definitive pre-begin name rejection,
  C1 control rejection and explicit Resume after a lost credential prompt.
  Uncertain replies retain the original intent and deadline; cached responses
  retry exact completion. Signed-in Home preserves its current account.
  Installed passkey, recovery and Save/Later proof remains open.
- Installed owner and Qwen Homes remain on `abefc7ae` and `8e6d298d`,
  respectively. Their guidance, focus and artifact evidence applies to those
  trees. The current onboarding and window source changes await combined
  installed acceptance.
- Optional single/multiple/hybrid window metadata passes through the existing
  manifest, Runtime catalog and launch path. The 41-manifest source inventory
  has ten single, six hybrid, one multiple, two content, two shells, two owned
  Home/Agent surfaces and eighteen providers. System, People, Inbox, Wallet,
  Assistant, Marketplace, Services, MetaMask, UniSat and WalletConnect declare
  single policy. Reuse preserves each current frame, token and draft, while
  explicit deep links and Recovery Kit Save bind to its current document.
  Browser, Library, Chat, Documents, Archive and GBA declare hybrid policy.
  Ordinary no-query Open reuses the window; explicit New Window and selected
  launches keep independent frames. Library pickers remain separate. Capsule
  File menus keep their commands and one Home New Window command where policy
  permits it. Absent policy preserves prior behavior. Elacity Player declares
  multiple policy for independent selected mint sessions and suppresses generic
  blank New Window. The owned Home Agent remains separate from Assistant.
- Documents, Library, Archive, GBA and Chat use one Home presentation-hint client
  through the existing host and GUI to the exact window/session record.
  Documents reports verified saved or Library-file selection, Library its
  verified folder, Archive its current stat/list selection, GBA its accepted
  engine switch, and Chat its guarded verified direct or explicit shared/default
  selection. Per-frame challenges, current-document nonces and sequences reject
  stale reports after reload, replacement or retirement; Home retains exact
  source/origin/token checks. Explicit new/clear removes the old selector;
  loading and unchanged public share mode retain startup state. Literal Library
  names keep spaces, percent, question and hash characters. Persisted hints
  contain settled selectors; Runtime reauthorizes fresh restored launches.
  Drafts, effects, one-shot actions and picker state stay outside hints.
  Browser retains its Runtime-owned instance and Player its selected mint.
- Home binds each Library picker to its exact opener, document and request.
  Library closes after receiver acceptance. Browser carries the chooser ID
  through the existing Runtime upload path; the guest control service checks it
  across asynchronous CDP operations and preserves a replacement chooser.
  Archive separates exact Home picker delivery from parent-owned menus. Chat
  adds current-document/request and conversation-selection checks to its
  attachment path. The matching control service is bundled into the guest
  initrd by `build-browser-vm-rootfs.sh`; installed chooser proof must include
  that artifact.
- Final-composite source and component checks are recorded below. These
  receipts apply to the combined source, rather than isolated intermediate
  commit builds. Installed navigation, picker, public share-reader and manual
  acceptance remain open in [TASKS.md](TASKS.md).

| Surface | Current source/component evidence |
| --- | --- |
| Window metadata | Common 92, window projection 1, Home launch 5, browser discovery 20 and catalog 23 tests pass; inventory covers all 41 manifests. |
| Reuse and selection | Selection 40 and Recovery Save 32 pass, including deep links, draft preservation, launch races, stale-document rejection and independent restores. System's real-Brave fixture passes 4/4. |
| Current navigation | Home regression proves verified A-to-B selection restores B alongside independent C with fresh tokens. The grouped consumer regression covers real Library/Archive/GBA/Chat selectors, relay and restore, failed/stale loads, picker exclusion and GBA save conflicts. |
| Documents and GBA saves | Documents save 13/close 9 and GBA save 18 pass. Real-Brave Documents layout/conflict and GBA opaque-frame proof pass with the shared module; storage and two-file failure limits remain in their sections above. |
| Picker delivery | Home/Library/Archive 18 and Browser/CDP 16 pass. The real-Brave Library menu fixture passes exact Archive request/document/acknowledgement delivery, visible standalone status and existing file/extract assertions. Its Browser chooser receiver is a fixture. |
| Generated Chat | Native tests pass 30/30. Regenerated JS/WASM pass all nine configured real-Brave scenarios: bootstrap/reload/reopen, failures, stale switches and 375/640/1280 layouts. Installed artifact parity remains open. |
| Host fixture lifecycle | Restored-Browser Brave proof records one Home refresh, two open requests, one provider effect, one cleanup effect and zero remaining pages/VMs. Fixture contracts pass 10/10, including error preservation, Shutdown and unique generations. Home bridge, Home/public-copy/Browser entropy, syntax, diff and applicable formatting checks pass. |

- Hosted target fixtures require the existing Runtime proxy, use one adapter
  process and generation, pass returned cleanup through that process, and
  request Shutdown even after close failure. A binary override requires an
  explicit SHA-256. The full control-service smoke passes Wallet consent,
  signatures and transactions, chooser/upload, navigation, paste and nested
  hosted-display launch, then fails at `engine_close_indeterminate`: its v1
  close response lacks the required generation-bound v2 terminal receipt with
  observed-absence proof. Independent reproduction confirms the same result
  and exited fixture processes. Fake signaling proves fixture behavior;
  hosted product completion and installed Browser media remain open.
  Browser repairs belong to a separate session. This remains external
  acceptance evidence for the model/onboarding closeout.
- Home fixtures use the current auth DOM and Runtime summary/presence shapes.
  Restored-lifecycle testing stops on captured bootstrap errors and accepts an
  optional browser executable; System keeps its boot/error assertions without
  temporary trace injection. Home entropy skips configured `target-build`
  output alongside `target`. Source-document link and read failures still fail
  the gate.

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
- Branch-override public smokes require a staged or published release-compatible
  manifest with the current `home` profile and checksummed artifacts.
- Source/local Carrier setup proof stays in `scripts/local-carrier-setup-smoke.sh`.
- Public install proof for an integrated candidate requires a staged or
  published release-compatible manifest with the current `home` profile and
  checksummed artifacts.
- Set `ELASTOS_PUBLIC_INSTALL_FORCE_RELAY_ONLY=1` only when the publisher relay
  path itself is under review.
- Integrated installed-path proof waits for one reviewed source tree and
  matching stable installation receipts on each target role.

## Open Blockers

- Product Browser completion is not claimed.
- Manual installed-device checks on Mac and Linux/aarch64 targets are still
  required before release handoff.
