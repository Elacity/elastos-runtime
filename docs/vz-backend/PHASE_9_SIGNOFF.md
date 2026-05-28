# Phase 9 — Sign-off

> **Outcome (2026-05-26):** Mac source-checkout developer
> experience is at parity with the Linux source-checkout
> developer experience. All five sign-off smokes green on this
> Mac, on the `sash/local-test` branch. The VM-substrate swap
> from Phases 1-8 holds — Layer 1 (`elastos-vz` ↔ `crosvm/qemu`)
> required zero changes in Phase 9; Layer 2 (supervisor,
> capsules, gateway, providers) required one cfg-gated constant
> (Day 3); Layer 3 (install / distribution) is where Phase 9
> spent its substance, replacing the Linux Carrier-based install
> pipeline with a source-build bootstrap so Mac developers can
> reach the same operator UX without waiting on a Mac release
> channel that doesn't exist yet.

## 1. The three-layer architectural audit

| Layer | LOC touched in Phase 9 | Files | Substance |
| ----- | ---------------------- | ----- | --------- |
| **Layer 1** — VM substrate (`elastos-vz` provider, microVM lifecycle) | **0** | 0 | Substrate work closed at Phase 8 Day 8 |
| **Layer 2** — Runtime substrate (supervisor, capsules, gateway, providers, run_cmd, capsule_cmd, viewer_gateway, browser_capsules, carrier_bridge) | **0** | 0 | Trait abstraction held — runtime is identical Linux ↔ Mac |
| **Layer 2½** — System-services dashboard (`home_cmd::SystemServiceSpec`) | **83** | 1 (`home_cmd.rs`) | Day 3 — single `#[cfg(target_os = "macos")]` constant for `FULL_SCREEN_APPS_BACKING` (vmlinux-only on Mac because Vz is embedded in the binary, not a separate executable like crosvm). Pure platform-conditional, no logic change. |
| **Layer 3** — Install / bootstrap (`scripts/dev/mac-local-setup.sh`) | **502** | 1 | Mac-equivalent of `install.sh + elastos setup --profile demo` — builds providers from source, stages capsules, mints local CIDs, auto-resigns the dev binary when cargo strips entitlements |

Net substrate touched across the entire phase: **83 LOC in one file**
(`home_cmd.rs`), all under `#[cfg(target_os = "macos")]`. Everything
else lives in a bootstrap script and docs. The runtime that runs on
Mac is the same Rust binary that runs on Linux, with the same
ComputeProvider trait selecting `elastos-vz` (Mac) or `crosvm`/`qemu`
(Linux) at startup.

## 2. Sign-off matrix

Each smoke is a one-line invocation against the `sash/local-test`
branch on this Mac. The matrix is additive — each row proves a layer
is healthy, and #5 proves all three layers wire together end-to-end.

| # | Smoke | Result | Proves |
| - | ----- | ------ | ------ |
| 1 | `bash scripts/dev/mac-local-setup.sh` | **GREEN** — exit 0; 6/8 services ready; 5/5 capsules registered with matching `.elastos-cid` ↔ `components.json` | Layer 3 idempotency + Day 4 auto-resign + Day 5 CID stamping |
| 2 | `cargo test -p elastos-server --no-fail-fast` | **GREEN** — 517 passed, 0 failed, 2 ignored (identical to Day 3 baseline) | Layer 2 zero regressions on Mac |
| 3 | `elastos run capsules/home` | **GREEN** — `home capsule launched … → [run] WASM capsule 'home' exited`, exit 0 | Layer 1 + 2 standalone (Hardened Runtime + JIT entitlements + wasmtime in-process) |
| 4 | `cargo test -p elastos-vz --test concurrent_launch --release` | **GREEN** — 3/3 (`single_vm_boots_to_userspace`, `concurrent_load_with_real_kernel`, `concurrent_load_rejections_isolate_per_vm`) | Layer 1 boots real Linux kernel through Apple Vz |
| 5 | `elastos capsule system --lifecycle interactive --interactive` | **GREEN** — `Runtime started … vz provider enabled … Loading capsule 'system' (Wasm) … WASM bridge active … system capsule launched` | Full canonical chain: resolve-plan → ensure-capsule (cached-CID match) → run_wasm_capsule → wasmtime |

Total: **5/5 GREEN.**

## 3. What this means in practice

A Mac developer with the repo cloned at `sash/local-test` and the
JIT entitlement plist present can run:

```bash
$ bash scripts/dev/mac-local-setup.sh   # one-time bootstrap
$ elastos home                          # full TUI dashboard
$ elastos capsule system --interactive  # launch a capsule through managed Home
$ elastos run capsules/home             # standalone WASM
```

