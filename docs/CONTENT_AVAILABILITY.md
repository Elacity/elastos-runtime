# Content Availability and IPLD

> Architecture contract and implementation direction.
>
> Current behavior has a first `content` provider seam that delegates to
> `ipfs-provider` for local CID creation/pinning, blocks normal capsule
> capability requests to `elastos://ipfs/*`, and records signed local
> availability receipts with peer-selection/quota/repair-worker metadata. This
> document defines where content addressing, IPLD, IPFS/Kubo, Carrier, and
> future replication providers belong as the SmartWeb content plane matures.

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
| Carrier | endpoint-authenticated peer discovery, coordination, and object/message/stream transport |
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

Those are Runtime, Carrier, availability, rights, custody, decrypt, and Chain
provider responsibilities.

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

The planned content-distribution contract treats games and model files as
content capsules. Home `Get` will resolve their signed bundle CID through
Runtime and the content provider. A CID proves
content identity but not current availability, so a normally Gettable entry
also needs an honest availability basis. The trusted publisher may be the only
bootstrap source while replication matures without changing the CID or creating
an HTTP fallback identity. See
[Content capsule distribution](CONTENT_CAPSULE_DISTRIBUTION.md).

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
    "peer_selection": {
      "mode": "single_local",
      "live_multi_peer_proof": false
    },
    "quota": {
      "policy": "not_enforced",
      "scope": "local_content_backend"
    },
    "repair_worker": {
      "scheduled": false,
      "status": "not_scheduled"
    },
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
It also appends an auditable `elastos.content.repair-task/v1` ledger under the
same system-owned content state, so local-only, queued, healthy, and retired
availability work is durable instead of only request-local. Local-only receipts
explicitly state that there is no live multi-peer proof and no enforced quota. A
configured availability provider may return richer `peer_selection`, `quota`,
and `repair_worker` metadata, but that metadata is not replication proof until
the provider proves live multi-peer availability.

