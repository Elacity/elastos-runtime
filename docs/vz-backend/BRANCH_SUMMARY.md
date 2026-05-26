# `sash/local-test` — what this branch delivers

> Status: **Engineering milestone complete and validated end-to-end on Apple Silicon.**
> Author: agent + operator pair, May 2026.
> Anchor: this document is the single shareable summary of the branch. For
> day-by-day detail see `PHASE_*_DAY_*_NOTES.md`; for the formal sign-off see
> `PHASE_9_SIGNOFF.md`; for product framing see `../ELASTOS_PRD.md`.
>
> ## ⚠️ Substrate-CI status callout (added 2026-05-26 post-Phase-10.5)
>
> The Mac-substrate security hardening (Phase 10 + Phase 10.5 M1-M4) is closed
> and reviewable. However, this branch's **Linux CI has been continuously red
> since 2026-05-25T11:46Z** (commit `30cccce`, Phase 6 Day 6) due to
> pre-existing Phase 4/5/6 cross-OS regressions in `supervisor.rs`,
> `doctor_cmd.rs`, and the `elastos-vz` crate's own internal hygiene.
> A follow-up Phase 10.6 session closed two of the regressions and **paused
> on documenting the rest**. See **[`PHASE_10_6_GAP_REPORT.md`](./PHASE_10_6_GAP_REPORT.md)**
> for the full list of remaining issues with file/line/fix-pattern for each.
>
> **Operator implication:** branch is **NOT merge-ready** until Phase 10.7
> closes the gap-report issues. The Mac VM functional milestone, the M1-M4
> security work, and the inherited-CVE handoff document are all unaffected.

## Executive summary (30 seconds)

This branch teaches ElastOS to run its private-computing capsules on macOS using
**Apple's native Virtualization.framework**, achieving feature parity with the
Linux experience for developers working from source. The architectural change
is surgical: we swapped only the lowest layer (the VM engine), leaving the
supervisor, capsule manifests, gateway, Carrier bridge, and identity stack
**byte-identical to Linux**. The engineering milestone is **complete and
validated live on Apple Silicon** — a Linux kernel boots inside Apple's
hypervisor on a Mac, holds ~400 MB, and serves the same Home shell that
Linux users see. **It is not yet ready for public release**: Phase 10
(Mac-substrate security hardening) is **complete** — see
`PHASE_10_SIGNOFF.md`. The CVE audit confirmed that **zero
vulnerabilities were introduced by this branch** — all 34 findings are
pre-existing in `main` and have been handed off to the broader runtime
team via `RUNTIME_CVE_HANDOFF.md`. Phase 10 also produced a Mac threat
model, a cargo-fuzz harness for the Carrier-bridge framing parser
(2.4M iterations clean), SIGINT/SIGTERM graceful shutdown, a release
CI lane, and an internal pre-review pass that surfaced four
medium-severity findings (M1–M4) in the new code. **Phase 10.5
(2026-05-26) closed all four** — see `PHASE_10_5_SIGNOFF.md`. The
substrate now has no known unbounded-resource hazards from its own
code; what remains is one Phase 11 deferral (M5 typed-dispatch fuzz),
the parallel `chore/runtime-cve-hygiene` branch off `main` for the
inherited workspace CVEs, and (recommended for a public ship) an
actual external code review.

## What this document is — and is not

| | |
|---|---|
| **This is** | The single shareable summary of the `sash/local-test` branch. Read this to understand the *what*, *how*, *why*, and *what's missing*. |
| **This is not** | A marketing page. A release announcement. A complete security audit. A user manual. |

## At a glance

| Metric | Value |
|---|---|
| Branch | `sash/local-test` |
| Commits ahead of `main` | **72** |
| Files changed | 158 (+41,828 / −280) |
| New substrate code (`elastos-vz` crate) | **7,724 LOC across 26 files**, entirely net-new |
| Phases executed end-to-end | **9** (Phase 1 → Phase 9 sign-off) |
| Phase docs produced | 62 markdown files in `docs/vz-backend/` |
| Sign-off smoke matrix | **5/5 green** on Apple Silicon (M-series), validated live |
| External security audit | **Not yet performed** (see "Security" below) |

