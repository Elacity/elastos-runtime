import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const setup = read("capsules/home-gui/browser/shell-setup-sheet.js");
function functionSource(source, name) {
  const start = source.search(new RegExp(`(?:export )?(?:async )?function ${name}\\(`));
  assert(start >= 0, name);
  return source.slice(start, source.indexOf("\n}", start) + 2).replace(/^export /, "");
}

function fixture() {
  const classes = new Set();
  const sheet = { hidden: false, inert: false, setAttribute() {},
    classList: { add: (c) => classes.add(c), remove: (c) => classes.delete(c), contains: (c) => classes.has(c) } };
  const summary = { authority: { signed_in: true }, identity: {
    profile_readiness: { schema: "elastos.profile.readiness/v1", status: "ready" },
    recovery_readiness: { schema: "elastos.recovery.readiness/v1", status: "setup_required" },
  } };
  const f = { classes, sheet, summary, opened: [], saveResult: true };
  f.card = { style: {}, attributes: {}, classList: { add() {}, remove() {} },
    setAttribute(name, value) { this.attributes[name] = value; }, removeAttribute(name) { delete this.attributes[name]; },
    getBoundingClientRect: () => ({ left: 900, top: 40, width: 360, height: 200 }) };
  f.context = vm.createContext({
    PROFILE_READINESS_SCHEMA: "elastos.profile.readiness/v1",
    RECOVERY_READINESS_SCHEMA: "elastos.recovery.readiness/v1",
    SETUP_REMINDER_ID: "setup",
    shellState: { currentSummary: summary, shellLayoutState: {} },
    sheet, card: f.card, recoveryBody: { textContent: "" }, recoveryButton: { disabled: false, focus() { f.focused = "recovery"; } },
    closeButton: { focus() { f.focused = "close"; } },
    recoveryActBusy: false, dismissedThisSession: false, drag: null,
    window: { innerWidth: 1400, innerHeight: 900 },
    targetById: () => ({ id: "system" }), setTimeout, queueMicrotask,
    renderSetupSheet() { f.context.recoveryButton.disabled = f.context.setupFinished(summary); }, forgetChromeNotification() {}, rememberChromeNotification() {}, showSetupSheet() {}, saveShellLayoutState() {},
    openTarget: (id, options) => f.opened.push([id, options]),
    openSystemRecoverySave: async () => {
      assert(classes.has("is-yielded"), "System is in front while it saves");
      if (f.saveResult) summary.identity.recovery_readiness.status = "ready";
      return f.saveResult;
    },
  });
  vm.runInContext(["typedReadinessStatus", "homeSetupStatus", "homeRecoveryStatus", "setupFinished",
    "homeSetupNeedsAct", "setupSheetOpen", "yieldSetupSheet", "restoreSetupSheetOverlay", "openRecoveryAct",
    "syncSetupSheet", "returnSetupSheet", "hideSetupSheet", "rememberSetupReminder", "openGoal",
    "onSetupCardPointerDown", "onSetupCardPointerMove", "onSetupCardPointerUp"]
    .map((name) => functionSource(setup, name)).join("\n"), f.context);
  f.shown = () => !sheet.hidden && !classes.has("is-yielded");
  return f;
}

test("Save Recovery Kit brings the onboarding sheet back, done or for Retry", async () => {
  const f = fixture();
  f.saveResult = false;
  await f.context.openRecoveryAct();
  assert(f.shown(), "failed save brings the sheet back for Retry");
  assert.match(f.context.recoveryBody.textContent, /not saved/);
  assert.equal(f.focused, "recovery", "Retry has keyboard focus");
  f.saveResult = true;
  await f.context.openRecoveryAct();
  assert(f.shown(), "saved kit brings the sheet back");
  assert.equal(f.focused, "close", "focus leaves System for the enabled sheet control");
  f.context.syncSetupSheet(null, f.summary);
  await new Promise((resolve) => setTimeout(resolve, 1500));
  assert(f.shown(), "step 1 done keeps the sheet open with the next steps");
  f.focused = "next step";
  f.context.syncSetupSheet(f.summary, f.summary);
  assert.equal(f.focused, "next step", "a routine refresh keeps focus on the chosen next step");
  f.context.openGoal("chat-room");
  f.context.syncSetupSheet(f.summary, f.summary);
  assert(f.classes.has("is-yielded"), "after setup finished, a routine refresh keeps the opened app in front");
});

