#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmpdir="$(mktemp -d /tmp/elastos-walletconnect-config-XXXXXX)"
trap 'rm -rf "$tmpdir"' EXIT

sdk_file="$tmpdir/reown-appkit-adapter.js"
data_dir="$tmpdir/data"

cat >"$sdk_file" <<'JS'
export async function connectWalletConnectEvm() {
  return {
    async request() {
      throw new Error("test adapter");
    },
  };
}
JS

node "$ROOT_DIR/scripts/configure-walletconnect-connector.mjs" \
  --data-dir "$data_dir" \
  --project-id test_walletconnect_project \
  --sdk-file "$sdk_file" \
  --sdk-version 0.0.0-test >/tmp/elastos-walletconnect-config-smoke.json

DATA_DIR="$data_dir" SDK_FILE="$sdk_file" node <<'JS'
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const dataDir = process.env.DATA_DIR;
const sdkFile = process.env.SDK_FILE;
const configPath = resolve(dataDir, "ElastOS/SystemServices/WalletConnect/config.json");
const copiedPath = resolve(dataDir, "capsules/wallet-walletconnect/vendor/reown-appkit.js");
const config = JSON.parse(readFileSync(configPath, "utf8"));
const expectedHash = createHash("sha256").update(readFileSync(sdkFile)).digest("hex");

if (config.schema !== "elastos.walletconnect.connector/v1") {
  throw new Error("wrong config schema");
}
if (config.project_id !== "test_walletconnect_project") {
  throw new Error("wrong project id");
}
if (config.sdk_package !== "@reown/appkit") {
  throw new Error("wrong sdk package");
}
if (config.sdk_version !== "0.0.0-test") {
  throw new Error("wrong sdk version");
}
if (config.sdk_sha256 !== expectedHash) {
  throw new Error("wrong sdk hash");
}
if (readFileSync(copiedPath, "utf8") !== readFileSync(sdkFile, "utf8")) {
  throw new Error("sdk adapter was not copied exactly");
}
JS

echo "[walletconnect-connector-config-smoke] OK"
