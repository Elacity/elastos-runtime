import assert from "node:assert/strict";
import test from "node:test";

import { createLibraryActions } from "./actions.js";

test("Library routes identifiers and resource URIs through distinct closed writers", async () => {
  const identifiers = [];
  const uris = [];
  const statuses = [];
  const actions = createLibraryActions({
    setStatus(status) {
      statuses.push(status);
    },
    async writeResourceIdentifier(value) {
      identifiers.push(value);
    },
    async writeResourceUri(value) {
      uris.push(value);
    },
  });

  await actions.copyText(
    "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
    "content CID",
    "resource.identifier",
  );
  await actions.copyText(
    "elastos://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
    "published link",
    "resource.uri",
  );

  assert.deepEqual(identifiers, [
    "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
  ]);
  assert.deepEqual(uris, [
    "elastos://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
  ]);
  assert.deepEqual(statuses, [
    "Copied content CID.",
    "Copied published link.",
  ]);
  await assert.rejects(
    actions.copyText("arbitrary text", "value", "browser.text"),
    /purpose is denied/,
  );
});
