# Content Availability and IPLD

> Architecture contract and implementation direction.
>
> Current behavior has a first `content` provider seam that delegates to
> `ipfs-provider` for local CID creation/pinning, blocks normal capsule
> capability requests to `elastos://ipfs/*`, and records signed local
> availability receipts. This document defines where content addressing, IPLD,
> IPFS/Kubo, Carrier, and future replication providers belong as the SmartWeb
> content plane matures.

## Decision

IPLD is a good fit for ElastOS, but it is not the network.

Use IPLD as the data model for content-addressed SmartWeb objects: published
documents, sites, shares, release manifests, sealed content, channel heads,
availability receipts, and provenance records.

Do not use IPLD as a replacement for Carrier, Runtime capabilities, dDRM,
storage incentives, or availability policy.

There is no reason to add a heavy IPLD dependency immediately. The current repo
already has CID validation and deterministic JSON objects in the share/document
paths. The near-term IPLD work is to keep those objects IPLD-compatible so they
can later be encoded as DAG-CBOR/DAG-JSON, exported as CAR files, or synchronized
with graph-selection protocols without changing the product contract.

Clean placement:

| Layer | Responsibility |
|---|---|
| Runtime | capability checks, principal/session binding, provider dispatch, audit |
| Carrier | secure peer discovery, coordination, object/message/stream transport |
| Content provider | publish/fetch/status/replicate/repair policy and receipts |
| IPLD | content-addressed object graph and manifest data model |
| IPFS/Kubo | first block/CID backend for local add, pin, cat, and fetch |
| Elacity/supernodes/volunteers | first availability and replication targets |
| dDRM/key provider | rights verification and decryption capability release |
| Blockchain | DID, provenance anchors, payment/licensing hooks, optional storage incentives |

## Why IPLD Fits

IPLD is the shared data model for hash-linked data. That maps directly to the
ElastOS object model: immutable objects, mutable signed heads, provenance links,
viewer links, package links, and encrypted payload links can all be represented
as one traversable content graph.

Use the least invasive IPLD path first:

1. Model SmartWeb objects as JSON/CBOR-shaped IPLD data.
2. Prefer DAG-CBOR for canonical signed objects and DAG-JSON where readability is more important.
3. Use IPLD schemas only after the object shapes stabilize.
4. Avoid new codecs unless an existing external format must be bridged.

This keeps the repo aligned with the IPLD recommendation to start with the data
model before adding schemas, ADLs, or custom codecs.

## What IPLD Does Not Solve

IPLD can identify and traverse a CID graph. It does not decide:

- who is allowed to publish or read an object
- which CIDs should be replicated
- whether a buyer can fetch content when the creator is offline
- how many peers must pin content
- who pays volunteer or professional storage providers
- who may decrypt sealed content

Those are Runtime, Carrier, availability, dDRM, key-provider, decrypt-provider,
and blockchain responsibilities.

## Capsule-Facing Contract

Normal app, viewer, and content capsules should not call `elastos://ipfs/*`.

They should call the higher-level content plane:

```text
elastos://content/publish
elastos://content/fetch
elastos://content/status
elastos://content/ensure
elastos://content/repair
elastos://content/unpublish
```

The content provider may use local Kubo, Elacity APIs, IPFS Cluster-like
replication, volunteer nodes, or future paid storage networks underneath. The
capsule contract stays the same.

`elastos://ipfs/*` remains only because current code already uses it as the
low-level provider/backend surface for system services and explicit operator
tooling. The IPFS backend is now system-only for ordinary capsule capability
requests and should continue moving behind the content provider wherever a path
is not intentionally low-level operator tooling.

PC2's Kubo/IPFS Cluster/supernode work is a useful backend reference for the
first real SmartWeb availability network: replication policy, health checks,
repair loops, and supernode pinning. It should plug in behind
`availability-provider`. It should not change the capsule-facing contract from
`elastos://content/*` back to raw IPFS or Elacity SDK calls.

## Publish Flow

Default publish should mean availability, not just "make a CID."

```text
object files
-> IPLD-compatible object manifest
-> local add/pin through ipfs-provider
-> availability policy selected by runtime/provider defaults
-> replicate to Elacity/supernodes/volunteer targets
-> verify fetch from a clean path
-> write signed availability receipt
-> return elastos:// object/CID link
```

Early implementations can honestly report `local_pinned` before replication is
available. The desired SmartWeb default is `network_available`: the object is
synced to the configured availability network without the app knowing which
backend made that true.

