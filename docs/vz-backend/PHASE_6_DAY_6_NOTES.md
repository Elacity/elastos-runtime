# Phase 6 Day 6 — First end-to-end FORCE_FULL smoke green (6a local-lane scaffolding)

**Phase**: 6 (macOS native-binary surface)
**Day**: 6 (split: 6a agent-shipped scaffolding, 6b operator-shipped first run + triage)
**Date**: 2026-05-25
**Status**: 6a complete; 6b operator handoff documented (see § 4)
**Predecessor**: [`PHASE_6_DAY_5_NOTES.md`](./PHASE_6_DAY_5_NOTES.md)
**Successor**: Day 7 — Real-microVM perf measurement

---

## 1. Scope deviation from plan — Day 6 split into 6a + 6b + local lane reframing

The original Day-6 prompt assumed the runner was already activated
(Day-5b complete) and that the first `mac-vz.yml::mac-vz-full-boot`
workflow run had produced a terminal outcome for Day 6 to triage.

**Two honest blockers** showed up on this trajectory:

1. **Day-5b's "procure a separate Apple-Silicon Mac as a self-hosted
   runner" is operator-side and gated on hardware procurement.** The
   user does not need to spend that capital to validate Phase 6's
   substrate work — they have an Apple-Silicon Mac in hand already
   (the dev machine).

2. **Day-4b's `vmlinux` build recipe was missing its kconfig
   fragment.** Day 4a shipped `scripts/build-vmlinux-arm64.sh` with a
   prerequisite check for `scripts/release/vmlinux-arm64.config`; that
   file did not exist yet. The recipe bailed cleanly with a typed
   error, but it meant Day 4b could not start without an additional
   agent-shipped piece.

**Day 6a (this commit — agent-shipped) resolves both:**

- Ships the missing `scripts/release/vmlinux-arm64.config` kconfig
  fragment (~80 lines including comments — Vz-required CONFIG_*
  overrides + capsule-isolation primitives + rootfs prereqs).
