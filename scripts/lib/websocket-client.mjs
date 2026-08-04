import { createHash, randomBytes } from "node:crypto";
import net from "node:net";
import { once } from "node:events";

const WEBSOCKET_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

export async function openWebSocket(value) {
  const url = new URL(value);
  if (url.protocol !== "ws:") throw new Error(`unsupported WebSocket protocol: ${url.protocol}`);
  const socket = net.createConnection({ host: url.hostname, port: Number(url.port || 80) });
  await once(socket, "connect");

  const key = randomBytes(16).toString("base64");
  socket.write([
    `GET ${url.pathname}${url.search} HTTP/1.1`,
    `Host: ${url.host}`,
    "Connection: Upgrade",
    "Upgrade: websocket",
    `Sec-WebSocket-Key: ${key}`,
    "Sec-WebSocket-Version: 13",
    "\r\n",
  ].join("\r\n"));

  let buffered = Buffer.alloc(0);
  await new Promise((resolve, reject) => {
    const onError = (error) => reject(error);
    const onData = (chunk) => {
      buffered = Buffer.concat([buffered, chunk]);
      const headerEnd = buffered.indexOf("\r\n\r\n");
      if (headerEnd < 0) return;
      socket.off("data", onData);
      socket.off("error", onError);
      const headers = buffered.subarray(0, headerEnd).toString("utf8");
      buffered = buffered.subarray(headerEnd + 4);
      if (!headers.startsWith("HTTP/1.1 101")) {
        reject(new Error(`WebSocket upgrade failed: ${headers.split("\r\n", 1)[0]}`));
        return;
      }
      const accept = headers.match(/^Sec-WebSocket-Accept:\s*(.+)$/im)?.[1]?.trim();
      const expected = createHash("sha1").update(key + WEBSOCKET_GUID).digest("base64");
      if (accept !== expected) {
        reject(new Error("WebSocket upgrade returned an invalid accept key"));
        return;
      }
      resolve();
    };
    socket.on("data", onData);
    socket.once("error", onError);
  });

  return new WebSocketConnection(socket, buffered);
}

class WebSocketConnection {
  constructor(socket, buffered) {
    this.socket = socket;
    this.buffered = buffered;
    this.messageListeners = new Set();
    socket.on("data", (chunk) => {
      this.buffered = Buffer.concat([this.buffered, chunk]);
      this.readFrames();
    });
    this.readFrames();
  }

  onMessage(listener) {
    this.messageListeners.add(listener);
  }

  send(value) {
    this.sendFrame(0x1, Buffer.from(String(value)));
  }

  close() {
    if (!this.socket.destroyed) {
      this.sendFrame(0x8, Buffer.alloc(0));
      this.socket.end();
    }
  }

  sendFrame(opcode, payload) {
    const mask = randomBytes(4);
    let header;
    if (payload.length < 126) {
      header = Buffer.from([0x80 | opcode, 0x80 | payload.length]);
    } else if (payload.length <= 0xffff) {
      header = Buffer.alloc(4);
      header[0] = 0x80 | opcode;
      header[1] = 0x80 | 126;
      header.writeUInt16BE(payload.length, 2);
    } else {
      header = Buffer.alloc(10);
      header[0] = 0x80 | opcode;
      header[1] = 0x80 | 127;
      header.writeBigUInt64BE(BigInt(payload.length), 2);
    }
    const masked = Buffer.from(payload);
    for (let index = 0; index < masked.length; index += 1) {
      masked[index] ^= mask[index % 4];
    }
    this.socket.write(Buffer.concat([header, mask, masked]));
  }

  readFrames() {
    while (this.buffered.length >= 2) {
      const first = this.buffered[0];
      const second = this.buffered[1];
      if ((first & 0x80) === 0) throw new Error("fragmented WebSocket frames are unsupported");
      const opcode = first & 0x0f;
      let length = second & 0x7f;
      let offset = 2;
      if (length === 126) {
        if (this.buffered.length < 4) return;
        length = this.buffered.readUInt16BE(2);
        offset = 4;
      } else if (length === 127) {
        if (this.buffered.length < 10) return;
        length = Number(this.buffered.readBigUInt64BE(2));
        offset = 10;
      }
      const masked = (second & 0x80) !== 0;
      const frameLength = offset + (masked ? 4 : 0) + length;
      if (this.buffered.length < frameLength) return;
      let payload = Buffer.from(this.buffered.subarray(offset + (masked ? 4 : 0), frameLength));
      if (masked) {
        const mask = this.buffered.subarray(offset, offset + 4);
        for (let index = 0; index < payload.length; index += 1) {
          payload[index] ^= mask[index % 4];
        }
      }
      this.buffered = this.buffered.subarray(frameLength);
      if (opcode === 0x1) {
        for (const listener of this.messageListeners) listener(payload.toString("utf8"));
      } else if (opcode === 0x8) {
        this.socket.end();
        return;
      } else if (opcode === 0x9) {
        this.sendFrame(0xA, payload);
      }
    }
  }
}
