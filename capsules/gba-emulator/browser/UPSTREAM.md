# Portable mGBA Engine

The viewer carries one browser-targeted mGBA WebAssembly build for every host.
It is loaded only after compatible content is selected; Mac and Linux do not
use separate GBA binaries.

- Package: `@thenick775/mgba-wasm` 2.4.1
- Source: `thenick775/mgba`, commit `be30a34e913da1ba7f040d3db4e10f700ce49f76`
- Upstream engine: mGBA
- License: MPL-2.0
- Upstream `mgba.js` SHA-256: `78e30a6542173e349e27b3cd3652f20d69b41ed742d1a80e64e253d17e25918a`
- Upstream `mgba.wasm` SHA-256: `546a99648d2ef52cb04e34e19a4d0ad2d5dc6bcf0f6749bbaba7d5771226f002`
- Product `mgba.js` SHA-256: `7ddadd7c564293bd6552fd9640e2ae85d927a2d756323c0f4e526aa4ccc72111`
- Product `mgba.wasm` SHA-256: `69b9fccd6cc616682a92866c2a3ad846ded3618661e3258da6582fbf54a2482e`

The upstream Emscripten build labels nine JavaScript-provided libc and MEMFS
imports with its Preview 1 module namespace. The deterministic
`scripts/normalize-gba-engine-imports.mjs` transform renames only that import
module to `capsule.local.memfs.v1`; it does not change engine code or add host
authority. The product artifact has no Runtime WASI adapter, host preopens,
environment variables, sockets, or FIFOs. The capsule ABI remains
`elastos.runtime-projection/v1`.

ROM bytes enter through authenticated Runtime viewer routes. Save bytes leave
through the viewer's principal-scoped Runtime storage route. The capsule CSP
allows only same-origin Runtime requests.