## Object Manifest

Directory publishes now include a deterministic `_elastos_object.json` sidecar
before CID creation. It is intentionally plain JSON with IPLD-compatible fields
so the shape can later move to DAG-CBOR/DAG-JSON without changing the product
contract.

Minimum shape:

```json
{
  "schema": "elastos.content.object.manifest/v1",
  "kind": "document",
  "content_digest": "sha256:...",
  "links": [
    {
      "rel": "rights.policy",
      "cid": "bafy..."
    },
    {
      "rel": "release",
      "cid": "bafy..."
    }
  ],
  "files": [
    {
      "path": "index.md",
      "sha256": "...",
      "size": 123
    }
  ]
}
```

`kind` is currently `capsule`, `directory`, `document`, `release`, `sealed`,
`share`, or `site`. Directory entries are sorted before publishing, duplicate
paths are rejected, and manifest links are validated CIDs sorted by relation so
the same object has one stable package shape. Availability receipts, provenance
records, rights policies, sealed payloads, and signed channel heads remain
separate schema-bearing objects linked by CID and provider state.

Optional identity fields may be attached when they are real:
`publisher_did` identifies the publisher/controller, and `object_did` identifies
a stable mutable object/head only if such an identity has been minted or
registered. Do not derive a fake object DID from the CID by default. The CID
already identifies one immutable graph; the object/head identity is only needed
when a stable logical object points to multiple revisions.

`sealed` objects are stricter than generic directories. They must include
`sealed.json` using `elastos.sealed.object/v1`, approved protected-content
algorithm metadata, and manifest links for `payload`, `rights.policy`,
`availability.receipt`, and `provenance`. The content provider rejects
incomplete sealed objects before they reach the IPFS backend.

Release publishing keeps the raw signed `release.json` and `release-head.json`
CIDs for existing update clients. In parallel, the publish flow now emits
`release` content objects whose `_elastos_object.json` manifests link the raw
release, head, runtime binary, components, and capsule artifact CIDs. That gives
the SmartWeb object graph a stable release package shape without breaking the
byte-fetching installer contract.

## Availability Receipt

Every successful publish or availability repair should produce a signed receipt.

Minimum shape:

```json
{
  "payload": {
    "schema": "elastos.content.availability.receipt/v1",
    "cid": "bafy...",
    "uri": "elastos://bafy...",
    "publisher_did": "did:key:...",
    "provider": "ipfs-provider",
    "policy": "local_pin",
    "status": "local_pinned",
    "replicas": 1,
    "checked_at": 1777852800
  },
  "signer_did": "did:key:...",
  "signature": "..."
}
```

This receipt is not a payment proof and not a rights grant. It says a provider
accepted or verified an availability responsibility for a CID under a policy.
The first implementation appends signed JSONL receipts under runtime-managed
system state and exposes the latest receipt through `elastos://content/status`.

## Availability Provider Seam

The content provider always creates the local CID through the low-level
`ipfs-provider` backend first. If a runtime-owned provider is registered for the
internal `availability` provider scheme, the content provider then asks it to
ensure network availability:

```json
{
  "op": "ensure",
  "cid": "bafy...",
  "uri": "elastos://bafy...",
  "policy": "network_default",
  "publisher_did": "did:key:...",
  "local": {
    "status": "local_pinned",
    "provider": "ipfs-provider",
    "replicas": 1
  }
}
```

A successful adapter may return `network_available` with a provider name and
replica count. A failing or malformed adapter is recorded as `repair_needed`.
If no availability provider is registered, the content provider reports the
honest local state. This is the seam where Elacity, supernode, volunteer, or
later market-backed availability providers should plug in without becoming
capsule-visible IPFS/Kubo/SDK authority.

## dDRM and Protected Content

Availability stores bytes. Rights decide who may use them.

Protected content should be encrypted before it is published. IPFS, Elacity,
supernodes, and volunteers can safely store encrypted blocks. The dDRM or
ElastOS-native rights provider verifies ownership, subscription, access token,
or license state. The `drm-provider` owns the protected-content open contract
and should delegate to rights, key, and decrypt providers. The key provider
releases a short-lived decryption capability only after Runtime authority and
rights checks pass.

That keeps the core invariant simple:

```text
public CID can expose encrypted bytes
valid rights holder gets decrypt capability
unauthorized opener gets no key
```

## Default SmartWeb Sync

The desired product behavior is that published ElastOS objects sync to the
SmartWeb availability network by default.

