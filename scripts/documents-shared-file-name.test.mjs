#!/usr/bin/env node
// Home, Documents and Library clean a Chat post's file name the same way, and
// the same way Chat does (chat-room-ui shared_file_display_name tests use the
// same cases).

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

import { sharedFileName as homeName } from "../capsules/home/browser/home-external-link.js";
import { sharedFileName as libraryName } from "../capsules/library/browser/src/shared-file.js";

const documentsSource = readFileSync(new URL("../capsules/documents/browser/documents.js", import.meta.url), "utf8");
const start = documentsSource.indexOf("function sharedFileName(");
const end = documentsSource.indexOf("\n}\n", start) + 2;
assert.ok(start >= 0 && end > start, "Documents must clean shared file names");
const documentsContext = vm.createContext({ TextEncoder });
vm.runInContext(documentsSource.slice(start, end) + "\nthis.sharedFileName = sharedFileName;", documentsContext);
const documentsName = documentsContext.sharedFileName;

const cases = [
  ["omp-test-note.txt", "omp-test-note.txt"],
  ["a · b.txt", "a · b.txt"],
  ["..hidden\u0007.txt", "hidden.txt"],
  [" .\u001b.hidden ", "hidden"],
  // At most 200 UTF-8 bytes (the Runtime's file-name room, named in
  // home-external-link.js), cut on a character boundary before the extension.
  ["n".repeat(300), "n".repeat(200)],
  ["中".repeat(80) + ".txt", "中".repeat(65) + ".txt"],
  ["中".repeat(66) + ".txt", "中".repeat(65) + ".txt"],
  ["中".repeat(65) + ".txt", "中".repeat(65) + ".txt"],
  ["a".repeat(196) + ".txt", "a".repeat(196) + ".txt"],
  ["a".repeat(197) + ".txt", "a".repeat(196) + ".txt"],
  ["中".repeat(300), "中".repeat(66)],
  ["a".repeat(190) + "." + "b".repeat(20), "a".repeat(190) + "." + "b".repeat(9)],
  ["😀".repeat(60) + ".md", "😀".repeat(49) + ".md"],
  // A cut never leaves a trailing dot before the extension.
  ["中".repeat(65) + ".pdf.txt", "中".repeat(65) + ".txt"],
  ["a".repeat(195) + " " + "中".repeat(10) + ".txt", "a".repeat(195) + ".txt"],
  [" \u001b ", ""],
  [".....", ""],
  ["dir/x.txt", ""],
  ["a\\b.txt", ""],
  [null, ""],
  [undefined, ""],
  [5, ""],
];

test("Home, Documents and Library clean a shared file name alike, within 200 UTF-8 bytes", () => {
  for (const [input, expected] of cases) {
    for (const [label, clean] of [["Home", homeName], ["Documents", documentsName], ["Library", libraryName]]) {
      const cleaned = clean(input);
      assert.equal(cleaned, expected, `${label} ${JSON.stringify(input)}`);
      assert.ok(new TextEncoder().encode(cleaned).length <= 200, `${label} stays within 200 bytes`);
    }
  }
});
