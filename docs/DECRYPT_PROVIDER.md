# Decrypt Provider

`decrypt-provider` will be the canonical protected-content decrypt and render
boundary. Runtime will bind one scoped session to the exact authenticated
Profile, Wallet-approved action, object, rights evidence, recipient
authorization, custody epoch, expiry, and provider identity.

The intended path is:

`Runtime coordinator -> decrypt-provider -> decrypt/render backend`

Custody providers return recipient-encrypted contributions. Runtime may relay
those opaque contributions or other sealed material, but it cannot open them.
The decrypt boundary is the only component that may reconstruct and briefly
hold the live CEK. It returns scoped output or an opaque session handle and
zeroizes the CEK when the session ends. It must not make outbound calls to gain
rights, custody, Wallet, Chain, storage, or network authority.

Capsules receive no raw CEK, raw plaintext authority, custody shares, provider
routes, endpoint DIDs, network locations, credentials, filesystem authority,
Wallet RPC, Chain RPC, or backend SDK.

## Implementation and retirement

The canonical implementation is `protected-content-decrypt-provider`, called
through the Runtime-owned protected-content coordinator. Its typed operations
cover reconstruction, scoped media reads and terminal cleanup. Installation
and activation evidence belongs in [state.md](../state.md).

The provisional `decrypt-provider` capsule uses
the old `elastos_common::protected_content` DTO, validates requests, and returns
`not_configured`. It remains only as a fail-closed retirement surface and must
be replaced atomically. It does not verify the canonical v1 path.

## Verification

Provisional retirement guard only:

```bash
cargo test --manifest-path capsules/decrypt-provider/Cargo.toml
cargo clippy --manifest-path capsules/decrypt-provider/Cargo.toml -- -D warnings
bash scripts/protected-content-provider-contract-smoke.sh
```
