# Object protection: implementation and migration guide

Status: staged implementation instructions for the target
[storage and access contract](STORAGE_AND_ACCESS.md). Wire compatibility and
product support require the acceptance evidence described below. Begin
implementation on the user-approved development base under
[AGENTS.md](../AGENTS.md).

## Objective and scope

Let Runtime protect and use an object through a stable semantic contract while
the selected key, rights, and decrypt mechanisms can change. Keep the existing
content provider for encrypted storage and availability. Keep the existing
Runtime authority system for permissions, sessions, and delegated effects.

The first extraction wraps current protected-media behavior. It covers the
creation side as well as access, because a decrypt-only wrapper leaves new
objects tied to the current custody format. General document storage, new
cryptography, group key protocols, and stronger execution environments are
later capabilities with their own acceptance gates.

Use a small internal interface in the existing protected-content Runtime
boundary. Add a crate or process only when an actual dependency or isolation
requirement justifies it. Reuse the existing registry, typed contracts, journals,
identity services, content path, and Runtime-owned Carrier dispatch. Keep one
selected execution path per operation. Test substitution with a fake backend
before implementing a second real cryptographic backend.

## Source entry points and evidence

Resolve the approved integration commit before editing; symbols and release
state can move. [Protected content](PROTECTED_CONTENT.md) describes the selected
protocol and its acceptance requirements. Read [state.md](../state.md) for
verified source and installed behavior. Record the exact implementation and
verification revisions there or in a dated evidence record.

| Source area on the development line | Reuse and inspect |
|---|---|
| `elastos/crates/elastos-protected-content-runtime/src/coordinator.rs` | `RuntimeRightsProvider`, `RuntimeCustodyProvider`, release journal and reconciliation. Keep custody-specific requests behind the adapter. |
| `elastos/crates/elastos-protected-content-runtime/src/open.rs` | `RuntimeDecryptProvider`, recipient preparation, opaque viewer sessions, read and close. The current open input includes purchase, custody, and CENC media fields. |
| `elastos/crates/elastos-server/src/protected_content_runtime.rs` | Existing protect, purchase, open, read, close, and cleanup orchestration. Move only responsibilities required by the contract; preserve session binding and effect ownership. |
| `elastos/crates/elastos-protected-content-provider-contracts/` | Strict provider decoding, versioning, bindings, bounded payloads, and typed failures. Reuse these conventions. |
| `elastos/crates/elastos-runtime/src/capability/manager.rs` | Grant validation, expiry, individual and epoch revocation, delegation. Prove revocation of already-issued child grants explicitly. |
| `elastos/crates/elastos-runtime/src/provider/registry.rs` | Runtime-only registration, provider readiness, dispatch, and lifecycle. |
| `capsules/protected-content-decrypt-provider/src/lib.rs` and `capsules/elacity-player/browser/player.js` | Current clear-media handoff and consuming viewer. Key confinement is distinct from protection against readable-content capture. |

Inspect how the capability manager records grant dependencies. If delegation
issues an independent token and validation checks only that token's revocation
state, parent revocation needs additional dependency handling. Carry this
dependency through the existing grant authority. dKMS consumes that authority
through the existing provider contracts.

Inspect the actual viewer output. A path that transfers decrypted media segments
to a browser player places that player and its host inside the trusted content
boundary. Classify that output as clear media; a protected render-only guarantee
requires separate evidence for the full execution and output path.

## Preserve the responsibility split

| Boundary | Responsibility |
|---|---|
| App, shell, or agent | Request a semantic action on an object and display permitted results. |
| Runtime | Derive caller identity; verify object, policy, and grant; select compatible providers; own operation lifecycle, cancellation, audit, and settlement. |
| Rights or authority adapter | Verify evidence from the object's accepted authority source, including group or commercial rules where applicable. |
| Protection backend | Protect revisions and establish bounded permitted-use sessions under validated authority. Own key material and protocol-specific work. |
| Execution or output provider | Enforce the selected consumer and output policy within its demonstrated trust boundary. |
| Content provider | Store, retrieve, retain, and repair encrypted content using the existing contract. |

These responsibilities can reuse existing services. An internal adapter may
compose existing providers. Runtime verifies provider evidence within the
caller's grant. Existing signed custody, rights, and payment checks retain their
own authority requirements.

## Minimum semantic contract

Define meanings before freezing names or codecs. The following operations are
illustrative internal operations, not new public endpoints:

