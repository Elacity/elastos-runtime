# State

Last updated: 2026-04-15 UTC

Product state and open truths for the ElastOS runtime.
For open work, see [TASKS.md](TASKS.md).
For direction, see [ROADMAP.md](ROADMAP.md).

## What works

- Signed install → setup → PC2 home as the default front door.
- Native P2P chat over Carrier with Ed25519 message signing and verification.
- Same-host native ↔ WASM chat interop on shared runtime (proven 2026-03-30).
- Sovereign room control with DID-backed invite/accept flow and hosted `room-browser` access through the explicit operator lane.
- WASM and microVM capsule execution with capability-gated provider access.
- Signed release, update, and publish pipeline (Carrier-first, explicit web bootstrap/override only).
- Operator-only remote node status, room control, and trusted-source update control over Carrier via `elastos node ...`.
- Content sharing, local site hosting, site publish/activate/rollback.
- DID-backed identity (did:key, Ed25519) with encrypted key storage.
- Agent capsule with signed gossip and verified-only AI responses.

## What is proven

- `just verify` — source-line gate: alignment, clean-home setup, command smoke, candidate command audit, fmt, clippy, and tests.
- `just verify-release` — release-trust gate: `just verify` plus the PTY PC2 frontdoor smoke.
- `scripts/shared-runtime-gossip-proof.sh` — bidirectional gossip delivery on shared runtime.
- `scripts/chat-wasm-native-interop-smoke.sh` — native ↔ WASM end-to-end.
- `scripts/chat-wasm-local-smoke.sh` — local WASM chat.
- `cargo test -p elastos-server --lib operator_control::tests::test_two_node_operator_status -- --ignored --exact --nocapture` — local two-runtime operator Carrier proof.
- `cargo test -p elastos-server --lib api::gateway::tests::test_room_service_cross_runtime_presence_syncs_join_and_leave -- --exact --nocapture` — local two-runtime room presence proof.
- `cargo test -p elastos-server --lib api::gateway::tests::test_room_service_cross_runtime_room_syncs_over_carrier -- --exact --nocapture` — local two-runtime room message-sync proof.
- `cargo test -p elastos-server --lib api::gateway::tests::test_room_service_cross_runtime_attachment_syncs_over_carrier -- --exact --nocapture` — local two-runtime room attachment-sync proof.
- `scripts/public-install-identity-smoke.sh` — installed-path DID/profile acceptance path.
- `scripts/public-install-operator-smoke.sh` — installed-path operator-node status/update acceptance path.
- `scripts/public-install-pc2-frontdoor-smoke.sh` — installed-path PC2 frontdoor acceptance path.
- the older dedicated `scripts/public-install-update-smoke.sh` and `scripts/public-linux-runtime-portability-smoke.sh` proofs are not separate active scripts on this line; their concerns are folded into the `scripts/public-install-*.sh` acceptance helpers, rerunning those helpers against a published gateway via `ELASTOS_PUBLISHER_GATEWAY=<published-url>`, and `just verify-release`.

## Open truths

- The main blocker is target-machine PC2 boringness, not missing features.
- PC2 is more honest than the earlier public line, but some installed-path surfaces are still secondary rather than boring.
- Hosted room setup currently spans `setup --profile demo` plus the explicit operator lane, and that split is still too implicit.
- Installed target-machine proof for the full `elastos → PC2 → app → home` path is still a manual acceptance item.
- GBA is locally promising but not yet earned as a public default path.

## Support boundary

- Linux is the truthful full-runtime baseline (x86_64 and aarch64).
- macOS is a developer workstation, not a full runtime target.
- PC2 is the intended front door but not fully boring on every target machine yet.
