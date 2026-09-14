import assert from "node:assert/strict";
import test from "node:test";

import {
  isRuntimeCustodyProtectable,
  protectedContentKindFor,
  viewerForProtectedContent,
} from "./model.js";

function protectableObject(overrides = {}) {
  return {
    uri: "localhost://Users/test/Documents/report.pdf",
    kind: "file",
    name: "report.pdf",
    mime: "application/pdf",
    capabilities: ["publish"],
    ...overrides,
  };
}

test("Library classifies video and audio as the media kind and everything else as an object", () => {
  for (const mime of ["video/mp4", "video/quicktime", "video/x-matroska", "video/mp2t"]) {
    assert.equal(protectedContentKindFor(mime), "media", mime);
  }
  for (const mime of ["audio/mpeg", "audio/mp4", "audio/flac", "audio/ogg"]) {
    assert.equal(protectedContentKindFor(mime), "media", mime);
  }
  for (const mime of [
    "application/pdf",
    "image/png",
    "image/jpeg",
    "application/epub+zip",
    "application/vnd.comicbook+zip",
    "text/plain",
    "application/zip",
  ]) {
    assert.equal(protectedContentKindFor(mime), "object", mime);
  }
  assert.equal(protectedContentKindFor(""), "object");
  assert.equal(protectedContentKindFor(undefined), "object");
});

test("Library offers protection for every file type the object side accepts", () => {
  for (const mime of [
    "video/mp4",
    "application/pdf",
    "image/png",
    "application/epub+zip",
    "application/vnd.comicbook+zip",
    "text/plain",
    "application/zip",
  ]) {
    assert.equal(
      isRuntimeCustodyProtectable(protectableObject({ mime })),
      true,
      mime,
    );
  }
});

test("Library withholds protection for audio until the media path accepts it", () => {
  // Task 16 makes the media path accept audio; this test flips to `true` then.
  // Until it does, the action must not be offered, because a listing started
  // from here cannot complete. The kind and the viewer routing stay media.
  for (const [name, mime] of [
    ["song.mp3", "audio/mpeg"],
    ["song.m4a", "audio/mp4"],
    ["song.flac", "audio/flac"],
    ["song.ogg", "audio/ogg"],
    ["song.wav", "audio/wav"],
  ]) {
    assert.equal(
      isRuntimeCustodyProtectable(
        protectableObject({
          uri: `localhost://Users/test/Music/${name}`,
          name,
          mime,
        }),
      ),
      false,
      mime,
    );
    assert.equal(protectedContentKindFor(mime), "media", mime);
    assert.equal(viewerForProtectedContent({ mime }), "elacity-player", mime);
  }
});

test("Library withholds protection for a type the publish side refuses", () => {
  // Unknown extensions arrive as `application/octet-stream`, which publish
  // refuses before any mint, so the action must never be offered for them.
  assert.equal(
    isRuntimeCustodyProtectable(
      protectableObject({
        uri: "localhost://Users/test/Documents/notes.xyz",
        name: "notes.xyz",
        mime: "application/octet-stream",
      }),
    ),
    false,
  );
  assert.equal(
    isRuntimeCustodyProtectable(
      protectableObject({
        uri: "localhost://Users/test/Documents/notes",
        name: "notes",
        mime: "",
      }),
    ),
    false,
  );
});

test("Library keeps every non-type protection guard", () => {
  const cases = [
    [null, "missing object"],
    [
      protectableObject({
        uri: "localhost://Users/test/Documents/folder",
        kind: "directory",
        name: "folder",
        mime: "inode/directory",
      }),
      "directory",
    ],
    [
      protectableObject({ uri: "localhost://Users/test/.Trash/report.pdf" }),
      "trashed object",
    ],
    [
      protectableObject({ uri: "localhost://WebSpaces/Cloud/report.pdf" }),
      "web space object",
    ],
    [protectableObject({ published: true }), "already published"],
    [protectableObject({ metadata: { readonly: true } }), "read-only object"],
    [
      protectableObject({
        metadata: {
          protected_content: { schema: "elastos.library.protected-content-identity/v1" },
        },
      }),
      "already protected",
    ],
    [protectableObject({ capabilities: [] }), "no publish capability"],
    [protectableObject({ capabilities: undefined }), "missing capabilities"],
  ];
  for (const [object, label] of cases) {
    assert.equal(isRuntimeCustodyProtectable(object), false, label);
  }
});

test("Library sends protected media to the player and protected objects to the reader", () => {
  assert.equal(viewerForProtectedContent({ mime: "video/mp4" }), "elacity-player");
  assert.equal(viewerForProtectedContent({ mime: "audio/mpeg" }), "elacity-player");
  assert.equal(viewerForProtectedContent({ mime: "application/pdf" }), "elacity-reader");
  assert.equal(viewerForProtectedContent({ mime: "image/png" }), "elacity-reader");
  assert.equal(
    viewerForProtectedContent({ mime: "application/vnd.comicbook+zip" }),
    "elacity-reader",
  );
  assert.equal(viewerForProtectedContent({}), "elacity-reader");
});
