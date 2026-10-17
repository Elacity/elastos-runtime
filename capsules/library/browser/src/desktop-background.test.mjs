import assert from "node:assert/strict";
import test from "node:test";

import { createLibraryActions } from "./actions.js";
import { desktopBackgroundRefusalMessage, isDesktopBackgroundCandidate } from "./model.js";

const noop = () => {};
const actionDefaults = {
  clearSelection: noop,
  closeSelf: noop,
  confirmDestructive: async () => true,
  currentFolderReadOnly: () => false,
  deliverToTarget: () => false,
  downloadObjectRaw: async () => ({ blob: new Blob([]), filename: "item.bin" }),
  loadCurrentFolder: async () => {},
  loadRoots: async () => {},
  navigate: async () => {},
  openPublishedUri: noop,
  openTarget: () => false,
  previewObject: async () => {},
  providerApi: {},
  renderUploads: noop,
  selectedObjects: () => [],
  setStatus: noop,
  setUploadProgress: noop,
  showMenuForObject: noop,
  showObjectStatus: noop,
  showProtectAndListDialog: async () => false,
  showProperties: noop,
  showShareDialog: noop,
  showShareReceipt: noop,
  showSharedAccessReceipt: noop,
  startCreateObject: noop,
  state: {},
  uploadObject: async () => {},
  writeResourceIdentifier: async () => {},
  writeResourceUri: async () => {},
};

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

test("Runtime's plain refusal reaches the person; anything else gives Library's own message", () => {
  const reason =
    'Library cannot check your protected file "clip.png" because it is not in "Pictures". ' +
    'Move it back to "Pictures", then try again. Or upload the picture in System > Background > Choose image.';
  assert.equal(desktopBackgroundRefusalMessage({ accepted: false, message: reason }), reason);
  assert.equal(desktopBackgroundRefusalMessage({ accepted: false, message: `  ${reason}\t` }), reason);
  assert.equal(desktopBackgroundRefusalMessage({ accepted: false }), "");
  assert.equal(desktopBackgroundRefusalMessage(null), "");
  assert.equal(desktopBackgroundRefusalMessage({ message: 42 }), "");
  assert.equal(desktopBackgroundRefusalMessage({ message: "x".repeat(601) }), "");
  assert.equal(desktopBackgroundRefusalMessage({ message: "line one\nline two" }), "");
  assert.equal(desktopBackgroundRefusalMessage({ message: "bell\u0007" }), "");
});

test("Set as Desktop Background shows Runtime's reason, or Library's message when there is none", async () => {
  const statuses = [];
  const run = async (result) => {
    const actions = createLibraryActions({
      ...actionDefaults,
      setStatus: (status) => statuses.push(status),
      setDesktopBackground: async () => result,
    });
    await actions.setObjectAsDesktopBackground(picture());
    return statuses.at(-1);
  };
  assert.equal(await run({ accepted: true, message: "" }), "Desktop background updated.");
  assert.equal(
    await run({ accepted: false, message: "Library did not finish the check of your protected files. Try again." }),
    "Library did not finish the check of your protected files. Try again.",
  );
  assert.equal(await run({ accepted: false, message: "" }), "Could not set this image as the desktop background.");
  assert.equal(await run({ accepted: false, message: "a\nb" }), "Could not set this image as the desktop background.");
});