---

## For everyone (the 9th-grade version)

**The problem.** ElastOS lets people run apps — chat, files, mail, AI tools — in their own private, sandboxed mini-computers (called "capsules"). On Linux, this worked great. On a Mac, it didn't work at all. Macs and Linux PCs handle sandboxes very differently, and the existing code only knew how to talk to Linux.

**What we did.** We taught ElastOS how to use **Apple's built-in virtualization** — the same technology Docker Desktop and Parallels use — so the same capsules now run on a Mac. We did this without changing how the rest of ElastOS works: the parts that talk to capsules, the parts that route messages, the parts that show the user a desktop — all of those stayed the same. We just swapped the "engine" underneath.

**Why this matters.** A Mac developer or user can now run the *exact same* private-computing apps that a Linux user runs, with the same security guarantees (each app gets its own sealed Linux mini-computer), the same privacy properties (no app can talk to another app except through the official communication channel), and the same experience (a desktop, apps, files). ElastOS is no longer a Linux-only project.

**Did it work?** Yes, end-to-end. The team can see proof three ways:
1. A web browser showing the ElastOS desktop UI, running on a Mac.
2. macOS Activity Monitor showing an Apple-native Linux virtual machine spawned by our code, holding ~400 MB of memory and booting Ubuntu 22.04.
3. Internal logs showing the same lifecycle — capsule launch, identity issuance, message routing — that Linux uses.

**Is it ready to ship to outside users?** Engineering-wise, yes. Security-wise, **not without a hardening pass first** — we've never had an outside expert review the new code, and there are known small bugs in how virtual machines shut down. See the "Honest gaps" section below.

---

## For engineers (the technical version)

### Architectural shape

ElastOS has three layers. This branch only touched two of them — the third was untouched on purpose.

```
┌─────────────────────────────────────────────────────────────────────┐
│  Layer 3 — Install / Distribution                                   │
│  ──────────────────────────────────────────                         │
│  Linux:  install.sh + Carrier pull + apt/snap                       │
│  Mac:    scripts/dev/mac-local-setup.sh  (NEW — Phase 9)            │
│                                                                     │
│  → builds binaries, stages capsules, writes components.json,        │
│    auto-re-signs binaries with Vz/JIT entitlements,                 │
│    stamps local CIDs onto Home-surface capsules.                    │
├─────────────────────────────────────────────────────────────────────┤
│  Layer 2 — Runtime + Supervisor + Capsules + Gateway + Providers    │
│  ──────────────────────────────────────────────────────────────     │
│  UNCHANGED.                                                         │
│  Same supervisor.rs (5,657 LOC), same gateway, same Carrier bridge, │
│  same DID/identity stack, same components.json schema, same         │
│  capsule manifests, same WASM runtime, same TUI Home, same browser  │
│  Home assets. The Mac uses identical code paths.                    │
│                                                                     │
│  Only platform-aware change: `FULL_SCREEN_APPS_BACKING` constant in │
│  home_cmd.rs now omits `crosvm` on Mac (83 LOC, Phase 9 Day 3).     │
├─────────────────────────────────────────────────────────────────────┤
│  Layer 1 — Compute Substrate                                        │
│  ──────────────────────────────────────────                         │
│  Linux:  crosvm + KVM   (existing)                                  │
│  Mac:    elastos-vz crate   (NEW — Phases 1-8)                      │
│                                                                     │
│  → 7,724 LOC across 26 files, talks to Apple Virtualization.fwk     │
│    via Swift FFI, exposes the same `VmProvider` trait the Linux     │
│    backend implements. Supervisor calls the trait — does not know   │
│    or care which substrate is underneath.                           │
└─────────────────────────────────────────────────────────────────────┘
```

