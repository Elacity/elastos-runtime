# Content capsule distribution

This document defines the intended distribution contract for CID-addressed
content. It covers free games, local model files, and other portable data.
Protected-content rights and
key release remain in [PROTECTED_CONTENT.md](PROTECTED_CONTENT.md), while
replication policy and availability receipts remain in
[CONTENT_AVAILABILITY.md](CONTENT_AVAILABILITY.md).

Runtime source projects installed capsules and can verify one locally supplied,
operator-pinned signed model catalog snapshot. Its typed preparation path can
admit that exact package through bounded local Content reads. Marketplace and
System source views expose preparation and Keep controls. Assistant and Home
Agent select current ready offers and preserve optional content CID intent.
The isolated cold Qwen source/process proof covers admission, reply, active
cancellation, exact restart replay and Keep persistence with a fixture catalog
and successful authority-revalidation callback. Operator-signed packaging,
bounded local Content import and installed catalog visibility are verified.
The actual installed Use failed after metadata progress, before admission or
activation; its exact exception was not retained. That result and cleanup facts
are recorded in [state.md](../state.md#current-isolated-owner-home).
Combined GUI/retention acceptance, content-to-Agent handoff and bounded off-box
delivery remain open. This is a local foundation for full model distribution.

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
- The intended model consumer is Agent/chat, analogous to a video's Player.
  Marketplace is primary content discovery; System also manages models.
  Selection carries content identity. Runtime owns admission and run authority,
  and the model provider owns execution. The content capsule remains passive.
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
| Installed inventory | Records admission, cached bytes and explicit Keep retention. Runtime derives current dispatch readiness from admission and the provider offer. |
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
content and retains the selected local model for repeated use. System owns
explicit storage removal with recoverability checks, warnings and consent.
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
7. Marketplace content browse/details/Use, System model management and existing
   Assistant/Home Agent pickers project the same catalog, inventory and offer
   records. They show availability, Preparing with progress, Ready, or an
   actionable failed, offline or incompatible state. The intended Open handoff
   selects that model in Agent/chat and composes preparation with use; inference
   waits for admission and provider readiness. Current Open Models instead
   targets System and still requires the bounded adaptation below.
   A pin is retention, not evidence of trust, license acceptance or inference readiness.
   Runtime keeps paths and backend routes private. Selection preserves drafts
   and existing runs and never silently substitutes another model.

Off-box catalog and package delivery belongs to the existing Content and
availability contracts over Carrier. Runtime applies the same publisher,
identity and authority checks. The bounded preparation path currently dispatches
locally and excludes availability retrieval; this paragraph defines intended
distribution, not implemented remote model transfer.

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
  through the retained admitted artifact, plus explicit System storage removal,
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
explicit local retention beyond that cache policy. Releasing Keep removes only
the caller's retention claim. Bytes become eligible for eviction only when other
claims and references permit it; release does not immediately delete them. Actual free-space
removal waits for active references and run settlement, then updates inventory
and the removal receipt. Cached, kept, admitted and ready remain distinct facts
in the same inventory records. The signed catalog identity remains visible with
its actual availability after local eviction. Unpinning local bytes says nothing
about whether another provider retains the CID. Catalog identity, local retention,
reachable retained copies and inference placement are distinct. Releasing Keep
neither proves a reachable remote copy nor selects remote inference. The current
local-only preparation path cannot promise refetch after eviction.

Accepted UX policy, with implementation pending: ordinary Use retains the selected local model for
repeated use. System owns storage management and checks recoverability evidence
before an ordinary Free up space action. Possible loss of the sole copy requires
a clear warning and explicit consent. The Keep/release behavior described above
is current implementation evidence; it does not establish this accepted flow.
Remote inference remains a separately selected service.

## Implemented catalog metadata profile

The optional `CapsuleManifest.model_content` block describes passive
`role=content`, `type=data` content. Its first-Qwen profile supports GGUF,
Q4_K_M, llama.cpp and `elastos.provider.model` version `0.1.0`. It requires
Apache-2.0 license references for the base and quantized model, a provenance
notice, bounded owner/repository identifiers and exact 40-character lowercase
Git revisions. The declared memory floor is 1 to 1,048,576 MiB. This profile
grants no execution, storage or provider capability and currently rejects the
`viewer` field. Generic content manifests support that field as a handoff hint;
the model-specific restriction needs compatibility review. Other formats,
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
declared CID/size and model metadata, with installed and launchable both false.
The caller's admission and exact-offer dispatch readiness determine whether the
model state is unprepared, admitted or ready. A same-name local directory does
not establish admission.
Absent model configuration preserves ordinary installed inventory; invalid
model configuration marks only the model catalog unavailable. Snapshot
verification provides publisher metadata. Transfer, atomic admission and
dispatch readiness have separate source checks; inference and deployed
availability require target proof.

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
Isolated process tests exercise fresh Use through real Content, Registry,
the native bridge and Kubo, including cold exact-Qwen admission, reuse without
Content reads, a real reply, active cancellation and exact restart replay.
Keep persists and owned processes/staging are cleaned up. Cancellation records
unknown settlement when backend stop is unconfirmed. The catalog signer and
successful revalidation callback belong to the fixture. They do not prove the
installed Home grant/session path. The installed operator catalog and receipt
exist, but full Use failed; current evidence and limits are in [state.md](../state.md).
Resource samples do not establish continuous peaks or complete idle/busy
retention and eviction acceptance.

Catalog GET, typed catalog list and preparation status share one caller-scoped
projection of admission, Keep, progress and `dispatch_ready`. Runtime derives
readiness from the current signed entry and admission, safe artifact metadata,
engine receipt identity and exactly one matching offer from the ready local
model provider. The startup composer supplies the same offer identity and
public policy. The offer read has byte, count and time bounds; Runtime
revalidates the caller after it completes. Inventory snapshots preserve pending
recovery data and store no readiness result. Polling uses metadata observations
without reading model or engine payloads. Activation still verifies full
hashes, and the provider verifies payloads before each new engine start.
Dispatch readiness describes the current binding; an actual run proves
inference.

## Content-to-consumer review

The existing viewer relationship is identity and handoff metadata, not a grant
to execute or read content. Home validates the sending frame and target, and
Runtime issues and checks the consumer's launch context. Protected video passes
only mint identity to Player; its scoped Runtime operations supply playback
authority. Model selection should preserve the analogous separation between
content CID and the consumer's typed model-run authority.

| Existing path to reuse | Bounded change to review |
| --- | --- |
| `gateway_capsule_catalog/read_model.rs` already merges signed passive model entries into the capsule catalog. | Make Marketplace the primary content discovery path; retain System management and one catalog. Review the exact-one-entry validation and both helpers' at-most-one limit before broader choices. |
| Generic `CapsuleManifest.viewer`, viewer compatibility audit and Home open-target handoff describe a content consumer. | The model profile rejects `viewer`; the current audit also requires an installed viewer-role target with a content interface. Agent/Assistant use their own roles and model interfaces. Map compatible selection explicitly rather than removing checks or relabeling execution as passive content. |
| Library's protected-video handoff and Home launch context keep identity separate from authority. | Define the smallest exact-CID selection handoff into Agent/chat, preserving drafts, current launch binding and deliberate run intent. Current ready-only choices and Open Models to System do not provide this flow. |
| Runtime preparation inventory, private artifact descriptors and the existing model provider own admission, retention and execution. | Compose selection with preparation while retaining fail-closed admission, exact offer binding and cancellation/outcome ownership. The content manifest supplies no execution capability. |
| Content/availability own delivery; Carrier is the private off-box transport. | Adapt bounded local preparation reads to that delivery contract with finite byte/time/cancellation limits. Both the ordinary whole-file materializer and `viewer_gateway::viewer_content`, which reads the entrypoint into browser-delivered bytes, are unsuitable for model weights. Reuse handoff identity, not those byte paths. |

Review in this order:

1. Reproduce the first failed boundary with the actual package facts and Home
   authority revalidation. Preserve structured phase/error evidence through
   cleanup: safe bounded public status and detailed Runtime-private diagnostics.
   Determine the cause before changing behavior or retrying installed Use.
2. Map the existing contracts above for model-to-Agent selection and bounded
   Carrier-backed off-box Content. Retain passive content, publisher trust,
   consumer authority, the current registry and inventory.
3. Propose the smallest coherent adaptation and focused rejection tests. After
   review, require installed Marketplace-to-Agent selection, preparation and
   exact-offer/reply proof, then the existing retention/onboarding/window checks
   before publication. Local success and off-box delivery receive separate
   verdicts; Browser repair retains its own owner.

## Source prerequisites and bounded implementation plan

The following separates implemented primitives from remaining package work:

| Existing surface | Current state and required extension |
| --- | --- |
| `elastos/crates/elastos-common/src/manifest.rs` | The bounded passive metadata profile above is implemented, including the current viewer rejection. Review handoff compatibility separately from execution authority; preparation verifies facts against the complete fetched package. |
| `elastos/crates/elastos-server/src/api/capsule_inventory.rs` and `gateway_capsule_catalog/read_model.rs` | The catalog projects installed inventory plus signed model metadata and caller-scoped admission, Keep and dispatch readiness. The preparation inventory owns reservations and admission receipts. Marketplace/System consume these facts; Assistant/Home Agent match exact ready offers and preserve optional CID intent. |
| `elastos/crates/elastos-server/src/content.rs` | Preparation uses the explicit bounded local-fetch loop. Ordinary `fetch_bytes_via_provider` and `materialize_data_capsule` still drain whole files. `import_exact` and aggregate `import_object` remain capped at 64 MiB and 512 files; these are separate paths. |
| `elastos/crates/elastos-runtime/src/provider/registry.rs` | Bounded reads validate and consume the native range once for Bytes and Stream. Ordinary `open_provider_stream` still decodes the full response into `ProviderStreamSession.bytes`; consumer chunking alone does not bound producer memory or cancel network work. |
| `capsules/ipfs-provider/src/main.rs` | Explicit bounded Cat enforces finite bytes/time and uses the existing backend lifecycle. Ordinary `cat` and `cat_to_path` still read the entire file before encoding or writing. |
| `elastos/crates/elastos-server/src/api/gateway_site.rs` | Gateway CID reads buffer content before enforcing the 100 MiB file limit. Ordinary browser file responses are not a model-transfer route. |
| `api/model_provider_config.rs` and `capsules/model-provider/src/config.rs` | Startup and admission share the private config composer. Verified descriptors bind admitted content to the existing model-provider lifecycle. Runtime alone can refresh that configuration. |

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

Implemented source boundaries and remaining acceptance:

1. **Package admission contract.** The typed preparation, status/cancel and
   inventory path is implemented. `content.retention` accepts `{cid, keep}` for
   an admitted CID owned by the current principal. The same inventory holds one
   retention claim per principal/CID, shared across aliases; status shows only
   the caller's claim. Keep/release leaves admission and accounting unchanged.
   Content fetch alone does not authorize installation.
   Inputs identify the exact catalog entry/CID or owned operation;
   Runtime derives principal, trust, provider and paths. Keep one inventory and
   its admission receipts, not an independent Store registry or second journal.
   Test canonical closure identity, signature/trust/revocation, size/license/
   provenance/compatibility rejection, caller isolation and bounded projections.
2. **Bounded transfer.** Runtime checks current authority and outstanding charges
   before each read and observes backend capacity in bounded byte windows.
   Source tests cover slow/oversized/ignored-range failures, cancellation,
   duplicate selection, low disk and exact restart cleanup. The cold Qwen
   process proof exercises the full closure with fixture authority revalidation.
   Installed memory/disk observations
   and complete lifecycle acceptance remain separate from these fixtures.
3. **Retention closure.** Additive activation is implemented through the existing
   Init boundary. Runtime holds the existing inventory worker lock through startup
   Init/registration and refresh Init/result handling. The shared composer emits
   private provenance for verified admitted offers and preserves operator config;
   short inventory transactions remain available for status and Keep.
   Its serialized coordinator keeps one process, Registry slot
   and journal, preserves exact existing offers, and blocks additions while
   workers, cached engines or unresolved runs retain execution ownership.
   Identical Init is idempotent; unresolved bindings survive expiry and restart.
   Busy activation retains admitted files for retry without transfer. Provider
   close retains exact engine ownership until bounded reaping and process-group
   closure succeed; uncertainty blocks replacement. Provider-source tests verify
   retirement by exact offer ID and immutable configuration through private Init,
   preserving operator offers and historical results. Runtime stores the exact
   activation descriptor and pending retirement in the same inventory. It
   withdraws the offer before removing eligible unkept admission files, and
   reconciles withdrawal before startup registration. CapacityPending keeps
   pending Use identity with zero reservation; uncertain closure preserves
   files, ownership and accounting. Current run/Keep/reference checks protect
   busy or retained content. Installed full-Qwen idle/busy retention and safe
   eviction acceptance remain open.
4. **Shared model experience.** Marketplace Models and System management use one
   vendored presentation/intent helper and the existing typed content methods.
   Catalog rows carry nested readiness; operation replies carry flat readiness.
   Visible in-flight preparation has bounded polling; request and CID ownership
   reject stale replies. An unconfirmed Use keeps its request identity until a
   successful read reconciles it. Keep is a caller retention choice, not deletion.
   Assistant/Home Agent keep exact offer/CID intent in existing workspaces and
   use current unique ready mappings. Missing choices and failed refreshes
   preserve drafts and accepted runs; new dispatch requires current readiness
   and deliberate Send. Current Open Models uses the Home handoff to System;
   the content-to-Agent review above covers the intended discovery/use flow.
   Source fixtures cover the current boundaries. Verify the combined installed views,
   offline/incompatible states and busy-safe eviction with the exact model.
   Keep ordinary app catalog behavior and hosted configuration unchanged.
5. **Installed proof and publication review.** Reuse the operator-signed package,
   bounded local import and installed artifact receipts. Resolve the observed
   Use failure before retry; record fresh-install prerequisites separately from
   the existing owner Home. After reviewed adaptation, select the real signed
   Qwen entry in Marketplace and open Agent/chat, prepare, receive a real reply
   through the existing typed run contract,
   restart, reuse without transfer, and verify explicit recoverability-aware
   removal through System storage management with busy-safe eviction.
   The generic directory publisher currently reads whole files into a base64
   JSON array. Large-model bootstrap must use bounded operator/provider import
   or a separately verified publisher repair. Capacity admission covers the
   complete proof layout, including an additional publisher backend copy when
   used, while preserving the 10% free-space floor.
   Verify actual
   artifact/receipt parity and human behavior, then publish code/tests/docs/
   manifests only after explicit authorization. Passkey ceremonies require the
   person's action; GitHub publication, live changes, paid calls and destructive
   data operations retain explicit approval gates. Routine isolated proof does
   not require a new approval per step. Fixture proof remains separate.

The first package is the currently verified Qwen3.5-9B Q4_K_M with the existing
llama.cpp engine; `components.json` estimates 6170 MB, which is not an exact
signed closure size. The local operator package records exact bytes,
complete-closure CID, publisher signature/trust, licenses, provenance claims,
resource limits and local availability. Its verified scope and upstream
provenance limits are in [state.md](../state.md#current-isolated-owner-home).
That local package proof does not establish off-box distribution. The existing
engine's platform/checksum receipt
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
