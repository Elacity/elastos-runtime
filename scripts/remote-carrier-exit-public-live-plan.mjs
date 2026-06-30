#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { analyzeRemoteCarrierExitArtifacts } from "./remote-carrier-exit-artifact-readiness.mjs";

const DEFAULT_LIVE_ROOT = "/home/wau/.local/share/elastos-public-gateway-live";
const PUBLIC_LIVE_TARGET = {
  os: "linux",
  arch: "x86_64",
  executable_format: "elf",
};

function usage() {
  console.error(`Usage:
  node scripts/remote-carrier-exit-public-live-plan.mjs \\
    [--candidate-gateway-bin elastos/target/release/elastos] \\
    [--candidate-exit-provider-bin capsules/exit-provider/target/release/exit-provider] \\
    [--installed-artifact-readiness /tmp/installed-readiness.json] \\
    [--commit <sha>] \\
    [--ssh-host elastos-server] \\
    [--live-root /home/wau/.local/share/elastos-public-gateway-live] \\
    [--data-root /home/wau/.local/share/elastos-public-gateway-live/xdg-data/elastos]

Creates a dry-run public-live update plan for the remote Carrier Browser Exit
artifacts. It never deploys, pushes, restarts, or edits live data. It fails
closed unless the candidate gateway and exit-provider contain the Carrier
Browser stream and remote Exit contracts needed for operator evidence.
`);
}

function scriptDir() {
  return path.dirname(fileURLToPath(import.meta.url));
}

function repoRoot() {
  return path.resolve(scriptDir(), "..");
}

