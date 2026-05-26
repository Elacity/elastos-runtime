# Key Provider

`key-provider` is the protected-content key-release boundary. It validates
Runtime-mediated key requests and keeps dangerous backend authority out of app
capsules:

`capsule -> runtime capability -> elastos://key/* -> key-provider -> dKMS backend`

Capsules do not receive raw CEKs, KMS node credentials, chain RPC, wallet RPC,
or provider credentials.

## Operations

- `status`: list supported schemes and blocked raw authority.
- `release`: validate a `KeyReleaseRequestV1` and request scoped key release.

Current implementation is intentionally fail-closed. It validates schema,
principal/session/object/action fields, supported schemes, and PQ-hybrid
algorithm metadata, then refuses backend work until an ElastOS dKMS adapter
exists.

## Capability Schema

| Scope | Resource |
|-------|----------|
| Status | `elastos://key/meta/status` |
| Release | `elastos://key/release` |

## Supported Schemes

- `elastos-pq-hybrid-threshold-v0`

## Algorithm Policy

The provider accepts only reviewed algorithm metadata:

- Payload cipher: `aes-256-gcm` or `chacha20-poly1305`
- KEM/share wrapping: both `x25519` and `ml-kem-768`
- Signatures: approved classical + PQ set, currently `ed25519` plus `ml-dsa-65`
  or `slh-dsa-sha2-256s`
- Share scheme: `shamir-t-of-n`

FROST is not a dKMS root. It can be a classical helper for receipts or cohort
decisions, but new long-lived protected content must use PQ-hybrid key
envelopes.

## Verification

```bash
cargo test --manifest-path capsules/key-provider/Cargo.toml
cargo clippy --manifest-path capsules/key-provider/Cargo.toml -- -D warnings
cargo test -p elastos-server --manifest-path elastos/Cargo.toml provider_resource
bash scripts/check-wci-alignment.sh
```
