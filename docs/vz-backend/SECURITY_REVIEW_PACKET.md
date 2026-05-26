# Security Review Packet — elastos-vz substrate + Carrier-bridge framing

> **Audience:** external security engineer hired/asked to review this branch.
> **Branch:** `sash/local-test` (head as of 2026-05-26).
> **Time budget:** ~30 minutes of careful reading; ~1-2 days of deeper poking.
> **Predecessor docs:** `PHASE_9_SIGNOFF.md`, `MAC_THREAT_MODEL.md`, `PHASE_10_DAY_8_NOTES.md`, `PHASE_10_DAY_9_NOTES.md`, `RUNTIME_CVE_HANDOFF.md`, `BRANCH_SUMMARY.md`.

This is the **Phase 10 Day 11-13 packet**. The agent has done a pre-review
pass (section 4); the external reviewer's job is to falsify the
agent's claims and surface anything the pass missed.

---

## 1. Review scope

### In scope (the new substrate code on this branch)

| Path | LOC (src) | LOC (tests) | Why it's the trust-boundary code |
|------|----------:|------------:|----------------------------------|
| `elastos/crates/elastos-vz/src/**/*.rs` | 6,784 | (separate file below) | The entire Apple Vz wrapper. Every `unsafe` block in this branch lives here. |
| `elastos/crates/elastos-vz/tests/concurrent_launch.rs` |  | 635 | Multi-VM lifecycle regression. |
| `elastos/crates/elastos-vz/tests/smoke.rs` |  | 255 | Single-VM end-to-end smoke. |
| `elastos/crates/elastos-server/src/carrier_bridge.rs` | 1,326 (567 new) |  | Host-guest framing parser + I/O loop. The trust boundary we co-own with the rest of the runtime. |
| **Total** | **~8,110 src** | **~890 tests** | |

`git diff main...sash/local-test` against the in-scope paths:

```
27 files changed, 8230 insertions(+), 61 deletions(-)
```

Use this to bound the review:

```bash
git diff main...sash/local-test \
  -- elastos/crates/elastos-vz/ \
     elastos/crates/elastos-server/src/carrier_bridge.rs
```

### Out of scope (please do not review on this branch)

- **All 34 inherited workspace CVEs** — `cargo audit` findings against
  the full workspace. Every vulnerable crate is on `main` at the same
  version (verified in `PHASE_10_DAY_1_NOTES.md`); fixes belong on
  `chore/runtime-cve-hygiene` off `main`, not coupled into this Mac
  substrate branch. Hand-off doc for that work is
  `RUNTIME_CVE_HANDOFF.md`.
- Every workspace crate not under `elastos-vz/` or
  `carrier_bridge.rs`. The rest of the runtime is on `main` unchanged.
- The Mac bootstrap script `scripts/dev/mac-local-setup.sh`. Operator
  tooling, not substrate code.
- The release CI lane — Day 14 work, not landed yet.

---

## 2. Existing artifacts (please read before touching code)

These exist on this branch. Reading them first will save the
reviewer a meaningful fraction of the time budget.

| Document | One-line summary |
|----------|------------------|
| `docs/vz-backend/MAC_THREAT_MODEL.md` | Canonical written threat model — eight trust boundaries, enforcement mechanism per boundary, what would constitute a break, known weaknesses. |
| `docs/vz-backend/PHASE_10_DAY_1_NOTES.md` | CVE audit. 34 vulnerabilities found; **all 34 inherited from `main`**, zero introduced by this branch. |
| `docs/vz-backend/RUNTIME_CVE_HANDOFF.md` | Hand-off packet for the broader team to fix those 34 inherited CVEs on a parallel branch. |
| `docs/vz-backend/PHASE_10_DAY_8_NOTES.md` | Carrier-bridge `cargo-fuzz` harness. 2.4 M iterations of `parse_carrier_line` in a 5-minute burst → zero panics, zero crashes. |
| `docs/vz-backend/PHASE_10_DAY_9_NOTES.md` | Two demo-bug fixes landed this week: SIGINT/SIGTERM graceful shutdown, test-binary auto-resign. Regression script included. |
| `docs/vz-backend/BRANCH_SUMMARY.md` | Team-facing branch summary. 9th-grade explainer + engineer deep-dive. |
| `docs/vz-backend/PHASE_9_SIGNOFF.md` | Engineering milestone close — three-layer architectural audit + the 5-step smoke matrix that was green at sign-off. |

