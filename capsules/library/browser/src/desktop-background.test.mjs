import assert from "node:assert/strict";
import test from "node:test";

import { isDesktopBackgroundCandidate } from "./model.js";

const picture = (extra = {}) => ({
  uri: "localhost://Users/did:key:z6Mk/Pictures/hills.png",
  kind: "file",
  mime: "image/png",
  size: 1024,
  capabilities: ["open", "read", "download"],
  metadata: { visibility: { placement: "private_folder" } },
  ...extra,
});

test("Set as Desktop Background is offered for an own, readable, unprotected picture", () => {
  assert.equal(isDesktopBackgroundCandidate(picture()), true);
});

test("the menu hides what Runtime would refuse; Runtime still decides", () => {
  assert.equal(isDesktopBackgroundCandidate(picture({ metadata: { protected_content: { mint_id: "m" } } })), false);
  assert.equal(isDesktopBackgroundCandidate(picture({ blocked_reason: "protected_principal_root_object_not_encrypted" })), false);
  assert.equal(isDesktopBackgroundCandidate(picture({ capabilities: ["properties"] })), false);
  assert.equal(isDesktopBackgroundCandidate(picture({ uri: "localhost://Users/did:key:z6Mk/.Trash/hills.png" })), false);
  assert.equal(isDesktopBackgroundCandidate(picture({ metadata: { visibility: { placement: "runtime_private" } } })), false);
  assert.equal(isDesktopBackgroundCandidate(picture({ uri: "localhost://WebSpaces/x/hills.png" })), false);
  assert.equal(isDesktopBackgroundCandidate(picture({ mime: "image/svg+xml" })), false);
  assert.equal(isDesktopBackgroundCandidate(picture({ size: 5 * 1024 * 1024 + 1 })), false);
  assert.equal(isDesktopBackgroundCandidate(picture({ kind: "directory", mime: "inode/directory" })), false);
});
