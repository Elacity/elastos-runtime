# Pre-mainnet deploy checklist — dKMS / decrypt plane

Fail-closed invariants that must hold for a **production** dKMS quorum + decrypt plane. These
are deploy/build properties, not runtime logic; CI enforces the build ones, the human ticks the
ops ones. Source of truth for the threat posture: [THREAT_MODEL.md](THREAT_MODEL.md).

## Build invariants (CI-enforced)

- [ ] **`dkms-authority` is a release build with DEFAULT features.** The legacy unsigned-receipt
      path (`legacy-receipt-authz`) and the `dev-modes` opt-in that pulls it in are dev/migration
      scaffolds and must NOT ship. A `compile_error!` keyed on a release build
      (`not(debug_assertions)`) rejects them, and the CI job **`dkms-release-invariant`** asserts
      both directions (release+default builds; release+`dev-modes`/`legacy-receipt-authz` fails to
      compile). Production build: `cargo build --release --manifest-path capsules/dkms-authority/Cargo.toml`.
- [ ] **`key-provider` is built with `--features key-authority-ref`, NOT `dev-modes`.** The dev
      `reference` backend (raw-CEK injection path) must be absent in production; only the dkms
      quorum rail may release a CEK.
- [ ] **`elastos-server` is built WITHOUT `dev-modes`.** Without it, `chain` is the only selectable
      rights mode (the dev/`chain-mock` rights modes are compiled out — wallet-signed grants +
      live on-chain `hasAccessByContentId` only).

## Ops invariants (human-ticked, per node)

- [ ] **`DKMS_AUTHORITY_NODE_SET_ID_B64` is set on every quorum node.** This pins the node's own
      quorum identity so a grant minted for a DIFFERENT node-set cannot authorize a recover here
      (cross-quorum replay defense). In a release build the pin is **mandatory**: absent it, every
      `recover` fails closed inside `authorize()` (it refuses to fall back to the caller-declared
      node-set). Note: this is enforced **at authorize-time on each recover**, not at process boot,
      and the mandatory branch is `#[cfg(not(any(test, dev-modes)))]` — it is therefore NOT
      reachable by a `cfg(test)` unit test. **Validate it with a release smoke**: a release node with
      the env unset must reject a real recover with the
      `DKMS_AUTHORITY_NODE_SET_ID_B64 must be set in release builds` error.
- [ ] **`DKMS_CHAIN_RPC_POOL` is configured** on every node (the node performs its OWN live
      `hasAccessByContentId` read; `NodeChain::from_env` returns `None` → trustless authorization
      fails closed).
- [ ] **The Carrier descriptor pins each node's PUBLISHED PQ identity** (`verifying_key_b64` +
      `recipient_pub_b64`); migrating transport rewrites only `authority_endpoint`. See
      [DKMS_OVER_CARRIER.md](DKMS_OVER_CARRIER.md).
- [ ] **Audit/forensic retention** is wired as designed (the `grant_digest` rides the permanent
      custody chain; minimization is via non-reversibility, not expiry — see THREAT_MODEL §watermark).

## Open, scoped with the external auditor (do not ship as "done")

- [ ] **Bind the re-seal AAD into the recover possession-proof** (`dkms-authority` `recover` →
      `seal_bound`). Today the node's re-seal AAD is the caller-supplied `aad_b64` and is NOT bound
      into the possession-proof; it is safe only because the decrypt boundary rebuilds the
      segment-bound AAD itself and fails closed. Landing test: *a recover with a tampered `aad_b64`
      fails the possession-proof closed at the node.* See the SECURITY INVARIANT comment at the
      `seal_bound` call and THREAT_MODEL §7.