The built-in Carrier availability provider enforces replica-count ceilings when
selecting remote replication candidates and records a quota verdict in signed
availability receipts: `enforced`, `effective_max_replicas`, `used_replicas`,
and `within_quota`/`at_quota`/`requirements_exceed_quota` status. That is
bounded provider-plane quota enforcement for this proof path. Signed receipts
also include `elastos.content.accounting/v1` metadata with observed local
file/byte counts and replica-byte estimates when the provider operation exposes
that data. Signed receipts also include `elastos.content.abuse-controls/v1`
metadata; the built-in Carrier path records candidate limits, attempted remote
provider invocations, failure counts, and whether the local attempt cap
throttled candidates. Configured federated quota-ledger and abuse-control
exchanges can now gate remote admission preflight, but production storage quota
networks, billing, network-wide banlists/rate policy, abuse markets, and signed
cross-runtime peer reputation/attestation policy remain separate production
work.
Candidate selection is deterministic and locally scored from signed announcement
metadata plus bounded local success/failure history. Runtime startup loads and
persists that local peer reputation under system content state, and redacted
score/reason plus local reputation fields are included in peer-selection
receipts. Federated cross-runtime reputation remains separate production
policy.

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
  "requirements": {
    "min_replicas": 1,
    "max_replicas": null,
    "require_live_multi_peer_proof": false
  },
  "local": {
    "status": "local_pinned",
    "provider": "ipfs-provider",
    "replicas": 1,
    "peer_selection": {
      "mode": "single_local",
      "live_multi_peer_proof": false
    },
    "quota": {
      "policy": "not_enforced",
      "scope": "local_content_backend"
    },
    "repair_worker": {
      "scheduled": false,
      "status": "not_scheduled"
    }
  }
}
```

A successful adapter may return `network_available` with a provider name and
replica count only when it also returns coherent `peer_selection`, `quota`, and
`repair_worker` metadata. Peer-selection metadata must name a concrete `mode`
or `strategy` so receipts do not sign anonymous availability claims.
Multi-replica claims must carry
`live_multi_peer_proof=true`; requested `min_replicas`, `max_replicas`, and
`require_live_multi_peer_proof` requirements are enforced before a signed
receipt can record network availability. Claims that miss those requirements are
recorded as `repair_needed`, not optimistic `network_available`. The external
availability-provider bridge also fails closed when a multi-replica upstream
claim omits peer-selection metadata.

The built-in Carrier availability provider signs and broadcasts CID
availability announcements on deterministic Carrier topics. When matching
signed remote announcements already exist, it treats those announcements as
candidate peers, invokes the remote peer's `content/ensure` over the Carrier
provider plane, falls back first to bounded remote `content/import_object` for
manifest-backed content objects, then to bounded remote `content/import_exact`
for file-like CIDs when remote pin cannot fetch the object, verifies the same
CID with remote `content/status`, and records `network_available` only when an
independent remote provider proves a live pinned replica. `import_object`
reconstructs the directory/object from the provider-owned object manifest and
listed file bytes; `import_exact` accepts bytes through a provider stream
envelope. Both fail closed unless the remote low-level content backend produces
the exact requested CID; mismatched imports are unpinned. Successful remote
`ensure`, `import_object`, or `import_exact` responses may also carry the remote
content provider's signed availability receipt; Carrier verifies and summarizes
that receipt in the peer-selection proof when present, including safe
peer-selection, quota, repair-worker, and accounting posture without exposing
raw provider internals.
If only the local pin is proven, the honest state is
`carrier_announced`; if requested replica/live-proof requirements are not met,
the state is `repair_needed`. Repair-only local announcements omit fetch
descriptors and are ignored as replication/fetch candidates. This is the seam
where Carrier, Elacity, supernode, volunteer, or later market-backed
availability providers should plug in without becoming capsule-visible
IPFS/Kubo/SDK authority.

`content-provider` also exposes a provider-only `repair_worker` operation. It
requires the Runtime provider invocation envelope, so app capsules cannot call
the autonomous worker directly through the capsule-facing content contract. The
worker reads the latest durable repair tasks, retries queued CIDs through the
same local pin plus availability-provider ensure path, writes a fresh signed
receipt, updates the repair task, and returns an explicit
`content_repair_worker_guardrail` quota/abuse-control receipt with run limits,
attempt budgets, and failure throttling state. Operators trigger the same path
with `elastos content repair-worker`, which calls the provider through Runtime
`ProviderInvocation` instead of raw provider JSON. Servers can also enable the
same bounded loop with `ELASTOS_CONTENT_REPAIR_SCHEDULER=true`; it is opt-in,
minimum-interval guarded, and uses the same run limit, attempt budget, and
failure budget controls as the manual worker. Worker runs and provider status
also expose an `elastos.content.repair-fleet/v1` policy/status surface. The
current repair fleet is intentionally scoped to a single Runtime:
`content-provider` is both the coordinator and local worker, scheduling is
driven by the durable repair task ledger, and external repair fleets,
storage-market admission, and settlement are explicitly reported as not
configured.
Provider status, per-CID status, and repair-worker runs also expose
`elastos.content.network-abuse-policy/v1`. That surface ties the existing signed
abuse-control receipts to one operator-visible policy: Runtime provider
invocation is required, Carrier provider-invocation candidate caps and remote
admission preflight are local guardrails, repair-worker attempt/failure budgets
are visible, and a configured
`ELASTOS_CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_*` endpoint or bounded
endpoint quorum can enforce signed external abuse-control admission exchange.
Production network-wide throttles, banlists, and abuse ledgers remain outside
the current provider-local policy plane.
Provider-wide status also includes `elastos.content.operator-dashboard/v1`, a
derived operator view over the signed receipt, repair-task, and
storage-accounting ledgers. It reports storage pressure, top principals by active
content bytes, replica-byte estimates, quota-exceeded records, fleet-history
attempts, recent repair rows, live-proof counts, and a non-production federation
posture without exposing raw backend, Carrier peer, or market authority.
Carrier peer selection and content status also expose
`elastos.carrier.peer-reputation/v1` policy metadata. Current scoring uses local
Runtime success/failure history only; content status aggregates whether local
history was applied, whether reputation was not reported, and whether any
federated-policy receipts were observed. Signed cross-runtime reputation
attestations are explicitly not configured.
Carrier peer selection, redacted remote receipt summaries, provider proof
summaries, and the operator dashboard also expose
`elastos.carrier.peer-attestation-exchange-policy/v1`. The current policy
records signed availability announcements, verified remote content receipts,
remote provider proofs, and local Runtime reputation as available. When
`ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_*` is configured, Carrier posts a
signed `elastos.carrier.peer-attestation.exchange-request/v1` with redacted
live remote proof summaries to either one operator-owned exchange endpoint or a
bounded configured endpoint set with an explicit quorum after remote replica
proof succeeds. Accepted endpoint responses must include a signed
`elastos.carrier.peer-attestation.exchange-receipt/v1`; Carrier verifies the
receipt signature/domain, records endpoint receipts and quorum counters, and
marks the exchange accepted only when configured quorum accepts. Third-party
attestations, revocation, and production fleet-wide reputation policy remain
explicitly not configured.
Storage-market receipts and provider-wide status also expose
`elastos.content.storage-settlement-policy/v1`. The current policy records
pricing, escrow, payment settlement, SLA enforcement, storage-market admission,
and cross-provider escrow as `not_configured`; this is explicit operator
posture, not live settlement execution.
Storage-market receipts and provider-wide status also expose
`elastos.content.storage-market-admission-policy/v1`. The current policy records
the local principal quota ledger and bounded remote `content/admission`
preflight as proof-path admission. The remote preflight now carries a signed
`elastos.content.admission/v1` receipt, and Carrier rejects unsigned or
payload-mismatched admission before bytes or DAG repair data move. When
`ELASTOS_CONTENT_STORAGE_MARKET_ADMISSION_*` is configured, `content/admission`
also calls either one external storage-market admission endpoint or a bounded
configured endpoint set with an explicit quorum before remote bytes or DAG
repair data move. The accepted quorum decision is normalized into
`elastos.content.storage-market-admission.decision/v1`, credential details are
redacted from status, and rejection, malformed response, endpoint failure, or
quorum failure fails closed into the signed admission receipt. Price discovery,
SLA admission, settlement/escrow, and economic abuse controls remain explicitly
not configured.
Quota receipts and provider-wide status also expose
`elastos.content.federated-quota-ledger-policy/v1`. The current policy records
the durable local per-principal storage-accounting ledger and remote
`content/admission` preflight with signed admission receipts as available. When
`ELASTOS_CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_*` is configured,
`content/admission` posts a signed
`elastos.content.federated-quota-ledger.exchange-request/v1` to either one
operator-owned quota-ledger endpoint or a bounded configured endpoint set with
an explicit quorum before remote bytes or DAG repair data move. Accepted
endpoint responses must include a signed
`elastos.content.federated-quota-ledger.exchange-receipt/v1`; the provider
verifies receipt signature/domain/schema, records endpoint receipts and quorum
counters in the signed admission receipt and quota-ledger policy, and rejects
admission fail-closed on configured quorum failure, malformed signed receipt,
timeout, or transport failure. Production independent provider-network
quota-ledger federation and production storage-admission networks remain
explicitly not configured.
Network-abuse policy status also records configured federated abuse-control
exchange posture. When
`ELASTOS_CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_*` is configured,
`content/admission` posts a signed
`elastos.content.federated-abuse-control.exchange-request/v1` to either one
operator-owned abuse-control endpoint or a bounded configured endpoint set with
an explicit quorum after local principal quota accepts and before quota-ledger,
storage-market, remote byte, or DAG repair movement. Accepted endpoint
responses must include a signed
`elastos.content.federated-abuse-control.exchange-receipt/v1`; the provider
verifies receipt signature/domain/schema, records endpoint receipts and quorum
counters in the signed admission receipt, and rejects admission fail-closed on
configured quorum failure, malformed signed receipt, timeout, transport
failure, or receipt verification failure. Production network-wide banlists,
cross-provider rate limits, and abuse ledger federation remain explicitly not
configured.
Provider-wide status, repair-worker runs, and the operator dashboard also expose
`elastos.content.external-repair-fleet-policy/v1`. The current policy records the
provider-owned local repair worker/scheduler as available. When
`ELASTOS_CONTENT_EXTERNAL_REPAIR_FLEET_*` is configured, the Runtime-gated
`repair_worker` can dispatch due tasks to either one operator-owned external
repair fleet endpoint or a bounded configured endpoint set with an explicit
quorum using `elastos.content.external-repair-fleet.dispatch-request/v1`;
responses are normalized into
`elastos.content.external-repair-fleet.dispatch-receipt/v1` with endpoint
receipts and quorum counters.
Worker attestation receipts, fleet settlement, and repair SLAs remain explicitly
not configured.
Provider-wide status and the nested operator dashboard also expose
`elastos.content.federated-operator-alerting-policy/v1`. The current policy
records provider-local status JSON, storage-pressure signals, repair-task
pressure, live-proof counters, remote-receipt counters, and optional
provider-local operator alert sink plus configured federated alert-exchange
posture as available through Runtime provider invocation. Operators may request
an `elastos.content.operator-alert/v1`
payload by calling provider-wide `content/status` with
`emit_operator_alert: true`. The provider always records a durable
`elastos.content.operator-alert.receipt/v1` outbox entry; when an operator sink
is configured, it also posts the alert to one HTTPS or loopback HTTP endpoint
without exposing sink credentials to apps or status JSON. When
`ELASTOS_CONTENT_FEDERATED_OPERATOR_ALERT_EXCHANGE_*` is configured, the same
Runtime-gated alert emission also posts an
`elastos.content.federated-operator-alert.exchange-request/v1` payload to one
operator-owned federated alert-exchange endpoint, requires an explicit
`accepted` decision, and records the normalized
`elastos.content.federated-operator-alert.exchange-receipt/v1` inside the same
durable operator-alert receipt. Cross-provider dashboards, peer-health
subscriptions, fleet-wide SLA policy, and operator UI remain explicitly not
configured.

The standalone `availability-provider` capsule preserves the same proof
metadata when an explicitly configured Elacity/supernode-compatible target
reports availability: `storage_market`, `repair_graph`, and `abuse_controls`
pass through to Runtime validation, and absent fields default to explicit
no-market / target-report-only posture. It also continues past `repair_needed`
configured targets and can aggregate multiple configured target reports into a
bounded `configured_availability_target_fanout` proof when min-replica or
live-proof requirements demand it. Max-replica quota still fails closed. This
keeps external target responses machine-readable without granting apps raw
target credentials or claiming a production storage market.

Every signed availability receipt also projects into a durable
`elastos.content.storage-accounting.ledger/v1` JSONL ledger keyed by CID and
grouped by publisher principal. Publish, exact import, and manifest-object
import can enforce `availability_requirements.max_storage_bytes_per_principal`
from that ledger before bytes enter the local backend; rejected operations fail
with `storage_quota_exceeded` before IPFS/Kubo provider calls, while accepted
operations record `principal_storage_quota` posture in the signed receipt and
ledger. `content/status` without a CID returns a provider-owned
`elastos.content.availability.dashboard/v1` summary of the same latest receipt,
storage-accounting, and repair-task ledgers: tracked object counts, status
counts, provider counts, quota verdict counts, accounting byte/file counters,
per-principal active/tracked objects, content bytes, replica-byte estimates,
no-settlement storage-market policy metadata, storage-market admission policy
metadata, abuse-control counters, live
proof counts, remote replica and verified remote receipt counts, capped recent
remote-replica proof rows with redacted peer-selection score/reason and
local-runtime reputation, queued/due/healthy repair counts, recent repair
tasks, scheduler posture, repair-fleet task pressure, and network-abuse policy
posture. The same response includes the derived operator dashboard so storage
pressure and fleet history are visible without scraping lower-level ledgers.
Peer reputation and peer-attestation exchange policy posture are included in
proof summaries and recent remote replica rows. Federated operator alerting
policy posture is included at the provider-wide level and inside the operator
dashboard so alert readiness is machine-readable instead of buried in release
notes. The same status operation
can emit a provider-local operator alert receipt and optional configured webhook
delivery plus optional configured federated alert-exchange delivery when
explicitly requested. Peer-attestation exchange policy posture is included in
Carrier peer selection, provider proof summaries, and the operator dashboard so
live peer proof is not confused with a production cross-runtime trust network.
Carrier availability also emits `elastos.content.repair-graph/v1` metadata.
Object-manifest and exact-byte repair remain bounded fallbacks, while arbitrary
`ipld_dag` repair now routes through the Runtime-only `elastos://block-graph/*`
provider path: Carrier asks local `content-block-graph-provider.export_graph`,
then invokes remote `content-block-graph-provider.import_graph` over the
provider plane. The provider uses the local `ipfs-provider` Kubo coordination
file to exchange bounded DAG CAR bytes and pin the imported root. If Kubo or
the provider is absent, the path fails closed and still refuses object/exact
fallback.
Operators can inspect the same availability/storage state with `elastos content status` for the
provider-wide dashboard or `elastos content status --cid <cid>` for a single
object; both commands route through Runtime provider invocation and expose the
provider status JSON without granting app capsules raw backend authority.
With the built-in Carrier provider, that retry can execute the same remote
`content/admission` + `content/ensure` + exact-import fallback +
`content/status` proof path against signed remote announcements. Remote
admission is a provider-only preflight: the remote content provider evaluates
projected principal storage quota and returns an
`elastos.content.admission/v1` receipt before Carrier sends exact bytes, object
manifests, or block-graph repair data. Carrier verifies that signed receipt and
its payload/CID binding before trusting the admission decision. When a live
multi-peer proof is required, Carrier keeps at least one remote provider
invocation in the candidate budget when quota permits it, even if the reported
local replica count already satisfies `min_replicas`.
Verified remote content-provider receipts are summarized with safe
peer-selection replica counts plus capped redacted score/reason/local
reputation rows with explicit cap/truncation metadata, admission, quota,
repair-worker, accounting, and abuse-control posture. This is
provider-mediated autonomous cross-peer repair for announced Carrier peers; it
is not yet a complete global storage market.
External availability providers still own production peer admission across
independent provider networks, production independent provider-network
quota-ledger federation beyond the configured bounded endpoint quorum,
pricing/escrow/settlement, actual
federated network abuse throttles/banlists/abuse ledgers beyond the configured
bounded abuse-control endpoint quorum, production repair execution across
Carrier/supernode/volunteer fleets beyond the current external-fleet
policy-status receipts, production peer reputation trust policy, third-party
attestations, revocation, and fleet-wide reputation exchange beyond the
configured Carrier peer-attestation endpoint quorum,
production storage-market admission/execution beyond the current signed
admission proof path, configured storage-market endpoint-quorum admission gate,
and storage-market-admission policy-status receipt, live
settlement/escrow execution
beyond the current storage-settlement policy-status receipt, and
production federated storage dashboards/UI, peer-health subscriptions, and
fleet-wide alert policy beyond the current provider-local alert sink plus
configured alert-exchange endpoint.

