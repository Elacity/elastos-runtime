# Portable mGBA Engine

The viewer carries one browser-targeted mGBA WebAssembly build for every host.
It is loaded only after compatible content is selected; Mac and Linux do not
use separate GBA binaries.

- Package: `@thenick775/mgba-wasm` 1.1.1
- NPM tarball SHA-1: `66567a2943e58b6b021cb55c23d8a0e504400835`
- NPM integrity: `sha512-nzDWAFDBBEf+lfI6Zsr4Q0njqbAKZK1fvTsA66trTaO6q4dk0gPKo4Uiykr+AbbPWoqtmc4urIujbnll0pzxGA==`
- Source: `thenick775/mgba`, commit `67036729f29589a428c7568ce68c5ee88ac89d46`
- Build image declared by that source: `emscripten/emsdk:3.1.70`
- Upstream engine: mGBA
- License: MPL-2.0
- Upstream `mgba.js` SHA-256: `18a379a8a316c58fff601253673862d3c9015adb5adc318e2b395c7cc7ec6c0c`
- Upstream `mgba.wasm` SHA-256: `bed02835f672a48b8be59f4e4cd65594109f2b54f30100539c6fd12c022d4bf9`
- Product `mgba.js` SHA-256: `0f37463aa2b7248564fd590fddf917ef3d8052ed0ed62d10b46717bb320bf3ea`
- Product `mgba.wasm` SHA-256: `9e43a33a8477cca6c277cbaa809ea2c519d6085dd844758b5cbe8e9503251a27`

This is the last published package before upstream commit
`4ce64f2529d29ef506f947545a5501503414c820` enabled Emscripten pthreads.
The product artifact contains no `Worker`, `SharedArrayBuffer`, or `Atomics`
runtime and therefore runs inside Home's required opaque sandbox without a
same-origin grant, credentialless exception, or cross-origin-isolation
carve-out.

This choice proves only the GBA product path. An opaque sandboxed frame cannot
become `crossOriginIsolated`, so it cannot host a future threaded engine even
when the gateway sends COOP/COEP headers. Shared-memory browser capsules need a
separate trustworthy per-capsule origin and reviewed isolation profile; they
must not reuse GBA as precedent for granting `allow-same-origin` on Home's
shared origin.

The upstream Emscripten build labels seven JavaScript-provided libc and MEMFS
imports with its Preview 1 module namespace. The deterministic
`scripts/normalize-gba-engine-imports.mjs` transform renames only that import
module to `capsule.local.memfs.v1`; it does not change engine code or add host
authority. The product artifact has no Runtime WASI adapter, host preopens,
environment variables, sockets, or FIFOs. The capsule ABI remains
`elastos.runtime-projection/v1`.

ROM bytes enter through authenticated Runtime viewer routes. Save bytes leave
through the viewer's principal-scoped Runtime storage route. The capsule CSP
allows only same-origin Runtime requests.
