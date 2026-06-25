# Auditor packet — dKMS decrypt plane (pre-mainnet)

> Focused external-audit hand-off for the dKMS / decrypt boundary. It **leads with the one
> deliberately-open invariant** (re-seal AAD binding), then gives the boundary, the trust roots, and
> the verification gates so a reviewer can orient fast. Authoritative sources are linked inline; this
> packet does not restate them in full.
>
> Contract: [../PRINCIPLES.md](../PRINCIPLES.md) · Threats: [THREAT_MODEL.md](THREAT_MODEL.md) ·
> Ops invariants: [DEPLOY_CHECKLIST.md](DEPLOY_CHECKLIST.md) · dKMS rail:
> [DKMS_OVER_CARRIER.md](DKMS_OVER_CARRIER.md).

---

## 0. What we are asking you to confirm

1. The **re-seal AAD invariant** (§1) is the *only* place where a trusted-core node emits a value it
   does not itself verify, and that the **decrypt boundary's fail-closed rebuild** is a sound
   compensating control **today** (single consumer, no trust).
2. The **landing fix** in §1 (bind `aad_b64` / `segment_digests` / `node_set_id` into the recover
   possession-proof) is the correct closure, and the **landing test** is the right acceptance gate.
3. No **second consumer** of the node's re-seal output trusts the AAD (we assert there is exactly
   one — the decrypt boundary). Independent confirmation wanted.

Everything else in this packet is context to make those three judgements.

---

## 1. THE open invariant — re-seal AAD is not bound into the possession-proof

**Status:** known, deliberately scoped with you, *not* shipped as "done". Tracked as an open item in
[DEPLOY_CHECKLIST.md](DEPLOY_CHECKLIST.md) and documented in [THREAT_MODEL.md](THREAT_MODEL.md) §7.

**Where:** `capsules/dkms-authority/src/main.rs`, the `recover` handler, at the `seal_bound` call
(see the `SECURITY INVARIANT` comment immediately above it):

```1028:1038:capsules/dkms-authority/src/main.rs
        // SECURITY INVARIANT (pre-mainnet, scoped with the external auditor): `aad` here is the
        // CALLER-SUPPLIED `args.aad_b64` (decoded above), and it is NOT bound into the recover
        // possession-proof — the node verifies the escrow (recover_escrowed_cek) and the producer,
        // but does NOT independently verify that this re-seal AAD matches the segment-bound
        // transcript / node-set the open claims. Therefore the node's re-seal AAD is NOT
        // independently trustworthy. This is safe TODAY only because the single consumer — the
        // decrypt boundary — rebuilds the segment-bound AAD itself and fails closed on a mismatch;
        // it does not trust this value. DO NOT add a consumer that trusts this re-seal AAD without
        // first binding aad_b64 / segment_digests / node_set_id into the recover possession-proof
        // (so a tampered aad_b64 fails the proof closed here). See docs/THREAT_MODEL.md.
        let envelope = ddrm_envelope::seal::seal_bound(&public, cek.as_slice(), &aad, &authority.signer);
```

**The threat.** A dKMS node, on `recover`, verifies the *escrow* (`recover_escrowed_cek` — fails
closed on a foreign/tampered blob, KID-swap, scheme mismatch, or forged producer) and then re-seals
the recovered CEK to the decrypt session. The **AAD** it seals under is the **caller-supplied
`aad_b64`** — it is *not* an input to the possession-proof. So a node cannot, by itself, tell that
the AAD it stamps actually matches the segment-bound transcript / node-set the open claims. **The
node's re-seal AAD is therefore not independently trustworthy.**

**Why it is safe today (the compensating control).** There is exactly **one** consumer of this
re-seal output — the **decrypt boundary** — and it does **not** trust the node's AAD: it
**rebuilds the segment-bound AAD itself** and the AEAD open **fails closed** on any mismatch. A
tampered `aad_b64` cannot widen access; it can only cause the boundary's open to fail.

