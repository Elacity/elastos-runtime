# Protected content

Protected content is Runtime-mediated. Library and Marketplace own the creator
and buyer experience, `elacity-player` owns video presentation,
`elacity-reader` owns everything else a bought file can be — a picture, a
document, text, a 3D model, a book or a comic — and `creator` uploads a file
through the Library transport and protects-and-lists it in one flow. Runtime
owns authority, durable operations, provider selection, Wallet and Chain
coordination, lifecycle, audit, and settlement.

Library offers protection on any file the Runtime accepts. The source mime
decides the kind: `video/*` and `audio/*` are media and take the rendition
path; everything else is an object and takes the chunked-payload path. The
kind then decides the viewer, so the creator never picks one.

The intended content-distribution contract gives free and protected content
the same package identity and availability path. Protected content adds rights,
key release, and decryption. See
[Content capsule distribution](CONTENT_CAPSULE_DISTRIBUTION.md) for the planned
catalog and Get contract.

The canonical path is the installed product path. Source tests cover
same-Runtime and two-Runtime journeys, and the installed proof driver recorded
`finalize` `overall_ok: true` on the simulation-only harness. There is no other
protected-content authority.

## Canonical path

The source path has one operation sequence:

1. Library or Creator submits a typed source object and the permitted product
   terms.
2. Runtime creates the durable operation identity and selects the private
   `media-provider`.
3. For media, `media-provider` prepares one bounded clear fMP4 rendition in
   Runtime-authorized private staging. It owns probe and transcode process
   behavior. For an object, no rendition step runs: the file is sealed as it
   is.
4. The private protect provider encrypts the prepared media, provisions the
   immutable 2-of-3 custody envelope, and returns bounded identities and
   receipts.
5. Runtime publishes the encrypted object through the normal
   `elastos://content` path with an exact three-replica requirement. The
   existing repair task keeps that same requirement after replica loss.
6. Runtime binds creator mint and listing effects to the verified Wallet
   account and operator Chain configuration. It projects a listing only after
   the exact effect and finalized evidence agree.
7. Marketplace reads bounded immutable listings. A buy request contains only
   the mint identity; Runtime derives the buyer, account, effect, and Chain
   authority.
8. Before buy or open, Runtime verifies one fresh signed availability receipt
   for the exact mint, content identity, CID, publisher, provider, policy,
   replica count, and freshness window.
9. Library or Marketplace asks Home to launch the viewer the content calls for
   — `elacity-player` for media, `elacity-reader` for everything else — with an
   opaque, short-lived launch authority. Runtime binds the open to the
   principal, object, verified executable actor, launch, decrypt session, and
   token.
10. Three independent custody nodes evaluate rights locally. Any two approved
    nodes return recipient-encrypted contributions.
11. The private protected-content decrypt provider reconstructs and uses the
    CEK inside its process. It serves bounded ordered media reads, or bounded
    fixed-size chunk reads for a non-media file, to the exact viewer session
    and settles open, read, and close ownership.

Source tests cover this sequence across two Runtimes with separate principals,
Wallets, device identities and state. The creator exports an immutable listing
package; the buyer imports and verifies it before purchase and playback. Custody
release authenticates the buyer Runtime issuer declared by the signed operation
and bound by the buyer Profile. The creator Runtime retains provisioning
authority. The funded installed localhost-to-seed journey remains open.
Shared listing links work independently of global listing discovery.

Runtime journals identities, state, receipts, and settlement. Providers keep
clear media, ciphertext staging, CEKs, shares, process details, and private
routes inside their owned boundaries. Carrier transports only
Runtime-selected remote custody traffic. Storage, provider, Carrier, and Chain
topology stays private.

## Media renditions and the CENC header

Audio is a first-class rendition, not a video with the picture missing.
`media-provider` probes the source and keeps exactly one track. A source that
carries a real video track becomes the pinned `video/mp4` / `avc1.640028`
rendition, exactly as before. A source with only sound becomes an
`audio/mp4` / `mp4a.40.2` AAC fMP4 rendition. Cover art is skipped rather
than taken for a video track, so a tagged music file keeps its sound instead
of being re-encoded into a still picture. The output profile and the
provider's configuration fields are unchanged.

