import assert from "node:assert/strict";
import test from "node:test";

import {
  MAX_HOME_CLIPBOARD_IDENTIFIER_UTF8_BYTES,
  MAX_HOME_CLIPBOARD_PURPOSE_CODE_UNITS,
  MAX_HOME_CLIPBOARD_TEXT_UTF8_BYTES,
  homeClipboardOperationAllowed,
  homeClipboardPolicyFor,
  homeClipboardTargetSupported,
  homeClipboardValidWriteText,
} from "./home-clipboard-protocol.js";

test("target and purpose lookup accepts own policy entries only", () => {
  assert.equal(homeClipboardTargetSupported("browser"), true);
  assert.equal(
    homeClipboardOperationAllowed("library", "resource.identifier", "write"),
    true,
  );
  assert.equal(
    homeClipboardOperationAllowed("library", "resource.uri", "write"),
    true,
  );

  for (const inheritedName of ["__proto__", "constructor", "toString"]) {
    assert.doesNotThrow(() => homeClipboardTargetSupported(inheritedName));
    assert.equal(homeClipboardTargetSupported(inheritedName), false);
    assert.equal(homeClipboardPolicyFor(inheritedName, "browser.text"), null);
    assert.equal(homeClipboardPolicyFor("browser", inheritedName), null);
    assert.equal(
      homeClipboardOperationAllowed("browser", inheritedName, "write"),
      false,
    );
  }

  assert.equal(
    homeClipboardPolicyFor(
      "browser",
      "x".repeat(MAX_HOME_CLIPBOARD_PURPOSE_CODE_UNITS + 1),
    ),
    null,
  );
});

test("code-unit bounds reject before UTF-8 encoding while byte bounds stay exact", () => {
  class MustNotEncode {
    constructor() {
      throw new Error("oversized text reached TextEncoder");
    }
  }

  assert.equal(
    homeClipboardValidWriteText(
      "browser",
      "browser.text",
      "a".repeat(MAX_HOME_CLIPBOARD_TEXT_UTF8_BYTES + 1),
      MustNotEncode,
    ),
    false,
  );
  assert.equal(
    homeClipboardValidWriteText(
      "library",
      "resource.identifier",
      "a".repeat(MAX_HOME_CLIPBOARD_IDENTIFIER_UTF8_BYTES + 1),
      MustNotEncode,
    ),
    false,
  );

  const exactUtf8Text = "é".repeat(MAX_HOME_CLIPBOARD_TEXT_UTF8_BYTES / 2);
  assert.equal(
    homeClipboardValidWriteText("browser", "browser.text", exactUtf8Text),
    true,
  );
  assert.equal(
    homeClipboardValidWriteText(
      "browser",
      "browser.text",
      `${exactUtf8Text}a`,
    ),
    false,
  );
});

test("Library identifiers are bounded, write-only, and not arbitrary text", () => {
  const exactIdentifier = "a".repeat(
    MAX_HOME_CLIPBOARD_IDENTIFIER_UTF8_BYTES,
  );
  for (const identifier of [
    "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
    "revision:object-head-1",
    "localhost://Users/alice/Documents/report.txt",
    exactIdentifier,
  ]) {
    assert.equal(
      homeClipboardValidWriteText(
        "library",
        "resource.identifier",
        identifier,
      ),
      true,
    );
  }
  for (const denied of [
    "",
    "identifier with spaces",
    "identifier\nwith-control",
    `a${"é".repeat(MAX_HOME_CLIPBOARD_IDENTIFIER_UTF8_BYTES / 2)}`,
  ]) {
    assert.equal(
      homeClipboardValidWriteText(
        "library",
        "resource.identifier",
        denied,
      ),
      false,
    );
  }
  assert.equal(
    homeClipboardOperationAllowed("library", "resource.identifier", "read"),
    false,
  );
  assert.equal(
    homeClipboardOperationAllowed("documents", "resource.identifier", "write"),
    false,
  );
});
