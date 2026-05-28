# Phase 10 — Sign-off

> **Outcome (2026-05-26):** the new Mac substrate is shippable to
> alpha-testers pending one of two preconditions: either (a) the
> external security reviewer signs off on the
> `SECURITY_REVIEW_PACKET.md` walkthrough and the two
> agent-surfaced unbounded-read findings (M1/M2) are remediated,
> or (b) the operator accepts those findings with rationale and
> ships under a documented "guest-host trust assumption" caveat.
> Phase 10 does not ship the alpha; it gates it.
>
> **Update 2026-05-26 (post-Phase-10.5):** M1, M2, M3, M4 are all
> **closed on this branch** in a follow-up 4-commit Phase 10.5
> session — see [`PHASE_10_5_SIGNOFF.md`](./PHASE_10_5_SIGNOFF.md).
> The "remediated" precondition above is now satisfied for those
> four findings without an external review round-trip. M5 remains
> Phase 11 (typed-dispatch fuzz expansion) and the 34 inherited
> workspace CVEs remain owned by the broader runtime team on a
> separate `chore/runtime-cve-hygiene` branch off `main`.

---

## 1. Scope reminder

Phase 9 signed off the **engineering** milestone — the Mac
source-checkout developer experience reached parity with Linux.
Phase 10 was scoped, on the day after Phase 9 closed, as 18-25
days of security hardening + release polish for the new
substrate.

**Day 1 re-scoped that down to 14 working days** after the CVE
audit revealed that all 34 vulnerabilities `cargo audit` reported
are inherited from `main` — none introduced by this branch. We
handed those off to the broader runtime team via
`RUNTIME_CVE_HANDOFF.md` (target: `chore/runtime-cve-hygiene` off
`main`, parallel to this work) and kept Phase 10's substance
focused on **what's new** here: the ~8,110 LOC of `elastos-vz/`
plus the trust-boundary additions in
`elastos-server/src/carrier_bridge.rs`.

The re-scope was an honesty call. Fixing other teams' CVEs in
this branch would have buried the Mac substrate's own security
work under a 50-commit dependency-bump diff that has nothing to
do with `Virtualization.framework`.

---

## 2. What landed, day by day

### Day 1 — CVE audit + ownership classification + handoff

`cargo audit` reported 34 vulnerabilities + 12 warnings across
the workspace. The agent classified each by **ownership** (this
branch vs. `main`) and severity. Result: **zero** vulnerabilities
were introduced by `sash/local-test`; all 34 reproduce on `main`
at the same crate versions.

