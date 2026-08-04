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
  upstreamJs: "18a379a8a316c58fff601253673862d3c9015adb5adc318e2b395c7cc7ec6c0c",
  upstreamWasm: "bed02835f672a48b8be59f4e4cd65594109f2b54f30100539c6fd12c022d4bf9",
  productJs: "0f37463aa2b7248564fd590fddf917ef3d8052ed0ed62d10b46717bb320bf3ea",
  productWasm: "9e43a33a8477cca6c277cbaa809ea2c519d6085dd844758b5cbe8e9503251a27",
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
  if (localImports.length !== 7) {
    throw new Error(`expected 7 capsule-local MEMFS imports, found ${localImports.length}`);
  }
  if (!js.includes(`'${localModule}': wasmImports`)) {
    throw new Error("GBA JavaScript does not provide the capsule-local MEMFS imports");
  }
  for (const threadedRuntime of ["SharedArrayBuffer", "Atomics.", "new Worker("]) {
    if (js.includes(threadedRuntime)) {
      throw new Error(`GBA JavaScript unexpectedly contains ${threadedRuntime}`);
    }
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
  wasm = replaceExact(wasm, upstreamModule, localModule, 7);
  await writeFile(jsPath, js);
  await writeFile(wasmPath, wasm);
}

validateProduct(js, wasm);
console.log("[gba-engine-imports] OK namespace=capsule.local.memfs.v1 imports=7 threads=none");
