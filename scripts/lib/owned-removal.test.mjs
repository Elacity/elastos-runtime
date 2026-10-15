import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { removeOwned } from "./owned-removal.mjs";

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

test("removes an exact file; an absent one is not an error", (t) => {
  const { root } = tree(t);
  removeOwned(root, "owned/file");
  assert.equal(fs.existsSync(path.join(root, "owned/file")), false);
  removeOwned(root, "owned/file");
  removeOwned(root, "missing/file");
});

test("never removes through a linked folder or a linked root, however it is spelled", (t) => {
  const { scratch, root, outside } = tree(t);
  fs.symlinkSync(outside, path.join(root, "linked"));
  assert.throws(() => removeOwned(root, "linked/file"), /link or not a folder/);
  const linkedRoot = path.join(scratch, "linked-root");
  fs.symlinkSync(outside, linkedRoot);
  for (const spelling of [linkedRoot, `${linkedRoot}/`, `${linkedRoot}/.`]) {
    assert.throws(() => removeOwned(spelling, "file"), /link or not a folder/, spelling);
  }
  assert.equal(fs.readFileSync(path.join(outside, "file"), "utf8"), "outside");
});

test("a leaf replaced before the identity check is kept", (t) => {
  const { root, outside } = tree(t);
  const file = path.join(root, "owned/file");
  const recorded = fs.lstatSync(file, { bigint: true });
  fs.renameSync(file, path.join(root, "owned/moved"));
  fs.symlinkSync(path.join(outside, "file"), file);
  assert.throws(() => removeOwned(root, "owned/file", recorded), /no longer the recorded file/);
  assert.equal(fs.readFileSync(path.join(outside, "file"), "utf8"), "outside");
  removeOwned(root, "owned/file", fs.lstatSync(file, { bigint: true }));
  assert.equal(fs.existsSync(file), false);
  assert.equal(fs.readFileSync(path.join(outside, "file"), "utf8"), "outside");
});
