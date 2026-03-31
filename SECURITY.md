# Security

## Reporting

If you find a security vulnerability, please report it privately via [GitHub Security Advisories](https://github.com/Elacity/elastos-runtime/security/advisories/new). Do not open a public issue.

## Known Issues

The following security-relevant findings remain open in the current runtime and are documented here for transparency.

### Chat message verification enforcement

**Severity:** Medium (reduced from High)
**Files:** `capsules/chat/src/main.rs`, `capsules/chat/src/main_stdio.rs`, `elastos/crates/elastos-server/src/chat_cmd.rs`
**Status:** Fixed (2026-03-28)

All chat surfaces (native, WASM, agent) now sign outgoing messages via the DID provider and verify incoming messages. Unknown senders with unverified or unsigned messages are dropped before display, nick recording, or peer attachment. The shared verification logic lives in `elastos_common::chat_protocol`.

**Residual risk:** Chat is still a pre-release surface. The signing payload lacks replay protection (see below).

### Presence announcement signing

**Severity:** Medium (reduced from High)
**Files:** `capsules/chat/src/session.rs`
**Status:** Fixed (2026-03-28)

Presence announcements are now signed via the DID provider. Unsigned presence messages are dropped on receive. This prevents fake presence injection with arbitrary tickets.

**Residual risk:** No freshness check on presence signatures — replay of valid presence is still possible.

### No replay protection in chat signatures

**Severity:** Medium
**Files:** `capsules/chat/src/session.rs`, `capsules/chat/src/app.rs`
**Status:** Fixing

The signing payload is `SHA256(sender_id:ts:content)` with no nonce, topic binding, or timestamp freshness validation. The same signed message is valid on any channel and can be replayed indefinitely.

### Bridge line length limits

**Severity:** Low (reduced from Medium)
**Files:** `elastos/crates/elastos-server/src/carrier_bridge.rs`, `elastos/crates/elastos-runtime/src/handler/io_bridge.rs`
**Status:** Fixed (2026-03-28)

Bridge paths now enforce a 1MB maximum line length. Oversized requests are rejected before parsing.

### Empty capability tokens in carrier service

**Severity:** Medium
**Files:** `elastos/crates/elastos-server/src/carrier_service.rs`
**Status:** Investigating

Host-plane carrier service providers receive empty capability tokens on all requests. This is by design (carrier services are trusted host-plane code), but it means the capability model is bypassed for this provider class. Needs explicit documentation or real token forwarding.


## Architecture

The runtime enforces a capability-based security model:

- **Capsules** run sandboxed (WASM or microVM) with zero ambient authority
- **Capability tokens** are Ed25519-signed by the runtime and validated on every resource access
- **12-point token validation** covers version, signature, issuer, caller, action, resource, epoch, revocation, timing, use-count, and classification
- **Audit events** are emitted at every security-critical operation
- **Carrier** is transport-only — it does not authenticate message content (that is the application's responsibility)

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full trust model.
