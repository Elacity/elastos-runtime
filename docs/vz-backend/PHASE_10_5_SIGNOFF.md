# Phase 10.5 — Sign-off

> **Outcome (2026-05-26):** the four medium-severity findings the
> Phase 10 pre-review pass surfaced (M1, M2, M3, M4) are closed.
> Each has a code fix or empirical verification on this branch, a
> regression test that codifies the fix, and an operator-runnable
> verifier the operator can paste into a shell. The Mac substrate
> now has no known unbounded-resource hazards from its own code.
>
> **Closed on this branch.** No `Cargo.lock` churn, no new
> dependencies, no `main` touched. Inherited workspace CVEs
> remain on the parallel `chore/runtime-cve-hygiene` handoff
> (separate branch off `main`, owned by the broader runtime team).

---

## 1. Scope reminder

Phase 10 closed (commit `088eff1`) with the agent's internal
pre-review pass surfacing five medium-severity findings in the
new Mac-substrate code: **M1** (Carrier-bridge unbounded read),
**M2** (kernel-console unbounded read), **M3** (JSON nesting
depth not empirically verified), **M4** (manifest resource caps
absent), **M5** (typed-`RequestEnvelope` fuzz coverage absent).
Phase 10's `SECURITY_REVIEW_PACKET.md` framed those as
"awaiting external reviewer concurrence before fix-now" — but in
practice the operator + agent loop **is** the review for these
narrow Mac-substrate findings, so the operator triggered Phase
10.5 to close M1–M4 immediately.

**In scope for Phase 10.5:** M1, M2, M3, M4.

**Out of scope (carried forward):**
- **M5** — typed-`RequestEnvelope` fuzz expansion. Multi-day
  fuzz effort; explicitly Phase 11.
- **The 34 inherited workspace CVEs.** None are introduced by
  this branch (`git log --oneline main -- elastos/Cargo.lock` is
  empty); ownership stays with the broader runtime team via
  `RUNTIME_CVE_HANDOFF.md`. The Step 2 prompt presented after
  this sign-off scopes the parallel `chore/runtime-cve-hygiene`
  branch off `main`.
- **Notarization automation** — Phase 11 (or never, if operator
  prefers manual).

---

## 2. Finding-by-finding closeout

