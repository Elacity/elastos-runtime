#!/usr/bin/env node

const deadline = Date.now() + 20_000;
let target;
while (Date.now() < deadline) {
  try {
    const targets = await fetch("http://127.0.0.1:9222/json/list").then((response) => response.json());
    target = targets.find((item) => item.url?.startsWith("http://127.0.0.1:8765/"));
    if (target?.webSocketDebuggerUrl) break;
  } catch {}
  await new Promise((resolve) => setTimeout(resolve, 50));
}
if (!target?.webSocketDebuggerUrl) throw new Error("GBA Chromium target was not found");

const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true });
  socket.addEventListener("error", reject, { once: true });
});
let nextId = 1;
const pending = new Map();
socket.addEventListener("message", (event) => {
  const message = JSON.parse(event.data);
  const waiter = pending.get(message.id);
  if (!waiter) return;
  pending.delete(message.id);
  if (message.error) waiter.reject(new Error(message.error.message));
  else waiter.resolve(message.result);
});

function send(method, params = {}) {
  const id = nextId++;
  socket.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
}

async function evaluate(expression) {
  const result = await send("Runtime.evaluate", { expression, returnByValue: true });
  return result.result.value;
}

while (Date.now() < deadline) {
  if (await evaluate("document.querySelector('#sound')?.disabled === false")) break;
  await new Promise((resolve) => setTimeout(resolve, 50));
}
const rect = await evaluate(`(() => {
  const value = document.querySelector('#sound')?.getBoundingClientRect();
  return value ? { x: value.x + value.width / 2, y: value.y + value.height / 2 } : null;
})()`);
if (!rect) throw new Error("GBA sound control was not ready");

await send("Input.dispatchKeyEvent", {
  type: "keyDown",
  key: "x",
  code: "KeyX",
  windowsVirtualKeyCode: 88,
  nativeVirtualKeyCode: 88,
});
await send("Input.dispatchKeyEvent", {
  type: "keyUp",
  key: "x",
  code: "KeyX",
  windowsVirtualKeyCode: 88,
  nativeVirtualKeyCode: 88,
});
await send("Input.dispatchMouseEvent", { type: "mousePressed", button: "left", clickCount: 1, ...rect });
await send("Input.dispatchMouseEvent", { type: "mouseReleased", button: "left", clickCount: 1, ...rect });

while (Date.now() < deadline) {
  if (await evaluate("document.querySelector('#sound')?.getAttribute('aria-pressed') === 'true'")) {
    socket.close();
    console.log("[gba-linux-input] OK keyboard=trusted audio_gesture=trusted");
    process.exit(0);
  }
  await new Promise((resolve) => setTimeout(resolve, 50));
}
throw new Error("trusted Chromium audio gesture did not enable sound");
