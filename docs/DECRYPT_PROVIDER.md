# Decrypt Provider

`protected-content-decrypt-provider` is the protected-content decrypt and media
read boundary, registered by Runtime on the Runtime-only
`protected-content-decrypt` target. Runtime binds one scoped session to the
exact authenticated Profile, Wallet-approved action, object, rights evidence,
recipient authorization, custody epoch, expiry, and provider identity.

The path is:

`Runtime coordinator -> protected-content-decrypt -> scoped viewer session`

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

The provisional `decrypt-provider` capsule and its `elastos://decrypt` scheme
were removed at cutover. There is no second decrypt route.

## Verification

```bash
cargo test --manifest-path capsules/protected-content-decrypt-provider/Cargo.toml
(cd elastos && cargo test -p elastos-server protected_content_runtime)
```
