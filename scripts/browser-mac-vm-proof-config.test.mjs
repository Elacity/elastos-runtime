import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const proofPath = fileURLToPath(new URL("./browser-mac-vm-proof.sh", import.meta.url));
const generatorPath = fileURLToPath(new URL("./browser-source-home-config.mjs", import.meta.url));
const configPath = dataDir => path.join(dataDir, "config/browser-engine-adapter.json");
const adapter = socket => ({ id: "browser-vm-product", supervisor: { control_socket_path: socket } });

function fixture(t) {
  const root = mkdtempSync(path.join(os.tmpdir(), "browser-proof-config-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  return root;
}

function probe(dataDir, override = "") {
  // Run the actual consumer with inert HTTP and stop at its first socket probe,
  // before either virtual-auth journey can run.
  return spawnSync("bash", ["-c", `
exec 3>&2
curl() {
  while [[ $# -gt 0 ]]; do
    if [[ "$1" == "--unix-socket" ]]; then
      printf 'CONTROL_SOCKET=%s\\n' "$2" >&3
      exit 77
    fi
    shift
  done
  printf 'HOME_PROBE\\n' >&2
  printf '200'
}
export -f curl
exec bash "$1"
`, "proof-config-test", proofPath], {
    cwd: path.dirname(path.dirname(proofPath)),
    env: { ...process.env, ELASTOS_NODE_BIN: process.execPath,
      ELASTOS_MAC_BROWSER_DATA_DIR: dataDir, ELASTOS_BROWSER_VM_CONTROL_SOCKET: override,
      ELASTOS_BROWSER_MAC_VM_PROOF_AUTH_PROFILE: "" },
    encoding: "utf8", timeout: 10_000,
  });
}

function expectSocket(result, socket) {
  assert.equal(result.error, undefined);
  assert.equal(result.status, 77, result.stderr);
  assert.ok(result.stderr.includes(`CONTROL_SOCKET=${socket}\n`), result.stderr);
}

test("Mac proof follows each target's generated socket and selects the product adapter by ID", t => {
  const root = fixture(t);
  const sockets = [];
  for (const home of ["Home One", "Home Two"]) {
    const dataDir = path.join(root, home, "Library/Application Support/elastos");
    execFileSync(process.execPath, [generatorPath, "--data-dir", dataDir, "--platform", "darwin-arm64"]);
    const config = JSON.parse(readFileSync(configPath(dataDir), "utf8"));
    const socket = config.adapters.find(entry => entry.id === "browser-vm-product").supervisor.control_socket_path;
    config.adapters.unshift({ id: "other-engine", supervisor: { control_socket_path: "/tmp/foreign-engine.sock" } });
    writeFileSync(configPath(dataDir), JSON.stringify(config));
    expectSocket(probe(dataDir), socket);
    sockets.push(socket);
  }
  assert.notEqual(sockets[0], sockets[1]);
});

test("explicit socket override wins and works without target config", t => {
  const root = fixture(t);
  const socket = path.join(root, "explicit control.sock");
  expectSocket(probe(root, socket), socket);
  mkdirSync(path.dirname(configPath(root)), { recursive: true });
  writeFileSync(configPath(root), JSON.stringify({ adapters: [adapter("/tmp/configured.sock")] }));
  expectSocket(probe(root, socket), socket);
});

test("missing, malformed, ambiguous or invalid target config fails before Home is contacted", t => {
  const root = fixture(t);
  const cases = [
    undefined, "{broken-json", {},
    { adapters: [adapter("/tmp/one.sock"), adapter("/tmp/two.sock")] },
    { adapters: [adapter(undefined)] },
    { adapters: [adapter("relative.sock")] },
    { adapters: [adapter("/tmp/control.sock\n")] },
  ];
  for (const [index, config] of cases.entries()) {
    const dataDir = path.join(root, String(index));
    if (config !== undefined) {
      mkdirSync(path.dirname(configPath(dataDir)), { recursive: true });
      writeFileSync(configPath(dataDir), typeof config === "string" ? config : JSON.stringify(config));
    }
    const result = probe(dataDir);
    assert.equal(result.status, 2, result.stderr);
    assert.match(result.stderr, /Cannot resolve Browser VM control socket/);
    assert.doesNotMatch(result.stderr, /HOME_PROBE|CONTROL_SOCKET=/);
  }
});