The player presents whichever rendition it is given. A rendition with no
image track and no poster collapses to the controls alone; that is the
honest state, not a failure. Runtime has no source for a poster yet, so
audio currently always lands on the controls-only frame.

Protected media carries a CENC Protection System Specific Header. One `pssh`
box declares the Elacity post-quantum hybrid-threshold protection scheme
under system id b6e254ef-0dc5-47fe-94e7-0e72ed1dc7b0. Its payload is public
scheme description and never key material: the versioned schema name, the
protection scheme, the sample-encryption profile, the key-encapsulation
suite, the content access id a consumer presents when it asks for a key, and
the custody pool, epoch and committee authorization identities that say which
quorum can answer. Those custody identities are bound at publish to the
mint's own, so a box lifted from one title cannot stand for another. One
module writes, parses and admits the box, so the producer, the read path that
rewrites a protected init segment back to clear, and the layout validator
cannot drift apart.

Sample encryption is AES-128-CTR, which has no per-sample authentication
tag. This is inherent to the CENC standard, which defines no authenticated
scheme. Tamper-evidence on the media path therefore comes from the staged
segment's SHA-256 and byte-length check, which runs before any keystream is
applied — not from the cipher. Any future consumer that fetches segments over
a network rather than reading Runtime-staged bytes has to carry that check
itself or it has no integrity protection at all. The object path does not
have this shape: its chunks are AES-256-GCM and the integrity is in the
cipher. [Protected-content crypto review](PROTECTED_CONTENT_CRYPTO_REVIEW.md)
records both, with the tests that pin them.

## Viewer admission

A viewer operation is admitted on the verified executable actor behind the
launch token, never on anything the caller sends. The gateway refuses a
viewer operation unless the launch token's selected resource and executable
actor are the same and that actor is one of the two admitted viewers. It then
strips every routing and binding field from the request body and substitutes
the verified values. The session binding is derived over that verified actor,
so a binding minted for one viewer can never match the other, and a viewer
launched for one kind of content cannot open the other kind.

## Private providers

`ProviderRegistry` owns four reserved Runtime-only targets:

- `protect`
- `media`
- `custody`
- `protected-content-decrypt`

Capsule URI lookup, public provider proxying, interface projection, and route
lists exclude all four targets. Runtime registers a target only after the
provider returns the exact successful identity, version, configured state,
schema, and ordered operation set. Rejected startup settles and reaps the child
before it returns an error.

Protect, media, and decrypt run as local native provider processes. Custody can
use Carrier to reach a Runtime-selected remote endpoint. Runtime keeps signed
operation authority, provider selection, policy, and response verification.

The custody node state root keeps the historical directory name
`protected-content/custody-provider/inactive`. Provisioned hosts already hold
state there, so the path is frozen; the name carries no meaning about whether
the custody plane is active.

## Identity and rights

The CENC KID and `EncryptedContentIdentityV1` are separate:

- The KID is the exact bytes16 value used by the deployed rights read.
- The encrypted-content identity binds the complete protected object and media
  contract.

Verified deployed read behavior is:

- `AuthorityGateway.hasAccessByContentId(address,bytes16) -> bool` owns access
  reads.
- `CentralStorage.ipReference(bytes16)` is the proven KID read resolution.
- Unknown KIDs revert with `UnboundContentId(bytes16)`; a bound KID without
  access returns `false`.

The configured Base 8453 contract uses these operations. Deployed verification
evidence belongs in [state.md](../state.md):

- `CentralStorage.bindIP(bytes16,address,uint256)` accepts acknowledged
  contracts only and is called by `AssetFactory.registerNewAsset`.
- Native `AuthorityGateway.buyAccess` uses selector `0xf7580ad9`.
- ERC20 `AuthorityGateway.buyAccess` uses selector `0x0ede2294`; Wallet first
  approves the operative `paymentProcessor()`.
