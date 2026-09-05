# Content capsule distribution

This document defines the intended distribution contract for downloadable
content. It covers free games, local model files, and other portable data.
Protected-content rights and
key release remain in [PROTECTED_CONTENT.md](PROTECTED_CONTENT.md), while
replication policy and availability receipts remain in
[CONTENT_AVAILABILITY.md](CONTENT_AVAILABILITY.md).

Runtime currently projects installed capsules. The signed network catalog,
Home Get operation, and model-content packaging described here remain planned
work; this contract does not claim that those features are implemented.

The implemented content plane already provides `elastos://content` publish,
fetch, status, ensure, repair, and unpublish operations. It records signed local
availability receipts, keeps raw IPFS access system-only, and serves verified
gateway CID reads. These form the package-delivery foundation. The signed
network catalog and install contract remain open.

## Decision

A downloadable game or model is a content capsule, not a service offer or a raw
URL entry in Home.

- A GBA game is a `role=content`, `type=data` capsule bound to a compatible
  viewer such as `gba-emulator`.
- A GGUF model is a `role=content`, `type=data` capsule whose manifest describes
  the model format, quantization, resource requirements, license, provenance,
  and compatible model-provider interface.
- The canonical identity is the CID of the complete immutable capsule closure,
  including the manifest and payload. Payload hashes may remain in the manifest
  for format-specific verification, but they do not create a second package
  identity.
- A verified publisher signature authenticates the package claim. Runtime trust
  policy decides whether to admit it.
- An availability receipt says which provider accepted responsibility for the
  CID. A CID alone does not prove that the bytes are currently retrievable.

`elastos.service.offer/v1` is separate. It describes an available running
service and its grant policy. A model provider may offer inference for an
installed model, but the GGUF capsule itself is content and does not need a
service offer to be listed, fetched, or pinned.

## One content identity, several projections

The content capsule manifest and CID are the package truth. Other records are
projections with narrower jobs:

| Record | Job |
| --- | --- |
| Signed catalog entry | Points to an exact capsule CID and presents publisher, version, compatibility, size, and license metadata. |
| Availability receipt | States where and under which policy the CID is retained or replicated. |
| Installed inventory | Records which verified capsules this Runtime has admitted and pinned. |
| Install or removal receipt | Records the principal, exact CID, operation, result, and time. |
| Service offer | Advertises a running provider capability after installation; it is not package identity or install authority. |

Home, Library, Apps, a future native model hub, and command projections must
derive from these records. They must not maintain independent package databases
or turn display rows into authority.

## First vertical slice: CID-delivered local Qwen

The first delivery slice packages the currently verified Qwen GGUF as one
content capsule and makes it available to the existing local model provider.
It uses this sequence:

1. The publisher creates a complete immutable closure with the content-capsule
   manifest, GGUF payload, format and quantization facts, resource requirements,
   license, provenance, and compatible model-provider interface.
2. The CID of that complete closure becomes the package identity. A publisher
   DID and signature authenticate the source claim for the exact CID.
3. Setup carries one trusted signed catalog-root CID or equivalent pinned
   signed head. Runtime verifies its CID and signature under configured
   publisher trust, then Home projects its signed entries. The initial
   availability basis may be the publisher alone when the entry states that
   limitation.
4. Home sends one typed Get request for the exact catalog entry. Runtime checks
   principal authority and composes the existing content `fetch`, `status`, and
   `ensure` operations with installed-inventory and provider-registration state.
   Get is an admission workflow, not a second content-transfer protocol.
5. Runtime stages the fetched closure under a bounded private path, verifies the
   CID, publisher signature, availability evidence, size, resources, license,
   format, paths, and declared provider interface, then admits it atomically and
   writes the install receipt.
6. Runtime durably binds each running model offer to the exact admitted record
   and content CID. From that record, Runtime gives `model-provider` only the
   private canonical artifact descriptor that it revalidates before inference.
   Package identity remains separate from service-offer identity and install
   authority.
7. Home derives Available, Downloading, and Ready from catalog, transfer, and
   installed records. Home may project an approved package name, CID, and
   status. Runtime keeps host paths and backend routes private. File names and
   service offers supply no install state.

Later catalog updates may arrive through content or Carrier providers. Runtime
applies the same publisher-signature and CID checks before Home projects them.

The CID proves the closure bytes. The publisher signature proves who made the
source claim. Availability receipts prove accepted retention. Runtime owns
policy and atomic admission. Content and availability providers own backend
selection and private routes.