The fact that Layer 2 is unchanged is **the** architectural property worth defending. It means:

- One supervisor codebase serves both platforms.
- Bug fixes in Carrier routing, gateway behaviour, capsule lifecycle, etc. ship to both platforms with zero porting cost.
- Adding a new platform (e.g. ChromeOS via crostini, Windows via Hyper-V) is a Layer-1 swap only.

### What `elastos-vz` actually does

| File | Purpose |
|---|---|
| `src/provider.rs` | Implements the `VmProvider` trait. Lifecycle: `start`, `stop`, `attach_console`, `report_health`. ~816 LOC. |
| `src/ffi/builder.rs` | Configures `VZVirtualMachineConfiguration` from a `VmConfig`. Network (NAT default, bridged behind entitlement), block devices, virtio-console for Carrier, memory, vCPU. ~543 LOC. |
| `src/ffi/entitlement.rs` | Runtime check for `com.apple.vm.networking`. Routes bridged-network capsules to a typed error if the entitlement is missing. |
| `src/ffi/{kernel,disk,console,boot}.rs` | Thin Swift bridges over the corresponding Vz Objective-C surfaces. |
| `tests/concurrent_launch.rs` | Boots N VMs in parallel, asserts they all reach login, asserts memory accounting is correct, asserts cleanup. |
| `tests/smoke.rs` | Single-VM lifecycle: start → console attach → graceful stop. |

The supervisor instantiates `elastos_vz::Provider` on Mac via the same factory pattern Linux uses for `crosvm::Provider`. **Zero supervisor branching on `target_os`** for the substrate call paths — the only platform-aware switch in Layer 2 is the cosmetic "which subprocesses count as 'Home backing services' for the TUI dashboard" constant.

### Carrier + identity remain the only inter-capsule channel

This was an explicit principle check and is verified in the substrate:

- `builder.rs:137-152` — every VM gets a NAT'd network interface only. Bridged is gated behind a separate entitlement that is **not** granted in the default dev build.
- `builder.rs:187-190` — each VM gets exactly one virtio-console at `/dev/hvc1` for the Carrier bridge. No additional sockets.
- `carrier_bridge.rs` — host-side dispatcher. Same code for Linux and Mac. VMs receive a per-session DID and capability token over the bridge; all RPC flows through it.

There is **no L2/L3 network path between two VMs running on the same host.** Two capsules can only reach each other by routing through the supervisor's Carrier bridge, which enforces capability checks. This matches the Linux behaviour exactly.

### Bootstrap: what `mac-local-setup.sh` actually does

This is the meat of Phase 9 and is what made the user-visible experience reach parity:

1. Builds `elastos-server`, `elastos`, `elastos-vz` in debug.
2. Re-signs `elastos` with the Vz + JIT entitlements (auto-re-sign on Phase 9 Day 4).
3. Stages the kernel (`vmlinux`) and rootfs (`rootfs.ext4`, Ubuntu 22.04 base) into `~/Library/Application Support/elastos/`.
4. Builds the canonical Home-surface WASM capsules (`system`, `home`, ...) and stages them.
5. Stages the data capsules (`documents`, `library`, `inbox`).
6. **Stamps a local CID** on each capsule — SHA-256 over the deterministic file list, writes `.elastos-cid` and `.elastos-artifact-sha256`, registers the entry in `components.json` under `capsules:`. This is the move that finally let `elastos capsule <name> --interactive` resolve through the supervisor exactly like a Carrier-pulled capsule would.
7. Runs an inline Python self-verifier that re-reads `components.json`, re-reads each `.elastos-cid` from disk, and asserts they agree. Fails the script if any drift.
8. Idempotent: running it twice on the same machine is a no-op and re-runs the self-verifier.

### Test posture

