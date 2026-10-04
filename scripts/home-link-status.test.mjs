import assert from "node:assert/strict";
import test from "node:test";

import {
  applyHomeLinkStatus,
  HOME_LINK_RECONNECTING,
} from "../capsules/home-gui/browser/shell-link-status.js";
import {
  createHomeLinkStatus,
  isGatewayUnreachable,
  LINK_RETRY_MS,
  LINK_STATUS_MESSAGE,
} from "../capsules/home/browser/home-link-status.js";

function httpError(status) {
  const error = new Error(`request failed: ${status}`);
  error.status = status;
  return error;
}

function harness() {
  const posted = [];
  const timers = new Map();
  let nextTimer = 1;
  let retries = 0;
  const link = createHomeLinkStatus({
    post: (message) => posted.push(message),
    retry: () => {
      retries += 1;
    },
    setTimer: (callback, ms) => {
      const id = nextTimer;
      nextTimer += 1;
      timers.set(id, { callback, ms });
      return id;
    },
    clearTimer: (id) => timers.delete(id),
  });
  const fireTimers = () => {
    const pending = [...timers.values()];
    timers.clear();
    pending.forEach(({ callback }) => callback());
  };
  return { link, posted, timers, fireTimers, retries: () => retries };
}

test("only a request that never reached the gateway, or a proxy saying it is gone, counts", () => {
  assert.equal(isGatewayUnreachable(new TypeError("Failed to fetch")), true);
  assert.equal(isGatewayUnreachable(new TypeError("Load failed")), true);
  for (const status of [502, 503, 504]) {
    assert.equal(isGatewayUnreachable(httpError(status)), true, String(status));
  }
  for (const status of [400, 401, 403, 404, 409, 500]) {
    assert.equal(isGatewayUnreachable(httpError(status)), false, String(status));
  }
  assert.equal(isGatewayUnreachable(new SyntaxError("bad json")), false);
  assert.equal(isGatewayUnreachable(null), false);
});

test("the first unreachable failure tells the shell once and schedules a retry", () => {
  const { link, posted, timers } = harness();
  assert.equal(link.reportFailure(new TypeError("Failed to fetch")), true);
  assert.equal(link.reportFailure(new TypeError("Failed to fetch")), true);
  assert.deepEqual(posted, [{ type: LINK_STATUS_MESSAGE, reachable: false }]);
  assert.equal(link.reachable(), false);
  assert.equal(timers.size, 1, "a repeat failure replaces the retry, never stacks one");
  assert.equal([...timers.values()][0].ms, LINK_RETRY_MS);
});

test("the retry asks the gateway again until an answer clears the state", () => {
  const { link, posted, timers, fireTimers, retries } = harness();
  link.reportFailure(httpError(503));
  fireTimers();
  assert.equal(retries(), 1);
  link.reportFailure(new TypeError("Failed to fetch"));
  link.reportSuccess();
  assert.equal(timers.size, 0, "an answer cancels the pending retry");
  assert.deepEqual(posted.at(-1), { type: LINK_STATUS_MESSAGE, reachable: true });
  assert.equal(link.reachable(), true);
});

test("an ordinary HTTP answer after an outage clears the state and leaves the error to the caller", () => {
  for (const status of [500, 401, 404]) {
    const { link, posted, timers } = harness();
    link.reportFailure(httpError(503));
    assert.equal(link.reportFailure(httpError(status)), false, String(status));
    assert.equal(link.reachable(), true, String(status));
    assert.deepEqual(posted, [
      { type: LINK_STATUS_MESSAGE, reachable: false },
      { type: LINK_STATUS_MESSAGE, reachable: true },
    ]);
    assert.equal(timers.size, 0, "the answer cancels the pending retry");
  }
});

test("answers and ordinary refusals while reachable say nothing", () => {
  const { link, posted, timers } = harness();
  link.reportSuccess();
  assert.equal(link.reportFailure(httpError(401)), false);
  assert.equal(link.reportFailure(httpError(500)), false);
  assert.deepEqual(posted, []);
  assert.equal(timers.size, 0);
});

test("a reloaded shell hears the state again only while the link is down", () => {
  const { link, posted } = harness();
  link.replay();
  assert.deepEqual(posted, []);
  link.reportFailure(new TypeError("Failed to fetch"));
  link.replay();
  assert.deepEqual(posted, [
    { type: LINK_STATUS_MESSAGE, reachable: false },
    { type: LINK_STATUS_MESSAGE, reachable: false },
  ]);
});

function fakeShellDocument() {
  const bar = { textContent: "" };
  return {
    body: { dataset: {} },
    querySelector: (selector) => (selector === "#toolbar-link-status-text" ? bar : null),
    bar,
  };
}

test("the shell keys the bar and Notification Centre on body[data-home-link]", () => {
  const doc = fakeShellDocument();
  applyHomeLinkStatus({ reachable: false }, doc);
  assert.equal(doc.body.dataset.homeLink, HOME_LINK_RECONNECTING);
  assert.equal(doc.bar.textContent, "Reconnecting…");
  applyHomeLinkStatus({ reachable: true }, doc);
  assert.equal(doc.body.dataset.homeLink, undefined);
  assert.equal(doc.bar.textContent, "");
});

test("the shell ignores anything but a boolean reachable", () => {
  const doc = fakeShellDocument();
  applyHomeLinkStatus({ reachable: false }, doc);
  applyHomeLinkStatus({ reachable: "false" }, doc);
  applyHomeLinkStatus({}, doc);
  applyHomeLinkStatus(null, doc);
  assert.equal(doc.body.dataset.homeLink, HOME_LINK_RECONNECTING);
  assert.equal(doc.bar.textContent, "Reconnecting…");
});
