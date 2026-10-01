#!/usr/bin/env node
// Isolated CI publisher: pinned model bytes, disposable signing authority,
// and the production Kubo import profile. Runtime owns package admission.
import assert from "node:assert/strict";
import { createHash, generateKeyPairSync, sign } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync, mkdirSync, copyFileSync, chmodSync } from "node:fs";
import { resolve, join, isAbsolute, sep } from "node:path";

const [dataArg, inputsArg, outputArg] = process.argv.slice(2);
const data = resolve(dataArg), inputs = resolve(inputsArg), output = resolve(outputArg);
const sha = bytes => createHash("sha256").update(bytes).digest("hex");
const canonical = value => JSON.stringify(sort(value));
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
const rawCid = bytes => "b" + encode(Buffer.concat([Buffer.from([1, 0x55, 0x12, 0x20]), Buffer.from(sha(bytes), "hex")]), "abcdefghijklmnopqrstuvwxyz234567", 5);
const entry = JSON.parse(readFileSync("model-catalog.json")).payload.entries.find(row => row.capsule_manifest.name.startsWith("smollm2-"));
assert(entry, "source catalog supplies SmolLM2 provenance");
const model = join(inputs, "SmolLM2-135M-Instruct-Q8_0.gguf");
const license = readFileSync(join(inputs, "LICENSE"));
assert.equal(sha(readFileSync(model)), "c4a3dd037301b6ecea31d6da37f5cd793ead920dd5ddfe6d589294628d6ce66a");
assert.equal(sha(license), "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30");
const packageDir = join(output, "package");
mkdirSync(packageDir, { recursive: true, mode: 0o700 });
const files = {
  "LICENSE": license, "LICENSE.base": license,
  "PROVENANCE.md": Buffer.from("Isolated CI publisher attestation. Weights match the pinned unsloth Q8_0 quantization of HuggingFaceTB/SmolLM2-135M-Instruct. The source catalog records repository revisions and Apache-2.0 licensing. This fixture uses its own temporary publisher key.\n"),
  "capsule.json": Buffer.from(canonical(entry.capsule_manifest)),
};
for (const [name, bytes] of Object.entries(files)) writeFileSync(join(packageDir, name), bytes, { mode: 0o400 });
copyFileSync(model, join(packageDir, "weights.gguf"));
chmodSync(join(packageDir, "weights.gguf"), 0o400);
const digest = createHash("sha256");
for (const file of entry.object_manifest.files) {
  if (files[file.path]) { file.size = files[file.path].length; file.sha256 = sha(files[file.path]); }
  digest.update(`${file.path}\0${file.sha256}\0${file.size}\0`);
}
entry.object_manifest.content_digest = `sha256:${digest.digest("hex")}`;
writeFileSync(join(packageDir, "_elastos_object.json"), canonical(entry.object_manifest), { mode: 0o400 });
const manifestBytes = readFileSync(join(data, "components.json"));
const components = JSON.parse(manifestBytes);
const host = ({ "linux-x64": "linux-amd64", "linux-arm64": "linux-arm64", "darwin-arm64": "darwin-arm64" })[`${process.platform}-${process.arch}`];
assert(host, "supported installed Kubo platform required");
const component = components.external.kubo, info = component.platforms[host];
assert(info, "installed manifest declares Kubo for this platform");
const sourceManifestBytes = readFileSync("components.json");
const sourceInfo = JSON.parse(sourceManifestBytes).external.kubo.platforms[host];
for (const field of ["url", "checksum", "extract_path"]) assert.equal(info[field], sourceInfo[field], `Kubo ${field} matches the archive-verifying seed contract`);
const installPath = info.install_path ?? component.install_path;
assert(typeof installPath === "string" && installPath && !isAbsolute(installPath) && !installPath.split(/[\\/]/).includes(".."), "Kubo install_path stays inside the isolated data root");
const kubo = resolve(data, installPath), repo = join(data, "ipfs-repo");
assert(kubo.startsWith(data + sep), "declared Kubo executable stays inside the data root");
const kuboReceipt = { platform: host, install_path: installPath, archive_checksum: info.checksum, executable_sha256: `sha256:${sha(readFileSync(kubo))}`, components_sha256: `sha256:${sha(manifestBytes)}`, source_components_sha256: `sha256:${sha(sourceManifestBytes)}` };
const run = args => execFileSync(kubo, args, { env: { ...process.env, IPFS_PATH: repo }, encoding: "utf8", maxBuffer: 1024 * 1024 }).trim();
run(["init", "--empty-repo"]);
for (const profile of ["test", "autoconf-off", "announce-off"]) run(["config", "profile", "apply", profile]);
run(["config", "Addresses.API", "/ip4/127.0.0.1/tcp/0"]);
entry.cid = run(["add", "--offline", "--recursive=true", "--quieter=true", "--wrap-with-directory=false", "--cid-version=1", "--hash=sha2-256", "--raw-leaves=true", "--chunker=size-262144", "--trickle=false", "--max-file-links=174", "--max-directory-links=0", "--max-hamt-fanout=256", "--inline=false", "--nocopy=false", "--fscache=false", "--preserve-mode=false", "--preserve-mtime=false", "--empty-dirs=false", "--progress=false", "--fast-provide-root=false", "--fast-provide-wait=false", packageDir]).split("\n").at(-1);
assert.match(entry.cid, /^bafy[a-z2-7]+$/);
const payload = { schema: "elastos.model.catalog/v1", published_at: Math.floor(Date.now() / 1000), entries: [entry] };
const { privateKey, publicKey } = generateKeyPairSync("ed25519");
const publicBytes = publicKey.export({ type: "spki", format: "der" }).subarray(-32);
const signer_did = "did:key:z" + encode(Buffer.concat([Buffer.from([0xed, 1]), publicBytes]), "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz");
const message = createHash("sha256").update("elastos.model.catalog.v1\0").update(canonical(payload)).digest();
const catalog = Buffer.from(canonical({ payload, signer_did, signature: sign(null, message, privateKey).toString("hex") }));
writeFileSync(join(data, "model-catalog.json"), catalog, { mode: 0o600 });
components.model_catalog = { head_cid: rawCid(catalog), publisher_dids: [signer_did], local_use: { max_cache_bytes: 1024 ** 3, max_model_memory_bytes: 4 * 1024 ** 3 } };
writeFileSync(join(data, "components.json"), JSON.stringify(components), { mode: 0o600 });
writeFileSync(join(output, "package.json"), JSON.stringify({ cid: entry.cid, model_sha256: entry.object_manifest.files.find(f => f.path === "weights.gguf").sha256, publisher: "disposable CI fixture", delivery: "local pinned Kubo package; Runtime Use admission", kubo: kuboReceipt }, null, 2));