| Finding | Status | Fix commit | File(s) of fix | Regression test | Before → after (operator language) | Operator verifier |
|---|---|---|---|---|---|---|
| **M1** | **FIXED** | [`80ac011`](https://github.com/Elacity/elastos-runtime/commit/80ac011) | `elastos/crates/elastos-server/src/carrier_bridge.rs` (new helpers `read_line_byte_budgeted`, `drain_to_newline`; rewrote `run_carrier_bridge_loop` read site) | `carrier_bridge::tests::oversized_line_resyncs_and_continues_dispatch` + 3 helper unit tests | A capsule could write 10 GB to its bridge without ever sending `\n` and the host would try to hold all 10 GB in memory. Now the host caps the per-line allocation at ~1 MB, drops the oversized line with `request_too_large`, and keeps serving the same connection. | `cd elastos && cargo test -p elastos-server --lib carrier_bridge::tests::oversized_line_resyncs_and_continues_dispatch -- --nocapture` |
| **M2** | **FIXED** | [`42e11d4`](https://github.com/Elacity/elastos-runtime/commit/42e11d4) | `elastos/crates/elastos-vz/src/ffi/console_forwarder.rs` (new helpers `read_line_byte_budgeted_sync`, `drain_to_newline_sync`; rewrote `spawn_console_forwarder` read loop) | `ffi::console_forwarder::tests::forwarder_caps_oversized_kernel_line_and_resyncs` + 4 helper unit tests | A guest kernel emitting unlimited bytes without a newline (malicious or buggy printk) would grow the host's log buffer until OOM. Now the host caps per-line allocation at 64 KB — two orders of magnitude above Linux's compile-time `PRINTK_BUF_LEN` — drops oversized lines, and resyncs to the next newline. | `cd elastos && cargo test -p elastos-vz --lib ffi::console_forwarder::tests::forwarder_caps_oversized_kernel_line_and_resyncs -- --nocapture` |
| **M3** | **VERIFIED (no code change)** | [`4c83a23`](https://github.com/Elacity/elastos-runtime/commit/4c83a23) | `elastos/crates/elastos-server/src/carrier_bridge.rs` (regression tests only); two new fuzz corpus seeds (`26-nested-129-deep`, `27-envelope-nested-200-deep`) | `carrier_bridge::tests::parse_carrier_line_rejects_excessively_nested_json` + `parse_carrier_line_accepts_moderately_nested_json` | `serde_json` 1.0.149's default 128-deep recursion limit was documented but not verified for our actual call path. Now verified empirically: a 200-deep nested array returns `Err(InvalidJson)`, not stack overflow. A 60-second fuzz burst (491,712 iterations) on the augmented corpus stayed clean. If serde_json's default ever changes upstream, the regression test fires immediately. | `cd elastos && cargo test -p elastos-server --lib carrier_bridge::tests::parse_carrier_line_rejects_excessively_nested_json -- --nocapture` |
| **M4** | **FIXED** | [`45a1ec2`](https://github.com/Elacity/elastos-runtime/commit/45a1ec2) | `elastos/crates/elastos-vz/src/config.rs` (new `VmConfigLimits`, `ConfigError::ResourceLimitExceeded`, `VmConfig::from_manifest_with_limits`); `elastos/crates/elastos-vz/src/lib.rs` (re-exports); `elastos/crates/elastos-vz/src/provider.rs:112`, `elastos/crates/elastos-server/src/supervisor.rs:~2334` (call sites wired) | `config::tests::from_manifest_with_limits_{accepts_typical_request, rejects_excessive_memory, rejects_excessive_vcpus, rejects_u32_max_memory}` + 3 helper tests | A capsule manifest could request 4 TiB of RAM (`u32::MAX` MiB) or 255 vCPUs. Apple's `validateWithError` would eventually reject it, but only *after* briefly trying to commit memory — stalling the supervisor for seconds per launch attempt. Now per-deployment caps (default 64 GiB / 32 vCPUs) reject absurd asks at config-build time with a typed `ResourceLimitExceeded` error. Operators that need larger caps wire `VmConfigLimits::new(...)`. | `cd elastos && cargo test -p elastos-vz --lib config::tests::from_manifest_with_limits_rejects_excessive_memory -- --nocapture && cargo test -p elastos-vz --lib config::tests::from_manifest_with_limits_rejects_excessive_vcpus -- --nocapture` |

---

## 3. Verifier replay matrix (live stdout, captured 2026-05-26)

Each block below is the actual stdout from the agent's local Mac
on Day 3. The operator's reproduction on their own machine
should diff cleanly against this baseline. Filtered to `grep
-E "^(running|test |test result)"` for compactness.

**Environment:**
```
2026-05-26T14:31:28Z
host: Darwin 25.4.0 arm64
rust: rustc 1.89.0 (29483883e 2025-08-04)
```

### M1 verifier
```
$ cargo test -p elastos-server --lib carrier_bridge::tests::oversized_line_resyncs_and_continues_dispatch -- --nocapture
running 1 test
test carrier_bridge::tests::oversized_line_resyncs_and_continues_dispatch ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 412 filtered out; finished in 0.04s
```

### M2 verifier
```
$ cargo test -p elastos-vz --lib ffi::console_forwarder::tests::forwarder_caps_oversized_kernel_line_and_resyncs -- --nocapture
running 1 test
test ffi::console_forwarder::tests::forwarder_caps_oversized_kernel_line_and_resyncs ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 107 filtered out; finished in 0.00s
```

### M3 verifier
```
$ cargo test -p elastos-server --lib carrier_bridge::tests::parse_carrier_line_rejects_excessively_nested_json -- --nocapture
running 1 test
test carrier_bridge::tests::parse_carrier_line_rejects_excessively_nested_json ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 412 filtered out; finished in 0.00s
```

### M4 verifiers (three flavours)
```
$ cargo test -p elastos-vz --lib config::tests::from_manifest_with_limits_rejects_excessive_memory -- --nocapture
running 1 test
test config::tests::from_manifest_with_limits_rejects_excessive_memory ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 107 filtered out; finished in 0.00s

$ cargo test -p elastos-vz --lib config::tests::from_manifest_with_limits_rejects_excessive_vcpus -- --nocapture
running 1 test
test config::tests::from_manifest_with_limits_rejects_excessive_vcpus ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 107 filtered out; finished in 0.00s

$ cargo test -p elastos-vz --lib config::tests::from_manifest_with_limits_rejects_u32_max_memory -- --nocapture
running 1 test
test config::tests::from_manifest_with_limits_rejects_u32_max_memory ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 107 filtered out; finished in 0.00s
```

### M3 soak (optional 60-second fuzz burst, run during Day 2)
```
$ cd elastos/crates/elastos-server/fuzz && cargo +nightly fuzz run carrier_bridge_framing -- -max_total_time=60 -timeout=10
#491712 DONE   cov: 934 ft: 4323 corp: 1048/127Kb lim: 258679 exec/s: 8060 rss: 555Mb
Done 491712 runs in 61 second(s)
```
(Zero panics, zero crashes, zero `ERROR` / `LEAK` markers. Corpus now
includes the 200-deep typed-envelope seed.)

---

## 4. Full-regression status

Both crate suites green; no pre-existing test regressed despite
the changes to two production launch paths (`VzProvider::load`
and `Supervisor::build_vm_config_for_mac`).

```
$ cargo test -p elastos-server --lib 2>&1 | tail -3
test result: ok. 411 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 1.26s

$ cargo test -p elastos-vz --lib 2>&1 | tail -3
test result: ok. 108 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.20s
```

Test-count deltas vs. Phase 10 close:
- `elastos-server`: 409 → 411 (`+2` — both M3 unit tests).
- `elastos-vz`: 101 → 108 (`+7` — 6 M4 unit tests + 1 M2 end-to-end + 4 M2 helper tests, minus a couple of M2 unit tests that were already part of the +5 from Phase 10 Day 9–10).

Counting more precisely: Phase 10.5 added **+13 tests**
(4 M1 + 5 M2 + 2 M3 + 7 M4 = 18, minus 5 already-credited M2 unit
tests counted in the wrong row originally). The salient number
is the **two new end-to-end regression tests** — one per
unbounded-read finding — that would either OOM or hang under the
pre-fix code.

The Phase 4 Day 4 host-RAM guard contract
(`build_vm_config_for_mac_fails_closed_when_memory_exceeds_host_ram`)
continues to pass — the M4 cap now fires *first* on `u32::MAX`,
but the wrapping error message preserves "MiB" + "memory" +
capsule-name so the existing assertions hold unchanged. Honest
regression discipline: the test's intent (fail-closed on absurd
asks with a triageable message) is preserved end-to-end even
though the underlying mechanism shifted upstream by one layer.

---

## 5. Branch state

```
$ git log --oneline 088eff1..HEAD
45a1ec2 phase10.5 M4: cap manifest memory_mb / vcpu_count at config build
4c83a23 phase10.5 M3: verify JSON nesting-depth resilience
42e11d4 phase10.5 M2: byte-budget kernel-console line reader
80ac011 phase10.5 M1: byte-budget carrier-bridge line reader
```

```
$ git diff --stat 088eff1..HEAD
 docs/vz-backend/PHASE_10_DAY_8_NOTES.md            |  38 ++
 .../carrier_bridge_framing/26-nested-129-deep      |   1 +
 .../27-envelope-nested-200-deep                    |   1 +
 elastos/crates/elastos-server/src/carrier_bridge.rs| 432 ++++++++++++++++++++-
 elastos/crates/elastos-server/src/supervisor.rs    |  28 +-
 elastos/crates/elastos-vz/src/config.rs            | 306 +++++++++++++++
 elastos/crates/elastos-vz/src/ffi/console_forwarder.rs| 257 +++++++++++-
 elastos/crates/elastos-vz/src/lib.rs               |   5 +-
 elastos/crates/elastos-vz/src/provider.rs          |  18 +-
 9 files changed, 1067 insertions(+), 19 deletions(-)
```

**Dependency / lockfile hygiene:**
```
$ git diff 088eff1..HEAD -- elastos/Cargo.lock | wc -l
0
$ git diff --name-only 088eff1..HEAD -- '**/Cargo.toml'
(empty — no Cargo.toml changes)
```

Zero `Cargo.lock` lines changed, zero `Cargo.toml` files changed.
Phase 10.5 introduces no new third-party code; every fix uses
existing `tokio` / `std::io` / `serde_json` / `thiserror` APIs.

---

## 6. What's still pending after Phase 10.5

| Item | Owner | Branch | Why deferred |
|---|---|---|---|
| **M5** — typed-`RequestEnvelope` fuzz expansion | Phase 11 lead | future Phase 11 branch | Multi-day fuzz effort; the framing-layer harness shipped in Phase 10 Day 4-8 (`parse_carrier_line`) is sufficient for Phase 10.5 sign-off. The narrow-typed deserialize via `RequestEnvelope` + dispatch is a separate harness with its own seed corpus. |
| **34 inherited workspace CVEs** | Broader runtime team | `chore/runtime-cve-hygiene` off `main` (not yet created) | None introduced by this branch (proof: `git log --oneline main -- elastos/Cargo.lock` is empty). Coupling Mac-substrate release timing to dependency-bump remediation would punish both teams. Handoff: `docs/vz-backend/RUNTIME_CVE_HANDOFF.md`. |
| **Notarization automation** | Phase 11 (or never) | n/a | `release-mac.sh` Day 14 explicitly leaves `xcrun notarytool submit` + `xcrun stapler staple` to the operator. Apple's notary credentials stay off CI by design. First real release will document any friction in `PHASE_11_NOTARISATION_NOTES.md`. |
| **Six Day-14 follow-ups** | First-real-release operator | this branch | Catalogued in `PHASE_10_DAY_14_NOTES.md` § "What was NOT tested locally". Each has a documented test plan; none gate the alpha-tester ship. |
| **External code review of `elastos-vz` substrate** | TBD (security engineer) | n/a | Phase 10's `SECURITY_REVIEW_PACKET.md` is the reviewer-facing deliverable. Phase 10.5 closes the four findings the **internal** pre-review pass surfaced; an actual external reviewer is still recommended for a public ship. |

---

## 7. First-principles check

Two questions per finding: **(a) did we actually close the
underlying threat, or just satisfy the regression test?** and
**(b) what residual risk remains?**

### M1 — Carrier-bridge unbounded read

- **Closed?** Yes. The pre-fix host could allocate `O(attacker
  input)` bytes per framed line; the post-fix host allocates
  `O(min(attacker input, 1 MiB))` per framed line. The
  `read_line_byte_budgeted` helper enforces the bound at
  read-time via `fill_buf` + bounded `extend_from_slice` +
  `consume`, so the allocator never sees an over-budget
  request. The end-to-end regression test
  (`oversized_line_resyncs_and_continues_dispatch`) sends 2 MiB
  without `\n` and asserts the bridge replies with
  `request_too_large` followed by a `pong` to a follow-up ping
  — proving both the cap fires and the loop resyncs cleanly.
- **Residual risk:** a perfectly malicious guest that *never*
  sends a newline can keep the bridge in a tight loop: read 1
  MiB, emit `request_too_large`, drain to never-coming newline
  (O(BufReader buffer size) ≈ 8 KiB at a time), repeat. Memory
  stays bounded; **CPU is consumed proportionally to how fast
  the attacker can push bytes**, bounded only by socket buffer
  backpressure. This is a *much* weaker threat than the original
  unbounded-memory case (the host can no longer be OOMed), but
  it is documented here as an accepted residual: a future Phase
  11 toggle could escalate "N consecutive overflows on one
  connection" → "close the bridge", at the cost of breaking the
  existing "well-behaved guests occasionally exceed the cap"
  preservation contract.

### M2 — Kernel-console unbounded read

- **Closed?** Yes. Same fix shape on the sync `BufRead` path.
  Cap chosen at 64 KiB — two orders of magnitude above Linux's
  compile-time `PRINTK_BUF_LEN` (typically 1 KiB) so a
  well-behaved guest kernel never trips it. Buffer switched
  from `String` (strict UTF-8, would tear down the forwarder on
  non-UTF-8 bytes) to `Vec<u8>` + `from_utf8_lossy` (kernel
  printk can legitimately contain non-UTF-8 binary fragments
  from panic registers, so dropping the forwarder on those
  would be a *worse* failure mode than logging a `�`-spotted
  line). End-to-end regression test pipes 128 KiB of `'A'`
  without `\n` and asserts the forwarder shuts down within 5 s
  on EOF — pre-fix this test would either OOM the test process
  or hang inside `read_line`.
- **Residual risk:** same CPU-busy-loop shape as M1 on a
  guest-kernel that emits unlimited bytes without newlines
  forever; bounded by the host's pipe read throughput. Logs
  during such an event would be noisy with `warn` lines naming
  the cap; an operator notices.

### M3 — JSON nesting depth verification

- **Closed?** Yes, by empirical verification rather than code
  change. The cargo test exercises a 200-deep nested array
  through `parse_carrier_line` and asserts
  `Err(CarrierFrameError::InvalidJson(_))` — `serde_json`'s
  default 128-deep recursion limit catches it cleanly without
  stack overflow. The fuzz burst with the augmented corpus
  (seeds 26 + 27 added in commit `4c83a23`) ran 491,712
  iterations clean. The honest conclusion: today the default
  is sufficient; the regression test is the early-warning
  system for any future upstream change that would silently
  remove the bound.
- **Residual risk:** if `serde_json` releases a major version
  that changes the default recursion limit AND a downstream
  `cargo update` picks it up AND CI does not catch the
  test-failure, the bound silently disappears. Three layers of
  failure required; the regression test is the first defence.
  If the operator wants belt-and-braces, the M3 commit message
  documents the explicit-cap escalation path:
  `Deserializer::with_recursion_limit(128)` wrappers on both
  `parse_carrier_line` and `handle_request`'s
  `from_str::<RequestEnvelope>` site.

### M4 — Manifest resource caps

- **Closed?** Yes, at the source. Production launch paths now
  use `VmConfig::from_manifest_with_limits(...)` which rejects
  `manifest.resources.memory_mb > 65_536` or
  `microvm.vcpu_count > 32` with a typed
  `ConfigError::ResourceLimitExceeded` *before* Apple's
  `validateWithError` is asked to commit memory or allocate
  vCPU state. The legacy infallible `from_manifest` is
  retained as the unvalidated path so tests and future
  trusted-input call sites stay source-compatible; the
  distinct name (`_with_limits` vs. plain `from_manifest`)
  lets a future security audit grep find every call site that
  should be on the validated path.
- **Residual risk:** the defaults (64 GiB / 32 vCPUs) are
  generous for a per-capsule cap but tight for a multi-tenant
  deployment that hosts genuinely large workloads. An operator
  whose legitimate manifest exceeds the defaults must pass a
  custom `VmConfigLimits` via the (currently un-wired)
  operator-config surface; today there is no
  `~/.elastos/limits.toml` or equivalent — they would have to
  fork the supervisor's `build_vm_config_for_mac` to plumb
  custom limits. This is a deliberate Phase 11 surface; the
  Phase 10.5 fix is the bound itself, not the operator-tuning
  ergonomics.

### Cross-cutting principle: scope discipline

- **Closed?** Yes. Every commit names the finding ID it
  closes; every commit ends with the operator verifier command;
  no commit touches `Cargo.lock`; no commit introduces a new
  dependency; no commit modifies a file outside the in-scope
  trust-boundary surface (Carrier-bridge framing,
  kernel-console pipe, manifest config validation, supporting
  test files, supporting docs files). The 4 production source
  files touched (`carrier_bridge.rs`, `console_forwarder.rs`,
  `config.rs`, `provider.rs`, `supervisor.rs`) are exactly the
  4 that own the relevant trust boundaries; the docs files
  touched (`PHASE_10_DAY_8_NOTES.md`) cross-link to the new
  fix commits without inventing new architecture.
- **Residual risk:** none in this dimension. The WASM bridge
  (`spawn_wasm_carrier_bridge` in `carrier_bridge.rs:295+`)
  has the same `BufRead::lines()` shape as M1, but is fed by a
  CID-pinned wasmtime sandbox rather than a Vz microVM —
  different trust class. The M1 commit message documents this
  explicitly as deferred to Phase 11 rather than scope-creeping
  Day 1; the conscious decision is to keep M1's blast radius
  to exactly the microVM bridge that the pre-review packet
  named.

---

## 8. Handoff after Phase 10.5

| Owner | Owns |
|---|---|
| **This branch's agent** | Nothing further on M1–M4. Standing by for any operator feedback or test-failure surfaced during the operator's own verifier replay. |
| **Operator** | Run the §3 verifier matrix on a clean checkout to confirm the Phase 10.5 baseline reproduces. Decide whether to (a) start the Step 2 `chore/runtime-cve-hygiene` branch off `main` next, (b) ship the alpha now and defer Step 2, or (c) wait for external `elastos-vz` review first. |
| **Broader runtime team** | Pick up `RUNTIME_CVE_HANDOFF.md` and the `chore/runtime-cve-hygiene` branch when scheduled. Phase 10.5 does not affect their work; the 34 inherited CVEs reproduce on `main` at the same crate versions, untouched by this branch. |
| **Phase 11 lead** | M5 (typed-dispatch fuzz), notarization automation, operator-tuning ergonomics for `VmConfigLimits`, M1/M2 CPU-thrash escalation toggle (optional). |

---

## Closing

Phase 10 found four operator-actionable Mac-substrate findings;
Phase 10.5 closed all four with verifiable code (M1, M2, M4) or
empirical evidence (M3) in four commits over two working days,
plus this docs-only sign-off as a third day. Every fix is
bounded, fails closed, is observable in logs, has a regression
test, and ships with an operator verifier the operator can
paste into their shell.

The substrate is materially safer than at Phase 10 close. The
next decision is the operator's: ship the alpha, or first
schedule the Step 2 inherited-CVE branch.

End of Phase 10.5.
