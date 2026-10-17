import assert from "node:assert/strict";
import test from "node:test";

import {
  cacheFolderListing,
  setLibraryObjects,
  visibleObjectsForState,
} from "../capsules/library/browser/src/state.js";

function makeState(objects = [], options = {}) {
  const state = {
    objectsVersion: 0,
    folderCache: new Map(),
    query: "",
    showHidden: false,
    sort: "name",
    sortOrder: "asc",
    ...options,
  };
  setLibraryObjects(state, objects);
  return state;
}

function item(name, fields = {}) {
  return { uri: `localhost://Library/${name}`, name, kind: "file", ...fields };
}

function names(state) {
  return visibleObjectsForState(state).map((object) => object.name);
}

test("name sorting preserves the default locale, case/accent equivalence and source objects", () => {
  const objects = ["zoo", "Éclair", "apple", "eclair", "Ångström", "éclair", "İ", "ı", "I", "2", "10"]
    .map((name) => item(name));
  const originalOrder = objects.slice();
  const expected = objects.slice().sort((left, right) =>
    left.name.localeCompare(right.name, undefined, { sensitivity: "base" }));
  const state = makeState(objects);

  assert.deepEqual(names(state), expected.map((object) => object.name));
  assert.deepEqual(objects, originalOrder, "sorting must preserve provider order");
  for (const object of visibleObjectsForState(state)) {
    assert.strictEqual(object, objects.find((source) => source.uri === object.uri));
  }

  const ties = makeState([item("Éclair"), item("eclair"), item("éclair")]);
  assert.deepEqual(names(ties), ["Éclair", "eclair", "éclair"]);
  ties.sortOrder = "desc";
  assert.deepEqual(names(ties), ["Éclair", "eclair", "éclair"]);
});

test("name sorting reuses one locale collator across comparisons and folder refreshes", async (t) => {
  const NativeCollator = Intl.Collator;
  let constructions = 0;
  let comparisons = 0;
  t.mock.method(Intl, "Collator", function (...args) {
    constructions += 1;
    const collator = new NativeCollator(...args);
    return {
      compare(left, right) {
        comparisons += 1;
        return collator.compare(left, right);
      },
    };
  });
  const subject = await import("../capsules/library/browser/src/state.js?collator-reuse");
  const state = makeState();
  for (const objects of [[item("Zulu"), item("alpha")], [item("delta"), item("Bravo"), item("charlie")]]) {
    subject.setLibraryObjects(state, objects);
    const before = comparisons;
    const ascending = subject.visibleObjectsForState(state).map((object) => object.name);
    assert.ok(comparisons > before, "name comparisons must use the shared collator");
    assert.deepEqual(ascending, objects.map((object) => object.name).sort(new NativeCollator(undefined, { sensitivity: "base" }).compare));
    state.sortOrder = "desc";
    assert.deepEqual(subject.visibleObjectsForState(state).map((object) => object.name), ascending.toReversed());
    state.sortOrder = "asc";
    assert.equal(constructions, 1, "new listings and sort directions must reuse locale setup");
  }
});

test("directories stay first in both name sort directions", () => {
  const state = makeState([
    item("beta.txt"), item("Z folder", { kind: "directory" }),
    item("Alpha.txt"), item("A folder", { kind: "directory" }),
  ]);
  assert.deepEqual(names(state), ["A folder", "Z folder", "Alpha.txt", "beta.txt"]);
  state.sortOrder = "desc";
  assert.deepEqual(names(state), ["Z folder", "A folder", "beta.txt", "Alpha.txt"]);
});

for (const [sort, field, values] of [
  ["size", "size", ["20", "3", undefined]],
  ["modified", "modified_at", ["20", "3", undefined]],
  ["type", "mime", ["video/mp4", "text/plain", undefined]],
]) {
  test(`${sort} sorting preserves defaults, stable ties and directory precedence`, () => {
    const state = makeState([
      item("large", { [field]: values[0] }),
      item("folder", { kind: "directory", [field]: values[0] }),
      item("small", { [field]: values[1] }),
      item("missing", { [field]: values[2] }),
      item("small tie", { [field]: values[1] }),
    ], { sort });
    assert.deepEqual(names(state), ["folder", "missing", "small", "small tie", "large"]);
    state.sortOrder = "desc";
    assert.deepEqual(names(state), ["folder", "large", "small", "small tie", "missing"]);
  });
}

test("hidden filtering and search cover names, rooted URIs, MIME and both CIDs", () => {
  const state = makeState([
    item("Photo", { mime: "image/png", content_cid: "bafyLocal", published_cid: "bafyPublic" }),
    item("Notes", { uri: "localhost://Library/special-location" }),
    item(".Photo", { mime: "image/png" }),
    item("Other"),
  ]);
  assert.deepEqual(names(state), ["Notes", "Other", "Photo"]);
  for (const query of ["  pHoTo  ", "IMAGE/PNG", "bafylocal", "BAFYPUBLIC"]) {
    state.query = query;
    assert.deepEqual(names(state), ["Photo"], query);
  }
  state.query = "special-location";
  assert.deepEqual(names(state), ["Notes"]);
  state.query = "photo";
  state.showHidden = true;
  assert.deepEqual(names(state), [".Photo", "Photo"]);
  state.query = "absent";
  assert.deepEqual(names(state), []);
});

