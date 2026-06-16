# Threat Model — dDRM (decentralized rights-managed content)

> Audience: the external security auditor, and any operator deciding what this system does and does
> not protect. This document states the trust model and the metadata exposure **plainly**, including
> what we deliberately do *not* yet defend. It follows [PRINCIPLES.md](../PRINCIPLES.md) §11 (fail
> closed, then explain) and §12 (docs, code, tests, and ops must agree): if you find code that
> contradicts a claim here, that is a bug in the code or this doc — file it.
>
> Companion docs: [SECURITY_AUDIT.md](SECURITY_AUDIT.md) (attacker's-eye review of the code),
> [convergence/DEV_MODE_GUARD_SPEC.md](convergence/DEV_MODE_GUARD_SPEC.md) (the release/dev fence).

## 1. What dDRM protects

dDRM lets a creator publish encrypted content whose **content-encryption key (CEK)** is never held
by any single party at rest, and whose release is gated by a **live, wallet-anchored, on-chain
rights check**. The protected asset is:

- **Confidentiality of the CEK / plaintext** for a viewer who does not currently hold the rights.
- **Integrity of the rights decision**: a recover only happens for a wallet that the chain says may
  open this content, re-checked inside the dKMS node's own boundary (not merely asserted by a
  caller).

The CEK is split with information-theoretic secret sharing (2-of-3 Shamir across a node set, or 2-of-2
XOR for the degraded path) and only reconstructed transiently, inside a boundary, bound to a
published commitment, after authorization. See `capsules/ddrm-envelope`, `capsules/dkms-authority`,
`capsules/decrypt-provider`.

## 2. The trust model is **2-of-3, NOT "no collusion"**

The security of CEK confidentiality rests on a **threshold assumption**, stated exactly:

- The CEK for a given node set is recoverable from **any 2 of its 3 shares**.
- A viewer's open is safe **as long as fewer than `t` (=2) of the serving node operators collude**
  for that content.
- **Two colluding node operators in a set CAN reconstruct the CEK** for content their set serves.
  This is an explicit, designed property of a `t`-of-`n` scheme — it is *not* a vulnerability, and it
  is *not* the same as "no node can ever see the key." We do **not** claim non-collusion.

Corollaries an auditor should hold us to:

- A single Byzantine node returning a well-formed-but-wrong share must **fail the open closed**, not
  yield a silently-wrong key. (Enforced: CEK is bound to a published commitment before use, with
  cheater-detection across the fetched candidate shares — `decrypt-provider/src/rail_shim.rs`,
  `ddrm-envelope` `cek_commitment` / `combine_cek_shamir2_checked`.)
- An uncommitted degraded (2-of-2) open is **refused**, not served on a best-effort basis.
- The threshold `t`, the set `n`, and the membership are **explicit and owned in this repo** — not
  opaque inside a third-party network.

## 3. Who can see what (the metadata reality)

Even with the CEK protected, **opening content emits an access pattern**. We are explicit about it
because hiding it would be dishonest and an auditor will find it anyway.

| Party | Sees | Notes |
|-------|------|-------|
| **dKMS node operator** (each node it serves) | The recover requests it serves: the **content id / kid, the principal (wallet) it is keyed on, the session, the right, and the time**. | This is the core exposure. A node operator learns *who opened what, when* for opens it participates in. The 2-of-3 split protects the *key*, not this *access pattern*. |
| **The chain / Base RPC provider** | The rights query `hasAccessByContentId(content_id, wallet)` as an `eth_call`. | The configured RPC endpoint sees `(content_id, wallet)` lookups. On-chain ownership/rights are inherently public. See `capsules/dkms-authority/src/node_chain.rs`. |
| **An on-path network observer** | TLS/connection metadata: peer addresses, connection timing, frame **counts** and **coarse bucketed sizes**. | The channel payloads are sealed (hybrid x25519+ML-KEM-768 AEAD, ML-DSA-65 signed). Frame **lengths** are bucket-padded (see §5), so an observer learns a size *class*, not the exact message size — but timing and frame *count* still leak. |
| **The local runtime / gateway** | Plaintext, the wallet linkage, the CEK transiently. | This is the **owner's own trusted local boundary** (PRINCIPLES §1, §5). It is trusted on behalf of the signed-in principal; it is not a remote adversary. A *compromised* runtime is out of scope for confidentiality (it is the thing serving the owner). |

### The `(wallet, content_id, time)` access pattern

This tuple is the central privacy limitation. Node operators (for opens they serve) and the chain RPC
(for the rights check) observe it. **We do not currently hide it.** What we *have* done is reduce its
incidental spread (§4, §5); fully blinding it is roadmap (§6).