Acceptance requires:

- one signed catalog root and entry that resolve to the exact complete-closure
  CID and verified publisher DID;
- honest availability evidence for publisher-only, local, or independently
  retained bytes, with the complete CID as the only package identity and
  publisher transport and topology kept private;
- bounded staging and checks for closure CID, signature, size, disk and memory,
  license, paths, GGUF format, and model-provider interface before admission;
- one atomic installed record and receipt, plus a durable Runtime-owned link
  from each running offer to that exact record and content CID, that yields only
  the private canonical artifact descriptor that `model-provider` revalidates;
- Home transitions from Available to Downloading to Ready from Runtime facts
  and projects only the approved package name, CID, and status while Runtime
  keeps host paths and backend routes private;
- restart proof that preserves the single admitted record without another
  transfer, followed by one fresh model request that produces one inference
  through the admitted artifact, plus explicit removal, unpin policy, provider
  cleanup, removal receipt, and partial-file cleanup; and
- installed negative tests for an incorrect CID, signature, publisher, digest,
  size, license, resource requirement, interface, availability claim, truncated
  transfer, and interrupted admission.

## Get flow

`Get` is a typed Runtime admission operation, not a browser download or another
content-provider transfer operation:

```text
signed catalog projection
-> person selects Get
-> Home sends exact content-capsule identity to Runtime
-> Runtime verifies principal, session, capability, publisher, and manifest
-> Runtime reuses content fetch and status for the CID
-> Runtime reuses availability ensure for the required pin
-> Runtime atomically admits the capsule and writes an install receipt
-> installed inventory and Home facts refresh
```

Runtime chooses the provider and route. Home must not call
`download_component`, supervisor download routes, IPFS/Kubo, a publisher HTTP
endpoint, or an external model host directly. A failed fetch, signature,
manifest, size, compatibility, license-policy, or availability check leaves no
partially admitted capsule.

The reverse operation must be explicit. Removing a local capsule updates the
installed inventory and writes a removal receipt. Unpinning local bytes does
not claim that the CID disappeared from the wider network.

## Bootstrap while the network matures

Content should already be available from the declared ElastOS availability
network before it is presented as normally Gettable. During bootstrap, the
existing trusted publisher may be the only declared source or replica. This is
an availability limitation, not a different identity model: the catalog still
names the content capsule by CID and Runtime still verifies the same package.

Small development or demo capsules may keep their bytes in this repository
when that is practical and legally permitted. Large assets such as GGUF files
should not be added to Git history. Their manifests, provenance, and catalog
records may be reviewed here while a trusted publisher serves the CID-addressed
bytes until community replication is ready.

The installed `components.json` may seed first-party setup and record installed
state. It is not the global content catalog. The current duplicate `capsules`
and `external` component shapes must converge rather than gain a third Store or
Home registry.

## External repositories and Hugging Face

An external web repository is not a Runtime trust root or a capsule-facing
network path. Home catalog rows must not expose mutable Hugging Face URLs as
content identity.

If Hugging Face support is added, it belongs in a dedicated gateway provider
capsule behind the normal Runtime and content-provider contracts. That provider
may:

1. resolve an explicitly approved immutable external revision;
2. verify the upstream and derived-model licenses, source, size, and digest;
3. package the approved artifact as an ElastOS content capsule;
4. publish it through the normal content and availability providers; and
5. return the resulting CID and receipts.

Ordinary capsules receive neither ambient web access nor Hugging Face
credentials. The gateway does not create an alternate install rail.

The preferred long-term product is an ElastOS-native, community-controlled
catalog of content capsules. Signed publishers describe exact CIDs, availability
providers retain and replicate them, and compatible local providers consume
them through Runtime contracts.

## Admission checks

Before a content capsule is shown as Gettable or admitted, verify at least:

- canonical manifest encoding and the complete bundle CID;
- publisher identity, signature, version, and revocation state;
- payload digest, exact size, media or model format, and strict path limits;
- viewer or provider compatibility without filename-only inference;
- disk and memory requirements before transfer;
- license and provenance for the payload and bundled artwork, audio, metadata,
  quantization, and base model where applicable;
- an availability basis that states honestly whether content is publisher-only,
  locally pinned, or independently replicated; and
- atomic install, idempotent retry, restart persistence, removal, audit, and
  partial-download cleanup.

Free means no payment is required. It does not remove publisher verification,
user install authority, license obligations, resource checks, or receipts.
