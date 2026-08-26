#!/usr/bin/env node

import assert from "node:assert/strict";
import vm from "node:vm";
import { readFileSync } from "node:fs";

const source = readFileSync(
  new URL("../capsules/assistant/browser/assistant.js", import.meta.url),
  "utf8",
);

async function runScenario({ fetchResult, fetchError = null, homeToken = "token-1" }) {
  const statusNode = {
    hidden: false,
    textContent: "",
  };
  const posts = [];
  const fetchCalls = [];
  const context = {
    URLSearchParams,
    fetch: async (...args) => {
      fetchCalls.push(args);
      if (fetchError) {
        throw fetchError;
      }
      return fetchResult;
    },
    document: {
      getElementById(id) {
        assert.equal(id, "assistant-status");
        return statusNode;
      },
    },
    window: {
      location: {
        hash: `#home_token=${encodeURIComponent(homeToken)}&home_origin=${encodeURIComponent("null")}`,
      },
      top: {
        postMessage(message, origin) {
          posts.push({ message, origin });
        },
      },
    },
    console,
    setTimeout,
    clearTimeout,
  };
  vm.runInNewContext(source, context, { filename: "assistant.js" });
  await new Promise((resolve) => setImmediate(resolve));
  return { statusNode, posts, fetchCalls };
}

{
  const { statusNode, posts, fetchCalls } = await runScenario({
    fetchResult: {
      ok: true,
      async json() {
        return { status: "ok", offers: [] };
      },
    },
  });
  assert.equal(posts.length, 1);
  assert.equal(posts[0].origin, "null");
  assert.equal(posts[0].message.type, "home:app-ready");
  assert.equal(posts[0].message.homeToken, "token-1");
  assert.equal(fetchCalls.length, 1);
  assert.equal(fetchCalls[0][0], "/api/provider/model/offers_list");
  assert.equal(statusNode.textContent, "No model offers available.");
  assert.equal(statusNode.hidden, false);
}

{
  const { statusNode, fetchCalls } = await runScenario({
    fetchResult: {
      ok: false,
      async json() {
        return { status: "error", code: "provider_error" };
      },
    },
  });
  assert.equal(fetchCalls.length, 1);
  assert.equal(fetchCalls[0][0], "/api/provider/model/offers_list");
  assert.equal(statusNode.textContent, "Model provider unavailable.");
  assert.equal(statusNode.hidden, false);
}
