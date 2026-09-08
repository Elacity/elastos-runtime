# Rights evaluation

Rights evaluation is the Runtime-owned protected-content policy boundary.
Runtime asks one typed policy question for the exact authenticated Profile,
Wallet, object, action, policy identity, chain, contract, selector, and evidence
window, and receives signed evidence. Rights evaluation releases no key material
and selects no custody nodes.

The path is:

`Runtime coordinator -> chain (protected_content_rights_evidence) -> chain-provider`

Each custody committee member settles every release by evaluating the same
rights request through its own `protected_content_rights_evidence` call against
its node-host Chain provider.

Capsules may request a protected-content action, but they do not call rights or
Chain providers directly. They do not supply provider routes, RPC URLs, contract
transports, endpoint DIDs, IP addresses, ports, or credentials.

## Contract

The canonical contract is `RightsPolicyBodyV1` with matching typed evidence
request and result values in `elastos-protected-content-contracts`.
`elastos-protected-content-rights` evaluates those contracts and acquires Chain
evidence through a Runtime-owned `ProviderRegistry` invoke of `chain` /
`protected_content_rights_evidence`.

This is the only rights authority. The provisional `rights-provider` capsule and
its `elastos://rights` scheme were removed at cutover, and the scheme is no
longer reservable for sub-provider registration.

## Verification

```bash
(cd elastos && cargo test -p elastos-protected-content-contracts)
(cd elastos && cargo test -p elastos-protected-content-rights -- --nocapture)
(cd elastos && cargo test -p elastos-server protected_content_runtime -- --nocapture)
```

