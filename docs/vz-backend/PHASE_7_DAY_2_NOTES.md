# Phase 7 Day 2 — `components.json` wiring: `elastos setup` installs the Mac kernel + initrd

**Phase**: 7 (CI lane + artifact publication)
**Day**: 2 (Mac vmlinux + initrd wiring)
**Date**: 2026-05-25
**Status**: GREEN — `elastos setup --profile minimal` on a bare Mac
fetches Ubuntu's signed kernel + initrd from Canonical, decompresses the
kernel in-place, and the resulting `bin/vmlinux` boots end-to-end through
`elastos-vz` to userspace via `single_vm_boots_to_userspace`. 14/14
elastos-vz tests + 379/379 elastos-server lib tests pass.
**Predecessor**: [`PHASE_7_DAY_1_NOTES.md`](./PHASE_7_DAY_1_NOTES.md)
**Successor**: Phase 7 Day 3 (supervisor wiring to use the fetched
`bin/initrd` as the default per-capsule initramfs, OR self-hosted Mac
runner activation — see § 8).

---

## 1. Headline

`elastos setup --profile minimal` on a bare Mac (no pre-staged artifacts
in `~/Library/Application Support/elastos/bin/`) downloads two artifacts
from Canonical's pinned `release-20260515` cloud-images URLs, verifies
checksums against the publisher's signed manifest, decompresses the
gzip-bearing vmlinuz into a raw ARM64 Image, and writes `bin/vmlinux` +
`bin/initrd` byte-for-byte identical to the Day-7-reference artifacts.

The same `single_vm_boots_to_userspace` test that closed the Phase-6
substrate then booted those fetcher-installed paths to userspace in
0.30 s wall-clock. **The macOS install + boot loop is now operator-free.**

## 2. What changed

### 2.1 `components.json` — `vmlinux.darwin-arm64` switched from placeholder to URL-pin

Before (Phase 6 Day 4b placeholder):
```json
"darwin-arm64": {
  "cid": "",
  "checksum": "",
  "size": 0,
  "release_path": "vmlinux-darwin-arm64",
  "install_path": "bin/vmlinux",
  "build_recipe": "scripts/build-vmlinux-arm64.sh",
  "note": "Day-4b operator handoff: run scripts/build-vmlinux-arm64.sh..."
}
```

After (Day 2):
```json
"darwin-arm64": {
  "url": "https://cloud-images.ubuntu.com/releases/jammy/release-20260515/unpacked/ubuntu-22.04-server-cloudimg-arm64-vmlinuz-generic",
  "checksum": "sha256:b712ef9919cad88f85e25e4b924c3dacde74e866363867b7b447b7841909462a",
  "size": 15392425,
  "compression": "gzip",
  "install_path": "bin/vmlinux",
  "note": "Pinned Ubuntu 22.04 LTS arm64 kernel (5.15.0-179-generic, immutable release-20260515 path)..."
}
```

Notes:

- The URL uses the **immutable `releases/jammy/release-20260515/`** path
  rather than `jammy/current/`. Canonical guarantees the dated release
  path never changes after publication; `current/` is a symlink that
  flips on every LTS point release. Day-1 §7 promised we'd pick the
  immutable form for content-addressed identity — we did.
- `checksum` and `size` describe the **as-downloaded (gzip-compressed +
  EFI-signed) bytes** Canonical signs in `SHA256SUMS`. Decompression
  happens **after** verification so the components.json hash matches the
  publisher's signed manifest, not the post-decompression bytes.
- `release_path` is **gone** from this entry. Its presence used to force
  the fetcher through the trusted-source Carrier path (with no HTTPS
  fallback by design — see [`setup.rs:1348–1350`](../../elastos/crates/elastos-server/src/setup.rs)).
  The new entry routes through the plain-HTTPS `else` branch instead,
  matching how `kubo`, `cloudflared`, and `llama-server` ship today.

### 2.2 `components.json` — new `initrd` component (darwin-arm64-only)

