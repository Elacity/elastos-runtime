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