function parseArgs(argv) {
  const root = repoRoot();
  const args = {
    candidateGatewayBin: path.join(root, "elastos/target/release/elastos"),
    candidateExitProviderBin: path.join(root, "capsules/exit-provider/target/release/exit-provider"),
    installedArtifactReadiness: "",
    commit: "",
    sshHost: "elastos-server",
    liveRoot: DEFAULT_LIVE_ROOT,
    dataRoot: "",
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const next = () => {
      index += 1;
      if (index >= argv.length || argv[index].startsWith("--")) {
        throw new Error(`${arg} requires a value`);
      }
      return argv[index];
    };
    if (arg === "--help" || arg === "-h") {
      usage();
      process.exit(0);
    } else if (arg === "--candidate-gateway-bin") {
      args.candidateGatewayBin = next();
    } else if (arg === "--candidate-exit-provider-bin") {
      args.candidateExitProviderBin = next();
    } else if (arg === "--installed-artifact-readiness") {
      args.installedArtifactReadiness = next();
    } else if (arg === "--commit") {
      args.commit = next();
    } else if (arg === "--ssh-host") {
      args.sshHost = next();
    } else if (arg === "--live-root") {
      args.liveRoot = next();
    } else if (arg === "--data-root") {
      args.dataRoot = next();
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }
  if (!args.dataRoot) {
    args.dataRoot = path.join(args.liveRoot, "xdg-data/elastos");
  }
  return args;
}

function sha256File(file) {
  return crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function shellQuote(value) {
  return `'${String(value).replaceAll("'", "'\"'\"'")}'`;
}

function remoteCommand(sshHost, command) {
  return `ssh ${shellQuote(sshHost)} ${shellQuote(command)}`;
}

function safeReadJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function currentCommit(root) {
  try {
    return execFileSync("git", ["-C", root, "rev-parse", "HEAD"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    return "";
  }
}

function readinessSummary(file) {
  if (!file) {
    return null;
  }
  const parsed = safeReadJson(file);
  if (parsed.schema !== "elastos.remote-carrier-exit.artifact-readiness/v1") {
    throw new Error("--installed-artifact-readiness must point to an elastos.remote-carrier-exit.artifact-readiness/v1 report");
  }
  return {
    path: file,
    sha256: sha256File(file),
    schema: parsed.schema || null,
    ok: parsed.ok === true,
    generated_at: parsed.generated_at || null,
    failures: Array.isArray(parsed.failures) ? parsed.failures : [],
    gateway_sha256: parsed.artifacts?.gateway?.sha256 || null,
    exit_provider_sha256: parsed.artifacts?.exit_provider?.sha256 || null,
  };
}

function elfMachineName(value) {
  if (value === 0x3e) return "x86_64";
  if (value === 0xb7) return "aarch64";
  return `unknown_${value}`;
}

function executableSummary(file) {
  const bytes = fs.readFileSync(file);
  const magic = bytes.subarray(0, 4).toString("hex");
  const summary = {
    path: file,
    size_bytes: bytes.length,
    magic,
    format: "unknown",
    arch: "unknown",
    ok_for_public_live: false,
  };
  if (magic === "7f454c46") {
    const elfClass = bytes[4] === 2 ? "elf64" : bytes[4] === 1 ? "elf32" : "unknown";
    const endian = bytes[5] === 1 ? "little" : bytes[5] === 2 ? "big" : "unknown";
    const machine = bytes.length >= 20 && endian === "little"
      ? bytes.readUInt16LE(18)
      : bytes.length >= 20 && endian === "big"
        ? bytes.readUInt16BE(18)
        : -1;
    summary.format = "elf";
    summary.elf_class = elfClass;
    summary.endian = endian;
    summary.machine = machine;
    summary.arch = elfMachineName(machine);
  } else if (["cffaedfe", "feedfacf", "cafebabe", "bebafeca"].includes(magic)) {
    summary.format = "mach-o";
  }
  summary.ok_for_public_live =
    summary.format === PUBLIC_LIVE_TARGET.executable_format &&
    summary.arch === PUBLIC_LIVE_TARGET.arch;
  return summary;
}

function publicLiveExecutableReadiness({ gatewayBin, exitProviderBin }) {
  const gateway = executableSummary(gatewayBin);
  const exitProvider = executableSummary(exitProviderBin);
  const failures = [];
  if (!gateway.ok_for_public_live) {
    failures.push("gateway_not_linux_x86_64_elf");
  }
  if (!exitProvider.ok_for_public_live) {
    failures.push("exit_provider_not_linux_x86_64_elf");
  }
  return {
    target: PUBLIC_LIVE_TARGET,
    ok: failures.length === 0,
    artifacts: {
      gateway,
      exit_provider: exitProvider,
    },
    failures,
  };
}

function commandPlan({
  candidateGatewayBin,
  candidateExitProviderBin,
  commit,
  dataRoot,
  installedGatewayBin,
  installedExitProviderBin,
  backupDir,
  sshHost,
}) {
  const components = path.join(dataRoot, "components.json");
  const configDir = path.join(dataRoot, "config");
  const capsulesDir = path.join(dataRoot, "capsules");
  const homeBrowserIndex = path.join(capsulesDir, "home/browser/index.html");
  const candidateDir = path.join(backupDir, "candidates");
  const stagedGatewayBin = path.join(candidateDir, "elastos");
  const stagedExitProviderBin = path.join(candidateDir, "exit-provider");
  return {
    local_preflight: [
      "git status --short --branch",
      `git log --oneline -1 ${shellQuote(commit)}`,
      `node scripts/remote-carrier-exit-artifact-readiness.mjs --gateway-bin ${shellQuote(candidateGatewayBin)} --exit-provider-bin ${shellQuote(candidateExitProviderBin)}`,
      "scripts/carrier-only-authority-check.sh",
      "git diff --check",
      "node scripts/home-entropy-check.mjs",
      "node scripts/browser-entropy-check.mjs",
      "(cd elastos && cargo fmt --all -- --check)",
    ],
    public_live_backup: [
      `backup_dir=${shellQuote(backupDir)}`,
      'mkdir -p "$backup_dir"',
      `cp -p ${shellQuote(installedGatewayBin)} "$backup_dir/elastos"`,
      `cp -p ${shellQuote(installedExitProviderBin)} "$backup_dir/exit-provider"`,
      `cp -p ${shellQuote(components)} "$backup_dir/components.json"`,
      `if [ -d ${shellQuote(configDir)} ]; then tar -C ${shellQuote(dataRoot)} -czf "$backup_dir/config.tgz" config; fi`,
      `if [ -d ${shellQuote(capsulesDir)} ]; then tar -C ${shellQuote(dataRoot)} -czf "$backup_dir/capsules.tgz" capsules; fi`,
    ],
    operator_workstation_stage_candidates: [
      remoteCommand(sshHost, `mkdir -p ${shellQuote(candidateDir)}`),
      `scp ${shellQuote(candidateGatewayBin)} ${shellQuote(`${sshHost}:${stagedGatewayBin}`)}`,
      `scp ${shellQuote(candidateExitProviderBin)} ${shellQuote(`${sshHost}:${stagedExitProviderBin}`)}`,
      remoteCommand(sshHost, `chmod 0755 ${shellQuote(stagedGatewayBin)} ${shellQuote(stagedExitProviderBin)}`),
      remoteCommand(sshHost, `sha256sum ${shellQuote(stagedGatewayBin)} ${shellQuote(stagedExitProviderBin)}`),
    ],
    public_live_install_after_explicit_approval: [
      `install -m 0755 ${shellQuote(stagedGatewayBin)} ${shellQuote(installedGatewayBin)}`,
      `install -m 0755 ${shellQuote(stagedExitProviderBin)} ${shellQuote(installedExitProviderBin)}`,
      "Restart the existing public-live gateway/supervisor with its current operator command.",
    ],
    post_restart_verify: [
      `node scripts/remote-carrier-exit-artifact-readiness.mjs --gateway-bin ${shellQuote(installedGatewayBin)} --exit-provider-bin ${shellQuote(installedExitProviderBin)}`,
      "curl -fsS -o /dev/null -w '%{http_code}\\n' http://127.0.0.1:8090/apps/home/",
      "curl -fsS -o /dev/null -w '%{http_code}\\n' https://elastos.elacitylabs.com/apps/home/",
      "curl -fsS https://elastos.elacitylabs.com/apps/home/ | sha256sum",
      `sha256sum ${shellQuote(homeBrowserIndex)} capsules/home/browser/index.html`,
      `ELASTOS_DATA_DIR=${shellQuote(dataRoot)} scripts/installed-provider-verify.sh exit-provider`,
    ],
    route_acceptance_template: [
      "node scripts/remote-carrier-exit-readiness.mjs --source-config <source-exit-provider.json> --exit-config <remote-exit-provider.json> --principal <principal> --grant-id <grant-id> --target <tcp-or-tls-target> --exit-did <exit-runtime-did>",
      "node scripts/remote-carrier-exit-operator-report.mjs --input <operator-filled-evidence.json>",
    ],
    rollback: [
      `install -m 0755 ${shellQuote(path.join(backupDir, "elastos"))} ${shellQuote(installedGatewayBin)}`,
      `install -m 0755 ${shellQuote(path.join(backupDir, "exit-provider"))} ${shellQuote(installedExitProviderBin)}`,
      `cp -p ${shellQuote(path.join(backupDir, "components.json"))} ${shellQuote(components)}`,
      "Restart the existing public-live gateway/supervisor with its current operator command, then rerun post_restart_verify.",
    ],
  };
}

function commandContexts() {
  return {
    local_preflight: "operator_workstation_before_approval",
    operator_workstation_stage_candidates: "operator_workstation_after_explicit_approval",
    public_live_backup: "public_server_after_explicit_approval",
    public_live_install_after_explicit_approval: "public_server_after_explicit_approval",
    post_restart_verify: "public_server_after_explicit_approval",
    route_acceptance_template: "operator_workstation_after_successful_live_verify",
    rollback: "public_server_if_post_restart_verify_fails",
  };
}

function assertNoPrivateMaterial(report) {
  const text = JSON.stringify(report);
  return !/connect_ticket|relay_ipc|adapter_ipc|runtime_stream_path|home_token|authorization|cookie|set-cookie/i
    .test(text);
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const root = repoRoot();
  const generatedAt = new Date().toISOString();
  const commit = args.commit || currentCommit(root) || "unknown";
  const shortCommit = commit.replace(/[^a-f0-9]/gi, "").slice(0, 12) || "unknown";
  const stamp = generatedAt.replace(/[-:]/g, "").replace(/\..+$/, "Z");
  const liveRoot = path.resolve(args.liveRoot);
  const dataRoot = path.resolve(args.dataRoot);
  const candidateGatewayBin = path.resolve(args.candidateGatewayBin);
  const candidateExitProviderBin = path.resolve(args.candidateExitProviderBin);
  const installedGatewayBin = path.join(liveRoot, "elastos");
  const installedExitProviderBin = path.join(dataRoot, "bin/exit-provider");
  const backupDir = path.join(liveRoot, "backups", `remote-carrier-exit-${shortCommit}-${stamp}`);
  const candidateDir = path.join(backupDir, "candidates");
  const stagedGatewayBin = path.join(candidateDir, "elastos");
  const stagedExitProviderBin = path.join(candidateDir, "exit-provider");
  const candidateReadiness = analyzeRemoteCarrierExitArtifacts({
    gatewayBin: candidateGatewayBin,
    exitProviderBin: candidateExitProviderBin,
    generatedAt,
  });
  const candidatePublicLiveExecutables = publicLiveExecutableReadiness({
    gatewayBin: candidateGatewayBin,
    exitProviderBin: candidateExitProviderBin,
  });
  const installedReadiness = readinessSummary(args.installedArtifactReadiness);
  const report = {
    schema: "elastos.remote-carrier-exit.public-live-update-plan/v1",
    ok: candidateReadiness.ok === true && candidatePublicLiveExecutables.ok === true,
    dry_run_only: true,
    mutation_allowed: false,
    generated_at: generatedAt,
    commit,
    ssh_host: args.sshHost,
    live_root: liveRoot,
    data_root: dataRoot,
    approval_required: {
      before_push_or_deploy: true,
      before_live_mutation: true,
      reason: "This artifact is only a reviewable plan; AGENTS.md requires explicit user approval before public-live mutation.",
    },
    candidate_readiness: candidateReadiness,
    candidate_public_live_executables: candidatePublicLiveExecutables,
    installed_artifact_readiness: installedReadiness,
    stale_installed_artifacts_observed: installedReadiness ? installedReadiness.ok !== true : null,
    install_targets: {
      gateway: installedGatewayBin,
      exit_provider: installedExitProviderBin,
      backup_dir: backupDir,
      candidate_staging_dir: candidateDir,
      staged_gateway: stagedGatewayBin,
      staged_exit_provider: stagedExitProviderBin,
    },
    command_contexts: commandContexts(),
    commands: commandPlan({
      candidateGatewayBin,
      candidateExitProviderBin,
      commit,
      dataRoot,
      installedGatewayBin,
      installedExitProviderBin,
      backupDir,
      sshHost: args.sshHost,
    }),
    next_steps: candidateReadiness.ok === true && candidatePublicLiveExecutables.ok === true ? [
      "Show this plan, local divergence, and verification results to the user before any push or public-live mutation.",
      "After explicit approval, perform the backup/install/restart steps on the public server and rerun installed artifact readiness.",
      "Only then collect route readiness, Browser machine proof, and operator evidence for the remote Carrier Browser Exit path.",
    ] : [
      "Rebuild the candidate gateway and exit-provider as Linux x86_64 ELF binaries from the reviewed commit before public-live install.",
      "Rerun this plan after candidate artifact readiness is green.",
    ],
  };
  report.private_material_redacted = assertNoPrivateMaterial(report);
  if (!report.private_material_redacted) {
    report.ok = false;
  }
  console.log(JSON.stringify(report, null, 2));
  if (!report.ok) {
    process.exit(1);
  }
}

try {
  main();
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  usage();
  process.exit(2);
}