Fetch uses the same seam in reverse. `content-provider` tries the local CID
backend first; if the local cache misses, it asks the internal availability
provider for the same CID/path. Fetch requests may include a Runtime
provider-transfer contract:

```json
{
  "range": { "start": 0, "end": 65535 },
  "progress": {
    "request_id": "content-fetch:...",
    "expected_bytes": 65536
  }
}
```

The content provider passes that contract to the local `ipfs-provider` read or
availability-provider fallback read, lets the Runtime provider registry enforce
bounded byte-range slicing on the provider response, and returns the typed
`elastos.provider.transfer/v1` receipt with source, target, capability,
transport, range, and progress metadata in the fetch response. This is still a
bounded JSON/base64 byte path, not the final streaming ABI.

Carrier has a typed internal
`content_fetch` byte operation on its file ALPN: a connected Runtime peer can
request a CID/path and the serving Runtime reads bytes through its local
`ipfs-provider`. That operation is still Runtime-internal; normal capsules do
not receive raw Carrier tickets, peer handles, or Kubo authority. It remains a
narrow compatibility/bootstrap path.

Internal `content-provider` calls use the Runtime provider-to-provider
invocation envelope for IPFS and availability effects. The envelope validates
source, target, operation, transfer class, byte range, and progress receipt
metadata. The provider plane has explicit local and Carrier transports:
Carrier `provider_invoke` runs over the Carrier ALPN with the same
`elastos.provider.invocation/v1` envelope and hides raw connect tickets from
receipts. Its current provider target allowlist includes `content`,
`availability`, and the provisional `rights`, `key`, `decrypt`, and `drm`
labels. It does not yet include a custody route. The target cutover will retain
the envelope while replacing the provisional protected-content labels with
Runtime-selected `rights`, `custody`, and `decrypt` providers rather than raw
backends.
`ProviderTransfer::Stream` now carries validated
`elastos.provider.stream/v1` base64 chunks with range/progress receipts, and
`content-provider` fetch opens that stream envelope as a Runtime-owned session
for local IPFS or availability-provider reads. The session exposes read-next
backpressure, live progress events, and cancel support without exposing raw
provider or Carrier handles to apps.
The built-in Carrier availability provider embeds internal fetch descriptors in
signed CID availability announcements. On a local cache miss it verifies
matching signed announcements, extracts the Carrier fetch ticket internally, and
uses `carrier-provider-plane` `provider_invoke` to call remote
`content/fetch` with `local_only: true` and `transfer: "stream"`, so remote
peers serve from their own content provider without recursive availability
loops or app-visible backend authority. The built-in availability provider
decodes the returned stream envelope before returning the normal byte response.
For replication, the same provider plane now calls remote `content/ensure`,
remote `content/import_object` when a manifest-backed object needs
provider-owned reconstruction, remote `content/import_exact` when a file-like
CID needs byte-push fallback, and remote `content/status`; `network_available`
requires that remote proof.
Verified remote content availability receipts are included in the proof
metadata when the remote content provider returns them, including safe
peer-selection, quota, repair-worker, and accounting posture; selected remote
replicas also include redacted local score and selection reason. No-CID
`content/status` returns the provider-owned availability dashboard from the same
signed receipt and repair-task ledgers, including quota verdict, live proof,
remote replica, verified remote receipt counters, capped recent remote-replica
proof rows, and local accounting counters.
Remaining work beyond this first proof path is arbitrary block-level/IPLD DAG
repair for non-manifest graphs, production scheduling policy beyond the current
opt-in bounded loop, richer remote/multi-peer dashboard/UI and federated
alerting surfaces beyond the provider status summary counters plus the
provider-local alert sink plus configured alert-exchange endpoint, external
storage quota enforcement/accounting markets beyond local receipt accounting,
network abuse policy beyond local provider-invocation guardrails,
historical/federated peer reputation beyond durable local scoring, and durable
remote peer-selection policy beyond verified receipt summaries and capped
dashboard rows.
`_runtime_invocation` and `_runtime_transfer` are
Runtime-owned fields; source providers cannot predeclare or spoof them in the
target request.

