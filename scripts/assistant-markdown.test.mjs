#!/usr/bin/env node

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import test from "node:test";

globalThis.CustomEvent ??= class CustomEvent extends Event {
  constructor(type, options = {}) {
    super(type, options);
    this.detail = options.detail ?? null;
  }
};
globalThis.window = Object.assign(new EventTarget(), {
  location: { href: "http://home.invalid/apps/assistant/", hash: "#home_token=fixture" },
  setTimeout, clearTimeout, setInterval, clearInterval,
});
const { renderMarkdown } = await import("../capsules/assistant/browser/agent-stream.js");

const count = (html, pattern) => (html.match(pattern) || []).length;

test("numbered items separated by blank lines stay one list", () => {
  const html = renderMarkdown("1. A\n\n2. B\n\n3. C");
  assert.equal(html, '<ol class="agent-md-list"><li>A</li><li>B</li><li>C</li></ol>');
});

test("a sentence under a numbered item stays inside that item", () => {
  const html = renderMarkdown("1. Fish\nsentence one\n2. Dog\nsentence two");
  assert.equal(
    html,
    '<ol class="agent-md-list"><li>Fish<br>sentence one</li><li>Dog<br>sentence two</li></ol>',
  );
});

test("indented bullets under a numbered item form a nested list", () => {
  const html = renderMarkdown("1. **Step**\n   - sub a\n   - sub b\n2. **Step**");
  assert.equal(
    html,
    '<ol class="agent-md-list"><li><strong>Step</strong><ul class="agent-md-list"><li>sub a</li><li>sub b</li></ul></li>' +
      "<li><strong>Step</strong></li></ol>",
  );
});

test("text between nested lists stays in source order", () => {
  const html = renderMarkdown(
    "1. Deploy\n   - Stop the service.\n\n   Replace its configuration.\n   - Start the service.",
  );
  const stop = html.indexOf("Stop the service.");
  const replace = html.indexOf("Replace its configuration.");
  const start = html.indexOf("Start the service.");
  assert.ok(stop >= 0 && replace >= 0 && start >= 0, html);
  assert.ok(stop < replace && replace < start, html);
  assert.equal(count(html, /<ol /g), 1);
});

test("a contiguous list renders as before", () => {
  assert.equal(renderMarkdown("- a\n- b\n- c"), '<ul class="agent-md-list"><li>a</li><li>b</li><li>c</li></ul>');
  assert.equal(renderMarkdown("1. a\n2. b"), '<ol class="agent-md-list"><li>a</li><li>b</li></ol>');
});

test("an ordered list keeps the model's first number", () => {
  assert.equal(renderMarkdown("3. x"), '<ol class="agent-md-list" start="3"><li>x</li></ol>');
  // A code fence splits the reply; the list after it continues its numbering.
  const html = renderMarkdown("1. one\n```\ncode\n```\n2. two");
  assert.equal(count(html, /<ol class="agent-md-list" start="2">/g), 1);
});

test("list text is still HTML-escaped", () => {
  const html = renderMarkdown('1. <img src=x onerror="alert(1)">\n   <b>under</b>');
  assert.ok(!html.includes("<img"));
  assert.ok(!html.includes("<b>"));
  assert.match(html, /&lt;img src=x onerror=&quot;alert\(1\)&quot;&gt;<br>&lt;b&gt;under&lt;\/b&gt;/);
});

test("a different list kind or an unindented paragraph after a blank ends the list", () => {
  assert.equal(
    renderMarkdown("1. a\n\n- b"),
    '<ol class="agent-md-list"><li>a</li></ol><ul class="agent-md-list"><li>b</li></ul>',
  );
  assert.equal(
    renderMarkdown("- a\n\nAfter."),
    '<ul class="agent-md-list"><li>a</li></ul><p class="agent-md-p">After.</p>',
  );
});

// Render in a child process with a deadline, so a renderer that stops
// advancing fails the test instead of hanging the suite.
function renderBounded(text) {
  const script =
    'globalThis.window = Object.assign(new EventTarget(), { location: { href: "http://home.invalid/", hash: "" }, setTimeout, clearTimeout, setInterval, clearInterval });' +
    `const { renderMarkdown } = await import(${JSON.stringify(new URL("../capsules/assistant/browser/agent-stream.js", import.meta.url).href)});` +
    "process.stdout.write(renderMarkdown(process.argv[1]));";
  return execFileSync(process.execPath, ["--input-type=module", "-e", script, text], { encoding: "utf8", timeout: 5000 });
}

test("a pipe table with a dashed separator renders as a table", () => {
  const html = renderBounded("| a | b |\n|---|---|\n| 1 | 2 |");
  assert.match(html, /<thead><tr><th>a<\/th><th>b<\/th><\/tr><\/thead><tbody><tr><td>1<\/td><td>2<\/td><\/tr><\/tbody>/);
  assert.match(renderBounded("| a | b |\n| :--- | ---: |\n| 1 | 2 |"), /<th>a<\/th>/);
});

test("lines with a stray pipe or an unclosed display-math opener still render", () => {
  assert.equal(renderBounded("cats | dogs"), '<p class="agent-md-p">cats | dogs</p>');
  assert.equal(renderBounded("Intro\n| not a table"), '<p class="agent-md-p">Intro</p><p class="agent-md-p">| not a table</p>');
  assert.match(renderBounded("$$ x"), /^<p class="agent-md-p">/);
});
