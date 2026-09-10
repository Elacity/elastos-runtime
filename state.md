# State

Last updated: 2026-09-10 UTC

This file records public-safe current truth for released 0.7.0 and active
development work. Private operator paths, credentials, target identities, and
volatile proof logs remain outside the repository.

## Current execution snapshot — 2026-09-10

The first execution pilot is the public storefront and `/home/` on the seed,
using reviewed source from the active local work. Mission R2 corrects an initial
private-Mac interpretation. [TASKS.md](TASKS.md) is the current queue; the linked
approved plan retains full release acceptance.

- Freshly fetched public main is `8ac18bec` (`v0.7.0`); development is
  `6c61c990b9a1d0f2c1b00ee6f5f091d3354cafac`, tree
  `184b7a52f93403956817cdb1a8a15781b76bb4e2`. PR58, PR52 and PR54 are merged.
- The user chose that development base for `feat/0.7.1-website-execution`.
  The branch began at the same commit/tree, zero ahead and zero behind. The
  website, route and restart-helper source is reviewed in `cdcb7902`, tree
  `564c5dddeebbd4f963182c05a240a157f6d1a775`, one commit after that fetched
  development ref. This is the scoped C1 source; journey donor intake follows
  under C2–C5. Publication and seed deployment remain pending.
- Follow-up source commits are `92d1a31f` for fresh Linux setup/restart,
  `e3478890` for installer platform/process handling and `c97e1c67` for dated
  hosted evidence. Their file hashes match the reviewed test receipts.
- Local integration donor `6972e165` is clean, tree `8b32a72a`, 51 ahead and
  zero behind dev. Browser donor `50196355`, tree `f650c4dd`, is 164 ahead
  and two behind dev, with seven preserved dirty files at the review snapshot.
  These local lines are absent from the fetched origin branches.
- Recorded Home installation remains `94ed0dc6`; diagnostic source `05ee824d`
  was built from `e9b9baf7` and is uninstalled. Browser recorded Mac Runtime
  is `690170bc`, Linux consumer/Exit is `b8c78d79`, and `de0a299e` remains
  a built candidate. This task has not yet repeated their installed proof.
- Public reads show root 200, `/home/` 404 and `/apps/home/` 200. Published
  installer metadata remains `0.1.2`, with Linux x86_64 and aarch64 entries.
  Served website and Home source identities remain unproved. The site must
  distinguish download, source and hosted Runtime facts.
- PR60 `617796a9` and PR59 `25ab205e` remain open; draft PR62 `decab1f5`
  contains planned follow-up work. PR63 `424acd3d` supplies verified framing
  under D6; the J1 replacement owns closure once visible.
- The source pilot passes 31 focused Rust cases, four manifest follow-up cases,
  Home recovery/sign-out checks, 20 website tests, six website checks and the
  required entropy/format gates. Four rendered desktop/mobile cases cover
  metadata success, absence and disagreement, keyboard use and layout.
  Independent review resolved moving-source instructions, stale installer proof
  and installed-app identity. Full Mac and Linux restart smokes also pass;
  committed objects match the corresponding source and workbook receipts.
  The three new journey rows retain pending target acceptance.
- The installer bootstrap passes 18 offline tests with stock Bash 3.2,
  including signed publisher envelopes and RFC 8032 vectors. Independent
  comparison with a second Ed25519 implementation accepted 128 valid signatures
  and rejected 256 modified inputs. Canonical Mac data/process handling and
  complete candidate installation remain open at that bootstrap snapshot.
  The subsequent platform slice passes 38 tests covering canonical Mac/Linux
  data paths and process ownership. It preserves another installation sharing
  a binary and retains state when captured children survive shutdown. Independent
  review passed. Real three-platform installation remains open.
- The existing 30-minute monitor is active. Nine independent process cases
  passed review. Its first real observation exposed task permission limits;
  a writable shared state and coordinator reads resolved them in a second real
  observation. The recurring timer subsequently delivered and the monitor wrote
  and verified its observation. Old tasks retain their pause.