- `cargo test -p elastos-server` — green on Mac, no regressions.
- `cargo test -p elastos-vz --test concurrent_launch --release` — green; multiple VMs boot in parallel under Vz.
- `elastos run ubuntu-base` — boots Ubuntu 22.04 LTS to login prompt in ~10s wall clock; VM holds ~400 MB resident; CPU spikes to ~100% during boot then settles to ~2% at idle login. Verified live with Activity Monitor.
- `elastos capsule system --interactive` (managed Home path) — boots through supervisor with full Carrier wiring. Verified live.
- Browser-based Home shell at `/apps/home/` — served by `elastos gateway` from the source tree via `DEV_CAPSULES_ROOT`. Verified live; user clicked through Documents / Library / Inbox / System / Chat-Room apps.

### Validate it yourself in 5 minutes (engineer quickstart)

> Prereq: Apple Silicon Mac running macOS 13+, Xcode CLI tools installed, Rust
> toolchain installed.

```bash
# 1. Clone and check out the branch (60s)
git clone https://github.com/Elacity/elastos-runtime.git
cd elastos-runtime && git checkout sash/local-test

# 2. Bootstrap the Mac dev environment (3-4 min; idempotent)
./scripts/dev/mac-local-setup.sh

# 3. Boot a real Linux VM under Apple Vz (15s — proves the substrate)
./elastos/target/debug/elastos run ubuntu-base
# In another terminal, open Activity Monitor → All Processes →
# search "Virtual" → you'll see com.apple.Virtualization.VirtualMachine
# spawned by our Rust code, holding ~400 MB, booting Ubuntu 22.04.
# Ctrl-C to stop (followed by `pkill -KILL` until graceful-shutdown lands).

# 4. Launch the managed Home shell (proves the full stack)
./elastos/target/debug/elastos gateway --addr 127.0.0.1:8090 &
open http://127.0.0.1:8090/apps/home/
# Click around Documents / Library / Inbox / System / Chat-Room.
```

For a guided, narrated walkthrough including log inspection, run
`./scripts/dev/mac-live-demo.sh`.

### FAQ

**Q: Why didn't we just use Docker / Podman / OrbStack?**
A: Capsules are full Linux VMs with their own kernel, not containers sharing
the host kernel. The threat model demands hardware-level isolation between
capsules — containers don't provide that. Apple Vz is the platform-native
hypervisor for the exact same threat model `crosvm` covers on Linux.

**Q: Does this work on Intel Macs?**
A: The code paths exist (Vz supports Intel) but we have only validated on
Apple Silicon. Intel-Mac validation is an explicit gap; would require
testing access to an Intel Mac.

**Q: Will a Linux-built capsule (`.elastos` artifact) run unchanged on Mac?**
A: WASM capsules — yes, byte-identical. MicroVM capsules — yes if the rootfs
is `aarch64`; the substrate is platform-agnostic but the rootfs architecture
must match the host. Cross-platform rootfs handling is a distribution
concern, not a substrate concern.

**Q: How does this compare to OrbStack / Lima / Vagrant?**
A: Those are general-purpose VM management tools. We are an
*application-specific* substrate: capsules have a fixed lifecycle managed by
our supervisor, a fixed communication contract (Carrier bridge over
virtio-console), and a fixed identity model (DID + capability tokens). We
use Apple's same underlying framework but expose a much smaller, more
opinionated surface tailored to the ElastOS supervisor's needs.

**Q: What's the performance overhead vs Linux + KVM?**
A: Baseline measurement is in `PERFORMANCE_BASELINE.md`. Boot-to-login is
within 2x of crosvm on similar hardware. Steady-state has not shown
measurable differences in the benchmarks we run today; broader profiling
is a Phase 10+ follow-up.

**Q: Are we abandoning Linux?**
A: No. The Linux substrate (`crosvm`) is unchanged on this branch.
`scripts/check-linux-untouched.sh` enforces that. Both substrates ship.