The reviewer should not have to discover any of these.

---

## 3. Reviewer focus points (≤ 10 high-leverage falsifiable questions)

These are the questions the agent most wants the reviewer to attempt
to falsify. Each one is worded so that a positive answer (a real
finding) is concrete enough to fix.

1. **Can a guest VM cause unbounded host-side memory growth by writing
   bytes without a newline on `/dev/hvc1`?** (See §4 / C21. Agent
   strongly suspects yes; remediation is bounded-`read_until`.)

2. **Can a guest VM craft a JSON document inside
   `CARRIER_MAX_LINE_BYTES` that triggers a stack overflow in
   `serde_json::from_str` via deeply-nested arrays/objects?** (See
   §4 / C23. `serde_json` has no nesting limit by default.)

3. **Does any `VZMachineHandle` or `VzProvider` method panic on an
   Apple framework error instead of returning a typed `VzError`?**
   (Agent surveyed for `unwrap` / `expect` on FFI-return paths;
   found none on the hot path. Reviewer to confirm independently.)

4. **Does the SIGINT/SIGTERM shutdown path land in `drop(provider)`
   under every observable code path, including a `panic!` in
   `provider.stop()`?** (Day 9 regression confirms the
   happy-path. Reviewer to confirm the panic-path.)

5. **Does `vm.vz_boot_args()` (the kernel command line handed to
   Apple's `VZLinuxBootLoader.setCommandLine`) ever embed a string
   that includes a guest-controlled or operator-untrusted byte?**
   (See §4 / C7, C11. Agent's reading is "operator-curated only",
   but the chain — capsule.json → `MicroVmConfig.boot_args` →
   `with_session(token, api_addr)` — deserves a second pass.)

6. **Is the entitlements plist
   (`scripts/dev/sign-elastos-vz/vz.entitlements.plist`) minimal —
   does it grant any privilege beyond what Vz + wasmtime JIT
   actually need?** Specifically:
   `com.apple.security.cs.disable-executable-page-protection` is the
   most expensive of the four; can wasmtime work without it on
   Apple Silicon?

7. **Is the Carrier socket-pair fd-handoff dance
   (`build_carrier_console_slot` in `ffi/console.rs` lines 225-293)
   double-close-safe under every error path?** The agent traced the
   four early-return paths; reviewer to confirm.

8. **Does `take_running_vm` followed by a concurrent `stop` /
   `status` / `info` produce any race window where Apple receives a
   call after the handle has been moved?** (See §4 / C19. Agent's
   reading is "no — the `RwLock<HashMap>` serialises", but the
   external mover then owns the handle while in-flight requests may
   still be queued.)

9. **Does `default_data_dir`'s `getpwuid` fallback have any
   path-traversal hazard when `vm_id` (UUIDv4) joins on top of the
   resolved home?** (See §4 / C26. UUIDv4 contains only hex + `-`,
   so the answer should be "no", but the substrate doesn't enforce
   the format.)

10. **Are `unsafe impl Send` and `unsafe impl Sync` correctly
    discharged on `SendableVm` and `SendableDelegate`?** Apple's
    threading contract is "all VM touches on the associated
    dispatch queue" — the wrappers move `Retained<>` across
    threads via `Arc` to let Tokio land closures onto the queue.
    Reviewer to confirm there is no path that derefs the inner
    `Retained<>` off-queue.

---

## 4. Internal pre-review pass

Performed 2026-05-26 by the agent that wrote the code. **Severity rubric:**

- **High** — concrete exploitable defect; ship-blocker unless explicitly accepted.
- **Medium** — defence-in-depth gap or unbounded resource consumption from a
  privileged source (guest VM, operator, manifest). Should be fixed before
  alpha unless the operator accepts the risk with rationale.
- **Low** — code-hygiene concern, refactor hazard, or theoretical-only issue
  with a clear mitigation already in place upstream.

**Notation:** `file.rs:NN` anchors to the exact line in the in-scope diff.
"Agent recommendation" is the action the agent thinks should happen; the
reviewer is welcome to override.

### Highs

**None found during this pass.** The reviewer is encouraged to falsify
this — the most likely sources for a "high" finding are #1 and #2 in
section 3 (unbounded reads, JSON-depth stack overflow).

### Mediums

#### M1 — Unbounded `BufReader::read_line` in Carrier-bridge loop
- **File:line:** `elastos/crates/elastos-server/src/carrier_bridge.rs:225`
- **Observed:** `reader.read_line(&mut line).await` has no upper bound. A
  guest writing `b"A" * 10_000_000_000` without a `\n` would grow `line`
  unboundedly. The `CARRIER_MAX_LINE_BYTES` check at line 234 is **after**
  the read returns — by that point the allocation has already happened.
- **Why it matters:** classic guest→host denial-of-service. A capsule
  with no special privilege could exhaust host memory.
- **Defence in place:** the post-read check correctly drops the line and
  writes back `request_too_large`. But the host-side allocation is the
  hazard — the check is too late.
- **Agent recommendation:** **fix-now.** Replace `read_line` with
  `take(CARRIER_MAX_LINE_BYTES as u64 + 1).read_until(b'\n', &mut buf)`
  so the read itself is bounded. The "+1" lets the post-read check
  observe an oversized line and refuse it without truncating the
  attacker's payload mid-byte.
- **Scope of fix:** ~10 LOC change; bridge tests already in place; the
  Day 4-8 fuzz harness's seed corpus already covers the
  not-newline-terminated cases.

#### M2 — Same unbounded read in kernel-console forwarder
- **File:line:** `elastos/crates/elastos-vz/src/ffi/console_forwarder.rs:94`
- **Observed:** `reader.read_line(&mut buf)` on the kernel-printk pipe.
  Same shape as M1 but the producer is the guest kernel via Vz, not the
  guest userspace via the Carrier bridge.
- **Why it matters lower than M1:** Linux kernel printk is well-behaved
  (`PRINTK_BUF_LEN` is typically 1 KB); a malicious or buggy guest
  kernel **could** exceed that. The bytes go to `tracing` then drop,
  not back to a sender — but the host-side `String` still grows.
- **Defence in place:** none, beyond Linux's convention.
- **Agent recommendation:** **fix-now alongside M1** (same change shape;
  saves a second commit).
- **Scope of fix:** ~5 LOC change; existing tests in
  `console_forwarder.rs` cover the happy path.

#### M3 — JSON nesting-depth resilience needs verification
- **File:line:** `elastos/crates/elastos-server/src/carrier_bridge.rs:96`
  (in `parse_carrier_line`) and `:479` (in `handle_request`).
- **Observed:** `serde_json` 1.0.149 (the workspace version) documents
  a default recursion limit of **128** for both `from_str` and
  `Deserializer`, so deeply-nested input should `Err(RecursionLimitExceeded)`
  rather than overflow the stack. **The agent did not verify this
  experimentally on the actual call path** — the limit is the default,
  but a transitive feature flag or a custom `Deserializer` builder
  elsewhere in the workspace could disable it.
- **Why it matters if the default has been disabled:** within
  `CARRIER_MAX_LINE_BYTES` (1 MiB) the attacker has roughly enough
  room for ~500K nesting levels — well past any feasible stack budget.
- **Defence in place:** `serde_json`'s documented 128-deep default;
  `CARRIER_MAX_LINE_BYTES` byte cap.
- **Agent recommendation:** **reviewer-verify-then-decide.** A
  ~3-line fuzz seed with a deeply-nested document landed in the
  Day 4-8 corpus + a 5-minute re-run should empirically confirm
  the limit holds for both `parse_carrier_line` (Value) and
  `handle_request` (typed `RequestEnvelope`). If clean, mark as
  defended; if not, the agent applies the recursion-limit override
  as a follow-up commit.
- **Scope of fix (if needed):** ~5 LOC change to add an explicit
  `Deserializer::from_str(...).into_iter::<RequestEnvelope>()` with
  the limit explicitly pinned; one new fuzz seed.

#### M4 — No upper bound on `mem_size_mib` or `vcpu_count`
- **File:line:** `elastos/crates/elastos-vz/src/config.rs:386-387`
  (`from_manifest`).
- **Observed:** `manifest.resources.memory_mb` is a `u32` and used
  unchecked; `vcpu_count` is a `u8`. A capsule manifest can request
  4 TiB of RAM (2^32 MiB) or 255 vCPUs.
- **Why it matters:** Apple's `validateWithError` rejects unrealistic
  values, but it's expensive (it tries to commit memory). A
  manifest-driven DoS where install of a single capsule briefly stalls
  the supervisor is possible.
- **Defence in place:** Apple's validation as the final gate.
- **Agent recommendation:** **fix-now, small.** Reject obviously
  unreasonable values at `from_manifest` time with a typed error.
  Suggested caps for alpha: `memory_mb <= 65_536` (64 GiB),
  `vcpu_count <= 32`. Make caps configurable via `VzConfig`.
- **Scope of fix:** ~15 LOC change in `config.rs`; one regression test.

#### M5 — Fuzz coverage does not extend to typed `RequestEnvelope` deserialize
- **File:line:** `elastos/crates/elastos-server/fuzz/fuzz_targets/carrier_bridge_framing.rs`
- **Observed:** Day 4-8's harness exercises `parse_carrier_line`, which
  uses `serde_json::Value` (permissive). Production's `handle_request`
  re-parses with the typed `RequestEnvelope`/method-specific shapes
  and dispatches into provider code. The narrow-typed deserialize can
  fail differently than the permissive one.
- **Why it matters:** the harness's "no panics" finding is **only**
  for the framing layer. Bugs in the typed dispatch are not covered.
- **Defence in place:** documented honestly in `PHASE_10_DAY_8_NOTES.md`
  ("what this fuzz harness does not yet prove").
- **Agent recommendation:** **defer to follow-up Phase 11.** The
  packet flagging this is sufficient; expanding the harness to fuzz
  the typed dispatch is a multi-day effort and out of Phase 10's
  Mac-substrate scope.

### Lows

#### L1 — `format_validate_error` matches on lowercase `"entitlement"`
- **File:line:** `elastos/crates/elastos-vz/src/ffi/lifecycle.rs:558`
- **Observed:** entitlement-missing detection uses
  `apple_message.to_lowercase().contains("entitlement")`. If Apple's
  error string is ever localised to French / Japanese / etc., the
  hint embed is skipped (the raw Apple message still surfaces).
- **Recommendation:** accept-with-rationale. The fallback is the raw
  Apple message — operator still sees what went wrong. Could be
  hardened by also matching on the numeric `VZErrorCode`, but that
  requires the typed-error refactor to land first.

#### L2 — Vz-side dispatch queue blocks on `exec_sync` could deadlock if called recursively
- **File:line:** `elastos/crates/elastos-vz/src/ffi/lifecycle.rs:419-438` (`current_state`)
- **Observed:** `current_state` calls `queue.as_raw().exec_sync(...)`.
  GCD's `dispatch_sync` deadlocks if called from inside another
  closure already executing on the same serial queue.
- **Defence in place:** no current path calls `current_state` from
  inside a dispatch-queue closure. The agent traced every call site.
- **Recommendation:** accept-with-rationale + leave a comment at the
  function header warning future maintainers. The hazard is real but
  not currently reachable.

#### L3 — `unsafe impl Send for SendableVm` enforced by convention
- **File:line:** `elastos/crates/elastos-vz/src/ffi/lifecycle.rs:119-130`
- **Observed:** the wrapper documents "never deref `SendableVm.0` from
  arbitrary threads" as a maintenance contract, not a compile-time
  invariant. A future maintainer could `vm.0.start()` outside an
  `exec_sync` and the compiler would allow it.
- **Recommendation:** accept-with-rationale. Wrapping the deref in a
  type-state pattern (`VmOnQueue<'q>`) would be cleaner but is a
  significant refactor. The current convention is auditable.

#### L4 — Vsock `connectToPort` accepts arbitrary u32 port unchecked
- **File:line:** `elastos/crates/elastos-vz/src/ffi/vsock.rs:130`
- **Observed:** no validation on `port` before passing to Apple.
- **Defence in place:** Apple rejects unconfigured ports with
  `ENOTCONN`; the host is the caller, not the guest.
- **Recommendation:** accept-with-rationale. The port range is the
  caller's concern, and the caller is host-side supervisor code.

#### L5 — State dir created with default umask
- **File:line:** `elastos/crates/elastos-vz/src/provider.rs:77-84`
- **Observed:** `tokio::fs::create_dir_all` uses the process umask.
  On macOS default umask (022), state dir becomes 755 → readable by
  other local users.
- **Why it matters slightly:** the dir contains per-VM machine
  identifier blobs (Apple's hardware-UUID seed). Knowing one doesn't
  enable a direct attack, but it's a fingerprint.
- **Recommendation:** fix-low. After `create_dir_all`, explicitly
  `chmod 0700` the state dir. ~5 LOC.

#### L6 — `with_session` boot-arg concatenation
- **File:line:** `elastos/crates/elastos-vz/src/config.rs:420-425`
- **Observed:** `format!("{} elastos.token={} elastos.api={}", ...)`.
  If `token` or `api_addr` ever contained whitespace, equals signs,
  or null bytes, they'd parse incorrectly as kernel command-line
  tokens.
- **Defence in place:** the caller (supervisor) generates these from
  UUIDv4 and `127.0.0.1:port`-shaped strings.
- **Recommendation:** fix-low (defence in depth). Validate
  `token` and `api_addr` against `[a-zA-Z0-9:/.\-_]+` at the
  `with_session` entry point. ~10 LOC.

#### L7 — `debug_assert!` on `console=hvc0` only fires in debug builds
- **File:line:** `elastos/crates/elastos-vz/src/ffi/boot_loader.rs:94-97`
- **Observed:** release builds skip the assertion. A misconfigured
  caller could ship a command line without `console=hvc0` → silent
  boot.
- **Defence in place:** `vz_boot_args()` enforces the rewrite at the
  config layer.
- **Recommendation:** accept-with-rationale. The config-layer enforcement
  is the real contract; the `debug_assert!` exists as a smoke check.

#### L8 — Test-only setters are `pub` in release builds
- **File:line:** `elastos/crates/elastos-vz/src/vm.rs:178, 193, 210`
- **Observed:** `set_last_exit_reason_for_testing` /
  `set_last_vz_error_for_testing` / `set_status_for_testing` are
  marked `#[doc(hidden)]` but still `pub` and compiled into release
  builds.
- **Defence in place:** only `elastos-server` depends on `elastos-vz`
  (workspace-internal), so the external API surface is closed.
- **Recommendation:** accept-with-rationale for alpha. For production
  release, wrap behind `#[cfg(any(test, feature = "test-hooks"))]`
  so they vanish from the default release build.

#### L9 — Identifier file `identifier.bin` written with default permissions
- **File:line:** `elastos/crates/elastos-vz/src/ffi/platform.rs:123`
- **Observed:** `fs::write(path, ...)` uses default umask. Same
  shape as L5.
- **Recommendation:** fix-low alongside L5 (single commit).

#### L10 — `take_running_vm` second-call returns CapsuleNotFound silently
- **File:line:** `elastos/crates/elastos-vz/src/provider.rs:328-332`
- **Observed:** documented as "fails closed", but a supervisor bug
  that calls `take` twice would be indistinguishable from "never loaded".
- **Recommendation:** accept-with-rationale (matches the documented
  contract).

### Where the agent is confident (positive findings to triage out)

The reviewer can shortcut these areas — the agent has verified them
either by reading or by Day-9 regression:

- **SIGINT/SIGTERM teardown path lands in `drop(provider)`** — verified
  by `scripts/dev/test-sigint-graceful.sh`, both signals → ~1 s clean
  exit, no orphaned Vz XPC.
- **`provider.stop()` is bounded** by a 10 s timeout via
  `tokio::time::timeout` (`run_cmd.rs:VZ_STOP_TIMEOUT`).
- **Apple delegate first-wins semantics** — covered by
  `delegate.rs::delegate_signal_exit_sends_first_terminal_observation_only`.
- **`parse_carrier_line` framing parser** — 2.4 M fuzz iterations
  in 5 minutes, zero panics, zero crashes (`PHASE_10_DAY_8_NOTES.md`).
- **Entitlement check returns false fail-closed** on every error path
  in `ffi/entitlement.rs::check_entitlement_via_security_framework`
  — every CFRelease pairs correctly even on early-return.
- **Block-device attachments** use `cachingMode=Cached` +
  `synchronizationMode=Fsync` (no host-crash tearing).
- **Per-VM machine identifier** persists across reboots; corrupt
  identifier surfaces a typed error rather than silently regenerating.
- **Bridged-network entitlement is fail-closed** — a capsule
  requesting `guest_network` without `com.apple.vm.networking`
  is rejected with a typed message naming both the entitlement and
  the manifest field. NO silent NAT downgrade.
- **All deprecated late-binding APIs** (`set_session_for_vm`,
  `set_network_for_vm`, `append_boot_args_for_vm`) fail closed with
  typed migration messages.
- **Send/Sync contracts on `SendableVm` and `SendableDelegate`** are
  discharged by routing every touch through `VzDispatchQueue::exec_sync`/
  `exec_async`. Convention-enforced (see L3) but currently watertight.

### Open questions the agent could not resolve from the code alone

The reviewer is asked to either resolve these or flag them as
out-of-scope for this engagement:

- **Q1.** When `VZVirtioSocketConnection` is released (after our
  `dup()` in `vsock.rs:170`), does Apple call `close()` or
  `shutdown()` on its internal fd? If `shutdown()`, our duplicated
  fd becomes useless. Apple's docs are silent.
- **Q2.** Does `VZVirtualMachine.stop` always invoke its completion
  handler exactly once? The substrate's first-wins channel handles
  zero-or-many gracefully, but the typed-exit-reason classifier
  assumes exactly-one.
- **Q3.** Are there any documented Apple-side resource leaks if a
  `VZVirtualMachine` is constructed but never started (e.g. due to
  a `start()` error)? The substrate drops the handle in that case;
  if Apple leaks kernel state, drops in a tight loop could
  accumulate.

---

## 5. What "review complete" means

The reviewer's deliverable is a written **finding log** that the
operator collates into `docs/vz-backend/PHASE_10_DAY_13_NOTES.md`,
covering:

For each reviewer finding:

| Field | Required content |
|-------|------------------|
| **ID** | `R1`, `R2`, … (independent from the agent's `M*` / `L*` IDs above) |
| **Severity** | High / Medium / Low (same rubric as §4) |
| **File:line** | Anchored to the in-scope diff |
| **Observed** | What the reviewer saw |
| **Why it matters** | Concrete consequence |
| **Disposition** | One of: `fix-now` (with commit SHA once landed), `accept-with-rationale` (with rationale text), or `out-of-scope` (with reason) |

The phase only signs off (Day 15) when:

- Every reviewer **High** is `fix-now` and landed, OR explicitly
  `accept-with-rationale` by the operator with sign-off attached.
- Every reviewer **Medium** has a disposition (any of the three).
- Every reviewer **Low** has at minimum been triaged (disposition can
  be `deferred-to-Phase-11`).
- The agent's own §4 findings each carry a matching reviewer
  disposition (either "concur" or "disagree with rationale").

---

## 6. Reviewer contact protocol

During the review window:

1. **Clarifying questions** — the reviewer files them in writing
   (email or shared doc thread). The operator forwards each
   question to the agent. The agent answers in writing, in this
   repo, by appending to `docs/vz-backend/PHASE_10_DAY_11_QA.md`
   (created on first question). No out-of-band conversations; all
   answers must be auditable.

2. **Agreed remediations** — the agent applies them as new commits
   on `sash/local-test`, one finding per commit, with the finding
   ID (e.g. `R3`) in the commit subject line. The commit message
   names the file:line, the fix, and the test that proves it.

3. **CI green** — the agent keeps the existing test suite green
   throughout the window. If a reviewer's remediation breaks an
   existing test the agent surfaces the conflict before landing.

The agent **does not** during this window:

- Add new substrate features.
- Touch any code outside the in-scope paths (§1).
- Apply remediations the reviewer has not explicitly approved.
- Re-scope the review (e.g. "I found something interesting in
  `elastos-server::supervisor` — let me also look at that").
  Anything out-of-scope is flagged to the operator as a future
  task; this branch stays focused.

---

## Appendix A — Commit history for in-scope code on this branch

```
$ git log --oneline main..sash/local-test -- \
    elastos/crates/elastos-vz/ \
    elastos/crates/elastos-server/src/carrier_bridge.rs
```

(Reviewer: run the above to get the linear history. Of note: the
`Phase 4` series adds typed `VzError` / `VzExitReason`; the
`Phase 8` series adds interactive-stdio + overlay-rootfs; the
`Phase 10` series this week adds the SIGINT fix and the fuzz
harness.)

## Appendix B — Sanity-check commands the reviewer can run locally

```bash
# Confirm in-scope LOC count
find elastos/crates/elastos-vz/src -name '*.rs' -print0 \
  | xargs -0 wc -l | tail -1

# Confirm unsafe block count (every unsafe in this branch is in elastos-vz)
rg -n '^\s*unsafe\s*\{' elastos/crates/elastos-vz/src/ | wc -l

# Confirm no `unwrap()` on FFI-return paths
rg -n 'unsafe.*\.unwrap\(\)' elastos/crates/elastos-vz/src/

# Re-run the framing fuzz harness for 10 minutes
cd elastos/crates/elastos-server/fuzz && \
  cargo +nightly fuzz run carrier_bridge_framing -- -max_total_time=600

# Re-run the SIGINT/SIGTERM regression
./scripts/dev/test-sigint-graceful.sh
```

---

*End of packet.*