- Updates `scripts/build-vmlinux-arm64.sh` to use the canonical
  kernel-build pattern: `make defconfig` → `merge_config.sh -m` →
  `make olddefconfig`. This replaces the "operator hand-derives a
  full 5000-line .config" Day-4b task with a "kernel merge_config.sh
  resolves dependencies from a small fragment" agent-deliverable.
  Day 4b's vmlinux-build sub-task is now an unmodified `bash
  scripts/build-vmlinux-arm64.sh` invocation — no manual config work.
- Ships `scripts/ci/local-day6-smoke.sh` — a one-command orchestrator
  that runs all 3 Phase-5 smokes under `ELASTOS_VZ_SMOKE_FORCE_FULL=1`
  against the dev Mac (no separate self-hosted runner needed). Wraps:
  setup-mac-runner.sh preflight delegate, cargo build, vmlinux probe,
  per-smoke run with the correct env-var matrix
  (FORCE_FULL + chat-interop OFFLINE/BIN_OVERRIDE pair where needed),
  per-smoke triage block on failure.

**Day 6b (operator-shipped — first run + triage cycle):**

- `brew install aarch64-elf-gcc make elfutils openssl@3 bc jq`
  (~5 min, one-time).
- `bash scripts/build-vmlinux-arm64.sh` (~30–40 min wall-clock,
  one-time per kernel-config change).
- `mkdir -p ~/.local/share/elastos/bin && cp
  elastos/target/vmlinux-darwin-arm64/Image
  ~/.local/share/elastos/bin/vmlinux` (~5 s).
- `bash scripts/ci/local-day6-smoke.sh` (~5–10 min per run).
- If failures: triage per the orchestrator's structured summary,
  fix or carry-forward, re-run with `ELASTOS_LOCAL_DAY6_SKIP_SETUP=1`
  for faster iteration.

**Why a "local lane" instead of a self-hosted runner.** Three lanes
were considered (also documented in
[`SELF_HOSTED_RUNNER_SPEC.md`](./SELF_HOSTED_RUNNER_SPEC.md) § 1):

| Lane | Day-6 viability | Why |
|---|---|---|
| GitHub-hosted `macos-latest` | ❌ Not viable | `Virtualization.framework` not reliably exposed on GitHub's macOS runner image — explicit "won't change" carry-forward in [`CI_RUNBOOK.md`](./CI_RUNBOOK.md) § 5. |
| Self-hosted (`mac-vz-full-boot`) | ⚠️ Possible | Wired but dormant (Phase 5 Day 6). Needs a dedicated Mac registered as a runner + Day-4b artefacts pushed to remote. ~20 min operator time + dedicated HW capital. |
| **Local dev-Mac lane** | ✅ **Most pragmatic** | Same Vz substrate, no separate HW. Produces real `mac-vz-full-boot`-equivalent runs. This is what Day 6a ships scaffolding for. |

The self-hosted lane remains the right substrate for **gated CI**
(public PR mergeability), but Day 6 — the headline gate "first
FORCE_FULL smoke green" — closes faster on the local lane and the
result is byte-identical (same Vz API surface, same kernel Image,
same elastos-server binary).

This mirrors precedent set on Linux: Phase 1–5 validated against the
dev box's KVM substrate; CI gated on the same path under a separate
runner. Day 6's local lane is the macOS analogue.

---

## 2. Concrete changes (Day 6a — agent-shipped)

### 2.1 `scripts/release/vmlinux-arm64.config` (new, 80 lines incl. comments)

Kconfig fragment with the minimum Vz-required `CONFIG_*` overrides.
Six logical groups:

| Group | Knobs | Why |
|---|---|---|
| Virtio transport | `VIRTIO`, `VIRTIO_PCI`, `VIRTIO_PCI_LEGACY` | Vz uses PCI as its only virtio transport. |
| Virtio devices | `VIRTIO_CONSOLE`, `VIRTIO_NET`, `VIRTIO_BLK`, `VIRTIO_BALLOON` | The four guest-visible devices the runtime + capsules need. |
| vsock | `VSOCKETS`, `VIRTIO_VSOCKETS` | Host↔guest RPC channel the supervisor uses. |
| PCI | `PCI`, `PCI_HOST_GENERIC` | Vz exposes PCI via "PCI Host Generic" controller. |
| Rootfs | `EXT4_FS`, `BLK_DEV_LOOP`, `DEVTMPFS`, `SQUASHFS`, `OVERLAY_FS` | Capsule rootfs is ext4 or squashfs+overlay; loop device for image mounts. |
| Capsule isolation | `CGROUPS`, `*_NS` (pid/net/mnt/user/uts), `NAMESPACES` | Matches the Linux crosvm guest contract. |

Plus 4 disabled subsystems (wireless, bluetooth, sound, DRM) for boot
trim — non-required in any capsule scenario; each strips a few MB +
seconds of boot probing.

**Authoring rules + merge_config.sh contract documented inline** so a
future maintainer doesn't need to read the kernel docs to add or
remove a knob.

### 2.2 `scripts/build-vmlinux-arm64.sh` (modified, +25 / −7)

Replaced the "treat ELASTOS_VMLINUX_CONFIG as a full .config" stage
with the canonical 3-stage kernel-config pattern:

```text
stage 1/3: make ARCH=arm64 defconfig            (baseline arm64 config)
stage 2/3: merge_config.sh -m .config <fragment> (apply overrides)
stage 3/3: make olddefconfig                     (resolve dependencies)
```

Each stage logs its purpose + appends to the same `build.log` file.
Failure at any stage exits with code 2 + tail of the log on stderr —
exit-code contract unchanged from Day 4a.

The change makes the recipe **kernel-version-agnostic**: bumping
6.1.59 → 6.1.X only requires changing `ELASTOS_VMLINUX_SRC_URL`; the
fragment continues to apply unchanged. (Without this change, the
operator would have had to derive a new 5000-line full .config from
each new kernel's `defconfig`.)

Header comments updated to reflect the fragment-based pattern.

### 2.3 `scripts/ci/local-day6-smoke.sh` (new, 200 LoC)

One-command orchestrator for the local Day-6 lane. 5-stage pipeline:

| Stage | Action | Typed exit on failure |
|---|---|---|
| 1 | Preflight delegate to `setup-mac-runner.sh` (skippable via `ELASTOS_LOCAL_DAY6_SKIP_SETUP=1` for fast iteration) | 1 |
| 2 | `cargo build -p elastos-server` (debug; skippable via `ELASTOS_LOCAL_DAY6_SKIP_BUILD=1`) | 2 |
| 3 | Vmlinux Image probe at `$XDG_DATA_HOME/elastos/bin/vmlinux`; prints exact rebuild recipe if absent | 3 |
| 4 | Run all 3 smokes with the correct env-var matrix; capture per-smoke log + exit + headline | (continues; aggregates in stage 5) |
| 5 | Triage summary: pass/fail table + per-fail headline + suggested next steps | 4 if any smoke failed |

**Smoke env-var matrix encoded as a single shared base + per-smoke
extension** so the runner contract is auditable in one place:

```bash
COMMON_ENV=(ELASTOS_VZ_SMOKE_FORCE_FULL=1)
CHAT_INTEROP_ENV=(ELASTOS_CHAT_INTEROP_OFFLINE=1 ELASTOS_BIN_OVERRIDE=…)
```

The chat-interop extension is needed because the gateway path
requires upstream binaries we don't have darwin-arm64 CIDs for yet
(Class-A `cid` fields are still empty pending the release pipeline).
The two other smokes use `elastos/target/debug/elastos` directly per
their existing hardcoded `ELASTOS_BIN` path.

**bash 3.2 clean** (verified with `/bin/bash -n`) — same constraint
as the smokes themselves.

**Live-tested today** end-to-end through stages 1–3 (the only stages
that can run on this Mac without the operator's `brew install` +
vmlinux build). Output:

```text
── 1. Preflight (delegate to setup-mac-runner.sh) ───────────
[local-day6] preflight green (full log: …/preflight.log)

