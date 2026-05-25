# Phase 7 Day 1 — Mac vmlinux artifact decision (Option A vs Option B)

**Phase**: 7 (CI lane + artifact publication)
**Day**: 1 (decision)
**Date**: 2026-05-25
**Status**: GREEN — decision recorded, no code changed, ready for Day-2 wiring
**Predecessor**: [`PHASE_6_DAY_7_NOTES.md`](./PHASE_6_DAY_7_NOTES.md)
**Successor**: Phase 7 Day 2 — `components.json` wiring + runtime resolver

---

## 1. Headline

Phase 7 Day 1 closes the Phase-0 §C.3 deferred decision: how does
`components.json` resolve the `vmlinux.darwin-arm64` artifact for end users?

Two options were on the table since Phase 0:

- **Option A** — Build the same 6.1.59 source tree as `linux-amd64`, but for
  ARM64. Best provenance ("same source as Linux").
- **Option B** — Pin Ubuntu LTS's published ARM64 kernel (signed by
  Canonical, SHA256-pinnable). Lower build burden.

**Decision: Option B.** Pin Ubuntu's `vmlinuz-5.15.0-179-generic.efi.signed`
(extracted ARM64 Image) as the darwin-arm64 vmlinux artifact. Reasoning is in
§ 4 below.

> Day 1 ships **no code change**. The components.json edit lands in Day 2 after
> the URL-pin-vs-IPFS-pin schema is decided.

## 2. Discoveries that reframed the decision

The "10/10 prompt" we drafted at the end of Day 7 assumed there was an
existing `linux-arm64` vmlinux artifact we could probe for reuse on
`darwin-arm64`. The first two steps of Day 1 disproved both halves of that
assumption:

### 2.1 There is no linux-arm64 vmlinux artifact to reuse

From [`components.json:501–506`](../../components.json):

```json
"linux-arm64": {
  "strategy": "local-copy",
  "source": "/boot/Image",
  "note": "Upstream crosvm aarch64 test kernel is broken on GICv3-only hosts (Jetson). Uses host kernel instead.",
  "install_path": "bin/vmlinux"
}
```

The Linux ARM64 host uses **whatever `/boot/Image` the running kernel left
in place** — typically the distro-installed kernel on a Jetson or RPi.
There is no content-addressed artifact. The Mac has no `/boot/Image` to
copy.

The Phase 0 risk register flagged this exact gap at
[`PHASE_0_SCOPE.md:235`](./PHASE_0_SCOPE.md):

> **NEW** — Shipped `vmlinux` on `linux-arm64` uses host-kernel-copy
> strategy; no content-addressed arm64 artifact exists for Mac Vz to fetch.

### 2.2 There is no `linux-amd64` build pipeline in this repo

`linux-amd64` has a published artifact:
```json
"cid": "Qmeb1qaqfMiri7G123FWmMz6qt74xhPjAgAJEy8ZSrFKh7"
"checksum": "sha256:eccb4f318ba37309af5fae01bc610b76033707ec8149002d26675aed66d8578b"
```

