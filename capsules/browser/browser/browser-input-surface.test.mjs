import assert from "node:assert/strict";
import test from "node:test";

import {
  plainTextFromClipboardEvent,
} from "./browser-input-surface.js";

function clipboardEvent(types, values = {}) {
  return {
    clipboardData: {
      types,
      getData(type) {
        return values[type] ?? "";
      },
    },
  };
}

test("explicit ClipboardEvent accepts only text/plain", () => {
  assert.equal(
    plainTextFromClipboardEvent(
      clipboardEvent(["text/plain"], { "text/plain": "paste me" }),
    ),
    "paste me",
  );
  assert.equal(
    plainTextFromClipboardEvent(
      clipboardEvent(["text/html"], { "text/html": "<b>no</b>" }),
    ),
    "",
  );
  assert.equal(
    plainTextFromClipboardEvent(
      clipboardEvent(["text"], { text: "alias is not accepted" }),
    ),
    "",
  );
});

test("missing or malformed ClipboardEvent data is rejected", () => {
  assert.equal(plainTextFromClipboardEvent(null), "");
  assert.equal(plainTextFromClipboardEvent({ clipboardData: {} }), "");
  assert.equal(
    plainTextFromClipboardEvent({
      clipboardData: {
        types: ["text/plain"],
        getData() {
          return null;
        },
      },
    }),
    "",
  );
});
