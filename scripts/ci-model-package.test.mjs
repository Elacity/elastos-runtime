import assert from "node:assert/strict";
import test from "node:test";
import { createHash, createPublicKey, verify } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildSmolEntry, canonical, contentManifest, fileRecord, rawCid, signedCatalog, SMOL_FIXTURE, verifyInstalledKubo } from "./ci-model-package.mjs";

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

test("Smol producer refuses absent or truncated model before writing a package", t => {
  const root = temporary(t), packageDir = join(root, "package");
  assert.throws(() => buildSmolEntry(root, packageDir), /ENOENT/);
  writeFileSync(join(root, SMOL_FIXTURE.model.name), "GGUF");
  assert.throws(() => buildSmolEntry(root, packageDir), /pinned size/);
  assert.throws(() => readFileSync(join(packageDir, "capsule.json")), /ENOENT/);
});

test("fixture fetch selects its declared immutable provenance and licensing", () => {
  const manifest = SMOL_FIXTURE.capsule_manifest, model = manifest.model_content;
  assert.equal(manifest.role, "content");
  assert.deepEqual(manifest.projections, ["content"]);
  assert.match(model.provenance.base_revision, /^[0-9a-f]{40}$/);
  assert.match(model.provenance.quantized_revision, /^[0-9a-f]{40}$/);
  const url = new URL(SMOL_FIXTURE.model.url);
  assert.equal(url.origin, "https://huggingface.co");
  assert.equal(url.pathname, `/${model.provenance.quantized_repository}/resolve/${model.provenance.quantized_revision}/${SMOL_FIXTURE.model.name}`);
  assert.equal(model.license.spdx_id, "Apache-2.0");
  assert.equal(model.license.path, SMOL_FIXTURE.license.name);
  assert.equal(model.provenance.base_license.path, "LICENSE.base");
  assert.equal(model.provenance.base_license.spdx_id, model.license.spdx_id);
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