In implementation terms, this means:

- the local runtime always pins newly published objects first
- a default availability provider attempts network replication
- volunteer/supernode participation is policy-governed and quota-limited
- repair workers keep receipts fresh and retry failed replication
- future blockchain incentives pay for proven storage/serving work

This belongs behind the Carrier/provider plane. Carrier coordinates peers and
secure messages. The content provider owns availability policy. IPFS/Kubo and
cluster-like systems move and replicate CID blocks underneath.

## Red Lines

- Do not make Carrier responsible for license policy or decryption.
- Do not expose raw Kubo, IPFS Cluster, Elacity SDK, or gateway APIs to normal capsules.
- Do not treat a CID as an availability guarantee.
- Do not call a single pinning service decentralized storage.
- Do not rely on CID secrecy for private content.
- Do not add payment incentives before availability receipts, quotas, repair, and abuse controls exist.

## Current Repo Foundation

The existing `ipfs-provider` is the right low-level starting point because it
already wraps Kubo behind a provider boundary. The first `content` provider seam
now sits above it for Documents publish/unpublish, reports honest
`local_pinned` / `local_unpinned` availability state, calls a registered
availability provider to upgrade publish/ensure results to `network_available`
or `repair_needed` when configured, writes signed availability receipts, routes
Documents, Share, Site, and provenance attestation writes through the same
content path, routes ordinary capsule publishes through content availability,
adds a first fetch operation for simple CID/path reads, routes gateway CID file
reads and share metadata/head reads through that fetch path, adds local
ensure/repair operations that re-pin a CID or record `repair_needed`, injects
deterministic `_elastos_object.json` manifests into directory publishes,
materializes data-capsule opens from that manifest through the content provider
with size/hash checks, routes `run --cid` and `serve --cid` materialization
through content availability, validates content status CIDs, keeps
`elastos://ipfs/*` system-only at the capsule capability request surface, routes
supervisor artifact downloads through `elastos://content/fetch` instead of
direct `ipfs` sub-provider calls, and emits release-object manifests for public
release/head/installer objects while preserving raw release CIDs for update
compatibility. `elastos open` now treats release object CIDs as release metadata
graphs, verifies the signed release/head envelope before summarizing them, and
CID materialization rejects release objects as non-launchable instead of treating
them as generic directories. The first `availability-provider` capsule can now
be registered for configured Elacity/supernode-compatible targets by setting
`ELASTOS_AVAILABILITY_ENSURE_URL` or `ELASTOS_AVAILABILITY_PROVIDER_CONFIG`; if
no target is configured, the provider is not registered and publish remains
honestly local. Public gateway installer publishing now also uses
`elastos://content/publish` instead of direct IPFS. The protected-content
foundation now adds shared sealed-object schemas, a fail-closed `drm-provider`
for `elastos://drm/open`, a fail-closed `rights-provider` for typed access
questions, a fail-closed `key-provider` for key release, a fail-closed
`decrypt-provider` for decrypt/render sessions, a canonical
`drm-provider.status.required_sequence` for protected-content opens, a
machine-readable fail-closed `open` response that repeats that sequence,
Runtime-owned receipt/audit steps in that sequence, and typed `chain-provider`
rights reads. The content provider now validates
`sealed` object publishes against the sealed descriptor, required graph links,
and protected-content algorithm allowlists. The
remaining work is:

- Library and share metadata should display object identity and availability without exposing raw backend details.
- Wire sealed-object publish/open to rights, PQ-hybrid key release, and decrypt/render providers without exposing raw CEKs or backend SDKs to app capsules.
- Move remaining release artifact uploads off explicit operator IPFS tooling when the release pipeline can use the same content-provider contract without losing install compatibility or build/release proof.
- MicroVM materialization paths should consult availability state and repair/fetch through explicit operator/provider-plane tooling.
- Raw gateway, Kubo RPC, IPFS Cluster, and Elacity SDK authority should stay unavailable to normal capsules.

Configured provider shape:

```bash
ELASTOS_AVAILABILITY_ENSURE_URL=https://your-supernode.example/availability/ensure
ELASTOS_AVAILABILITY_PROVIDER_ID=elacity-supernode
ELASTOS_AVAILABILITY_AUTHORIZATION='Bearer ...'
```

or:

```bash
ELASTOS_AVAILABILITY_PROVIDER_CONFIG='{"targets":[{"id":"elacity-supernode","ensure_url":"https://your-supernode.example/availability/ensure"}]}'
```
