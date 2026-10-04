#!/usr/bin/env node
/* The Assistant mark's contract: still until a real activation; the chevrons
   close and return to rest in full, and only then does the face open; repeat
   clicks are ignored until the sequence settles; reduced motion and
   non-opening activations skip the motion. */

import assert from "node:assert/strict";
import test from "node:test";

const { bindAssistantMark, ASSISTANT_MARK_CONTACT_MS, ASSISTANT_MARK_RETURN_MS } = await import(
  new URL("../capsules/home-gui/browser/shell-assistant-mark.js", import.meta.url)
);

function animatable(log, name) {
  return {
    animate(frames, options) {
      let settle;
      const animation = {
        cancelled: false,
        finished: new Promise((resolve) => {
          settle = resolve;
        }),
        cancel() { this.cancelled = true; },
      };
      log.push({ name, frames, options, animation, settle: () => settle(animation) });
      return animation;
    },
  };
}

function fakeArt(log) {
  const layers = { glass: animatable(log, "glass"), orange: animatable(log, "orange") };
  return {
    querySelector(selector) {
      return layers[/data-assistant-layer="(\w+)"/.exec(selector)?.[1]] || null;
    },
  };
}

function fakeButton(log, { withArt = true } = {}) {
  const art = withArt ? fakeArt(log) : null;
  const listeners = new Map();
  return {
    art,
    dataset: {},
    querySelector(selector) { return selector === ".assistant-mark" ? art : null; },
    addEventListener(type, handler) { listeners.set(type, handler); },
    removeEventListener(type) { listeners.delete(type); },
    click() { return listeners.get("click")?.({ preventDefault() {} }); },
    hasClickListener() { return listeners.has("click"); },
  };
}

async function settleAll(log) {
  const pending = log.splice(0);
  pending.forEach((entry) => entry.settle());
  await new Promise((resolve) => setTimeout(resolve, 0));
  return pending;
}

test("the chevrons close, return to rest, and only then does the face open", async () => {
  const log = [];
  const button = fakeButton(log);
  let activations = 0;
  bindAssistantMark(button, { onActivate: () => { activations += 1; }, reducedMotion: () => false });

  const sequence = button.click();
  assert.equal(activations, 0, "the face waits for the whole contact");
  assert.equal(button.dataset.animating, "true");

  assert.equal(log.length, 2, "both chevrons move together");
  const [glassClose, orangeClose] = log;
  assert.equal(glassClose.options.duration, ASSISTANT_MARK_CONTACT_MS);
  assert.equal(glassClose.options.easing, "cubic-bezier(0.32, 0, 0.18, 1)");
  assert.equal(glassClose.frames[1].transform, "translate3d(0, 10.35%, 0)");
  assert.equal(orangeClose.frames[1].transform, "translate3d(0, -10.35%, 0)", "equal and opposite travel");

  const close = await settleAll(log);
  assert.equal(activations, 0, "meeting is not yet opening");
  assert.equal(log.length, 2, "the return starts only once the chevrons have met");
  const [glassBack, orangeBack] = log;
  assert.equal(glassBack.options.duration, ASSISTANT_MARK_RETURN_MS);
  assert.equal(glassBack.options.easing, "cubic-bezier(0.2, 0, 0, 1)");
  assert.equal(glassBack.frames[0].transform, "translate3d(0, 10.35%, 0)", "the return picks up where the close left off");
  assert.equal(glassBack.frames[1].transform, "translate3d(0, 0, 0)");
  assert.equal(orangeBack.frames[0].transform, "translate3d(0, -10.35%, 0)");

  const back = await settleAll(log);
  await sequence;
  assert.equal(activations, 1, "the face opens once the mark is back at rest");
  assert.equal(button.dataset.animating, undefined);
  assert.ok([...close, ...back].every((entry) => entry.animation.cancelled), "filled transforms are released at rest");
});

test("repeat clicks are ignored until the sequence settles", async () => {
  const log = [];
  const button = fakeButton(log);
  let activations = 0;
  bindAssistantMark(button, { onActivate: () => { activations += 1; }, reducedMotion: () => false });

  const first = button.click();
  button.click();
  button.click();
  assert.equal(log.length, 2, "no second sequence started");
  await settleAll(log);
  await settleAll(log);
  await first;
  assert.equal(activations, 1, "the repeats did not open the face a second time");

  const second = button.click();
  await settleAll(log);
  await settleAll(log);
  await second;
  assert.equal(activations, 2, "a new activation is accepted once settled");
});

test("reduced motion opens immediately without motion", () => {
  const log = [];
  const button = fakeButton(log);
  let activations = 0;
  bindAssistantMark(button, { onActivate: () => { activations += 1; }, reducedMotion: () => true });
  button.click();
  assert.equal(activations, 1);
  assert.equal(log.length, 0);
  assert.equal(button.dataset.animating, undefined);
});

test("a non-opening activation skips the contact", () => {
  const log = [];
  const button = fakeButton(log);
  let activations = 0;
  bindAssistantMark(button, {
    onActivate: () => { activations += 1; },
    animate: () => false,
    reducedMotion: () => false,
  });
  button.click();
  assert.equal(activations, 1);
  assert.equal(log.length, 0);
});

test("a toggle without the mark still opens; unbind removes the handler; null is a no-op", () => {
  const bare = fakeButton([], { withArt: false });
  let activations = 0;
  const unbind = bindAssistantMark(bare, { onActivate: () => { activations += 1; }, reducedMotion: () => false });
  bare.click();
  assert.equal(activations, 1);
  assert.equal(bare.hasClickListener(), true);
  unbind();
  assert.equal(bare.hasClickListener(), false);
  assert.equal(typeof bindAssistantMark(null, { onActivate() {} }), "function");
});
