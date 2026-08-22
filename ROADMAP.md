# ElastOS Runtime roadmap

This roadmap describes future work. See [TASKS.md](TASKS.md) for active work
and [state.md](state.md) for current behavior.

## Mission

Make this repository the trusted local runtime layer for ElastOS:

- execute capsules predictably
- expose one coherent local object model
- route Component effects through ElastOS Bus and web-projection effects
  through narrow, capsule-scoped Runtime adapters
- make local and remote effects follow the same authority model
- keep release, install, update, share, and site flows boring
- give ElastOS a stable default Home without weakening the Runtime boundary

## Non-goals

The broader SmartWeb stack is outside this repository. Blockchain and payment
remain separate layers. Carrier and Boson have broader programs beyond this
repository's Runtime integration. All integrations use explicit boundaries.

This repository owns the Runtime and Home contract. That includes the default
local front door, capsule execution, authority, and the object boundaries that
Home and other host adapters must obey.

## Constraints and order

[PRINCIPLES.md](PRINCIPLES.md) owns the implementation constraints.
[Architecture](docs/ARCHITECTURE.md#elastos-four-quadrants) owns the
responsibility model. This roadmap orders outcomes; it does not redefine either
document.

The numbered outcomes below define the order. This keeps one substrate from
becoming the root of trust. Token economics, public storage markets, broad dDRM
economics, and specialized device products come after principals, capabilities,
packages, availability, and spaces work end to end.

### Planning review gate

Future plans should answer these questions before implementation:

- Does the work strengthen explicit authority and avoid ambient access?
- What is the smallest complete Runtime behavior or user journey?
- Which quadrant changes, and which quadrants do not?
- Does provider-specific behavior stay behind a provider contract?
- What source, artifact, or manual evidence will prove the result?
- Which existing truth surface owns the contract, state, task, and history?

## Near-term direction

### 0. Enforce capsule authority at every Runtime ingress

Components request effects through typed ElastOS Bus resources. Web projections
and MicroVMs use narrow, capsule-scoped Runtime adapters. Every surface enters
Runtime's authority and routing boundary without exposing local or remote
topology. Runtime handles core operations directly and sends provider-backed
effects through the provider registry. Each substrate keeps its documented
lifecycle and cleanup contract. No executable substrate receives guest network,
host process execution, raw transport, direct provider API, or host filesystem
authority. Content capsules carry portable data under a data contract. They do
not execute or request effects.

Provider capsules are narrow exceptions. Their manifests define an upper bound,
not a self-grant. Explicit Runtime system services require registered policy.
Runtime verifies authority before dispatch, and unknown operations fail closed.
Gateways authenticate and route host projections; they do not reimplement
provider policy.

The normative authority rules live in [PRINCIPLES.md](PRINCIPLES.md),
[ESP v0](docs/ESP_V0.md), and the
[Capsule interface contract](docs/CAPSULE_INTERFACE_CONTRACT.md). Capsule
packaging and interface rules live in
[Capsule authoring](docs/CAPSULE_AUTHORING.md) and
[Capsule interface contract](docs/CAPSULE_INTERFACE_CONTRACT.md).
The current Component/Bus v1 path is implemented and conformance-tested, while
shipped first-party UI Apps remain web projections. Keep Bus v1 bounded and do
not claim product adoption until a shipped first-party Component proves it.
Define Bus v2 only for a tested product need such as resident lifecycle,
cancellation, or streams.

### 1. Finish passkey-first authority and recovery

Home should start with passkey-backed principals. The first owner controls local
enrollment policy. Other people create and control their own authenticators,
principal roots, and recovery material. An administrator can operate the
Runtime and revoke access policy, but cannot silently decrypt another person's
root.

Removing a passkey revokes its proof bindings and sessions. It does not delete
the principal root or attach it to a new passkey. Recovery is an explicit,
audited reassignment that verifies the root and its protectors before mutation.
Hosted, loopback, native, and mobile hosts need explicit origin or host-auth
policy; none may gain authority from a trusted header.

Principal-root protection should use random per-principal data keys. Protectors
may include a Recovery Kit, client-side WebAuthn PRF wrapping, DID-backed
recovery, and future quantum-resistant envelopes. Runtime stores wrapped
envelopes and protector metadata. Raw PRF output and recovery secrets never
cross into ordinary Runtime routes.

Agents use their own principals, keys, and short-lived grants. They do not
borrow human cookies or automate a person's passkey. High-risk effects still
require human review through the authority-owning surface.

### 2. Build content availability

Publishing packages an object, retains it under local policy, submits it to
configured availability providers, and records signed receipts. Calculating a
CID alone is not publication.

The capsule-facing contract is `elastos://content/*`. IPFS, Kubo, Carrier,
cluster services, storage markets, and repair workers are provider
implementations under that boundary. Capsules should not know which backend
stored a block or which transport reached a peer.

Availability grows in this order:

- local packaging, verification, pinning, and unpublishing
- peer discovery, replication, and repair through configured providers
- quotas, abuse controls, health reporting, and operator alerts
- storage offers and settlement after receipts and enforcement are credible

Receipts must distinguish local retention from independent remote availability.
They must not turn configured endpoints or test fixtures into a claim of a
production storage network.

Publish, open, share, and repair remain operations on one content object. A
working copy can gain a published identity without becoming a different object
in the UI. Recipients verify it before retaining, mounting, or forking it.

The contract and its limits live in
[Content availability](docs/CONTENT_AVAILABILITY.md).

### 3. Build Runtime-mediated protected content

The target protected-content design uses the same effect path as ordinary
content:

`viewer -> Runtime -> rights provider -> custody providers -> decrypt provider`

Runtime will resolve the content object, check availability, verify rights,
request recipient-encrypted custody contributions, create a scoped decrypt or
render session, and record the result. Each dependency will have a typed
contract and fail closed. Viewers will receive scoped output or a scoped
session, not content keys, custody shares, chain RPC, Wallet authority, storage
APIs, provider routes, network locations, or credentials.

The dependency order for this work is strict:

A. define and review the canonical source contracts and custody crypto
   boundary;
B. define source-only provider protocols and custody-node state without making
   them active product paths;
C. add Wallet-rights, typed Chain evidence, and Runtime-owned durable
   coordination as source-only prerequisites;
D. implement inactive Runtime provider lifecycle, registration, routing, audit,
   and exact identity-bound reconciliation
   (`feat/protected-content-runtime-integration` from rights `43a83e5b`); do not
   continue `feat/protected-content-runtime-coordinator-v1`;
E. PQ-hybrid share wrap, recipient possession, decrypt-session wrap, and the
   Runtime mint journal/2-of-3 provision are on the unpublished integration
   tree (`elastos-xwing-draft06-hkdf-sha256-aes256gcm/v1`, X-Wing draft-06
   confidentiality only). This remains a source-only permissioned draft; the
   current authority signatures remain classical and are not claimed
   quantum-safe. Full PQ authorization remains a pre-activation decision.
   External cryptographic review is still required. The Runtime mint journal
   separates durable 2-of-3 custody provisioning from identity-only verified
   content availability. The private server adapter publishes the fixed
   descriptor/init/indexed-segment directory through existing `elastos://content`,
   reads its signed status receipt, and refetches its generic manifest/files.
   It pins Runtime-selected provider, object, and publisher identities plus
   exact CENC media identity, policy, replica requirement, and freshness before
   buy/open. A separate inactive Runtime test-provider composition covers
   mint -> availability -> buy -> open ->
   init/segment read -> close; it is not the process-backed product proof.
   The decrypt provider generates each operation-scoped recipient key and keeps
   its secret private; the authenticated Profile must authorize that exact
   public key. Separate focused tests now prove the passkey-bound Profile
   signing adapter and Runtime release-operation assembly seams.
   Separate lower-level Runtime lifecycle tests and decrypt-provider process tests prove PQ-hybrid
   reconstruction, CENC media reads, close replay, restart, and old-handle
   absence. The current process-backed inactive proof now uses production
   rights wiring, three independently addressed custody-provider processes, one
   protect-provider process, and the decrypt-provider process. What remains in
   that combined path is the wrong-object plus restart/crash/cleanup matrix,
   and any future live Profile/Wallet/Chain process proof only if explicitly
   required before atomic cutover; and
F. only then atomically replace the provisional DTO/provider surface and prove
   the installed mint -> buy -> open -> play path.

Share wrap on this unpublished tree is PQ-hybrid. It is still not a product
mint path. First minted objects must stay PQ-hybrid; do not add a classical
journey. PR #15 / `feat/dkms-esp-port` is research evidence for
PQ-hybrid envelope crypto, threshold tests, node-local custody, lifecycle
scenarios, CENC/play, and UX shape. Its public `shares[]` metadata, PQ-off
decrypt defaults, raw-CEK/reference operations, old DRM orchestration, direct
TCP/IP topology, and standalone harness must not become the product path.

The published protected-content review line now reaches
`origin/feat/protected-content-custody-provider` (`f7cd6c3d`), which is still
source-only and unregistered. Local descendants add Wallet-rights, a private
Runtime coordinator, typed Chain evidence, and a typed rights evaluator; those
remain unpublished source work. Do not continue
`feat/protected-content-runtime-coordinator-v1`. Local
`feat/protected-content-runtime-integration` continues D without replacing the
provisional `key` or `rights` routes. Rights evaluation invokes existing `chain`
evidence through the Runtime registry. The remaining-work plan is
[Protected-content extraction](docs/PROTECTED_CONTENT_EXTRACTION.md).

Carrier remains transport only throughout that sequence. It carries
Runtime-selected traffic, but it does not define rights authority, custody
policy, or capsule-visible contract meaning.

New protected objects should carry encrypted payload identity, rights policy,
algorithm metadata, provenance, availability receipts, declared viewer
interface, CEK commitment, and object-bound pool/epoch/committee-authorization
identities. Public metadata must not carry custody shares. `CustodyEnvelopeV1`
remains a private ephemeral provisioning bundle; durable custody storage is one
node-sealed share per selected custody node. The first product proof must use
three distinct custody provider identities and state roots for a 2-of-3
committee. First minted objects use PQ-hybrid envelopes. Later cryptographic
successors use versioned envelopes and migration rules; 0.7 does not mint
classical-only objects to migrate later.
A permissioned key service can precede a public network, but production claims
require independent review and operational evidence.

The repository now has a canonical source-only v1 review line in
[Protected-content v1 contracts](docs/PROTECTED_CONTENT_CONTRACTS_V1.md),
[Protected content](docs/PROTECTED_CONTENT.md), and
[Protected-content extraction](docs/PROTECTED_CONTENT_EXTRACTION.md). The current installed/provider
path still uses the older provisional `elastos_common::protected_content` DTOs
plus fail-closed provider stubs. The canonical source line is not product proof
until Runtime, providers, Library, Wallet, Chain, custody, decrypt output, and
viewer are connected in one no-fallback path.

### 4. Add wallet, DID, and node proofs behind Runtime authority

Wallet accounts and DIDs can link proofs or identities to a Runtime principal.
They do not replace or mint it.

Wallet Provider owns keys, linked accounts, signing, proof verification, and
approval execution. Runtime owns launch authority, capability checks,
orchestration, durable effect state, and audit. Connector capsules own only the
browser or device interaction needed to reach an external wallet.

Wallet shows accounts and approval methods. Inbox reviews effects that need a
person. System shows policy and provider health. Apps receive typed results
after approval, never raw wallet or node RPC.

Chain Provider owns typed reads, transaction preparation, broadcast, sync
status, and approved node lifecycle operations. Backend URLs, credentials, and
ports stay hidden from capsules. Price and oracle data also belongs behind a
provider contract with configured sources and auditable receipts.

EID, `did:key`, chain accounts, external wallets, and future global names are
proof or resolver adapters. Each needs a verification policy. No adapter mints
a Runtime principal, session, or privileged capability on its own.

See [Wallet Provider](docs/WALLET_PROVIDER.md) and
[Chain Provider](docs/CHAIN_PROVIDER.md).

### 5. Build Spaces on one rooted object model

Home is the friendly view of the active principal's local object root. Spaces
add mounted local, shared, and provider-backed views without exposing raw host
paths or other principals.

Keep these concepts separate:

- `localhost://` identifies Runtime-authorized local objects and mounted views
- `elastos://` identifies global content and capability resources
- a path is a projection over an object, not its immutable identity
- Public is a placement under a principal root, not automatic publication

Spaces need typed object metadata, signed mutable heads, access policy,
watch and sync behavior, conflict handling, and provider health. Local editing
must remain fast and usable without a remote round trip. Carrier and providers
carry remote synchronization, sharing, and repair.

The namespace contract lives in [Namespaces](docs/NAMESPACES.md). Open syntax
and provider questions stay in [Tasks](TASKS.md).

### 6. Establish signed capsule publication and installation

An installable capsule needs identity for the complete package. A manifest or
entrypoint alone is insufficient. The install contract should bind:

- the full bundle root and publisher
- signatures, revocation, and trust policy
- required and provided interface versions
- immutable dependencies and compatible Runtime versions
- state schema, migration rules, and install receipts
- update, rollback, removal, and availability policy

Runtime should reject missing interfaces, incompatible versions, incomplete
packages, and untrusted publishers before launch. The same verified package
should be admissible on another compatible node without depending on the
original source checkout.

Checked-in source packages and development web projections are build inputs,
not complete signed Digital Capsules. Development tooling may launch them from
a local source path, but it must not present that result as signed distribution,
portable installation, or Runtime admission.

Marketplace can browse verified installed capsules before remote installation
exists. Remote install, update, and removal come only after the signed package
and receipt contract is complete. The install contract should define the CLI
and UI actions instead of the roadmap inventing a command in advance.

Executable capsule isolation remains governed by
[Principle 18](PRINCIPLES.md#18-executable-capsules-are-isolated-execution-environments)
and [Capsule model](docs/CAPSULE_MODEL.md).

The gateway already serves read-only content by CID at `/s/<cid>/`. Once
capsule packages are signed and content-addressed, opening a capsule's CID
should load the capsule itself: the gateway resolves the package, verifies
it, and boots it with whatever host it declares — a game capsule loads its
emulator, and the Home GUI capsule loads the Home host first, the same way
today's launch path binds a viewer to its executable actor. A capsule's CID
then works as a shareable link to the running thing, not just to its files.
This depends on the signed package contract above; an unsigned directory
served by CID must stay inert content.

### 7. Keep Home as the Runtime-owned front door

Home is the default user surface. Runtime owns identity, object access,
capabilities, hosted-route policy, and capsule lifecycle. Home consumes those
contracts. System is a separate app launched through the same authority model.

The main path should stay simple:

1. sign in
2. find an app, person, or object
3. open it through Runtime authority
4. focus, close, or return Home cleanly

The graphical and terminal shells should share Runtime facts and explicit shell
switching while owning different presentation surfaces. Neither shell gains
provider authority from its role. Host adapters for server, desktop, mobile,
and kiosk deployments must preserve the same Home and capsule contracts.

Home should grow from an app launcher into an object browser with useful People,
Spaces, Apps, and System surfaces. It should hide provider names and transport
details unless the user opens technical inspection.

The host boundary is defined in
[Home shell host contract](docs/HOME_SHELL_HOST_CONTRACT.md).

### 8. Complete Browser behind one Browser, Net, and Exit contract

Browser is one dangerous capsule that must remain contained. It is not the
platform and it must not become an ambient escape to host networking.

Browser asks Runtime for a page and display session. Runtime authorizes Net and
Exit effects, selects a Browser Engine, and returns only the scoped page,
display, input, profile, and Wallet bridge surfaces. The engine has no direct
wallet, chain, storage, DNS, or off-box network authority.

Every host adapter must implement the same Browser, Net, Exit, Wallet, and
lifecycle contracts. Native and microVM adapters are the preferred performance
path for local devices. Hosted engines are valid for web deployments only when
they pass the same isolation, audio, video, input, navigation, Wallet,
lifecycle, and manual UX gates.

Browser profile state belongs to the active principal. Cookies, storage,
permissions, bookmarks, history, and downloads must not leak through a shared
engine profile. Recovery and migration should eventually cover that state under
the same principal-root policy as other personal objects.

Web pages use a constrained Runtime-mediated wallet bridge. Account discovery,
chain selection, signing, and transaction effects retain their normal Wallet
and Inbox approval rules. The page receives the approved result, not Runtime
tokens, provider credentials, or connector authority.

Product readiness requires real target evidence for frame continuity, audio,
input, resize, reconnect, concurrent use where claimed, explicit close, and
orphan cleanup. Diagnostic frames and source-only helpers cannot substitute for
the product display path.

Browser work should stop at the contract/gate layer when the current host
cannot produce that target evidence. Do not keep tuning a hosted proof baseline
as if it were the product architecture.

The hosted operator-image and durable service wrapper path now exists for
deployments that select Selkies. Its presence is packaging evidence, not
Browser product acceptance.

[Browser capsule](docs/BROWSER_CAPSULE.md) owns the product contract.
[Browser VM target](docs/BROWSER_VM_TARGET.md) owns the substrate contract.
[Browser provider acceptance](docs/BROWSER_PROVIDER_BAKEOFF.md) compares hosted
display candidates. [Scripts](scripts/README.md) maps the executable proof and
operator commands; current acceptance and known limits belong in
[state.md](state.md).

### 9. Build People and collaboration around durable objects

People is the trust surface for profiles, contacts, requests, device bindings,
and service discovery. Chat owns direct, group, and public conversations.
Inbox owns review of contact, conversation, and capability requests.

The identity split must stay explicit:

- passkeys authorize a local principal; they are never the network identity;
- a principal-owned Profile DID is the stable person/contact identity;
- a device DID is endpoint and signing identity only;
- signed profile documents authorize the current delivery device bindings and
  must be retained by highest accepted revision plus previous-hash linkage;
- direct conversations are scoped to Profile DIDs, not to device rotation.

The product must distinguish people from devices and contacts from
conversations. Discovery is opt-in and describes its actual network scope.
Deterministic signed invites remain available as an explicit alternate
onboarding path.
Stable transport identifiers stay out of ordinary UI.

Service offers can arrive through trusted People and Carrier relationships, but
People does not become the provider control plane. Enabling an offer creates or
selects a principal-scoped provider grant. Providers continue to enforce quota,
expiry, policy, and audit.

Conversation objects need stable identity, participant and device bindings,
message order, attachment references, delivery state, and revocation policy.
Direct messages cannot be presented as private unless their transport and
storage enforce that claim. Local composition and reading should remain usable
without waiting for remote transport.

Profile identity and transport routing must not drift:

- the signed public profile document is the public identity truth;
- older or conflicting profile revisions must fail closed once a newer accepted
  revision is retained;
- device revocation becomes effective when the newer signed profile revision is
  observed, not by transport metadata alone;
- direct messages need both proofs: the device signature proves the sending
  endpoint, and the retained signed profile document proves that device is
  currently authorized for the participant Profile DID;
- Carrier/bootstrap configuration remains connectivity only and never becomes
  person, contact, or conversation authority.

The target contract carries authenticated messages, object updates, presence,
and attachments between runtimes. Runtime must verify the sender, capability,
replay policy, and destination object. Unauthenticated raw gossip does not meet
that contract. Compatibility bridges may map external systems into the target
contract, but they do not define the native model.

See [People and conversations](docs/PEOPLE_CONVERSATIONS.md) for the target
model and ordered implementation slices, and
[Tasks](TASKS.md#collaboration-and-messaging) for open outcomes.

### 10. Keep release, install, share, and sites on truthful paths

Release and installation should use one signed source and one component
identity. A public manifest must describe the artifacts that setup installs.
Update must fail closed when trust, checksums, interfaces, or required artifacts
do not match.

Sharing and sites use the same object model as local content. A public link,
channel head, or site deployment is an explicit promotion of a verified object,
not a hidden side effect of moving a file. Activation and rollback must preserve
the prior good head until the new head is verified.

Operator and debug paths can exist, but they remain secondary and clearly named.
Current release proof belongs in [State](state.md), open release work belongs in
[Tasks](TASKS.md), and command inventory belongs in
[Command matrix](docs/COMMAND_MATRIX.md).

Version, package identity, channel head, and installed artifact are different
facts. Release tooling should derive them from one reviewed candidate and reject
partial publication. The version contract lives in
[Runtime versioning](docs/VERSIONING.md).

## Later direction

### Cross-platform Runtime and host adapters

Server, desktop, mobile, and kiosk hosts present different surfaces over one
Runtime contract. Capsules do not branch on host mode. A host changes how a
surface appears, not identity, capability, object access, or provider effects.

Use Linux as the full-runtime baseline until another platform earns equivalent
evidence. macOS, Windows, mobile, and remote hosts can support useful subsets
without claiming Linux or KVM parity.

### Native object model and content-first design

Packaging existing web apps helps bring software into ElastOS, but the native
model starts with objects. A photo, document, model, video, or published package
has a typed Runtime identity and one or more compatible viewers. Apps view and
edit objects through capabilities instead of owning isolated data silos.

Keep three axes separate:

- execution substrate, such as Component, native process, VM, or data
- product role, such as shell, app, viewer, provider, or content
- launch exposure and orchestration rights

Home organizes around authorized objects and lets Runtime select a compatible
viewer. People owns relationships, Spaces owns mounted views, Apps shows
installed tools, and System owns policy and diagnostics. A role, filename
extension, or manifest field never grants authority.

Marketplace is a catalog over signed capsule packages. Remote mutation waits
for package identity, publisher trust, interface compatibility, applicable
rights, and rollback receipts.

### Identity evolution

Keep `did:key` as the device and node foundation, not the human account root.
Local people remain Runtime principals unlocked by passkeys. Add persona
separation, EID linking, credentials, global names, and cross-device recovery
only through explicit proof and resolver contracts.

### Stronger protection and attestation

Future work includes stronger package attestation, reproducible-build policy,
TPM or TEE evidence, protected-content provider deployment, and public key
services. Each feature extends the existing capability and object contracts.
It must not create a separate trust root or expose key material to apps.

### AI and operator surfaces

AI providers and agents need explicit identity, capabilities, budget policy,
data access, and audit. Hosted credentials stay inside configured providers.
Local and hosted models should expose the same typed Runtime contract.

Operator tools should derive decisions from source, signed artifacts, and
machine-readable evidence. Durable docs should record contracts and current
truth, not terminal transcripts or machine-specific paths.
