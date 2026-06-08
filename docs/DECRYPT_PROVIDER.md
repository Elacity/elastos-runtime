# Decrypt Provider

`decrypt-provider` is the protected-content decrypt/render boundary. It
validates scoped decrypt-session requests and fails closed until a real
decrypt/render backend is configured.

The contract is:

`capsule -> runtime capability -> elastos://decrypt/* -> decrypt-provider -> decrypt/render backend`

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

`DecryptSessionRequestV1` binds:

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

## Current State

The provider is intentionally not configured for real decrypt/render work yet.
It only proves the Runtime/provider boundary:

- request validation is typed
- object IDs are treated as opaque identifiers
- raw CEK and raw plaintext paths are absent
- `open_session` and `render` return `not_configured`

## Verification

```bash
cargo test --manifest-path capsules/decrypt-provider/Cargo.toml
cargo clippy --manifest-path capsules/decrypt-provider/Cargo.toml -- -D warnings
cargo test -p elastos-server --manifest-path elastos/Cargo.toml provider_resource
scripts/check-wci-alignment.sh
```
