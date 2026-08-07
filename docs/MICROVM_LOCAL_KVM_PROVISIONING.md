# Provisioning a local nested-KVM box for microVM spend/audit verification

Operational notes for standing up a throwaway Linux box that can actually boot
ElastOS crosvm microVMs, so the spend-meter + durable-audit path can be
re-verified on real silicon. These are the two non-obvious prerequisites that
cost real time to rediscover, plus the offline catalog wiring.

Verified working on: aarch64 (Apple-silicon host → Lima `vz`/Virtualization.framework
guest, nested virt, `/dev/kvm` exposed), Ubuntu 24.04 guest, `flint @ 5d4f4c7d1`,
2026-06-29 — 7/7 (see `docs/KNOWN_GAPS.md` G-HWV).

---

## Prereq 1 — a guest kernel that crosvm can actually boot (the GICv3 trap)

**Symptom (silent):** the VM "launches" and exits cleanly (`exited with code 0`,
crosvm "exiting with success") but **nothing ever runs** — no guest console, no
`carrier_invoke`. Captured via `--serial type=file`, the guest kernel is
panicking before init:

```
VFS: Cannot open root device "vda" or unknown-block(0,0): error -6
Kernel panic - not syncing: VFS: Unable to mount root fs on unknown-block(0,0)
```

**Cause:** the upstream crosvm test kernel
(`guest-bzimage-aarch64-r0016`, the default `scripts/setup-crosvm.sh` would
fetch) does not enumerate virtio-pci on GICv3-only hosts — so it never sees the
`vda` block device. This is exactly the case `scripts/setup-crosvm.sh` documents
(its `fetch_kernel` prefers `/boot/Image` on aarch64 for this reason). If a
previous "box-ready" step installed the test kernel, **no VM has ever booted**;
the clean exit is the panic→reboot, not a successful run.

**Fix — use the host's own kernel** (Ubuntu generic has
`CONFIG_VIRTIO_BLK/PCI/MMIO=y`, `CONFIG_EXT4_FS=y` built in). On aarch64 `/boot`
ships a gzip-compressed `vmlinuz`, but crosvm needs a **raw decompressed
`Image`**:

```bash
# decompress vmlinuz → raw Image (arm64 vmlinuz is gzip; find the 1f 8b 08 magic)
sudo cp /boot/vmlinuz-$(uname -r) /tmp/vmlinuz && sudo chown "$USER" /tmp/vmlinuz
python3 - <<'PY'
import zlib
d=open('/tmp/vmlinuz','rb').read()
i=d.find(b'\x1f\x8b\x08')                       # gzip member start
open('/tmp/Image','wb').write(zlib.decompressobj(31).decompress(d[i:]))
PY
file /tmp/Image     # → "Linux kernel ARM64 boot executable Image"
install -m 644 /tmp/Image ~/.local/share/elastos/bin/vmlinux
```

(`scripts/setup-crosvm.sh --kernel /tmp/Image` does the same install + validation.)

After this the guest boots: `EXT4-fs (vda): mounted filesystem`, `Run /init`,
and the capsule's console trace appears.

## Prereq 2 — let crosvm's sandbox create namespaces (Ubuntu 24.04)

**Symptom:** crosvm dies building the VM:

```
crosvm[1]: libminijail[1]: unshare(CLONE_NEWNS) failed: Operation not permitted
ERROR crosvm] exiting with error 1: the architecture failed to build the vm
... failed to create a PCI root hub: failed to create proxy device ...
```

**Cause:** Ubuntu 23.10+/24.04 ships `kernel.apparmor_restrict_unprivileged_userns=1`,
which blocks crosvm's minijail from entering an unprivileged user namespace.

**Fix:**

```bash
sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
sudo sysctl -w kernel.unprivileged_userns_clone=1   # belt-and-suspenders
# persist via /etc/sysctl.d/ if the box survives reboots
```