…but no in-repo recipe. A repo-wide search for `build-*` scripts surfaces
only `scripts/build-vmlinux-arm64.sh` (the Phase-6 macOS recipe we know
doesn't complete on bare macOS) and `scripts/build-chat-room-ui.sh`
(unrelated). The `linux-amd64` vmlinux was built by an operator out-of-band
and pinned to IPFS as an **opaque blob**. There is no reproducible build
process for ANY kernel architecture in this tree.

This means Option A's "build the same 6.1.59 source as linux-amd64"
provenance claim is **aspirational, not operational** — there is no in-repo
truth we can compare ARM64 output against.

## 3. Evidence gathered today

### 3.1 Ubuntu 5.15.0-179-generic ARM64 — driver set

The kernel that booted in Day 7 (sha256 `9ffae683…`) was inspected
with `strings -n 8`. Built-in driver name strings detected:

| Driver | Status | Phase 0 §C.3 requirement |
|---|---|---|
| `virtio_pci` | **built-in** | ✅ required (validates Phase 0 row) |
| `virtio_console` | **built-in** | ✅ required (validates Phase 0 row) |
| `virtio_balloon` | **built-in** | ✅ (Phase 5+ optional, present) |
| `PF_VSOCK` + `vsock_socket` | **built-in protocol family** | ✅ vsock layer present |
| `virtio_blk` / `virtio_net` / `virtio_vsock` | not built-in | ⚠️ loaded as modules by `initrd-generic` |

Day 7 already proved end-to-end boot, so the modular drivers are confirmed
loadable from initrd. The kernel does NOT have `CONFIG_IKCONFIG` (Ubuntu
strips it from signed shipping kernels to save ~1 MB), but Canonical
publishes the `.config` separately as `/boot/config-5.15.0-179-generic` for
deterministic verification when needed (Day 2 may add a checked-in copy).

### 3.2 Day 7's boot empirically validated the driver set

Phase 0 §F listed three "honest unknowns." All three are now answered by
the Day-7 boot trace ([`PHASE_6_DAY_7_NOTES.md` § 4](./PHASE_6_DAY_7_NOTES.md)):

1. ~~Does the kernel have `CONFIG_VIRTIO_VSOCKETS=y`?~~ — **answered**:
   vsock layer is built-in (PF_VSOCK), transport is modular and loadable.
2. ~~Does the kernel boot cleanly under `VZGenericPlatformConfiguration`
   without source patches?~~ — **answered**: yes, in 0.30 s wall-clock on
   the dev Mac, reaching `Run /init` with no kernel panic.
3. ~~Boot latency target.~~ — **answered**: 0.30 s end-to-end load+start+boot
   (well under the Phase 6 baseline of "3× the Linux baseline").

## 4. Why Option B wins on the available evidence

| Criterion | Option A (build 6.1.59 ARM64) | Option B (pin Ubuntu) |
|---|---|---|
| **Functionally proved on Vz** | ❌ unproved (would need to be built first) | ✅ Day 7 boot test passed |
| **Existing infrastructure to extend** | ❌ none — no `linux-amd64` recipe in repo | ✅ kubo / cloudflared / llama-server already URL-pinned in `components.json` |
| **Provenance** | ⚠️ "same source as linux-amd64" — but linux-amd64 is itself an opaque blob; claim is aspirational | ✅ Canonical's signed-and-published release chain; SHA256-pinnable; supported through Ubuntu 22.04 LTS EOL (2027) |
| **Build burden** | ❌ requires creating a new build pipeline from scratch, then activating Linux CI to run it (~1-2 weeks engineering + CI tax) | ✅ zero build burden (Canonical builds it) |
| **Update path** | ⚠️ manual bump of source SHA + rebuild | ✅ point at next Ubuntu LTS arm64 URL; bump SHA256 |
| **Content-addressed identity** | ✅ would be self-built CID | ✅ URL+SHA256 pin (kubo precedent) or IPFS-mirror (operator can `ipfs add` the downloaded file to get a CID — Day 2 decides) |
| **Functional vs `linux-amd64` rootfs** | ✅ same kernel major.minor | ⚠️ kernel 5.15 vs 6.1 — drivers/syscalls compatible (validated by boot), but subtle behavioral deltas possible |

The only column where Option A wins is *"functional alignment with
linux-amd64's 6.1.59"*. Three counter-arguments make that win marginal:

1. The runtime's userspace contract is the **rootfs**, not the kernel. The
   rootfs is identical across `linux-arm64` and `darwin-arm64` (see
   [`PHASE_6_COMPONENTS_AUDIT.md`](./PHASE_6_COMPONENTS_AUDIT.md) § 4.2
   Decision D.2.a — "share linux-arm64 bundle"). What the rootfs needs
   from the kernel is the virtio/vsock device set, which is present in
   both 5.15 and 6.1.
2. The `linux-arm64` path uses the **host's** kernel (whatever the user
   installed) — it could already be 5.15, 6.1, or 6.8 in production. The
   runtime is already designed to tolerate kernel version variance.
