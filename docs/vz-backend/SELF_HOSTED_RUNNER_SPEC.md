# Self-Hosted Mac Runner Specification (Phase 5 Day 6 → Phase 6 Day 5a)

> Authoritative spec for a self-hosted **macOS Apple-Silicon** GitHub Actions
> runner that can execute the `mac-vz-full-boot` job in
> [`.github/workflows/mac-vz.yml`](../../.github/workflows/mac-vz.yml).
>
> **Status:** Spec authored in Phase 5 Day 6. Phase 6 Day 5a turned the spec's
> § 4 provisioning checklist into the executable recipe
> [`scripts/ci/setup-mac-runner.sh`](../../scripts/ci/setup-mac-runner.sh);
> the smokes' Mac pre-flight no longer visible-skips (Phase 6 Days 2–3 landed
> the `darwin-arm64` `components.json` metadata; Day 4a landed the kernel
> build recipe). The runner is **not yet physically provisioned**; this
> document remains the contract a future operator follows. See § 4.5 for the
> one-command preflight the operator runs first.

---

## 1. Why this exists

GitHub-hosted macOS runners (`macos-latest`, `macos-14`, etc.) do not reliably
support `Virtualization.framework` workloads. Nested virt is restricted, host
networking is limited, and the runner image is recycled between jobs. The
Phase-5-Day-1..5 work landed the **dry-run CI substrate** (build + helper-test +
shell-smoke-syntax) but the **end-to-end Vz microVM boot** needs a real
Apple-Silicon machine.

This document specifies that machine.

---

## 2. Hardware & OS

| Field             | Requirement                                                                                                                |
| ----------------- | -------------------------------------------------------------------------------------------------------------------------- |
| CPU               | Apple Silicon (M1 / M2 / M3 / M4 family). Intel Macs are **not supported** (Phase 5 targets `arm64-darwin` exclusively).   |
| RAM               | ≥ 16 GiB. Vz microVMs cap out at 4 GiB each; 16 GiB leaves headroom for the host + runner + concurrent capsules.           |
| Disk              | ≥ 100 GiB free on the volume that hosts `~/.local/share/elastos`. Rootfs caches + overlays can grow during a long run.     |
| macOS             | macOS 13 (Ventura) or newer. macOS 12 (Monterey) is supported by the Vz crate but several runtime APIs are macOS-13-only.  |
| Network           | Outbound to `github.com`, `crates.io`, `download.developer.apple.com`. No inbound port is required by the runner agent.    |
| Power             | Always-on or wake-on-LAN. The 6-hour heartbeat probe needs the runner to be awake to register a successful claim.          |

---

## 3. Labels (exact set)

The runner **must** be registered with all four labels below. Any subset
will cause `mac-vz-full-boot` and `_self-hosted-probe.yml::probe-attempt`
to never schedule.

```text
self-hosted
macOS
ARM64
vz-capable
```

The first three are the GitHub-Actions-standard labels for "this is an
Apple-Silicon Mac". The `vz-capable` label is **owner-applied** and signals
"this machine has been provisioned per § 4 below and is allowed to run full
Vz boots". Removing `vz-capable` from a runner is the kill-switch — the job
will queue until timeout and an operator can pull the runner from the pool
for maintenance.

---

## 4. Provisioning checklist

The following steps prepare a fresh Mac for the runner role. They are
ordered so a partial completion still leaves the machine in a usable state.

### 4.1 Pre-runner system setup

