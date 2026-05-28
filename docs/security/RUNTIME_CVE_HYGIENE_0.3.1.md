# Runtime CVE Hygiene For 0.3.1

## Scope

This work integrates the `chore/runtime-cve-hygiene` audit branch onto the
`v0.3.0` runtime line. The source branch remains the detailed audit trail; this
document records the maintainer-facing state intended for the clean release
branch.

The goal is dependency and runtime security hygiene without changing capsule,
wallet, browser, provider, or Home product behavior.

## Security Changes

- Upgrade the WASM runtime stack from Wasmtime 17 to Wasmtime 36.
- Replace WASM carrier fd injection with a named-FIFO bridge compatible with
  modern `wasmtime-wasi` preview1 APIs.
- Remove dead TUI dependencies from `elastos-server` that pulled vulnerable or
  unmaintained transitive crates.
- Upgrade `lru` in `elastos-storage` to remove the 0.12 unsoundness advisory.
- Upgrade `axum-server` and remove the direct `rustls-pemfile` dependency from
  `elastos-tls`; PEM parsing now uses `rustls-pki-types` through the active
  Rustls stack.
- Remove the direct `rsa` dependency from WebAuthn. RS256 passkeys remain
  supported, but verification now goes through `aws-lc-rs`, which is already in
  the active networking/TLS dependency graph.
- Pin `distributed-topic-tracker` to `=0.2.7` until the server migrates the
  whole iroh stack together. This avoids an iroh 0.96/0.97 split in the same
  dependency tree.

## Runtime Boundary Notes

The carrier bridge still uses the same JSON-line capsule protocol. The transport
changes from injected WASI file descriptors to FIFOs mounted through a preopened
WASI directory:

```text
capsule
  -> /_carrier/request FIFO
  -> runtime bridge
  -> provider registry
  -> /_carrier/response FIFO
  -> capsule
```

This keeps the ElastOS model intact: capsules communicate through Carrier and
provider contracts, not through raw host APIs.

## Residual Advisories

The original audit branch documents two residual `hickory-proto` CVEs and
several unmaintained transitive warnings. The Hickory advisories are explicitly
ignored in `elastos/.cargo/audit.toml` so `cargo audit` can be used as a
repeatable check while keeping the exception visible. They are not force-updated
here because the current compatible iroh generations still pin vulnerable
Hickory versions, while the newer iroh line requires a coordinated Carrier and
Rust-toolchain migration beyond this patch branch.

Recommended follow-ups:

- `chore/runtime-cve-residuals-iroh`: revisit hickory/postcard/paste warnings
  when iroh publishes a compatible update path.
- `chore/bincode-2-migration`: migrate token encoding with explicit versioning
  instead of silently changing the wire format.
- `chore/wasmtime-rustc-hash`: revisit `fxhash` after upstream Wasmtime moves.

## Required Verification

Before merging this security branch:

- `cargo fmt --manifest-path elastos/Cargo.toml --all -- --check`
- `cargo check --manifest-path elastos/Cargo.toml --workspace`
- `cargo clippy --manifest-path elastos/Cargo.toml --workspace --all-targets -- -D warnings`
- `cargo test --manifest-path elastos/Cargo.toml -p elastos-compute --lib`
- `cargo test --manifest-path elastos/Cargo.toml -p elastos-guest --lib`
- `cargo test --manifest-path elastos/Cargo.toml -p elastos-storage --lib`
- `cargo test --manifest-path elastos/Cargo.toml -p elastos-tls --lib`
- `scripts/check-wci-alignment.sh`
- `scripts/auth-wallet-focus-smoke.sh`
- `scripts/browser-abi-provider-contract-smoke.sh`
- `scripts/browser-objective-audit-smoke.sh`

Human verification should focus on:

- Home passkey login still works.
- Wallet still opens, shows accounts, sends a built-in transaction, and records
  activity.
- Browser still opens and routes wallet requests through Inbox/Wallet.
- A capsule using `elastos-guest` can invoke Carrier through the FIFO bridge.
