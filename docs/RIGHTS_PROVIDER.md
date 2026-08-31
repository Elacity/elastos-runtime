# Rights Provider

`rights-provider` will be the canonical protected-content policy boundary.
Runtime will ask one typed policy question for the exact authenticated Profile,
Wallet, object, action, policy identity, chain, contract, selector, and evidence
window. The provider will use typed `chain-provider` reads and return signed
evidence. It will not release key material or select custody nodes.

The intended path is:

`Runtime coordinator -> rights-provider -> chain-provider`

Capsules may request a protected-content action, but they do not call rights or
Chain providers directly. They do not supply provider routes, RPC URLs, contract
transports, endpoint DIDs, IP addresses, ports, or credentials.

## Current source state

The canonical source-only contract is `RightsPolicyBodyV1` with matching typed
evidence request and result values in
`elastos-protected-content-contracts`. `elastos-protected-content-rights`
evaluates those contracts and can acquire Chain evidence through a
Runtime-owned `ProviderRegistry` invoke of `chain` /
`protected_content_rights_evidence`. That adapter is source-only. It does not
replace the installed provisional `rights-provider`.

The provisional `rights-provider` capsule
uses the old `elastos_common::protected_content` DTO and supports a wider set of
unwired operations. It remains fail closed and must be replaced atomically
during Runtime integration. It is not a second supported contract.

## Verification

Canonical source-only contract:

```bash
(cd elastos && cargo test -p elastos-protected-content-contracts)
(cd elastos && cargo test -p elastos-protected-content-rights -- --nocapture)
(cd elastos && cargo test -p elastos-server protected_content_runtime -- --nocapture)
```

Provisional retirement guard only:

```bash
cargo test --manifest-path capsules/rights-provider/Cargo.toml
cargo clippy --manifest-path capsules/rights-provider/Cargo.toml -- -D warnings
bash scripts/protected-content-provider-contract-smoke.sh
```