- EventHub is the mint event emitter.
- A deployed bound KID has recorded allowed, denied, and unbound results.

The exact funded buy receipt and event remain installed proof. Deployed access
is one boolean per holder and KID, so signed Runtime policy owns the View and
Download action distinction.

The operator supplies one owner-only
`protected-content/chain-provider.json` with one versioned
`protected_content_network`. Runtime supplies its operation issuer identity
separately. Each custody node uses its own node-host Chain provider, reads 2-5
explicit private RPC sources, and requires two exact agreeing finalized
results. Generic Chain configuration remains separate and is not a
protected-content fallback.

## Custody and decryption

The source policy is exactly 2-of-3 across distinct operators and failure
domains. Each custody node stores one share and checks the exact signed Runtime
operation, Wallet subject, KID, full encrypted-content identity, action,
custody epoch, policy, finalized rights evidence, recipient authorization, and
time window.

`CustodyEnvelopeV1` is private Runtime provisioning material. Runtime can
carry the envelope but cannot open the node-sealed shares. Public metadata
contains bounded identities, threshold and epoch facts, the CEK commitment,
and signatures.

The decrypt provider generates each operation-scoped recipient key and keeps
the secret behind an opaque handle. The authenticated Profile authorizes the
exact public key. Reconstruction uses the authenticated release operation,
verified signed epoch, released contributions and terminal receipt, recipient
possession, and public CEK commitment. Runtime stores no playback custody
envelope or sealed-share bytes. Close expires the launch authority and settles
Runtime, decrypt, viewer, and staging ownership.

The current confidentiality suite is
`elastos-xwing-draft06-hkdf-sha256-aes256gcm/v1` for new protected content.
External cryptographic review remains open before public dKMS or production
confidentiality claims. [Protected-content crypto review](PROTECTED_CONTENT_CRYPTO_REVIEW.md)
is the reviewer-facing package: every primitive with its crate version and
parameters, what each construction binds, a threat model naming the test that
pins each claim, and committed golden vectors replayed against the shipped
code.

## Capsule boundary

Capsules receive typed object identities, product terms, progress, immutable
listing projections, opaque launch authority, and bounded media output. Runtime
derives the principal, Wallet account, Chain authority, provider selection,
availability decision, rights evidence, and effect identity.

Visible protected-content UI is live product surface backed only by the
Runtime-owned path. Source behavior alone is still not installed or live
product evidence: each installed claim needs its own receipt.

## Installed source contract

Full `scripts/setup-source-home.sh` installs the stable Runtime under the
platform data root at `bin/elastos`. It writes the owner-only
`receipts/source-home-installation.json` receipt after components, native
providers, capsule trees, and source-home capsule metadata are final.

Run `scripts/protected-content-installed-static-audit.py` with explicit source
root, installed data root, installed Runtime, platform, profile, and role. Its
bounded redacted receipt separates:

- source and static artifact failures;
- operator configuration prerequisites; and
- active installed proof prerequisites.

`ready_for_active_proof` means the static artifacts and supplied operator
configuration passed admission. Runtime startup, signed custody validation,
live Chain evidence, replication and repair, and mint-buy-play remain active
proof.

Operator provisioning of that configuration is one explicit command surface,
`elastos protected-content-config`. It creates the policy authority key,
provisions each custody host's state root and exports its node
descriptor, prints the Runtime operation issuer custody hosts must trust,
assembles and signs the owner-only 2-of-3 custody composition from three node
descriptors, and installs the private multi-source Chain configuration. A
generated composition or Chain configuration is proven against the Runtime
loader before success is reported, and an installed composition can be
re-verified in place. The installer never creates this operator state.

The macOS and Linux source-home restart helpers validate the installation
receipt and current clean source before any stop or migration. They select only
the stable installed Runtime, own one exact PID file, stop only the proven
prior Runtime, preserve one bounded principal-root rollback, and write one
owner-only restart receipt.

