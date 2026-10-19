import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = fileURLToPath(new URL("../", import.meta.url));
const chat = "capsules/chat-room/browser/chat-room.js";
// Every file the gate reads. The gate scans only capsules/*/browser, so this
// tree holds the one trusted host and the capsules that must use the client.
const files = [
  "scripts/home-clipboard-source-gate.mjs",
  "capsules/home/browser/home-clipboard-host.js",
  "capsules/home/browser/home-clipboard-client.js",
  "capsules/home/browser/home-clipboard-protocol.js",
  "capsules/browser/browser/browser.js",
  "capsules/wallet/browser/wallet.js",
  "capsules/wallet-metamask/browser/wallet-metamask.js",
  "capsules/wallet-unisat/browser/wallet-unisat.js",
  "capsules/wallet-walletconnect/browser/wallet-walletconnect.js",
  "capsules/library/browser/src/app.js",
  "capsules/library/browser/src/actions.js",
  "capsules/library/browser/src/dialog.js",
  "capsules/documents/browser/documents.js",
  "capsules/system/browser/system.js",
  chat,
  "capsules/chat-room-ui/src/lib.rs",
];

function gate(t, change) {
  const root = mkdtempSync(join(tmpdir(), "clipboard-gate-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (const file of files) {
    mkdirSync(dirname(join(root, file)), { recursive: true });
    copyFileSync(join(repoRoot, file), join(root, file));
  }
  if (change) {
    const source = readFileSync(join(root, chat), "utf8");
    const changed = change(source);
    assert.notEqual(changed, source, "the fixture change applies");
    writeFileSync(join(root, chat), changed);
  }
  return spawnSync(process.execPath, [join(root, "scripts/home-clipboard-source-gate.mjs")], { encoding: "utf8" });
}

test("Chat's renewable Home Clipboard client passes the gate", (t) => {
  const result = gate(t);
  assert.equal(result.status, 0, result.stderr);
  assert.ok(JSON.parse(result.stdout).migrated_capsules.includes(chat));
});

for (const [name, change, message] of [
  ["a different target", (source) => source.replace('targetId: "chat-room"', 'targetId: "home"'), /fixed target binding/],
  ["a second target", (source) => source.replace('targetId: "chat-room",', 'targetId: "chat-room",\n        other: { targetId: "home" },'), /fixed target binding/],
  ["no client start", (source) => source.replaceAll("homeClipboard.start()", "homeClipboard.ready()"), /fixed target binding/],
  ["no canonical client", (source) => source.replaceAll("createHomeClipboardClient(", "createLocalClipboard("), /fixed target binding/],
  ["direct navigator.clipboard", (source) => source + "\nnavigator.clipboard.writeText('x');\n", /Only the trusted top-level Home Clipboard host/],
]) {
  test(`the gate refuses Chat with ${name}`, (t) => {
    const result = gate(t, change);
    assert.notEqual(result.status, 0, result.stdout);
    assert.match(result.stderr, message);
  });
}
