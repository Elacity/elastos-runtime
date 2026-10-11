import assert from "node:assert/strict";
import dgram from "node:dgram";
import net from "node:net";
import test from "node:test";
import { turnListenerPortAbsent, turnRelayPortsAbsent } from "./browser-vm-control-service.mjs";

test("TURN proofs keep TCP control and UDP relay ownership separate", async t => {
  const tcp = net.createServer();
  await new Promise(resolve => tcp.listen({ host: "127.0.0.1", port: 0, exclusive: true }, resolve));
  t.after(() => new Promise(resolve => tcp.close(resolve)));
  const port = tcp.address().port;
  const turn = { listen_host: "127.0.0.1", listen_port: port,
    relay_host: "127.0.0.1", relay_port_min: port, relay_port_max: port };
  assert.equal(await turnListenerPortAbsent(turn), false, "live TCP control holder keeps cleanup pending");
  assert.equal(await turnRelayPortsAbsent(turn), true, "TCP relay allocations are disabled");
  const udp = dgram.createSocket({ type: "udp4", reuseAddr: false });
  await new Promise(resolve => udp.bind({ address: "127.0.0.1", port, exclusive: true }, resolve));
  let closed = false;
  t.after(() => { if (!closed) udp.close(); });
  assert.equal(await turnRelayPortsAbsent(turn), false, "live UDP relay holder keeps cleanup pending");
  await new Promise(resolve => udp.close(resolve));
  closed = true;
  assert.equal(await turnRelayPortsAbsent(turn), true);
});
