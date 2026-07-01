/**
 * Drift guard — the capsule-inspector's browser-shipped ESP projection is byte-identical
 * to the freshly-built source of truth.
 *
 * The vanilla-JS inspector (capsules/capsule-inspector/browser) cannot import from
 * elastos/esp at runtime (it's a standalone browser capsule with no bundler), so it ships
 * a COPY of the compiled projection at browser/esp/spend_audit.js. That copy is the exact
 * same custody-display contract the Svelte <CapsuleCustodyPanel> consumes — the whole point
 * is that both shells paint the three channels identically. This test fails the moment the
 * two diverge (someone edited spend_audit.ts without re-copying, or hand-edited the copy),
 * so a stale fork can never ship silently.
 *
 * `npm test` runs `tsc` before this, so build/spend_audit.js is always the current build.
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const built = new URL("./build/spend_audit.js", import.meta.url);
const shipped = new URL(
  "../../capsules/capsule-inspector/browser/esp/spend_audit.js",
  import.meta.url,
);

test("the inspector's shipped ESP projection matches the freshly-built source of truth", () => {
  const a = readFileSync(built, "utf8");
  const b = readFileSync(shipped, "utf8");
  assert.equal(
    b,
    a,
    "capsules/capsule-inspector/browser/esp/spend_audit.js has drifted from " +
      "elastos/esp/build/spend_audit.js — rebuild ESP (`npm run build`) and re-copy the " +
      "artifact so the inspector and the Svelte panel paint the same custody contract.",
  );
});

test("the shipped projection exports the exact symbols the inspector imports", () => {
  const b = readFileSync(shipped, "utf8");
  // The inspector's `import { homeCustodyView, custodyDisplayRows }` must resolve.
  assert.ok(/export function homeCustodyView\b/.test(b), "homeCustodyView must be exported");
  assert.ok(/export function custodyDisplayRows\b/.test(b), "custodyDisplayRows must be exported");
});