and get the same operator UX a Linux developer gets with
`bash scripts/home-demo-local.sh`. The substrate underneath swaps
from KVM/crosvm to Apple's Virtualization.framework transparently;
the operator experience above the substrate is identical.

## 4. Honest list of known gaps

These are real gaps that are deliberately not part of this phase's
sign-off because they belong to other phases or are pre-existing
non-Mac-specific issues:

- **Mac test-binary signing for `cargo test -p elastos-vz`.** Day 4's
  auto-resign in `mac-local-setup.sh` only signs `target/debug/elastos`,
  not the integration test binaries under `target/release/deps/`. The
  error message itself prints the canonical recipe
  (`scripts/dev/sign-elastos-vz/sign.sh <test-binary>`) so the
  operator hits a clean, instructive error rather than silent failure.
  **Day-6 candidate:** extend the bootstrap to optionally sign test
  binaries after they're built. ~10 LOC.

- **Camofox browser-Home on Mac.** The canonical browser-hosted Home
  UX (used by `home-camofox-smoke.mjs` on Linux CI) needs a Mac
  camofox build to be exercisable on Mac. The gateway routes
  (`/apps/<name>/*path`) and the launch token machinery
  (`/api/apps/home/launch`) are already wired and would Just Work
  against the now-registered home-surface capsules; only the browser
  client is missing. **Out of scope for Phase 9.**

- **Terminal `elastos home` → Data-capsule launch (Gap B).** The
  terminal Home action handler routes capsule launches through
  `capsule_cmd::run_capsule` which falls through to
  `Supervisor::launch_capsule` with no `CapsuleType::Data` branch.
  This is a **pre-existing Linux limitation** and affects both
  platforms equally — terminal Home was never the canonical UX for
  Data capsules; browser-hosted Home is. **Not Mac-specific.**

- **`brew install elastos` / signed `.dmg`.** No Mac release channel
  exists yet. Mac source-checkouts use `mac-local-setup.sh`; a real
  installer requires a notarised distribution pipeline that's a
  release-engineering phase, not a substrate phase. **Out of scope.**

- **Third-party dependencies for the last 2 system services.** `Content
  Exchange` (needs `kubo`) and `Public Edge` (needs `cloudflared`)
  show `[no ]` on a clean Mac because they're not in PATH. Same
  gap exists on a clean Linux source checkout. `brew install kubo
  cloudflared` flips both to `[ok ]`. **Not a Mac substrate issue.**

## 5. The principle-checks that shaped the phase

Three operator interventions changed the phase trajectory and are
worth recording so the same traps don't recur in later phases:

1. **Day 3 — "is this row red because of a Mac substrate issue or
   because of a hard-coded Linux assumption?"** Caught the
   `SystemServiceSpec.backing = ["crosvm", "vmlinux"]` hard-code that
   guaranteed the row was red on Mac forever. Turned a phantom
   "incomplete substrate" complaint into one cfg-gated constant.

2. **Day 4 — "wait, why does this work, then fail, then work again?"**
   Caught the silent codesign-strip footgun: every `cargo build`
   invalidates Vz/JIT entitlements, and the failure modes are
   invisible to `elastos home --status` (which doesn't need Vz or
   JIT). Turned an intermittent ghost-bug into an idempotent
   bootstrap check.

3. **Day 5 — "does the original have HTTP wiring? remember our
   principles, please just double check."** Caught the design pull
   to invent a Mac-specific short-circuit in `capsule_cmd::run_capsule`
   that would have bypassed `resolve-plan` + `ensure-capsule`
   entirely. The real fix lived in the bootstrap (mirror the
   canonical chat-staging local-CID pattern from
   `scripts/home-demo-local.sh`), not the substrate. Turned a
   substrate change into a 60-LOC bash + python patch.

Each intervention saved net LOC, kept the substrate clean, and
preserved Linux ↔ Mac parity. Phase 9 spent 502 bootstrap LOC + 83
substrate LOC because the principle-checks shrank it from what
would otherwise have been multiple times that.

## 6. Closing position

The substrate is done. The developer experience is done. The phase
arc that started with "make microVMs work on Mac without abandoning
the Linux design" closes here with that promise kept — and with
Mac at full parity with Linux for everything that has a Linux
source-checkout equivalent today.

The remaining work is in two future arcs: camofox-on-Mac (browser
UX) and Mac release engineering (signed distribution). Both are
above the substrate layer and don't require any further
substrate changes.

**Sign-off granted on `sash/local-test`. Main untouched.**
