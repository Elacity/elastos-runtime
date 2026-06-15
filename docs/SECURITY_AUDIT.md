# Security audit — 2026-06-14

Method: `cargo audit` (dependency CVEs) + four read-only adversarial sub-agents — crypto
correctness, identity/transport binding, secrets hygiene & side channels, and memory-safety /
sandbox-escape / deserialization. Every finding is `file:line`, read not grepped. Complements
[CAPABILITY_AUDIT.md](CAPABILITY_AUDIT.md) (authorization architecture) — this pass is the
attacker's-eye view of where keys actually leak.

## Verdict

Strong. **No memory-safety bug, no sandbox escape, no cryptographic-primitive flaw, no secret
logged/serialized/Debug-leaked, no timing side channel.** The crypto core (hybrid x25519+ML-KEM-768
KEM, ML-DSA-65 signatures, per-seal CSPRNG GCM nonces, length-prefixed domain-separated KDF/AAD,
fail-closed t-of-n threshold) is genuinely well-built. **One High** finding: the authorization gate
feeding the dDRM key path is forgeable. The math is correct; the trust root above it is missing a
signature.

And two **capability gaps resolved as SAFE** (below) — the runtime's identity binding is
safe-by-construction, not the weakness it was flagged as.

## Findings

| Sev | Area | file:line | Finding | Fix |
|-----|------|-----------|---------|-----|
| **High** | dDRM authorization | `capsules/dkms-authority/src/main.rs:1714`, `capsules/key-provider/src/main.rs:2120`, `elastos-common/src/protected_content.rs:228` | The **rights-decision receipt gates CEK recovery but has no signature and is never verified** — `reauthorize`/`validate_rights_receipt_binding` only check `schema`, `allowed==true`, and field-equality. A caller that clears the hello gates (allow-list empty by default; possession proof uses the caller's own key) submits `recover`/`release` with a forged `allowed:true` receipt + attacker session pubkey; the node re-seals the CEK/share to the attacker. On the single-node rail (`encrypt-provider` escrows the full CEK) this yields a usable CEK from one node. **Design intent (`docs/PROTECTED_CONTENT.md:91,133`) already calls for *signed* release receipts — not yet implemented.** Confirms & upgrades CAPABILITY_AUDIT GAP-7. | `rights-provider` signs the receipt (ML-DSA); `key-provider`/`dkms-authority` verify the signature against the pinned rights-authority key before release. |
| Med | wallet key memory | `capsules/wallet-provider/src/main.rs:655`, `:670-680` | Decrypted secp256k1 **private key** (`Vec<u8>`) and the derived **AES-256 wrapping key** (`[u8;32]`) are dropped **un-zeroized** on every sign/recover; `Zeroizing` is used nowhere in the crate (every other secret-handling crate uses it). Raw key bytes linger in freed heap (core-dump / memory-scrape risk). | Wrap the intermediates in `Zeroizing<…>`. Mechanical. |
| Med | dDRM defense-in-depth | `capsules/dkms-authority/src/main.rs:916`, `key-provider/src/main.rs:143` | The authority seals the CEK under a **caller-supplied transcript AAD** without rebuilding `DecryptTranscriptV1::to_aad()` from authenticated fields. Contained by the downstream AEAD (a wrong transcript fails the viewer open), so not independently exploitable — but it removes the layer that would otherwise blunt the High finding by welding the seal to an authenticated transcript at the sealing side. | Rebuild the transcript AAD from verified fields server-side; don't trust the caller's `aad_b64`. |
| Low | did-provider memory | `capsules/did-provider/src/main.rs:442`, field `:163` | `storage_key`/`device_key` (`[u8;32]`) not zeroized — inconsistent with the crate's own `:456-457` zeroization and `elastos-identity/store.rs:48` (`Zeroizing<[u8;32]>`). | Wrap in `Zeroizing`. |
| Low | supply chain | `cargo audit` → RUSTSEC-2024-0436 | `paste 1.0.15` is **unmaintained** (transitive via `iroh → netwatch → netlink-packet-core`). No active exploit; advisory is maintenance status. | Track upstream `iroh`/`netlink` updates; add a `cargo deny`/`audit` gate. |
| Info | elastos-tls | `elastos/crates/elastos-tls/src/lib.rs:112,174` | Local-CA private-key PEM held as a plain `String` (never logged; written `0o600`). Low impact (dev CA, rcgen `KeyPair` doesn't expose zeroization). | Note only. |

## Resolved — capability gaps that are NOT vulnerabilities (safe by construction)

The identity-binding agent traced these to definitive verdicts:
- **GAP-1 (caller-identity spoofing) — SAFE.** There is no `from`/`capsule_id` field on the wire;
  identity is bound per-connection host-side (`io_bridge.rs:38`, `carrier_bridge.rs` uses
  `bridge_ctx.capsule_id` set at `supervisor.rs:1099`; each VM has its own dedicated socket). A
  guest has no field to forge.
- **GAP-4 (principal_id session binding) — SAFE.** Self-asserted principal authority is rejected at
  the launch boundary (`supervisor_api.rs:189`); `principal_id` derives only from a gateway-DID-signed,
  verified, non-delegatable launch grant (`gateway_home_token.rs:298-353`) and flows host-side into
  dispatch (`carrier_bridge.rs:663`). A capsule cannot act under a principal it doesn't own without
  forging the gateway's signature.
- **Shell exemption — SAFE.** Single-registration unguessable-UUID `shell_id`; `from` is host-bound;
  the production app-capsule path (`carrier_bridge`) has no shell concept and rejects all
  runtime-control verbs.

**Residual trust root (one link to confirm):** the whole binding rests on each guest being able to
write only to its own dedicated carrier socket / stdio pipe — enforced by the crosvm/WASM sandbox
wiring at `supervisor.rs:1063-1110`. That isolation is standard and no code path was found that
violates it, but it lives in sandbox config rather than the audited Rust; worth an explicit confirm.

## What is sound (verified, not assumed)

- Crypto primitives: genuine PQ-hybrid (needs both x25519 + ML-KEM secrets), per-seal OsRng GCM
  nonces, CENC CSPRNG IV + per-asset CEK (no keystream reuse), thorough domain separation, constant-
  time signature verification, replay-protected dKMS (`recover_seq` + session token + possession
  proof), fail-closed Shamir/XOR threshold (no single node or forged share recovers the key).
- Memory safety: all `unsafe` is fixed-size `repr(C)` FFI with correct lengths/FD ownership, no
  attacker-influenced pointer/length; the one `transmute` is POD of exact size.
- Sandbox: WasiCtx is minimal — no `inherit_network`, no extra preopens, no custom host functions; a
  capsule reaches only `/data` (if granted) + the `/_carrier` FIFO. No escape surface found.
- Deserialization: the one remote-controlled allocation (`carrier.rs:5183`) is capped at 200 MB;
  manifest parsing is pure serde + `deny_unknown_fields`; all slices on parsed input are length-guarded.
- Secrets: no key in logs/Debug/Serialize/responses/panics; no timing side channel on a secret.

## Priorities

1. **Sign the rights-decision receipt (High).** It gates the crown jewel and the design already
   intends it — best fixed *as part of the dDRM work in flight*, before it ships.
2. **Zeroize wallet-provider key material (Med).** Mechanical, isolated, do anytime.
3. **Rebuild transcript AAD server-side (Med, defense-in-depth).** Blunts #1.
4. Add a `cargo audit`/`deny` gate; track the `paste`/`iroh` advisory.
