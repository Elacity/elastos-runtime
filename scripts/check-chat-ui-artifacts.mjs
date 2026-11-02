#!/usr/bin/env node

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { lstatSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = fileURLToPath(new URL("../", import.meta.url));
const STAMP = "capsules/chat-room-ui/.chat_room_ui_source.sha256";
const WASM = "capsules/chat-room/browser/chat_room_ui_bg.wasm";
const BINDINGS = "capsules/chat-room/browser/chat_room_ui.js";
const SCHEMA = "elastos.chat-room-ui-artifacts/v1";
const HASH_FIELDS = ["source_fingerprint_sha256", "compiler_wasm_sha256", "browser_wasm_sha256", "bindings_sha256"];

function readFile(root, path) {
  const absolute = resolve(root, path);
  assert(lstatSync(absolute).isFile(), `${path}: regular file required`);
  return readFileSync(absolute);
}

const sha256 = bytes => createHash("sha256").update(bytes).digest("hex");

export function sourceFingerprint(root = ROOT) {
  function sourceFiles(directory) {
    assert(lstatSync(resolve(root, directory)).isDirectory(), `${directory}: source directory required`);
    return readdirSync(resolve(root, directory), { withFileTypes: true }).flatMap(entry => {
      const path = `${directory}/${entry.name}`;
      if (entry.isDirectory()) return sourceFiles(path);
      assert(entry.isFile(), `${path}: regular source file required`);
      return [path];
    });
  }
  const sources = sourceFiles("capsules/chat-room-ui/src");
  assert(sources.length > 0, "Chat UI source files required");
  const files = [...sources, "capsules/chat-room-ui/Cargo.toml", "capsules/chat-room-ui/Cargo.lock", "rust-toolchain.toml"].sort();
  const hash = createHash("sha256");
  for (const path of files) {
    const bytes = readFile(root, path);
    // Relative names and byte lengths frame each input without checkout paths.
    hash.update(path).update("\0").update(String(bytes.length)).update("\0").update(bytes);
  }
  return hash.digest("hex");
}

export function checkArtifacts(root = ROOT) {
  const stamp = JSON.parse(readFile(root, STAMP).toString("utf8"));
  assert(stamp && typeof stamp === "object" && !Array.isArray(stamp), "Chat UI artifact stamp required");
  assert.deepEqual(Object.keys(stamp).sort(), ["schema", ...HASH_FIELDS].sort(), "Chat UI artifact stamp fields differ");
  assert.equal(stamp.schema, SCHEMA, "Chat UI artifact stamp schema differs");
  for (const field of HASH_FIELDS) assert(typeof stamp[field] === "string" && /^[0-9a-f]{64}$/.test(stamp[field]), `Chat UI artifact stamp ${field} is invalid`);
  assert.equal(stamp.source_fingerprint_sha256, sourceFingerprint(root), "Chat UI source differs from committed artifacts; run scripts/build-chat-room-ui.sh");
  assert.equal(stamp.browser_wasm_sha256, sha256(readFile(root, WASM)), "Chat UI browser WASM differs from its stamp");
  assert.equal(stamp.bindings_sha256, sha256(readFile(root, BINDINGS)), "Chat UI browser bindings differ from their stamp");
  return stamp;
}

export function compilerMatches(compilerWasm, root = ROOT) {
  return checkArtifacts(root).compiler_wasm_sha256 === sha256(readFile(root, compilerWasm));
}

export function writeStamp(compilerWasm, root = ROOT) {
  const stamp = {
    schema: SCHEMA,
    source_fingerprint_sha256: sourceFingerprint(root),
    compiler_wasm_sha256: sha256(readFile(root, compilerWasm)),
    browser_wasm_sha256: sha256(readFile(root, WASM)),
    bindings_sha256: sha256(readFile(root, BINDINGS)),
  };
  writeFileSync(resolve(root, STAMP), `${JSON.stringify(stamp, null, 2)}\n`);
  return stamp;
}

function main(args) {
  const [command = "check", input, root] = args;
  if (command === "check" && args.length <= 2) {
    checkArtifacts(input ? resolve(input) : ROOT);
    console.log("Chat UI source and committed artifacts match");
  } else if (["stamp", "compiler-match"].includes(command) && input && args.length <= 3) {
    const directory = root ? resolve(root) : ROOT;
    if (command === "stamp") writeStamp(input, directory);
    else if (!compilerMatches(input, directory)) process.exitCode = 1;
  } else throw new Error("usage: check-chat-ui-artifacts.mjs check [root] | stamp <compiler-wasm> [root] | compiler-match <compiler-wasm> [root]");
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  try { main(process.argv.slice(2)); }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}