## Protected Content

Availability stores bytes. Rights decide who may use them.

Protected content should be encrypted before it is published. IPFS, Elacity,
supernodes, and volunteers can store encrypted blocks. In the intended open
path, Runtime will own the operation. It will verify the authenticated request,
ask `rights-provider` for typed policy evidence, select custody providers for
recipient-encrypted contributions, and create a scoped `decrypt-provider`
session. Carrier will transport only Runtime-selected endpoint traffic.

The current Library path still uses the provisional
`drm-provider -> rights-provider -> key-provider -> decrypt-provider` sequence.
Those providers remain fail closed without configured backends. They do not
implement or verify the intended custody path.

That keeps the core invariant simple:

```text
public CID can expose encrypted bytes
	valid rights holder gets scoped output
	unauthorized opener gets no decrypt session
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

This belongs behind the Carrier/provider plane. Carrier coordinates peers,
availability announcements, and eventually content transport/repair. The
content provider owns availability policy and receipts. IPFS/Kubo and
cluster-like systems are backend implementations for CID creation, pinning,
fetch, or replication underneath.

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
availability provider to upgrade publish/ensure results to
`carrier_announced`, `network_available`, or `repair_needed` when configured,
writes signed availability receipts with peer-selection/quota/repair-worker
metadata, routes
Documents, Share, Site, and provenance attestation writes through the same
content path, routes ordinary capsule publishes through content availability,
adds a first fetch operation for simple CID/path reads, falls back to an
availability provider on local fetch miss, adds a Carrier `content_fetch`
operation for connected Runtime peers to serve CID/path bytes from their local
content backend, uses Carrier `provider_invoke` for availability-provider peer
fetches, routes gateway CID file reads and share metadata/head reads through
that fetch path, adds local ensure/repair operations that re-pin a CID
or record `repair_needed`, records durable
`elastos.content.repair-task/v1` entries for local-only, queued, healthy, and
retired availability state, records `elastos.content.accounting/v1`
file/byte/replica-byte estimates in signed receipts and the no-CID status
dashboard, and exposes a Runtime-provider-only
`repair_worker` pass that retries queued CIDs through the same
provider-mediated repair/ensure path with bounded run limits, attempt budgets,
failure throttling, explicit local guardrail receipts, and an
`elastos content repair-worker` operator trigger plus an opt-in
`ELASTOS_CONTENT_REPAIR_SCHEDULER=true` server loop that both route through
Runtime provider invocation, and a provider-owned availability dashboard from
no-CID `content/status`,
injects
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
them as generic directories. The first external `availability-provider` capsule
can now be registered for configured Elacity/supernode-compatible targets by
setting `ELASTOS_AVAILABILITY_ENSURE_URL` or
`ELASTOS_AVAILABILITY_PROVIDER_CONFIG`. If no external target is configured,
the built-in Carrier availability provider signs and announces the CID on
Carrier, attempts remote provider-mediated replication proof from signed remote
announcements, uses bounded `content/import_exact` byte-push fallback for
file-like exact-CID imports and bounded `content/import_object` fallback for
manifest-backed content objects when remote pin cannot fetch, records
`network_available` only after remote `content/ensure`/`import_object` or
`import_exact` plus `content/status` proves a live independent replica,
verifies/summarizes remote content availability receipts when present, and otherwise records
`carrier_announced` or `repair_needed`; if Carrier is unavailable, publish
remains honestly local or `repair_needed`. Public gateway
installer publishing now also uses `elastos://content/publish` instead of
direct IPFS. Protected content composes Runtime coordination, typed rights
evidence, signed recipient authorization, immutable custody epochs,
recipient-encrypted node contributions, threshold reconstruction, node-local
claims and exact encrypted result replay. [Protected content](PROTECTED_CONTENT.md)
owns that contract; [state.md](../state.md) owns source and installed proof.
Provisional DRM, rights, key and decrypt capsules remain a separate retirement
surface. The content provider validates
`sealed` object publishes against the sealed descriptor, required graph links,
and protected-content algorithm allowlists. The
remaining work is:

- Replace the provisional protected-content DTO/provider surface atomically with
  Runtime-owned rights, custody, and decrypt orchestration. Do not expose raw
  CEKs, custody shares, provider routes, network locations, or backend SDKs to
  app capsules.
- Move remaining release artifact uploads off explicit operator IPFS tooling when the release pipeline can use the same content-provider contract without losing install compatibility or build/release proof.
- MicroVM materialization paths should consult availability state and repair/fetch through explicit operator/provider-plane tooling.
- Promote the first Carrier remote proof path into production-grade storage policy: production independent provider-network quota-ledger federation beyond the configured bounded endpoint quorum, repair-fleet worker attestations/SLA/settlement beyond the configured external dispatch endpoint quorum, live market pricing/escrow/settlement beyond the configured storage-market endpoint-quorum admission gate and current storage-settlement policy-status receipt, actual federated network abuse throttles/banlists/abuse ledgers beyond the configured bounded abuse-control endpoint quorum, richer remote storage receipt policy, production peer reputation trust policy/third-party attestations/revocation beyond the configured Carrier peer-attestation endpoint quorum, and live federated dashboard/UI/peer-health subscriptions beyond the current provider-local dashboard plus configured alert-exchange endpoint.
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

Optional provider-local operator alert sink:

```bash
ELASTOS_CONTENT_OPERATOR_ALERT_URL=https://ops.example/content-alerts
ELASTOS_CONTENT_OPERATOR_ALERT_AUTHORIZATION='Bearer ...'
ELASTOS_CONTENT_OPERATOR_ALERT_TIMEOUT_SECS=5
```

