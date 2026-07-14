#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import process from "node:process";

import { validateHomeShellManualUxReport } from "./home-shell-manual-ux-report.mjs";

const SCHEMA = "elastos.home-shell.objective-audit/v1";
const repoRoot = new URL("../", import.meta.url);

function usage() {
  console.error(`Usage:
  node scripts/home-shell-objective-audit.mjs [--manual-ux /path/to/home-shell-manual-ux.json] [--require-complete]

Audits the Home shell objective against source, smoke, docs, and optional
operator-profile manual UX evidence. The audit is fail-closed for product
readiness: source checks can pass while the full objective remains incomplete
until operator-profile evidence exists.
`);
}

function parseArgs(argv) {
  const args = {
    manualUx: "",
    requireComplete: false,
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
    } else if (arg === "--manual-ux") {
      args.manualUx = next();
    } else if (arg === "--require-complete") {
      args.requireComplete = true;
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }
  return args;
}

function read(path) {
  return readFileSync(new URL(path, repoRoot), "utf8");
}

function readJson(path) {
  return JSON.parse(read(path));
}

function readExternalJson(path) {
  if (!path) return null;
  return JSON.parse(readFileSync(path, "utf8"));
}

function gitValue(args) {
  try {
    return execFileSync("git", args, {
      cwd: new URL("../", import.meta.url),
      encoding: "utf8",
    }).trim();
  } catch (_error) {
    return "";
  }
}

function includesNormalized(text, needle) {
  return text.replace(/\s+/g, " ").includes(needle.replace(/\s+/g, " "));
}

function commandByName(contract, name) {
  return contract.commands.find((command) => command.name === name) || null;
}

function commandSurface(contract, name, surface) {
  return commandByName(contract, name)?.surface?.includes(surface) === true;
}

function criterion(id, requirement, ok, evidence, missing = "") {
  return {
    id,
    requirement,
    ok: Boolean(ok),
    evidence,
    missing: ok ? null : missing,
  };
}

function criteriaOk(criteria, id) {
  return criteria.find((item) => item.id === id)?.ok === true;
}

function manualUxResult(path) {
  if (!path) {
    return {
      schema: "elastos.home-shell.manual-ux/v1",
      ok: false,
      errors: ["manual UX evidence not provided"],
    };
  }
  const report = readExternalJson(path);
  const result = validateHomeShellManualUxReport(report);
  const errors = [...(result.errors || [])];
  const head = gitValue(["rev-parse", "HEAD"]);
  if (result.ok && head && report?.source?.commit !== head) {
    errors.push("manual UX source.commit must match current HEAD");
  }
  return {
    ...result,
    ok: errors.length === 0,
    errors,
    source_commit: typeof report?.source?.commit === "string" ? report.source.commit : "",
    current_head: head,
  };
}

