import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const read = (path) => readFileSync(new URL("../" + path, import.meta.url), "utf8");
const system = read("capsules/system/browser/system.js");
const setup = read("capsules/home-gui/browser/shell-setup-sheet.js");
assert(!read("capsules/system/browser/index.html").includes('id="recovery-profile-name"'));
assert(read("capsules/home-gui/browser/home-gui-template.html").includes('id="setup-sheet-later"'));
function functionSource(source, name) {
  const start = source.search(new RegExp("(?:async )?function " + name + "\\("));
  assert(start >= 0, name);
  return source.slice(start, source.indexOf("\n}", start) + 2);
}
const calls = [];
const outcomes = [];
const context = vm.createContext({
  recoveryProfileRequired: false, recoveryProfileStatus: "unavailable", recoveryExportBusy: false,
  readText: (value) => typeof value === "string" ? value.trim() : "",
  recoveryDownloadPassword: () => "", shellHeaders: (headers) => headers,
  requestPasskeyStepUp: async (scope, intent) => { calls.push({ scope, intent }); return "step-up"; },
  fetchJson: async (url, init) => { calls.push({ url, request: JSON.parse(init.body) }); return {}; },
  hasShellAccess: () => true, recoveryDownloadButton: { textContent: "Download", disabled: false },
  clearRecoveryPending() {}, recoveryPasswordInput: null,
  showRecoveryStatus: (text, kind) => outcomes.push({ text, kind }),
  showRecoveryNote: (text, kind) => outcomes.push({ text, kind }),
  downloadRecoveryKit: () => outcomes.push("download"),
  notifyHomeSummaryChanged: () => outcomes.push("refresh-home"),
  refreshSystemSummary: async () => { throw new Error("summary failed"); },
});
vm.runInContext(["renderRecoveryProfileSetup", "exportFullRecoveryBundle", "onRecoveryDownload", "setRecoveryButton", "setRecoveryStatus"].map((name) => functionSource(system, name)).join("\n"), context);
const readiness = (status) => ({ profile_readiness: { schema: "elastos.profile.readiness/v1", status } });
const status = { principal_id: "person:fixture", localhost_root: "localhost://fixture" };
context.renderRecoveryProfileSetup(readiness("setup_required"));
context.setRecoveryStatus(status);
assert.equal(context.recoveryDownloadButton.disabled, true, "System export waits for an existing Profile");
assert(outcomes.some((item) => item.text?.includes("Import your Recovery Kit")));
await assert.rejects(context.exportFullRecoveryBundle(status), /Import your Recovery Kit/);
context.recoveryDownloadButton.disabled = false;
assert.equal(await context.onRecoveryDownload(), false, "readiness guards the action independently of the button");
assert.equal(calls.length, 0, "missing Profile fails before status or step-up requests");
context.renderRecoveryProfileSetup({});
context.setRecoveryStatus(status);
assert.equal(context.recoveryDownloadButton.disabled, true);
assert(outcomes.some((item) => item.text?.includes("Refresh System and try again")));
await assert.rejects(context.exportFullRecoveryBundle(status), /could not be checked/);
assert.equal(calls.length, 0, "unknown readiness fails before step-up");
context.renderRecoveryProfileSetup(readiness("ready"));
context.setRecoveryStatus(status);
assert.equal(context.recoveryDownloadButton.disabled, false, "verified readiness permits retry");
await context.exportFullRecoveryBundle(status);
const [stepUp, exported] = calls;
assert.equal(stepUp.scope, "auth.full-recovery-bundle.export");
assert.equal(exported.url, "/api/auth/recovery/full-export");
assert.deepEqual(Object.keys(exported.request).sort(), ["schema", "principal_id", "localhost_root", "label", "step_up_token"].sort());
assert(!Object.hasOwn(stepUp.intent, "profile_display_name"));
await assert.rejects(context.exportFullRecoveryBundle(status, () => false), /cancelled/);
assert.equal(calls.length, 3, "document lost during step-up cannot dispatch full-export");
outcomes.length = 0;

context.renderRecoveryProfileSetup(readiness("ready"));
context.recoveryDownloadButton.disabled = false;
context.fetchJson = async () => status;
context.exportFullRecoveryBundle = async () => ({ included: { people_identity: false } });
assert.equal(await context.onRecoveryDownload(), true);
assert(outcomes.includes("download"));
assert(outcomes.includes("refresh-home"));
assert(outcomes.some((item) => item.text === "Recovery Kit downloaded. Store it offline."));
assert(!outcomes.some((item) => item.kind === "error"), "post-download refresh failure stays successful");