or:

```bash
ELASTOS_CONTENT_OPERATOR_ALERT_CONFIG='{"url":"https://ops.example/content-alerts","authorization":"Bearer ...","timeout_secs":5}'
```

The sink is operator-owned and provider-local. `https://` is required unless the
target is loopback `http://127.0.0.1`, `http://localhost`, or `http://[::1]`.
Credentials are accepted only through provider configuration and are redacted
from status. Alerts are sent only when the operator explicitly asks for
provider-wide status with `emit_operator_alert: true`; every request writes a
receipt to the provider-owned alert outbox even when no sink is configured.

Optional federated operator alert exchange:

```bash
ELASTOS_CONTENT_FEDERATED_OPERATOR_ALERT_EXCHANGE_URL=https://ops.example/alerts/exchange
ELASTOS_CONTENT_FEDERATED_OPERATOR_ALERT_EXCHANGE_AUTHORIZATION='Bearer ...'
ELASTOS_CONTENT_FEDERATED_OPERATOR_ALERT_EXCHANGE_TIMEOUT_SECS=5
```

or:

```bash
ELASTOS_CONTENT_FEDERATED_OPERATOR_ALERT_EXCHANGE_CONFIG='{"url":"https://ops.example/alerts/exchange","authorization":"Bearer ...","timeout_secs":5}'
```

