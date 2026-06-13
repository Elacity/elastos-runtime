# Upstream Runtime

This capsule vendors the browser runtime from:

- npm: `@thenick775/mgba-wasm@2.4.1`
- repository: <https://github.com/thenick775/mgba.git>
- license: MPL-2.0

These files are vendored here:

- `mgba.js`
- `mgba.wasm`

Refresh with:

```bash
bash scripts/vendor-gba-runtime.sh
```

This viewer expects COOP/COEP headers so SharedArrayBuffer and threaded WASM remain available.
ElastOS already applies those headers when serving web capsules.

## Mobile / WebView support

The vendored mGBA build is compiled with Emscripten pthreads. Threaded WASM requires
Web Workers plus shared WebAssembly memory, and browsers only expose shared memory
through `SharedArrayBuffer` in a cross-origin-isolated context. Full browsers can
usually satisfy this with the capsule COOP/COEP headers. Some embedded WebViews do
not expose `SharedArrayBuffer` even when the headers are present.

Current behavior is capability detection, not user-agent detection: if
`SharedArrayBuffer`, `crossOriginIsolated`, Workers, or shared WASM memory are not
available, the capsule fails fast with a clear message instead of hanging at
initialization.

Viable paths if GBA must run in those WebViews:

- Build and vendor a non-threaded mGBA WASM artifact. This is the cleanest browser
  alternative but may reduce performance.
- Vendor a different single-threaded emulator core for constrained WebViews.
- Add a native/mobile host adapter that can provide the required isolation and
  worker settings outside the generic browser capsule path.