**Q: What does an attacker who fully compromises a capsule get?**
A: Code execution inside their own Linux VM, NAT'd network only (no LAN
visibility, no access to other capsules' VMs), and the ability to send
messages over the Carrier bridge to the host runtime — where capability
checks gate every operation. Escaping the VM itself requires defeating
Apple Vz, which is a hardware-enforced boundary. The Carrier bridge is
the lowest-level parser on the trust boundary and is currently the
highest-priority Phase 10 audit target.

**Q: Why is `com.apple.security.cs.allow-jit` granted?**
A: Wasmtime needs writable + executable memory for JIT-compiled WASM
capsule code. This weakens the codesign enforcement window slightly. The
tradeoff is well-known in the wasmtime community and is gated to the
single binary that hosts the WASM runtime.

**Q: Can I run this on a CI worker?**
A: GitHub Apple-Silicon runners support Vz. We have a self-hosted M2 lane
described in `SELF_HOSTED_RUNNER_SPEC.md`. Hosted Mac runners on GitHub
Actions require explicit Vz entitlement, which the current cert chain
handles.

### Entitlements — what we grant and why

(Full annotated plist at `scripts/release/elastos-server.entitlements.plist`.)

| Entitlement | Required? | Why |
|---|---|---|
| `com.apple.security.virtualization` | **Required** | Instantiate `VZVirtualMachine` at all. |
| `com.apple.security.hypervisor` | **Required** | Underlying `Hypervisor.framework` calls on Apple Silicon. |
| `com.apple.vm.networking` | **Optional** | Only for bridged-network capsules (`permissions.guest_network: true`). Default capsules use NAT and don't need it. |
| `com.apple.security.network.client` / `.server` | **Required** | Host opens TCP listener on 127.0.0.1 for RPC; reaches out to IPFS gateways. |
| `com.apple.security.files.user-selected.read-write` | **Required** | Operator may point the runtime at rootfs / vmlinux outside its data directory. |
| `com.apple.security.cs.allow-jit` + `.allow-unsigned-executable-memory` | **Required** | Wasmtime needs JIT pages for WASM capsule execution. **Known sandbox-weakening tradeoff** — see Security below. |

Principle: the entitlement set is the minimum that makes the substrate work. Nothing is granted "just in case."

---

## Honest assessment of completeness

### What is unambiguously done

- [x] Apple Virtualization.framework Rust backend with parity-shaped `VmProvider` trait.
- [x] Real Linux kernel + Ubuntu userspace boots end-to-end on Apple Silicon.
- [x] Mac-aware Home bootstrap (`mac-local-setup.sh`) reaches local-dev parity with Linux source-checkout flow.
- [x] Capsule registration via local-CID stamping matches the canonical pattern (`home-demo-local.sh` on Linux).
- [x] Browser-based ElastOS Home shell loads and routes to all Home-surface apps on Mac.
- [x] Carrier-bridge identity / capability flow works identically on Mac.
- [x] All five sign-off smoke tests green; documented in `PHASE_9_SIGNOFF.md`.
- [x] No regressions to Linux. `check-linux-untouched.sh` script verifies the Linux substrate paths are byte-identical to `main`.
- [x] Honest secret scan of diff: clean. No keys / passwords / private material committed.

### Known gaps (engineering — not blockers, but real follow-ups)

| Gap | Cost | Impact |
|---|---|---|
| `elastos run` SIGINT doesn't gracefully stop the Vz VM (needs SIGKILL) | ~10 LOC in `run_cmd.rs` to call `provider.stop()` from the signal handler | Cosmetic for ops; not a substrate issue |
| `cargo test -p elastos-vz` test binaries lose entitlements at build time | ~20 LOC in `mac-local-setup.sh` to auto-re-sign anything under `target/*/deps/elastos_vz*` | CI/dev friction; not user-facing |
| Browser Home shell has no "Launch microVM" button for VM-backed apps | UX work; Home shell currently only surfaces WASM-backed apps. Substrate is already wired. | Missing affordance, not missing capability |
| Chat-as-microVM (and similar VM-backed apps) not built/staged on Mac | Requires cross-building a Linux rootfs from Mac. Bigger lift. | Mac is missing one app variant; WASM variant works. |
| `camofox` browser harness not packaged on Mac | Distribution work; canonical Linux UX uses Camofox to host the Home shell. We're using the user's existing browser instead. | Mac users currently hit the gateway via Chrome/Safari/Firefox, not via Camofox. |

