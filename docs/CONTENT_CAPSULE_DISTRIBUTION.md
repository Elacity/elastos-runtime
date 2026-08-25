# Content capsule distribution

This document defines how downloadable content enters ElastOS. It covers free
games, local model files, and other portable data. Protected-content rights and
key release remain in [PROTECTED_CONTENT.md](PROTECTED_CONTENT.md), while
replication policy and availability receipts remain in
[CONTENT_AVAILABILITY.md](CONTENT_AVAILABILITY.md).

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

## Get flow

`Get` is a typed Runtime operation, not a browser download:

```text
signed catalog projection
-> person selects Get
-> Home sends exact content-capsule identity to Runtime
-> Runtime verifies principal, session, capability, publisher, and manifest
-> content provider resolves and fetches the CID
-> availability provider verifies or establishes the required pin
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
