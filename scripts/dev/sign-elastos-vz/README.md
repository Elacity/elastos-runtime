# `sign-elastos-vz` — local-dev codesign helper for the Vz backend

Phase 2 Day 4 of the Apple Silicon Vz backend
(`docs/vz-backend/PLAN.md`). One-page operator guide.

## Why this exists

Apple's `VZVirtualMachineConfiguration.validateWithError` refuses
to accept a configuration unless the host process carries the
`com.apple.security.virtualization` entitlement. A freshly-built
`cargo build` binary does not, so the Day 3 lifecycle wiring in
`elastos-vz/src/ffi/lifecycle.rs::VzMachineHandle::new` returns
a typed error that points right here:

```text
vz validate (vm_id='…'): missing com.apple.security.virtualization
entitlement — sign the binary with scripts/dev/sign-elastos-vz/
(Phase 2 Day 4) or see docs/MAC.md. Apple error: Invalid virtual
machine configuration. The process doesn't have the
"com.apple.security.virtualization" entitlement.
```

Running `sign.sh` ad-hoc signs the binary with the minimal
entitlement plist in this directory. **Local development only.**
Phase 6 will replace this with a proper developer-certificate
signing pipeline + notarization for distribution.

## How to use it

```bash
# 1. Build the binary you want to use.
cargo build -p elastos-server

# 2. Sign it (default: target/debug/elastos).
scripts/dev/sign-elastos-vz/sign.sh

# 3. Verify the entitlement landed.
scripts/dev/sign-elastos-vz/sign.sh --verify-only

# 4. Drive a guest VM end-to-end.
target/debug/elastos vm-debug boot \
  --rootfs /path/to/your-rootfs.img \
  --kernel /path/to/Image
```

Re-run `sign.sh` after **every** `cargo build` — codesign does
not survive a relink. If you skip step 2, the operator-friendly
error above tells you exactly what to do.

You can also sign a release binary:

```bash
cargo build --release -p elastos-server
scripts/dev/sign-elastos-vz/sign.sh target/release/elastos
```

## Files

- `sign.sh` — idempotent macOS-only signer. Refuses to run
  anywhere else. Verifies the entitlement actually landed before
  exiting `0`.
- `vz.entitlements.plist` — minimal entitlements plist. Grants
  one key: `com.apple.security.virtualization`. Nothing else.
- `README.md` — this file.

## What this does NOT do

- It does **not** notarize. The signed binary will not pass
  Gatekeeper on a distribution-stamped Mac. That is Phase 6.
- It does **not** sign any dependent binaries (e.g. helper
  processes). Day 4's scope is the `elastos` entry point only.
- It does **not** make the binary trust a non-existent
  developer certificate. We use **ad-hoc** signing (`-s -`)
  exclusively.

## Anchors

- `docs/vz-backend/PLAN.md` — Phase 2 Day 4
- `elastos/crates/elastos-vz/src/ffi/lifecycle.rs::ENTITLEMENT_HINT`
- `docs/MAC.md` — first-boot recipe
