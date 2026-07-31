#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { BUTTON_BITS, gamepadMask } from "../capsules/gba-emulator/browser/gba-input.js";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const browserRoot = path.join(root, "capsules/gba-emulator/browser");
const provenance = await readFile(path.join(browserRoot, "UPSTREAM.md"), "utf8");
const emulator = await readFile(path.join(browserRoot, "emulator.js"), "utf8");
const projection = await readFile(path.join(browserRoot, "index.html"), "utf8");
const style = await readFile(path.join(browserRoot, "style.css"), "utf8");
const manifest = JSON.parse(await readFile(path.join(root, "capsules/gba-emulator/capsule.json")));
const ucityManifest = JSON.parse(await readFile(path.join(root, "capsules/gba-ucity/capsule.json")));
const mgbaJs = await readFile(path.join(browserRoot, "mgba.js"));
const mgbaWasm = await readFile(path.join(browserRoot, "mgba.wasm"));
const normalizer = await readFile(path.join(root, "scripts/normalize-gba-engine-imports.mjs"), "utf8");
const gbaAssetVersion = "gba-20260724a";

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

const buttons = Array.from({ length: 16 }, () => ({ pressed: false }));
buttons[0].pressed = true;
buttons[5].pressed = true;
buttons[8].pressed = true;
const mask = gamepadMask({ axes: [-0.8, 0.9], buttons });
assert(mask & BUTTON_BITS.a, "standard gamepad A was not mapped");
assert(mask & BUTTON_BITS.r, "standard gamepad R was not mapped");
assert(mask & BUTTON_BITS.select, "standard gamepad Select was not mapped");
assert(mask & BUTTON_BITS.left, "negative horizontal axis was not mapped");
assert(mask & BUTTON_BITS.down, "positive vertical axis was not mapped");
assert(!(mask & BUTTON_BITS.right), "opposite horizontal direction was also mapped");
assert(gamepadMask(null) === 0, "disconnected gamepad did not release all buttons");

assert(
  sha256(mgbaJs) === "0f37463aa2b7248564fd590fddf917ef3d8052ed0ed62d10b46717bb320bf3ea",
  "portable mGBA JavaScript does not match the pinned artifact",
);
assert(
  sha256(mgbaWasm) === "9e43a33a8477cca6c277cbaa809ea2c519d6085dd844758b5cbe8e9503251a27",
  "portable mGBA WebAssembly does not match the pinned artifact",
);
assert(
  provenance.includes("Package: `@thenick775/mgba-wasm` 1.1.1") &&
    provenance.includes("NPM integrity: `sha512-nzDWAFDBBEf+lfI6Zsr4Q0njqbAKZK1fvTsA66trTaO6q4dk0gPKo4Uiykr+AbbPWoqtmc4urIujbnll0pzxGA==`") &&
    provenance.includes("Source: `thenick775/mgba`, commit `67036729f29589a428c7568ce68c5ee88ac89d46`") &&
    provenance.includes("License: MPL-2.0") &&
    provenance.includes("Product `mgba.js` SHA-256: `0f37463aa2b7248564fd590fddf917ef3d8052ed0ed62d10b46717bb320bf3ea`") &&
    provenance.includes("Product `mgba.wasm` SHA-256: `9e43a33a8477cca6c277cbaa809ea2c519d6085dd844758b5cbe8e9503251a27`"),
  "portable mGBA provenance, license, or product hashes are incomplete",
);
assert(
  projection.includes(`style.css?v=${gbaAssetVersion}`) &&
    projection.includes(`emulator.js?v=${gbaAssetVersion}`),
  "GBA projection assets do not share the current cache identity",
);
const imports = WebAssembly.Module.imports(new WebAssembly.Module(mgbaWasm));
const allowedLocalMemfs = new Set([
  "fd_close",
  "fd_write",
  "fd_read",
  "fd_sync",
  "environ_sizes_get",
  "environ_get",
  "fd_seek",
]);
for (const item of imports) {
  assert(
    item.module === "env" ||
      (item.module === "capsule.local.memfs.v1" && allowedLocalMemfs.has(item.name)),
    `portable mGBA has an unexpected import: ${item.module}.${item.name}`,
  );
  assert(!/sock|path_open|fd_prestat/i.test(item.name), `portable mGBA imports host authority: ${item.name}`);
}
assert(
  !imports.some((item) => item.module === "wasi_snapshot_preview1"),
  "portable mGBA still exposes a WASI import namespace",
);
assert(
  normalizer.includes('const localModule = "capsule.local.memfs.v1"') &&
    normalizer.includes("expectedCount)"),
  "portable mGBA import normalization is not reproducible",
);
const mgbaSource = mgbaJs.toString("utf8");
assert(
  !mgbaSource.includes("SharedArrayBuffer") &&
    !mgbaSource.includes("Atomics.") &&
    !mgbaSource.includes("new Worker("),
  "portable mGBA unexpectedly carries a threaded runtime",
);

