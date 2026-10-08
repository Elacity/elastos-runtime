#!/usr/bin/env node
// Isolated CI publisher: pinned model bytes, disposable signing authority,
// and the production Kubo import profile. Runtime owns package admission.
import assert from "node:assert/strict";
import { createHash, generateKeyPairSync, sign } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync, mkdirSync, copyFileSync, chmodSync, lstatSync, openSync, readSync, closeSync, existsSync, createWriteStream, renameSync, mkdtempSync, rmSync, constants } from "node:fs";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { resolve, join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const sha = bytes => createHash("sha256").update(bytes).digest("hex");
export const canonical = value => JSON.stringify(sort(value));
function sort(value) {
  if (Array.isArray(value)) return value.map(sort);
  if (value && typeof value === "object") return Object.fromEntries(Object.keys(value).sort().map(k => [k, sort(value[k])]));
  return value;
}
function encode(bytes, alphabet, bits) {
  if (bits) {
    let result = "", accumulator = 0, count = 0;
    for (const byte of bytes) {
      accumulator = (accumulator << 8) | byte; count += 8;
      while (count >= bits) { count -= bits; result += alphabet[(accumulator >>> count) & 31]; }
    }
    if (count) result += alphabet[(accumulator << (bits - count)) & 31];
    return result;
  }
  let number = BigInt(`0x${bytes.toString("hex")}`), result = "";
  while (number) { result = alphabet[Number(number % 58n)] + result; number /= 58n; }
  for (const byte of bytes) { if (byte) break; result = alphabet[0] + result; }
  return result;
}
export const rawCid = bytes => "b" + encode(Buffer.concat([Buffer.from([1, 0x55, 0x12, 0x20]), Buffer.from(sha(bytes), "hex")]), "abcdefghijklmnopqrstuvwxyz234567", 5);

// The producer and build-only input proof share this fixture's byte pins and
// provenance independently of any production catalog.
export const SMOL_FIXTURE = JSON.parse(readFileSync(new URL("./pinned-smollm2-fixture.json", import.meta.url), "utf8"));

function regular(path) {
  for (let current = resolve(path);; current = dirname(current)) {
    assert(!lstatSync(current).isSymbolicLink(), "fixture input path contains a symlink");
    if (current === dirname(current)) break;
  }
  const stat = lstatSync(path);
  assert(stat.isFile() && stat.nlink === 1, "fixture input must be a single-link regular file");
  return stat;
}

export function fileRecord(path, name, expected) {
  const stat = regular(path);
  if (expected) assert.equal(stat.size, expected.size, `${name} differs from its pinned size`);
  const fd = openSync(path, "r"), value = createHash("sha256"), chunk = Buffer.alloc(1024 * 1024);
  try {
    for (let count; (count = readSync(fd, chunk)) > 0;) value.update(chunk.subarray(0, count));
  } finally { closeSync(fd); }
  const record = { path: name, size: stat.size, sha256: value.digest("hex") };
  if (expected) assert.equal(record.sha256, expected.sha256, `${name} differs from its pinned checksum`);
  return record;
}

export function contentManifest(files) {
  const sorted = [...files].sort((a, b) => a.path < b.path ? -1 : a.path > b.path ? 1 : 0);
  const digest = createHash("sha256");
  for (const file of sorted) digest.update(`${file.path}\0${file.sha256}\0${file.size}\0`);
  return { schema: "elastos.content.object.manifest/v1", kind: "capsule", files: sorted, content_digest: `sha256:${digest.digest("hex")}` };
}

// The publisher named in PROVENANCE.md is the closure's attesting publisher:
// the CI fixture uses a disposable key; production names the maintainer DID.
export function buildSmolEntry(inputs, packageDir, publisher = null, fixture = SMOL_FIXTURE) {
  const model = join(inputs, fixture.model.name);
  fileRecord(model, "weights.gguf", fixture.model);
  fileRecord(join(inputs, "LICENSE"), "LICENSE", fixture.license);
  const license = readFileSync(join(inputs, "LICENSE"));
  const provenance = fixture.capsule_manifest.model_content.provenance;
  const attestation = publisher ? `Publisher attestation by ${publisher}.` : "Isolated CI publisher attestation.";
  const keyNote = publisher ? "" : " This fixture uses an in-memory disposable publisher key.";
  const files = {
    LICENSE: license, "LICENSE.base": license,
    "PROVENANCE.md": Buffer.from(`${attestation} Apache-2.0 base: ${provenance.base_repository} at ${provenance.base_revision}. Q8_0 weights: ${provenance.quantized_repository} at ${provenance.quantized_revision}. Weights SHA-256: ${fixture.model.sha256}.${keyNote}\n`),
    "capsule.json": Buffer.from(canonical(fixture.capsule_manifest)),
  };
  mkdirSync(packageDir, { mode: 0o700 });
  for (const [name, bytes] of Object.entries(files)) writeFileSync(join(packageDir, name), bytes, { mode: 0o400, flag: "wx" });
  copyFileSync(model, join(packageDir, "weights.gguf"));
  chmodSync(join(packageDir, "weights.gguf"), 0o400);
  // Check the copied bytes before committing their complete closure metadata.
  const weights = fileRecord(join(packageDir, "weights.gguf"), "weights.gguf", fixture.model);
  const object_manifest = contentManifest([
    ...Object.entries(files).map(([path, bytes]) => ({ path, size: bytes.length, sha256: sha(bytes) })), weights,
  ]);
  writeFileSync(join(packageDir, "_elastos_object.json"), canonical(object_manifest), { mode: 0o400, flag: "wx" });
  return { capsule_manifest: structuredClone(fixture.capsule_manifest), object_manifest };
}

export function verifyInstalledKubo(data, components, host, recipe) {
  const component = components.external?.kubo, info = component?.platforms?.[host];
  assert(info, "installed manifest declares Kubo for this platform");
  const receiptPath = join(data, "receipts/kubo-build.json");
  regular(receiptPath);
  const receiptBytes = readFileSync(receiptPath), receipt = JSON.parse(receiptBytes);
  assert.equal(receipt.schema, "elastos.release-upstream-input/v1");
  assert.equal(receipt.component, "kubo");
  assert.equal(receipt.platform, host);
  for (const field of ["extract_path", "install_path"]) assert.equal(receipt[field], recipe[field], `Kubo receipt ${field} matches the build recipe`);
  assert.equal(receipt.release_path, `kubo-${host}.tar.gz`);
  assert.match(receipt.checksum, /^sha256:[0-9a-f]{64}$/);
  assert.deepEqual(component.capsule_metadata?.platforms?.[host], receipt.capsule_metadata, "installed Kubo capsule metadata matches its build receipt");
  assert.equal(component.capsule_metadata.install_path, "capsules/kubo");
  assert.equal(receipt.capsule_metadata.checksum, receipt.checksum);
  assert.equal(receipt.capsule_metadata.install_path, "capsules/kubo");
  assert.equal(receipt.capsule_metadata.extract_path, recipe.root);
  assert.equal(receipt.capsule_metadata.release_path, receipt.release_path);
  assert.equal(receipt.capsule_metadata.size, receipt.size);
  const capsule = join(data, "capsules/kubo");
  regular(join(capsule, ".elastos-artifact-sha256"));
  assert.equal(readFileSync(join(capsule, ".elastos-artifact-sha256"), "utf8"), receipt.checksum.slice(7) + "\n", "Kubo artifact marker matches its build receipt");
  const indexPath = join(capsule, "_elastos_object.json");
  regular(indexPath);
  assert.deepEqual(JSON.parse(readFileSync(indexPath)), receipt.object_manifest, "Kubo closure matches its build receipt");
  const files = receipt.object_manifest.files;
  assert(Array.isArray(files) && files.length > 0 && files.length <= 4096, "Kubo closure has a bounded file set");
  const names = new Set();
  for (const file of files) {
    assert(/^[A-Za-z0-9][A-Za-z0-9._+-]*(\/[A-Za-z0-9][A-Za-z0-9._+-]*)*$/.test(file.path) && !names.has(file.path), "Kubo closure requires unique portable file paths");
    names.add(file.path);
    fileRecord(join(capsule, file.path), file.path, file);
  }
  assert.deepEqual(contentManifest(files), receipt.object_manifest, "Kubo closure digest matches its complete file records");
  assert.deepEqual(JSON.parse(readFileSync(join(capsule, "capsule.json"))), receipt.capsule_manifest, "Kubo capsule manifest matches its build receipt");
  assert.deepEqual(receipt.capsule_manifest, {
    schema: "elastos.capsule/v1", name: recipe.component, version: recipe.version,
    role: "content", type: "data", projections: ["content"], entrypoint: recipe.entrypoint,
  }, "Kubo passive capsule manifest matches its build recipe");
  const provenance = JSON.parse(readFileSync(join(capsule, "PROVENANCE.json")));
  assert.equal(provenance.recipe_sha256, sha(Buffer.from(canonical(recipe) + "\n")), "Kubo provenance binds the current pinned build recipe");
  assert.deepEqual(provenance.upstream, recipe.source);
  for (const notice of [...recipe.license.files, ...(recipe.notices || [])]) {
    const file = files.find(file => file.path === notice.name);
    assert(file, "Kubo closure includes each pinned license notice");
    assert.equal(`sha256:${file.sha256}`, notice.source.checksum, "Kubo license bytes match the build recipe");
  }
  const installPath = "bin/kubo";
  assert.equal(info.install_path, installPath);
  const executable = fileRecord(join(data, installPath), recipe.entrypoint);
  const capsuleExecutable = files.find(file => file.path === recipe.entrypoint);
  assert(capsuleExecutable, "Kubo closure includes its executable");
  assert.deepEqual(executable, capsuleExecutable, "installed Kubo executable matches its licensed capsule");
  assert.equal(info.checksum, `sha256:${executable.sha256}`, "installed Kubo native checksum matches components");
  assert.equal(info.size, executable.size);
  return { platform: host, install_path: installPath, archive_checksum: receipt.checksum, executable_sha256: info.checksum, build_receipt_sha256: `sha256:${sha(receiptBytes)}` };
}

export function signedCatalog(entry, publishedAt = Math.floor(Date.now() / 1000)) {
  const payload = { schema: "elastos.model.catalog/v1", published_at: publishedAt, entries: [entry] };
  const { privateKey, publicKey } = generateKeyPairSync("ed25519");
  const publicBytes = publicKey.export({ type: "spki", format: "der" }).subarray(-32);
  const signer_did = "did:key:z" + encode(Buffer.concat([Buffer.from([0xed, 1]), publicBytes]), "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz");
  const message = createHash("sha256").update("elastos.model.catalog.v1\0").update(canonical(payload)).digest();
  const catalog = Buffer.from(canonical({ payload, signer_did, signature: sign(null, message, privateKey).toString("hex") }));
  return { catalog, trust: { head_cid: rawCid(catalog), publisher_dids: [signer_did], local_use: { max_cache_bytes: 1024 ** 3, max_model_memory_bytes: 4 * 1024 ** 3 } } };
}

// Production Kubo import profile; the package CID only reproduces with these flags.
export const KUBO_ADD_FLAGS = ["--offline", "--recursive=true", "--quieter=true", "--wrap-with-directory=false", "--cid-version=1", "--hash=sha2-256", "--raw-leaves=true", "--chunker=size-262144", "--trickle=false", "--max-file-links=174", "--max-directory-links=0", "--max-hamt-fanout=256", "--inline=false", "--nocopy=false", "--fscache=false", "--preserve-mode=false", "--preserve-mtime=false", "--empty-dirs=false", "--progress=false", "--fast-provide-root=false", "--fast-provide-wait=false"];

// Adds one directory to a fresh offline repo and returns its root CID.
export function kuboAdd(kubo, repo, dir) {
  const run = args => execFileSync(kubo, args, { env: { ...process.env, IPFS_PATH: repo }, encoding: "utf8", maxBuffer: 1024 * 1024, timeout: 600000 }).trim();
  run(["init", "--empty-repo"]);
  for (const profile of ["test", "autoconf-off", "announce-off"]) run(["config", "profile", "apply", profile]);
  run(["config", "Addresses.API", "/ip4/127.0.0.1/tcp/0"]);
  const cid = run(["add", ...KUBO_ADD_FLAGS, dir]).split("\n").at(-1);
  assert.match(cid, /^bafy[a-z2-7]+$/);
  return cid;
}

// Unsigned production payload for `release-signer.py --model-catalog`.
export function produceCatalogPayload({ inputs, output, publisher, add, fixture = SMOL_FIXTURE, publishedAt = Math.floor(Date.now() / 1000) }) {
  assert.match(publisher, /^did:key:z[1-9A-HJ-NP-Za-km-z]+$/, "publisher must be a did:key");
  mkdirSync(output, { recursive: true, mode: 0o700 });
  const packageDir = join(output, "package"), entry = buildSmolEntry(inputs, packageDir, publisher, fixture);
  entry.cid = add(packageDir);
  const payload = { schema: "elastos.model.catalog/v1", published_at: publishedAt, entries: [entry] };
  writeFileSync(join(output, "payload.json"), canonical(payload) + "\n", { mode: 0o600, flag: "wx" });
  return { cid: entry.cid, packageDir, payload };
}

// Creates `path` (and parents) as directories with no symlink anywhere on the
// path; the leaf must be ours so nobody else can swap entries inside it.
export function ownedDirectory(path) {
  const check = () => {
    for (let current = resolve(path);; current = dirname(current)) {
      const stat = lstatSync(current, { throwIfNoEntry: false });
      assert(!stat || stat.isDirectory(), `${current} must be a real directory, not a symlink`);
      if (current === dirname(current)) break;
    }
  };
  check();
  mkdirSync(path, { recursive: true, mode: 0o700 });
  check();
  assert.equal(lstatSync(path).uid, process.getuid(), `${path} must be owned by the current user`);
  return path;
}

// The partial file is created exclusively without following links, before any
// network read; a pre-placed partial or symlink is refused and left untouched.
export async function download(source, path, get = fetch) {
  if (existsSync(path)) return fileRecord(path, source.name, source);
  const pending = `${path}.partial`;
  const fd = openSync(pending, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
  try {
    const response = await get(source.url);
    assert(response.ok, `download ${source.url} failed: ${response.status}`);
    await pipeline(Readable.fromWeb(response.body), createWriteStream(null, { fd }));
    fileRecord(pending, source.name, source);
    renameSync(pending, path);
  } catch (error) {
    rmSync(pending, { force: true });
    throw error;
  }
}

// usage: produce <output-dir> <publisher-did> <kubo-data>
// <kubo-data> is a data dir prepared by scripts/seed-kubo-cache.sh (pinned recipe Kubo).
async function produce(args) {
  assert.equal(args.length, 3, "usage: ci-model-package.mjs produce <output-dir> <publisher-did> <kubo-data>");
  const output = resolve(args[0]), publisher = args[1], data = resolve(args[2]);
  const receipt = JSON.parse(readFileSync(join(data, "receipts/kubo-build.json")));
  const entrypoint = receipt.object_manifest.files.find(file => file.path === receipt.capsule_manifest.entrypoint);
  const kubo = join(data, "bin/kubo");
  assert.deepEqual(fileRecord(kubo, entrypoint.path), entrypoint, "Kubo binary matches its pinned recipe build");
  const inputs = ownedDirectory(join(ownedDirectory(output), "inputs"));
  await download(SMOL_FIXTURE.model, join(inputs, SMOL_FIXTURE.model.name));
  await download(SMOL_FIXTURE.license, join(inputs, "LICENSE"));
  const repo = mkdtempSync(join(output, ".ipfs-repo-"));
  try {
    const { cid, packageDir } = produceCatalogPayload({ inputs, output, publisher, add: dir => kuboAdd(kubo, repo, dir) });
    console.log(`package_cid ${cid}\npackage_dir ${packageDir}\npayload ${join(output, "payload.json")}`);
  } finally { rmSync(repo, { recursive: true, force: true }); }
}

function main(args) {
  if (args[0] === "produce") return produce(args.slice(1));
  assert.equal(args.length, 3, "usage: ci-model-package.mjs <isolated-data> <pinned-inputs> <fixture-output> | produce <output-dir> <publisher-did> <kubo-data>");
  const [data, inputs, output] = args.map(arg => resolve(arg));
  const manifestPath = join(data, "components.json");
  regular(manifestPath);
  const manifestBytes = readFileSync(manifestPath), components = JSON.parse(manifestBytes);
  const host = ({ "linux-x64": "linux-amd64", "linux-arm64": "linux-arm64", "darwin-arm64": "darwin-arm64" })[`${process.platform}-${process.arch}`];
  assert(host, "supported installed Kubo platform required");
  const recipes = JSON.parse(readFileSync(join(root, "scripts/release-upstream-recipes.json"))).recipes.filter(row => row.component === "kubo" && row.platform === host);
  assert.equal(recipes.length, 1, "Kubo requires one pinned build recipe for this platform");
  const kuboReceipt = verifyInstalledKubo(data, components, host, recipes[0]);
  kuboReceipt.components_sha256 = `sha256:${sha(manifestBytes)}`;
  mkdirSync(output, { recursive: true, mode: 0o700 });
  const packageDir = join(output, "package"), entry = buildSmolEntry(inputs, packageDir);
  entry.cid = kuboAdd(join(data, "bin/kubo"), join(data, "ipfs-repo"), packageDir);
  const { catalog, trust } = signedCatalog(entry);
  writeFileSync(join(data, "model-catalog.json"), catalog, { mode: 0o600 });
  components.model_catalog = trust;
  writeFileSync(manifestPath, JSON.stringify(components), { mode: 0o600 });
  writeFileSync(join(output, "package.json"), JSON.stringify({ cid: entry.cid, model_sha256: SMOL_FIXTURE.model.sha256, publisher: "disposable CI fixture", delivery: "local pinned Kubo package; Runtime Use admission", kubo: kuboReceipt }, null, 2));
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) await main(process.argv.slice(2));
