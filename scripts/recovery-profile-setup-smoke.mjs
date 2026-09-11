import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const system = read("capsules/system/browser/system.js");
const setup = read("capsules/home-gui/browser/shell-setup-sheet.js");
function functionSource(source, name) {
  const start = source.search(new RegExp(`(?:async )?function ${name}\\(`));
  assert(start >= 0, name);
  const end = source.indexOf("\n}", start);
  return source.slice(start, end + 2);
}
const input = { value: "", focus() { this.focused = true; } };
const panel = { hidden: true };
const calls = [];
const context = vm.createContext({
  recoveryProfileName: input, recoveryProfileSetup: panel,
  recoveryProfileRequired: false, recoveryProfileDraftInitialized: false,
  recoveryProfileStatus: "unavailable",
  recoveryExportBusy: false,
  readText: (value) => typeof value === "string" ? value.trim() : "",
  recoveryDownloadPassword: () => "",
  shellHeaders: (headers) => headers,
  requestPasskeyStepUp: async (scope, intent) => { calls.push({ scope, intent }); return "step-up"; },
  fetchJson: async (url, init) => { calls.push({ url, request: JSON.parse(init.body) }); return {}; },
});
vm.runInContext(functionSource(system, "renderRecoveryProfileSetup") + "\n" + functionSource(system, "exportFullRecoveryBundle"), context);
const identity = { profile_readiness: { schema: "elastos.profile.readiness/v1", status: "setup_required" }, profile_setup_display_name: "Suggested" };
context.renderRecoveryProfileSetup(identity);
assert.equal(input.value, "Suggested");
assert.equal(panel.hidden, false);
const status = { principal_id: "person:fixture", localhost_root: "localhost://fixture" };
await context.exportFullRecoveryBundle(status);
assert.equal(calls[0].intent.profile_display_name, "Suggested");
assert.equal(calls[1].request.profile_display_name, "Suggested");
assert.equal(calls[1].url, "/api/auth/recovery/full-export");
assert.deepEqual(Object.keys(calls[1].request).sort(), ["schema", "principal_id", "localhost_root", "label", "step_up_token", "profile_display_name"].sort());
assert.equal(calls[0].scope, "auth.full-recovery-bundle.export");
input.value = "Edited";
context.renderRecoveryProfileSetup(identity);
assert.equal(input.value, "Edited");
await context.exportFullRecoveryBundle(status);
assert.equal(calls[2].intent.profile_display_name, "Edited");
assert.equal(calls[3].request.profile_display_name, "Edited");
input.value = "";
context.renderRecoveryProfileSetup(identity);
assert.equal(input.value, "");
await assert.rejects(context.exportFullRecoveryBundle(status), /Enter a Profile name/);
assert.equal(calls.length, 4, "blank name must stop before passkey or export");
context.renderRecoveryProfileSetup({ profile_readiness: { schema: "elastos.profile.readiness/v1", status: "ready" } });
await context.exportFullRecoveryBundle(status);
assert.equal(panel.hidden, true);
assert(!("profile_display_name" in calls[4].intent));
assert(!("profile_display_name" in calls[5].request));
context.renderRecoveryProfileSetup({});
await assert.rejects(context.exportFullRecoveryBundle(status), /could not be checked/);
assert.equal(calls.length, 6);

