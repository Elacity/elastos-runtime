# Protected content

Protected content is Runtime-mediated. Library and Marketplace own the creator
and buyer experience, and `elacity-player` owns video presentation. Runtime
owns authority, durable operations, provider selection, Wallet and Chain
coordination, lifecycle, audit, and settlement.

The intended content-distribution contract gives free and protected content
the same package identity and availability path. Protected content adds rights,
key release, and decryption. See
[Content capsule distribution](CONTENT_CAPSULE_DISTRIBUTION.md) for the planned
catalog and Get contract.

The canonical source path is implemented but inactive. Source tests cover
same-Runtime and two-Runtime journeys. The installed product still selects the
provisional `drm`, `rights`, `key`, and `decrypt` authority surfaces until the
installed proof and atomic cutover are complete.

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

Operator provisioning of that configuration is one explicit command surface,
`elastos protected-content-config`. It creates the policy authority key,
provisions each custody host's inactive state root and exports its node
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

## Remaining work

The ordered release proof is in [TASKS.md](../TASKS.md). The protected-content
part requires:

1. final combined-source review and CI;
2. matching localhost, seed and third-node installation receipts;
3. one signed owner-only 2-of-3 custody composition across distinct operators;
4. private multi-source Chain configuration and deployed Base evidence;
5. bound-KID allow/deny/unbound reads, CentralStorage binding and the exact
   funded `AuthorityGateway.buyAccess` receipt/event;
6. three replicas plus repair after one replica is lost;
7. the installed two-Runtime mint-list-deny-buy-open-play-close journey,
   including restart, replay, tamper rejection, settlement and cleanup;
8. the manual UIUX matrix in `TASKS.md`; and
9. one atomic cutover that removes the provisional authority surfaces.

The cutover activates the Runtime-owned path and removes provisional startup,
registration, resources, packaging, tests, and docs in the same reviewable
change. It keeps one registry, supervisor, coordinator, journal, and
protected-content path.

Global listing discovery, public custody governance, and document or 3D typed
viewers are separate later work.