`elastos run <provider> [--with <provider>]... --carrier-addr <addr>` hosts
one or more native providers standalone, outside the full Home/gateway
supervisor tree, and writes an owner-only readiness receipt at
`<data_dir>/run/provider-host.ready.json` once every requested provider is
registered and Carrier is bound. A custody node process is exactly this
composition: `elastos run custody-provider --with availability-provider
--with ipfs-provider --with chain-provider --carrier-addr <addr>`. The chain
plane is not optional on a committee member: a node settles every release
by evaluating the rights request through its own
`protected_content_rights_evidence` call, so the host refuses to start the
chain plane without a `protected-content/chain-provider.json` naming the
protected-content network, and its evidence RPC URLs must be reachable from
the node itself (not only from the client).

`deploy/custody-host/` builds a simulation-only container image of that
composition plus a same-host, container-per-node compose harness for three
custody nodes. Its own README states the simulation boundary explicitly:
process isolation and real Carrier transport between the containers are
faithfully simulated, but distinct hardware, distinct operators, and
distinct failure domains are not, and the image must never hold a real
recipient's key material. The operator flow over that harness is: each
node's container provisions its custody state and exports a
DID-named public descriptor to a shared handoff directory on first boot, and
reads the client's protected-content chain configuration back from that same
directory on every boot (`up.sh sync-chain-config` derives it, rewriting
host-loopback evidence RPC URLs to the container-reachable host gateway); the
operator runs `elastos node peer add` against each node's exported connect
ticket from the client Runtime; the operator then runs the offline
composition ceremony described above over the three descriptors to
assemble and sign the owner-only 2-of-3 composition; a node restart (the
platform restart helper, or the container's own restart) proves the node
returns with the same DID and no required environment; and the installed
proof driver script (see [scripts/README.md](../scripts/README.md)) drives
the resulting composition end to end.

## Remaining work

The ordered release proof is in [TASKS.md](../TASKS.md). The protected-content
part requires:

1. final combined-source review and CI;
2. matching localhost, seed and third-node installation receipts; a
   simulation-only container harness proves the same node-provisioning,
   peer-add, and restart-resurrection mechanics on one host, not distinct
   installed hardware;
3. one signed owner-only 2-of-3 custody composition across distinct
   operators; the composition ceremony and its offline verify path are
   proven over three real node descriptors and real Carrier transport
   dials on the simulation-only harness, not yet across genuinely distinct
   operators and failure domains;
4. private multi-source Chain configuration and deployed Base evidence;
5. bound-KID allow/deny/unbound reads, CentralStorage binding and the exact
   funded `AuthorityGateway.buyAccess` receipt/event; proven headless on an
   Anvil fork of Base by the installed e2e proof driver, not yet on the real
   Base deployment;
6. three replicas plus repair after one replica is lost; proven on the
   simulation-only harness by the same driver, not yet on distinct hardware;
7. the installed two-Runtime mint-list-deny-buy-open-play-close journey,
   including restart, replay, tamper rejection, settlement and cleanup;
   proven headless by the same driver (`finalize` reads `overall_ok: true`,
   see `state.md`), not yet through Brave on the seed;
8. the manual UIUX matrix in `TASKS.md`.

## Provisional retirement (completed)

The provisional `drm-provider`, `rights-provider`, `key-provider`, and
`decrypt-provider` capsules, the `elastos://drm`, `elastos://rights`,
`elastos://key`, and `elastos://decrypt` schemes, the
`elastos_common::protected_content` DTO surface, the content plane's `sealed`
object kind, the Library share provider chain, and the provisional
retirement-guard smoke were removed in one slice. No fallback, dual route, or
compatibility decoder exists.

The retired names are not reservable: they were removed from the provider
registry's reserved sub-provider allowlist, so a later provider cannot
re-register those routes. `scripts/check-wci-alignment.sh` fails if any of them
reappears in `capsules/`, `components.json`, an install profile, or the
capability mapping.

Picture, document, text, 3D, book and comic viewers ship in
`elacity-reader`. Global listing discovery and public custody governance are
separate later work.