let rejectExport;
let exports = 0;
context.exportFullRecoveryBundle = () => {
  exports += 1;
  return new Promise((_resolve, reject) => { rejectExport = reject; });
};
const pending = context.onRecoveryDownload();
await Promise.resolve();
assert.equal(context.recoveryDownloadButton.disabled, true);
assert.equal(await context.onRecoveryDownload(), false);
assert.equal(exports, 1);
rejectExport(new Error("Passkey cancelled"));
assert.equal(await pending, false);
assert.equal(context.recoveryDownloadButton.disabled, false, "cancel permits explicit retry");
assert(outcomes.some((item) => item.text === "Passkey cancelled"));

let activeTab = "security";
Object.assign(context, {
  requestedSettingsActionFocused: false, requestedSettingsTab: "security",
  readQueryParam: () => "",
  recoveryDownloadButton: { disabled: false, focus() { this.focused = true; } },
  document: { querySelector: () => ({ dataset: { settings: activeTab } }) },
});
vm.runInContext(functionSource(system, "focusRequestedSettingsAction"), context);
context.focusRequestedSettingsAction();
assert.equal(context.recoveryDownloadButton.focused, true);
context.requestedSettingsActionFocused = false;
context.recoveryDownloadButton.focused = false;
activeTab = "about";
context.focusRequestedSettingsAction();
assert.equal(context.recoveryDownloadButton.focused, false, "late completion keeps the active tab's focus");

