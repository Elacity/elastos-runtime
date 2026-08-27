# Key Provider

`key-provider` is a provisional 0.6 capsule. It validates the old
`elastos_common::protected_content` key-release request and remains fail closed
when no backend is configured. It is not part of the canonical v1 architecture.

The intended canonical path is:

`Runtime coordinator -> rights-provider -> custody providers -> decrypt-provider`

Runtime will select custody providers after it verifies the exact authenticated
request, Wallet approval, rights evidence, recipient authorization, and custody
epoch. Each custody provider will return only a recipient-encrypted contribution
and an authenticated receipt. Runtime and capsules will receive neither raw CEKs
nor raw shares. `decrypt-provider` will be the only boundary that may reconstruct
and briefly hold the CEK for one scoped session.

The provisional `key-provider` must not be connected to a dKMS or retained as a
second key-release route. Runtime integration must remove its DTO and provider
surface atomically after the canonical path is reviewed. There is no supported
compatibility or migration path between the two authority models.

## Retirement guard

The provisional capsule still rejects unsupported algorithms, raw key requests,
and unconfigured backend work. This behavior is checked only to keep the old
surface fail closed until removal:

```bash
cargo test --manifest-path capsules/key-provider/Cargo.toml
cargo clippy --manifest-path capsules/key-provider/Cargo.toml -- -D warnings
bash scripts/protected-content-provider-contract-smoke.sh
```

These commands do not verify canonical custody, Runtime orchestration,
decryption, or playback.
