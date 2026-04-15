# Changelog

All notable changes to the public ElastOS Runtime repository.

## [0.1.2] - Unreleased

### Added
- Added device-backed local identity profile storage and shared DID-backed nickname handling across the CLI, did-provider, and PC2 surfaces.
- Added hosted browser-capsule foundation, the shipped `room-browser` asset set, and sovereign room invite/accept control with cross-runtime Carrier sync.
- Added explicit operator-lane setup, remote node control over Carrier, and release-line public-install/operator acceptance scripts.

### Changed
- Kept PC2 as the honest front door by surfacing room, chat, and identity flows with the current runtime and return-home contract instead of stale placeholder doctrine.
- Split setup profiles more explicitly between the core PC2 path, the broader demo surface, and the explicit operator lane.
- Hardened release proofing around clean-home setup, the PTY PC2 frontdoor, room-browser packaging, and source-local trusted-source checks.

### Fixed
- Unified main DID derivation with the device key and aligned local nickname persistence onto one shared codec.
- Removed stale live-host conflicts so managed PC2/chat lanes and the explicit operator lane do not silently share one home.
- Cleaned up the public naming around `room-browser` so the shipped browser route, packaging, and proof tooling all agree.

## [0.1.1] - 2026-03-31

### Fixed
- Removed the installer's undeclared `xxd` dependency from signature verification so minimal environments can install from the canonical gateway without extra packages.
- Pinned the documented and declared Rust toolchain to `1.89+` so fresh source builds match the actual compiler floor.
- Tightened PC2 home guidance and native chat runtime reuse so the public onboarding path stays coherent on WSL and Jetson.

## [0.1.0] - 2026-03-31

### Added
- Signed install, setup, and update flow with a canonical public onboarding path.
- Native Carrier chat with signed message verification, cross-host WSL ↔ Jetson proof, and same-host native ↔ WASM proof coverage.
- Capability-gated capsule execution across native runtime surfaces, WASM capsules, and microVM capsules.
- DID-backed identity, local sharing, site hosting/publish/activate/rollback, and agent capsule support.

### Changed
- The public repository starts fresh at `0.1.0`.
- `elastos chat` is native Carrier chat only; packaged chat surfaces launch through `elastos capsule ...`.
- The installer and first-run story are centered on `install.sh -> elastos setup -> elastos`.

### Removed
- Runtime/proof override residue including `ELASTOS_COMPONENTS_MANIFEST`, `ELASTOS_DEV_SEARCH`, `SkippedDevPath`, `InstalledBinaryVerification`, and `chat --mode ...`.

## Pre-public internal lineage

Earlier internal release candidates and development history existed before the public repository launch. They are intentionally not carried forward as the public release line.