1. Install Xcode Command-Line Tools: `xcode-select --install`.
2. Install `rustup` and the stable toolchain:
   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
   ```
3. Install Bash 5+ (optional but recommended — the smokes are Bash-3.2
   compatible, but Bash 5 gives better stack-trace output on failures):
   ```sh
   brew install bash
   ```
4. Verify `Virtualization.framework` is present (it is on every supported
   macOS, but the probe script asserts this anyway):
   ```sh
   test -d /System/Library/Frameworks/Virtualization.framework && echo OK
   ```

### 4.2 Provision the elastos data directory

The full smokes expect `~/.local/share/elastos` to exist with a warmed
kernel/rootfs cache. **Phase 6 Day 5a turned this into a recipe:**

```sh
bash scripts/ci/setup-mac-runner.sh
```

The recipe verifies HW/OS prereqs, installs toolchains where absent,
delegates to [`scripts/lib/components-json-verify.sh`](../../scripts/lib/components-json-verify.sh)
to confirm manifest invariants, probes for the operator-built vmlinux
Image (Day-4b artefact), reports the Class-E helper cache state, and
prints the exact `gh variable set` + runner-registration commands for
the next steps. See § 4.5 below for full details.

> **Today's reality (post Phase 6 Day 4a):** the `darwin-arm64` release
> metadata is **structurally present** in `components.json` for all
> Class-A/B/C/E entries. The Mac pre-flight no longer skips the smokes;
> they now run their real Vz boot path. **Day 4b operator handoff is
> still pending** for the `vmlinux` checksum + signed `elastos-server`;
> when this completes the smokes have a real kernel to boot. Until
> then the smokes detect the missing artefact at runtime and exit with
> a typed error (NOT a pre-flight skip), which is a real test signal.

### 4.3 Install the GitHub Actions runner agent

Follow GitHub's standard self-hosted runner install flow
(`Settings → Actions → Runners → New self-hosted runner`). When prompted
for labels, apply:

```text
self-hosted,macOS,ARM64,vz-capable
```

Install as a launch-agent so the runner survives reboots:

```sh
./svc.sh install
./svc.sh start
```

### 4.4 Enable the lane

Once the runner shows **Idle** in the GitHub UI:

1. Settings → Secrets and variables → Actions → **Variables** → New repo variable.
2. Name: `MAC_VZ_FULL_BOOT_ENABLED`. Value: `true`.
3. Trigger `_self-hosted-probe.yml` from the Actions UI ("Run workflow")
   and confirm the `probe-attempt` job completes (< 1 min).

The next `mac-vz.yml` run will pick up the new gate state and schedule
`mac-vz-full-boot` on the runner.

### 4.5 One-command preflight (Phase 6 Day 5a addition)

The recipe at [`scripts/ci/setup-mac-runner.sh`](../../scripts/ci/setup-mac-runner.sh)
collapses § 4.1 + § 4.2 + the pre-flight checks into a single
re-runnable bash script. It's idempotent: re-running it on a partially
provisioned machine completes the gaps and reports what was already
done.

The recipe enforces the floors in § 2 (arm64, macOS ≥ 13, RAM ≥ 16 GiB,
free disk ≥ 100 GiB) and uses typed exit codes:

| Exit | Meaning                                                                                                                      |
|----:|------------------------------------------------------------------------------------------------------------------------------|
| 0   | All checks green; provisioning ready for § 4.3 (runner agent install) + § 4.4 (variable flip).                              |
| 1   | HW/OS prerequisite failed (Intel Mac, macOS < 13, RAM/disk floor). Diagnostic on stderr names the failing check.            |
| 2   | Toolchain install failed (xcode-select / rustup error). Stderr captures the underlying tool failure.                        |
| 3   | `Virtualization.framework` absent (impossible on a supported macOS — fail hard so the operator notices a corrupted system). |
| 4   | `components.json` verifier failed (manifest drift). Re-run Phase 6 Day 1–4 verifier to localise the regression.             |

The recipe deliberately does **not** register the runner agent or set
the repository variable: the registration token is single-use and
short-lived, and the variable flip needs repo-admin credentials. The
recipe prints the exact commands for both steps in its terminal
hand-off block at the end.

---

## 5. Security posture

The self-hosted runner executes **arbitrary code from any push or PR** to
this repository (the workflow triggers don't filter contributor identity
because Day 6 doesn't change `mac-vz.yml`'s triggers — Days 1..5 already
opened the dry-run lane to PRs). To stay safe:

- **Hardware isolation.** The runner machine must NOT also be a developer
  workstation. Treat it as an appliance with no persistent secrets beyond
  the Actions runner token.
- **No long-lived shell sessions.** Do not log into the machine for
  unrelated work; SSH access should be operator-only.
- **Network segmentation.** If your network allows it, place the runner
  on a separate VLAN that can reach the internet but not internal LAN
  resources. The smoke tests do not need LAN connectivity.
- **Restricted repo variable.** Only repository admins should be able to
  toggle `MAC_VZ_FULL_BOOT_ENABLED`. The workflow gate respects the
  variable instantly, so a misconfigured PR cannot raise its own gate.
- **Audit.** `_self-hosted-probe.yml::probe-fallback` always runs on
  `ubuntu-latest` and prints the variable's current value, giving you a
  scheduled audit trail in the Actions UI for "when was the lane on?".
- **Kill switch.** Remove the `vz-capable` label or unset the variable —
  either action immediately stops new jobs scheduling on the runner.

---

## 6. Day-6 acceptance: what the runner must do

A successfully-provisioned runner satisfies the following:

- [ ] `_self-hosted-probe.yml::probe-attempt` completes in < 1 minute
      with **all three** of `sw_vers`, `uname -a`, `Virtualization.framework
      PRESENT` printed in the log.
- [ ] `mac-vz.yml::mac-vz-full-boot` completes within its 30-minute
      timeout, with the three smoke steps printing `FORCE_FULL=1` and
      either (a) running to full completion or (b) failing with a
      structured Vz error (which is a successful test of the Phase-4
      `VzError` plumbing, not a Day-6 failure).
- [ ] No tests on the GitHub-hosted lane (`mac-rust-tests`,
      `mac-shell-helpers`, `mac-smokes-dry-run`) regress.
- [ ] The Linux-untouched gate (`linux-untouched.yml`) stays green.

---

## 7. What this spec does NOT cover (yet)

The following are intentionally **Phase 6+** deliverables, with current
status as of Phase 6 Day 5a:

- **darwin-arm64 release metadata** in `components.json` — Phase 6 Days
  2–4a **completed structurally**; Class-A/B/D/E green, Class-C
  awaiting Day-4b operator handoff for the `vmlinux` `{checksum,size}`
  populate.
- **Code signing + notarisation** of the elastos binaries shipped to the
  runner. **Phase 6 Day 4a shipped** the recipe + entitlements
  ([`scripts/release-mac.sh`](../../scripts/release-mac.sh) +
  [`scripts/release/elastos-server.entitlements.plist`](../../scripts/release/elastos-server.entitlements.plist));
  Day 4b operator handoff runs them against an Apple Developer Program
  cert. The Day-6 self-hosted runner accepts an un-notarised dev
  binary today because the runner is a controlled environment; the
  public release flips this on per
  [`PHASE_6_DAY_4_NOTES.md`](./PHASE_6_DAY_4_NOTES.md) § 4 Gate 4b-6.
- **Performance benchmarks.** The full-boot lane currently has no
  perf-regression detection. A Phase 6 follow-up will land a
  capture-and-compare baseline.
- **Multi-runner fleet.** Day 6 assumes a single self-hosted runner. A
  fleet (e.g. 4 machines for parallel smoke coverage) needs Action-level
  matrix work that's not in scope today.
- **Auto-recovery.** If the runner machine kernel-panics, an operator
  has to reboot it. There is no out-of-band recovery channel.

---

## 8. References

- [`.github/workflows/mac-vz.yml`](../../.github/workflows/mac-vz.yml) — the Day-5/Day-6 workflow.
- [`.github/workflows/_self-hosted-probe.yml`](../../.github/workflows/_self-hosted-probe.yml) — heartbeat probe.
- [`scripts/ci/setup-mac-runner.sh`](../../scripts/ci/setup-mac-runner.sh) — **Phase 6 Day 5a** one-command preflight (§ 4.5).
- [`scripts/build-vmlinux-arm64.sh`](../../scripts/build-vmlinux-arm64.sh) — Day-4b operator recipe for the `vmlinux` Image.
- [`scripts/release-mac.sh`](../../scripts/release-mac.sh) — Day-4b operator recipe for codesign + notarize + staple.
- [`docs/vz-backend/CI_RUNBOOK.md`](./CI_RUNBOOK.md) — operator runbook for CI.
- [`docs/vz-backend/PHASE_5_DAY_6_NOTES.md`](./PHASE_5_DAY_6_NOTES.md) — Day-6 completion notes.
- [`docs/vz-backend/PHASE_5_PLAN.md`](./PHASE_5_PLAN.md) § Day 6 — the upstream prompt.
- [`docs/vz-backend/PHASE_6_DAY_4_NOTES.md`](./PHASE_6_DAY_4_NOTES.md) — Day-4a/4b split + Day-4b operator queue.
- [`docs/vz-backend/PHASE_6_DAY_5_NOTES.md`](./PHASE_6_DAY_5_NOTES.md) — Day-5a/5b split (this commit).