assert(emulator.includes('await import("./mgba.js")'), "the GBA engine is not loaded lazily");
assert(
  !emulator.includes("crossOriginIsolated") &&
    !emulator.includes("SharedArrayBuffer") &&
    !emulator.includes("typeof Worker"),
  "the GBA projection still requires browser thread isolation",
);
assert(emulator.includes("/content/${encodeURIComponent(request.capsule)}"), "content capsules do not use the viewer route");
assert(emulator.includes('raw: "true"'), "Library ROMs do not use the authenticated raw viewer route");
assert(
  emulator.includes("request?.capsule || VIEWER_ID") &&
    emulator.includes("/storage/${encodeURIComponent(capsule)}/save/") &&
    emulator.includes("/storage/${encodeURIComponent(activeStorageCapsule)}/state/"),
  "save and state storage do not preserve the launch-token capsule projection",
);
assert(!emulator.includes("/api/apps/gba-emulator/sessions"), "the removed native session gateway returned");
assert(!emulator.includes("new AudioContext"), "the projection duplicates the engine audio pipeline");
assert(emulator.includes('window.addEventListener("keydown"'), "keyboard input is not bound");
assert(emulator.includes('element.addEventListener("pointerdown"'), "touch input is not bound");
assert(emulator.includes("navigator.getGamepads"), "gamepad input is not polled");
assert(emulator.includes('window.addEventListener("pagehide"'), "capsule lifecycle cleanup is not bound");
assert(
  emulator.includes('{ type: "home:open-target", target: "library", homeToken }'),
  "GBA does not request Library through Home",
);
assert(manifest.runtime_abi === "elastos.runtime-projection/v1", "viewer is not a Runtime projection");
assert(!manifest.capabilities?.length, "the portable viewer declares an unrelated provider capability");
assert(
  !manifest.requires?.some((item) => item.name === "gba-engine-provider"),
  "the portable viewer still requires a host engine provider",
);
assert(
  manifest.permissions?.storage?.includes("localhost://Users/self/.AppData/LocalHost/GBA/*"),
  "the viewer lacks principal-scoped save storage",
);
assert(
  ucityManifest.permissions?.storage?.includes(
    "localhost://Users/self/.AppData/LocalHost/GBA/ucity/*",
  ),
  "uCity lacks its principal-scoped save storage",
);
assert(
  projection.includes("connect-src 'self'") && projection.includes("worker-src 'none'"),
  "the GBA capsule does not constrain engine networking and workers",
);
assert(
  projection.includes('id="btn-ff"') &&
    projection.includes('id="emulator-card"') &&
    projection.includes('class="screen-card"') &&
    projection.includes('class="utility-card"') &&
    projection.includes('id="utility-panel"') &&
    projection.includes('id="btn-save1"') &&
    projection.includes('id="btn-load3"') &&
    projection.includes('id="slot-status1"') &&
    projection.includes('id="power-led"') &&
    !projection.includes('id="utility-toggle"') &&
    !projection.includes('id="file-input"') &&
    !projection.includes('id="sound"'),
  "the responsive Runtime viewer controls were replaced or removed",
);
assert(
  !emulator.includes("resumeAudio") &&
    emulator.includes("engine.setVolume(Number(volume.value) / 100)") &&
    emulator.includes("enableSound().catch"),
  "audio is not enabled by the first game control gesture",
);
assert(
  emulator.includes('slotStatus.textContent = saved ? "Saved" : "Empty"'),
  "save-state controls do not update their visible slot status",
);
assert(
  style.includes(".emulator-card {") &&
    style.includes("grid-template-columns: minmax(0, 1fr) 15.75rem;") &&
    style.includes(".screen-card,") &&
    style.includes(".utility-card {") &&
    style.includes(".state-grid {") &&
    style.includes(".touch-controls {") &&
    style.includes("@media (max-width: 780px)") &&
    !style.includes("--gba-width"),
  "GBA controls no longer preserve the responsive embedded viewer layout",
);

console.log(
  `[gba-projection] OK portable=1 lazy=1 threads=none imports=${imports.length} keyboard=ok touch=ok gamepad=ok runtime_io=ok`,
);