| Operation | Required result |
|---|---|
| Protect a revision | A durable encrypted revision and authenticated protection descriptor, with creation effects journaled and retry-safe. |
| Open for a permitted use | A bounded opaque session bound to the exact object, revision, caller, purpose, and approved consumer. |
| Use the session | Ordered or random-access work supported by its content format, within current authority, output, resource, and lifetime limits. |
| Renew | Re-establish required authority and session validity. If renewal is unsupported, require a fresh open under the same policy. |
| Close or cancel | Idempotent termination with owned provider cleanup and an honest terminal or cleanup-pending result. |

Grant, share, and revoke remain Runtime authority operations. They invalidate or
renew dependent backend sessions through the above lifecycle. Initial support
can be restricted to the current media type, with unsupported object types
reported explicitly.

### Object descriptor

Persist the exact content revision and an authenticated, versioned protection
descriptor. It binds the content and integrity commitments, authority or rights
reference, protection format and cryptographic suite, required output policy,
and the backend protocol needed to interpret the protected material. Bind the
current policy version at operation time so permission changes can be verified
without rewriting all ciphertext.

Use existing object and content types wherever their meanings fit. Separate a
portable protocol identifier from a private deployment or provider route. Keep
the following generations distinct: content revision, data-key generation,
permission version, device credentials, custody configuration, and active
session generation. Reusing one epoch counter for all of them creates unrelated
revocations and unsafe migration assumptions.

Preserve the current approved cryptographic profile during extraction. A new
private-storage format needs reviewed encryption and key-wrapping primitives,
unique nonces as required by its scheme, authenticated metadata, integrity
verification before releasing usable data, and an explicit key-rotation and
recovery design. Deterministic derivation from one long-lived root is a custody
and compromise decision, not a shortcut supplied by the abstraction.

### Authorised request and session

Runtime constructs validated requests after verifying incoming authority. Bind
at least the exact object and revision, principal or workspace, device where
required, app or agent instance, session and grant dependency, action, accepted
policy version, output destination, and freshness requirement. Bind retries to
an operation identifier and digest. Derive authority-bearing fields from verified
context rather than trusting caller-supplied values.

Return an opaque handle with its validated binding, expiry or renewal conditions,
output kind, and bounded evidence references. Apps receive only authorised
outputs. Keys and shares stay inside the selected private protection boundary;
wallet accounts, custody nodes, mint fields, CENC details, and chain routes stay
inside adapters that need them. Transport and provider selection remain private.

Validate bindings on every applicable operation, including after restart and
provider replacement. A handle's possession alone is insufficient authority.
Observe output size, memory, concurrency, cancellation, and back-pressure limits.
Reject malformed or unknown contract versions explicitly.

### Outcomes and compatibility

Distinguish allowed, denied or revoked, authority unavailable, content
unavailable, unsupported policy or format, and unresolved provider effects.
Expose safe explanations while retaining detailed evidence within Runtime.
After an uncertain effect, use existing reconciliation ownership before retrying
an operation that could create a second key provision, charge, or transfer.

Describe a backend's actual supported operations, formats, custody assumptions,
freshness rules, recovery behavior, and execution/output guarantees. Runtime
matches these against the object's required policy using verified configuration
and the required evidence. A provider's self-declared capability is not an
attestation. Permanent key delivery cannot satisfy revocable controlled-use
requirements merely because it fits an interface.

Keep the required policy in force when a provider denies access or is unavailable.
Supplementary providers compose only through an explicit policy with defined
failure and agreement rules. Avoid parallel adapters that can each grant access
under incompatible interpretations of the same rights.

## Revocation and time

Define revocation's ordering point relative to starting an operation and
releasing output. Once a revocation is accepted, affected later operations must
fail according to the chosen consistency policy. In-flight work must stop at a
documented boundary; receipts distinguish already completed output from cancelled
or unresolved work. Apply invalidation to dependent grants, active sessions,
provider-held handles, and controlled caches.

For bounded use, define the maximum age of accepted authority and the treatment
of clock rollback, suspend/resume, restart, restored backups, and stale signed
state. Local monotonic deadlines can bound a live session; a restarted process
must re-establish trusted freshness before extending use. Stronger guarantees
against a hostile host require an appropriate time and execution trust model.
Remote revocation takes effect when accepted evidence arrives or the permitted
freshness window ends. An unreachable authority supplies neither a new grant
nor evidence of denial.

After key exposure or membership removal, protect future revisions with keys
unavailable to the removed party. Rewrapping an unchanged data key preserves
the ability of anyone who already retained it to decrypt matching ciphertext.
Short-lived wrappers around that same key do not fix this limitation. Define
controlled-cache cleanup and history access separately from storage retention.

## Replace, improve, or supplement a backend

