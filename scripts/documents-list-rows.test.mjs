#!/usr/bin/env node
// A Documents list row: the title once, then one quiet line with the folder
// (only for a subfolder of Documents) and a short time.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../capsules/documents/browser/documents.js", import.meta.url), "utf8");
const functionSource = (name) => {
  const start = source.indexOf(`function ${name}(`);
  assert.ok(start >= 0, `missing ${name}`);
  return source.slice(start, source.indexOf("\n}\n", start) + 2);
};
const context = vm.createContext({});
vm.runInContext(["listTitle", "listFolder", "shortTime"].map(functionSource).join("\n"), context);
const { listTitle, listFolder, shortTime } = context;

test("one title rule for Documents-made rows and plain files", () => {
  assert.equal(listTitle({ title: "test", file_name: "test.md" }), "test");
  assert.equal(listTitle({ title: "New File.txt", file_name: "New File.txt" }), "New File");
  assert.equal(listTitle({ title: "Shared Note.TXT" }), "Shared Note");
  assert.equal(listTitle({ title: "report.pdf" }), "report.pdf");
  assert.equal(listTitle({ title: ".md" }), ".md");
  assert.equal(listTitle({ title: "  " }), "Untitled");
});

test("the folder shows only for a file in a subfolder of Documents", () => {
  assert.equal(listFolder({ file_name: "test.md" }), "");
  assert.equal(listFolder({ file_name: "Notes/Plain.txt" }), "Notes");
  assert.equal(listFolder({ file_name: "Work/2026/plan.md" }), "2026");
  assert.equal(listFolder({}), "");
});

test("times are short: Today with the time, Yesterday, then the date", () => {
  const now = new Date(2026, 9, 28, 17, 0);
  const at = (...parts) => new Date(...parts).getTime() / 1000;
  assert.match(shortTime(at(2026, 9, 28, 16, 41), now), /^Today 4:41\s?PM$/);
  assert.equal(shortTime(at(2026, 9, 27, 23, 59), now), "Yesterday");
  assert.equal(shortTime(at(2026, 9, 26, 9, 0), now), "Oct 26");
  assert.equal(shortTime(at(2025, 11, 31, 9, 0), now), "Dec 31, 2025");
  assert.equal(shortTime(0, now), "");
});
