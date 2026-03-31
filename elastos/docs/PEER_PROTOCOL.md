# elastos://peer/ Provider Protocol Contract v1.1

Any capsule that implements this JSON-over-stdio protocol can serve as an `elastos://peer/` provider. The current implementation uses Iroh + DHT, but alternatives (WebSocket relay, libp2p, plain TCP) are valid as long as they honor this contract.

## Wire Format

Line-delimited JSON over stdin (requests) and stdout (responses). One JSON object per line. Every request gets exactly one response.

## Response Envelope

All responses use this shape:

```json
// Success
{"status": "ok", "data": { ... }}

// Success (no data)
{"status": "ok"}

// Error
{"status": "error", "code": "error_code", "message": "Human-readable message"}
```

## Operations

### init

Runtime sends this on provider startup. Provider returns its capabilities.

```json
// Request
{"op": "init", "config": {}}

// Response
{"status": "ok", "data": {
  "protocol_version": "1.1",
  "provider": "peer",
  "features": ["dht_discovery", "gossip", "tickets"]
}}
```

### init_node

Create the network endpoint. Must be called before any other peer operation. The `secret_key` (hex-encoded 32 bytes) enables stable node identity across restarts. If empty, the provider generates a random key.

```json
// Request
{"op": "init_node", "secret_key": "abcd1234..."}

// Response
{"status": "ok", "data": {"node_id": "..."}}
```

**Errors:** `already_init`, `invalid_key`, `bind_failed`

### get_ticket

Return a shareable ticket string for manual peer connection.

```json
// Request
{"op": "get_ticket"}

// Response
{"status": "ok", "data": {"ticket": "...", "node_id": "..."}}
```

**Errors:** `not_init`

### connect

Connect to a peer using their ticket string (manual fallback when DHT is unavailable).

```json
// Request
{"op": "connect", "ticket": "..."}

// Response
{"status": "ok", "data": {"connected": ["node_id_1", ...]}}
```

**Errors:** `not_init`, `invalid_ticket`

### gossip_join

Subscribe to a named topic. Peer discovery happens automatically (DHT, mDNS, or whatever the provider supports). Messages from other peers on this topic will be buffered for `gossip_recv`.

```json
// Request
{"op": "gossip_join", "topic": "#general"}

// Response
{"status": "ok", "data": {"topic": "#general", "discovery": "dht"}}
```

**Errors:** `not_init`, `already_joined`, `join_failed`

### gossip_leave

Unsubscribe from a topic.

```json
// Request
{"op": "gossip_leave", "topic": "#general"}

// Response
{"status": "ok"}
```

**Errors:** `not_joined`

### gossip_send

Broadcast a message to all peers on a topic.

The `sender_id` and `ts` fields are optional overrides for relaying messages from other users (e.g., history sync). If omitted, the provider fills them with the local node ID and current time.

```json
// Request
{"op": "gossip_send", "topic": "#general", "message": "hello world", "sender": "alice", "sender_id": "abc123", "ts": 1708000000}

// Response
{"status": "ok"}
```

**Errors:** `not_joined`, `send_failed`

### gossip_recv

Poll buffered incoming messages for a topic. Returns up to `limit` messages (default 50) and removes them from the buffer.

```json
// Request
{"op": "gossip_recv", "topic": "#general", "limit": 50}

// Response
{"status": "ok", "data": {"messages": [
  {"sender_id": "node123", "sender_nick": "bob", "content": "hello", "ts": 1708000000}
]}}
```

Each message has:
| Field | Type | Description |
|-------|------|-------------|
| `sender_id` | string | Sender's node/public key ID |
| `sender_nick` | string | Sender's display name |
| `content` | string | Message text (may contain DM markers, see below) |
| `ts` | u64 | Unix timestamp (seconds) |

**Errors:** `not_joined`

### list_peers

Return currently connected peer IDs.

```json
// Request
{"op": "list_peers"}

// Response
{"status": "ok", "data": {"peers": ["node_id_1", "node_id_2"]}}
```

### list_topics

Return currently joined topics.

```json
// Request
{"op": "list_topics"}

// Response
{"status": "ok", "data": {"topics": ["#general", "#random"]}}
```

### shutdown

Gracefully shut down the provider.

```json
// Request
{"op": "shutdown"}

// Response
{"status": "ok", "data": {"message": "Provider shutting down"}}
```

## DM Convention (Application Layer)

DMs are an application-layer convention, transparent to the provider. The chat capsule embeds DM markers in the `content` field:

```
\x01DM:<recipient_pubkey>\x01<actual message>
```

The provider broadcasts this like any message. Recipients check if `recipient_pubkey` matches their own pubkey; non-recipients silently drop it. This avoids any provider-level changes for private messaging.

## Implementation Requirements

A conforming `elastos://peer/` provider MUST:
1. Accept line-delimited JSON on stdin, respond on stdout
2. Support all operations listed above (may return `not_supported` for optional features)
3. Buffer incoming messages per-topic until polled via `gossip_recv`
4. Track connected peers for `list_peers`

A conforming provider MAY:
- Use any discovery mechanism (DHT, mDNS, relay servers, manual tickets)
- Use any transport (QUIC, TCP, WebSocket, carrier pigeon)
- Add extra fields to responses (clients must ignore unknown fields)

## Current Implementation

`peer-provider` v1.1 — Iroh 0.96 + distributed-topic-tracker (DHT discovery)

- **Transport:** QUIC via iroh
- **Discovery:** BitTorrent Mainline DHT + mDNS (LAN) + pkarr
- **Gossip:** iroh-gossip (ALPN-based)
- **Dependencies:** ~470 unique crates (heavy crypto stack)
- **Binary size:** ~16MB release
- **Status:** Frozen for stability. Rebuild with `./scripts/chat.sh --rebuild`
