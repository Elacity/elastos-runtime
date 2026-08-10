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

### Empty capability tokens in carrier service

**Severity:** Medium
**Files:** `elastos/crates/elastos-server/src/carrier_service.rs`
**Status:** Open

Host-plane Carrier service providers receive empty capability tokens on all requests. This is by design for trusted host-plane code, but it means this provider class does not use the same token-forwarding path as ordinary app capsules. The trust-domain distinction needs to stay explicit in docs, manifests, and audit output.

## Resolved Findings

These findings are fixed in the current branch but remain listed as security history because they shaped the runtime contract.

### Bridge line length limits

**Severity:** Low (reduced from Medium)
**Files:** `elastos/crates/elastos-server/src/carrier_bridge.rs`, `elastos/crates/elastos-runtime/src/handler/io_bridge.rs`
**Status:** Fixed (2026-03-28)

Bridge paths now enforce a 1MB maximum line length. Oversized requests are rejected before parsing.

## Architecture

The runtime enforces a capability-based security model:

- **Capsules** run sandboxed (WASM or microVM) with zero ambient authority
- **Capability tokens** are Ed25519-signed by the runtime and validated on every resource access
- **12-point token validation** covers version, signature, issuer, caller, action, resource, epoch, revocation, timing, use-count, and classification
- **Audit events** are emitted at every security-critical operation
- **Carrier** is transport-only — it does not authenticate message content (that is the application's responsibility)

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full trust model.
