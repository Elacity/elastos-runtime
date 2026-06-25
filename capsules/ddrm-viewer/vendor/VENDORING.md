# Vendored third-party assets — `ddrm-viewer`

These files are **pinned, vendored copies** of upstream three.js (no package manager, no
auto-update). They render decrypted 3D models (`.glb`/`.gltf`/`.stl`/`.obj`) inside the
viewer. Recording the pin here so a refresh is deliberate and auditable, not silent.

## Pin

| Asset | Upstream | Version | License |
|---|---|---|---|
| `three.module.js` | three.js core | **r160** (`REVISION = '160'`, ©2010–2023) | MIT |
| `controls/OrbitControls.js` | three.js examples | r160 | MIT |
| `loaders/GLTFLoader.js` | three.js examples | r160 | MIT |
| `loaders/STLLoader.js` | three.js examples | r160 | MIT |
| `loaders/OBJLoader.js` | three.js examples | r160 | MIT |
| `utils/BufferGeometryUtils.js` | three.js examples | r160 | MIT |

Source: <https://github.com/mrdoob/three.js> (tag `r160`). The examples modules
(`controls/`, `loaders/`, `utils/`) must stay on the **same revision** as `three.module.js`
— three.js does not guarantee cross-revision compatibility between core and examples.

## Refresh / upstream-watch plan

- **Trigger a refresh** on: a three.js security advisory (watch the repo's GitHub Security
  Advisories / `npm audit` for `three`), or when a viewer feature needs a newer revision.
- **How:** bump all six files together to the new tag from the same upstream release, keep
  `three.module.js` and the `examples/` modules on one revision, update the table above, and
  re-run the 3D-model open path against `.glb`/`.gltf`/`.stl`/`.obj` fixtures.
- **Do not** hand-edit these files; re-vendor from upstream so the pin stays verifiable.
- **Boundary note:** this code runs in the viewer (browser) on **already-decrypted** bytes,
  below the dDRM boundary — it never sees key material. A vendored-dependency compromise is a
  client-side render risk, not a CEK-exfiltration path.