**The fix (scoped, on the mainnet path).** Bind `aad_b64` / `segment_digests` / `node_set_id` into
the recover possession-proof so a tampered AAD fails the proof **closed at the node**, removing the
reliance on the downstream rebuild.
**Landing test (acceptance gate):** *a `recover` with a tampered `aad_b64` fails the
possession-proof closed at the node.*

**The standing rule for reviewers and future contributors:** do **not** add any consumer that trusts
the node's re-seal AAD before the fix above lands.

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
- Re-seal AAD open item (§1) appears as an explicit unchecked box under *"Open, scoped with the
  external auditor"* — it is not allowed to be ticked until the landing test passes.

## 4b. Already-verified — scope these OUT (so the engagement bills for the hard crypto)

The dDRM / dKMS findings that were investigated and **resolved** (or **cleared** safe-by-construction)
are carried as **build-visible ratchets** — each verdict paired with the test (or CI job / explicit
structural reason) that pins it — so a reviewer can confirm them in minutes and scope them out instead
of re-deriving them:

- **`elastos/crates/elastos-server/tests/ddrm_verdicts.rs`** — the dDRM verdict registry: H1
  (watermark-anchor safe-by-construction), M1 (ECDSA malleability not exploitable — replay is
  nonce-keyed), M3 (CENC box-count bounds, fail-closed), A7 (retry-nonce refresh), A1/A2 (perf,
  byte-identical), PRE-1 (CEK-reconstruction integrity / Byzantine-share fail-closed), PRE-3 (audit
  tamper-evidence), PRE-4 (central fail-closed action enforcement), PRE-5/7/8, PRE-2 (log-redaction
  half). `verdicts_registry_is_intact` fails if a *settled* verdict cites no real pin.
- **`elastos/crates/elastos-runtime/tests/capability_conformance.rs`** — `KNOWN_GAPS` for the
  runtime-core capability findings, with `#[ignore]`d ratchet placeholders for the still-open ones.
- **[PRE_AUDIT.md](PRE_AUDIT.md)** — internal adversarial pass: **7/8 findings RESOLVED** (incl. the
  CRITICAL CEK-reconstruction integrity — production-wired + Byzantine-tested); #2 metadata is PARTIAL
  by design. **[PRINCIPLES_CONFORMANCE.md](PRINCIPLES_CONFORMANCE.md)** "do not re-churn" carries the
  traced-safe verdicts.

These are the **scope-out** evidence; §1 (re-seal AAD binding) is the **scope-in** item that remains.

## 5. How to reproduce the verification state

```bash
just verify            # full gate: alignment-check + smoke + fmt/lint/test (definition of "green")
just alignment-check   # fail-closed contract-drift detection (docs/code/tests/ops must agree)
just test-crate dkms-authority
just test-crate key-provider     # reference backend tests need --features dev-modes
cargo test -p elastos-server --test ddrm_verdicts -- --nocapture   # print the verdict registry (§4b)
cargo test -p elastos-runtime --test capability_conformance         # KNOWN_GAPS capability ratchet
```

`just alignment-check` is the contract-drift guard: if this packet, the threat model, the deploy
checklist, and the code ever disagree about the §1 invariant, it should fail.

---

## 6. Reviewer checklist

- [ ] Confirm §1 is the **only** trusted-core seam emitting an unverified value.
- [ ] Confirm the decrypt boundary is the **single** consumer and rebuilds + fails closed (no trust).
- [ ] Confirm the §1 fix (bind into possession-proof) + landing test are the correct closure.
- [ ] Confirm release-build fences (§4) actually prevent `dev-modes` / `legacy-receipt-authz`.
- [ ] Confirm `Debug` redaction (§4) covers all CEK / escrow-bearing structs.
- [ ] Note any second consumer of the node re-seal output anywhere in the tree (should be none).
- [ ] Spot-check the §4b verdict registries against the code and confirm the scope-out is sound.
