#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const browserRoot = resolve(root, "capsules/gba-emulator/browser");
const jsPath = resolve(browserRoot, "mgba.js");
const wasmPath = resolve(browserRoot, "mgba.wasm");
const upstreamModule = "wasi_snapshot_preview1";
const localModule = "capsule.local.memfs.v1";
const expected = {
  upstreamJs: "78e30a6542173e349e27b3cd3652f20d69b41ed742d1a80e64e253d17e25918a",
  upstreamWasm: "546a99648d2ef52cb04e34e19a4d0ad2d5dc6bcf0f6749bbaba7d5771226f002",
  productJs: "7ddadd7c564293bd6552fd9640e2ae85d927a2d756323c0f4e526aa4ccc72111",
  productWasm: "69b9fccd6cc616682a92866c2a3ad846ded3618661e3258da6582fbf54a2482e",
};

function hash(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function replaceExact(bytes, from, to, expectedCount) {
  if (from.length !== to.length) {
    throw new Error("GBA import module names must have equal byte lengths");
  }
  const source = bytes.toString("latin1");
  const count = source.split(from).length - 1;
  if (count !== expectedCount) {
    throw new Error(`expected ${expectedCount} ${from} references, found ${count}`);
  }
  return Buffer.from(source.split(from).join(to), "latin1");
}

function validateProduct(js, wasm) {
  if (hash(js) !== expected.productJs || hash(wasm) !== expected.productWasm) {
    throw new Error("normalized GBA engine does not match the pinned product hashes");
  }
  const imports = WebAssembly.Module.imports(new WebAssembly.Module(wasm));
  if (imports.some((item) => item.module === upstreamModule)) {
    throw new Error("normalized GBA engine still imports the upstream WASI namespace");
  }
  const localImports = imports.filter((item) => item.module === localModule);
  if (localImports.length !== 9) {
    throw new Error(`expected 9 capsule-local MEMFS imports, found ${localImports.length}`);
  }
  if (!js.includes(`'${localModule}': wasmImports`)) {
    throw new Error("GBA JavaScript does not provide the capsule-local MEMFS imports");
  }
}

let js = await readFile(jsPath);
let wasm = await readFile(wasmPath);
const write = process.argv.includes("--write");

if (hash(js) === expected.upstreamJs && hash(wasm) === expected.upstreamWasm) {
  if (!write) {
    throw new Error("upstream GBA engine is not normalized; run this script with --write");
  }
  js = Buffer.from(
    js
      .toString("utf8")
      .replace(`'${upstreamModule}': wasmImports`, `'${localModule}': wasmImports`),
  );
  wasm = replaceExact(wasm, upstreamModule, localModule, 9);
  await writeFile(jsPath, js);
  await writeFile(wasmPath, wasm);
}

validateProduct(js, wasm);
console.log("[gba-engine-imports] OK namespace=capsule.local.memfs.v1 imports=9");
