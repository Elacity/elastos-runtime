import assert from "node:assert/strict";
import test from "node:test";

import {
  STAGE_HISTORY_STATE,
  createStageHistory,
  isStageHistoryState,
} from "../capsules/home-gui/browser/shell-phone-stage-history.js?v=home-20260813a";

const HOST = "host";
const CAPSULE = "capsule";

// A joint session history: a stack of entries where traversal is
// asynchronous (flush() delivers it) and the shell only sees popstate when
// its own document's entry changes, as browsers do for frames.
function harness() {
  const entries = [HOST];
  let index = 0;
  let pending = 0;
  let leftHome = false;
  const log = [];
  let layers = 0;
  const stage = {
    // What the stage would report after the top layer closes.
    onBackResult: null,
  };
  const ownEntry = (entry) => entry === STAGE_HISTORY_STATE;
  // The shell document's own current entry: capsule steps above it belong to
  // another document, so history.state still reads ours beneath them.
  const shellEntryAt = (at) => {
    for (let i = at; i >= 0; i -= 1) {
      if (entries[i] !== CAPSULE) {
        return entries[i];
      }
    }
    return HOST;
  };

  const history = createStageHistory({
    onOurEntry: () => ownEntry(shellEntryAt(index)),
    layerCount: () => layers,
    pushState: () => {
      entries.splice(index + 1);
      entries.push(STAGE_HISTORY_STATE);
      index += 1;
      log.push("push");
    },
    back: () => {
      pending += 1;
      log.push("back");
    },
    onBack: () => {
      log.push("onBack");
      layers = stage.onBackResult ?? Math.max(0, layers - 1);
      stage.onBackResult = null;
      return layers;
    },
  });

  // Deliver queued traversals; popstate reaches the shell only when its own
  // document's entry changed (from or to one of ours).
  const flush = () => {
    while (pending > 0) {
      pending -= 1;
      if (index === 0) {
        leftHome = true;
        continue;
      }
      const from = shellEntryAt(index);
      index -= 1;
      if (shellEntryAt(index) !== from) {
        history.handlePopState();
      }
    }
  };
  const userBack = () => {
    pending += 1;
    flush();
  };
  const open = () => {
    layers += 1;
    history.sync();
  };
  const closeByHand = () => {
    layers = Math.max(0, layers - 1);
    history.sync();
  };
  return {
    history,
    log,
    stage,
    flush,
    userBack,
    open,
    closeByHand,
    get entries() {
      return entries.slice(0, index + 1);
    },
    get layers() {
      return layers;
    },
    get leftHome() {
      return leftHome;
    },
  };
}

test("isStageHistoryState accepts only our tagged object", () => {
  assert.equal(isStageHistoryState(STAGE_HISTORY_STATE), true);
  assert.equal(isStageHistoryState({ elastosStage: true }), true);
  assert.equal(isStageHistoryState(null), false);
  assert.equal(isStageHistoryState({ elastosStage: "yes" }), false);
});

test("first layer pushes one entry; more layers push nothing", () => {
  const h = harness();
  h.open();
  h.open();
  h.open();
  assert.deepEqual(h.log, ["push"]);
  assert.deepEqual(h.entries, [HOST, STAGE_HISTORY_STATE]);
});

test("bare desktop never pushes and ignores stray pops", () => {
  const h = harness();
  h.history.sync();
  assert.equal(h.history.handlePopState(), false);
  assert.deepEqual(h.log, []);
  assert.deepEqual(h.entries, [HOST]);
});

test("closing the last layer by hand leaves our entry and calls no back()", () => {
  const h = harness();
  h.open();
  h.closeByHand();
  h.flush();
  assert.deepEqual(h.log, ["push"]);
  assert.deepEqual(h.entries, [HOST, STAGE_HISTORY_STATE]);
});

test("Back off a leftover entry with nothing open finishes leaving Home in one press", () => {
  const h = harness();
  h.open();
  h.closeByHand();
  h.userBack();
  assert.deepEqual(h.log, ["push", "back"]);
  assert.equal(h.leftHome, true);
  assert.equal(h.log.includes("onBack"), false);
});

test("reopening after a close by hand reuses the leftover entry", () => {
  const h = harness();
  h.open();
  h.closeByHand();
  h.open();
  assert.deepEqual(h.log, ["push"]);
  h.userBack();
  assert.deepEqual(h.log, ["push", "onBack"]);
  assert.equal(h.layers, 0);
  assert.equal(h.leftHome, false, "Back closed the layer and nothing else");
  assert.deepEqual(h.entries, [HOST]);
});

test("system back closes the top layer and re-arms while layers remain, then goes home", () => {
  const h = harness();
  h.open();
  h.open();
  h.userBack();
  assert.deepEqual(h.log, ["push", "onBack", "push"]);
  assert.equal(h.layers, 1);
  assert.deepEqual(h.entries, [HOST, STAGE_HISTORY_STATE]);
  h.userBack();
  assert.deepEqual(h.log, ["push", "onBack", "push", "onBack"]);
  assert.equal(h.layers, 0);
  assert.deepEqual(h.entries, [HOST]);
  // Bare Home now: the next Back is the user's own way out, untouched.
  h.userBack();
  assert.equal(h.leftHome, true);
  assert.equal(h.log.filter((step) => step === "back").length, 0);
});

test("every pop is one user Back: close, reopen, Back at once closes the reopened layer", () => {
  const h = harness();
  h.open();
  h.userBack();
  h.open();
  h.userBack();
  assert.deepEqual(h.log, ["push", "onBack", "push", "onBack"]);
  assert.equal(h.layers, 0);
  assert.equal(h.leftHome, false);
  assert.deepEqual(h.entries, [HOST]);
});

test("off a phone (no layers counted) the shell neither pushes nor calls back()", () => {
  const calls = [];
  let ours = true;
  const history = createStageHistory({
    onOurEntry: () => ours,
    layerCount: () => 0,
    pushState: () => calls.push("push"),
    back: () => calls.push("back"),
    onBack: () => 0,
  });
  history.sync();
  history.sync();
  assert.deepEqual(calls, []);
  // The user's own Back off our leftover entry still leaves in one press.
  ours = false;
  assert.equal(history.handlePopState(), false);
  assert.deepEqual(calls, ["back"]);
});