(`/dev/kvm` must also be present and group-accessible — for a Lima guest, add the
user to the `kvm` group and restart the VM so the membership takes effect.)

## Prereq 3 — load the NFLOG module (W1b egress-audit custody)

**Symptom:** the per-TAP egress firewall still DROPS correctly, but no `EgressDenied`
custody is recorded — the audit reader's `NETLINK_NETFILTER` bind fails closed because
the kernel has no NFLOG backend. (Enforcement is independent of the reader by design, so
containment is never lost; only the audit half goes dark.)

**Cause:** the `nft ... log group N` rules deliver drops through `nfnetlink_log`, which is a
loadable module not always present on a minimal box.

**Fix:**

```bash
sudo modprobe nfnetlink_log
lsmod | grep nfnetlink_log   # confirm it's loaded
# persist via /etc/modules-load.d/ if the box survives reboots
```

This is required to run the W1b C4 egress exercise — the in-tree `#[ignore]`d harness
`tests/c4_egress_spine.rs`, run as root:

```bash
sudo -E cargo test -p elastos-server --test c4_egress_spine -- --ignored --nocapture
```

It is compile-gated in normal CI (so W1b API drift fails the build) but only runs when
asked. Without NFLOG the firewall still enforces, but the per-drop NFLOG → signed
`EgressDenied` custody (and the flood→`suppressed` reconcile marker) cannot be proven.

## Offline catalog (capsule sourcing only — never stub meter/carrier/audit)

The plain `serve` daemon + supervisor resolve binaries/capsules from
`<data_dir>/components.json` and trust operator binary overrides. To run fully
offline (the only thing allowed to be non-prod is **capsule sourcing**):

- `~/.local/share/elastos/components.json` with `external.crosvm` +
  `external.vmlinux` entries carrying `sha256:<hex>` checksums for the
  `linux-arm64` platform (matching the installed `bin/crosvm` / `bin/vmlinux`),
  plus a `capsules.<name>` entry (`cid` non-empty; `sha256` may be `""`).
- Stage the capsule under `<data_dir>/capsules/<name>/` (extract the
  `*.capsule.tar.gz`; write `.elastos-cid` matching the components entry) so the
  supervisor's `ensure_capsule` short-circuits to the local cache (no IPFS).
- Provide the trusted-core providers via env overrides (binaries.rs trusts an
  exact-path `ELASTOS_<NAME>_BIN` without manifest verification):
  `ELASTOS_LOCALHOST_PROVIDER_BIN`, `ELASTOS_SHELL_BIN`.

The metered/audit code path (`serve → supervisor → per-VM bridge_ctx.spend_policy
→ carrier_invoke → meter`, and the durable audit chain) is the **real** code in
every case — only where the capsule came from is offline.

## Run + expected evidence (7/7)

```bash
ELASTOS_AUDIT_LOG_PATH=~/.local/share/elastos/audit/custody.log \
ELASTOS_DEFAULT_SPEND_BUDGET=5 \
ELASTOS_LOCALHOST_PROVIDER_BIN=.../target/debug/localhost-provider \
ELASTOS_SHELL_BIN=.../target/debug/shell \
RUST_LOG=info elastos serve            # → "Durable audit log enabled (verified-on-open)"

elastos capsule act-emitter --config '{"count":6}'
# guest: ACT 1..5 ok ; ACT 6 REFUSED budget_exhausted ; ok=5 exhausted=1
# chain: 5 × spend_debit + 1 × budget_exhausted for vm-act-emitter (signed)
# tamper a record  → next serve start REFUSES: "audit tamper at seq N: record_hash mismatch"
# truncate last rec → next serve start REFUSES: "head-anchor committed seq N but only N-1 records verify"
```

See `capsules/act-emitter/README.md` for the fixture and
`docs/KNOWN_GAPS.md` (G-HWV) for the verified scope + the `Users/self` residual.