Two artifacts: `PHASE_10_DAY_1_NOTES.md` (full per-CVE table,
ownership column, agent's recommendation per cluster) and
`RUNTIME_CVE_HANDOFF.md` (formal handoff packet for the broader
team, with five remediation clusters and a suggested branch
name).

Commits: `ca7476c` (audit), `74a811d` (re-scope based on
ownership analysis).

**Verifier:** `cd elastos && cargo audit` from anywhere in the
workspace tree reproduces the 34/12 numbers; `git log --oneline
main -- Cargo.lock` shows no Cargo.lock churn from this branch.

### Day 2-3 — Mac substrate threat model

`docs/vz-backend/MAC_THREAT_MODEL.md` documents eight trust
boundaries unique to the Mac substrate (Host operator ↔
supervisor; Supervisor ↔ Apple Vz; Guest VM ↔ Carrier bridge;
Capsule ↔ capsule; Operator ↔ runtime API; Upstream registry ↔
runtime; WASM guest ↔ wasmtime host; macOS kernel ↔ Vz
hypervisor). Per boundary: attacker, attempt, enforcement
mechanism with file:line code anchors, what would constitute a
break, known weaknesses + accepted risks, relevant inherited CVE
weakenings. Reviewer-focused; not academic.

Commit: `d8df96c`.

**Verifier:** every "enforcement mechanism" cell has a file:line
anchor the reviewer can open and verify against the current code.

### Day 4-8 — Carrier-bridge `cargo-fuzz` harness

Refactored `parse_carrier_line` out of the bridge I/O loop into a
pure function so it can be fuzz-driven without an async runtime
or a Unix socket. Built a `cargo-fuzz` harness under
`elastos/crates/elastos-server/fuzz/` (isolated workspace with a
copied `Cargo.lock` to avoid the Day-1b `pkcs8` resolution issue),
seeded a 22-entry hand-crafted corpus of known-good envelopes +
edge cases, plus a 66-line dictionary of JSON structural tokens
and `RuntimeRequest` types.

**5-minute fuzzing burst: 2,400,000 iterations, zero panics,
zero crashes, zero findings.**

Commit: `fee933d`.

**Verifier:** `cd elastos/crates/elastos-server/fuzz && cargo
+nightly fuzz run carrier_bridge_framing -- -max_total_time=300`
reproduces the burst.

### Day 9-10 — SIGINT/SIGTERM graceful shutdown + test-binary auto-resign

**Bug 1:** `elastos run <microvm>` on Mac responded to `Ctrl-C`
but not to `kill <pid>` / `kill -TERM <pid>` / `pkill elastos`
— SIGTERM was unhandled, so the operator had to escalate to
SIGKILL and the Apple Vz XPC (`com.apple.Virtualization.VirtualMachine`)
re-parented to `launchd` (PPID=1) and lingered as an orphan.

Fix: added a SIGTERM handler alongside `tokio::signal::ctrl_c()`,
wrapped `provider.stop()` with a 10-second `tokio::time::timeout`,
explicit `drop(provider)` so the `VzMachineHandle::Drop` teardown
runs while we can still observe it.

**Bug 2:** A fresh checkout's `cargo test -p elastos-vz` failed
with `signal: 9, SIGKILL: kill` because the test binaries
(`concurrent_launch-*`, `smoke-*`) ship without the Vz/JIT
entitlements. macOS kernel killed them on first Vz call.

Fix: extended `mac-local-setup.sh` to glob for test binaries
under `target/{debug,release}/deps/` and ad-hoc re-sign each one
with the same entitlements plist used for the main binary.

Verifier: `scripts/dev/test-sigint-graceful.sh` — sends SIGINT
then SIGTERM to a real microVM and asserts the Vz XPC
disappears within budget. Uses set-difference of pre/post Vz
PIDs to detect the XPC (since `ppid` is `launchd`, not `elastos run`).

Commits: `e48a691` (SIGINT/SIGTERM), `cee44d5` (test-binary
auto-resign), `a4675ef` (notes).

**Verifier:** `bash scripts/dev/test-sigint-graceful.sh`
reproduces the regression (passes today).

### Day 11-13 — External security review packet

`docs/vz-backend/SECURITY_REVIEW_PACKET.md` — a single document
(~511 lines) a hired external security engineer reads in ~30
minutes to know exactly what to look at, what's out of scope,
what the agent has already checked, and what the agent's own
honest list of concerns is.

Six sections: scope (8,110 src LOC); existing-artifact pointers;
10 reviewer focus points (falsifiable questions);
**internal pre-review pass** (5 mediums + 10 lows + 11
positive findings + 3 open questions the agent couldn't resolve);
"review complete" hand-back contract;
reviewer-contact protocol.

The internal pre-review pass is the most valuable section.
Highlights:

- **M1** — `BufReader::read_line` in `carrier_bridge.rs:225`
  is unbounded → guest→host memory DoS via no-newline floods.
  Agent recommends fix-now (~10 LOC).
- **M2** — same shape in `console_forwarder.rs:94`
  (kernel printk path).
- **M3** — JSON nesting-depth resilience needs empirical
  verification (default `serde_json` limit is 128 but agent did
  not confirm experimentally).
- **M4** — no upper bound on `mem_size_mib` / `vcpu_count`
  in manifest deserialization.
- **M5** — fuzz coverage does not extend to typed
  `RequestEnvelope` deserialize (Phase 11 defer).

Commit: `e52b059`.

**Verifier:** the doc itself; reviewer time-bounds confirm
30-min readability.

### Day 14 — GitHub Actions Mac release lane

`scripts/release/release-mac.sh` (412 LOC) + `.github/workflows/release-mac.yml`
(167 LOC). Fires on `release: types: [published]`, runs on
`macos-14`, produces a signed + smoke-checked
`elastos-<tag>-aarch64-apple-darwin.tar.gz` plus a `.sha256`
companion, uploads as workflow artifact + attaches to the
release. Notarisation is **explicitly NOT** automated — Apple's
notary credentials stay on the operator's machine.

Local dry-run on `HEAD` of `sash/local-test` with stub tag
`v0.0.0-test`: build 60s warm-cache, tarball 21 MB compressed,
all 4 entitlements survive tar/gzip round-trip.

`shellcheck` exit 0 (0 findings); `actionlint` exit 0 (0 errors).

Commit: `82d692b`.

**Verifier:** `bash scripts/release/release-mac.sh v0.0.0-test
--dry-run` reproduces; `shellcheck scripts/release/release-mac.sh
&& actionlint .github/workflows/release-mac.yml` clean.

### Day 15 — Phase 10 sign-off

This document.

---

## 3. Sign-off matrix

| Day | Deliverable | Shipped | Verified by author | Reviewed externally | Follow-ups documented |
|-----|-------------|:-------:|:------------------:|:-------------------:|:---------------------:|
| **D1** | CVE audit + ownership classification + handoff doc | ✓ | ✓ | n/a (handoff is the review) | ✓ |
| **D2-3** | Mac threat model | ✓ | ✓ | in-flight (part of D11-13 packet) | ✓ |
| **D4-8** | Carrier-bridge fuzz harness (2.4M-iter clean) | ✓ | ✓ | in-flight (part of D11-13 packet) | ✓ (M5: typed-dispatch fuzz) |
| **D9-10** | SIGINT/SIGTERM fix + test-binary auto-resign | ✓ | ✓ (`test-sigint-graceful.sh`) | in-flight (part of D11-13 packet) | ✓ |
| **D11-13** | External security review packet | ✓ | ✓ (pre-review pass) | **in-flight** (operator hands to reviewer) | ✓ (M1-M5) |
| **D14** | GitHub Actions Mac release lane | ✓ | ✓ (dry-run + shellcheck + actionlint) | n/a (operator infra) | ✓ (6 items deferred to first real release) |
| **D15** | This sign-off | ✓ | ✓ (this doc) | n/a | ✓ (next-phase planning) |

**One column matters most: "Reviewed externally."** Four cells
read "in-flight" because the external reviewer's window opened
when D11-13 landed and is still running. Phase 10's
substantive close is gated on that review returning. The next
section makes that gate explicit.

---

## 4. Known gaps that cross the phase boundary

Phase 10 surfaced more work than it closed. That is by design:
the threat model + the fuzz harness + the pre-review pass exist
precisely to **find** gaps, not to close them silently. Each
gap below carries: owner, current status, decision point, and
deferral target.

### Gap A — 34 inherited workspace CVEs

- **Owner:** broader runtime team.
- **Status:** formal handoff via `RUNTIME_CVE_HANDOFF.md` (5
  remediation clusters, suggested branch `chore/runtime-cve-hygiene`
  off `main`, verification steps for ownership).
- **Decision point:** does the alpha-tester ship gate on these
  being closed, or do alpha-testers get the Mac substrate with
  a documented dependency-vulnerability advisory and the CVE
  fixes follow?
- **Agent's recommendation:** ship the Mac substrate to
  alpha-testers; document the inherited CVEs in the
  release notes; track the cleanup branch separately. Coupling
  Mac-substrate release timing to runtime-team CVE remediation
  punishes both teams.

### Gap B — M1/M2 unbounded `read_line` in Carrier-bridge + kernel-console

- **Owner:** this branch — **CLOSED in Phase 10.5.**
- **Status:** **FIXED.** M1 in commit
  [`80ac011`](https://github.com/Elacity/elastos-runtime/commit/80ac011)
  (`phase10.5 M1: byte-budget carrier-bridge line reader`); M2 in
  [`42e11d4`](https://github.com/Elacity/elastos-runtime/commit/42e11d4)
  (`phase10.5 M2: byte-budget kernel-console line reader`).
  Bounded-read helpers (`read_line_byte_budgeted` async + sync
  flavours, `drain_to_newline` resync) replace the unbounded
  `BufReader::read_line` on both paths. Per-line allocation
  capped at 1 MiB+1 (Carrier-bridge) and 64 KiB+1
  (kernel-console). Two end-to-end regression tests prove the
  bound holds + the loop resyncs cleanly + dispatch resumes.
- **Operator verifiers (both pass on `sash/local-test` HEAD):**
  ```bash
  cargo test -p elastos-server --lib carrier_bridge::tests::oversized_line_resyncs_and_continues_dispatch -- --nocapture
  cargo test -p elastos-vz --lib ffi::console_forwarder::tests::forwarder_caps_oversized_kernel_line_and_resyncs -- --nocapture
  ```
- See `PHASE_10_5_SIGNOFF.md` § 2 for the full closeout.

### Gap C — M3 JSON-depth resilience verification

- **Owner:** this branch — **CLOSED in Phase 10.5.**
- **Status:** **VERIFIED (no code change required).** Commit
  [`4c83a23`](https://github.com/Elacity/elastos-runtime/commit/4c83a23)
  (`phase10.5 M3: verify JSON nesting-depth resilience`) added
  two regression tests + two new fuzz corpus seeds
  (`26-nested-129-deep`, `27-envelope-nested-200-deep`). Cargo
  test confirms a 200-deep nested array returns
  `Err(CarrierFrameError::InvalidJson(_))` (not stack overflow);
  60-second fuzz burst with the augmented corpus completed
  491,712 iterations clean. If `serde_json`'s default 128-deep
  recursion limit ever changes upstream, the regression test
  fires immediately.
- **Operator verifier:**
  ```bash
  cargo test -p elastos-server --lib carrier_bridge::tests::parse_carrier_line_rejects_excessively_nested_json -- --nocapture
  ```

### Gap D — M4 manifest resource caps

- **Owner:** this branch — **CLOSED in Phase 10.5** (was
  previously deferred to Phase 11; operator chose to close now
  alongside M1–M3).
- **Status:** **FIXED.** Commit
  [`45a1ec2`](https://github.com/Elacity/elastos-runtime/commit/45a1ec2)
  (`phase10.5 M4: cap manifest memory_mb / vcpu_count at config
  build`) adds `VmConfigLimits` (default 64 GiB / 32 vCPUs),
  `ConfigError::ResourceLimitExceeded`, and
  `VmConfig::from_manifest_with_limits(...)`. Production launch
  paths (`VzProvider::load`, `Supervisor::build_vm_config_for_mac`)
  wired to the fallible variant. `u32::MAX` MiB manifest now
  rejected at config-build time *before* Apple's
  `validateWithError` is asked to commit memory. Legacy
  infallible `from_manifest` retained as the unvalidated path
  for tests + future trusted-input call sites.
- **Operator verifier:**
  ```bash
  cargo test -p elastos-vz --lib config::tests::from_manifest_with_limits_rejects_excessive_memory -- --nocapture
  cargo test -p elastos-vz --lib config::tests::from_manifest_with_limits_rejects_excessive_vcpus -- --nocapture
  ```

### Gap E — M5 typed-`RequestEnvelope` fuzz expansion

- **Owner:** Phase 11.
- **Status:** the Day 4-8 harness covers `parse_carrier_line`
  (framing layer, permissive `serde_json::Value`). The typed
  `RequestEnvelope` deserialize + dispatch into provider code
  is a much larger surface and a multi-day fuzz effort.
  Explicitly deferred and documented in `PHASE_10_DAY_8_NOTES.md`.

### Gap F — Notarisation automation

- **Owner:** Phase 11 (or never, if operator prefers manual).
- **Status:** explicitly out of scope for Day 14.
  Notarisation credentials are not in CI secrets at this stage;
  operator runs `xcrun notarytool submit` + `xcrun stapler
  staple` manually. First real release will produce
  `PHASE_11_NOTARISATION_NOTES.md` documenting any friction.

### Gap G — Six Day-14 follow-ups deferred to first real release

Each catalogued in `PHASE_10_DAY_14_NOTES.md` § "What was NOT
tested locally": artifact-upload happy path,
softprops-release-attach happy path, macos-14 runner
availability, notarisation roundtrip, fetch-depth cost, tarball
bit-reproducibility. Each has a documented test plan; none gate
the alpha-tester ship.

---

## 5. Verifier replay

Commands a future reader runs to convince themselves Phase 10
is real. Each is copy-pastable from this doc.

### CVE audit reproduction (Gap A baseline)

```bash
cd elastos
cargo audit
# expect: 34 vulnerabilities + 12 warnings
# cross-check ownership:
git log --oneline main -- Cargo.lock
# expect: empty (this branch did not modify Cargo.lock)
```

### Carrier-bridge fuzz burst (D4-8)

```bash
cd elastos/crates/elastos-server/fuzz
cargo +nightly fuzz run carrier_bridge_framing -- -max_total_time=300
# expect: ~2-3 M iterations in 5 minutes, zero crashes
```

### SIGINT/SIGTERM regression (D9-10)

```bash
bash scripts/dev/test-sigint-graceful.sh
# expect: both signals → clean Vz XPC shutdown within ~10 s
# (requires a built `elastos` binary; the script builds if missing)
```

### Release-lane dry-run (D14)

```bash
bash scripts/release/release-mac.sh v0.0.0-test --dry-run
# expect: 8 sections, all pass, tarball + sha256 under
# elastos/target/release-mac/v0.0.0-test/
```

### Lint gates (D14)

```bash
shellcheck scripts/release/release-mac.sh
actionlint .github/workflows/release-mac.yml
# expect: exit 0 from each, zero findings
```

### Threat-model + review-packet readability check (D2-3, D11-13)

```bash
wc -l docs/vz-backend/MAC_THREAT_MODEL.md \
     docs/vz-backend/SECURITY_REVIEW_PACKET.md
# expect: ~500 + ~500 LOC respectively
# both readable in ~30 min each by a security engineer
```

---

## 6. Handoff contracts

Who owns what after Phase 10 closes. One sentence per owner;
no ambiguity.

- **Broader runtime team** owns `RUNTIME_CVE_HANDOFF.md` and
  the `chore/runtime-cve-hygiene` branch that closes Gap A
  on `main`.
- **External security reviewer** owns the
  `SECURITY_REVIEW_PACKET.md` walkthrough and produces
  `PHASE_10_DAY_13_NOTES.md` with their findings + dispositions.
- **This branch's agent** owns the Gap B (M1/M2) fix commits
  once reviewer concurrence lands, plus the Gap C (M3)
  fuzz-seed verification.
- **Operator** owns the manual notarisation step on every real
  release, and owns the GitHub Actions secrets policy for
  whether Phase 11 introduces notarytool-via-CI.
- **Phase 11 lead** owns Gaps D + E + F (manifest caps, typed
  fuzz, notarisation automation) + the Homebrew formula +
  auto-update mechanism scoping.

---

## 7. First-principles check

Did Phase 10 make the substrate genuinely safer to ship, or did
we just paint over the surface?

Five principles, each with one **yes-with-evidence** and one
**no-with-rationale**:

### Defence in depth

- **Yes** — the trust boundaries are written down
  (`MAC_THREAT_MODEL.md`), the most critical parser is
  fuzz-clean over 2.4M iterations, and the operator-facing
  shutdown path is bounded by an explicit timeout + drop. Three
  independent layers each fail closed.
- **No** — M1/M2 prove a guest-controlled input can grow host
  memory unboundedly. The post-read size check exists but is the
  wrong layer; the bounded-read fix has been recommended but not
  yet shipped. Until that lands, the "defence in depth" claim
  has one layer too few in the Carrier-bridge path.

### Fail closed

- **Yes** — `from_manifest` rejects missing rootfs (typed
  `CapsuleNotFound`); the bridged-network path rejects missing
  entitlement (typed `Compute` with operator hint); the
  deprecated late-binding APIs (`set_session_for_vm`,
  `set_network_for_vm`, `append_boot_args_for_vm`) all return
  typed migration errors instead of silently succeeding.
- **No** — M4 means a manifest can request 4 TiB of RAM and
  the substrate forwards it to Apple's validate rather than
  rejecting it locally. "Apple says no" is not the same as "the
  substrate says no", and the failure mode (Apple's
  validateWithError running expensive checks before failing)
  is worse than an early reject.

### Observable failure modes

- **Yes** — every exit path in `run_carrier_bridge_loop` fires
  the `on_terminate` notifier so the supervisor can deterministically
  await bridge teardown after `vm.stop`; `VzExitReason` classifies
  every terminal observation into one of four telemetry labels
  (`guest_clean_stop`, `host_initiated_stop`, `forced_after_timeout`,
  `stopped_with_error`); `release-mac.sh`'s six distinct exit codes
  let CI failure mode be triaged from the exit code alone.
- **No** — Apple's framework errors come back as `NSError`
  with `localizedDescription` strings. The substrate
  classifies via `VzError::from_ns_error_parts`, but the
  classifier is convention-based (matches on error string
  fragments); a future Apple message-text drift can silently
  re-route a known error class into `VzError::Internal`. Pre-review
  L1 documented this.

### Operator agency

- **Yes** — the operator can run `release-mac.sh` end-to-end
  on their own Mac without GitHub Actions, can re-run a release
  build via `workflow_dispatch` for an existing tag without
  re-publishing, owns the notarisation step explicitly, can
  inspect the threat model + review packet + Day notes before
  cutting any release.
- **No** — there's no operator-visible kill switch for a
  rogue capsule on Mac other than `pkill elastos`. The fixed
  SIGINT/SIGTERM path now correctly tears the VM down, but a
  capsule that refuses to honour `init 0` from inside still
  takes ~10 s of the `stop_timeout` budget. The Linux
  SIGTERM→SIGKILL 5-second escalation has no direct Vz
  equivalent (no `kill -9` for a `VZVirtualMachine`); the
  `ForcedAfterTimeout` exit reason and the explicit `drop(provider)`
  are the closest analogue.

### Blast radius

- **Yes** — Vz VMs are NAT'd by default with no inter-VM
  routing; the Carrier bridge is the only host-side surface a
  capsule sees, and that surface has a bounded line limit + a
  fuzz harness behind it. A compromised capsule's reach is
  bounded to its own VM + whatever the Carrier provider
  capabilities let through; the threat model § "Capsule ↔
  capsule" boundary documents the enforcement.
- **No** — the ad-hoc-signed binary the dev workflow uses
  grants the four entitlements (virtualization, allow-jit,
  allow-unsigned-executable-memory,
  disable-executable-page-protection) to every Mac user
  account that runs the binary. A user-account compromise on
  the Mac inherits those entitlements; the substrate has no
  per-invocation entitlement boundary. This is a known macOS
  limitation, not a Phase 10 regression, but it caps the
  "blast radius" claim.

---

## Closing

Phase 10 produced six artifacts of substance (CVE handoff,
threat model, fuzz harness, demo-bug fixes + regression, review
packet, release lane) and one piece of administrative work
(this sign-off). The substrate is materially safer than it was
at Phase 9 close, but the work is honest about what it didn't
close — M1/M2/M3 are surfaced and ready for the reviewer; M4/M5
are documented Phase 11 work; Gap A is owned by the broader
team.

The next decision point is the alpha-tester ship: does it happen
on this branch as-is (with M1/M2 deferred to a fast follow-up
post-review), or does it wait for M1/M2/M3 to land first? The
agent recommends the former for a known-audience alpha and the
latter for a public alpha. Either is defensible; both are
better than shipping in ignorance of the gaps Phase 10 found.

End of Phase 10.
