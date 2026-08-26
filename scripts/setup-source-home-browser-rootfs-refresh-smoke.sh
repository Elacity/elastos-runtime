#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

find_e2fs_tool() {
  local name="$1"
  local candidate
  if command -v "$name" >/dev/null 2>&1; then
    command -v "$name"
    return
  fi
  for candidate in \
    "/opt/homebrew/opt/e2fsprogs/sbin/$name" \
    "/usr/local/opt/e2fsprogs/sbin/$name" \
    "/usr/sbin/$name" \
    "/sbin/$name"
  do
    if [[ -x "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return
    fi
  done
  echo "$name not found" >&2
  exit 2
}

mke2fs="$(find_e2fs_tool mke2fs)"
debugfs="$(find_e2fs_tool debugfs)"
rootfs="$tmp_dir/rootfs.ext4"
old_relay="$tmp_dir/old-relay"
old_bridge="$tmp_dir/old-bridge"
current_relay="$tmp_dir/current-relay"
current_bridge="$tmp_dir/current-bridge"

node - "$old_relay" "$old_bridge" "$current_relay" "$current_bridge" <<'NODE'
const fs = require("node:fs");

function elf(marker) {
  const header = Buffer.alloc(64);
  header[0] = 0x7f;
  header[1] = 0x45;
  header[2] = 0x4c;
  header[3] = 0x46;
  header[4] = 2;
  header[5] = 1;
  header[6] = 1;
  header.writeUInt16LE(2, 16);
  header.writeUInt16LE(183, 18);
  return Buffer.concat([header, Buffer.from(`\n${marker}\n`)]);
}

const [oldRelay, oldBridge, currentRelay, currentBridge] = process.argv.slice(2);
fs.writeFileSync(oldRelay, elf("old-runtime-relay"), { mode: 0o755 });
fs.writeFileSync(oldBridge, elf("old-guest-control-bridge"), { mode: 0o755 });
fs.writeFileSync(currentRelay, elf("current-runtime-relay"), { mode: 0o755 });
fs.writeFileSync(currentBridge, elf("current-guest-control-bridge"), { mode: 0o755 });
NODE

truncate -s 16M "$rootfs"
"$mke2fs" -q -t ext4 -F "$rootfs"
for directory in /opt /opt/elastos /opt/elastos/bin; do
  "$debugfs" -w -R "mkdir $directory" "$rootfs" >/dev/null 2>&1
done
"$debugfs" -w -R "write $old_relay /opt/elastos/bin/browser-vm-runtime-relay" \
  "$rootfs" >/dev/null 2>&1
"$debugfs" -w -R "write $old_bridge /opt/elastos/bin/browser-vm-guest-control-bridge" \
  "$rootfs" >/dev/null 2>&1

functions_file="$tmp_dir/setup-source-home-functions.sh"
awk '
  /^echo "\[setup-source-home\] repo:/ { exit }
  { print }
' "$repo_root/scripts/setup-source-home.sh" >"$functions_file"
source "$functions_file"

resolve_root="$tmp_dir/resolve-root"
rust_target="aarch64-unknown-linux-musl"
mkdir -p \
  "$resolve_root/elastos/tools/browser-vm-runtime-relay" \
  "$resolve_root/elastos/tools/browser-vm-guest-control-bridge" \
  "$resolve_root/elastos/target/$rust_target/release" \
  "$resolve_root/relative-target/$rust_target/release" \
  "$resolve_root/elastos/tools/browser-vm-runtime-relay/target/$rust_target/release" \
  "$resolve_root/elastos/tools/browser-vm-guest-control-bridge/target/$rust_target/release"
printf '[package]\nname = "browser-vm-runtime-relay"\nversion = "0.0.0"\n' \
  >"$resolve_root/elastos/tools/browser-vm-runtime-relay/Cargo.toml"
printf '[package]\nname = "browser-vm-guest-control-bridge"\nversion = "0.0.0"\n' \
  >"$resolve_root/elastos/tools/browser-vm-guest-control-bridge/Cargo.toml"
cp "$current_relay" "$resolve_root/elastos/target/$rust_target/release/browser-vm-runtime-relay"
cp "$current_bridge" "$resolve_root/elastos/target/$rust_target/release/browser-vm-guest-control-bridge"
cp "$old_relay" \
  "$resolve_root/elastos/tools/browser-vm-runtime-relay/target/$rust_target/release/browser-vm-runtime-relay"
cp "$old_bridge" \
  "$resolve_root/elastos/tools/browser-vm-guest-control-bridge/target/$rust_target/release/browser-vm-guest-control-bridge"
cp "$current_relay" "$resolve_root/relative-target/$rust_target/release/browser-vm-runtime-relay"
cp "$current_bridge" "$resolve_root/relative-target/$rust_target/release/browser-vm-guest-control-bridge"

ROOT="$resolve_root"
unset ELASTOS_BROWSER_VM_RUNTIME_RELAY_BIN
unset ELASTOS_BROWSER_VM_GUEST_CONTROL_BRIDGE_BIN
unset CARGO_TARGET_DIR
default_runtime_relay="$(resolve_browser_vm_guest_helper_source \
  "browser-vm-runtime-relay" \
  "ELASTOS_BROWSER_VM_RUNTIME_RELAY_BIN" \
  "browser-vm-runtime-relay" \
  "browser-vm-runtime-relay" \
  "linux-arm64")"
default_guest_bridge="$(resolve_browser_vm_guest_helper_source \
  "browser-vm-guest-control-bridge" \
  "ELASTOS_BROWSER_VM_GUEST_CONTROL_BRIDGE_BIN" \
  "browser-vm-guest-control-bridge" \
  "browser-vm-guest-control-bridge" \
  "linux-arm64")"
[[ "$default_runtime_relay" == "$resolve_root/elastos/target/$rust_target/release/browser-vm-runtime-relay" ]]
[[ "$default_guest_bridge" == "$resolve_root/elastos/target/$rust_target/release/browser-vm-guest-control-bridge" ]]

abs_target="$tmp_dir/absolute-target"
mkdir -p "$abs_target/$rust_target/release"
cp "$current_relay" "$abs_target/$rust_target/release/browser-vm-runtime-relay"
cp "$current_bridge" "$abs_target/$rust_target/release/browser-vm-guest-control-bridge"
export CARGO_TARGET_DIR="$abs_target"
abs_runtime_relay="$(resolve_browser_vm_guest_helper_source \
  "browser-vm-runtime-relay" \
  "ELASTOS_BROWSER_VM_RUNTIME_RELAY_BIN" \
  "browser-vm-runtime-relay" \
  "browser-vm-runtime-relay" \
  "linux-arm64")"
abs_guest_bridge="$(resolve_browser_vm_guest_helper_source \
  "browser-vm-guest-control-bridge" \
  "ELASTOS_BROWSER_VM_GUEST_CONTROL_BRIDGE_BIN" \
  "browser-vm-guest-control-bridge" \
  "browser-vm-guest-control-bridge" \
  "linux-arm64")"
[[ "$abs_runtime_relay" == "$abs_target/$rust_target/release/browser-vm-runtime-relay" ]]
[[ "$abs_guest_bridge" == "$abs_target/$rust_target/release/browser-vm-guest-control-bridge" ]]

export CARGO_TARGET_DIR="relative-target"
relative_runtime_relay="$(resolve_browser_vm_guest_helper_source \
  "browser-vm-runtime-relay" \
  "ELASTOS_BROWSER_VM_RUNTIME_RELAY_BIN" \
  "browser-vm-runtime-relay" \
  "browser-vm-runtime-relay" \
  "linux-arm64")"
relative_guest_bridge="$(resolve_browser_vm_guest_helper_source \
  "browser-vm-guest-control-bridge" \
  "ELASTOS_BROWSER_VM_GUEST_CONTROL_BRIDGE_BIN" \
  "browser-vm-guest-control-bridge" \
  "browser-vm-guest-control-bridge" \
  "linux-arm64")"
[[ "$relative_runtime_relay" == "$resolve_root/relative-target/$rust_target/release/browser-vm-runtime-relay" ]]
[[ "$relative_guest_bridge" == "$resolve_root/relative-target/$rust_target/release/browser-vm-guest-control-bridge" ]]
[[ "$relative_runtime_relay" != "$resolve_root/elastos/tools/browser-vm-runtime-relay/target/$rust_target/release/browser-vm-runtime-relay" ]]
[[ "$relative_guest_bridge" != "$resolve_root/elastos/tools/browser-vm-guest-control-bridge/target/$rust_target/release/browser-vm-guest-control-bridge" ]]

ROOT="$repo_root"
DATA_DIR="$tmp_dir/data"
BROWSER_VM_ROOTFS_BACKUP=""
export ELASTOS_BROWSER_VM_RUNTIME_RELAY_BIN="$current_relay"
export ELASTOS_BROWSER_VM_GUEST_CONTROL_BRIDGE_BIN="$current_bridge"
unset CARGO_TARGET_DIR

refresh_browser_vm_native_helpers "$rootfs" "$debugfs" "linux-arm64"

actual_relay="$tmp_dir/actual-relay"
actual_bridge="$tmp_dir/actual-bridge"
"$debugfs" -R "cat /opt/elastos/bin/browser-vm-runtime-relay" \
  "$rootfs" >"$actual_relay" 2>/dev/null
"$debugfs" -R "cat /opt/elastos/bin/browser-vm-guest-control-bridge" \
  "$rootfs" >"$actual_bridge" 2>/dev/null
cmp "$current_relay" "$actual_relay"
cmp "$current_bridge" "$actual_bridge"

if [[ -z "$BROWSER_VM_ROOTFS_BACKUP" || ! -f "$BROWSER_VM_ROOTFS_BACKUP" ]]; then
  echo "native helper refresh did not preserve one rollback rootfs" >&2
  exit 1
fi
backup_relay="$tmp_dir/backup-relay"
backup_bridge="$tmp_dir/backup-bridge"
"$debugfs" -R "cat /opt/elastos/bin/browser-vm-runtime-relay" \
  "$BROWSER_VM_ROOTFS_BACKUP" >"$backup_relay" 2>/dev/null
"$debugfs" -R "cat /opt/elastos/bin/browser-vm-guest-control-bridge" \
  "$BROWSER_VM_ROOTFS_BACKUP" >"$backup_bridge" 2>/dev/null
cmp "$old_relay" "$backup_relay"
cmp "$old_bridge" "$backup_bridge"

printf '%s\n' '{"schema":"elastos.setup-source-home.browser-rootfs-refresh-smoke/v1","ok":true}'
