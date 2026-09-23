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
  const capsuleNavigates = () => {
    entries.splice(index + 1);
    entries.push(CAPSULE);
    index += 1;
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
    capsuleNavigates,
    open,
    closeByHand,
    get entries() {
      return entries.slice(0, index + 1);
    },
    get layers() {
      return layers;
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

test("closing the last layer by hand leaves our entry; the pop that follows is not treated as back", () => {
  const h = harness();
  h.open();
  h.closeByHand();
  h.flush();
  assert.deepEqual(h.log, ["push", "back"]);
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
});

test("a capsule's own history step above ours: back pops it first, then the shell layer", () => {
  const h = harness();
  h.open();
  h.capsuleNavigates();
  assert.deepEqual(h.entries, [HOST, STAGE_HISTORY_STATE, CAPSULE]);
  // The first back is the capsule's own (its document steps back); the shell
  // sees nothing and the window stays.
  h.userBack();
  assert.deepEqual(h.log, ["push"]);
  assert.equal(h.layers, 1);
  assert.deepEqual(h.entries, [HOST, STAGE_HISTORY_STATE]);
  // The next back pops our entry: the window goes home.
  h.userBack();
  assert.deepEqual(h.log, ["push", "onBack"]);
  assert.equal(h.layers, 0);
  assert.deepEqual(h.entries, [HOST]);
});

test("leaving past a capsule step calls back() once; the leftover entry is consumed by the next close", () => {
  const h = harness();
  h.open();
  h.capsuleNavigates();
  h.closeByHand();
  // That back() only popped the capsule step; our entry is still current and
  // no second back() follows — it could walk out of the host's history.
  h.flush();
  h.history.sync();
  h.history.sync();
  assert.deepEqual(h.entries, [HOST, STAGE_HISTORY_STATE]);
  assert.equal(h.log.filter((step) => step === "back").length, 1);
  // Reopening reuses the leftover entry; closing again consumes it.
  h.open();
  assert.equal(h.log.filter((step) => step === "push").length, 1);
  h.closeByHand();
  h.flush();
  assert.deepEqual(h.entries, [HOST]);
  assert.equal(h.log.filter((step) => step === "back").length, 2);
});

test("only one back() per close cycle while the traversal is in flight", () => {
  const calls = [];
  let layers = 0;
  const history = createStageHistory({
    onOurEntry: () => true,
    layerCount: () => layers,
    pushState: () => {},
    back: () => calls.push("back"),
    onBack: () => 0,
  });
  history.sync();
  history.sync();
  history.sync();
  assert.equal(calls.length, 1);
  assert.equal(history.leaving, true);
  layers = 1;
  history.sync();
  assert.equal(history.leaving, false);
  layers = 0;
  history.sync();
  assert.equal(calls.length, 2);
});

test("a leftover entry surfacing under the user's back with nothing open keeps leaving", () => {
  const h = harness();
  h.open();
  h.capsuleNavigates();
  h.closeByHand();
  h.flush();
  // Bare desktop, our entry beneath the user: back pops it and no layer is touched.
  h.userBack();
  assert.deepEqual(h.entries, [HOST]);
  assert.equal(h.log.includes("onBack"), false);
});

test("a layer opened while a leave is in flight re-arms only once the traversal settles", () => {
  const h = harness();
  h.open();
  h.closeByHand();
  // Traversal still pending: state is ours, so no second push.
  h.open();
  assert.deepEqual(h.log, ["push", "back"]);
  h.flush();
  // The pop arrives with a layer open: treated as back (closes it) — the
  // documented edge; the next sync re-arms for whatever remains.
  assert.deepEqual(h.log, ["push", "back", "onBack"]);
  h.open();
  assert.deepEqual(h.entries, [HOST, STAGE_HISTORY_STATE]);
});