The exchange endpoint is operator-configured, provider-owned, and invoked only
from explicit provider-wide status alert emission. `https://` is required unless
the target is loopback `http://127.0.0.1`, `http://localhost`, or
`http://[::1]`. The response must include an `accepted` boolean and may include
`exchange_id`, `receipt_id`, and `reason`. Malformed responses, timeouts, and
transport failures produce failed federated exchange receipts without exposing
endpoint credentials.

Optional Carrier peer-attestation exchange:

```bash
ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_URL=https://attest.example/peer-attestation/exchange
ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_AUTHORIZATION='Bearer ...'
ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_TIMEOUT_SECS=5
```

or:

```bash
ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_CONFIG='{"url":"https://attest.example/peer-attestation/exchange","authorization":"Bearer ...","timeout_secs":5}'
```

or a bounded endpoint quorum:

```bash
ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_CONFIG='{"quorum":2,"endpoints":[{"id":"attest-a","url":"https://attest-a.example/peer-attestation/exchange","authorization":"Bearer ...","timeout_secs":5},{"id":"attest-b","url":"https://attest-b.example/peer-attestation/exchange","authorization":"Bearer ...","timeout_secs":5}]}'
```

The endpoint set is operator-configured, provider-owned, capped at five
endpoints, and invoked only by `carrier-availability` after it has live remote
provider proofs to attest. `https://` is required unless the target is loopback
`http://127.0.0.1`, `http://localhost`, or `http://[::1]`. Carrier signs the
exchange request and requires accepted endpoint responses to include a signed
exchange receipt; configured quorum failure, malformed responses, failed receipt
verification, timeouts, or transport failures produce failed
attestation-exchange policy receipts without exposing connect tickets or
endpoint credentials.

Optional federated quota-ledger exchange:

```bash
ELASTOS_CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_URL=https://quota.example/exchange
ELASTOS_CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_AUTHORIZATION='Bearer ...'
ELASTOS_CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_TIMEOUT_SECS=5
```

or:

```bash
ELASTOS_CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_CONFIG='{"url":"https://quota.example/exchange","authorization":"Bearer ...","timeout_secs":5}'
```

or a bounded endpoint quorum:

```bash
ELASTOS_CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_CONFIG='{"quorum":2,"endpoints":[{"id":"ledger-a","url":"https://quota-a.example/exchange","authorization":"Bearer ...","timeout_secs":5},{"id":"ledger-b","url":"https://quota-b.example/exchange","authorization":"Bearer ...","timeout_secs":5}]}'
```

The endpoint set is operator-configured, provider-owned, capped at five
endpoints, and invoked only from `content/admission` after the local principal
quota preflight accepts and before Carrier transfers remote bytes or DAG repair
data. `https://` is required unless the target is loopback
`http://127.0.0.1`, `http://localhost`, or `http://[::1]`. The provider signs
`elastos.content.federated-quota-ledger.exchange-request/v1`; accepted
responses must include a signed
`elastos.content.federated-quota-ledger.exchange-receipt/v1`. Rejection,
configured quorum failure, malformed signed receipt, timeout, transport
failure, or receipt verification failure rejects admission fail-closed without
exposing endpoint credentials.

