# dKMS Quorum Transport over Carrier

Status: transport scaffold landed and validated locally (full real-stack 2-of-3 recover
over Carrier). Live-node rollout is node-by-node and additive. This document is the
contract + threat model for moving the dKMS quorum transport from WireGuard to Carrier.

## Why

The dKMS 2-of-3 quorum was reachable only over a manually-provisioned WireGuard mesh
(`dkms0`, `tcp:10.66.66.x:9443`). That violates Principle 4 (Carrier plane for local and
off-box), forces every runtime to install a VPN, and requires per-node caller enrollment —
none of which scales to millions of sovereign runtimes. Carrier (iroh: QUIC + pkarr DHT +
relays + hole-punching) lets a runtime reach each node by its `did:key` with zero VPN, zero
manual setup, and NAT traversal from anywhere.

## What changes, and what does NOT

Only the transport changes. The descriptor pins each node's PUBLISHED PQ identity
(`verifying_key_b64` + `recipient_pub_b64`); migrating to Carrier rewrites ONLY the
`authority_endpoint` from `tcp:10.66.66.x:9443` to `carrier:did:key:z6Mk...`. Everything
that matters for security is byte-for-byte unchanged end-to-end:

- the PQ-hybrid encrypted, mutually-authenticated channel (`hello` attestation under the
  descriptor-pinned ML-DSA identity + channel KEM key),
- the Shamir 2-of-3 split/escrow/recover,
- the wallet-signed `AccessGrantV1` authorization + the node's own live on-chain
  `hasAccessByContentId` read.

This satisfies Principle 15: "a capability-checked access/decryption plane that can use
local storage, Carrier, or other substrates underneath without changing the capsule
contract."

## Architecture (relay-sidecar, Phase 1)

```text
key-provider ─loopback tcp─▶ dkms-carrier-client ─Carrier/iroh (ALPN elastos/dkms-authority/1)─▶ dkms-carrier-node ─tcp 10.66.66.x:9443─▶ dkms-authority
   (runtime)                    (runtime sidecar)         (QUIC, relay/holepunch)                  (per quorum node)        (unchanged binary)
        └──────────────────────── PQ-hybrid encrypted channel established END-TO-END ─────────────────────────────────────────────┘
```

- `key-provider` gains a `carrier:`/`did:` endpoint branch: it connects to the local
  sidecar over loopback, writes a one-line `did:key` preamble, and then runs its EXISTING
  framed protocol with `network=true` (mandatory encrypted channel). iroh stays OUT of the
  audited crypto binary.
- `dkms-carrier-client` (sidecar) resolves the did via pkarr/mDNS, dials the node's Carrier
  endpoint, and relays raw bytes.
- `dkms-carrier-node` (per node) accepts the ALPN, dials the node's existing
  `dkms-authority` TCP listener, and relays raw bytes. The `dkms-authority` binary, env,
  and systemd unit are unchanged; deploying the bridge is purely additive.

The principled end-state (follow-on) is to retire the bespoke ALPN + sidecars and route
dKMS recovery through the runtime's single Carrier endpoint via the carrier-provider-plane
(`elastos.provider.invocation/v1`, `key`/`decrypt`/`drm` providers — see `docs/CARRIER.md`).

## Threat model

- The relays/sidecars/bridges are UNTRUSTED transport. They only ever see ciphertext: the
  PQ channel terminates between `key-provider` and `dkms-authority`. A malicious relay can
  drop or delay frames (fail-closed: bounded read timeouts, no partial release) but cannot
  read or forge a recover.
- A man-in-the-middle that terminates the QUIC connection still cannot attest its own KEM
  channel key under the descriptor-pinned ML-DSA identity, so a wrong-key channel fails
  closed at `hello` before anything is delegated (`network=true` makes the channel
  mandatory on the carrier scheme; a missing channel block is fatal).
- The node's `did:key` is published to the pkarr DHT for discovery. The did is public and
  carries no authority — it is an address, not a credential. Authorization remains the
  wallet-signed grant + on-chain check, performed by the node itself.
- Anonymous-caller posture: with the per-node allow-list removed, ANY runtime may connect,
  but a recover still requires a valid wallet-signed `AccessGrantV1` for an on-chain-held
  access token. Connection is not authorization. This is the millions-of-runtimes scale
  posture; the allow-list was only a DoS gate, never the security boundary.

## Non-disruption on the supernodes (InterServer / Contabo)

- The bridge is a NEW systemd unit running as the unprivileged `dkms` user, connecting to
  the node's EXISTING `10.66.66.x:9443` listener. It does NOT touch `dkms-authority`,
  PC2 (nginx/awg0/wg0/IPFS/sing-box), or the ELA DAO node (`ela`/`arbiter`/`esc`/`eid`).
- Firewall surface decreases vs WireGuard: relays/hole-punching remove the need for the
  manual inbound UDP rule. If best-case direct connectivity is wanted, add exactly one
  additive `ufw allow <port>/udp` — never edit existing rules or the default-deny posture.
- Rollback: `systemctl stop dkms-carrier-bridge`. WireGuard `dkms0` stays live as fallback
  until the carrier path is proven, then the WireGuard step is dropped from the runtime
  path (kept on nodes as rollback).

## Validation

- Local full-stack: `scripts/dev/ddrm-producer-smoke/live-producer-carrier-verify.sh`
  stands up 3 real `dkms-authority` daemons behind 3 `dkms-carrier-node` bridges + the
  sidecar and drives the REAL `key-provider` 2-of-3 recover over Carrier. PASS.
- Live: deploy `dkms-carrier-node` beside each node's `dkms-authority`, capture each did,
  build the carrier descriptor with `scripts/dev/dkms-make-carrier-descriptor.py`, and run
  the gateway with `ELASTOS_DKMS_CARRIER=1` (off the dkms0 mesh) for a real browser open.
