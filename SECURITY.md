# Security

## Reporting

If you find a security vulnerability, please report it privately via [GitHub Security Advisories](https://github.com/Elacity/elastos-runtime/security/advisories/new). Do not open a public issue.

## Open Findings

The following security-relevant findings remain open in the current runtime and are documented here for transparency.

### Capability state and key rotation are not restart safe

**Severity:** Medium
**Files:** `elastos/crates/elastos-runtime/src/capability/manager.rs`, `elastos/crates/elastos-server/src/security_cmd.rs`
**Status:** Open

Capability signing-key creation and `elastos emergency rotate` log persistence
errors but continue with in-memory state. The emergency command can therefore
report success even when the new key was not written. Rotation also advances a
fresh in-process capability store rather than a durable epoch record. Until the
key and revocation state are committed atomically, operators must verify the
persisted key and restart result instead of treating command completion as a
durable rotation receipt.

### Host-plane Carrier providers use Runtime admission

**Severity:** Medium
**Files:** `elastos/crates/elastos-server/src/carrier_service.rs`
**Status:** Open

Host-plane Carrier service requests use a raw operation/path envelope without
a capsule capability-token field. Runtime owns admission for this trusted
provider class. Its trust boundary must remain explicit in manifests and audit
output; the envelope is not an independent grant of capsule authority.

### Request framing remains incompletely bounded

**Files:** `elastos/crates/elastos-server/src/carrier.rs`, `elastos/crates/elastos-runtime/src/handler/io_bridge.rs`
**Status:** Open

The incoming Carrier request handler reads a line before parsing without a
request-size cap or read deadline at that boundary. The I/O bridge rejects
complete lines above 1 MiB, but its line readers allocate before that check.
Size checks after reading do not bound memory use or an incomplete frame's
lifetime. Add bounds while reading, with oversized and slow-frame tests;
the Carrier integration task is tracked in [TASKS.md](TASKS.md#deferred-source-integration).

## Resolved Findings

These findings are fixed in the current branch but remain listed as security history because they shaped the runtime contract.

### I/O bridge parse-size check

**Severity:** Low (reduced from Medium)
**Files:** `elastos/crates/elastos-runtime/src/handler/io_bridge.rs`
**Status:** Fixed (2026-03-28)

The I/O bridge rejects complete request lines above 1 MiB before parsing. The
old `carrier_bridge.rs` has been removed. This resolved parse-size check does
not close the read-time framing gap above.

## Architecture

The runtime enforces a capability-based security model:

- **Capsules** run sandboxed (WASM or microVM) with zero ambient authority
- **Capability tokens** are Ed25519-signed by the runtime and validated on every resource access
- **12-point token validation** covers version, signature, issuer, caller, action, resource, epoch, revocation, timing, use-count, and classification
- **Audit events** are emitted at every security-critical operation
- **Carrier** authenticates transport endpoints. Runtime verifies product
  identity and signed message authority before exposing typed app projections.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full trust model.