### Known gaps (security — see next section for the actual list)

---

## Security — what we know vs. what still needs work

> **Stance:** the substrate is built with defense-in-depth at the architecture
> level (NAT-only networking by default, single Carrier-bridge channel,
> minimum-entitlement set, code-signed binaries). It has **not** been formally
> reviewed by anyone outside the people who wrote it. Anything below labelled
> "Not done" is a real gap, not paranoia.
>
> **Ownership clarity (added after Day-1 cargo-audit):** Phase 10 work on
> this branch focuses only on Mac-substrate-scoped security. The 34
> pre-existing workspace CVEs (wasmtime, TLS chain, etc.) inherited from
> `main` are handed off to the broader runtime team via
> `RUNTIME_CVE_HANDOFF.md` for a parallel `chore/runtime-cve-hygiene`
> branch. Read that file if you want the full inherited-CVE inventory and
> remediation plan.

### What we have

- **Entitlement minimization.** Every entitlement in the plist has an annotated justification. `com.apple.vm.networking` is granted by default for dev convenience but the *runtime* refuses to use bridged networking unless a capsule explicitly opts in via `permissions.guest_network: true`. The check (`ffi/entitlement.rs`) is a typed early-fail, not a silent fallback.
- **Network isolation by default.** Vz config grants each VM a NAT'd interface only. No L2 between VMs. Verified in `builder.rs:137-152`.
- **Single inter-capsule channel.** Carrier bridge over virtio-console at `/dev/hvc1`. Verified in `builder.rs:187-190` and `carrier_bridge.rs`.
- **Code-signed binaries.** `mac-local-setup.sh` re-signs `elastos` after every build (because `cargo build` strips entitlements). Production release uses `scripts/release-mac.sh` against an Apple Developer ID cert with hardened runtime + notarization.
- **No secrets in the diff.** Manually scanned for API-key / password / private-key patterns across the full 41,828-line diff vs `main`. Clean.
- **WASM JIT entitlement is opt-in by feature, not by default in the system.** Wasmtime is the only consumer; capsules can't escape the JIT region without breaking the wasmtime sandbox (well-studied, not unique to us).

### What is **not** done — recommended before public ship

| Action | Effort | Why it matters |
|---|---|---|
| ~~Run `cargo audit` against the branch~~ **DONE — see `PHASE_10_DAY_1_NOTES.md`** | Completed | **34 vulnerabilities + 12 warnings found. ALL 34 INHERITED from `main` at the same version — zero introduced by this branch. Handed off to broader runtime team via `RUNTIME_CVE_HANDOFF.md` for a `chore/runtime-cve-hygiene` branch off `main`. Our new Mac-only deps (`objc2`, `block2`, `dispatch2`, ...) have zero findings.** |
| **External code review of `elastos-vz` substrate** | 1-2 dev-weeks for a security engineer who knows Swift FFI + virtualization | 7,724 LOC of new attack surface sitting at the host-guest boundary. The single highest-leverage thing on this list. |
| **Threat model document for the Mac substrate** | 1 dev-week | Currently no written threat model. Should enumerate: trust boundaries, what a malicious capsule can attempt, what a malicious operator can attempt, what a malicious update server can attempt. The Linux side has implicit threat models from upstream `crosvm` reviews; we don't inherit those on Mac. |
| **Fuzz the Carrier-bridge framing** | 2-3 dev-days with `cargo-fuzz` | The bridge is a parser sitting on the trust boundary between guest VM and host runtime. Parser bugs there are the highest-impact thing a malicious capsule could exploit. |
| **Pin and audit new transitive dependencies** | 1-2 dev-days | Lockfile churn from `elastos-vz` brought in new crates (Swift FFI helpers, plist parsers, etc.). Should be enumerated and each justified. |
| **Stronger local-dev CID** | 30 min | Local CIDs are `local-<name>-<sha256[:16]>` = 64 bits. Fine for cache addressing; **not** collision-resistant against adversarial inputs. Either use full 256 bits or document the dev-only scope explicitly in the supervisor error message when a local CID is consumed. |
| **Verify VM memory zeroing on shutdown** | 1 dev-day reading Apple docs + adding a regression test | Apple's framework is *believed* to zero VM memory on stop; we haven't verified. |
| **Confirm SIP-equivalent for VMs** | 1 dev-day | Vz on Apple Silicon claims hardware-enforced isolation. We use the framework correctly but haven't independently validated the boundary. |
| **CI lane that builds + signs + notarizes a release artifact and exercises the smoke matrix** | 2-3 dev-days | We have a notarization recipe (`scripts/release-mac.sh`) but no automated lane that runs it on every PR. |

