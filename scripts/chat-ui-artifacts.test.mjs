import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { checkArtifacts, compilerMatches, sourceFingerprint, writeStamp } from "./check-chat-ui-artifacts.mjs";

const STAMP = "capsules/chat-room-ui/.chat_room_ui_source.sha256";

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), "elastos-chat-ui-artifacts-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const files = {
    "capsules/chat-room-ui/src/lib.rs": "mod direct;\n",
    "capsules/chat-room-ui/src/direct.rs": "pub fn render() {}\n",
    "capsules/chat-room-ui/Cargo.toml": "[package]\nname = 'chat-room-ui'\n",
    "capsules/chat-room-ui/Cargo.lock": "version = 4\n",
    "rust-toolchain.toml": "[toolchain]\nchannel = '1.91.0'\n",
    "compiler.wasm": Buffer.from([0, 97, 115, 109, 1]),
    "capsules/chat-room/browser/chat_room_ui_bg.wasm": Buffer.from([0, 97, 115, 109, 2]),
    "capsules/chat-room/browser/chat_room_ui.js": "export function init() {}\n",
  };
  for (const [path, bytes] of Object.entries(files)) {
    const absolute = join(root, path);
    mkdirSync(join(absolute, ".."), { recursive: true });
    writeFileSync(absolute, bytes);
  }
  writeStamp("compiler.wasm", root);
  return root;
}

test("unchanged source and browser artifacts pass, with compiler hash controlling bindgen reuse", t => {
  const root = fixture(t);
  const first = readFileSync(join(root, STAMP));
  assert.equal(checkArtifacts(root).source_fingerprint_sha256, sourceFingerprint(root));
  assert.equal(compilerMatches("compiler.wasm", root), true);
  writeFileSync(join(root, "compiler.wasm"), "different compiler output");
  assert.equal(compilerMatches("compiler.wasm", root), false);
  assert.deepEqual(readFileSync(join(root, STAMP)), first);
});

test("source fingerprint includes every source name and byte, Cargo inputs and the toolchain", t => {
  for (const path of ["capsules/chat-room-ui/src/lib.rs", "capsules/chat-room-ui/src/direct.rs", "capsules/chat-room-ui/Cargo.toml", "capsules/chat-room-ui/Cargo.lock", "rust-toolchain.toml"]) {
    const root = fixture(t);
    writeFileSync(join(root, path), "changed input\n");
    assert.throws(() => checkArtifacts(root), /Chat UI source differs/);
    assert.throws(() => compilerMatches("compiler.wasm", root), /Chat UI source differs/);
  }
  const root = fixture(t);
  renameSync(join(root, "capsules/chat-room-ui/src/direct.rs"), join(root, "capsules/chat-room-ui/src/renamed.rs"));
  assert.throws(() => checkArtifacts(root), /Chat UI source differs/);
  const added = fixture(t);
  mkdirSync(join(added, "capsules/chat-room-ui/src/nested"));
  writeFileSync(join(added, "capsules/chat-room-ui/src/nested/new.rs"), "new source\n");
  assert.throws(() => checkArtifacts(added), /Chat UI source differs/);
});

test("the fingerprint is independent of checkout location and directory creation order", t => {
  const a = fixture(t), b = fixture(t);
  const path = "capsules/chat-room-ui/src/direct.rs";
  const bytes = readFileSync(join(b, path));
  unlinkSync(join(b, path));
  writeFileSync(join(b, path), bytes);
  assert.equal(sourceFingerprint(a), sourceFingerprint(b));
  assert.deepEqual(checkArtifacts(a), checkArtifacts(b));
});

test("changed browser WASM or bindings refuse both source gate and compiler reuse", t => {
  for (const [path, message] of [["capsules/chat-room/browser/chat_room_ui_bg.wasm", /browser WASM differs/], ["capsules/chat-room/browser/chat_room_ui.js", /browser bindings differ/]]) {
    const root = fixture(t);
    writeFileSync(join(root, path), "changed artifact");
    assert.throws(() => checkArtifacts(root), message);
    assert.throws(() => compilerMatches("compiler.wasm", root), message);
  }
});

test("missing, old and malformed stamps are refused", t => {
  const root = fixture(t);
  unlinkSync(join(root, STAMP));
  assert.throws(() => checkArtifacts(root), /ENOENT/);
  for (const bytes of ["not JSON", "a".repeat(64), "null", JSON.stringify({ schema: "old" })]) {
    writeFileSync(join(root, STAMP), bytes);
    assert.throws(() => checkArtifacts(root));
  }
  const stamp = writeStamp("compiler.wasm", root);
  for (const mutate of [s => { s.schema = "old"; }, s => { s.compiler_wasm_sha256 = "invalid"; }, s => { s.extra = true; }]) {
    const changed = structuredClone(stamp);
    mutate(changed);
    writeFileSync(join(root, STAMP), JSON.stringify(changed));
    assert.throws(() => checkArtifacts(root), /stamp/);
  }
});
