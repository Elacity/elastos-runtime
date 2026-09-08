# Content capsule distribution

This document defines the intended distribution contract for CID-addressed
content. It covers free games, local model files, and other portable data.
Protected-content rights and
key release remain in [PROTECTED_CONTENT.md](PROTECTED_CONTENT.md), while
replication policy and availability receipts remain in
[CONTENT_AVAILABILITY.md](CONTENT_AVAILABILITY.md).

Runtime source projects installed capsules and can verify one locally supplied,
operator-pinned signed model catalog snapshot. Its typed preparation path can
admit that exact package through bounded local Content reads. Shared model
selection, retention controls, real-model packaging and network catalog updates
remain planned work.

The implemented content plane already provides `elastos://content` publish,
fetch, status, ensure, repair, and unpublish operations. It records signed local
availability receipts, keeps raw IPFS access system-only, and serves verified
gateway CID reads. These form the package-delivery foundation. The signed
network delivery and deployed package acceptance remain open.

## Decision

A game or model is a content capsule, not a service offer or a raw
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
| Installed inventory | Records admission, cached bytes, explicit Keep retention and provider readiness as distinct facts. |
| Install or removal receipt | Records the principal, exact CID, operation, result, and time. |
| Service offer | Advertises a running provider capability after installation; it is not package identity or install authority. |

Home, Library, Marketplace, System and Agent model pickers must
derive from these records. They must not maintain independent package databases
or turn display rows into authority.

## First vertical slice: use and retain local Qwen

The first delivery slice packages the currently verified Qwen GGUF as one
content capsule and makes it available to the existing local model provider.
This is required for the current closeout, not a later catalog-only milestone.
The person selects or uses the model; Runtime resolves and prepares its exact
content. Settings may offer Keep on this device or release local retention.
Transfer and installation are backend mechanisms, not a traditional download
workflow, file picker or editable model path. It uses this sequence:

1. The publisher creates a complete immutable closure with the content-capsule
   manifest, GGUF payload, format and quantization facts, resource requirements,
   license, provenance, and compatible model-provider interface.
2. The CID of that complete closure becomes the package identity. A publisher
   DID and signature authenticate the detached source claim for the exact CID;
   the hashed closure does not contain its own resulting CID.
3. Setup carries one trusted signed catalog-root CID or equivalent pinned
   signed head. Runtime verifies its CID and signature under configured
   publisher trust, then Home projects its signed entries. The initial
   availability basis may be the publisher alone when the entry states that
   limitation.
4. Home sends a typed selection/preparation intent for the exact catalog entry.
   Runtime checks principal authority and composes existing content `fetch`, `status`, and
   `ensure` operations with installed-inventory and provider-registration state.
   Preparation composes admission with existing transfer authority.
5. Runtime stages the fetched closure under a bounded private path, verifies the
   CID, publisher signature, availability evidence, size, resources, license,
   format, paths, and declared provider interface, then admits it atomically and
   writes the install receipt.
6. Runtime durably binds each running model offer to the exact admitted record
   and content CID. From that record, Runtime gives `model-provider` only the
   private canonical artifact descriptor that it revalidates before inference.
   Package identity remains separate from service-offer identity and install
   authority.
7. Marketplace Models browse/details/Use, System model management and existing
   Assistant/Home Agent pickers project the same catalog, inventory and offer
   records. They show availability, Preparing with progress, Ready, or an
   actionable failed, offline or incompatible state. Selection can prepare the
   exact model; inference waits for admission and provider readiness. A pin is
   retention, not evidence of trust, license acceptance or inference readiness.
   Runtime keeps paths and backend routes private. Selection preserves drafts
   and existing runs and never silently substitutes another model.

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
- Home transitions from available content through Preparing to Ready from Runtime facts
  and projects only the approved package name, CID, and status while Runtime
  keeps host paths and backend routes private;