A compatible implementation can replace a backend without changing the object
when it interprets the same format and meets the same verified policy. A change
of protection format or custody assumptions needs an explicit migration:

1. Verify migration authority and that the new backend meets the object's
   required rights, output, recovery, and cryptographic policy.
2. Read the existing format through its compatible adapter. Perform key
   rewrapping or content re-encryption inside authorised private boundaries.
3. Persist the new protected material and required availability evidence before
   publishing a new descriptor or revision.
4. Verify access and denial behavior with the new mechanism, including recovery
   and failure during migration.
5. Commit the descriptor or signed-head change atomically under the object's
   update authority. Reconcile an uncertain commit before retrying.
6. Retain old material and its adapter only while required by protected history,
   recovery, or an explicit rollback condition. Retire it through the retention
   policy after the migration gate closes.

Track key and ciphertext migration separately from interface compatibility.
Retained old material remains subject to its original exposure. A cryptographic
suite upgrade also needs an assessment of identity, signatures, authority
evidence, recovery, and retained historical ciphertext before making a
system-wide post-quantum claim.

Future mechanisms can fill separate roles. Owner-selected key custody may
serve private files; threshold custody may serve a shared service; a ledger
adapter may verify particular rights. Preserve the object's accepted authority
and trust assumptions when composing them. The first implementation does not
need a new chain, global policy engine, plugin framework, or cryptographic
invention to establish this boundary.

## Delivery slices and acceptance

### 1. Extract the existing path

Freeze current behavior on the approved development base. Place the smallest
typed internal seam above the existing protocol-specific coordinator and
protect/open lifecycle. Adapt current dKMS without changing its wire formats,
threshold assumptions, rights checks, crypto profile, or installation selection
as a side effect of the extraction. Preserve audit and durable reconciliation.

Add a deterministic fake backend for conformance tests. Demonstrate both a
successful substitution and rejection of a backend that lacks required policy.
No app or Marketplace change should be needed to select a compatible internal
implementation. Versioned media consumers may still be necessary for different
content formats; an abstraction does not make video and document operations
identical.

### 2. Close authority and lifecycle gaps

Bind the existing grant system to the session lifecycle. Implement dependent
revocation and honest status projection. Classify the present clear-media
handoff accurately; stronger protected output is a separate tested capability.
Keep current product routes selected until their intended replacement passes
installed acceptance and an explicit cutover.

### 3. Prove ordinary encrypted storage

Use the same boundary for one editable document on two devices, shared with a
group and a scoped agent. Demonstrate saving, search, previews, history,
conflicts, large or partial reads, recovery, and deletion under its policy.
Keep authority and secret metadata protected across indexes, caches, and
derived outputs. Expand to additional object types only after this journey is
usable and recoverable.

### Acceptance matrix

| Test | Evidence required |
|---|---|
| Existing dKMS regression | Current valid and invalid publish/buy/open/read/close journeys retain their decisions and receipt bindings. |
| Substitution | A fake compatible backend succeeds through the same semantic caller; an unsupported format or weaker output policy is rejected. |
| Binding attacks | Wrong principal, device, app, object, revision, action, consumer, grant, generation, and replayed handle fail. |
| Revocation | Remove a parent grant after a child and session exist. Later child operations fail; active use settles at its defined boundary; unrelated grants still work. |
| Partition and time | Authority unavailable, delayed revocation, expiry, rollback of clock or saved state, restart, and renewal cannot silently extend authority. |
| Output control | Copy/export denied at API and effect boundaries, including a permitted read combined with an unrestricted write, clipboard, cache, or agent service path. |
| Group and device changes | Concurrent membership updates resolve under the group's authority rules; removed devices lose future access; history policy remains correct. |
| Durable save and recovery | Interrupted writes preserve the last committed revision; loss of a provider or device still permits the promised key-and-data recovery. |
| Migration | Old objects remain readable through their adapter until migration; interruption preserves an authoritative descriptor and reconciles side effects. |
| Resource and installation proof | Bound memory, streaming and cancellation; verify the installed provider, Runtime, and viewer artifacts on each claimed host. |

Run the repository's required basic gates and narrow tests for changed crates.
Future implementation work should reuse the protected-content Runtime,
provider-contract, custody, server, and provider test suites appropriate to its
slice. Record exact commands and results at implementation time. Source tests
establish source behavior; installed and cross-device acceptance need their own
artifact-bound evidence under [AGENTS.md](../AGENTS.md).

Update [TASKS.md](../TASKS.md) as slices close. Add verified behavior to
[state.md](../state.md) only after its evidence exists. Keep this guide focused
on the stable boundary rather than accumulating release logs or operator paths.
