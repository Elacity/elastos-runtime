# Key Provider

`key-provider` is the protected-content key-release boundary. It validates
Runtime-mediated key requests and keeps dangerous backend authority out of app
capsules:

`capsule -> runtime capability -> elastos://key/* -> key-provider -> dKMS backend`

This page describes the older provisional provider surface. It does not
describe or prove the canonical source-only v1 contract in
[`PROTECTED_CONTENT_CONTRACTS_V1.md`](PROTECTED_CONTENT_CONTRACTS_V1.md). Future
integration must replace the provisional surface atomically, with no parallel
decoder, fallback, or migration path.

Capsules do not receive raw CEKs, KMS node credentials, chain RPC, wallet RPC,
or provider credentials.

## Operations

- `status`: list supported schemes and blocked raw authority.
- `release`: validate a `KeyReleaseRequestV1` and request scoped key release.

Current implementation is intentionally fail-closed. It validates schema,
principal/session/object/action fields in the provisional
`elastos_common::protected_content::KeyReleaseRequestV1`, an allowed
`elastos.rights.decision.receipt/v1` bound to the same
principal/session/object/action, supported schemes, and PQ-hybrid algorithm
metadata, then refuses backend work until an ElastOS dKMS adapter exists.

## Decrypt Handoff

`key-provider` owns key-release validation and dKMS authority, but app and viewer
capsules must never receive raw CEKs. The next live decrypt integration should
seal decrypt material to a one-time public key generated for the decrypt
session:

- `decrypt-provider` supplies a one-time decrypt-session public key.
- `key-provider` or the dKMS release backend seals the CEK/material to that key
  using the approved PQ-hybrid envelope profile.
- the release receipt remains a receipt; it is not a key carrier.
- the sealed material is handed to the decrypt session and can only be unwrapped
  inside that decrypt sandbox.

Prefer direct dKMS sealing to the decrypt-session key. A key-provider re-seal is
acceptable only as a migration step if it remains provider-internal, signed,
auditable, and short-lived. Lit/Chipotle can be one compatibility backend behind
this provider, but it must not define the Runtime contract or become the only
key-release dependency. The independent target is an ElastOS-native PQ-hybrid
threshold dKMS that can produce the same backend-neutral sealed material
handoff.

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
