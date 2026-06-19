# Auditor packet — dKMS decrypt plane (pre-mainnet)

> Focused external-audit hand-off for the dKMS / decrypt boundary. It **leads with the invariant we
> just closed** (re-seal AAD binding — the diff is in this branch; please confirm the fix), then gives
> the boundary, the trust roots, and the verification gates so a reviewer can orient fast.
> Authoritative sources are linked inline; this packet does not restate them in full.
>
> Contract: [../PRINCIPLES.md](../PRINCIPLES.md) · Threats: [THREAT_MODEL.md](THREAT_MODEL.md) ·
> Ops invariants: [DEPLOY_CHECKLIST.md](DEPLOY_CHECKLIST.md) · dKMS rail:
> [DKMS_OVER_CARRIER.md](DKMS_OVER_CARRIER.md).

---

## 0. What we are asking you to confirm

1. The **re-seal AAD binding** (§1, now landed) is a correct closure: binding `sha256(reseal_aad)`
   into the recover possession-proof, verified at the node before any CEK is recovered, soundly
   prevents a MITM from making the node seal under an AAD the caller did not prove possession over.
2. Binding the **AAD digest** (rather than `aad_b64` / `segment_digests` / `node_set_id` as separate
   fields) is sufficient — i.e. that `DecryptTranscriptV1` genuinely carries `node_set_id` +
   `segment_digests`, so the digest binds all of them, with no field left unbound.
3. No **second consumer** of the node's re-seal output trusts the AAD (we assert there is exactly
   one — the decrypt boundary, which independently rebuilds it as defense-in-depth). Independent
   confirmation wanted.

Everything else in this packet is context to make those three judgements.

---

## 1. THE invariant — re-seal AAD now bound into the possession-proof (LANDED)

**Status:** closed in this branch. The fix is the diff below; we are asking you to confirm it is a
sound closure (§0). Tracked as landed in [DEPLOY_CHECKLIST.md](DEPLOY_CHECKLIST.md) and
[THREAT_MODEL.md](THREAT_MODEL.md) §7.

**The threat (was).** A dKMS node, on `recover`, verifies the *escrow* (`recover_escrowed_cek` — fails
closed on a foreign/tampered blob, KID-swap, scheme mismatch, or forged producer) and then re-seals
the recovered CEK to the decrypt session. The **AAD** it seals under is the **caller-supplied
`aad_b64`**. Previously this was *not* an input to the possession-proof, so a MITM that tampered
`aad_b64` in transit could make the node seal under an AAD of its choosing — safe only because the
single downstream consumer (the decrypt boundary) rebuilt the AAD and failed closed.

**The fix.** The canonical recover possession-proof preimage now binds `sha256(reseal_aad)`
(`ddrm_envelope::recover_proof_message`, domain bumped `…/recover-proof/v1` → `…/v2`). The client
signs over the exact AAD it sends (`key-provider`), and the node verifies the proof over the
**byte-identical** `args.aad_b64` in `verify_session`, **before** any CEK is recovered or re-sealed.
Because the AAD (`DecryptTranscriptV1`) already encodes `node_set_id` + `segment_digests`, the digest
binds all three named fields transitively (so the preimage stays bounded for long presentations).

**Node verification (before recover):**

```1797:1804:capsules/dkms-authority/src/main.rs
    // RE-SEAL-AAD BINDING (v2): verify the proof over the EXACT AAD this recover will seal under. This
    // is the SAME `args.aad_b64` `recover_inner` decodes and passes to `seal_bound` — so a MITM that
    // tampers `aad_b64` in transit (incl. its embedded node_set_id / segment_digests) makes this proof
    // fail closed HERE, before any CEK is recovered or re-sealed. Closes the pre-mainnet invariant.
    let reseal_aad = b64()
        .decode(&args.aad_b64)
        .map_err(|_| "aad_b64 is not valid base64".to_string())?;
    if !ddrm_envelope::verify_recover_proof(
```

**Re-seal site (now CLOSED — the comment records why):**

```1028:1035:capsules/dkms-authority/src/main.rs
        // SECURITY INVARIANT (re-seal AAD — CLOSED): `aad` here is the caller-supplied `args.aad_b64`,
        // but it is now BOUND into the recover possession-proof: `verify_session` (above, before any CEK
        // is recovered) verifies `verify_recover_proof(.., reseal_aad = decode(args.aad_b64), ..)`, which
        // is the byte-identical AAD passed here. So a MITM-tampered `aad_b64` — including its embedded
        // `node_set_id` / `segment_digests` — invalidates the proof and is refused at the node, fail-closed
        // (test: `recover_fails_closed_on_a_tampered_aad`). The decrypt boundary STILL independently
        // rebuilds the segment-bound AAD and fails closed on any mismatch — defense-in-depth, not the sole
        // control. See docs/THREAT_MODEL.md §7 and docs/AUDITOR_PACKET.md §1.
```