```json
"initrd": {
  "install_path": "bin/initrd",
  "description": "Initial ramdisk that brings up userspace on the Mac (Vz) microVM path...",
  "platforms": {
    "darwin-arm64": {
      "url": ".../ubuntu-22.04-server-cloudimg-arm64-initrd-generic",
      "checksum": "sha256:8cb79fdcbf90313d7a5a315a2dc90bca7435976c3603a28929bce5feefab2b1c",
      "size": 33053012,
      "install_path": "bin/initrd"
    }
  }
}
```

The `initrd` component intentionally has **only a `darwin-arm64` entry**.
On Linux the resolver hits the "no platform entry → silently skip"
branch at [`setup.rs:201`](../../elastos/crates/elastos-server/src/setup.rs):
```
[skip] initrd — not available for linux-arm64
```
That's the right behavior: crosvm on Linux boots `vmlinux` + rootfs
directly (kernel built-in drivers cover the device set); only the Vz
path needs an initrd because Ubuntu's generic kernel ships virtio_blk /
virtio_net / virtio_vsock as **modules** that initramfs has to load
before the rootfs mount. See [`PHASE_7_DAY_1_NOTES.md` § 3.1](./PHASE_7_DAY_1_NOTES.md)
for the strings-derived driver inventory.

The initrd is **not** re-compressed. Canonical's `initrd-generic` arrives
zstd-compressed; Linux 5.15+ self-decompresses zstd initramfs at boot, so
Vz passes the bytes through verbatim. No `compression` field on this
entry — the fetcher writes the verified bytes as-is.

### 2.3 `components.json` — three profiles include `initrd`

`minimal`, `chat`, and `full` — the three profiles that already pull
`vmlinux` — now also pull `initrd`. Linux users see `[skip] initrd —
not available for linux-arm64`; macOS users get both. No new profile
was added.

### 2.4 `elastos-server/src/setup.rs` — fetcher learned single-file gzip

Three small changes (~55 LOC net):

#### 2.4.1 `PlatformInfo` gained `compression: Option<String>`

A new `#[serde(default)]` field. Currently the only supported scheme is
`"gzip"`. The doc-comment on the field captures the contract:

> When set, the fetcher verifies the checksum against the *compressed*
> bytes (matching what the publisher signed) and then decompresses to
> `install_path`. Currently supports: `"gzip"`. Does not apply to
> tarball artifacts — those still use `extract_path` for unpacking.

#### 2.4.2 New helper: `write_decompressed_or_verbatim`

A single-file analogue of `extract_from_tarball`. When `compression` is
unset, falls through to `atomic_write_file` (today's behavior, byte-for-
byte identical). When set to `"gzip"`, decompresses through
`flate2::read::GzDecoder` (already a `flate2 = "1.0"` dep — used by the
existing tarball path) before writing.

The two existing `atomic_write_file` call sites in `download_component`
and `install_first_party_component_via_carrier` were re-pointed at the
new helper.

#### 2.4.3 Verifier short-circuit for compression-bearing artifacts

The post-install state machine ([`component_install_state` lines
742–784](../../elastos/crates/elastos-server/src/setup.rs)) used to
re-hash and re-size the on-disk file and compare against
`platform_info.size` / `platform_info.checksum`. For a compression-bearing
component that breaks by design: the on-disk file is the **decompressed**
form, with a different hash and size than the publisher-signed bytes
those fields capture.

The fix mirrors how tarball-extracted bundles already skip per-file
checksums (see [`setup.rs:676–714`](../../elastos/crates/elastos-server/src/setup.rs)):
when `compression.is_some()`, skip the on-disk byte-level check and
trust the download-time `verify_checksum`. The on-disk file is still
inspected for existence — corruption shows up loudly at VZ-boot time,
not silently here.

The same short-circuit was added to `verify_installed_component_binary`
(consumed by the supervisor at [`supervisor.rs:707`](../../elastos/crates/elastos-server/src/supervisor.rs)),
keeping pre-launch verification consistent.

#### 2.4.4 Seven test fixtures updated

Seven `PlatformInfo { … }` struct literals in `setup.rs` (4) and
`supervisor.rs` (3) had to grow `compression: None,`. Mechanical change;
no semantic impact. Verified by `cargo test -p elastos-server --lib` →
379/379 pass.

## 3. The boot test

