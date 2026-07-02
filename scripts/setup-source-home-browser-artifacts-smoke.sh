#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

parent="$tmp_dir/xdg/elastos"
linux_managed="$parent/managed-runtimes/home/home/.local/share/elastos"
mac_managed="$parent/managed-runtimes/mac/home/.local/share/elastos"
explicit_source="$tmp_dir/explicit-artifacts"

mkdir -p \
  "$parent/bin" \
  "$parent/browser-vm" \
  "$linux_managed/bin" \
  "$linux_managed/browser-vm" \
  "$mac_managed/bin" \
  "$mac_managed/browser-vm" \
  "$explicit_source/bin" \
  "$explicit_source/browser-vm"

printf 'parent-crosvm\n' > "$parent/bin/crosvm"
printf 'parent-kernel\n' > "$parent/bin/vmlinux"
printf 'parent-linux-initrd\n' > "$parent/browser-vm/initrd"
printf 'parent-rootfs\n' > "$parent/browser-vm/rootfs.ext4"
printf 'parent-vz-initrd\n' > "$parent/bin/initrd"

printf 'keep-existing-kernel\n' > "$linux_managed/bin/vmlinux"

linux_output="$("$repo_root/scripts/setup-source-home-browser-artifacts.sh" \
  --data-dir "$linux_managed" \
  --platform linux-arm64)"

OUTPUT="$linux_output" \
PARENT="$parent" \
MANAGED="$linux_managed" \
node - <<'NODE'
const fs = require("node:fs");
const path = require("node:path");

const output = process.env.OUTPUT.trim().split(/\n/).at(-1);
const receipt = JSON.parse(output);
const parent = process.env.PARENT;
const managed = process.env.MANAGED;
if (receipt.schema !== "elastos.setup-source-home.browser-artifacts/v1") throw new Error("wrong schema");
if (receipt.linked !== 3) throw new Error(`expected 3 Linux links, got ${receipt.linked}`);
if (fs.readFileSync(path.join(managed, "bin/vmlinux"), "utf8") !== "keep-existing-kernel\n") {
  throw new Error("existing real kernel file must not be replaced");
}
for (const [dest, source] of [
  ["bin/crosvm", "bin/crosvm"],
  ["browser-vm/initrd", "browser-vm/initrd"],
  ["browser-vm/rootfs.ext4", "browser-vm/rootfs.ext4"],
]) {
  const actual = fs.readlinkSync(path.join(managed, dest));
  const expected = path.join(parent, source);
  if (actual !== expected) throw new Error(`${dest} link mismatch: ${actual} !== ${expected}`);
}
if (fs.existsSync(path.join(managed, "bin/initrd"))) {
  throw new Error("Linux managed setup must not create the Mac VZ initrd path");
}
NODE

printf 'explicit-kernel\n' > "$explicit_source/bin/vmlinux"
printf 'explicit-rootfs\n' > "$explicit_source/browser-vm/rootfs.ext4"
printf 'explicit-vz-initrd\n' > "$explicit_source/bin/initrd"

mac_output="$("$repo_root/scripts/setup-source-home-browser-artifacts.sh" \
  --data-dir "$mac_managed" \
  --platform darwin-arm64 \
  --artifact-data-dir "$explicit_source")"

OUTPUT="$mac_output" \
SOURCE="$explicit_source" \
MANAGED="$mac_managed" \
node - <<'NODE'
const fs = require("node:fs");
const path = require("node:path");

const output = process.env.OUTPUT.trim().split(/\n/).at(-1);
const receipt = JSON.parse(output);
const source = process.env.SOURCE;
const managed = process.env.MANAGED;
if (receipt.schema !== "elastos.setup-source-home.browser-artifacts/v1") throw new Error("wrong schema");
if (receipt.linked !== 3) throw new Error(`expected 3 Mac links, got ${receipt.linked}`);
for (const [dest, sourceRel] of [
  ["bin/vmlinux", "bin/vmlinux"],
  ["browser-vm/rootfs.ext4", "browser-vm/rootfs.ext4"],
  ["bin/initrd", "bin/initrd"],
]) {
  const actual = fs.readlinkSync(path.join(managed, dest));
  const expected = path.join(source, sourceRel);
  if (actual !== expected) throw new Error(`${dest} link mismatch: ${actual} !== ${expected}`);
}
if (fs.existsSync(path.join(managed, "bin/crosvm"))) {
  throw new Error("Mac managed setup must not create a crosvm link");
}
NODE

printf '%s\n' '{"schema":"elastos.setup-source-home.browser-artifacts-smoke/v1","ok":true}'