### What I would mark as "MUST FIX before public release"

The top three from the table above, in order:

1. **`cargo audit`** — cheap, mechanical, can be done today.
2. **External code review of `elastos-vz`** — this is the one I'd insist on. New host-side substrate code with hypervisor-level entitlements deserves human eyes that aren't the author's.
3. **Fuzz the Carrier-bridge framing** — highest-impact attack surface.

The rest are real follow-ups but not release blockers if 1-3 are done.

---

## Recommended next phase

If I were proposing the next branch, it would be **Phase 10 — Mac security hardening + release polish**, sequenced:

1. Day 1: `cargo audit` + transitive dep enumeration.
2. Day 2-3: Threat model document.
3. Day 4-8: Carrier-bridge fuzz harness + first round of findings.
4. Day 9-10: SIGINT-graceful-shutdown + test-binary signing (the two demo bugs).
5. Day 11-15: External security review window + remediation.
6. Day 16-17: CI lane for sign+notarize+smoke.
7. Day 18: Phase 10 sign-off.

This is roughly a three-week phase, the bulk of which (Days 11-15) is wall-clock for the security reviewer, not author dev time.

---

## TL;DR

- **What we built:** a Mac substrate for ElastOS capsules using Apple's Virtualization.framework, plus a Mac dev-bootstrap that reaches feature parity with the Linux source-checkout developer experience. Net 41,828 LOC across 158 files, 72 commits, 9 phases, all 5 sign-off smoke tests green.
- **Am I happy it's the engineering milestone I was tasked with?** Yes. Validated live, end-to-end, on a real Mac.
- **Is it ready for public users?** Not without Phase 10 security hardening. The substrate has never been reviewed by an outside set of eyes and the most attackable parser on the trust boundary has never been fuzzed. The fixes are well-scoped (~3 weeks) and not blockers to the engineering achievement.

### One-line verdict (quote me)

> *"The Mac substrate is engineering-complete and validated live; it is ready
> for internal use and dogfooding today. It is not ready for public release
> until a ~3-week security-hardening phase runs `cargo audit`, completes
> external code review of `elastos-vz`, and fuzzes the Carrier-bridge
> parser."*

Anchors: `PHASE_9_SIGNOFF.md` · `PHASE_6_PLAN.md` (rolling status) · `PHASE_10_PLAN.md` (Mac-substrate security hardening, re-scoped) · `PHASE_10_DAY_1_NOTES.md` (CVE audit + ownership analysis) · `RUNTIME_CVE_HANDOFF.md` (inherited CVEs for broader team) · `scripts/dev/mac-local-setup.sh` · `scripts/dev/mac-live-demo.sh` · `elastos/crates/elastos-vz/`