3. If/when we set up a Linux CI build pipeline for vmlinux (Phase 8+),
   we can upgrade the `darwin-arm64` entry to point at our self-built
   ARM64 kernel as a drop-in. The components.json schema already supports
   this (a CID replaces the URL pin).

## 5. Decision

**`components.json` darwin-arm64 vmlinux entry → URL+SHA256 pin to Ubuntu
22.04 LTS `vmlinuz-5.15.0-N-generic` (decompressed Image).**

Day-2 work:

- Decide URL-pin (kubo-style) vs IPFS-mirror (operator runs `ipfs add` and
  pins) — both are viable; kubo-style is simpler.
- Pick a specific Ubuntu point release (`5.15.0-179-generic` from Day 7 is
  a candidate; Canonical may have a newer 5.15.0-N by the time we ship).
- Wire the runtime fetcher / resolver to handle whichever schema we pick.
- Add the initramfs as a second darwin-arm64 component (the Day-7 test
  used Canonical's `initrd-generic`, which we'll also need to ship).
- Add a `vmlinux-arm64-decompress.sh` helper (Ubuntu's vmlinuz is
  gzip-compressed with an EFI signature wrapper; Vz wants the raw Image).
- Optionally check in Canonical's published `/boot/config-5.15.0-N-generic`
  for deterministic CONFIG_* verification.

## 6. Re-scoping note

Phase 0 documented the three options at
[`PHASE_0_SCOPE.md:151–157`](./PHASE_0_SCOPE.md) and deferred the choice
to Phase 6. Phase 6 closed the **substrate** (does Vz drive a guest
kernel?) without making the **shipping choice** (which kernel binary do
we publish?). Phase 7 Day 1 makes the shipping choice with evidence
Phase 6 gathered:

```
                       Phase 0 deferred → Phase 6 closes substrate → Phase 7 Day 1 closes shipping choice
                       (A vs B vs C)      (Day 6: Vz accepts our    (Decision: B; Day 2 wires components.json)
                                           config; Day 7: kernel
                                           boots to userspace)
```

Option C (embed kernel in Vz binary) was rejected in Phase 0 and remains
rejected — it breaks the components.json contract that artifacts are
content-addressed and updateable independently of the runtime.

## 7. What is NOT decided today

Day-1 explicitly leaves these for Day 2 (or later):

- The specific Ubuntu point release to pin (5.15.0-179-generic vs the
  latest -N-generic; the LTS line is stable, so newest patch is fine).
- URL-pin vs IPFS-mirror schema. Both work for `components.json`; Day-2
  reads the runtime resolver and picks the simpler path.
- Whether to bundle a decompression step in the runtime's component
  installer, or ship the already-decompressed Image. (Operator preference;
  decompression is cheap.)
- Phase-8+ upgrade path to a self-built ARM64 kernel (when/if we activate
  a Linux CI runner). The decision today doesn't preclude that — the
  components.json entry can swap URL→CID later without touching the
  runtime resolver.

## 8. Reproducing the discovery

```bash
# Discovery 1: linux-arm64 has no artifact
rg '"linux-arm64"' components.json -A 4
# Confirms strategy: "local-copy" from /boot/Image

# Discovery 2: no in-repo build pipeline for any kernel
ls scripts/build-vmlinux* scripts/build-linux*
# Only build-vmlinux-arm64.sh exists (the Phase-6 macOS recipe)

# Discovery 3: Ubuntu kernel driver set
strings -n 8 ~/.local/share/elastos/bin/vmlinux | \
  grep -E '^(virtio_pci|virtio_blk|virtio_net|virtio_console|virtio_balloon|virtio_input|virtio_mmio|virtio_vsock|vsock|vhost_vsock)$' | \
  sort -u
# Lists: virtio_balloon, virtio_console, virtio_pci (built-in)

# Discovery 4: vsock protocol family is built-in
strings ~/.local/share/elastos/bin/vmlinux | grep -E '^PF_VSOCK$|^vsock_socket$' | sort -u
# Lists: PF_VSOCK, vsock_socket
```

## 9. Branch state

`sash/local-test` at the time of this commit. Day-1 ships docs only — no
code or `components.json` mutations. Day-2 begins after operator review of
the decision recorded here.
