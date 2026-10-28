#!/usr/bin/env node
// Home, Documents and Library clean a Chat post's file name the same way, and
// the same way Chat does (chat-room-ui shared_file_display_name tests).

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
const documentsContext = vm.createContext({});
vm.runInContext(documentsSource.slice(start, end) + "\nthis.sharedFileName = sharedFileName;", documentsContext);
const documentsName = documentsContext.sharedFileName;

const cases = [
  ["omp-test-note.txt", "omp-test-note.txt"],
  ["a · b.txt", "a · b.txt"],
  ["..hidden\u0007.txt", "hidden.txt"],
  [" .\u001b.hidden ", "hidden"],
  ["n".repeat(300), "n".repeat(120)],
  [" \u001b ", ""],
  [".....", ""],
  ["dir/x.txt", ""],
  ["a\\b.txt", ""],
  [null, ""],
  [undefined, ""],
  [5, ""],
];

test("Home, Documents and Library clean a shared file name alike", () => {
  for (const [input, expected] of cases) {
    for (const [label, clean] of [["Home", homeName], ["Documents", documentsName], ["Library", libraryName]]) {
      assert.equal(clean(input), expected, `${label} ${JSON.stringify(input)}`);
    }
  }
});
