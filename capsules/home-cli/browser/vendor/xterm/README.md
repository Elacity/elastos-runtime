# Vendored xterm.js

Home CLI vendors the browser terminal renderer so the capsule can run without
network fetches at launch time.

Source package:

- npm package: `@xterm/xterm`
- version: `6.0.0`
- license: MIT
- repository: `https://github.com/xtermjs/xterm.js`
- tarball: `https://registry.npmjs.org/@xterm/xterm/-/xterm-6.0.0.tgz`
- npm integrity: `sha512-TQwDdQGtwwDt+2cgKDLn0IRaSxYu1tSUjgKarSDkUM0ZNiSRXFpjxEsvc/Zgc5kq5omJ+V0a8/kIM2WD3sMOYg==`
- npm shasum: `93637b0f2ee3a70718b5746a27c9c506af16745b`

Vendored files are byte-identical to these package paths:

| Local file | Package path | SHA-256 |
| --- | --- | --- |
| `LICENSE` | `package/LICENSE` | `b569f629d00f2626a8100df2a1798210535621e42164dfd426a6fe5aac7b0ccd` |
| `xterm.css` | `package/css/xterm.css` | `854a7c0fb70e8b1a083c16797ab827299fb18744f5ad34f227b48337e33293c6` |
| `xterm.mjs` | `package/lib/xterm.mjs` | `b336ec65a086c056d4804b3d4c2347da5663d3f23c3f25be866467bd8857ad59` |

Verification command:

```sh
tmpdir="$(mktemp -d /tmp/xterm-6.0.0.XXXXXX)"
curl -fsSL 'https://registry.npmjs.org/@xterm/xterm/-/xterm-6.0.0.tgz' -o "$tmpdir/xterm-6.0.0.tgz"
tar -xzf "$tmpdir/xterm-6.0.0.tgz" -C "$tmpdir"
cmp -s "$tmpdir/package/LICENSE" capsules/home-cli/browser/vendor/xterm/LICENSE
cmp -s "$tmpdir/package/css/xterm.css" capsules/home-cli/browser/vendor/xterm/xterm.css
cmp -s "$tmpdir/package/lib/xterm.mjs" capsules/home-cli/browser/vendor/xterm/xterm.mjs
```