Optional federated abuse-control exchange:

```bash
ELASTOS_CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_URL=https://abuse.example/exchange
ELASTOS_CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_AUTHORIZATION='Bearer ...'
ELASTOS_CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_TIMEOUT_SECS=5
```

or:

```bash
ELASTOS_CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_CONFIG='{"url":"https://abuse.example/exchange","authorization":"Bearer ...","timeout_secs":5}'
```

or a bounded endpoint quorum:

```bash
ELASTOS_CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_CONFIG='{"quorum":2,"endpoints":[{"id":"abuse-a","url":"https://abuse-a.example/exchange","authorization":"Bearer ...","timeout_secs":5},{"id":"abuse-b","url":"https://abuse-b.example/exchange","authorization":"Bearer ...","timeout_secs":5}]}'
```

The endpoint set is operator-configured, provider-owned, capped at five
endpoints, and invoked only from `content/admission` after local principal quota
accepts and before any later quota-ledger, storage-market, byte-transfer, or
repair-graph movement. `https://` is required unless the target is loopback
`http://127.0.0.1`, `http://localhost`, or `http://[::1]`. The provider signs
`elastos.content.federated-abuse-control.exchange-request/v1`; accepted
responses must include a signed
`elastos.content.federated-abuse-control.exchange-receipt/v1`. Rejection,
configured quorum failure, malformed signed receipt, timeout, transport
failure, or receipt verification failure rejects admission fail-closed without
exposing endpoint credentials.

Optional storage-market endpoint-quorum admission gate:

```bash
ELASTOS_CONTENT_STORAGE_MARKET_ADMISSION_URL=https://market.example/admission
ELASTOS_CONTENT_STORAGE_MARKET_ADMISSION_AUTHORIZATION='Bearer ...'
ELASTOS_CONTENT_STORAGE_MARKET_ADMISSION_TIMEOUT_SECS=5
```

or one explicit client config:

```bash
ELASTOS_CONTENT_STORAGE_MARKET_ADMISSION_CONFIG='{"url":"https://market.example/admission","authorization":"Bearer ...","timeout_secs":5}'
```

or a bounded endpoint quorum:

```bash
ELASTOS_CONTENT_STORAGE_MARKET_ADMISSION_CONFIG='{"quorum":2,"endpoints":[{"id":"market-a","url":"https://market-a.example/admission","authorization":"Bearer ...","timeout_secs":5},{"id":"market-b","url":"https://market-b.example/admission","authorization":"Bearer ...","timeout_secs":5}]}'
```

The endpoint set is operator-configured, provider-owned, capped at five
endpoints, and invoked only from `content/admission`. `https://` is required
unless the target is loopback `http://127.0.0.1`, `http://localhost`, or
`http://[::1]`. Each response must include an `accepted` boolean and may include
`status`, `reason`, `market_id`, `offer_id`, and `receipt`. Rejection,
malformed response, configured quorum failure, timeout, or transport failure
rejects the admission before Carrier transfers bytes or DAG repair data.

Optional external repair-fleet dispatch:

```bash
ELASTOS_CONTENT_EXTERNAL_REPAIR_FLEET_URL=https://repair.example/dispatch
ELASTOS_CONTENT_EXTERNAL_REPAIR_FLEET_AUTHORIZATION='Bearer ...'
ELASTOS_CONTENT_EXTERNAL_REPAIR_FLEET_TIMEOUT_SECS=5
```

or:

```bash
ELASTOS_CONTENT_EXTERNAL_REPAIR_FLEET_CONFIG='{"url":"https://repair.example/dispatch","authorization":"Bearer ...","timeout_secs":5}'
```

or a bounded endpoint quorum:

```bash
ELASTOS_CONTENT_EXTERNAL_REPAIR_FLEET_CONFIG='{"quorum":2,"endpoints":[{"id":"repair-a","url":"https://repair-a.example/dispatch","authorization":"Bearer ...","timeout_secs":5},{"id":"repair-b","url":"https://repair-b.example/dispatch","authorization":"Bearer ...","timeout_secs":5}]}'
```

The endpoint set is operator-configured, provider-owned, capped at five
endpoints, and invoked only by the Runtime-gated `content repair-worker` path
after local guardrails select a due repair task. `https://` is required unless
the target is loopback `http://127.0.0.1`, `http://localhost`, or
`http://[::1]`. Each response must include an `accepted` boolean and may include
`status`, `reason`, `fleet_id`, `job_id`, and `receipt`. Dispatch acceptance is
recorded only when configured quorum accepts as an external fleet
receipt, but local provider verification still decides final availability.