```bash
# Pre-state — move any pre-staged artifacts aside so the fetcher runs
mv ~/Library/Application\ Support/elastos/bin/{vmlinux,initrd} \
   ~/Library/Application\ Support/elastos/bin.day2-pre/   # was empty here

# Run setup
./elastos/target/debug/elastos setup --profile minimal
# →  [skip] crosvm — not available for darwin-arm64
# →  [install] vmlinux ...
# →    Downloading https://cloud-images.ubuntu.com/.../vmlinuz-generic...
# →    Checksum verified (sha256)
# →    Installed: .../bin/vmlinux
# →  [install] initrd ...
# →    Downloading https://cloud-images.ubuntu.com/.../initrd-generic...
# →    Checksum verified (sha256)
# →    Installed: .../bin/initrd
# →  Done. 2 installed, 1 skipped.

# Verify the installed artifacts byte-exact match Day-7's references
shasum -a 256 ~/Library/Application\ Support/elastos/bin/{vmlinux,initrd}
# vmlinux: 9ffae683f615230c53ced0c1f4d9aa13554fb5377d26a5fabb002a22bb078a19
# initrd : 8cb79fdcbf90313d7a5a315a2dc90bca7435976c3603a28929bce5feefab2b1c

# Boot it
export ELASTOS_VZ_TEST_KERNEL=~/Library/Application\ Support/elastos/bin/vmlinux
export ELASTOS_VZ_TEST_INITRD=~/Library/Application\ Support/elastos/bin/initrd
scripts/dev/sign-elastos-vz/sign.sh elastos/target/debug/deps/concurrent_launch-*
elastos/target/debug/deps/concurrent_launch-* single_vm_boots_to_userspace --nocapture --exact
# →  single_vm_boots_to_userspace: PASS (marker 'Run /init' observed)
# →  test result: ok. 1 passed; finished in 0.32s
```

Console-capture excerpt from the actual run:

```
[    0.124594] cacheinfo: Unable to detect cache hierarchy for CPU 0
[    0.125180] loop: module loaded
[    0.125248] SPI driver altr_a10sr has no spi_device_id for altr,a10sr
[    0.125379] tun: Universal TUN/TAP device driver, 1.6
[    0.125522] ehci-pci: EHCI PCI platform driver
[    0.125615] ohci-pci: OHCI PCI platform driver
[    0.125749] i2c_dev: i2c /dev entries driver
[    0.125937] device-mapper: ioctl: 4.45.0-ioctl (2021-03-22) initialised
[    0.126234] NET: Registered PF_INET6 protocol family
[    0.126564] NET: Registered PF_PACKET protocol family
[    0.126714] Loading compiled-in X.509 certificates
[    0.126950] Loaded X.509 cert 'Build time autogenerated kernel key...'
[    0.127183] Loaded X.509 cert 'Canonical Ltd. Live Patch Signing 2025 Kmod...'
[    0.127416] Loaded X.509 cert 'Canonical Ltd. Live Patch Signing...'
[    0.127643] Loaded X.509 cert 'Canonical Ltd. Kernel Module Signing 2025 Kmod...'
[    0.127865] Loaded X.509 cert 'Canonical Ltd. Kernel Module Signing...'
```

Note the Canonical-signed module certificates appearing in the printk —
that's the kernel proving its own provenance from within the boot trace.

## 4. Idempotence — re-run is a no-op

```
./elastos/target/debug/elastos setup --profile minimal
# →  Components to install:
# →    - crosvm
# →    - vmlinux [already installed]
# →    - initrd [already installed]
# →
# →  [skip] crosvm — not available for darwin-arm64
# →  [skip] vmlinux — already installed
# →  [skip] initrd — already installed
# →
# →  Done. 0 installed, 3 skipped.
```

The verifier short-circuit (§ 2.4.3) is what makes this work for a
compression-bearing artifact — without it the on-disk hash would
mismatch every time and the fetcher would re-download forever.

## 5. Regression coverage