── 2. Cargo build (debug elastos binary) ────────────────────
[local-day6] cargo build -p elastos-server (debug)…
[local-day6] binary ready: …/elastos/target/debug/elastos

── 3. Vmlinux Image probe ───────────────────────────────────
[local-day6] vmlinux NOT FOUND at …/elastos/bin/vmlinux.
The smokes will fail at the LaunchMicroVm step without a kernel
Image. Run the build recipe once (~30–40 min wall-clock on M1/M2):
    brew install aarch64-elf-gcc make elfutils openssl@3 bc jq
    bash scripts/build-vmlinux-arm64.sh
    mkdir -p …/.local/share/elastos/bin
    cp …/Image …/.local/share/elastos/bin/vmlinux
Then re-run this script.
```

Exit code = 3 (typed). ✅ The orchestrator correctly identifies the
operator gap + prints the exact remediation commands.

### 2.4 What did NOT change

- `.github/workflows/mac-vz.yml` — the `mac-vz-full-boot` self-hosted
  lane is still wired exactly as Phase 5 Day 6 shipped it. Day-6a's
  local lane is a complement, not a replacement; the self-hosted
  lane remains the correct path for gated CI.
- `components.json` — Day 6a doesn't touch metadata; Day-4b's vmlinux
  checksum populate is still operator-shipped (and the
  components-json-verify Class-C soft note still fires correctly).
- `scripts/ci/setup-mac-runner.sh` — unchanged; Day-6a's orchestrator
  delegates to it untouched.
- `scripts/lib/components-json-verify.sh` — unchanged; Day-6a doesn't
  change the manifest contract.

---

## 3. Quality gates — Day 6a (8 of 8 green)

### Gate 6a-1 — Kconfig fragment syntax sanity

```text
kconfig fragment OK
```

Each non-comment non-blank line matches `CONFIG_X=y|m|n|"..."|<num>`
or `# CONFIG_X is not set`. ✅