## 4. Audit & logging exposure (mitigated)

- **Gateway logs no longer persist the raw access pattern.** The viewer-open path logs the wallet
  subject and content id only as **non-reversible short fingerprints** (`fp:` + truncated SHA-256),
  so an operator can correlate one open across log lines without the raw `(wallet, content_id)` being
  written to disk/shipped logs. Operational signal (verdict, which gate ran, timing) is retained.
  See `elastos-server/src/api/viewer_open.rs` (`log_fp`). The node itself still observes the raw
  values in memory while serving — logging is not the leak, the node's role is.
- **The custody trail is tamper-evident.** The runtime-core audit log is hash-chained
  (`seq`+`prev_hash`+`record_hash`) and **ed25519-signed** with a crypto-agility tag, and a
  content-open custody record is written **before** an authorized open proceeds — a failed audit
  append fails the open closed. See `elastos-runtime/src/primitives/audit.rs`,
  `viewer_open.rs`.
  - **Honest caveat (non-repudiation scope):** the signature defends against **offline / post-hoc
    tampering** and gives **non-repudiation of the log** — it does **not** defend against a
    **live-compromised runtime**, which holds the signing key and could re-sign a rewritten chain.
    Defending that requires **external anchoring** (periodically checkpointing the chain head to the
    Base chain or an external witness). That is a deliberate follow-on; until it lands, do not claim
    more than "tamper-evident against external editing + non-repudiable."

## 5. Channel metadata minimization (coarse, in place)

The dKMS encrypted channel pads each frame's **plaintext to a coarse size bucket before sealing**
(powers of two from 256 B up to a cap; ISO/IEC 7816-4 padding), so the on-wire frame length reveals a
size *class* rather than the exact message size — collapsing, e.g., a `status` poll and a small
`recover` into the same bucket. See `ddrm-envelope` `channel_pad`, applied symmetrically at every
channel seal/open site (`dkms-authority`, `key-provider`, dev recover tools).

**This is a coarse defense, stated as such:** it does not hide frame **count** or **timing**, and
large content-binding frames above the cap are not bucket-expanded. It raises the bar against trivial
size-fingerprinting; it is **not** traffic-analysis resistance.

## 6. What we do NOT defend (explicit, for the auditor)

These are known and deliberate. The "deep fix" for the access-pattern items is roadmap and should be
scoped by the external firm:

1. **The `(wallet, content_id, time)` access pattern to node operators and the chain RPC.** The
   real fix is **blinded identifiers / oblivious lookup** (the node serves a recover without learning
   which wallet/content it is for, and the rights check is done without revealing the pair to the
   RPC). Not implemented; roadmap. Today, mitigated only incidentally (§4, §5).
2. **Collusion of `t` or more node operators in a set** — by construction (§2).
3. **A live-compromised runtime/gateway** — it is the owner's trusted boundary; it sees plaintext and
   the audit signing key (§4). External anchoring is the roadmap mitigation for audit integrity.
4. **Network timing and frame-count traffic analysis** — only frame *size* is partially padded (§5).
5. **A compromised or malicious Base RPC endpoint's view** of rights queries — the *correctness* of
   the rights answer is defended (quorum/agreement over a configured RPC pool; a disagreeing or
   unavailable pool fails closed), but the RPC operator's *visibility* of `(content_id, wallet)` is
   not hidden.

## 7. Cryptographic trust roots (for completeness)

- **CEK transport / re-seal:** hybrid x25519 + ML-KEM-768 KEM → AES-256-GCM, ML-DSA-65 signatures,
  per-seal CSPRNG nonces, length-prefixed domain-separated KDF/AAD. PQ-conscious for the
  harvest-now-decrypt-later threat on confidential material.
- **Rights anchor:** wallet EIP-191/1271 signature on an `AccessGrantV1`, bound to
  kid/node-set/chain/≤24h window + per-request nonce, **re-verified and re-checked on-chain inside the
  dKMS node's own boundary**.
- **Audit log:** ed25519 (in the trusted core, no capsule dependency), crypto-agility tag for future
  rotation to a PQ signature with zero format change.

## 8. Summary for the auditor

- The **key** is protected under a **2-of-3 threshold** — we claim exactly that, not "no node sees
  it" and not "no collusion."
- The **access pattern** `(wallet, content_id, time)` **is observable** to node operators (for opens
  they serve) and to the chain RPC. We have minimized its *incidental* spread (fingerprinted logs,
  bucket-padded frames) but **not** hidden it. Oblivious lookup is the roadmap fix and is the single
  most valuable thing to scope next.
- The **custody trail** is tamper-evident and non-repudiable against external editing, **not** against
  a live-compromised runtime (anchoring is roadmap).