- restart proof that preserves the single admitted record without another
  transfer, followed by one fresh model request that produces one inference
  through the admitted artifact, plus explicit release of local retention,
  busy/run protection, unpin policy, provider cleanup, removal receipt and
  partial-file cleanup; and
- installed negative tests for an incorrect CID, signature, publisher, digest,
  size, license, resource requirement, interface, availability claim, truncated
  transfer, and interrupted admission.

## Selection and retention flow

Selection requests Runtime preparation; the content provider owns transfer:

```text
signed catalog projection
-> person selects or uses the model
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

Runtime may cache bytes for ordinary on-demand use. Keep on this device requests
explicit local retention beyond that cache policy. Releasing Keep makes bytes
eligible for eviction; it does not immediately delete them. Actual free-space
removal waits for active references and run settlement, then updates inventory
and the removal receipt. Cached, kept, admitted and ready remain distinct facts
in the same inventory records. The signed catalog identity remains visible with
its actual availability after local eviction. Unpinning local bytes says nothing
about whether another provider retains the CID.

## Implemented catalog metadata profile

The optional `CapsuleManifest.model_content` block describes passive
`role=content`, `type=data` content. Its first-Qwen profile supports GGUF,
Q4_K_M, llama.cpp and `elastos.provider.model` version `0.1.0`. It requires
Apache-2.0 license references for the base and quantized model, a provenance
notice, bounded owner/repository identifiers and exact 40-character lowercase
Git revisions. The declared memory floor is 1 to 1,048,576 MiB. This profile
grants no execution, viewer, storage or provider capability. Other formats,
licenses and provenance schemes remain outside this closeout.

The existing `_elastos_object.json` owns file paths, exact sizes and SHA-256
digests. Catalog validation checks its ordered unique file list and content
digest, canonical `capsule.json` bytes, entrypoint and notice references. It
rejects traversal, aliases and missing references. Limits are 32 files, 256-byte
relative paths, a 64 KiB capsule manifest, 1 MiB per auxiliary file and 16 GiB
total declared content. Preparation verifies payload bytes and the complete
package CID before admission.

Operator configuration in `components.json.model_catalog` pins the exact raw
CIDv1/SHA-256 head and one to eight trusted publisher DIDs. Runtime reads the
fixed local `model-catalog.json` snapshot with a 128 KiB bound, verifies that
head and signature independently from entry claims, and accepts one current
signed entry. The entry names a canonical DAG-PB/SHA-256 package closure CID.
The existing authenticated catalog exposes verified publisher identity,
declared CID/size and model metadata as `unprepared`, with installed and
launchable both false. A same-name local directory does not establish admission.
Absent model configuration preserves ordinary installed inventory; invalid
model configuration marks only the model catalog unavailable. Snapshot
verification provides publisher metadata, while transfer, atomic admission,
provider readiness and deployed availability require their own proof.

## Implemented bounded local reads

The existing Content fetch request accepts strict `bounded_read: true` with a
closed byte range, or `max_bytes` for complete `_elastos_object.json` metadata.
These modes are mutually exclusive. Runtime routes them locally to native IPFS Cat. Each read has
a 64 KiB cap and a five-second total HTTP/body deadline. Native Cat borrows the
ready backend and refreshes its existing activity record. It uses encoded query
parameters with redirects and proxy inheritance disabled, and performs no
startup, pin, retry or fallback for this mode.

Runtime validates the private applied-range receipt against the exact CID,
relative path, range and byte count. It consumes the range once and removes
the receipt from Bytes and Stream output. Missing or conflicting receipts fail;
bounded Content failures do not enter ordinary availability fallback. Remote
bounded calls are outside this local contract.

Complete metadata uses a maximum of 64 KiB and requests at most that cap plus
one byte from Kubo. Success requires EOF within the cap. Runtime checks the
private CID/path/completed/length receipt and preserves the exact bytes,
including whitespace, before removing the receipt. Range or expected-length
claims cannot accompany this mode. Other provider operations retain ownership
of their own `max_bytes` fields.

Source tests prove deadline-driven socket closure, a distinct next read and the
existing bridge's serialized response drain. The read receipt establishes the
bounded transfer; preparation separately verifies file hashes and package CID.
One inventory worker owns preparation, status, cancellation and restart
reconciliation. It reserves the full staging/backend/index charge before a new
operation, checks aggregate budget and current authority, and keeps charges
until provider work drains or an uncertain admission is reconciled. Verified
staging enters admission by exclusive same-filesystem rename. Exact-CID reuse
retains one artifact charge with separate actor/request bindings.

Private native/Registry capacity observation uses the ready backend's actual
repository and same-volume datastores. It returns bounded facts and enforces the 10%
free-space floor. Current-owner backend directories may use `0755` with no
special or group/world write bits; staging remains owner-only `0700`.
This filesystem observation is separate from the inventory reservation.
Explicit isolated process tests exercise fresh Use through real Content,
Registry, the native bridge and offline Kubo, including atomic admission,
reopened-owner reuse without Content reads and cleanup. The signed synthetic
packages are seeded in local cache. Current proof and measurements are in
[state.md](../state.md); scaled cold delivery, continuous peak usage, exact
Qwen, inference and installed acceptance remain open.

## Source prerequisites and bounded implementation plan

The following separates implemented primitives from remaining package work:

| Existing surface | Current state and required extension |
| --- | --- |
| `elastos/crates/elastos-common/src/manifest.rs` | The bounded passive metadata profile above is implemented. Preparation must verify its declared facts against the complete fetched package before admission. |
| `elastos/crates/elastos-server/src/api/capsule_inventory.rs` and `gateway_capsule_catalog/read_model.rs` | The catalog projects installed inventory plus verified, unprepared model metadata. The preparation inventory now owns reservations and admission receipts. Shared admission/offer projections remain to be connected. |
| `elastos/crates/elastos-server/src/content.rs` | Preparation uses the explicit bounded local-fetch loop. Ordinary `fetch_bytes_via_provider` and `materialize_data_capsule` still drain whole files. `import_exact` and aggregate `import_object` remain capped at 64 MiB and 512 files; these are separate paths. |
| `elastos/crates/elastos-runtime/src/provider/registry.rs` | Bounded reads validate and consume the native range once for Bytes and Stream. Ordinary `open_provider_stream` still decodes the full response into `ProviderStreamSession.bytes`; consumer chunking alone does not bound producer memory or cancel network work. |
| `capsules/ipfs-provider/src/main.rs` | Explicit bounded Cat enforces finite bytes/time and uses the existing backend lifecycle. Ordinary `cat` and `cat_to_path` still read the entire file before encoding or writing. |
| `elastos/crates/elastos-server/src/api/gateway_site.rs` | Gateway CID reads buffer content before enforcing the 100 MiB file limit. Ordinary browser file responses are not a model-transfer route. |
| `server_infra.rs` and `capsules/model-provider/src/config.rs` | Offers come from private startup config; local descriptors contain path and SHA-256. Bind admitted content to those verified descriptors and the existing ProviderRegistry lifecycle, rather than giving Settings write access to config. |

Preparation must confirm the selected backend through Runtime and use bounded
reads for the exact CID, manifest-approved relative path, offset and length.
Keep one in-flight read, incremental integrity verification and observed
progress. Cancel stops further dispatch, then waits for the dispatched read and
its response drain before terminal cleanup. Verify the complete closure and
payload digests before admission; a range receipt alone cannot establish them.

Runtime owns one private staging operation for the exact package identity and
admission record. Bound catalog/manifest bytes, file count, each file, total
bytes and time from validated policy and exact package facts. Account for
backend pin storage, staging and final placement, preserving at least 10% free
space on every affected volume before and during preparation. Verify ownership,
mode, symlink/hard-link refusal and containment. Atomically admit only the full
verified closure; restart reconciles the same record and removes only its owned
partial staging. Retry neither duplicates admission nor repeats a completed
transfer. Cancellation and cleanup leave other packages and user data intact.

Proposed local commit order, after the separate onboarding/recovery and window
policy slices:

1. **Package admission contract.** The typed preparation, status/cancel and
   inventory path is implemented; shared retention intents remain open.
   Content fetch alone does not authorize installation.
   Inputs identify the exact catalog entry/CID or owned operation;
   Runtime derives principal, trust, provider and paths. Keep one inventory and
   its admission receipts, not an independent Store registry or second journal.
   Test canonical closure identity, signature/trust/revocation, size/license/
   provenance/compatibility rejection, caller isolation and bounded projections.
2. **Scaled transfer proof.** Extend the verified small-package production
   preparation proof to the intended model scale and cold delivery.
   Test a deterministic streamed fixture larger than old whole-file limits with
   bounded peak buffers, slow/oversized/ignored-range failures, mid-read cancel,
   retry, concurrent duplicate selection, low disk, crash/restart and exact
   partial cleanup. Include proof that provider work stops, not just UI progress.
3. **Admission-to-offer binding.** Reuse the installed engine receipt, private
   artifact verification, ProviderRegistry and model run journal. Derive offers
   from admitted records; reinitialize the existing provider only when idle and
   safe, preserving active and unknown-settlement runs. The current coordinator
   rejects a second Init, so this requires a guarded change within that owner.
   Its serialized coordinator must check the run journal and adapter workers
   and apply reinitialization in the same operation. Ordinary idle counts exclude
   terminal unknown-settlement runs; preserve those records and their artifact
   bindings until settlement is proved. Registry shutdown alone is not an idle
   admission gate. Keep one provider slot
   and the same journal across restart. Test absent/incompatible
   engine, tampered artifacts, duplicate admission, restart without transfer,
   busy retention release and exact CID-to-offer/run binding. This is not a new
   inference provider or a capsule-facing config operation.
4. **Shared model experience.** Add Models within existing Marketplace and model
   management within System; extend Assistant/Home Agent selectors. Test the
   same records across
   all views; Use/Keep/release intent shapes; Preparing, progress, cancel, retry,
   failed/offline/incompatible/ready states; selection while preparing; draft/run
   preservation; and inference disabled until the selected model is ready.
   Keep ordinary app catalog behavior and hosted configuration unchanged.
5. **Cold proof and publication review.** After source review, use the existing
   authorized isolated proof scope with a compatible fresh install and no
   model bytes or operator offer preconfiguration. Select the real signed Qwen
   entry, prepare, receive a real reply through the existing typed run contract,
   restart, reuse without transfer, release Keep and verify busy-safe eviction.
   Verify actual
   artifact/receipt parity and human behavior, then publish code/tests/docs/
   manifests only after explicit authorization. Passkey ceremonies require the
   person's action; GitHub publication, live changes, paid calls and destructive
   data operations retain explicit approval gates. Routine isolated proof does
   not require a new approval per step. Fixture proof remains separate.

The first package is the currently verified Qwen3.5-9B Q4_K_M with the existing
llama.cpp engine; `components.json` estimates 6170 MB, which is not an exact
signed closure size. Packaging must establish exact bytes, complete-closure CID,
publisher signature/trust, base and quantization licenses/provenance, resource
limits and real availability. The existing engine's platform/checksum receipt
and shared libraries must work on a genuinely fresh supported install; missing
engine or unsupported hardware yields an actionable incompatible/unavailable
state rather than using an ambient executable or another model. Engine delivery
uses the existing verified component mechanism, not a second UI downloader.
No production publisher, signed catalog CID or availability deployment is
invented by fixtures. Git contains code, tests, docs and reviewed manifests;
large model bytes and private publisher keys remain outside it. Remote hosted
inference publication, Jetson and mandates remain later work.

## Bootstrap while the network matures

Content should already be available from the declared ElastOS availability
network before it is presented as available for use. During bootstrap, the
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

Before a content capsule is shown as available for preparation or admitted, verify at least:

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
