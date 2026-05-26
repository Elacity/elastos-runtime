#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import process from "node:process";

function usage() {
  console.error(`Usage:
  node scripts/browser-experiment-cleanup.mjs [--apply]

Dry-run by default. With --apply, removes only safe browser experiment leftovers:
  - orphaned Xvfb 1x1 proof displays whose parent is PID 1,
  - exited Docker containers named elastos-selkies-runtime-exit-target-*.

It never stops running Docker containers and never touches the active 1920x1080
Selkies baseline display.
`);
}

function parseArgs(argv) {
  const args = { apply: false };
  for (const arg of argv) {
    if (arg === "--help" || arg === "-h") {
      usage();
      process.exit(0);
    }
    if (arg === "--apply") {
      args.apply = true;
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }
  return args;
}

function run(command, args, options = {}) {
  return spawnSync(command, args, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    ...options,
  });
}

function listStaleXvfb() {
  const result = run("ps", ["-eo", "pid,ppid,etimes,stat,args"]);
  if (result.status !== 0) {
    throw new Error(result.stderr || "ps failed");
  }
  const stale = [];
  for (const line of result.stdout.split(/\r?\n/).slice(1)) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const match = /^(\d+)\s+(\d+)\s+(\d+)\s+(\S+)\s+(.+)$/.exec(trimmed);
    if (!match) continue;
    const [, pid, ppid, etimes, stat, args] = match;
    if (
      ppid === "1" &&
      args.includes("/usr/bin/Xvfb") &&
      args.includes("1x1x24") &&
      args.includes("-nolisten tcp")
    ) {
      stale.push({
        pid: Number(pid),
        ppid: Number(ppid),
        etimes: Number(etimes),
        stat,
        args,
      });
    }
  }
  return stale;
}

function listSelkiesContainers() {
  const result = run("docker", [
    "ps",
    "-a",
    "--format",
    "{{.Names}}\t{{.Status}}",
    "--filter",
    "name=elastos-selkies-runtime-exit-target-",
  ]);
  if (result.status !== 0) {
    return {
      docker_available: false,
      error: result.stderr.trim() || "docker ps failed",
      exited: [],
      running: [],
    };
  }
  const exited = [];
  const running = [];
  for (const line of result.stdout.split(/\r?\n/)) {
    if (!line.trim()) continue;
    const [name, status = ""] = line.split("\t");
    const entry = { name, status };
    if (/^(Exited|Created|Dead)/.test(status)) {
      exited.push(entry);
    } else {
      running.push(entry);
    }
  }
  return {
    docker_available: true,
    exited,
    running,
  };
}

function killPids(pids) {
  if (pids.length === 0) return [];
  const result = run("kill", pids.map(String));
  return pids.map((pid) => ({
    pid,
    ok: result.status === 0,
    error: result.status === 0 ? null : result.stderr.trim(),
  }));
}

function removeExitedContainers(containers) {
  const removed = [];
  for (const container of containers) {
    const result = run("docker", ["rm", container.name]);
    removed.push({
      name: container.name,
      ok: result.status === 0,
      error: result.status === 0 ? null : result.stderr.trim(),
    });
  }
  return removed;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const staleXvfb = listStaleXvfb();
  const selkiesContainers = listSelkiesContainers();
  const killed_xvfb = args.apply ? killPids(staleXvfb.map((entry) => entry.pid)) : [];
  const removed_containers =
    args.apply && selkiesContainers.docker_available
      ? removeExitedContainers(selkiesContainers.exited)
      : [];

  console.log(JSON.stringify({
    schema: "elastos.browser.experiment-cleanup/v1",
    dry_run: !args.apply,
    stale_xvfb: staleXvfb,
    killed_xvfb,
    selkies_containers: selkiesContainers,
    removed_containers,
    running_containers_preserved: selkiesContainers.running || [],
  }, null, 2));
}

try {
  main();
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  usage();
  process.exit(1);
}
