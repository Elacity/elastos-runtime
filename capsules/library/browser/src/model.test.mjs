import assert from "node:assert/strict";
import test from "node:test";

import { protectionLabel } from "./model.js";

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
