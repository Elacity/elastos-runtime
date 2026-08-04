import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

import {
  projectRuntimeProxyOnlineState,
} from "./browser-selkies-control-service.mjs";

const controlServiceSource = fs.readFileSync(
  new URL("./browser-selkies-control-service.mjs", import.meta.url),
  "utf8",
);

test("Runtime online projection returns a bounded verified state", async () => {
  const calls = [];
  const cdp = {
    async request(method, params) {
      calls.push({ method, params });
      if (method === "Runtime.evaluate") {
        return {
          result: {
            value: JSON.stringify({
              online: true,
              connection_type: "other",
              effective_type: "4g",
            }),
          },
        };
      }
      return {};
    },
  };
  const state = await projectRuntimeProxyOnlineState(
    cdp,
    new URL("http://127.0.0.1:19094/"),
  );
  assert.deepEqual(state, {
    online: true,
    connection_type: "other",
    effective_type: "4g",
  });
  assert.deepEqual(calls, [
    { method: "Network.enable", params: undefined },
    {
      method: "Network.overrideNetworkState",
      params: {
        offline: false,
        latency: 0,
        downloadThroughput: -1,
        uploadThroughput: -1,
        connectionType: "other",
      },
    },
    {
      method: "Runtime.evaluate",
      params: {
        expression: `JSON.stringify({
      online: navigator.onLine === true,
      connection_type: String(navigator.connection?.type || ""),
      effective_type: String(navigator.connection?.effectiveType || "")
    })`,
        returnByValue: true,
      },
    },
  ]);
});

test("online projection brackets initial navigation and follows later navigation", () => {
  const beforeProjection = controlServiceSource.indexOf(
    '"before_initial_navigation"',
  );
  const navigation = controlServiceSource.indexOf(
    "const navigation = await navigateInitialBrowserPage(cdp, browserControl, url);",
    beforeProjection,
  );
  const afterProjection = controlServiceSource.indexOf(
    '"after_initial_navigation"',
    navigation,
  );
  assert.ok(
    beforeProjection >= 0 &&
      navigation > beforeProjection &&
      afterProjection > navigation,
  );
  assert.match(controlServiceSource, /`after_\$\{command\}_navigation`/);
  const replacementStart = controlServiceSource.indexOf(
    "async function replaceBrowserPageTarget(",
  );
  const replacementOpen = controlServiceSource.indexOf(
    "const replacement = await openBrowserPage(",
    replacementStart,
  );
  const exactReplacement = controlServiceSource.indexOf(
    "{ forceNewTarget: true }",
    replacementOpen,
  );
  assert.ok(
    replacementStart >= 0 &&
      replacementOpen > replacementStart &&
      exactReplacement > replacementOpen,
    "exact target replacement must re-enter the projected open path",
  );
});

test("unsupported or ineffective online projection fails closed", async () => {
  await assert.rejects(
    projectRuntimeProxyOnlineState(
      { async request() { return {}; } },
      null,
    ),
    /Runtime proxy is required/,
  );
  await assert.rejects(
    projectRuntimeProxyOnlineState(
      {
        async request(method) {
          if (method === "Network.overrideNetworkState") {
            throw new Error("Method not found");
          }
          return {};
        },
      },
      new URL("http://127.0.0.1:19094/"),
    ),
    /Method not found/,
  );
  await assert.rejects(
    projectRuntimeProxyOnlineState(
      {
        async request(method) {
          return method === "Runtime.evaluate"
            ? { result: { value: JSON.stringify({ online: false }) } }
            : {};
        },
      },
      new URL("http://127.0.0.1:19094/"),
    ),
    /did not accept/,
  );
});