const press = (target, extra = {}) => ({ button: 0, clientX: 1000, clientY: 60, preventDefault() {},
  target: { closest: (selector) => (selector.includes("button") && target === "button" ? {} : null) }, ...extra });

test("Every goal and the settings step collapse the sheet to its card; clicking the card brings it back", async () => {
  for (const finished of [false, true]) {
    for (const target of ["chat-room", "assistant", "browser", "settings"]) {
      const f = fixture();
      if (finished) f.summary.identity.recovery_readiness.status = "ready";
      if (target === "settings") {
        f.summary.identity.profile_readiness.status = "setup_required";
        await f.context.openRecoveryAct();
      } else {
        f.context.openGoal(target);
      }
      assert.deepEqual(f.opened.map(([id]) => id), [target === "settings" ? "system" : target]);
      assert(f.classes.has("is-yielded") && !f.sheet.hidden, `${target}: the card stays visible, never hidden`);
      assert.equal(f.card.attributes.tabindex, "0", `${target}: the card is reachable by keyboard`);
      f.context.syncSetupSheet(f.summary, f.summary);
      assert(f.classes.has("is-yielded"), `${target}: a routine refresh keeps the app in front`);
      // A drag moves the card and keeps it collapsed.
      f.context.onSetupCardPointerDown(press("card"));
      f.context.onSetupCardPointerMove({ clientX: 700, clientY: 300 });
      f.context.onSetupCardPointerUp({ type: "pointerup" });
      assert(f.classes.has("is-yielded"), `${target}: dragging the card does not expand it`);
      // Its own buttons act; they do not expand it.
      f.context.onSetupCardPointerDown(press("button"));
      f.context.onSetupCardPointerUp({ type: "pointerup" });
      assert(f.classes.has("is-yielded"), `${target}: card buttons keep their own action`);
      f.focused = "app";
      f.context.onSetupCardPointerDown(press("card"));
      f.context.onSetupCardPointerMove({ clientX: 1001, clientY: 61 });
      f.context.onSetupCardPointerUp({ type: "pointerup" });
      assert(f.shown(), `${target}: clicking the card brings the full sheet back`);
      assert.equal(f.card.attributes.tabindex, undefined);
      const recoveryStepReady = target === "settings" ? false : finished;
      assert.equal(f.context.recoveryButton.disabled, recoveryStepReady, `${target}: current step state`);
      assert.equal(f.focused, recoveryStepReady ? "close" : "recovery", `${target}: focus moves into the sheet`);
    }
  }
});

test("Close or Later keeps the sheet closed, also from the card", async () => {
  const f = fixture();
  f.context.openGoal("browser");
  f.context.hideSetupSheet({ restoreFocus: false });
  assert.equal(f.sheet.hidden, true);
  assert.equal(f.context.shellState.shellLayoutState.setupReminderDismissed, true, "the dismissal is saved");
  f.context.syncSetupSheet(f.summary, f.summary);
  f.context.onSetupCardPointerDown(press("card"));
  f.context.onSetupCardPointerUp({ type: "pointerup" });
  assert.equal(f.sheet.hidden, true, "nothing brings a closed sheet back on its own");
  assert(!read("capsules/home-gui/browser/shell-windows.js").includes("windowClosed"), "no app-close tracking");
  assert(read("capsules/home-gui/browser/shell-keyboard.js").includes('!event.target?.closest?.("#setup-sheet")'),
    "Escape from the card reaches the sheet's Close on every Space");
  // Pinned at source: no DOM library or browser is available to the behaviour suite.
  assert(/event\.target === card\s*&& sheet\.classList\.contains\("is-yielded"\)\) \{[^}]*event\.stopPropagation\(\);\s*returnSetupSheet\(\);/
    .test(setup), "Enter on the card opens the sheet only, not the selected desktop item");
  const gui = read("capsules/home-gui/browser/home-gui.js");
  const outside = gui.slice(gui.indexOf("assistantFaceActive() &&"), gui.indexOf("hideAssistantFace();", gui.indexOf("assistantFaceActive() &&")));
  assert(outside.includes('!event.target.closest("#setup-sheet")'), "using the card does not close the Assistant");
});