### Gate 6a-2 — `build-vmlinux-arm64.sh` syntax (post-modification)

```text
build script syntax OK
```

✅ The fragment-merge changes preserved bash syntax.

### Gate 6a-3 — `local-day6-smoke.sh` syntax (bash 5 + bash 3.2)

```text
bash 5 OK
bash 3.2 OK
```

✅ macOS-system-default `/bin/bash` (3.2.57) parses cleanly.

### Gate 6a-4 — Orchestrator typed exit-3 path lives

```text
exit=3 (expect 3)
```

✅ Missing-vmlinux remediation message + typed exit 3 work end-to-end.

### Gate 6a-5 — `setup-mac-runner.sh` + `components-json-verify` still green

```text
setup-mac-runner: exit=0
components-json-verify: exit=0
```

✅ Day-5a + Day-2/3/4a invariants preserved.

### Gate 6a-6 — `cross-platform-test.sh` 47/47

```text
cross-platform.sh: 47 passed, 0 failed
```

✅ Phase-5 baseline preserved.

### Gate 6a-7 — Mac dry-run pre-flight still 3/3

```text
PASS: local-carrier-setup-smoke.sh
PASS: home-frontdoor-smoke.sh
PASS: chat-wasm-native-interop-smoke.sh
```

✅ Day-3's headline outcome preserved.

### Gate 6a-8 — Diff scope

```text
M  scripts/build-vmlinux-arm64.sh
?? scripts/ci/local-day6-smoke.sh
?? scripts/release/vmlinux-arm64.config
```

✅ 3 files in this commit + this notes file + Day-6 banner edit = 5
files total. Same parsimony tier as Day 5a.

---

## 4. The Day-6b operator queue (single afternoon, ~45 min active time)

### Step 1 — Install toolchain (~5 min, one-time)

```bash
brew install aarch64-elf-gcc make elfutils openssl@3 bc jq
```

The build recipe + orchestrator both verify these are present and
exit with typed messages if any are missing.

### Step 2 — Build the kernel Image (~30–40 min, one-time per kernel-config change)

```bash
bash scripts/build-vmlinux-arm64.sh
```

Day-6a's modifications made this a single-command invocation —
`scripts/release/vmlinux-arm64.config` is now the default fragment;
no further config work needed. Outputs:

- `elastos/target/vmlinux-darwin-arm64/Image`
- `elastos/target/vmlinux-darwin-arm64/Image.sha256`
- `elastos/target/vmlinux-darwin-arm64/Image.size`

The recipe's terminal summary prints the exact `jq` command to update
`components.json` with the real checksum + size; running it is
optional for the local lane (the orchestrator doesn't verify the
checksum against components.json when components.json shows empty
checksum), but it's recommended so future runs catch any drift.

### Step 3 — Stage the Image at the runtime's install path (~5 s, one-time)

```bash
mkdir -p ~/.local/share/elastos/bin
cp elastos/target/vmlinux-darwin-arm64/Image ~/.local/share/elastos/bin/vmlinux
```

The runtime reads the Image from this path (via `components.json`
`vmlinux.darwin-arm64.install_path = "bin/vmlinux"`). Until the
release pipeline ships the kernel via CID-addressed download, this
hand-stage step is the bridge.

### Step 4 — Run the smokes (~5–10 min per cycle)

```bash
bash scripts/ci/local-day6-smoke.sh
```

The orchestrator prints a structured triage summary at the end. If
all three smokes pass, the Phase-6 substrate is end-to-end validated.
If any fail, follow the orchestrator's printed remediation matrix.