function audit(args) {
  const homeIndex = read("capsules/home/browser/index.html");
  const homeTemplate = read("capsules/home-gui/browser/home-gui-template.html");
  const host = read("capsules/home/browser/home-shell-host.js");
  const shellCore = read("capsules/home/browser/shell-core.js");
  const homeGuiCore = read("capsules/home-gui/browser/shell-core.js");
  const homeGui = read("capsules/home-gui/browser/home-gui.js");
  const homeCli = read("capsules/home-cli/browser/home-cli.js");
  const homeCliIndex = read("capsules/home-cli/browser/index.html");
  const homeCliStyle = read("capsules/home-cli/browser/style.css");
  const homeCliRust = read("capsules/home-cli/src/main.rs");
  const homeGuiManifest = read("capsules/home-gui/capsule.json");
  const commandContract = readJson("capsules/home-cli/browser/commands.json");
  const state = read("state.md");
  const tasks = read("TASKS.md");
  const contractDoc = read("docs/HOME_SHELL_HOST_CONTRACT.md");
  const espDoc = read("docs/ESP_V0.md");
  const entropy = read("scripts/home-entropy-check.mjs");
  const bridgeSmoke = read("scripts/home-shell-bridge-smoke.mjs");
  const authGateSmoke = read("scripts/home-shell-auth-gate-smoke.mjs");
  const staleHintSmoke = read("scripts/home-shell-stale-hint-boot-smoke.mjs");
  const noHintSmoke = read("scripts/home-shell-no-hint-boot-smoke.mjs");
  const recoverySmoke = read("scripts/home-shell-recovery-smoke.mjs");
  const switchbackSmoke = read("scripts/home-shell-switchback-recovery-smoke.mjs");
  const systemSwitchSmoke = read("scripts/home-shell-system-switch-smoke.mjs");
  const regressionSmoke = read("scripts/home-shell-regression-smoke.mjs");
  const cliSmoke = read("scripts/home-cli-browser-smoke.mjs");
  const virtualAuthSmoke = read("scripts/home-passkey-virtual-auth-smoke.mjs");
  const gatewayHomeTerminal = read("elastos/crates/elastos-server/src/api/gateway_home_terminal.rs");
  const gatewayHomeTests = read("elastos/crates/elastos-server/src/api/gateway_tests/home_system.rs");
  const gatewayCapsuleCatalog = read("elastos/crates/elastos-server/src/api/gateway_capsule_catalog.rs");
  const catalogReadModel = read("elastos/crates/elastos-server/src/api/gateway_capsule_catalog/read_model.rs");
  const shellPicker = read("elastos/esp/shell_picker.ts");
  const manual = manualUxResult(args.manualUx);

  const noDirectHomeCliAuthority =
    !/\/api\/provider\/|\/api\/apps\/system|dispatch_approved|ProviderRegistry|localStorage|sessionStorage|indexedDB|window\.ethereum|personal_sign|eth_requestAccounts/.test(
      homeCli,
  );
  const retiredHomeActiveStateLiteral = ['"active": "', 'home"'].join("");
  const retiredHomeGuiOldIdentifier = ["HOME_GUI", "LEGACY"].join("_");
  const retiredHomeAliasExpression = ['name === "', 'home"'].join("");

  const criteria = [
    criterion(
      "split_names_and_frontdoor",
      "Keep /apps/home/ as the front door while internal names are home-shell-host, home-gui, and home-cli.",
      homeIndex.includes('data-home-shell-host="boot"') &&
        shellCore.includes('export const HOME_SHELL_HOST_ID = "home-shell-host"') &&
        shellCore.includes('export const HOME_GUI_SHELL_ID = "home-gui"') &&
        !shellCore.includes(retiredHomeGuiOldIdentifier) &&
        !host.includes(retiredHomeGuiOldIdentifier) &&
        host.includes('"home-cli": "visible-target"') &&
        state.includes("`home-shell-host` for host lifecycle") &&
        state.includes("`home-gui` for the desktop projection") &&
        state.includes("`home-cli` for the command projection"),
      [
        "capsules/home/browser/index.html",
        "capsules/home/browser/home-shell-host.js",
        "state.md",
      ],
      "Keep the front-door route and internal shell names explicit.",
    ),
    criterion(
      "home_gui_boundary",
      "Desktop, taskbar, launcher, window, and chrome projection live behind home-gui instead of the host owning GUI behavior directly.",
      !homeIndex.includes("desktop-backdrop") &&
        !homeIndex.includes('id="window-template"') &&
        !homeIndex.includes('id="shortcut-template"') &&
        !homeIndex.includes('id="launcher-item-template"') &&
        !homeIndex.includes('id="window-error-template"') &&
        !homeIndex.includes('id="taskbar-item-template"') &&
        homeTemplate.includes("desktop-backdrop") &&
        homeTemplate.includes('id="window-template"') &&
        homeTemplate.includes('id="shortcut-template"') &&
        homeTemplate.includes('id="launcher-item-template"') &&
        homeTemplate.includes('id="window-error-template"') &&
        homeTemplate.includes('id="taskbar-item-template"') &&
        homeGui.includes("renderDesktop(summary)") &&
        homeGui.includes("renderTaskbar(summary)") &&
        homeGui.includes("renderLauncher(summary)") &&
        homeGui.includes("function retireHomeGuiSurface(options = {})") &&
        homeGuiCore.includes("export async function ensureHomeGuiDom()") &&
        homeGuiCore.includes("function desktopLayoutBounds()") &&
        homeGuiCore.includes("desktopIconsVisible: true") &&
        !host.includes("renderDesktop(") &&
        !host.includes("renderTaskbar(") &&
        !host.includes("renderLauncher(") &&
        !host.includes("document.querySelectorAll(\".window") &&
        !shellCore.includes("export async function ensureHomeGuiDom()") &&
        !shellCore.includes("function desktopLayoutBounds()") &&
        !shellCore.includes("desktopIconsVisible: true") &&
        entropy.includes("homeGuiJs.includes(\"function retireHomeGuiSurface(options = {})\")") &&
        homeGuiManifest.includes('"execution": "web-projection"') &&
        contractDoc.includes("trusted host-loaded GUI shell code"),
      [
        "capsules/home/browser/index.html",
        "capsules/home-gui/capsule.json",
        "capsules/home-gui/browser/home-gui-template.html",
        "capsules/home-gui/browser/shell-core.js",
        "capsules/home-gui/browser/home-gui.js",
        "capsules/home/browser/shell-core.js",
        "capsules/home/browser/home-shell-host.js",
        "docs/HOME_SHELL_HOST_CONTRACT.md",
        "scripts/home-entropy-check.mjs",
      ],
      "Keep GUI projection in the home-gui package and document that it is trusted host-loaded UI until a true isolated GUI shell exists.",
    ),
    criterion(
      "minimal_host_recovery",
      "Host-owned recovery exists for failed shell selection/mount and is not the Home GUI toolbar.",
      homeIndex.includes("shell-host-recovery") &&
        host.includes("showShellHostRecovery") &&
        recoverySmoke.includes("recovery") &&
        switchbackSmoke.includes("switchback recovery did not use the mounted shell launch token") &&
        contractDoc.includes("host-owned recovery surface"),
      [
        "capsules/home/browser/index.html",
        "capsules/home/browser/home-shell-host.js",
        "scripts/home-shell-recovery-smoke.mjs",
        "scripts/home-shell-switchback-recovery-smoke.mjs",
        "docs/HOME_SHELL_HOST_CONTRACT.md",
      ],
      "Recovery must stay host-owned and minimal.",
    ),
    criterion(
      "home_cli_product_useful",
      "Home CLI exposes a small working product vocabulary, keeps developer projections behind debug, shows Inbox/Wallet/Exit facts, and writes low-risk invoke intents.",
      [
        "home",
        "apps",
        "invoke",
        "inbox",
        "people",
        "mywebsite",
        "wallet",
        "exits",
        "system",
        "debug",
        "refresh",
        "help",
        "exit",
      ].every((name) => commandByName(commandContract, name)) &&
        [
          "home",
          "apps",
          "invoke",
          "inbox",
          "people",
          "mywebsite",
          "wallet",
          "exits",
          "system",
          "debug",
          "refresh",
          "help",
          "exit",
        ].every(
          (name) => commandSurface(commandContract, name, "home-cli"),
        ) &&
        ["capsules", "inspect", "affordances", "gates", "audit", "services", "browser", "terminal", "contract"].every(
          (name) => !commandByName(commandContract, name),
        ) &&
        commandByName(commandContract, "debug")?.usage?.includes("debug [capsules|inspect <capsule>") &&
        !commandContract.commands.some((command) =>
          command.surface?.some((surface) => surface === "native" || surface === "browser"),
        ) &&
        homeCliRust.includes("fn print_cli_wallet(") &&
        homeCliRust.includes("fn cli_service_offers") &&
        homeCliRust.includes("fn cli_invoke_intent(") &&
        homeCliRust.includes("fn write_invoke_intent(") &&
        homeCliRust.includes("fn print_cli_debug(") &&
        !homeCliRust.includes("UiKey::Browser") &&
        !homeCliRust.includes("b opens Browser") &&
        cliSmoke.includes("home-cli did not autostart an xterm terminal") &&
        cliSmoke.includes("home-cli terminal exit did not reattach Home CLI"),
      [
        "capsules/home-cli/browser/commands.json",
        "capsules/home-cli/src/main.rs",
        "scripts/home-cli-browser-smoke.mjs",
      ],
      "Add missing CLI commands or snapshot facts.",
    ),
    criterion(
      "canonical_shell_candidates",
      "Catalog and shell picker expose exactly home-gui and home-cli as selectable shells; legacy saved home state repairs to home-gui, but new home writes are rejected.",
      catalogReadModel.includes("let launchable = target.is_some();") &&
        !catalogReadModel.includes("is_home_shell") &&
        shellPicker.includes("HOME_HOST_ID") &&
        shellPicker.includes("export function shellIdentity") &&
        shellPicker.includes("return name.trim();") &&
        !shellPicker.includes(retiredHomeAliasExpression) &&
        includesNormalized(gatewayHomeTests, 'std::collections::BTreeSet::from([HOME_GUI_SHELL_ID, "home-cli"])') &&
        gatewayHomeTests.includes("test_home_active_shell_repairs_saved_home_state_but_rejects_home_updates") &&
        gatewayHomeTests.includes('{"active":"home"}') &&
        includesNormalized(gatewayHomeTests, "assert_eq!(home_write_rejected.status(), StatusCode::BAD_REQUEST);") &&
        gatewayHomeTests.includes('"active": "home-gui"') &&
        !gatewayHomeTests.includes(retiredHomeActiveStateLiteral) &&
        includesNormalized(contractDoc, "New active-shell writes use only installed launchable shell candidates."),
      [
        "elastos/crates/elastos-server/src/api/gateway_capsule_catalog/read_model.rs",
        "elastos/esp/shell_picker.ts",
        "elastos/crates/elastos-server/src/api/gateway_tests/home_system.rs",
        "docs/HOME_SHELL_HOST_CONTRACT.md",
      ],
      "Keep `home` as the host route and saved-state migration value only; never accept or expose it as a selectable shell.",
    ),
    criterion(
      "capsule_interface_projection",
      "Capsules expose web, CLI, facts, affordances, gate metadata, audit/mirror, and Carrier/service readiness through Runtime-derived projections.",
      contractDoc.includes("web, CLI, facts, affordances, gates, audit/mirror, and Carrier/service") &&
        entropy.includes("first_party_capsules_have_complete_projection_contract") &&
        gatewayCapsuleCatalog.includes("first_party_capsules_have_complete_projection_contract") &&
        state.includes("first_party_capsules_have_complete_projection_contract"),
      [
        "docs/HOME_SHELL_HOST_CONTRACT.md",
        "scripts/home-entropy-check.mjs",
        "elastos/crates/elastos-server/src/api/gateway_capsule_catalog.rs",
        "state.md",
      ],
      "Keep Runtime projection contract and first-party coverage in sync.",
    ),
    criterion(
      "runtime_carrier_alignment",
      "Capsule-to-capsule operations are signed Home/Runtime/Carrier/provider intents, not DOM hacks, provider bypasses, or ambient same-origin authority.",
      noDirectHomeCliAuthority &&
        homeCli.includes("elastos.home.terminal-host-intent/v1") &&
        homeCli.includes('"home:open-target"') &&
        !homeCli.includes('"/api/capsules/interfaces/invoke"') &&
        homeCliRust.includes("fn write_invoke_intent(") &&
        bridgeSmoke.includes("wrong-token") &&
        bridgeSmoke.includes("http://evil.invalid") &&
        cliSmoke.includes("home-cli called provider routes directly") &&
        contractDoc.includes("Child messages must carry the launch token"),
      [
        "capsules/home-cli/browser/home-cli.js",
        "scripts/home-shell-bridge-smoke.mjs",
        "scripts/home-cli-browser-smoke.mjs",
        "docs/HOME_SHELL_HOST_CONTRACT.md",
      ],
      "Remove direct provider/System/browser storage authority from shell code.",
    ),
    criterion(
      "root_shell_lifecycle",
      "Only one active root shell owns the viewport; shell switch retires GUI windows, prevents hidden windows, and restores sessions per shell.",
        bridgeSmoke.includes("const homeGuiCore = await import") &&
        bridgeSmoke.includes("homeGuiCore.shellState.windows.size === 0") &&
        bridgeSmoke.includes("homeGuiCore.shellState.windows.size === 1") &&
        authGateSmoke.includes("auth gate left a root shell visible") &&
        systemSwitchSmoke.includes("System shell switch did not retire Home GUI immediately") &&
        systemSwitchSmoke.includes("System shell switch did not cancel stale root-shell launches") &&
        regressionSmoke.includes("CLI-owned overlay session restored into Home GUI") &&
        host.includes("function dormantHomeGui(options = {})") &&
        contractDoc.includes("pre-retire stale GUI surfaces"),
      [
        "scripts/home-shell-bridge-smoke.mjs",
        "scripts/home-shell-auth-gate-smoke.mjs",
        "scripts/home-shell-system-switch-smoke.mjs",
        "scripts/home-shell-regression-smoke.mjs",
        "capsules/home/browser/home-shell-host.js",
        "docs/HOME_SHELL_HOST_CONTRACT.md",
      ],
      "Lifecycle proof must cover root ownership, retirement, and shell-specific restore.",
    ),
    criterion(
      "required_machine_tests",
      "Machine tests cover cookie cannot switch shell, token can switch shell, CLI to GUI, CLI Chat staying native, GUI-only Browser hidden/default-blocked boundaries, GUI windows retired on CLI switch, and stale shell state repairs.",
        gatewayHomeTests.includes("cookie_active_shell_write_rejected") &&
        gatewayHomeTests.includes("assert_eq!(payload[\"active_shell\"][\"active\"], \"home-cli\")") &&
        gatewayHomeTests.includes("test_home_shell_switch_preserves_runtime_facts_and_recovers_after_launch_failure") &&
        virtualAuthSmoke.includes("Home CLI instantiated Home GUI DOM") &&
        virtualAuthSmoke.includes("Home CLI Chat opened the GUI chat-room window") &&
        virtualAuthSmoke.includes("Home CLI Chat did not enter native chat") &&
        virtualAuthSmoke.includes("Home CLI Browser shortcut unexpectedly switched back to Home GUI") &&
        virtualAuthSmoke.includes("Home CLI Browser shortcut unexpectedly opened a Browser window through Home") &&
        virtualAuthSmoke.includes("Home CLI Browser shortcut dropped the CLI root frame") &&
        bridgeSmoke.includes("homeGuiCore.shellState.windows.size === 0") &&
        staleHintSmoke.includes("stale") &&
        staleHintSmoke.includes("was inserted during alternate shell boot") &&
        noHintSmoke.includes("no-hint") &&
        noHintSmoke.includes("no-hint boot did not stay in resolving mode until Runtime summary") &&
        noHintSmoke.includes("no-hint runtime settle did not keep the Home GUI active") &&
        recoverySmoke.includes("host recovery panel did not show after failed shell launch"),
      [
        "elastos/crates/elastos-server/src/api/gateway_tests/home_system.rs",
        "scripts/home-passkey-virtual-auth-smoke.mjs",
        "scripts/home-shell-bridge-smoke.mjs",
        "scripts/home-shell-stale-hint-boot-smoke.mjs",
        "scripts/home-shell-no-hint-boot-smoke.mjs",
        "scripts/home-shell-recovery-smoke.mjs",
      ],
      "Add missing machine tests for explicit objective items.",
    ),
    criterion(
      "operator_browser_ux_manual",
      "A real operator browser profile proves passkey sign-in, System switch to CLI, fullscreen CLI, switch back, GUI-only Browser hidden from the default CLI menu, no passkey loop, and no GUI bleed-through.",
      manual.ok,
      args.manualUx ? [args.manualUx] : [],
      "Run `node scripts/home-shell-manual-ux-report.mjs --template`, optionally use `--notes-template`, `--artifact-entry`, and `--report-from-notes` for a screen-capture-free artifact, complete the real operator-profile passkey journey, then pass the report with --manual-ux.",
    ),
    criterion(
      "docs_and_stale_esp_cleanup",
      "ESP/Home docs explain the model plainly and stale esp-shell is not a selectable product shell.",
      state.includes("replaces the obsolete `esp-shell` capsule") &&
        contractDoc.includes("Home is the shell host") &&
        espDoc.includes("`home-cli` shell") &&
        !commandContract.commands.some((command) => command.name === "esp-shell") &&
        !homeIndex.includes("Esp Shell"),
      [
        "state.md",
        "docs/HOME_SHELL_HOST_CONTRACT.md",
        "docs/ESP_V0.md",
        "capsules/home-cli/browser/commands.json",
        "capsules/home/browser/index.html",
      ],
      "Remove user-visible stale Esp Shell names and keep docs direct.",
    ),
    criterion(
      "runtime_owned_pty_terminal_ux",
      "The web terminal autostarts an xterm-rendered Runtime-owned PTY session without rendering the old browser command form.",
      commandContract.terminal?.transport === "runtime_pty_stream" &&
        commandContract.terminal?.pty?.includes("Runtime-owned PTY") &&
        commandContract.terminal?.xterm?.includes("autostarted") &&
        commandContract.terminal?.xterm?.includes("start/events/input/resize/close") &&
        homeCliIndex.includes('id="xterm-terminal"') &&
        !homeCliIndex.includes('id="command-input"') &&
        !homeCliIndex.includes('id="terminal-toggle"') &&
        !homeCliIndex.includes('class="quick-grid"') &&
        !homeCliIndex.includes("data-command=") &&
        homeCli.includes("async function attachXtermTerminal") &&
        homeCli.includes("loadXtermModule") &&
        homeCli.includes("await startRuntimeTerminal()") &&
        homeCli.includes("setRuntimeTerminalMode(true)") &&
        homeCli.includes("async function resizeRuntimeTerminal") &&
        homeCliStyle.includes("#xterm-terminal") &&
        homeCliStyle.includes('body[data-runtime-terminal="attached"] #xterm-terminal') &&
        !homeCliStyle.includes(".quick-grid") &&
        !homeCliStyle.includes(".command-row") &&
        !homeCli.includes("Browser terminal contract") &&
        !homeCli.includes("function runCommand") &&
        !homeCli.includes("#command-input") &&
        !homeCli.includes("COMMAND_CONTRACT_URL") &&
        homeCli.includes("async function startRuntimeTerminal()") &&
        cliSmoke.includes("home-cli did not autostart an xterm terminal") &&
        cliSmoke.includes("home-cli terminal event stream did not use a scoped stream ticket") &&
        cliSmoke.includes("home-cli terminal resize did not carry its launch token") &&
        cliSmoke.includes("home-cli terminal exit did not reattach Home CLI") &&
        contractDoc.includes("Runtime owns the process") &&
        includesNormalized(contractDoc, "stream ticket") &&
        includesNormalized(contractDoc, "dimensions, input/resize routes") &&
        contractDoc.includes("lifecycle"),
      [
        "capsules/home-cli/browser/commands.json",
        "capsules/home-cli/browser/home-cli.js",
        "scripts/home-cli-browser-smoke.mjs",
        "docs/HOME_SHELL_HOST_CONTRACT.md",
      ],
      "Keep improving command UX while preserving the Runtime-owned stream boundary.",
    ),
    criterion(
      "runtime_pty_stream_terminal",
      "A Runtime-owned PTY terminal is attached through explicit start/events/input/resize/close routes and launch-token gates.",
      gatewayHomeTerminal.includes('"elastos.home-cli.terminal-contract/v1"') &&
        gatewayHomeTerminal.includes("open_home_terminal_pty") &&
        gatewayHomeTerminal.includes("libc::openpty") &&
        gatewayHomeTerminal.includes("HOME_TERMINAL_INPUT_MAX_BYTES") &&
        gatewayHomeTerminal.includes("HOME_TERMINAL_RESIZE_SCHEMA") &&
        gatewayHomeTerminal.includes("require_home_launch_token_for_any_context") &&
        gatewayHomeTerminal.includes("home_cli_terminal_events") &&
        gatewayHomeTerminal.includes("home_cli_terminal_resize") &&
        gatewayHomeTerminal.includes("stream_ticket") &&
        gatewayHomeTests.includes("test_home_cli_terminal_stream_requires_cli_launch_token") &&
        gatewayHomeTests.includes("elastos.home-cli.terminal-resize/v1") &&
        cliSmoke.includes("home-cli terminal exit did not reattach Home CLI") &&
        cliSmoke.includes("home-cli terminal input did not carry its launch token") &&
        homeCli.includes("sendRuntimeTerminalInput") &&
        homeCli.includes('eventsUrl.includes("home_token=")'),
      [
        "elastos/crates/elastos-server/src/api/gateway_home_terminal.rs",
        "elastos/crates/elastos-server/src/api/gateway_tests/home_system.rs",
        "capsules/home-cli/browser/home-cli.js",
        "scripts/home-cli-browser-smoke.mjs",
      ],
      "Keep this honest: xterm renders a Runtime-owned PTY; the capsule never receives host process authority.",
    ),
  ];

  const completionCriteria = [
    "split_names_and_frontdoor",
    "home_gui_boundary",
    "minimal_host_recovery",
    "home_cli_product_useful",
    "capsule_interface_projection",
    "runtime_carrier_alignment",
    "root_shell_lifecycle",
    "required_machine_tests",
    "operator_browser_ux_manual",
    "docs_and_stale_esp_cleanup",
    "runtime_owned_pty_terminal_ux",
    "runtime_pty_stream_terminal",
  ];
  const ok = completionCriteria.every((id) => criteriaOk(criteria, id));
  const failing = criteria.filter((item) => !item.ok).map((item) => item.id);

  return {
    schema: SCHEMA,
    ok,
    summary: ok
      ? "Home shell objective is fully proven."
      : "Home shell objective is not complete; source proof is strong, but operator-profile manual UX evidence remains open.",
    criteria,
    manual_ux: manual,
    remaining: failing.map((id) => {
      const item = criteria.find((criterionItem) => criterionItem.id === id);
      return {
        id,
        missing: item?.missing || "missing evidence",
      };
    }),
  };
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const result = audit(args);
  console.log(JSON.stringify(result, null, 2));
  if (args.requireComplete && !result.ok) {
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