| Suite | Result |
|---|---|
| `cargo test -p elastos-vz` (lib + 3 integration tests) | **14/14 pass** |
| `cargo test -p elastos-server --lib` | **379/379 pass** |
| Idempotent re-run of `elastos setup` | **no-op** (matches expected behavior) |
| URL-pin path for `kubo`, `cloudflared`, etc. | **unchanged** (no compression field, falls through to verbatim write) |
| CID-pin path for `linux-amd64` vmlinux | **unchanged** (Carrier fetch + no compression) |
| `local-copy` path for `linux-arm64` vmlinux | **unchanged** (Linux runtime self-stamps checksum at install time) |

The fetcher change is **strictly additive** — the new field is
`#[serde(default)]` so existing components without it work exactly as
they did before.

## 6. Three small framework decisions made today

- **URL-pin over IPFS-mirror.** components.json already has 6 existing
  components on URL-pin schema (kubo, cloudflared, llama-server, models)
  using `url` + `sha256/sha512` checksums. Mirroring that pattern keeps
  the fetcher surface area constant. IPFS-mirror would have required
  someone with operator credentials to `ipfs add` Canonical's bytes and
  pin them in our IPFS provider — more moving parts for no provenance
  gain.
- **Immutable `releases/jammy/release-20260515/`.** Canonical's
  `current/` symlink would have worked today (SHA256s match), but flips
  on every LTS point release. The immutable path freezes against a
  specific kernel version (5.15.0-179-generic) until we deliberately
  bump it.
- **Decompress at fetcher time, not at publish time.** A `compression:
  "gzip"` field on PlatformInfo (+ ~30 LOC fetcher patch) means the
  components.json `checksum` field describes **what Canonical signs**
  (compressed bytes). The alternative — pre-decompress and re-publish
  to our own URL — would have required us to maintain a separate
  mirror with weaker provenance, since we'd be re-signing what was
  originally Canonical's responsibility.

## 7. What is NOT done today

Deliberately deferred (Day-3+ or Phase 8):

- **Supervisor wiring of the default per-capsule initramfs.** Right
  now the supervisor still treats `initramfs_path` as opt-in per
  `VmConfig` ([`supervisor.rs:2305–2309`](../../elastos/crates/elastos-server/src/supervisor.rs)).
  A real-capsule boot on Mac will fail to mount its rootfs until the
  supervisor learns to default `VzConfig.initramfs_path` to
  `data_dir.join("bin/initrd")` when that file exists on darwin-arm64.
  Day-3 work, isolated to the supervisor's Mac arm.
- **Bumping `release-20260515`.** Canonical may publish a newer
  5.15.0-N point release. A bump script (`scripts/release/bump-vmlinux-darwin-arm64.sh`)
  that fetches the latest `SHA256SUMS`, recomputes the components.json
  entries, and commits is a follow-up nice-to-have, not blocking.
- **Linux `initrd` entry.** crosvm doesn't need a separate initrd today.
  If a future Linux capsule requirement surfaces (e.g. for an Ubuntu-
  kernel-on-Linux-host configuration), we'd add `linux-arm64` /
  `linux-amd64` entries to the same `initrd` component without
  changing the fetcher.
- **`vmlinux-darwin-arm64` self-build pipeline.** Phase-0 § C.3
  Option A (build our own 6.1.59 ARM64 kernel) stays a future
  upgrade path. The components.json schema admits a URL→CID swap
  without runtime resolver changes — when a Linux CI build pipeline
  materializes (Phase 8+), we just replace the URL with our content-
  addressed CID and the operator-facing behavior is unchanged.

## 8. Branch state + next prompt

`sash/local-test` at this commit. No `main` push, per the user's
standing instruction.

Day-3 candidates, ordered by smallest-first:

1. **Supervisor default-initramfs wiring** (~30 LOC, isolated to the
   Mac arm of `supervisor.rs`). Unblocks real-capsule boot through the
   `elastos run`/`launch_capsule` flows.
2. **Self-hosted Mac runner activation** (Phase 6 left this scaffolded
   but not switched on — needs the operator to attach a physical
   Apple-Silicon runner to the GitHub org).
3. **Day-2 hardening**: a CI smoke that runs `elastos setup --profile
   minimal` + the boot test on every Mac PR. Closes the regression-
   detection loop the runner activation enables.
