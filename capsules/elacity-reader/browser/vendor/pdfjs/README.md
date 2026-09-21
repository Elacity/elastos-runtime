# Vendored pdf.js

Elacity Reader vendors the PDF renderer so the capsule can draw a protected
document without fetching anything at open time.

Source package:

- npm package: `pdfjs-dist`
- version: `4.10.38`
- license: Apache-2.0
- repository: `https://github.com/mozilla/pdf.js`
- tarball: `https://registry.npmjs.org/pdfjs-dist/-/pdfjs-dist-4.10.38.tgz`
- npm integrity: `sha512-/Y3fcFrXEAsMjJXeL9J8+ZG9U01LbuWaYypvDW2ycW1jL269L3js3DVBjDJ0Up9Np1uqDXsDrRihHANhZOlwdQ==`
- npm shasum: `3ee698003790dc266cc8b55c0e662ccb9ae18f53`

Vendored files are byte-identical to these package paths:

| Local file | Package path | SHA-256 |
| --- | --- | --- |
| `LICENSE` | `package/LICENSE` | `0d542e0c8804e39aa7f37eb00da5a762149dc682d7829451287e11b938e94594` |
| `pdf.min.mjs` | `package/build/pdf.min.mjs` | `27fc2a057a00f92a4334ad06e17dbd7259912954e9fb7f76400bcca5fd190a9c` |
| `pdf.worker.min.mjs` | `package/build/pdf.worker.min.mjs` | `1baa1844c89c80a5b2797c916e75ab29254be46d8e9cb53cb6364d7aad84be36` |

Verification command:

```sh
tmpdir="$(mktemp -d /tmp/pdfjs-4.10.38.XXXXXX)"
curl -fsSL 'https://registry.npmjs.org/pdfjs-dist/-/pdfjs-dist-4.10.38.tgz' -o "$tmpdir/pdfjs-dist-4.10.38.tgz"
tar -xzf "$tmpdir/pdfjs-dist-4.10.38.tgz" -C "$tmpdir"
cmp -s "$tmpdir/package/LICENSE" capsules/elacity-reader/browser/vendor/pdfjs/LICENSE
cmp -s "$tmpdir/package/build/pdf.min.mjs" capsules/elacity-reader/browser/vendor/pdfjs/pdf.min.mjs
cmp -s "$tmpdir/package/build/pdf.worker.min.mjs" capsules/elacity-reader/browser/vendor/pdfjs/pdf.worker.min.mjs
```

## Why both files load on the page thread

This page runs at an origin of its own that matches nothing, and a background
thread cannot be started there by either route:

- from a file address, `new Worker("./pdf.worker.min.mjs", { type: "module" })`
  raises `SecurityError: … cannot be accessed from origin 'null'`;
- from a minted address, a module thread never starts either — measured with a
  one-line script, so it is the address and not the size or the contents of
  this file that is refused. `pdf.worker.min.mjs` is a module, so the ordinary
  fallback of minting an address over the fetched source does not apply here.
  A plain, non-module thread does start from a minted address, but this file
  cannot run as one.

Both refusals are asserted every run by `scripts/elacity-reader-smoke.mjs`,
which drives the page in a real engine at that origin and also renders a
two-page document there, so this note is a standing measurement rather than a
remembered one.

`render.js` therefore imports both files directly and hands the reading half to
the drawing half itself, before the first document is opened:

```js
globalThis.pdfjsWorker = { WorkerMessageHandler };
```

The library checks for exactly that and then never tries to start a background
thread at all, so the page's rules can keep `worker-src 'none'` truthfully. A
document is read and drawn on the page thread instead, which is slower for a
long file and is the honest cost of the origin this page runs at.

Character maps and the substitute font set are not vendored. A document that
embeds its fonts — which is nearly all of them — draws exactly; one that relies
on the reader supplying a font falls back to a system face, and text in a CJK
document that carries no embedded font may not draw at all.
