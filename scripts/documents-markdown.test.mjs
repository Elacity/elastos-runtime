#!/usr/bin/env node

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../capsules/documents/browser/documents.js", import.meta.url), "utf8");
const start = source.indexOf("function escapeHtml(");
const end = source.indexOf("function shortCid(", start);
assert.ok(start >= 0 && end > start, "Documents Markdown renderer must exist");
const renderer = vm.runInNewContext(source.slice(start, end) + "\n({ renderMarkdownDocument, renderInlineMarkdown, safeMarkdownHref })");

test("Documents loads its module through an external script", () => {
  const html = readFileSync(new URL("../capsules/documents/browser/index.html", import.meta.url), "utf8");
  assert.match(html, /<script type="module" src="\.\/documents\.js"><\/script>/);
  for (const script of html.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script>/gi)) {
    assert.equal(script[1].trim(), "", "Documents scripts must load from source files");
  }
});

const payload = '<img src=x onerror=alert(1)><script>alert(1)</script>';
const escapedPayload = '&lt;img src=x onerror=alert(1)&gt;&lt;script&gt;alert(1)&lt;/script&gt;';
const allowedTags = new Set(["p", "br", "pre", "code", "strong", "em", "a", "hr", "h1", "h2", "h3", "h4", "h5", "h6", "blockquote", "ul", "ol", "li", "label", "input", "span", "table", "thead", "tbody", "tr", "th", "td"]);

function assertSafeHtml(html) {
  for (const match of html.matchAll(/<\/?([a-z][a-z0-9]*)\b[^>]*>/gi)) {
    assert.ok(allowedTags.has(match[1]), `Unexpected element: ${match[0]}`);
    assert.doesNotMatch(match[0], /\son[a-z]+\s*=/i, `Event handler: ${match[0]}`);
  }
}

test("hostile fence metadata cannot create an element or event handler", () => {
  for (const language of ['js" onmouseover="alert(1)', 'js">' + payload, 'js&quote; onload=alert(1)', "js name", "js`name"]) {
    const { html } = renderer.renderMarkdownDocument("```" + language + "\n" + payload + "\n```");
    assert.equal(html, "<pre><code>" + escapedPayload + "</code></pre>");
    assertSafeHtml(html);
  }
});

for (const [field, markdown, expectedTag, options] of [
  ["paragraph", payload, "p"],
  ["heading", "# " + payload, "h1"],
  ["quote", "> " + payload, "blockquote"],
  ["unordered item", "- " + payload, "li"],
  ["ordered item", "1. " + payload, "li"],
  ["task label", "- [x] " + payload, "li"],
  ["interactive task label", "- [ ] " + payload, "input", { interactiveTasks: true }],
  ["table heading", "| " + payload + " |\n| --- |\n| safe |", "th"],
  ["table cell", "| safe |\n| --- |\n| " + payload + " |", "td"],
  ["fence body", "```js\n" + payload + "\n```", "code"],
  ["inline code", "`" + payload + "`", "code"],
  ["strong text", "**" + payload + "**", "strong"],
  ["emphasized text", "*" + payload + "*", "em"],
  ["link label", "[" + payload + "](https://example.test/)", "a"],
  ["image label", "![" + payload + "](https://example.test/image.png)", "a"],
]) {
  test(`${field} keeps hostile HTML as text`, () => {
    const { html, outline } = renderer.renderMarkdownDocument(markdown, options);
    assert.ok(html.includes(escapedPayload), html);
    assert.match(html, new RegExp("<" + expectedTag + "(?:>| )"));
    assertSafeHtml(html);
    if (field === "heading") {
      assert.equal(outline[0].title, payload);
      assert.match(outline[0].id, /^[a-z0-9-]+$/);
    }
  });
}

test("link and image destinations refuse executable or hidden schemes", () => {
  for (const href of [
    "javascript:alert", "JaVaScRiPt:alert", "java\tscript:alert", "java\nscript:alert",
    "&#106;avascript:alert", "javascript&#58;alert", "data:text/html,<script>alert</script>",
    "vbscript:alert", "file:///etc/passwd", "blob:https://example.test/id", "//example.test/",
  ]) {
    for (const prefix of ["", "!"]) {
      const { html } = renderer.renderMarkdownDocument(prefix + "[label](" + href + ")");
      assert.doesNotMatch(html, /href="(?!#")/);
      assertSafeHtml(html);
    }
    assert.equal(renderer.safeMarkdownHref(href), "#");
  }
});

test("link and image destinations escape attribute and element payloads", () => {
  const href = 'https://example.test/" onmouseover="alert&quote;<img src=x onerror=alert>';
  for (const prefix of ["", "!"]) {
    const { html } = renderer.renderMarkdownDocument(prefix + "[label](" + href + ")");
    assert.equal(html, '<p>' + prefix + '<a href="https://example.test/&quot; onmouseover=&quot;alert&amp;quote;&lt;img src=x onerror=alert&gt;" rel="noopener noreferrer">label</a></p>');
    // The hostile words belong to the quoted URL value, not new attributes.
    assert.doesNotMatch(html, /" onmouseover=/);
    assert.doesNotMatch(html, /<img\b/);
  }
});

test("safe Markdown retains formatting, tasks, links, tables, and fence languages", () => {
  for (const language of ["js", "C++", "C#", "objective-c", "python3"]) {
    assert.equal(renderer.renderMarkdownDocument("```" + language + "\nconst x = 1 < 2;\n```").html,
      '<pre><code class="language-' + language + '">const x = 1 &lt; 2;</code></pre>');
  }
  assert.equal(renderer.renderInlineMarkdown("**bold** *italic* `literal [link](https://example.test/)`"),
    "<strong>bold</strong> <em>italic</em> <code>literal [link](https://example.test/)</code>");
  for (const href of ["https://example.test/?a=1&b=2", "http://example.test/", "elastos://document/example", "localhost://example", "#heading", "/document/example", "./example", "../example"]) {
    const escapedHref = href.replaceAll("&", "&amp;");
    assert.equal(renderer.renderInlineMarkdown("[**label**](" + href + ")"),
      '<a href="' + escapedHref + '" rel="noopener noreferrer"><strong>label</strong></a>');
  }
  const { html, outline } = renderer.renderMarkdownDocument("# Title\n\n> Quote\n\n- [x] Done\n- [ ] Next\n\n1. First\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n\n---", { interactiveTasks: true });
  assert.equal(outline[0].id, "title-1");
  assert.equal((html.match(/class="task-checkbox"/g) || []).length, 2);
  assert.match(html, /data-source-line="4" checked/);
  assert.match(html, /<blockquote><p>Quote<\/p><\/blockquote>/);
  assert.match(html, /<ol><li>First<\/li><\/ol>/);
  assert.match(html, /<th>A<\/th><th>B<\/th>/);
  assert.match(html, /<td>1<\/td><td>2<\/td>/);
  assert.match(html, /<hr>/);
  assertSafeHtml(html);
});
