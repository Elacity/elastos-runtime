# Changelog

All notable changes to the public ElastOS Runtime repository.

## [0.2.0] - 2026-04-29

### Added
- Added the Home browser shell capsule, `home-cli`, and the `elastos home` command path as the visible front door.
- Added first-party System, Inbox, Library, Documents, Chat Room, GBA Emulator, and uCity browser/content capsules to the shipped Home catalog.
- Added runtime-owned browser capsule routing, object/viewer launch foundations, and app-scoped launch tokens for Home-launched surfaces.
- Added Documents provider APIs for summary, create, get, save, save-as, publish, unpublish, delete, and immutable `elastos://<cid>` document opens.
- Added Library document browsing and Chat Room attachment flow through Home orchestration instead of browser file upload.
- Added Chat Room browser-session pairing, same-browser Home session reuse, guest identity separation, guest kick controls, and runtime member invite/block controls.
- Added System appearance controls for wallpaper, reset, overlay toggle, and overlay opacity.
- Added Home PWA metadata, mobile fullscreen support, touch-first desktop icon behavior, reversible desktop icons, and mobile-safe window behavior.
- Added GBA save-state persistence, mobile touch controls, keyboard mapping labels, fullscreen ratio handling, compact controls, and fail-fast unsupported-WebView detection.
- Added shared first-party design-system docs and `scripts/home-entropy-check.mjs` for UI, naming, authority, and stale-copy drift checks.

### Changed
- Renamed the visible product front door from PC2 to Home and aligned setup profiles, proof scripts, docs, and CLI smoke tests around the Home naming.
- Replaced `md-viewer` with Documents and `room-browser` with Chat Room.
- Split setup profiles more explicitly between the core Home path, the broader demo surface, and the explicit operator lane.
- Hardened release proofing around clean-home setup, the PTY Home front door, `chat-room` packaging, source-local trusted-source checks, and Home/browser journey smoke coverage.
- Moved Home wallpaper and contrast overlay configuration into `System -> Appearance` backed by the runtime appearance store.
- Made Documents object-first: DID-backed document identity is the mutable object, `localhost://Users/self/Documents/...` is only the local working copy, and `elastos://<cid>` is the immutable published revision.
- Made Library content-first around documents and typed content instead of raw working-copy paths.
- Moved Inbox list rendering, read state, approval, denial, dismissal, and source-app open actions into the Inbox capsule; Home now owns only badge and launch.
- Aligned first-party capsule UI colors, spacing, mobile padding, and accessible controls with the shared light capsule token set.
- Aligned roadmap, principles, architecture, namespaces, and security docs around the four quadrants, object/capsule/space ontology, and capability-scoped Carrier/provider boundary.

### Fixed
- Unified main DID derivation with the device key and aligned local nickname persistence onto one shared codec.
- Removed stale live-host conflicts so managed Home/chat lanes and the explicit operator lane do not silently share one home.
- Cleaned up the public room naming so the shipped `chat-room` route, packaging, and proof tooling all agree.
- Moved Documents publish/unpublish IPFS-specific logic out of the gateway edge into the provider plane.
- Removed the gateway-owned IPFS provider bridge path; public CID reads now use cached content or the runtime provider registry and fail closed otherwise.
- Required Home authority before minting app launch tokens, app-scoped launch tokens for System and Inbox APIs, and browser-context-bound chat-room access polling.
- Redacted room bearer tokens from public/Home summaries and preferred the native Home room session over paired browser identity when both exist in one browser profile.
- Fixed browser Chat Room identity handling so messages from the native Home member no longer appear as the browser guest's own messages.
- Fixed Documents publish/unpublish behavior so unchanged content does not produce unnecessary new published revisions.
- Fixed document delete confirmation to use in-surface UI instead of browser alerts.
- Fixed Home window dragging so windows can move partially offscreen without jumping back when their title bar is clicked.
- Fixed desktop drag selection, desktop icon removal/re-add, mobile launcher focus, and maximized-window coverage over Home chrome.

## [0.1.2] - 2026-04-16

### Added
- Added device-backed local identity profile storage and shared DID-backed nickname handling across the CLI, did-provider, and PC2 surfaces.
- Added hosted browser-capsule foundation, the shipped `room-browser` asset set, and sovereign room invite/accept control with cross-runtime Carrier sync.
- Added explicit operator-lane setup, remote node control over Carrier, and release-line public-install/operator acceptance scripts.

### Changed
- Kept PC2 as the honest front door by surfacing room, chat, and identity flows with the current runtime and return-home contract.
- Split setup profiles more explicitly between the core PC2 path, the broader demo surface, and the explicit operator lane.
- Hardened release proofing around clean-home setup, the PTY PC2 front door, room-browser packaging, and source-local trusted-source checks.

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