### Step 5 — Iterate (only if Step 4 surfaced bugs)

Each iteration:

```bash
# After applying a fix:
ELASTOS_LOCAL_DAY6_SKIP_SETUP=1 bash scripts/ci/local-day6-smoke.sh
```

The skip-setup flag saves ~5 s per run by not re-verifying the
unchanged HW/OS floors. Cargo rebuild remains a hot reload (~10 s
typically) so we don't skip stage 2 unless the binary is genuinely
unchanged.

### Step 6 — Update notes (once green)

When the orchestrator's triage block prints "Phase 6 Day 6a
local-lane: 3/3 GREEN":

1. Run each smoke once more to capture stable wall-clock numbers
   (median of 3 runs).
2. Append the timings + green-state baseline to a new
   "§ 5. Day-6b first-green outcome" section in this file.
3. Update `docs/vz-backend/PERFORMANCE_BASELINE.md` § "Mac local-lane
   timings" (new sub-section) with the same numbers — Day 7 expands
   that section with the `vz_perf_harness.rs` real-microVM-boot
   metric.

### Total Day-6b operator wall-clock

| Step | Wall-clock |
|---|---|
| 1. brew install | ~5 min |
| 2. vmlinux build | ~30–40 min (one-time) |
| 3. stage Image | ~5 s |
| 4. first smoke run | ~5–10 min |
| 5. per-iter (if needed) | ~5–10 min × N |
| **First-green sitting** | **~45–55 min** (assuming no bugs) |

If real-Vz substrate bugs surface on the first run (highly likely —
we've never run these three smokes end-to-end against real Vz with
real metadata before), each iter adds ~5–10 min. The expected number
of iters before green is 1–3 based on Phase 4/5 precedent.

---

## 5. Carry-forward to Day 7 + Phase 7

### Day-7 unblocked (modulo Day-6b green)

Day 7's headline gate is `vz_perf_harness.rs::perf_real_microvm_boot`
— a real-Vz launch path measurement. Day-6a + Day-6b together produce
the substrate Day 7 measures. **Day 7 starts when Day-6b produces a
3/3 green run.**

### Phase-7 carry-forward (cross-referenced with prior days)

- **Self-hosted GitHub Actions runner activation** (Day 5b operator
  handoff) remains the right substrate for *gated CI*. The local lane
  Day-6 shipped is for *developer-side validation*; the self-hosted
  lane is for *PR mergeability gating*. Both are valid; Phase 7 may
  activate the self-hosted lane once the local lane has shaken out the
  first round of substrate bugs.
- **Class-A/B `cid` populate** — the release pipeline's responsibility.
  When CIDs land, the smokes can run without `ELASTOS_BIN_OVERRIDE`
  and `ELASTOS_CHAT_INTEROP_OFFLINE`, exercising the full
  Carrier-install path. Until then the local lane is functionally
  equivalent + tracks the same Vz substrate.
- **vmlinux release-pipeline publish** — the operator-built Image
  from Day-4b/6b needs to be pushed to the release pipeline so other
  runners (and future installs) can fetch by CID. Phase 7 closes
  this loop.

---

## 6. Day-7 entry signal

Day 7 may start when:

- [x] All 8 Day-6a quality gates green (§ 3).
- [x] Kconfig fragment + modified build recipe + orchestrator live-
      tested through stages 1–3.
- [x] No regression on Day-5a's outcomes or Day-3's Mac pre-flight
      3/3.
- [ ] Day-6b operator handoff complete: vmlinux built + staged;
      `local-day6-smoke.sh` produces "3/3 GREEN"; wall-clock per
      smoke captured.

Three of four signals met at the time of this commit. The fourth is
the Day-6b operator handoff. Day 7 needs a real-Vz substrate
producing real-boot timings; Day-6b green is the gate.

---

**End of PHASE_6_DAY_6_NOTES.md.**
