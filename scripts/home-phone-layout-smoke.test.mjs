import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

// Exercise the canonical report gate without launching a browser or importing
// the smoke's top-level fixture server. There is one production gate.
const source = readFileSync(new URL("./home-phone-layout-smoke.mjs", import.meta.url), "utf8");
const from = source.indexOf("function shellFailures(run) {");
const to = source.indexOf("\nfunction summarize(run)", from);
assert.ok(from >= 0 && to > from);
const context = vm.createContext({
  BASELINE: { phone: { window: { targets: 1, text: 0 } }, tablet: {} },
  CAPSULE_TARGET_BASELINE: { phone: { "chat-room": 0, people: 0 } },
  EXPECTED_CAPSULE_FORM_FACTOR: { phone: "phone", tablet: "tablet" },
  MIN_TARGET_PX: 44,
  MIN_TEXT_PX: 12,
});
vm.runInContext(source.slice(from, to), context);
const gate = (surfaces, profile = "phone") => Array.from(context.shellFailures({ engine: "fixture", profile, surfaces }));
const surface = (target = "chat-room", overrides = {}) => ({
  surface: "window", target, overflow: false, smallTargets: [], smallText: [],
  capsule: { smallTargets: [] }, capsuleLayout: { sharedTheme: true, formFactor: "phone" },
  ...overrides,
});
const withPeople = (chat) => [chat, surface("people")];

for (const [label, capsule] of [["null", null], ["absent", undefined], ["empty object", {}], ["non-array", { smallTargets: {} }]]) {
  test(`a ${label} capsule measurement fails rather than counting as zero`, () => {
    assert.match(gate(withPeople(surface("chat-room", { capsule })))[0], /measurement unavailable/);
  });
}

test("missing capsule layout fails", () => {
  assert.match(gate(withPeople(surface("chat-room", { capsuleLayout: null })))[0], /measurement unavailable/);
});
test("malformed capsule layout fails", () => {
  assert.match(gate(withPeople(surface("chat-room", { capsuleLayout: {} })))[0], /measurement unavailable/);
});
test("each baselined capsule must be measured", () => {
  assert.match(gate([surface("people")])[0], /chat-room: capsule measurement missing/);
});
test("an empty run cannot establish capsule acceptance", () => {
  assert.equal(gate([]).length, 2);
});
test("a measured zero-target result passes", () => {
  assert.deepEqual(gate(withPeople(surface())), []);
});
test("a real target regression and form-factor mismatch fail", () => {
  assert.match(gate(withPeople(surface("chat-room", { capsule: { smallTargets: [{}] } })))[0], /1 capsule targets/);
  assert.match(gate(withPeople(surface("chat-room", { capsuleLayout: { sharedTheme: true, formFactor: "desktop" } })))[0], /form factor desktop/);
});
test("tablet measurements retain their recorded-only capsule policy", () => {
  assert.deepEqual(gate([surface("chat-room", { capsule: null, capsuleLayout: null })], "tablet"), []);
});

test("the real phone baselines cover Chat and every app fixture", () => {
  const real = vm.createContext({});
  const apps = source.slice(source.indexOf("const FIRST_PARTY_APPS ="), source.indexOf("\nconst PROFILES ="));
  const baseline = source.slice(source.indexOf("const CAPSULE_TARGET_BASELINE ="), from);
  vm.runInContext(`${apps}\n${baseline}\nthis.apps = FIRST_PARTY_APPS; this.baseline = CAPSULE_TARGET_BASELINE;`, real);
  const targets = Array.from(real.apps, ([target]) => target).sort();
  assert.ok(targets.includes("chat-room"));
  for (const profile of ["phone-portrait", "phone-landscape"]) {
    assert.deepEqual(Object.keys(real.baseline[profile]).sort(), targets);
  }
});
