# RUN_E2E — open a video you own, on owned dDRM, in the ElastOS runtime

This is the operator runbook for the runnable dDRM vertical: **publish an owned,
content-addressed, multi-segment asset and open it end-to-end with a real distributed
key — fetched by CID, released once, decrypted in-VM, fail-closed at every seam, with no
CEK or plaintext ever crossing a process boundary.** It is the ElastOS-runtime replacement
for Elacity's Lit-based dDRM (PC2): every command below runs REAL capsule binaries — no
Lit, no mocked crypto.

Everything here is verified by the smoke scripts under `scripts/` (each builds the real
binaries and drives them cross-process). Start at §1 to just SEE it run; §2–§4 walk the
zero→provision→publish→open path; §5 maps each piece to the Lit thing it replaces; §6 is
the trust-boundary cheat-sheet; §7 is the honest status.

---

## 0. Prerequisites

- A Rust toolchain (stable) — `cargo` on `PATH`.
- macOS or Linux. The dKMS Unix-socket transport is `unix`-gated; everything else is portable.
- No blockchain, no IPFS daemon, and no Lit account are required to run the full happy path
  (ownership is checked through the real `chain-provider` against an in-process JSON-RPC mock
  by default; point it at Base mainnet when you want a real wallet check — §4).

```bash
cd /path/to/elastos-runtime
```

---

## 1. The 60-second proof — see it run

One command builds the real binaries and drives the **whole consumer open** end-to-end
(`drm/open → rights → key → decrypt`), in verify mode (which also runs the adversarial
fail-closed gates, the live multi-MiB content plane, and the **live multi-segment** open):

```bash
scripts/ddrm-consumer-smoke.sh
```

You should see, among the steps:

```
[content-plane] ciphertext fetched by its CIDv1 (bafkrei…) through the content capability …
[content-plane/multi-MiB] 2101248-byte media published as chunked UnixFS (Helia-byte-compatible dag-pb root bafybei…), fetched back BY ROOT CID + reassembled …; a tampered leaf failed closed
[decrypt-rail/multi-segment] 3-segment asset opened LIVE end-to-end (key released ONCE, each segment fetched by its CIDv1, all decrypted in-VM): segment_count=3, sample_count=5 …; a SUBSTITUTED segment failed the whole open closed; no CEK/plaintext crossed the boundary
…
ddrm-consumer-smoke (authority=reference): PASS
```

To see a CEK minted, escrowed, recovered, re-sealed and used to decrypt **all in one run**
(a video sealed *now*, decrypted *now*, no golden):

```bash
scripts/ddrm-producer-smoke.sh
```

That's the entire vertical. The rest of this doc explains what each step is and how to run
it against real secret-holding nodes / a real chain.

---

## 2. The shape of an open

```
 producer (once)                  consumer (per open)
 ───────────────                  ───────────────────
 encrypt-provider                 drm-provider     → the canonical open PLAN (no authority)
   mint CEK, CENC-encrypt,        rights-provider  → ownership decision (typed receipt)
   content-address (CID),           └─ chain-provider: has_access_by_content_id (eth_call)
   escrow CEK to the              key-provider     → recover the escrowed CEK + re-seal it
   authority's recipient            └─ dkms-authority node(s): hold the secret, recover in-VM
                                  decrypt-provider → unwrap the CEK IN-VM, decrypt the segment(s)
```

Two invariants make this safe, enforced at every seam:

- **The whole CEK never exists outside a sandbox.** The producer escrows it sealed; the
  key authority re-seals it to the decrypt boundary's per-open session key; the boundary
  unwraps it in-VM and zeroizes it. With a threshold/quorum the CEK is split and only
  reassembles inside the decrypt boundary — never in `key-provider`, never on a wire.
- **Plaintext never crosses a process boundary.** The decrypt boundary returns a *scoped
  session* (counts + metadata), never the decrypted bytes or the CEK.

The single canonical entrypoint the runtime calls is the config-driven
`scripts/dev/ddrm-runtime-open` binary; the smoke scripts write its config and invoke it.

---

## 3. Run it against REAL secret-holding nodes (dKMS)

The `reference` backend keeps the authority's master inside the runtime (simplest). The
`dkms` backend splits it out into one or more **external, secret-holding NODE daemons** —
the real Lit replacement. The open path is byte-identical; only `authority.backend` changes.

| Topology | Command | What it proves |
|---|---|---|
| Single external node | `scripts/ddrm-consumer-dkms-smoke.sh` | master lives in the node's own store, never enters the runtime; the runtime holds only the node's PUBLIC identity + delegates recovery |
| 2-of-2 threshold | `scripts/ddrm-consumer-dkms-threshold-smoke.sh` | CEK XOR-split across two nodes; neither holds the whole key; reassembled only in the decrypt boundary; a one-share release fails closed |
| 2-of-3 quorum (survives a dead node) | `scripts/ddrm-consumer-dkms-quorum-smoke.sh` | CEK Shamir-split over GF(256); ANY TWO live nodes serve; node-kill failover; below quorum fails closed |
| Off-localhost (TCP + encrypted channel) | `scripts/ddrm-consumer-dkms-tcp-smoke.sh` | the 2-of-2 rail over real TCP with a mutually-authenticated, sealed, non-replayable channel; plaintext/downgrade/MITM all fail closed |

