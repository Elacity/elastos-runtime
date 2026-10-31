import assert from "node:assert/strict";
import test from "node:test";

import { pathCrumbs, protectionLabel } from "./model.js";

test("Library names each protection state in Properties", () => {
  assert.equal(protectionLabel({ blocked_reason: null }), "ok");
  assert.equal(
    protectionLabel({ blocked_reason: "protected_principal_root_object_damaged" }),
    "Damaged or not readable here",
  );
  assert.equal(
    protectionLabel({ blocked_reason: "protected_principal_root_object_not_encrypted" }),
    "Not encrypted",
  );
});

test("Library place paths use the longest root and decoded names", () => {
  const roots = [
    { label: "Home", uri: "localhost://Users/test" },
    { label: "Documents", uri: "localhost://Users/test/Documents" },
  ];
  assert.deepEqual(pathCrumbs(roots, "localhost://Users/test/Documents/Probe%20Folder/50%25"), [
    { label: "Documents", uri: "localhost://Users/test/Documents" },
    { label: "Probe Folder", uri: "localhost://Users/test/Documents/Probe%20Folder" },
    { label: "50%", uri: "localhost://Users/test/Documents/Probe%20Folder/50%25" },
  ]);
  assert.deepEqual(pathCrumbs(roots, "localhost://Users/test"), [{ label: "Home", uri: "localhost://Users/test" }]);
  assert.deepEqual(pathCrumbs(roots, "localhost://Users/test/Bad%"), [
    { label: "Home", uri: "localhost://Users/test" },
    { label: "Bad%", uri: "localhost://Users/test/Bad%" },
  ]);
  assert.deepEqual(pathCrumbs(roots, "localhost://Users/other/Documents"), []);
  assert.deepEqual(pathCrumbs(roots, "localhost://Users/testing"), []);
});
