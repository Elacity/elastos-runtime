# Vendored three.js

Elacity Reader vendors the 3D renderer and the four model readers it needs so
the capsule can show a protected model without fetching anything at open time.

Source package:

- npm package: `three`
- version: `0.160.0`
- license: MIT
- repository: `https://github.com/mrdoob/three.js`
- tarball: `https://registry.npmjs.org/three/-/three-0.160.0.tgz`
- npm integrity: `sha512-DLU8lc0zNIPkM7rH5/e1Ks1Z8tWCGRq6g8mPowdDJpw1CFBJMU7UoJjC6PefXW7z//SSl0b2+GCw14LB+uDhng==`
- npm shasum: `cd1e4dbd01aee0719280a9086d75545db52b7a8f`

Vendored files are byte-identical to these package paths:

| Local file | Package path | SHA-256 |
| --- | --- | --- |
| `LICENSE` | `package/LICENSE` | `852e0e8699169bf9f6fdc6bda3e682d078dcbc738b5d33e74df594721bff271d` |
| `three.module.js` | `package/build/three.module.js` | `76dea8151bc9352aef3528b4262e249b2604f62543828328db978d060d61a495` |
| `controls/OrbitControls.js` | `package/examples/jsm/controls/OrbitControls.js` | `5a44a9e86a2a0fb11933eed69bc2cd33c76a496854c1aed6ed776efa87d7b064` |
| `loaders/GLTFLoader.js` | `package/examples/jsm/loaders/GLTFLoader.js` | `d073b438e6a07e1359741dd5d6c76c953420cc0d4fd84eb1bdde94315540e6a3` |
| `loaders/OBJLoader.js` | `package/examples/jsm/loaders/OBJLoader.js` | `022e0334f837c60506276e136faf3e54b21b20aea672eed4a0c50651d4fc0a5d` |
| `loaders/STLLoader.js` | `package/examples/jsm/loaders/STLLoader.js` | `896d006a48b8f125385a485ccae154dadee801a953f0b45ceffe7ddd8a29ca93` |
| `utils/BufferGeometryUtils.js` | `package/examples/jsm/utils/BufferGeometryUtils.js` | `9be041e96308775d00e2695cc607645b9a9b64fd7c0e759dd8f7c00a8d92becb` |

Verification command:

```sh
tmpdir="$(mktemp -d /tmp/three-0.160.0.XXXXXX)"
curl -fsSL 'https://registry.npmjs.org/three/-/three-0.160.0.tgz' -o "$tmpdir/three-0.160.0.tgz"
tar -xzf "$tmpdir/three-0.160.0.tgz" -C "$tmpdir"
cmp -s "$tmpdir/package/LICENSE" capsules/elacity-reader/browser/vendor/three/LICENSE
cmp -s "$tmpdir/package/build/three.module.js" capsules/elacity-reader/browser/vendor/three/three.module.js
cmp -s "$tmpdir/package/examples/jsm/controls/OrbitControls.js" capsules/elacity-reader/browser/vendor/three/controls/OrbitControls.js
cmp -s "$tmpdir/package/examples/jsm/loaders/GLTFLoader.js" capsules/elacity-reader/browser/vendor/three/loaders/GLTFLoader.js
cmp -s "$tmpdir/package/examples/jsm/loaders/OBJLoader.js" capsules/elacity-reader/browser/vendor/three/loaders/OBJLoader.js
cmp -s "$tmpdir/package/examples/jsm/loaders/STLLoader.js" capsules/elacity-reader/browser/vendor/three/loaders/STLLoader.js
cmp -s "$tmpdir/package/examples/jsm/utils/BufferGeometryUtils.js" capsules/elacity-reader/browser/vendor/three/utils/BufferGeometryUtils.js
```

## Why the page carries a name map

The five files under `controls/`, `loaders/` and `utils/` are shipped by the
package with a plain `import … from 'three'`, and there is no build step here
to turn that name into an address. Rewriting the line in the copy would break
the byte-for-byte promise above, so `browser/index.html` carries a short name
map instead, and the page's own rules admit that one block by its exact
content hash — nothing else inline can run. The map's text and the hash in the
rules are pinned together in `scripts/home-entropy-check.mjs`, so a change to
one without the other fails the check rather than the page.