All four are thin wrappers over `ddrm-consumer-smoke.sh` with flags
(`--backend dkms`, `--threshold`, `--nodes 3`, `--transport tcp`).

### Provisioning a long-lived dKMS node (production shape)

In the smokes the runtime provisions + reaps the node daemons for you. To run a real,
long-lived node the way you would in production, the `dkms-authority` binary reads its
configuration from the environment (the **operator/provisioner** sets these; a connecting
client can never override them):

| Env var | Meaning |
|---|---|
| `DKMS_AUTHORITY_KEY_STORE=<path>` | the node's durable master-seed store (node-local; the runtime never reads it) |
| `DKMS_AUTHORITY_LISTEN=<unixpath>` or `tcp:HOST:PORT` | bind + listen as a framed remote authority (default: stdin/stdout one-shot) |
| `DKMS_AUTHORITY_ALLOWED_CALLERS=<b64vk,…>` | allow-list of known caller verifying keys; an unlisted caller is refused at `hello`. DKMS-8: validated fail-closed BEFORE the listener binds — empty, malformed, or duplicate ⇒ the daemon exits non-zero (never silently anonymous) |
| `DKMS_AUTHORITY_ALLOW_ANONYMOUS=1` | EXPLICIT anonymous opt-in: with NO allow-list, serve any well-formed caller. Required to run anonymous — unset allow-list AND unset flag is a startup error, so anonymous is always a deliberate choice. Contradicts an allow-list (both set ⇒ fails closed) |
| `DKMS_AUTHORITY_OPERATOR_VK=<b64vk>` | the operator identity that authorizes lifecycle ops (`rotate_share`, `revoke_caller`); absent ⇒ those fail closed |

```bash
# one secret-holding node, listening on TCP, serving only a known caller, operator-pinned:
DKMS_AUTHORITY_KEY_STORE=/var/lib/elastos/dkms-node-a.json \
DKMS_AUTHORITY_LISTEN=tcp:0.0.0.0:7001 \
DKMS_AUTHORITY_ALLOWED_CALLERS=$RUNTIME_CALLER_VK_B64 \
DKMS_AUTHORITY_OPERATOR_VK=$OPERATOR_VK_B64 \
  cargo run --manifest-path capsules/dkms-authority/Cargo.toml
```

The node publishes its PUBLIC identity (verifying key + escrow recipient) at startup; the
runtime is handed only a PUBLIC-ONLY descriptor (`verifying_key_b64`, `recipient_pub_b64`,
`authority_endpoint`) — never the master. For a t-of-n set, run one node per member and
hand the runtime a descriptor with a `threshold` block listing all members.

The node has a full **lifecycle** (all proven in the quorum/threshold smokes): operator-signed
share **rotation** to a fresh successor (the CEK never reassembles during rotation), live
caller **revocation** (cuts a session off mid-stream), quorum **reconfiguration** (change t
and n on a live set), born-distributed **DKG** (the CEK is generated already-split), and a
portable, offline-verifiable **threshold attestation** (the quorum proves which t-of-n nodes
served an open, checkable by anyone without trusting the runtime).

---

## 4. Run the REAL wallet-ownership check (Base mainnet)

By default the open drives the real `chain-provider` `has_access_by_content_id` path
(encode calldata → `eth_call` → decode the ABI bool → rights decision) against an in-process
JSON-RPC mock, so ownership is a real query with no network. To check a real wallet against
a real contract on Base, point it at an RPC endpoint:

```bash
DDRM_SMOKE_CHAIN_RPC=https://mainnet.base.org \
DDRM_SMOKE_CHAIN_CONTRACT=0xYourAuthorityGateway \
DDRM_SMOKE_CHAIN_SELECTOR=0xHasAccessSelector \
DDRM_SMOKE_CHAIN_SUBJECT=0xYourWallet \
DDRM_SMOKE_CONTENT_ID=<bytes16 contentId / KID> \
  scripts/ddrm-consumer-smoke.sh
```

To prove the gate is real, force a NOT-OWNED answer and watch the open fail closed:

```bash
scripts/ddrm-consumer-smoke.sh --deny-ownership
# → "the chain says you do not own it" → open fails closed → PASS
```

---

## 5. What replaces Lit (PC2 → ElastOS runtime)

Elacity's PC2 dDRM leans on Lit Protocol for key custody and access control. This vertical
owns that entire stack. The mapping:

