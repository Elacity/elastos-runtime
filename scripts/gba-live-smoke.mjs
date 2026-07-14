#!/usr/bin/env node

const baseUrl = (process.env.ELASTOS_GBA_BASE_URL || "http://localhost:61180").replace(/\/$/, "");
const token = process.env.ELASTOS_GBA_HOME_TOKEN || "";
if (!token) throw new Error("ELASTOS_GBA_HOME_TOKEN is required");
const headers = { "x-elastos-home-token": token };

async function request(path, init = {}) {
  const response = await fetch(`${baseUrl}${path}`, {
    ...init,
    headers: { ...headers, ...(init.headers || {}) },
  });
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (!response.ok) {
    throw new Error(`${path} failed (${response.status}): ${new TextDecoder().decode(bytes)}`);
  }
  return bytes;
}

const rom = await request("/api/viewers/gba-emulator/content/gba-ucity");
if (!rom.length) throw new Error("uCity returned no ROM bytes");

const savePath = "/api/viewers/gba-emulator/storage/gba-emulator/save/live-smoke.sav";
const expected = new TextEncoder().encode(`gba-live-${Date.now()}`);
await request(savePath, { method: "PUT", body: expected });
const restored = await request(savePath);
if (restored.length !== expected.length || restored.some((byte, index) => byte !== expected[index])) {
  throw new Error("principal-scoped save did not round-trip");
}

console.log(`[gba-live] OK rom_bytes=${rom.length} save_bytes=${restored.length}`);