- Seed preparation reproduced W2's two Linux prerequisites: an `already_ready`
  upgrade creates no backup directory, and inherited shared-write permissions
  can make the installation unsafe. The repairs pass focused Mac/Linux tests,
  including empty and malformed receipts, inherited permissions and preserved
  existing artifacts. The repaired frozen source `3e27cac8`, tree
  `619657c5fee77bb226d5dedff689f873cbfb93e6`, completes fresh target
  installation and startup using the completed build cache. Built, installed
  and running Runtime SHA-256 is
  `cac06eaef00b95559a5415ddce2ef4c3ba428d9c47deb34eb13121e2624e8947`.
  All 25 HTTP checks pass, including website/Home artifact parity, redirects,
  manifest identity and traversal rejection. Private desktop/mobile layout,
  passkey sign-out/sign-in, Recovery Kit download and Profile creation pass.
  The public target package is prepared. Public deployment and full journey
  acceptance remain open.
- Hosted website evidence can now identify a dated operator check of source,
  tree and artifact hashes on the exact serving origin. Its source template
  stays unverified until public proof. The follow-up passes 22 website tests,
  six truth checks and four rendered receipt cases; it keeps install actions
  unavailable. This dated record makes no claim of continuous verification.

- Subsequent delivery source `a0743a17` binds the exact release envelope bytes
  to a digest in the signed head. Independent review, 42 installer tests and
  ten updater tests pass across both metadata transports. This source is outside
  frozen seed `3e27cac8`. New clients require binding-ready publisher metadata;
  publication and rollback must preserve that ordering.
- Installed browser QA exposed stale driver controls and expired-token cleanup.
  The repaired driver preserves recoverable test credentials, checks the actual
  cleared sandbox and uses the visible returning-user control. Independent
  review accepts the repair. Failed attempts remain in private evidence.
  Ordinary seed UI checks then completed Recovery Kit and Profile creation.
  A subsequent full app smoke stops at its outdated System launcher selector;
  automated shell/app-matrix acceptance remains open in J1/C2.
  The driver now uses the source-defined ElastOS menu and System Settings
  control. Syntax, entropy and format checks pass; target rerun is pending.
- The monitor delivered recurring observations but misapplied a historical stop
  from a prior mission. The original message date and scope established the
  error; the correction was withdrawn and the freshness rule is now explicit.
- The user reported 20% weekly usage before the first public result. Current
  execution is focused on C1 public website/Home delivery. Broader installer
  and journey work stays queued at this boundary.
- The public `v0.7.0` tag is `8ac18bec`; the distinct local `0.7.0` tag is
  `3585f340`. The old updater selects release platform by CPU architecture,
  so its Mac first hop needs an actual baseline and a recorded bridge decision.
  Current tag assets and source-home receipts alone do not establish the
  required ordinary update path.

- The visitor page offers Open Home and installation on the user's hardware.
  One command installs the older public 0.1.2 Linux preview; Apple silicon
  developers have a source-guide link, and 0.7.1 downloads remain in preparation.
  Current content and local source/served parity checks pass. The private seed
  website package now serves this copy revision: all eleven website files and
  the complete staged artifact set pass source/hash checks. Public copy review
  and deployment approval remain open.
- The candidate installer now runs Home setup and opens terminal Home after
  verified bootstrap. It uses the installed binary's absolute path and reads
  interactive input from the terminal. Headless setup prints the launch path;
  install-only mode lets automation own setup and process cleanup. Forty-three
  offline installer checks pass; four optional captured-public-fixture checks
  were skipped. This is source/fixture proof. Actual fresh-device installation
  and browser Home launch remain open.
- The installation guides now explain that the source Home profile already
  includes Documents, Library and the CLI's IPFS backend. Getting started omits
  redundant component setup. The Mac guide selects an explicit development ref,
  loads Rust into the shell and supplies the required collaboration setup mode.
  These documentation corrections preserve the open Content/Carrier acceptance.

