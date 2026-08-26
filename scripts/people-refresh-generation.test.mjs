#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";

const peopleSource = fs.readFileSync(
  new URL("../capsules/people/browser/people.js", import.meta.url),
  "utf8",
);

function extractFunction(source, name) {
  const start = source.indexOf(`async function ${name}(`);
  assert.notEqual(start, -1, `${name} function not found`);
  const parametersOpen = source.indexOf("(", start);
  let parameterDepth = 0;
  let parametersClose = -1;
  for (let index = parametersOpen; index < source.length; index += 1) {
    if (source[index] === "(") parameterDepth += 1;
    if (source[index] === ")") parameterDepth -= 1;
    if (parameterDepth === 0) {
      parametersClose = index;
      break;
    }
  }
  const open = source.indexOf("{", parametersClose);
  let depth = 0;
  for (let index = open; index < source.length; index += 1) {
    if (source[index] === "{") depth += 1;
    if (source[index] === "}") depth -= 1;
    if (depth === 0) return source.slice(start, index + 1);
  }
  throw new Error(`${name} function body is not balanced`);
}

test("a stale People summary cannot replace a newer normalized summary", async () => {
  assert.match(peopleSource, /let refreshGeneration = 0;/);
  const pending = [];
  const renderedRequestCounts = [];
  const context = vm.createContext({
    refreshGeneration: 0,
    setBusy() {},
    fetchJson() {
      return new Promise((resolve) => pending.push(resolve));
    },
    renderSummary(summary) {
      renderedRequestCounts.push(summary.discovery.request_count);
    },
  });
  vm.runInContext(
    `${extractFunction(peopleSource, "refreshPeople")}\nthis.refreshPeople = refreshPeople;`,
    context,
  );

  const staleRefresh = context.refreshPeople({ quiet: true });
  const normalizedRefresh = context.refreshPeople({ quiet: true });
  assert.equal(pending.length, 2);

  pending[1]({ discovery: { request_count: 0 } });
  await normalizedRefresh;
  assert.deepEqual(renderedRequestCounts, [0]);

  pending[0]({ discovery: { request_count: 2 } });
  await staleRefresh;
  assert.deepEqual(renderedRequestCounts, [0]);
});
