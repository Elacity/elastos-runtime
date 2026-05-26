#!/usr/bin/env node

import { createHash } from "node:crypto";
import { copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
import { basename, dirname, resolve } from "node:path";

const CONFIG_SCHEMA = "elastos.walletconnect.connector/v1";
const SDK_PACKAGE = "@reown/appkit";
const CONFIG_PATH = "ElastOS/SystemServices/WalletConnect/config.json";
const SDK_PATH = "capsules/wallet-walletconnect/vendor/reown-appkit.js";

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
});

async function main() {
  const flags = parseFlags(process.argv.slice(2));
  if (flags.help) {
    printUsage();
    return;
  }

  const dataDir = requiredFlag(flags, "data-dir");
  const projectId = requiredFlag(flags, "project-id");
  const sdkFile = requiredFlag(flags, "sdk-file");
  const sdkVersion = requiredFlag(flags, "sdk-version");

  validateProjectId(projectId);
  validateSdkVersion(sdkVersion);

  const sdkBytes = await readFile(sdkFile);
  if (sdkBytes.length === 0) {
    throw new Error("SDK adapter file is empty");
  }
  const sdkSha256 = createHash("sha256").update(sdkBytes).digest("hex");

  const targetSdkPath = resolve(dataDir, SDK_PATH);
  const targetConfigPath = resolve(dataDir, CONFIG_PATH);
  await mkdir(dirname(targetSdkPath), { recursive: true });
  await mkdir(dirname(targetConfigPath), { recursive: true });
  await copyFile(sdkFile, targetSdkPath);

  const config = {
    schema: CONFIG_SCHEMA,
    project_id: projectId,
    sdk_package: SDK_PACKAGE,
    sdk_version: sdkVersion,
    sdk_sha256: sdkSha256,
  };
  await writeFile(targetConfigPath, `${JSON.stringify(config, null, 2)}\n`, "utf8");

  console.log(JSON.stringify({
    schema: CONFIG_SCHEMA,
    data_dir: resolve(dataDir),
    sdk_asset: SDK_PATH,
    sdk_source: basename(sdkFile),
    sdk_package: SDK_PACKAGE,
    sdk_version: sdkVersion,
    sdk_sha256: sdkSha256,
    config: CONFIG_PATH,
  }, null, 2));
}

function parseFlags(args) {
  const flags = {};
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === "--help" || arg === "-h") {
      flags.help = true;
      continue;
    }
    if (!arg.startsWith("--")) {
      throw new Error(`Unexpected argument: ${arg}`);
    }
    const key = arg.slice(2);
    const value = args[index + 1];
    if (!value || value.startsWith("--")) {
      throw new Error(`Missing value for --${key}`);
    }
    flags[key] = value;
    index += 1;
  }
  return flags;
}

function requiredFlag(flags, name) {
  const value = flags[name];
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`Missing --${name}`);
  }
  return value.trim();
}

function validateProjectId(value) {
  if (!/^[A-Za-z0-9_-]{8,128}$/.test(value)) {
    throw new Error("Invalid WalletConnect project id");
  }
}

function validateSdkVersion(value) {
  if (!value || /^[~^*]/.test(value) || /\s/.test(value)) {
    throw new Error("SDK version must be exact and pinned");
  }
}

function printUsage() {
  console.log(`Usage:
  node scripts/configure-walletconnect-connector.mjs \\
    --data-dir <runtime-data-dir> \\
    --project-id <reown-project-id> \\
    --sdk-file <local-reown-appkit-adapter.js> \\
    --sdk-version <exact-version>

The SDK file must be a local, reviewed adapter bundle exporting
connectWalletConnectEvm(options). This script copies it into the runtime data
dir and writes the matching sha256 into the WalletConnect connector config.`);
}