| PC2 / Lit | ElastOS runtime | Why ours is superior |
|---|---|---|
| `hasAccessByContentId` inside a Lit action | **`chain-provider`** `has_access_by_content_id` (the rights gate's source of truth) | the access check is our own inspectable code path, real-by-default |
| Lit network nodes / PKP (opaque t-of-n BLS) | **`dkms-authority`** node daemons (explicit, owned, inspectable t-of-n over GF(256)) | we own the share set, the field arithmetic, the quorum policy, failover, rotation, reconfiguration, and DKG — and a node can prove which set served an open |
| Lit action: decrypt-in-TEE → reseal-to-session → return only the envelope | **`decrypt-provider`** `open_session_v1` (unwrap in-VM, decrypt, return a scoped session) | the CEK unwrap + the decrypt happen in our sandbox; plaintext + CEK never leave |
| `recoverCEKEnvelope` RPC (client holds only the public `pkpId`) | **`key-provider`** delegating recovery to the node over an authenticated channel (holds only the node's public identity) | the runtime holds no recovery secret; a leaked descriptor recovers nothing |
| Helia `unixfs.addBytes` content addressing | the **content capability** + an in-tree Helia-byte-compatible UnixFS importer (raw leaves under a dag-pb root) | byte-for-byte CID compatibility, pinned against the real `@helia/unixfs` oracle, with per-block integrity + fail-closed fetch |
| HTTPS with `rejectUnauthorized: false` (TLS verification OFF) | a PQ-hybrid, mutually-authenticated, sealed channel that authenticates the NODE | the channel itself authenticates the secret-holder; MITM/downgrade/replay all fail closed |

Net: **the whole dDRM vertical — access control, key custody, recovery, decrypt, and content
addressing — is owned, inspectable, and fail-closed**, with no dependency on Lit's opaque
network.

---

## 6. Trust-boundary cheat-sheet

- **`drm-provider`** holds ZERO authority — it only emits the canonical open PLAN (no CEK,
  no keys, no RPC). The runtime EXECUTES the plan.
- **`rights-provider` / `chain-provider`** decide ownership and emit a typed receipt; they
  never see key material.
- **`key-provider`** recovers the escrowed CEK and re-seals it to the decrypt session — for
  `dkms` it holds only the node's PUBLIC identity and delegates recovery. It never holds the
  master and (with a threshold) never reassembles the CEK.
- **`dkms-authority` node** holds the secret in its OWN store; recovery happens inside its
  boundary; it returns only a re-sealed envelope, never the master or the raw CEK.
- **`decrypt-provider`** is the only place the whole CEK and the plaintext exist, transiently,
  in-VM; it returns only a scoped session.
- **Content plane**: ciphertext is content-addressed (CIDv1 / dag-pb root); every fetch
  verifies the bytes hash back to the requested CID and fails closed on a tampered/missing
  block. The multi-segment open also welds the ordered segment set into the decrypt
  transcript, so a substituted/reordered fragment fails the unwrap closed.

Run the standing pre-PR gate any time to confirm the whole thing is intact:

```bash
scripts/ddrm-verify.sh          # contract drift + PC2 conformance + test ladder + WASI smoke
scripts/ddrm-ladder-check.sh    # just the test-count ladder (per-feature rungs)
```

---

## 7. Status — what's runnable, what's left

**Runnable end-to-end today (all green in the smokes):**

- Publish an owned asset → open it: `drm/open → rights (real chain query) → key → decrypt`.
- Content-addressed payloads of **any size**: raw leaf, single dag-pb root, and a balanced dag-pb
  **tree** above the fan-out — all Helia-byte-compatible, fail-closed at every block and tree level.
- **Multi-segment** assets through the LIVE rail on **every** rail — single-node AND the
  2-of-2 threshold / 2-of-3 quorum split rails: each fragment fetched by its CID, the key
  reconstructed once in-VM, all segments decrypted, a substituted fragment failing the whole
  open closed (the `[28]` split-rail gate runs in both the threshold and quorum smokes).
- Real secret-holding dKMS nodes: single, 2-of-2 threshold, 2-of-3 quorum (survives a dead
  node), and off-localhost over TCP with an authenticated channel.
- The full key lifecycle: rotation, revocation, reconfiguration, DKG, and offline-verifiable
  threshold attestation.

**Out of the runnable happy path (explicitly deferred):**

- Upstream only: folding the consolidated `SealedDecryptMaterialV1` into the shared
  `elastos-common` contract (needs push access) and a dKMS-direct sealing producer.

**Where we are, as a %:** ~**99%** of the original goal — "download and run a video I own,
on owned dDRM, in the runtime." The full happy path is live and fail-closed: the content
plane is size-unbounded (balanced dag-pb tree, Helia-byte-compatible) and every rail —
single-node, threshold, and quorum — is multi-segment-capable. What remains is upstream
contract folding (needs push access / Anders), not runnable-vertical work.
