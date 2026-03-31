# State

Last updated: 2026-03-31 UTC

Product state and open truths for the ElastOS runtime.
For open work, see [TASKS.md](TASKS.md).
For direction, see [ROADMAP.md](ROADMAP.md).

## What works

- Signed install → setup → PC2 home as the default front door.
- Native P2P chat over Carrier with Ed25519 message signing and verification.
- Same-host native ↔ WASM chat interop on shared runtime (proven 2026-03-30).
- WASM and microVM capsule execution with capability-gated provider access.
- Signed release, update, and publish pipeline (Carrier-first, explicit web bootstrap/override only).
- Content sharing, local site hosting, site publish/activate/rollback.
- DID-backed identity (did:key, Ed25519) with encrypted key storage.
- Agent capsule with signed gossip and verified-only AI responses.

## What is proven

- `scripts/shared-runtime-gossip-proof.sh` — bidirectional gossip delivery on shared runtime.
- `scripts/chat-wasm-native-interop-smoke.sh` — native ↔ WASM end-to-end.
- `scripts/chat-wasm-local-smoke.sh` — local WASM chat.
- `cargo fmt --check && cargo clippy -D warnings && cargo test` — code gates pass.
- `scripts/pc2-frontdoor-smoke.sh` — release-trust gate (requires canonical publisher key, not dev key).
- `scripts/public-install-update-smoke.sh` — explicit stamped-install update proof required for release readiness.
- `scripts/public-install-identity-smoke.sh` and local-identity-profile-smoke.sh — DID/profile proof path for public install and local profile behavior.
- `scripts/public-linux-runtime-portability-smoke.sh` — explicit public Linux runtime portability proof required for release readiness.

## Open truths

- The main blocker is target-machine PC2 boringness, not missing features.
- Public PC2 still over-promises: some app surfaces are visible before they are useful.
- Jetson and WSL proof for the full `elastos → PC2 → app → home` path is not yet boring.
- GBA is locally promising but not yet earned as a public default path.

## Support boundary

- Linux is the truthful full-runtime baseline (x86_64 and aarch64).
- macOS is a developer workstation, not a full runtime target.
- PC2 is the intended front door but not fully boring on every target machine yet.