**Landing test (green):** `dkms-authority::tests::recover_fails_closed_on_a_tampered_aad` — a recover
whose `aad_b64` is tampered after the caller signed its proof is refused at the node (`session_invalid`)
before any CEK is recovered. Plus `ddrm-envelope`'s `dkms_recover_proof_round_trips…` asserts a
tampered AAD fails `verify_recover_proof`.

**The standing rule (unchanged):** the decrypt boundary remains the second, independent fail-closed
check on the AAD; do **not** remove it (defense-in-depth).

---

## 2. Trust boundary (so you know what is in scope)

- `runtime/` and the **trusted-core capsules** (`dkms-authority`, `key-provider`) are the small
  trusted base. Application/provider logic lives in other `capsules/*`. App logic must **not** be in
  the trusted core (PRINCIPLES 5, 13). The re-seal invariant in §1 is the one audited seam where the
  core emits an unverified value, mitigated downstream.
- **Identity is rooted / content-addressed; transport is an adapter** (PRINCIPLES 2, 4, 9). HTTP /
  Carrier sockets are below the capsule contract — never treated as truth. The dKMS rail can run over
  Carrier; see [DKMS_OVER_CARRIER.md](DKMS_OVER_CARRIER.md).
- **One canonical path per operation; explicit fail-closed when the intended path isn't ready**
  (PRINCIPLES 10, 11). No hidden alternate decrypt path.

---

## 3. Cryptographic trust roots (orientation)

From [THREAT_MODEL.md](THREAT_MODEL.md) §7:

- **CEK transport / re-seal:** hybrid x25519 + ML-KEM-768 KEM → AES-256-GCM, ML-DSA-65 signatures,
  per-seal CSPRNG nonces, length-prefixed domain-separated KDF/AAD (PQ-conscious for
  harvest-now-decrypt-later). The §1 invariant lives here.
- **Escrow recovery:** `recover_escrowed_cek` is fail-closed on foreign/tampered escrow, wrong
  KID/scheme, or bad producer key (`capsules/dkms-authority/src/main.rs` `recover`).
- **Rights anchor:** wallet EIP-191/1271 signature over an `AccessGrantV1`. (The forensic-watermark
  codeword is derived from this grant digest — see [AV_WATERMARKING.md](AV_WATERMARKING.md) — but AV
  is roadmap, not part of the live decrypt plane.)

## 4. Release / build invariants the reviewer can rely on (CI-enforced)

These are asserted in CI, not just documented (see [DEPLOY_CHECKLIST.md](DEPLOY_CHECKLIST.md) and
`.github/workflows/ci.yml` `dkms-release-invariant`):

- Release builds **cannot** enable `dev-modes` or `legacy-receipt-authz` — a `compile_error!` guard
  in `dkms-authority` fences them out of release.
- `key-provider` redacts CEK / escrow blobs from `Debug` (manual `impl Debug`, fields → `<redacted>`)
  so secrets cannot leak via debug logs.
- Re-seal AAD binding (§1) is checked off under *"Landed"* now that the landing test passes; it was
  not allowed to be ticked until then.

## 5. How to reproduce the verification state

```bash
just verify            # full gate: alignment-check + smoke + fmt/lint/test (definition of "green")
just alignment-check   # fail-closed contract-drift detection (docs/code/tests/ops must agree)
# The dKMS capsules build standalone (not elastos-workspace members):
cd capsules/dkms-authority && cargo test --features legacy-receipt-authz   # incl. the landing test
cd capsules/ddrm-envelope  && cargo test --features access-grant,av-variants dkms_recover_proof
cd capsules/key-provider   && cargo check --features key-authority-ref
```

`just alignment-check` is the contract-drift guard: if this packet, the threat model, the deploy
checklist, and the code ever disagree about the §1 invariant, it should fail.

---

## 6. Reviewer checklist

- [ ] Confirm the §1 fix (bind `sha256(reseal_aad)` into the possession-proof, verified before
      recover) + the landing test are a correct closure.
- [ ] Confirm the AAD digest binds all of `aad_b64` / `segment_digests` / `node_set_id` (no field
      left unbound) — i.e. `DecryptTranscriptV1` carries them.
- [ ] Confirm no **other** trusted-core seam emits a value it does not itself verify.
- [ ] Confirm the decrypt boundary remains the independent fail-closed rebuild (defense-in-depth).
- [ ] Confirm release-build fences (§4) actually prevent `dev-modes` / `legacy-receipt-authz`.
- [ ] Confirm `Debug` redaction (§4) covers all CEK / escrow-bearing structs.
- [ ] Note any second consumer of the node re-seal output anywhere in the tree (should be none).
