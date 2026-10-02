# Security

## Reporting

If you find a security vulnerability, please report it privately via [GitHub Security Advisories](https://github.com/Elacity/elastos-runtime/security/advisories/new). Do not open a public issue.

## Current isolation and target boundary

Apps run as web projections in the browser's opaque sandboxed frames. Runtime
checks their capability tokens before it performs an effect. Home can currently
obtain every app's capability, so a compromised Home can reach those apps'
authority. The target gives Home only delegation and gives each app a separate,
revocable capability inside its frame. The web Terminal is disabled by default
after the terminal gate is integrated. If an owner enables it, Terminal runs
commands as the host user through a host process. Treat that choice as trusted
operator access.

Providers run as operating-system processes. Only the model provider is partly
confined; a provider declaration does not prove host isolation. Complete provider
confinement and Runtime-owned key custody are target behavior. Recovery from a
stolen device key requires a new identity.

The [isolation plan](https://github.com/Elacity/elastos-runtime/issues/173)
records the remaining gates. Source checks describe a source candidate. Accepted
installation proof binds the exact Runtime, components and app assets to the
journeys tested on that device. An installed version label alone proves neither
artifact parity nor isolation. These statements require the content sandbox,
front-door and disabled-terminal changes to be integrated and tested together.

## Trust and stored keys

Runtime trusts its own operating-system account and root. Code with either
account's access can use the stored keys. The user's browser, extensions,
physical access, a stolen Recovery Kit or passkey authenticator, and hardware
key protection are outside this isolation claim. A stolen device or profile key
requires a new identity. Independent code review remains a development gate.

Keys are stored next to the data they protect. Full data-home backups include
all keys. The seed operator can read data, wallet keys and recovery phrases
stored on the seed. Encryption on that server does not isolate these secrets
from its operator. Keep sensitive data on a Home whose operator you trust.

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
the Carrier integration task is tracked in [deferred work](docs/DEFERRED_WORK.md#retained-source-integration).

## Resolved Findings

These findings are resolved in source but remain listed as security history because they shaped the runtime contract.

### I/O bridge parse-size check

**Severity:** Low (reduced from Medium)
**Files:** `elastos/crates/elastos-runtime/src/handler/io_bridge.rs`
**Status:** Fixed (2026-03-28)

The I/O bridge rejects complete request lines above 1 MiB before parsing. The
old `carrier_bridge.rs` has been removed. This resolved parse-size check does
not close the read-time framing gap above.

## Architecture

Runtime validates signed capability tokens against the caller, action,
resource, epoch, revocation and token constraints. It checks principal and
session authority separately. Carrier authenticates transport endpoints;
Runtime verifies product identity and signed message authority before exposing
typed app projections. These checks complement the current browser boundary
and do not establish provider process confinement or a separate Home authority.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full trust model.
