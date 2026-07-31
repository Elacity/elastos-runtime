import { openWebSocket } from "./websocket-client.mjs";

export async function openCdpClient(webSocketDebuggerUrl) {
  const socket = await openWebSocket(webSocketDebuggerUrl);
  let nextId = 1;
  const pending = new Map();
  socket.onMessage((value) => {
    const message = JSON.parse(value);
    if (!message.id) return;
    const waiter = pending.get(message.id);
    if (!waiter) return;
    pending.delete(message.id);
    if (message.error) waiter.reject(new Error(message.error.message));
    else waiter.resolve(message.result);
  });
  return {
    send(method, params = {}) {
      const id = nextId++;
      socket.send(JSON.stringify({ id, method, params }));
      return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
    },
    close() {
      socket.close();
    },
  };
}
