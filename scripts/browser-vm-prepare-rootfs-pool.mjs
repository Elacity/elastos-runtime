#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

function fail(message) {
  console.error(message);
  process.exit(1);
}

function validateAbsolute(label, value) {
  if (typeof value !== "string" || !value.startsWith("/") || /[\r\n\0]/.test(value)) {
    fail(`${label} must be an absolute path without control characters`);
  }
}

function defaultDataDir() {
  if (process.env.ELASTOS_BROWSER_VM_DATA_DIR) return process.env.ELASTOS_BROWSER_VM_DATA_DIR;
  if (process.env.XDG_DATA_HOME) return path.join(process.env.XDG_DATA_HOME, "elastos");
  if (process.env.HOME) return path.join(process.env.HOME, ".local/share/elastos");
  return "/var/lib/elastos";
}

function parseArgs(argv) {
  const args = {
    count: 1,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    const next = () => {
      i += 1;
      if (i >= argv.length || argv[i].startsWith("--")) fail(`${arg} requires a value`);
      return argv[i];
    };
    switch (arg) {
      case "--data-dir":
        args.dataDir = next();
        break;
      case "--rootfs":
        args.rootfs = next();
        break;
      case "--pool-dir":
        args.poolDir = next();
        break;
      case "--count":
        args.count = Number(next());
        break;
      case "--help":
      case "-h":
        console.log("Usage: browser-vm-prepare-rootfs-pool.mjs [--data-dir DIR] [--rootfs FILE] [--pool-dir DIR] [--count N]");
        process.exit(0);
      default:
        fail(`unknown option: ${arg}`);
    }
  }
  return args;
}

function readyRootfsFiles(poolDir) {
  try {
    return fs.readdirSync(poolDir).filter((name) => /^rootfs-[A-Za-z0-9._-]+\.ext4$/.test(name));
  } catch (error) {
    if (error?.code === "ENOENT") return [];
    throw error;
  }
}

function emit(value) {
  console.log(JSON.stringify(value));
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const dataDir = args.dataDir || defaultDataDir();
  validateAbsolute("--data-dir", dataDir);
  const rootfs = args.rootfs || path.join(dataDir, "browser-vm/rootfs.ext4");
  const poolDir = args.poolDir || path.join(dataDir, "browser-vm/rootfs-pool");
  validateAbsolute("--rootfs", rootfs);
  validateAbsolute("--pool-dir", poolDir);
  if (!Number.isInteger(args.count) || args.count < 1 || args.count > 8) {
    fail("--count must be an integer from 1 to 8");
  }
  if (!fs.existsSync(rootfs)) {
    fail(`Browser VM rootfs does not exist: ${rootfs}`);
  }
  fs.mkdirSync(poolDir, { recursive: true, mode: 0o700 });
  const existing = readyRootfsFiles(poolDir).length;
  const created = [];
  for (let index = existing; index < args.count; index += 1) {
    const id = `${Date.now().toString(36)}-${crypto.randomBytes(4).toString("hex")}`;
    const partial = path.join(poolDir, `rootfs-${id}.ext4.partial`);
    const ready = path.join(poolDir, `rootfs-${id}.ext4`);
    const startedAt = Date.now();
    emit({
      schema: "elastos.browser.vm-rootfs-pool.event/v1",
      event: "copy_start",
      rootfs,
      partial,
    });
    const result = spawnSync("cp", ["--reflink=auto", "--sparse=always", rootfs, partial], {
      stdio: "pipe",
      encoding: "utf8",
      timeout: Number(process.env.ELASTOS_BROWSER_VM_ROOTFS_COPY_TIMEOUT_MS || "900000"),
    });
    if (result.error || result.status !== 0) {
      try {
        fs.rmSync(partial, { force: true });
      } catch {}
      fail(result.error?.message || result.stderr || result.stdout || `cp exited ${result.status}`);
    }
    fs.chmodSync(partial, 0o600);
    fs.renameSync(partial, ready);
    created.push(ready);
    emit({
      schema: "elastos.browser.vm-rootfs-pool.event/v1",
      event: "copy_ready",
      ready,
      elapsed_ms: Date.now() - startedAt,
    });
  }
  emit({
    schema: "elastos.browser.vm-rootfs-pool/v1",
    ok: true,
    platform: `${process.platform}-${os.arch()}`,
    data_dir: dataDir,
    pool_dir: poolDir,
    requested_count: args.count,
    ready_count: readyRootfsFiles(poolDir).length,
    created,
    script: fileURLToPath(import.meta.url),
  });
}

main();
