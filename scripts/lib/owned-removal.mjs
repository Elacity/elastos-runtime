// Remove one exact path below a directory this code owns, never through a link.
//
//   removeOwned(root, relative, expected = null)
//
// Node has no openat or unlinkat, so a check-then-remove by path could follow
// a folder swapped for a link in between. This runs scripts/lib/owned_removal.py
// instead, which walks and removes through directory descriptors; its header
// states the rules and the accepted limit. `expected` is a { dev, ino } recorded
// earlier (for example from fs.lstatSync(path, { bigint: true })); a leaf with
// another identity is kept. Returns after removing or when the path is absent;
// throws when the removal was refused or failed.
//
// scripts/removal-guard.py refuses raw deletion added to product code; this is
// the Node helper it names. Rust: elastos_common::owned_removal.
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const HELPER = fileURLToPath(new URL("./owned_removal.py", import.meta.url));

export function removeOwned(root, relative, expected = null) {
  const args = [HELPER, String(root), String(relative)];
  if (expected) args.push("--expect", `${expected.dev}:${expected.ino}`);
  const result = spawnSync("python3", args, { encoding: "utf8", stdio: ["ignore", "ignore", "pipe"] });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(result.stderr.trim() || `owned removal exited with ${result.status ?? result.signal}`);
  }
}
