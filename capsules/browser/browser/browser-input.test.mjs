import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import test from "node:test";

const sourceRef = process.env.BROWSER_INPUT_COORDINATE_SOURCE_REF;
const source = sourceRef
  ? execFileSync("git", ["show", `${sourceRef}:capsules/browser/browser/browser-input.js`],
    { cwd: new URL("../../../", import.meta.url), encoding: "utf8" })
  : readFileSync(new URL("./browser-input.js", import.meta.url), "utf8");
const { selkiesMessagesForInput } = await import(`data:text/javascript,${encodeURIComponent(source)}`);
const view = { width: 1280, height: 720 };

for (const [label, point, expected] of [
  ["top-left corner", { x: 0, y: 0 }, [0, 0]],
  ["left edge", { x: 0, y: 37 }, [0, 37]],
  ["top edge", { x: 19, y: 0 }, [19, 0]],
  ["zero x and omitted y", { x: 0 }, [0, 360]],
  ["omitted x and zero y", { y: 0 }, [640, 0]],
  ["ordinary fractions", { x: 19.6, y: 37.4 }, [20, 37]],
  ["omitted coordinates", {}, [640, 360]],
  ["null coordinates", { x: null, y: null }, [640, 360]],
  ["explicit undefined", { x: undefined, y: undefined }, [640, 360]],
  ["false fallback", { x: false, y: false }, [640, 360]],
  ["empty-string fallback", { x: "", y: "" }, [640, 360]],
  ["NaN fallback", { x: NaN, y: NaN }, [640, 360]],
  ["numeric-string coercion", { x: "0", y: "37.4" }, [0, 37]],
]) {
  test(`wheel coordinates preserve ${label}`, t => {
    if (label === "top-left corner") {
      t.diagnostic(`actual encoder SHA-256 ${createHash("sha256").update(source).digest("hex")}`);
    }
    const [x, y] = expected;
    assert.deepEqual(selkiesMessagesForInput({ type: "wheel", ...point, delta_y: 80 }, view),
      [`m,${x},${y},8,1`, `m,${x},${y},0,0`]);
  });
}

test("omitted view keeps the existing default center while zero remains an edge", () => {
  assert.deepEqual(selkiesMessagesForInput({ type: "wheel", delta_y: 80 }),
    ["m,640,360,8,1", "m,640,360,0,0"]);
  assert.deepEqual(selkiesMessagesForInput({ type: "wheel", x: 0, y: 0, delta_y: 80 }),
    ["m,0,0,8,1", "m,0,0,0,0"]);
});

test("custom view center is used only for missing coordinates", () => {
  assert.deepEqual(selkiesMessagesForInput({ type: "wheel", x: 0, y: null, delta_y: 80 },
    { width: 1920, height: 1080 }), ["m,0,540,8,1", "m,0,540,0,0"]);
});

for (const [delta, mask, magnitude] of [[1, 8, 1], [-1, 16, 1], [80, 8, 1], [-80, 16, 1],
  [81, 8, 2], [-81, 16, 2], [2000, 8, 12], [-2000, 16, 12]]) {
  test(`wheel delta ${delta} keeps its direction, quantization and release`, () => {
    assert.deepEqual(selkiesMessagesForInput({ type: "wheel", x: 19, y: 37, delta_x: 123, delta_y: delta }, view),
      [`m,19,37,${mask},${magnitude}`, "m,19,37,0,0"]);
  });
}

test("zero vertical delta and horizontal-only input keep their existing empty encoding", () => {
  for (const event of [{ delta_y: 0 }, { delta_x: 80, delta_y: 0 }, { delta_x: 80 }]) {
    assert.deepEqual(selkiesMessagesForInput({ type: "wheel", x: 0, y: 0, ...event }, view), []);
  }
});
