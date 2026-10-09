import assert from "node:assert/strict";
import test from "node:test";
import { createHash, createPublicKey, verify } from "node:crypto";
import { chmodSync, lstatSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildEntry, canonical, contentManifest, download, fileRecord, loadFixture, ownedDirectory, producePackage, rawCid, signedCatalog, SMOL_FIXTURE, verifyInstalledKubo, writeCatalogPayload } from "./ci-model-package.mjs";

const QWEN_FIXTURE = loadFixture(new URL("./pinned-qwen2.5-1.5b-fixture.json", import.meta.url));

const sha = bytes => createHash("sha256").update(bytes).digest("hex");
function temporary(t) {
  const root = mkdtempSync(join(realpathSync(tmpdir()), "elastos-ci-model-package-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  return root;
}

test("pinned inputs refuse missing, replaced and linked bytes", t => {
  const root = temporary(t), path = join(root, "input");
  const bytes = Buffer.from("fixture bytes"), pin = { size: bytes.length, sha256: sha(bytes) };
  assert.throws(() => fileRecord(path, "input", pin), /ENOENT/);
  writeFileSync(path, bytes);
  assert.deepEqual(fileRecord(path, "input", pin), { path: "input", ...pin });
  writeFileSync(path, Buffer.from("changed bytes"));
  assert.throws(() => fileRecord(path, "input", pin), /pinned checksum/);
  writeFileSync(path, "short");
  assert.throws(() => fileRecord(path, "input", pin), /pinned size/);
  const linked = join(root, "linked");
  symlinkSync(path, linked);
  assert.throws(() => fileRecord(linked, "input", pin), /symlink/);
});

test("producer refuses absent or truncated model before writing a package", t => {
  const root = temporary(t), packageDir = join(root, "package");
  assert.throws(() => buildEntry(root, packageDir, null, SMOL_FIXTURE), /ENOENT/);
  writeFileSync(join(root, SMOL_FIXTURE.model.name), "GGUF");
  assert.throws(() => buildEntry(root, packageDir, null, SMOL_FIXTURE), /pinned size/);
  assert.throws(() => readFileSync(join(packageDir, "capsule.json")), /ENOENT/);
});

for (const fixture of [SMOL_FIXTURE, QWEN_FIXTURE]) {
  test(`${fixture.id} fixture fetch selects its declared immutable provenance and licensing`, () => {
    const manifest = fixture.capsule_manifest, model = manifest.model_content;
    assert.equal(manifest.role, "content");
    assert.deepEqual(manifest.projections, ["content"]);
    assert.equal(model.license.spdx_id, "Apache-2.0");
    assert.equal(model.license.path, fixture.license.name);
    assert.equal(model.provenance.base_license.path, "LICENSE.base");
    assert.equal(model.provenance.base_license.spdx_id, model.license.spdx_id);
  });
}

test("fixture loading refuses escaping names, injected text, unpinned weights and a title Runtime would not show", t => {
  const root = temporary(t), path = join(root, "fixture.json");
  const refused = (mutate, message) => {
    const fixture = structuredClone(QWEN_FIXTURE);
    mutate(fixture);
    writeFileSync(path, JSON.stringify(fixture));
    assert.throws(() => loadFixture(path), message);
  };
  const provenance = f => f.capsule_manifest.model_content.provenance;
  refused(f => { f.model.name = "../../outside.gguf"; f.model.url = f.model.url.replace(/[^/]+$/, f.model.name); }, /one plain \.gguf file name/);
  refused(f => { f.license.name = "../LICENSE"; }, /package LICENSE file/);
  refused(f => { provenance(f).quantized_repository = "Qwen/Qwen2.5\nSigned by someone else."; }, /owner\/name identifiers/);
  refused(f => { provenance(f).base_repository = "Qwen/../../etc"; }, /owner\/name identifiers/);
  refused(f => { f.model.url = f.model.url.replace(/resolve\/[0-9a-f]+\//, "resolve/main/"); }, /pinned quantized repository revision/);
  refused(f => { f.license.url = "https://example.com/LICENSE"; }, /pinned revision or apache\.org/);
  refused(f => { provenance(f).quantized_revision = "91cad511"; }, /full commit hashes/);
  refused(f => { f.capsule_manifest.description = "one\nline"; }, /one bounded line/);
  refused(f => { f.display_name = "Qwen 1.5B"; }, /display_name/);
});

function decodeBase(bytes, alphabet, bits) {
  if (bits) {
    const output = []; let accumulator = 0, count = 0;
    for (const character of bytes) {
      accumulator = (accumulator << bits) | alphabet.indexOf(character); count += bits;
      if (count >= 8) { count -= 8; output.push((accumulator >>> count) & 255); }
    }
    return Buffer.from(output);
  }
  let value = 0n;
  for (const character of bytes) value = value * 58n + BigInt(alphabet.indexOf(character));
  let hex = value.toString(16);
  if (hex.length % 2) hex = "0" + hex;
  return Buffer.from(hex, "hex");
}

test("disposable catalog verifies the domain, signed bytes, DID and raw content pin", () => {
  const entry = { cid: "bafyfixture", capsule_manifest: SMOL_FIXTURE.capsule_manifest, object_manifest: contentManifest([]) };
  const first = signedCatalog(entry, 12345), second = signedCatalog(entry, 12345);
  assert.notEqual(first.trust.publisher_dids[0], second.trust.publisher_dids[0]);
  const envelope = JSON.parse(first.catalog), multicodec = decodeBase(envelope.signer_did.slice("did:key:z".length), "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz");
  assert.deepEqual(multicodec.subarray(0, 2), Buffer.from([0xed, 1]));
  assert.equal(multicodec.length, 34);
  const key = createPublicKey({ key: Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), multicodec.subarray(2)]), format: "der", type: "spki" });
  const signature = Buffer.from(envelope.signature, "hex");
  const message = payload => createHash("sha256").update("elastos.model.catalog.v1\0").update(canonical(payload)).digest();
  assert(verify(null, message(envelope.payload), key, signature));
  assert(!verify(null, message({ ...envelope.payload, published_at: 12346 }), key, signature));
  const wrongDomain = createHash("sha256").update("elastos.other.catalog.v1\0").update(canonical(envelope.payload)).digest();
  assert(!verify(null, wrongDomain, key, signature));
  assert.deepEqual(first.trust.publisher_dids, [envelope.signer_did]);
  assert.equal(first.trust.head_cid, rawCid(first.catalog));
  const raw = decodeBase(first.trust.head_cid.slice(1), "abcdefghijklmnopqrstuvwxyz234567", 5);
  assert.deepEqual(raw.subarray(0, 4), Buffer.from([1, 0x55, 0x12, 0x20]));
  assert.equal(raw.subarray(4).toString("hex"), sha(first.catalog));
  assert.deepEqual(Object.keys(first).sort(), ["catalog", "trust"]);
});

function kuboFixture(t) {
  const data = temporary(t), host = "linux-amd64", capsule = join(data, "capsules/kubo");
  for (const path of [capsule, join(data, "receipts"), join(data, "bin")]) mkdirSync(path, { recursive: true });
  const binary = Buffer.from("synthetic Kubo executable"), license = Buffer.from("synthetic licensed input");
  const recipe = {
    schema: "elastos.release-upstream-input/v1", component: "kubo", platform: host, version: "0.40.1",
    root: "kubo", entrypoint: "ipfs", extract_path: "kubo/ipfs", install_path: "bin/kubo",
    source: { url: "https://example.test/pinned-kubo.tar.gz", checksum: `sha256:${"b".repeat(64)}`, max_bytes: 1024 },
    license: { spdx_id: "MIT", files: [{ name: "LICENSE", source: { checksum: `sha256:${sha(license)}` } }] },
  };
  const capsule_manifest = { schema: "elastos.capsule/v1", name: "kubo", version: recipe.version, role: "content", type: "data", projections: ["content"], entrypoint: "ipfs" };
  const files = {
    ipfs: binary, LICENSE: license, "capsule.json": Buffer.from(canonical(capsule_manifest) + "\n"),
    "PROVENANCE.json": Buffer.from(canonical({ recipe_sha256: sha(Buffer.from(canonical(recipe) + "\n")), upstream: recipe.source }) + "\n"),
  };
  for (const [path, bytes] of Object.entries(files)) writeFileSync(join(capsule, path), bytes);
  writeFileSync(join(data, "bin/kubo"), binary);
  const object_manifest = contentManifest(Object.entries(files).map(([path, bytes]) => ({ path, size: bytes.length, sha256: sha(bytes) })));
  const checksum = `sha256:${"a".repeat(64)}`, release_path = `kubo-${host}.tar.gz`, size = 200;
  const capsule_metadata = { install_path: "capsules/kubo", extract_path: "kubo", release_path, checksum, size };
  const receipt = { schema: recipe.schema, component: "kubo", platform: host, extract_path: recipe.extract_path, install_path: recipe.install_path, release_path, checksum, size, capsule_metadata, capsule_manifest, object_manifest };
  const components = { external: { kubo: { capsule_metadata: { install_path: "capsules/kubo", platforms: { [host]: capsule_metadata } }, platforms: { [host]: { install_path: "bin/kubo", checksum: `sha256:${sha(binary)}`, size: binary.length } } } } };
  const save = () => {
    writeFileSync(join(capsule, "_elastos_object.json"), canonical(receipt.object_manifest));
    writeFileSync(join(data, "receipts/kubo-build.json"), canonical(receipt));
  };
  writeFileSync(join(capsule, ".elastos-artifact-sha256"), checksum.slice(7) + "\n");
  save();
  return { data, host, capsule, recipe, receipt, components, save, check: () => verifyInstalledKubo(data, components, host, recipe) };
}

test("Kubo verification accepts a complete licensed receipt and native checksum", t => {
  const fixture = kuboFixture(t), proof = fixture.check();
  assert.equal(proof.archive_checksum, fixture.receipt.checksum);
  assert.equal(proof.executable_sha256, fixture.components.external.kubo.platforms[fixture.host].checksum);
  assert.match(proof.build_receipt_sha256, /^sha256:[0-9a-f]{64}$/);
});

for (const [name, mutate, message] of [
  ["missing build receipt", fixture => rmSync(join(fixture.data, "receipts/kubo-build.json")), /ENOENT/],
  ["wrong platform", fixture => { fixture.receipt.platform = "darwin-arm64"; fixture.save(); }, /darwin-arm64/],
  ["changed native executable", fixture => writeFileSync(join(fixture.data, "bin/kubo"), "corrupt Kubo executable"), /pinned (size|checksum)|licensed capsule/],
  ["changed native component checksum", fixture => { fixture.components.external.kubo.platforms[fixture.host].checksum = `sha256:${"c".repeat(64)}`; }, /native checksum/],
  ["changed license", fixture => writeFileSync(join(fixture.capsule, "LICENSE"), "changed licensed input"), /pinned (size|checksum)/],
  ["missing license record", fixture => { fixture.receipt.object_manifest = contentManifest(fixture.receipt.object_manifest.files.filter(file => file.path !== "LICENSE")); fixture.save(); }, /each pinned license/],
  ["stale build recipe provenance", fixture => { fixture.recipe.source.checksum = `sha256:${"d".repeat(64)}`; }, /current pinned build recipe/],
  ["changed capsule index", fixture => writeFileSync(join(fixture.capsule, "_elastos_object.json"), "{}"), /closure matches/],
  ["changed artifact marker", fixture => writeFileSync(join(fixture.capsule, ".elastos-artifact-sha256"), "wrong\n"), /artifact marker/],
]) {
  test(`Kubo verification refuses ${name}`, t => {
    const fixture = kuboFixture(t);
    mutate(fixture);
    assert.throws(fixture.check, message);
  });
}

test("production packages are deterministic and combine into one ordered payload", t => {
  const root = temporary(t), inputs = join(root, "inputs"), weights = Buffer.from("GGUF fake weights"), license = Buffer.from("Apache-2.0 text\n");
  mkdirSync(inputs);
  writeFileSync(join(inputs, "fake.gguf"), weights);
  writeFileSync(join(inputs, "LICENSE"), license);
  const pins = { model: { name: "fake.gguf", size: weights.length, sha256: sha(weights) }, license: { name: "LICENSE", size: license.length, sha256: sha(license) } };
  const smol = { ...SMOL_FIXTURE, ...pins }, qwen = { ...QWEN_FIXTURE, ...pins };
  const publisher = "did:key:z6MkgwHd2BCWe1jHMXPiR6H1q1RFPcv1YzhMbK5G1kBarbfe";
  // Stand-in for Kubo: each fixture's real package CID.
  const add = cid => () => cid;
  const QWEN_CID = "bafybeibijmrzexz5wumwq6e2kcnf7mxvoej3npgbjbhcmveqmz4yj54tlu", SMOL_CID = "bafybeiew3vuq32fvuz2kmps7lmgl4rxpvwsvklxykx4covogbrkhgy5qky";
  const runs = ["a", "b"].map(name => producePackage({ inputs, output: join(root, name), publisher, add: add(QWEN_CID), fixture: qwen }));
  assert.equal(runs[0].cid, runs[1].cid);
  assert.deepEqual(readFileSync(join(root, "a/entry.json")), readFileSync(join(root, "b/entry.json")));
  // Provenance names this fixture's own quantization and repositories.
  const provenance = readFileSync(join(runs[0].packageDir, "PROVENANCE.md"), "utf8");
  assert.match(provenance, new RegExp(publisher));
  assert.match(provenance, /Q4_K_M weights: Qwen\/Qwen2\.5-1\.5B-Instruct-GGUF at 91cad51170dc346986eccefdc2dd33a9da36ead9\./);
  assert.doesNotMatch(provenance, /Q8_0|SmolLM2/);
  assert.throws(() => producePackage({ inputs, output: join(root, "a"), publisher, add: add(QWEN_CID), fixture: qwen }), /EEXIST/);

  const first = producePackage({ inputs, output: join(root, "smol"), publisher, add: add(SMOL_CID), fixture: smol }).entry;
  mkdirSync(join(root, "catalog"));
  const payload = writeCatalogPayload({ output: join(root, "catalog"), publisher, entries: [first, runs[0].entry], publishedAt: 1700000000 });
  assert.deepEqual(JSON.parse(readFileSync(join(root, "catalog/payload.json"))), payload);
  assert.equal(payload.schema, "elastos.model.catalog/v1");
  assert.deepEqual(payload.entries.map(entry => entry.capsule_manifest.name), [SMOL_FIXTURE.capsule_manifest.name, QWEN_FIXTURE.capsule_manifest.name]);
  assert(!("expires_at" in payload));
  assert.throws(() => writeCatalogPayload({ output: join(root, "catalog"), publisher, entries: [first], publishedAt: 1 }), /EEXIST/);
  for (const entries of [[], [first, first], Array.from({ length: 9 }, (_, index) => ({ ...first, cid: `c${index}` }))]) {
    assert.throws(() => writeCatalogPayload({ output: join(root, `refused-${entries.length}`), publisher, entries, publishedAt: 1 }), /1 to 8|unique/);
  }
  // The signer's own check refuses what it would refuse at signing time,
  // before any payload is written.
  mkdirSync(join(root, "refused"));
  for (const [entry, message] of [
    [{ ...first, cid: rawCid(Buffer.from("raw leaf, not a package closure")) }, /DAG-PB/],
    [{ ...first, object_manifest: { ...first.object_manifest, content_digest: `sha256:${"0".repeat(64)}` } }, /digest mismatch/],
  ]) {
    assert.throws(() => writeCatalogPayload({ output: join(root, "refused"), publisher, entries: [entry, runs[0].entry], publishedAt: 1 }), message);
    assert.throws(() => readFileSync(join(root, "refused/payload.json")), /ENOENT/);
  }
});

test("download refuses a pre-placed symlink partial and leaves its target untouched", async t => {
  const root = temporary(t), inputs = join(root, "inputs"), victim = join(root, "victim");
  mkdirSync(inputs);
  writeFileSync(victim, "keep");
  symlinkSync(victim, join(inputs, "LICENSE.partial"));
  let fetched = false;
  const get = async () => { fetched = true; return new Response("replaced"); };
  await assert.rejects(download(SMOL_FIXTURE.license, join(inputs, "LICENSE"), get), /EEXIST/);
  assert.equal(readFileSync(victim, "utf8"), "keep");
  assert.equal(fetched, false);
});

test("output directory refuses a symlinked ancestor", t => {
  const root = temporary(t), real = join(root, "real");
  mkdirSync(real);
  symlinkSync(real, join(root, "link"));
  assert.throws(() => ownedDirectory(join(root, "link", "out")), /not a symlink/);
  assert.throws(() => lstatSync(join(real, "out")), /ENOENT/);
});

test("output directory refuses a group/other-writable OUT or ancestor before writing", t => {
  const root = temporary(t), shared = join(root, "shared");
  mkdirSync(shared);
  chmodSync(shared, 0o777);
  assert.throws(() => ownedDirectory(shared), /group\/other-writable/);
  assert.throws(() => ownedDirectory(join(shared, "out", "inputs")), /group\/other-writable/);
  assert.deepEqual(readdirSync(shared), []);
});
