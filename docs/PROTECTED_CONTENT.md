# Protected content

Protected content is Runtime-mediated. Library and Marketplace own the creator
and buyer experience, and `elacity-player` owns video presentation. Runtime
owns authority, durable operations, provider selection, Wallet and Chain
coordination, lifecycle, audit, and settlement.

The canonical same-Runtime source path is implemented but inactive. Its proof
uses one Runtime and two principals. The installed product still selects the
provisional `drm`, `rights`, `key`, and `decrypt` authority surfaces until the
cross-Runtime source boundary, installed proof, and atomic cutover are complete.

## Canonical path

The source path has one operation sequence:

1. Library submits a typed source object and the permitted product terms.
2. Runtime creates the durable operation identity and selects the private
   `media-provider`.
3. `media-provider` prepares bounded clear fMP4 in Runtime-authorized private
   staging. It owns probe and transcode process behavior.
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
9. Library or Marketplace asks Home to launch `elacity-player` with an opaque,
   short-lived launch authority. Runtime binds the open to the principal,
   object, accepted viewer, launch, decrypt session, and token.
10. Three independent custody nodes evaluate rights locally. Any two approved
    nodes return recipient-encrypted contributions.
11. The private protected-content decrypt provider reconstructs and uses the
    CEK inside its process. It serves bounded ordered media reads to the exact
    viewer session and settles open, read, and close ownership.

Current source runs this sequence inside one Runtime, and the creator listing is
a local Runtime record. The localhost-to-seed journey needs two additional
typed source behaviors: the buyer Runtime must import or resolve that immutable
listing from its content and Chain identity, and custody nodes must authenticate
a buyer Runtime issuer that differs from the provisioning Runtime. That custody
change preserves the
Profile-signed recipient-key binding, node-local rights decision, signed
operation, exact replay, and provisioning issuer authority. A shared listing
link is sufficient for 0.7. Global listing discovery remains later work.

Runtime journals identities, state, receipts, and settlement. Providers keep
clear media, ciphertext staging, CEKs, shares, process details, and private
routes inside their owned boundaries. Carrier transports only
Runtime-selected remote custody traffic. Storage, provider, Carrier, and Chain
topology stays private.

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

The CentralStorage KID-binding write and its authorization still need deployed
proof. `AuthorityGateway.buyAccess` remains the purchase operation to prove
with its exact deployed ABI, transaction receipt, and event. Deployed
`View`/`Download` contract semantics also remain open. Signed Runtime policy
owns the View and Download action distinction until that contract truth is
verified.

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
exact public key. Reconstruction requires exact threshold settlement and
holder proof, verifies the CEK commitment, and keeps the live CEK inside the
decrypt process. Close expires the launch authority and settles Runtime,
decrypt, viewer, and staging ownership.

The current confidentiality suite is
`elastos-xwing-draft06-hkdf-sha256-aes256gcm/v1` for new protected content.
External cryptographic review remains open before public dKMS or production
confidentiality claims.

## Capsule boundary

Capsules receive typed object identities, product terms, progress, immutable
listing projections, opaque launch authority, and bounded media output. Runtime
derives the principal, Wallet account, Chain authority, provider selection,
availability decision, rights evidence, and effect identity.

Visible protected-content UI may ship only as a disabled/read-only readiness
rail until the installed path and atomic cutover pass. Source behavior alone is
not installed or live product evidence.

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

The macOS and Linux source-home restart helpers validate the installation
receipt and current clean source before any stop or migration. They select only
the stable installed Runtime, own one exact PID file, stop only the proven
prior Runtime, preserve one bounded principal-root rollback, and write one
owner-only restart receipt.

## Remaining work

The ordered release proof is in [TASKS.md](../TASKS.md). The protected-content
part requires:

1. the bounded cross-Runtime listing and custody-issuer source slice described
   above, without changing the frozen public contracts;
2. final source review and a review branch;
3. exact same-tree localhost and seed installation receipts;
4. one real signed owner-only 2-of-3 custody composition across distinct
   operators;
5. private multi-source Chain configuration and deployed Base evidence;
6. one bound KID with allowed and denied Wallet evidence, the CentralStorage
   binding proof, and the exact `AuthorityGateway.buyAccess` receipt/event;
7. three replicas plus repair after one replica is lost;
8. the two-Runtime, two-principal mint-list-deny-buy-open-play-close journey,
   including restart, replay, tamper, settlement, and cleanup;
9. the named manual UIUX journeys; and
10. one atomic cutover that removes the provisional authority surfaces.

The cutover activates the Runtime-owned path and removes provisional startup,
registration, resources, packaging, tests, and docs in the same reviewable
change. It keeps one registry, supervisor, coordinator, journal, and
protected-content path.

Global listing discovery, public custody governance, and document or 3D typed
viewers are separate later work.