- Release publisher manifests now use full OS/CPU platform filenames in local
  exports and ledger reads. A regression reproduced Linux ARM capsule data in
  the Darwin ARM ledger before the repair. The three-platform ledger test and
  all 21 publisher tests pass; shell export checks pass for three host/cross
  combinations. Independent source review found no further affected consumers.
  Native support assets now select the host OS target while microVM guests
  retain Linux targets. ARM Linux preflight now checks the musl artifact used
  by the shell publisher; GNU-only input rejects before publication. Three
  native/guest target cases and all 21 publisher tests pass with independent
  source review. Complete artifact assembly and fresh installation remain
  delivery work.

- The publisher now assembles its served artifact directory before release
  signing, including universal app archives previously omitted from that copy.
  Staged Runtime and manifest hashes must match the release descriptors. The
  component checker also verifies advertised local app, provider metadata and
  capsule file sizes/hashes, and rejects unsafe paths and file types. Five
  regression groups pass, including ten cases through the actual pre-signing
  gate. Independent source review found no concrete issues. These checks are
  wired into CI and `just verify`; remote CI has not run for this local work.
  URL-only dependencies, complete platform-input admission, atomic promotion
  and fresh installation remain separate open delivery checks.

- The low-level publisher now rejects `--dry-run` before side effects and
  directs operators to the existing read-only `elastos publish-release` planner.
  The former shell mode still reached upload/Publisher-write code despite its
  no-write description. The full-script tripwire test reproduced an attempted
  temporary-key allocation, then passed after repair. Six publisher regression
  groups and independent source review pass. Normal publication keeps its
  existing behavior and approval gate.

- App and provider-metadata archives now use Python's standard library, replacing
  GNU-only tar options that failed on macOS. The archive writer preserves app
  layout, executable files and symbolic links, with normalized file metadata.
  Seven publisher regression groups pass. The same fixture on macOS ARM64
  (Python 3.9) and Linux x86_64 (Python 3.12) produces identical compressed
  bytes and extracts with each system's tar. Independent source review passes.
  Archive hashes change from the previous writer; a new candidate must bind
  the new bytes. Full candidate builds and device installation remain open.

- Direct app, native provider and provider-metadata builders now record local
  file descriptors before any capsule upload. Publication attaches real CIDs
  in a separate step. Build and archive failures return through Bash command
  substitutions, and nested app entrypoints get their parent directory before
  copying. Ten regression groups pass on macOS and Linux, including four
  preparation failures that stop before upload. Independent source review and
  required source checks pass. Actual candidate installation remains open.

- The native preparation worker builds from a clean Git revision and tracked
  Cargo lockfiles, then exports local artifacts with an unsigned source/file
  receipt. Admission checks all three platform inputs against the candidate
  source, component template, native OS/CPU, provider contracts and exact file
  inventory. Preparation and admission fixtures pass for all three platforms.
  Actual Mac, Linux x86 and Linux ARM preparation pass from 8587dff6, including managed
  media delivery. All three inputs pass full file verification; shared app and
  provider-metadata archives match across platforms. Linux ioctl and custody
  syscall repairs pass native compilation and focused tests. Publisher input
  staging and its CLI pass offline checks and Linux musl tests. Complete
  publication promotion, final input regeneration and signed fresh installs
  remain open.

- Three standalone Linux Browser helpers now have tracked Cargo lockfiles.
  Their registry versions and checksums match the existing Runtime workspace
  pins. Offline locked dependency resolution passes for all three projects.

- Home CLI delivery now uses one platform-specific capsule archive containing
  its native terminal renderer. Runtime and source-home provisioning use the
  same installed capsule path. Checks reject missing, non-executable, malformed
  or wrong-platform renderers before launch. A real Mac renderer build and
  archive extraction match byte for byte; narrow Rust extraction/launch tests
  and eight preparation fixtures pass. Development fixtures use the installed
  renderer, and the demo reports incompatible older releases before launch.
  Full signed fresh-device acceptance remains open.

- Managed Home now has one media-tools component containing FFmpeg and FFprobe,
  built from pinned FFmpeg 9.0.1 and x264 sources with their source and licenses.
  Setup installs and verifies this archive before private media import. Ordinary
  setup uses the supplied pair; source-home has an explicit developer directory.
  New data roots and archive directories have explicit permissions. Tests cover
  common umasks, real archive extraction/import, existing unsafe paths, pair
  mismatch and source/recipe tampering. Mac tools build with system-only shared
  dependencies. Signed fresh installation and replacement of older imported
  media tools remain open.

