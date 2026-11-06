import assert from "node:assert/strict";
import test from "node:test";
import { stopInstalledModelRun } from "./ci-installed-home-model-stop.mjs";

function deferred() {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
}

function fixture({ holdSettlement = false, status = "cancelled" } = {}) {
  const calls = [], terminal = deferred(), cancelled = deferred(), clicked = deferred();
  let settled;
  const settle = value => { settled ??= value; terminal.resolve({ status: settled }); };
  const owner = { run: { run_id: "run:fixture", status: "running", terminal: null }, terminal: terminal.promise };
  const response = (frame, runId) => ({
    url: () => "http://fixture/api/provider/model/runs_cancel", ok: () => true,
    request: () => ({ frame: () => frame, postDataJSON: () => ({ run_id: runId }) }),
  });
  const assistant = {
    page: () => ({ waitForResponse: predicate => {
      calls.push("observe_cancel");
      assert.equal(predicate(response({}, owner.run.run_id)), false, "another frame cannot settle this Stop");
      assert.equal(predicate(response(assistant, "run:other")), false, "another run cannot settle this Stop");
      assert.equal(predicate(response(assistant, owner.run.run_id)), true);
      return cancelled.promise;
    } }),
    // A short reply can deliver text and a terminal in the same events page.
    waitForFunction: async () => { calls.push("visible_text"); settle("completed"); },
    locator: selector => ({
      click: async () => {
        assert.equal(selector, '#agent-composer-send[data-mode="stop"]');
        calls.push("click_stop");
        if (!holdSettlement) settle(status); // A completed run keeps its terminal result.
        cancelled.resolve(response(assistant, owner.run.run_id));
        clicked.resolve();
      },
      waitFor: async () => {
        assert.equal(selector, '#agent-composer-send[data-mode="send"]');
        assert.equal(settled, "cancelled");
        calls.push("assistant_settled");
      },
    }),
  };
  return {
    assistant, owner, calls, clicked: clicked.promise, settle,
    startTurn: async () => { calls.push("accepted"); return owner; },
    resourceAction: async action => {
      calls.push(action);
      if (action === "engine_stopped") assert.equal(settled, "cancelled");
      return { whole_engine_exit: true, runtime_unchanged: true };
    },
  };
}

test("Stop cancels an accepted run when visible text may already be terminal", async () => {
  const app = fixture();
  assert.equal(await stopInstalledModelRun(app), app.owner);
  assert.deepEqual(app.calls, ["engine_running", "accepted", "observe_cancel", "click_stop", "engine_stopped", "assistant_settled"]);
});

test("Stop waits for Runtime's terminal and Assistant's settled control", async () => {
  const app = fixture({ holdSettlement: true });
  const stopping = stopInstalledModelRun(app);
  stopping.catch(() => {});
  await app.clicked;
  assert(!app.calls.includes("engine_stopped"));
  assert(!app.calls.includes("assistant_settled"));
  app.settle("cancelled");
  await stopping;
  assert.deepEqual(app.calls.slice(-2), ["engine_stopped", "assistant_settled"]);
});

test("a successful cancel response still requires the exact cancelled terminal", async () => {
  const app = fixture({ status: "failed" });
  await assert.rejects(stopInstalledModelRun(app), error => error.journeyAssertion === "stop_terminal_status"
    && error.journeyActual === "failed" && error.journeyExpected === "cancelled");
  assert(!app.calls.includes("assistant_settled"));
});