const outcomes = [];
context.renderRecoveryProfileSetup({ profile_readiness: { schema: "elastos.profile.readiness/v1", status: "ready" } });
Object.assign(context, {
  hasShellAccess: () => true, recoveryDownloadButton: { textContent: "Download" },
  clearRecoveryPending: () => {}, setRecoveryButton: () => {},
  showRecoveryStatus: (text, kind) => outcomes.push({ text, kind }),
  showRecoveryNote: (text, kind) => outcomes.push({ text, kind }),
  downloadRecoveryKit: () => outcomes.push("download"), recoveryPasswordInput: null,
  notifyHomeSummaryChanged: () => outcomes.push("refresh-home"),
  refreshSystemSummary: async () => { throw new Error("summary failed"); },
});
vm.runInContext(functionSource(system, "onRecoveryDownload"), context);
context.exportFullRecoveryBundle = async () => ({ included: { people_identity: true } });
context.fetchJson = async () => status;
await context.onRecoveryDownload();
assert(outcomes.includes("download"));
assert(outcomes.includes("refresh-home"));
assert(!outcomes.some((outcome) => outcome.kind === "error"));
assert(outcomes.some((outcome) => outcome.text?.includes("Recovery Kit downloaded")));
let releaseExport;
let exportCount = 0;
vm.runInContext(functionSource(system, "setRecoveryButton"), context);
context.exportFullRecoveryBundle = () => {
  exportCount += 1;
  return new Promise((_resolve, reject) => { releaseExport = reject; });
};
const pending = context.onRecoveryDownload();
await Promise.resolve();
assert.equal(input.readOnly, true, "signed name stays frozen and focusable during export");
await context.onRecoveryDownload();
assert.equal(exportCount, 1, "pending export cannot be submitted twice");
releaseExport(new Error("export refused"));
await pending;
assert.equal(input.readOnly, false, "failed export restores name editing");
assert.equal(context.recoveryDownloadButton.disabled, false);
let releaseSummary;
let summaryReached;
const atSummary = new Promise((resolve) => { summaryReached = resolve; });
context.exportFullRecoveryBundle = async () => { exportCount += 1; return { included: { people_identity: true } }; };
context.refreshSystemSummary = () => new Promise((resolve) => { releaseSummary = resolve; summaryReached(); });
const pendingSummary = context.onRecoveryDownload();
await atSummary;
assert.equal(typeof releaseSummary, "function");
assert.equal(input.readOnly, true);
assert.equal(context.recoveryDownloadButton.disabled, true);
await context.onRecoveryDownload();
assert.equal(exportCount, 2, "summary refresh remains in the same single-flight operation");
releaseSummary();
await pendingSummary;
assert.equal(input.readOnly, false);
assert.equal(context.recoveryDownloadButton.disabled, false);

let activeTab = "security";
Object.assign(context, {
  requestedSettingsActionFocused: false, requestedSettingsTab: "security",
  readQueryParam: () => "",
  recoveryDownloadButton: { disabled: false, focus() { this.focused = true; } },
  document: { querySelector: () => ({ dataset: { settings: activeTab } }) },
});
vm.runInContext(functionSource(system, "focusRequestedSettingsAction"), context);
context.renderRecoveryProfileSetup(identity);
input.focused = false;
context.focusRequestedSettingsAction();
assert.equal(input.focused, true, "initial setup focuses the editable name");
context.requestedSettingsActionFocused = false;
activeTab = "about";
input.focused = false;
context.focusRequestedSettingsAction();
assert.equal(input.focused, false, "late readiness does not steal another tab's focus");

const home = vm.createContext({ PROFILE_READINESS_SCHEMA: "elastos.profile.readiness/v1", RECOVERY_READINESS_SCHEMA: "elastos.recovery.readiness/v1" });
vm.runInContext(["typedReadinessStatus", "homeSetupStatus", "homeRecoveryStatus", "setupFinished", "homeSetupNeedsAct"].map((name) => functionSource(setup, name)).join("\n"), home);
const summary = { authority: { signed_in: true }, identity: {
  profile_readiness: { schema: "elastos.profile.readiness/v1", status: "ready" },
  recovery_readiness: { schema: "elastos.recovery.readiness/v1", status: "setup_required" },
} };
assert.equal(home.homeSetupNeedsAct(summary), true, "existing Profile still needs an exported kit covering it");
let shown = 0;
Object.assign(home, { SETUP_HOLD_TARGETS: new Set(["chat-room"]), shellState: { currentSummary: summary }, showSetupSheet: () => { shown += 1; } });
vm.runInContext(functionSource(setup, "holdHomeSetupAct"), home);
assert.equal(home.holdHomeSetupAct("chat-room"), false, "kit coverage reminder does not block an existing Profile's Chat");
for (const status of ["setup_required", "unavailable"]) {
  summary.identity.profile_readiness.status = status;
  assert.equal(home.holdHomeSetupAct("chat-room"), true);
}
assert.equal(shown, 2);
summary.identity.profile_readiness.status = "ready";
summary.identity.recovery_readiness.status = "ready";
assert.equal(home.homeSetupNeedsAct(summary), false);
summary.authority.signed_in = false;
assert.equal(home.homeSetupNeedsAct(summary), false);
assert.equal(home.holdHomeSetupAct("chat-room"), false);
assert(read("capsules/people/browser/people.js").includes('window.top.postMessage({ type: "home:refresh-summary", homeToken }, homeParentOrigin);'));
assert(system.includes("renderRecoveryProfileSetup(identity);"));
assert(system.includes("notifyHomeSummaryChanged();"));