- The focused required video repair from donor e3a8c4eb makes media-provider
  accept Runtime's current invocation ABI and normalize FFmpeg DASH segments to
  the existing zero-based contract. Runtime preserves codec configuration boxes
  through protection. Provider and contract regressions pass; an actual generated
  video run through the relocated tools, provider and Runtime validator accepts
  one track, two segments and 128 samples. Full J5 purchase/playback/cleanup and
  the required external review remain open.

The source and installed sections below preserve the earlier evidence snapshot.
Their original identities qualify those results. Current milestone receipts
above take precedence where the work has moved; remaining sections are refreshed
as their journeys are accepted.

## Prior release/source snapshot — 2026-09-03

- A fresh fetch records `origin/main` at `8ac18bec` as the released `v0.7.0`
  source and `origin/upstream/0.7.1-dev` at `c511b133` as the active
  integration line.
- Released `v0.7.0` already carries the coordinated workspace version,
  changelog, manifest bumps, and lock refresh. Installed artifacts report
  `0.7.0` only after the checked publish flow stamps
  `ELASTOS_RELEASE_VERSION`; unstamped source builds report `0.7.0-dev`.
- Local candidate `900d7e5c` has tree `c9a9effe` and is 30 commits ahead of
  `origin/upstream/0.7.1-dev@c511b133`. It contains the reviewed PR52 source at
  `origin/feat/protected-content-installed-provisioning@4d688cc5`, PR54 at
  `origin/feat/home-first-run-seed-0.7.1@2a49ea57`, and the PR55 Home Agent
  source from `origin/feat/home-shelf-assistant-face-0.7.1@923193bb`. PR54 and
  PR55 remain the original feature review slices. The tested candidate still
  needs publication and combined team review before a merge decision.
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
  ancestor. The candidate source passes formatting, alignment, Home and Browser
  entropy, Home shell, People discovery, the 26-case Browser close handshake,
  and Home Agent shell checks. Remote CI remains separate from these local
  checks.
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
  Agent Space. The installation has no configured model offer and reports that
  state directly. This does not prove model inference.
- People reports discovery as unavailable because the isolated Home has no
  collaboration configuration. The pending Wallet approval remained unchanged
  during the journey.
- Protected-content acceptance is blocked on one real three-node custody
  composition, private Chain and RPC configuration, three replicas, funded
  creator and buyer accounts, and installed two-Runtime proof. No mint, buy,
  playback, seed, third-node, or cutover claim follows from this localhost run.
- First-run work remains open. Recovery Kit navigation and readiness are slow
  and indirect. Profile creation should carry the display name into the form
  and guide the exact create action.
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
- The current localhost installation has no
  `providers/model-provider/config.json`, so the provider reports an honest
  zero-offer state. The current provider can call an OpenAI-compatible Chat
  Completions backend. It does not yet own a local engine lifecycle, implement
  a provider-internal OpenAI Responses API adapter, or publish a remote model
  service.
- The current source does not integrate the Codex SDK. Codex remains a later
  agent-execution adapter behind typed agent operations and explicit
  filesystem, network, tool, and approval grants. It is not a model offer.
- `model-provider` now accepts the Runtime Init envelope fields
  `base_path`, `allowed_paths`, `read_only`, `encryption_key`, and `extra`
  without weakening strict unknown-field handling. The zero-offer stdio Init
  test passes with the Runtime envelope in source tests.
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
  records observed UI behavior separately; configured model-run and advanced
  workflow acceptance remain open.
- Home Agent is the canonical Home-integrated Agent surface. Standalone
  Assistant remains installed while the product inventories and tests its
  distinct working behavior. It can remain as an explicitly scoped optional
  app, or its useful behavior can move before removal. Future Home Agent tools,
  Library reads, web search, Studio, Usage, and sampling controls require their
  typed Runtime operations first. The zero-offer Home Agent state is installed
  proof of honest absence, not configured inference.

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
