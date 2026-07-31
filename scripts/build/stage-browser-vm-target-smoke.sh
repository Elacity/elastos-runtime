#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

mkdir -p "$tmp_dir/bin"
node - "$tmp_dir/bin" <<'NODE'
const fs = require("node:fs");
const path = require("node:path");

const elf = Buffer.alloc(64);
elf[0] = 0x7f;
elf[1] = 0x45;
elf[2] = 0x4c;
elf[3] = 0x46;
elf[4] = 2;
elf[5] = 1;
elf[6] = 1;
elf.writeUInt16LE(2, 16);
elf.writeUInt16LE(183, 18);

for (const executable of [
  "browser-native-proxy-engine",
  "browser-vm-runtime-relay",
  "browser-vm-guest-control-bridge",
  "node",
  "chromium",
]) {
  const target = path.join(process.argv[2], executable);
  const contents = executable === "browser-vm-guest-control-bridge"
    ? Buffer.concat([
        elf,
        Buffer.from("\nelastos.browser.vm-guest-control-bridge.config/v1\ncontrol_socket_ready_timeout_ms\ncontrol_request_timeout_ms\n"),
      ])
    : elf;
  fs.writeFileSync(target, contents, { mode: 0o755 });
}
NODE
printf '#!/usr/bin/env node\n' > "$tmp_dir/bin/browser-selkies-control-service.mjs"

output="$("$repo_root/scripts/build/stage-browser-vm-target.sh" \
  --out-dir "$tmp_dir/stage" \
  --target-platform linux-arm64 \
  --native-proxy-bin "$tmp_dir/bin/browser-native-proxy-engine" \
  --runtime-relay-bin "$tmp_dir/bin/browser-vm-runtime-relay" \
  --guest-control-bridge-bin "$tmp_dir/bin/browser-vm-guest-control-bridge" \
  --control-service "$tmp_dir/bin/browser-selkies-control-service.mjs" \
  --node-bin "$tmp_dir/bin/node" \
  --chromium-bin "$tmp_dir/bin/chromium")"

OUTPUT="$output" node - <<'NODE'
const result = JSON.parse(process.env.OUTPUT);
if (result.schema !== "elastos.browser.vm-target-stage/v1") throw new Error("wrong schema");
if (result.ok !== true) throw new Error(`stage failed: ${process.env.OUTPUT}`);
if (result.preflight?.ok !== true) throw new Error("preflight failed");
if (result.preflight?.manifest?.runtime_exit_transport !== "vsock_relay") throw new Error("wrong runtime exit transport");
if (result.preflight?.manifest?.control_transport !== "vsock_relay") throw new Error("wrong control transport");
if (result.preflight?.manifest?.media_transport !== "runtime_relay") throw new Error("missing runtime relay");
const fs = require("node:fs");
const start = fs.readFileSync(`${result.rootfs}/opt/elastos/bin/browser-vm-selkies-start`, "utf8");
const init = fs.readFileSync(`${result.rootfs}/opt/elastos/bin/browser-vm-init`, "utf8");
const bootstrap = `${result.rootfs}/opt/elastos/bin/browser-vm-vz-transport-bootstrap.mjs`;
if (!fs.statSync(bootstrap).isFile()) throw new Error("missing VZ transport bootstrap");
for (const expected of [
  "elastos.browser_vz_transport",
  "browser-vm-bootstrap-relay.json",
  "guest_loopback_tcp",
  "browser-vm-media-relay.json",
]) {
  if (!init.includes(expected)) throw new Error(`VZ init is missing ${expected}`);
}
for (const forbidden of ["nft", "iptables", "pfctl", "guest_network_policy"]) {
  if (init.includes(forbidden)) {
    throw new Error(`VZ init introduced forbidden policy surface ${forbidden}`);
  }
}
if (!start.includes("elastos.browser_profile_disk")) throw new Error("missing profile disk boot contract");
if (!start.includes("/dev/vdb")) throw new Error("missing Browser profile data disk mount");
if (!start.includes("--user-data-dir=${ELASTOS_BROWSER_VM_PROFILE_DIR}")) throw new Error("Chromium profile dir is not Runtime-owned");
if (!start.includes(': "${ELASTOS_BROWSER_VM_SELKIES_ENCODER:=openh264enc}"')) throw new Error("VM Selkies encoder default must be explicit");
if (!start.includes('--encoder="$ELASTOS_BROWSER_VM_SELKIES_ENCODER"')) throw new Error("VM Selkies encoder must be runtime-selected");
if (start.includes("--encoder=x264enc")) throw new Error("VM Selkies encoder must not be hardcoded to x264enc");
if (start.includes("x264enc-striped|jpeg")) throw new Error("VM Selkies encoder validation must match Selkies GStreamer");
NODE

printf '%s\n' '{"schema":"elastos.browser.vm-target-stage-smoke/v1","ok":true}'