// Independent Save acknowledgement/cancellation cases adapted from
// 6972e165:scripts/recovery-profile-setup-smoke.mjs; keep Profile creation above.
context.renderRecoveryProfileSetup({ profile_readiness: { schema: "elastos.profile.readiness/v1", status: "ready" } });
vm.runInContext(functionSource(system, "exportFullRecoveryBundle"), context);
let activeSaveDocument = true;
let dispatchedExports = 0;
context.requestPasskeyStepUp = async () => {
  activeSaveDocument = false;
  return "step-up";
};
context.fetchJson = async (url) => {
  if (url === "/api/auth/recovery/status") return status;
  assert.equal(url, "/api/auth/recovery/full-export");
  dispatchedExports += 1;
  return { included: { people_identity: true } };
};
context.refreshSystemSummary = async () => { throw new Error("summary failed"); };
outcomes.length = 0;
assert.equal(await context.onRecoveryDownload({ isActive: () => activeSaveDocument }), false);
assert.equal(dispatchedExports, 0, "document lost during passkey verification cannot dispatch export");
assert(!outcomes.includes("download"));
assert(outcomes.some((outcome) => outcome.text === "Recovery Kit save was cancelled."));
assert.equal(input.readOnly, false, "cancel restores Profile-name editing");
assert.equal(context.recoveryDownloadButton.disabled, false, "cancel permits an explicit retry");

activeSaveDocument = true;
context.requestPasskeyStepUp = async () => "step-up";
outcomes.length = 0;
assert.equal(await context.onRecoveryDownload({ isActive: () => activeSaveDocument }), true);
assert.equal(dispatchedExports, 1);
assert(outcomes.includes("download"));
assert(outcomes.includes("refresh-home"));
assert(!outcomes.some((outcome) => outcome.kind === "error"), "summary failure after download retains successful acknowledgement");
assert.equal(input.readOnly, false);
assert.equal(context.recoveryDownloadButton.disabled, false);

// Independent Later persistence case adapted from the same donor smoke.
// Exercise the current hide/sync functions without its older import-only flow.
const laterSummary = { authority: { signed_in: true }, identity: {
  profile_readiness: { schema: "elastos.profile.readiness/v1", status: "ready" },
  recovery_readiness: { schema: "elastos.recovery.readiness/v1", status: "setup_required" },
} };
const reminders = [];
let savedLayout;
let reopenedSheets = 0;
const later = vm.createContext({
  PROFILE_READINESS_SCHEMA: "elastos.profile.readiness/v1",
  RECOVERY_READINESS_SCHEMA: "elastos.recovery.readiness/v1",
  SETUP_REMINDER_ID: "setup",
  shellState: { currentSummary: laterSummary, shellLayoutState: {} },
  sheet: { hidden: false, inert: false, setAttribute() {} },
  finishedHideTimer: null, dismissedThisSession: false, drag: null,
  restoreSetupSheetOverlay() {},
  showSetupSheet: () => { reopenedSheets += 1; },
  rememberChromeNotification: (item) => reminders.push(item),
  saveShellLayoutState: () => { savedLayout = { ...later.shellState.shellLayoutState }; },
});
vm.runInContext(["typedReadinessStatus", "homeSetupStatus", "homeRecoveryStatus", "setupFinished", "homeSetupNeedsAct", "setupSheetOpen", "rememberSetupReminder", "hideSetupSheet", "syncSetupSheet"].map((name) => functionSource(setup, name)).join("\n"), later);
later.hideSetupSheet({ restoreFocus: false });
assert.equal(savedLayout.setupReminderDismissed, true, "Later persists its dismissal");
assert.equal(later.sheet.hidden, true);
assert.equal(later.sheet.inert, true);
later.shellState.shellLayoutState = { ...savedLayout };
later.dismissedThisSession = false;
reminders.length = 0;
later.syncSetupSheet(null, laterSummary);
assert.equal(reopenedSheets, 0, "Later stays a quiet reminder after reload");
assert.equal(reminders.length, 1);
assert.equal(laterSummary.identity.recovery_readiness.status, "setup_required", "dismissal preserves the pending Recovery Kit requirement");
assert(read("capsules/home-gui/browser/shell-core.js").includes("setupReminderDismissed: stored?.setupReminderDismissed === true"));
console.log("PASS combined Profile and Recovery Kit setup smoke");
