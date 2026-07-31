#!/usr/bin/env node

import { readFile } from "node:fs/promises";
import path from "node:path";
import { openCdpClient } from "../../lib/cdp-client.mjs";

const profileDir = process.env.ELASTOS_GBA_PROFILE_DIR;
const serverPort = process.env.ELASTOS_GBA_SERVER_PORT;
if (!profileDir || !serverPort) throw new Error("GBA proof ports were not provided");

const deadline = Date.now() + 20_000;
let target;
let debuggingPort = "";
while (Date.now() < deadline) {
  try {
    if (!debuggingPort) {
      debuggingPort = (await readFile(path.join(profileDir, "DevToolsActivePort"), "utf8"))
        .split("\n", 1)[0]
        .trim();
    }
    const targets = await fetch(`http://127.0.0.1:${debuggingPort}/json/list`)
      .then((response) => response.json());
    target = targets.find((item) => item.url?.startsWith(`http://127.0.0.1:${serverPort}/`));
    if (target?.webSocketDebuggerUrl) break;
  } catch {}
  await new Promise((resolve) => setTimeout(resolve, 50));
}
if (!target?.webSocketDebuggerUrl) throw new Error("GBA Chromium target was not found");

const pageClient = await openCdpClient(target.webSocketDebuggerUrl);
const send = pageClient.send;

async function evaluate(expression) {
  const result = await send("Runtime.evaluate", { expression, returnByValue: true });
  return result.result.value;
}

while (Date.now() < deadline) {
  if (await evaluate("document.body?.dataset?.gbaReady === 'true'")) break;
  await new Promise((resolve) => setTimeout(resolve, 50));
}
if (!(await evaluate("document.body?.dataset?.gbaReady === 'true'"))) {
  throw new Error("opaque GBA frame was not ready");
}
if (!(await evaluate("document.querySelector('#gba-frame')?.focus(); document.activeElement?.id === 'gba-frame'"))) {
  throw new Error("opaque GBA frame could not receive trusted input");
}

let frameTarget;
while (!frameTarget && Date.now() < deadline) {
  const targets = await fetch(`http://127.0.0.1:${debuggingPort}/json/list`)
    .then((response) => response.json());
  frameTarget = targets.find((item) => {
    try {
      return new URL(item.url).pathname === "/apps/gba-emulator/"
        && Boolean(item.webSocketDebuggerUrl);
    } catch {
      return false;
    }
  });
  if (!frameTarget) await new Promise((resolve) => setTimeout(resolve, 25));
}
if (!frameTarget) throw new Error("opaque GBA execution target was not found");
const frameClient = await openCdpClient(frameTarget.webSocketDebuggerUrl);
const readTrustedInput = async () => {
  const result = await frameClient.send("Runtime.evaluate", {
    expression: `(() => {
      const input = window.__elastosGbaTrustedInput;
      const pressed = document.querySelector('[data-key="a"]')?.classList.contains("pressed") === true;
      const startPressed = document.querySelector('[data-key="start"]')?.classList.contains("pressed") === true;
      if (input?.keydown_trusted && !input.keyup_trusted && pressed) input.pressed = true;
      if (input?.keyup_trusted && !pressed) input.released = true;
      if (input?.start_keydown_trusted && !input.start_keyup_trusted && startPressed) {
        input.start_pressed = true;
      }
      if (input?.start_keyup_trusted && !startPressed) input.start_released = true;
      return { ...input };
    })()`,
    returnByValue: true,
  });
  return result.result?.value || {};
};

if (!(await evaluate("document.querySelector('#gba-frame')?.focus(); document.activeElement?.id === 'gba-frame'"))) {
  throw new Error("opaque GBA frame lost focus before trusted input");
}

await send("Input.dispatchKeyEvent", {
  type: "keyDown",
  key: "x",
  code: "KeyX",
  windowsVirtualKeyCode: 88,
  nativeVirtualKeyCode: 88,
});
const pressDeadline = Date.now() + 5_000;
let trustedInput = await readTrustedInput();
while (!(trustedInput.keydown_trusted && trustedInput.pressed) && Date.now() < pressDeadline) {
  await new Promise((resolve) => setTimeout(resolve, 10));
  trustedInput = await readTrustedInput();
}
await send("Input.dispatchKeyEvent", {
  type: "keyUp",
  key: "x",
  code: "KeyX",
  windowsVirtualKeyCode: 88,
  nativeVirtualKeyCode: 88,
});
const releaseDeadline = Date.now() + 5_000;
trustedInput = await readTrustedInput();
while (!(trustedInput.keyup_trusted && trustedInput.released) && Date.now() < releaseDeadline) {
  await new Promise((resolve) => setTimeout(resolve, 10));
  trustedInput = await readTrustedInput();
}
await send("Input.dispatchKeyEvent", {
  type: "keyDown",
  key: "Enter",
  code: "Enter",
  windowsVirtualKeyCode: 13,
  nativeVirtualKeyCode: 13,
});
const startPressDeadline = Date.now() + 5_000;
trustedInput = await readTrustedInput();
while (!(trustedInput.start_keydown_trusted && trustedInput.start_pressed) &&
    Date.now() < startPressDeadline) {
  await new Promise((resolve) => setTimeout(resolve, 10));
  trustedInput = await readTrustedInput();
}
await new Promise((resolve) => setTimeout(resolve, 150));
await send("Input.dispatchKeyEvent", {
  type: "keyUp",
  key: "Enter",
  code: "Enter",
  windowsVirtualKeyCode: 13,
  nativeVirtualKeyCode: 13,
});
const startReleaseDeadline = Date.now() + 5_000;
trustedInput = await readTrustedInput();
while (!(trustedInput.start_keyup_trusted && trustedInput.start_released) &&
    Date.now() < startReleaseDeadline) {
  await new Promise((resolve) => setTimeout(resolve, 10));
  trustedInput = await readTrustedInput();
}
if (
  !trustedInput.keydown_trusted ||
  !trustedInput.keyup_trusted ||
  !trustedInput.pressed ||
  !trustedInput.released ||
  !trustedInput.start_keydown_trusted ||
  !trustedInput.start_keyup_trusted ||
  !trustedInput.start_pressed ||
  !trustedInput.start_released
) {
  throw Object.assign(new Error("trusted GBA input did not change the product control state"), {
    details: trustedInput,
  });
}
const recorded = await fetch(`http://127.0.0.1:${serverPort}/proof/trusted-input`, {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify(trustedInput),
});
if (!recorded.ok) throw new Error(`trusted GBA input receipt failed: ${recorded.status}`);
frameClient.close();
pageClient.close();
console.log("[gba-opaque-frame-input] OK keyboard=trusted mapping=pressed-released frame=focused");