const home = vm.createContext({
  PROFILE_READINESS_SCHEMA: "elastos.profile.readiness/v1",
  RECOVERY_READINESS_SCHEMA: "elastos.recovery.readiness/v1",
});
vm.runInContext(["typedReadinessStatus", "homeSetupStatus", "homeRecoveryStatus", "setupFinished", "homeSetupNeedsAct"].map((name) => functionSource(setup, name)).join("\n"), home);
const summary = { authority: { signed_in: true }, identity: {
  ...readiness("ready"),
  recovery_readiness: { schema: "elastos.recovery.readiness/v1", status: "setup_required" },
} };
let shown = 0;
const reminders = [];
Object.assign(home, {
  SETUP_HOLD_TARGETS: new Set(["chat-room"]), SETUP_REMINDER_ID: "setup",
  shellState: { currentSummary: summary, shellLayoutState: { setupReminderDismissed: true } },
  dismissedThisSession: false,
  showSetupSheet: () => { shown += 1; },
  setupSheetOpen: () => false,
  rememberChromeNotification: (item) => reminders.push(item),
});
vm.runInContext(["holdHomeSetupAct", "syncSetupSheet", "rememberSetupReminder"].map((name) => functionSource(setup, name)).join("\n"), home);
assert.equal(home.holdHomeSetupAct("chat-room"), false, "backup is independent of local Profile readiness");
home.syncSetupSheet(null, summary);
assert.equal(shown, 0, "Later persists as a quiet reminder on reload");
assert.equal(reminders.length, 1);
assert.equal(summary.identity.recovery_readiness.status, "setup_required", "dismissal does not fabricate export readiness");
for (const value of ["setup_required", "unavailable"]) {
  summary.identity.profile_readiness.status = value;
  assert.equal(home.holdHomeSetupAct("chat-room"), true);
}
summary.identity.profile_readiness.status = "ready";
summary.identity.recovery_readiness.status = "ready";
assert.equal(home.homeSetupNeedsAct(summary), false);
summary.authority.signed_in = false;
assert.equal(home.homeSetupNeedsAct(summary), false);
assert(read("capsules/home-gui/browser/shell-core.js").includes("setupReminderDismissed: stored?.setupReminderDismissed === true"));
assert(read("capsules/people/browser/people.js").includes('window.top.postMessage({ type: "home:refresh-summary", homeToken }, homeParentOrigin);'));
// Compose the actual Recover callback, Home shell sync and System focus/import
// handlers. A ready HTTP callback must not leave a competing Profile modal.
function recoveryPresentationFixture(profile = "setup_required", recovery = "setup_required") {
  const elements = new Map(), notifications = new Map(), launches = [];
  let focus = "", reminder, unlock, saveCalls = 0;
  const element = id => {
    if (!elements.has(id)) {
      const classes = new Set(), handlers = new Map();
      elements.set(id, { hidden: id === "#setup-sheet", inert: true, disabled: false, textContent: "", style: {},
        classList: { contains: name => classes.has(name), add: name => classes.add(name), remove: name => classes.delete(name),
          toggle(name, on) { if (on) classes.add(name); else classes.delete(name); } },
        setAttribute() {}, addEventListener: (type, handler) => handlers.set(type, handler),
        emit: type => handlers.get(type)?.({}), focus() { focus = id; } });
    }
    return elements.get(id);
  };
  const current = { authority: { signed_in: true }, identity: { ...readiness(profile),
    recovery_readiness: { schema: "elastos.recovery.readiness/v1", status: recovery } } };
  const systemView = vm.createContext({ requestedSettingsActionFocused: false, requestedSettingsTab: "security",
    readQueryParam: () => "import", readText: value => value,
    recoveryImportInput: element("#recovery-import"), recoveryDownloadButton: element("#recovery-download"),
    document: { querySelector: () => ({ dataset: { settings: "security" } }) },
    hasShellAccess: () => true, clearRecoveryPending() {}, showRecoveryStatus() {}, showRecoveryNote() {},
    pendingRecoveryImport: null, shellHeaders: () => ({}), fetchJson: async () => ({}),
    recoveryImportPlan: () => ({ request: {} }), submitRecoveryImport: async () => ({ complete: true }),
    recoveryImportIsComplete: response => response.complete, showWalletRestorePending() {},
  });
  vm.runInContext(["focusRequestedSettingsAction", "onRecoveryImport"].map(name => functionSource(system, name)).join("\n"), systemView);
  const openSystem = async (target, options) => {
    assert.equal(target, "system");
    launches.push({ target, options });
    systemView.readQueryParam = () => options?.query?.recovery || "";
    systemView.requestedSettingsActionFocused = false;
    systemView.focusRequestedSettingsAction();
  };
  const shell = vm.createContext({ shellState: { currentSummary: current, shellLayoutState: {} },
    document: { querySelector: element },
    window: { addEventListener() {}, clearTimeout() {}, setTimeout() { return 1; }, matchMedia: () => ({ matches: true }) },
    targetById: (_summary, target) => target === "system" ? { id: target } : null,
    saveShellLayoutState() {}, forgetChromeNotification: id => notifications.delete(id),
    rememberChromeNotification: note => notifications.set(note.id, note),
    setHomeSetupNotificationHandler: callback => { reminder = callback; },
    openTarget: openSystem, openSystemRecoverySave: async () => { saveCalls++; return true; },
  });
  vm.runInContext(setup.replace(/^import[\s\S]*?;\n/gm, "").replace(/export /g, ""), shell);
  shell.bindSetupSheet();
  const sync = (previous = current) => shell.syncSetupSheet(previous, current);
  const host = vm.createContext({ document: { body: { dataset: {} } }, console,
    activeShellBootHintTarget: () => "", refreshHomeSession: async () => ({}),
    refreshShellSummary: async () => { sync(null); return current; }, homeSummarySignedIn: () => true,
    fetchJson: async () => ({}), hideHomeUnlock() {}, startShellTimers() {}, isHomeAuthError: () => false,
    enterHostAuthGate() {}, currentSignedProfileDisplayName: () => "", hideHostBootMask() {},
    showHomeUnlock(callback) { unlock = callback; }, activateDesktopShell: async () => {},
    openTargetFromHomeGui: openSystem,
  });
  const hostSource = read("capsules/home/browser/home-shell-host.js");
  vm.runInContext(functionSource(read("capsules/home/browser/shell-auth.js"), "profileReadinessActionTarget"), host);
  vm.runInContext(["boot", "showHostAuthGate"].map(name => functionSource(hostSource, name)).join("\n"), host);
  return { shell, current, sync, systemView, element, notifications, launches, focus: () => focus,
    reminder: () => reminder(), saveCalls: () => saveCalls,
    async signIn(response = current.identity, flow) { await host.showHostAuthGate(); await unlock(response, flow); await Promise.resolve(); },
    async recover() { await host.showHostAuthGate(); await unlock(current.identity, { enrollmentPurpose: "recover" }); await Promise.resolve(); } };
}
{
  const f = recoveryPresentationFixture();
  await f.recover();
  assert.equal(f.element("#setup-sheet").hidden, true, "Recover callback + shell sync opened a competing Profile sheet");
  assert.equal(f.focus(), "#recovery-import");
  assert.match([...f.notifications.values()][0].title, /recovery/i);
  assert.equal(f.saveCalls(), 0);
  f.sync();
  assert.equal(f.focus(), "#recovery-import", "repeat summary stole active import focus");
  f.reminder();
  assert.equal(f.element("#setup-sheet-recovery").textContent, "Continue recovery");
  assert.doesNotMatch(f.element("#setup-sheet-lead").textContent, /confirm|name|People/i);
  await f.shell.openRecoveryAct();
  assert.deepEqual(JSON.parse(JSON.stringify(f.launches.at(-1))), { target: "system", options: { query: { settings: "security", recovery: "import" } } });
  assert.equal(f.focus(), "#recovery-import");
  for (const files of [[], [{ text: async () => "invalid kit" }], [{ text: async () => "{}" }]]) {
    await f.systemView.onRecoveryImport({ target: { files } });
    f.sync();
    assert.equal(f.element("#setup-sheet-recovery").textContent, "Continue recovery", "file/callback outcome fabricated Profile readiness");
    assert.equal(f.focus(), "#recovery-import");
  }
  f.shell.hideSetupSheet({ restoreFocus: false });
  assert.equal(f.shell.holdHomeSetupAct("chat-room"), true);
  assert.equal(f.element("#setup-sheet-recovery").textContent, "Continue recovery");
  f.current.identity.profile_readiness.status = "ready";
  f.sync();
  assert.equal(f.element("#setup-sheet-recovery").textContent, "Save Recovery Kit");
  assert.equal(f.shell.holdHomeSetupAct("chat-room"), false);
}
{
  const reload = recoveryPresentationFixture();
  await reload.signIn();
  assert.equal(reload.element("#setup-sheet").hidden, true, "reload before import must keep quiet recovery guidance");
  assert.deepEqual(JSON.parse(JSON.stringify(reload.launches)), [{ target: "system", options: { query: { settings: "security", recovery: "import" } } }]);
  assert.equal(reload.focus(), "#recovery-import", "normal sign-in after restart must continue import without a stored Recover flag");
  assert.match([...reload.notifications.values()][0].body, /Recovery Kit/);
  for (const response of [{}, readiness("unavailable"), readiness("unknown"),
    { profile_readiness: { schema: "wrong", status: "setup_required" } },
    { profile_readiness: { schema: "elastos.profile.readiness/v1", status: null } }]) {
    const unknown = recoveryPresentationFixture("unavailable");
    await unknown.signIn(response);
    assert.deepEqual(JSON.parse(JSON.stringify(unknown.launches)), [{ target: "system", options: { query: { settings: "security" } } }]);
    assert.equal(unknown.saveCalls(), 0);
    assert.equal(unknown.element("#setup-sheet").hidden, true);
  }
  for (const [profile, recovery] of [["unavailable", "setup_required"], ["ready", "unavailable"]]) {
    const f = recoveryPresentationFixture(profile, recovery);
    f.sync(null); f.reminder();
    assert.equal(f.element("#setup-sheet-recovery").textContent, "Open System");
    await f.shell.openRecoveryAct();
    assert.deepEqual(JSON.parse(JSON.stringify(f.launches.at(-1).options.query)), { settings: "security" });
    assert.equal(f.saveCalls(), 0, "unknown readiness dispatched Save");
  }
  const created = recoveryPresentationFixture("ready");
  await created.signIn(created.current.identity, { enrollmentPurpose: "create" });
  assert.equal(created.launches.length, 0, "Profile-ready Create stays in Home");
  assert.equal(created.element("#setup-sheet-recovery").textContent, "Save Recovery Kit");
  created.element("#setup-sheet-later").emit("click");
  created.sync();
  assert.equal(created.element("#setup-sheet").hidden, true);
  assert.equal(created.saveCalls(), 0);
  created.reminder(); await created.shell.openRecoveryAct();
  assert.equal(created.saveCalls(), 1);
  assert.equal(created.current.identity.recovery_readiness.status, "setup_required");
  created.current.identity.recovery_readiness.status = "ready";
  created.sync();
  assert.equal(created.element("#setup-sheet-recovery").textContent, "Ready");
  const existing = recoveryPresentationFixture("ready", "ready");
  await existing.signIn();
  assert.equal(existing.launches.length, 0, "existing ready Profile sign-in stays in Home");
  assert.equal(existing.element("#setup-sheet").hidden, true);
}
console.log("PASS existing-Profile export, retryable readiness, single-flight export, focus, Later and composed Recover guidance");
