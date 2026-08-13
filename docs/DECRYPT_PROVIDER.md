# Decrypt Provider

`decrypt-provider` is the protected-content decrypt/render boundary. It
validates scoped decrypt-session requests and fails closed until a real
decrypt/render backend is configured.

The contract is:

`capsule -> runtime capability -> elastos://decrypt/* -> decrypt-provider -> decrypt/render backend`

This page describes the older provisional provider surface. It does not
describe or prove the canonical source-only v1 contract in
[`PROTECTED_CONTENT_CONTRACTS_V1.md`](PROTECTED_CONTENT_CONTRACTS_V1.md). Future
integration must replace the provisional surface atomically, with no parallel
decoder, fallback, or migration path.

Apps and viewers must not receive raw CEKs, broad plaintext authority,
filesystem authority, key-backend SDKs, KMS credentials, chain RPC, wallet RPC,
or provider credentials. They receive scoped rendered output, streams, or
working copies after Runtime, rights, and key-release checks succeed.

## Operations

- `status` -> `elastos://decrypt/meta/status`
- `open_session` -> `elastos://decrypt/session/open`
- `render` -> `elastos://decrypt/render`

Unsupported operations fail closed and do not create broad provider wildcards.

## Request Shape

The current provisional `elastos_common::protected_content::DecryptSessionRequestV1`
binds:

- request ID
- principal ID
- session ID
- object CID
- requested action
- viewer interface
- typed `elastos.release.receipt/v1` from `key-provider`, bound to the same
  principal/session/object/action
- output kind
- reason
- expiry

The current provider accepts only documented protected-content actions and
outputs: `rendered`, `stream`, and `working_copy`.

## Key Material Rail

The current provisional `elastos_common::protected_content::ReleaseReceiptV1`
is an authorization receipt, not key material. It proves that the key-provider
accepted the rights-bound release request for the same
principal/session/object/action, but it does not contain a CEK.

When the real decrypt backend is wired, the recommended normal path is:

- `decrypt-provider` creates a per-session one-time public key for the decrypt
  sandbox.
- `key-provider` or the dKMS release backend seals decrypt material to that
  public key.
- `DecryptSessionRequestV1` is extended with a sealed decrypt material envelope,
  bound to the same principal/session/object/action and release receipt.
- The decrypt sandbox unwraps inside the boundary, decrypts/renders, zeroizes
  the live CEK, and returns only scoped output.
- `decrypt-provider` must not be granted outbound key-fetch authority as the
  normal path.

This preserves the provider-to-provider chain without letting the component that
briefly sees the live CEK call out to other providers or raw backends.

The alternatives are intentionally narrower:

- outbound key-fetch from `decrypt-provider` is only acceptable for an explicit
  adapter with a scoped, audited capability, not as the default rail.
- a combined key/decrypt provider is acceptable for tests but not for the target
  trust boundary because it merges dKMS authority with decrypt/render authority.
- Lit/Chipotle-style CEK envelopes are compatibility inputs only. The decrypt
  contract must validate a backend-neutral sealed material envelope so an
  ElastOS-native dKMS backend can replace vendor key release.

## Current State

The provider is intentionally not configured for real decrypt/render work yet.
It only proves the Runtime/provider boundary:

- request validation is typed
- object IDs are treated as opaque identifiers
- raw CEK and raw plaintext paths are absent
- `open_session` and `render` return `not_configured`
- the sealed decrypt material envelope is not implemented yet

## Verification

```bash
cargo test --manifest-path capsules/decrypt-provider/Cargo.toml
cargo clippy --manifest-path capsules/decrypt-provider/Cargo.toml -- -D warnings
cargo test -p elastos-server --manifest-path elastos/Cargo.toml provider_resource
scripts/check-wci-alignment.sh
```