test("visible cache reuses results until preferences or fresh objects change", () => {
  const state = makeState([item("Beta", { size: 1 }), item("Alpha", { size: 2 }), item(".Hidden")]);
  let previous = visibleObjectsForState(state);
  assert.strictEqual(visibleObjectsForState(state), previous);
  for (const change of [
    { query: "alpha" }, { query: "" }, { showHidden: true },
    { sort: "size" }, { sortOrder: "desc" },
  ]) {
    Object.assign(state, change);
    const next = visibleObjectsForState(state);
    assert.notStrictEqual(next, previous);
    assert.strictEqual(visibleObjectsForState(state), next);
    previous = next;
  }
  assert.deepEqual(names(state), ["Alpha", "Beta", ".Hidden"]);

  const replacement = item("Beta", { size: 3, mime: "video/mp4", capabilities: [] });
  const version = state.objectsVersion;
  setLibraryObjects(state, [replacement]);
  assert.equal(state.objectsVersion, version + 1);
  assert.deepEqual(names(state), ["Beta"]);
  assert.strictEqual(visibleObjectsForState(state)[0], replacement);
  assert.strictEqual(state.objectsByUri.get(replacement.uri), replacement);
  setLibraryObjects(state, null);
  assert.deepEqual(visibleObjectsForState(state), []);
  assert.equal(state.objectsByUri.size, 0);
});

const folderUri = "localhost://Library";

function listingSignature(objects, folder = null) {
  return cacheFolderListing(makeState(), {}, folderUri, objects, folder).signature;
}

test("folder cache keeps equivalent listings while detecting membership and order changes", () => {
  const first = item("Alpha", { metadata: { visibility: { placement: "private_folder" } } });
  const second = item("Beta");
  const state = makeState();
  const perf = {};
  const objects = [first, second];
  const folder = item("Library", { uri: folderUri, kind: "directory", metadata: { readonly: false } });
  const cached = cacheFolderListing(state, perf, folderUri, objects, folder);
  assert.strictEqual(cached.objects, objects);
  assert.strictEqual(cached.object, folder);
  assert.strictEqual(state.folderCache.get(folderUri), cached);
  assert.equal(perf.folderCacheSize, 1);
  assert.equal(cached.signature, listingSignature(structuredClone(objects), structuredClone(folder)));
  for (const changed of [[first], [first, second, item("Gamma")], [second, first]]) {
    assert.notEqual(cached.signature, listingSignature(changed, folder));
  }
  assert.notEqual(cached.signature, listingSignature(objects, { ...folder, metadata: { readonly: true } }));
  assert.equal(listingSignature(null), listingSignature([]));
});

test("fresh action, viewer and property metadata invalidates a same-revision cached listing", async (t) => {
  const object = item("movie.mp4", {
    revision: "same-revision",
    mime: "video/mp4",
    capabilities: ["open", "download", "publish", "protect"],
    viewers: [{ id: "media-player", default: true }],
    created_at: 10,
    metadata: { visibility: { placement: "private_folder", effective_access: "principal_private" } },
  });
  const signature = listingSignature([object]);
  const changes = {
    mime: { mime: "application/octet-stream" },
    capabilities: { capabilities: [] },
    viewers: { viewers: [{ id: "elacity-player", default: true }] },
    created_at: { created_at: 20 },
    readonly: { metadata: { ...object.metadata, readonly: true } },
    protection: { metadata: { ...object.metadata, protected_content: { schema: "elastos.library.protected-content-identity/v1", mint_id: "42" } } },
    archive: { metadata: { ...object.metadata, archive_support: { status: "policy_gated_unsupported_archive_family" } } },
    visibility: { metadata: { ...object.metadata, visibility: { ...object.metadata.visibility, share_policy: "shared" } } },
    resolver: { metadata: { ...object.metadata, resolver_target: "localhost://NewTarget" } },
    trash: { metadata: { ...object.metadata, trash: { original_uri: "localhost://Library/Original" } } },
  };
  for (const [name, change] of Object.entries(changes)) {
    await t.test(name, () => {
      const fresh = { ...object, ...change };
      assert.equal(fresh.revision, object.revision);
      assert.notEqual(listingSignature([fresh]), signature, `${name} changed without a content revision`);
    });
  }
  assert.notEqual(listingSignature([item("File")]), listingSignature([item("File", { capabilities: [] })]),
    "missing capabilities and an explicit empty capability list have different action behavior");
});
