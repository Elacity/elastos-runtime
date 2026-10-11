// Remove one exact path below a directory this code owns, never through a link.
//
//   removeOwned(root, relative, expected = null) -> boolean
//
// `root` is the owned directory; its last component must be a real directory,
// the folders above it are the caller's trust anchor. `relative` names the
// exact path below it in plain components. Every folder from the root to the
// leaf's parent is checked with lstat and must be a real directory; the leaf
// is inspected without following it. A link is removed itself, a directory is
// removed entry by entry without following links inside it, anything else is
// unlinked. With `expected` = { dev, ino } (BigInt, from lstat with
// { bigint: true }) recorded earlier, a leaf with another identity is kept.
// Each folder's device+inode is checked again immediately before every
// removal, so a folder swapped for a link or another folder stops it.
//
// Node has no openat, so a folder swapped in the instant between that check
// and the removal is not excluded. Where another account can write inside
// the root, call scripts/lib/owned_removal.py instead (descriptor-relative).
//
// Returns false when the leaf or one of its folders does not exist and true
// after removing it. Throws UnsafeRemoval when a folder on the path is a link
// or not a folder, the path is not exact, or the identity changed.
//
// scripts/removal-guard.py refuses raw deletion added to product code; this is
// the Node helper it names. Rust: elastos_common::owned_removal.
import fs from "node:fs";
import path from "node:path";

export class UnsafeRemoval extends Error {}

function lstat(target) {
  try {
    return fs.lstatSync(target, { bigint: true });
  } catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
}

function sameIdentity(left, right) {
  return left.dev === right.dev && left.ino === right.ino;
}

function exactNames(relative) {
  const names = String(relative).split("/");
  if (path.isAbsolute(relative) || names.some((name) => name === "" || name === "." || name === "..")) {
    throw new UnsafeRemoval(`${relative} is not an exact path below the owned root`);
  }
  return names;
}

function confirmFolders(folders) {
  for (const [folder, identity] of folders) {
    const found = lstat(folder);
    if (!found || !found.isDirectory() || !sameIdentity(found, identity)) {
      throw new UnsafeRemoval(`${folder} changed during removal; nothing more was removed`);
    }
  }
}

function removeTree(folders, target, identity) {
  const inside = [...folders, [target, identity]];
  for (const name of fs.readdirSync(target)) {
    const child = path.join(target, name);
    const found = lstat(child);
    if (!found) continue;
    if (found.isDirectory()) {
      removeTree(inside, child, found);
    } else {
      confirmFolders(inside);
      fs.unlinkSync(child);
    }
  }
  confirmFolders(inside);
  fs.rmdirSync(target);
}

export function removeOwned(root, relative, expected = null) {
  const names = exactNames(relative);
  const folders = [];
  let current = root;
  for (const name of [null, ...names.slice(0, -1)]) {
    if (name !== null) current = path.join(current, name);
    const found = lstat(current);
    if (!found) return false;
    if (!found.isDirectory()) {
      throw new UnsafeRemoval(`${current} is a link or not a folder; nothing was removed`);
    }
    folders.push([current, found]);
  }
  const leaf = path.join(current, names.at(-1));
  const found = lstat(leaf);
  if (!found) return false;
  if (expected && !sameIdentity(found, expected)) {
    throw new UnsafeRemoval(`${relative} is no longer the recorded file; it was kept`);
  }
  if (found.isDirectory()) {
    removeTree(folders, leaf, found);
  } else {
    confirmFolders(folders);
    fs.unlinkSync(leaf);
  }
  return true;
}
