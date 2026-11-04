import assert from "node:assert/strict";
import dgram from "node:dgram";
import fs from "node:fs";
import net from "node:net";
import test from "node:test";
import vm from "node:vm";

// Run the control service's real Close relay probe (unexported) against real sockets.
const control = fs.readFileSync(new URL("./browser-vm-control-service.mjs", import.meta.url), "utf8");
const probes = control.slice(control.indexOf("function closeBoundHandle("),
  control.indexOf("function supervisorOwnedVsockBridgesAbsent("));
const context = vm.createContext({ net, dgram, Promise });
vm.runInContext(`${probes}\nglobalThis.turnRelayPortsAbsent = turnRelayPortsAbsent;`, context);
const host = "127.0.0.1";

function close(handle) {
  return new Promise((resolve) => handle.close(() => resolve()));
}

// A free two-port relay range: both ports free for TCP and UDP at selection time.
async function relayRange(t) {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    const server = net.createServer();
    await new Promise((resolve) => server.listen({ host, port: 0 }, resolve));
    const port = server.address().port;
    await close(server);
    if (port >= 65535) continue;
    const turn = { relay_host: host, relay_port_min: port, relay_port_max: port + 1 };
    if (await context.turnRelayPortsAbsent(turn)) return turn;
  }
  t.skip("no free relay range");
}

function listenTcp(port) {
  const server = net.createServer();
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen({ host, port, exclusive: true }, () => resolve(server));
  });
}

function bindUdp(port) {
  const socket = dgram.createSocket({ type: "udp4", reuseAddr: false });
  return new Promise((resolve, reject) => {
    socket.once("error", reject);
    socket.bind({ address: host, port, exclusive: true }, () => resolve(socket));
  });
}

test("free relay ports are absent", async (t) => {
  const turn = await relayRange(t);
  assert.equal(await context.turnRelayPortsAbsent(turn), true);
});

test("a TCP socket on a relay port does not hold Close (no-tcp-relay)", async (t) => {
  const turn = await relayRange(t);
  const server = await listenTcp(turn.relay_port_max);
  t.after(() => close(server));
  assert.equal(await context.turnRelayPortsAbsent(turn), true);
});

test("a UDP socket on a relay port is a held relay", async (t) => {
  const turn = await relayRange(t);
  const socket = await bindUdp(turn.relay_port_max);
  t.after(() => close(socket));
  assert.equal(await context.turnRelayPortsAbsent(turn), false);
});
