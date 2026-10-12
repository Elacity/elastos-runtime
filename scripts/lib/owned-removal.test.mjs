import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { removeOwned, UnsafeRemoval } from "./owned-removal.mjs";

function tree(t) {
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "owned-removal-"));
  t.after(() => fs.rmSync(scratch, { recursive: true, force: true }));
  const root = path.join(scratch, "root");
  const outside = path.join(scratch, "outside");
  fs.mkdirSync(path.join(root, "owned/nested"), { recursive: true });
  fs.mkdirSync(outside);
  fs.writeFileSync(path.join(root, "owned/file"), "owned");
  fs.writeFileSync(path.join(outside, "file"), "outside");
  return { scratch, root, outside };
}

test("removes an exact file and reports absence", (t) => {
  const { root } = tree(t);
  assert.equal(removeOwned(root, "owned/file"), true);
  assert.equal(fs.existsSync(path.join(root, "owned/file")), false);
  assert.equal(removeOwned(root, "owned/file"), false);
  assert.equal(removeOwned(root, "missing/file"), false);
});

test("never removes through a linked folder or linked root", (t) => {
  const { scratch, root, outside } = tree(t);
  fs.symlinkSync(outside, path.join(root, "linked"));
  assert.throws(() => removeOwned(root, "linked/file"), UnsafeRemoval);
  const linkedRoot = path.join(scratch, "linked-root");
  fs.symlinkSync(outside, linkedRoot);
  assert.throws(() => removeOwned(linkedRoot, "file"), UnsafeRemoval);
  assert.equal(fs.readFileSync(path.join(outside, "file"), "utf8"), "outside");
});

test("a linked leaf is removed itself and links inside a tree are not followed", (t) => {
  const { root, outside } = tree(t);
  fs.symlinkSync(outside, path.join(root, "owned/nested/to-outside"));
  fs.symlinkSync(outside, path.join(root, "leaf-link"));
  assert.equal(removeOwned(root, "leaf-link"), true);
  assert.equal(removeOwned(root, "owned"), true);
  assert.equal(fs.existsSync(path.join(root, "owned")), false);
  assert.equal(fs.readFileSync(path.join(outside, "file"), "utf8"), "outside");
});

test("a replaced leaf is kept", (t) => {
  const { root } = tree(t);
  const file = path.join(root, "owned/file");
  const recorded = fs.lstatSync(file, { bigint: true });
  fs.renameSync(file, path.join(root, "owned/moved"));
  fs.writeFileSync(file, "replacement");
  assert.throws(() => removeOwned(root, "owned/file", recorded), UnsafeRemoval);
  assert.equal(fs.readFileSync(file, "utf8"), "replacement");
  assert.equal(removeOwned(root, "owned/file", fs.lstatSync(file, { bigint: true })), true);
});

test("only plain relative names are accepted", (t) => {
  const { root } = tree(t);
  for (const relative of ["", "/etc/passwd", "../outside/file", "owned/../owned/file", "owned//file"]) {
    assert.throws(() => removeOwned(root, relative), UnsafeRemoval, relative);
  }
  assert.equal(fs.existsSync(path.join(root, "owned/file")), true);
});
