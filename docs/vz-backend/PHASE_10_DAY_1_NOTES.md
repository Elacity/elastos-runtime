# Phase 10 Day 1 — CVE audit (findings + ownership classification + remediation plan)

> Status: **Audit complete. Ownership classified.** All 34 vulnerabilities are
> **inherited from `main`** — zero introduced by our branch. This re-scopes
> Phase 10 to focus on the security work that is legitimately ours (Mac
> substrate, Carrier-bridge fuzzing, demo bugs, threat model) and flags the
> inherited workspace-wide vulnerabilities to the broader runtime team via
> `RUNTIME_CVE_HANDOFF.md`.
>
> Predecessor: `PHASE_10_PLAN.md`. Sibling: `RUNTIME_CVE_HANDOFF.md`.

## TL;DR

- Ran `cargo audit v0.22.1` against branch HEAD on the `elastos/` workspace.
- **34 vulnerabilities and 12 warnings** found.
- **2 are CVSS 9.0 (CRITICAL).** Both in `wasmtime 17.0.3`.
- **Ownership: 34/34 are INHERITED from `main`.** Every vulnerable crate
  exists on `main` at the **same version** as on our branch. Zero of our new
  Mac-only direct dependencies (`objc2`, `objc2-virtualization`,
  `objc2-foundation`, `block2`, `dispatch2`) have audit findings.
- **Conclusion:** the inherited vulnerabilities are a workspace-wide concern
  that affects both Linux and Mac builds, and the fixes (notably the
  wasmtime 17→45 bump and the TLS chain refresh) benefit both platforms.
  They should be addressed on a **separate branch off `main`** owned by
  the broader runtime team — not coupled into this Mac substrate branch.
- **Re-scope of Phase 10 on `sash/local-test`:** drop the "fix all 34
  workspace vulnerabilities" target; keep only the security work that is
  legitimately Mac-substrate-scoped. Total Phase 10 calendar drops from
  ~25 days back to ~14 days.
- Handoff document for the broader team: `RUNTIME_CVE_HANDOFF.md`.

## Full audit log

- Raw: `/tmp/cargo-audit-day1.log` (not committed — regeneratable by running
  `cargo audit` in `elastos/`).
- Parsed structured: `/tmp/audit-table.tsv` (not committed — same).

## Severity histogram

| CVSS | Count | Notes |
|---|---|---|
| 9.0 (CRITICAL) | 2 | Both `wasmtime` |
| 8.7 (HIGH) | 1 | `rustls-webpki` panic |
| 7.5 (HIGH) | 3 | `bytes` integer overflow, AWS-LC PKCS7 chain bypass, `atomic-polyfill` unmaintained |
| 7.4 (HIGH) | 1 | AWS-LC PKCS7 signature bypass |
| 6.9 (MEDIUM) | 3 | `wasmtime` UTF-16 panics + segfault |
| 6.8 (MEDIUM) | 1 | `wasmtime` Winch 64-bit table host data leak |
| 6.1 (MEDIUM) | 2 | `wasmtime` `flags` panic, `wasmtime-wasi` `path_open(TRUNCATE)` bypass |
| 5.9 (MEDIUM) | 3 | AWS-LC name constraints bypass, `wasmtime` panic adding fields, `wasmtime` Windows device filenames |
| 5.6 (MEDIUM) | 1 | `wasmtime` Winch sandbox escape |
| 5.1 (MEDIUM) | 2 | `tar` chmod-via-symlink, `time` stack exhaustion |
| 4.1 (MEDIUM) | 1 | `wasmtime` pooling allocator data leak |
| 3.3 (LOW) | 1 | `wasmtime` `table.grow` Winch |
| 2.3 (LOW) | 3 | `hickory-proto` O(n²) compression, `wasmtime` aarch64 Cranelift, `wasmtime` `table.fill` panic |
| 1.8 (LOW) | 1 | `wasmtime` transcoding write |
| warn (unsound / unmaintained / unscored) | 16 | See triage table |

## Pattern analysis

| Source crate | Count | Status |
|---|---|---|
| `wasmtime` + `wasmtime-wasi` + `wasmtime-jit-debug` | **15** | All require wasmtime 17 → 45 bump. Single dominant cluster. |
| `aws-lc-sys` (via `reqwest`/TLS chain) | **5** | Fixable by `reqwest` minor bump pulling newer rustls/aws-lc-sys. |
| `rustls-webpki` + `rustls-pemfile` | **5** | Same `reqwest`/rustls chain. |
| `wasmtime` ecosystem-adjacent (cap-primitives, tar) | **3** | `cap-primitives` is wasmtime's sandbox; `tar` is direct dep. |
| Other (bytes, time, hickory-proto, lru, mach, paste, fxhash, bincode, atomic-polyfill, core2, rand, quinn-proto) | **18** | Mix; some fixable via targeted bump, some unmaintained (need replacement). |

**One sentence summary:** *bumping wasmtime closes ~40% of the findings; the
rest are addressable via targeted bumps in our `Cargo.toml` + a handful of
unmaintained-crate replacements.*

## Concrete actions taken today

1. **Installed `cargo-audit v0.22.1`** via `cargo install cargo-audit --locked`.
2. **Ran `cargo audit` against the workspace.** Captured full advisory list.
3. **Attempted `cargo update` (semver-compatible patch updates).** Result:
   reduces vulnerabilities from 34 → 20 (closes 14), but breaks the build
   on `ed25519-dalek` because `pkcs8 0.11.0-rc.11 → 0.11.0` changed the
   `Error::KeyMalformed` variant signature. **Reverted `Cargo.lock` to
   pre-update state; build is green again.**
4. **Classified every vulnerable crate by ownership.** Used `main`'s
   `Cargo.lock` as ground truth: if the crate appears in `main` at the
   same version, the vulnerability is INHERITED; if only in our branch,
   INTRODUCED. **Result: 34/34 INHERITED, 0 INTRODUCED.** See full table
   below and ownership verification commands in
   `RUNTIME_CVE_HANDOFF.md` §Verification.
5. **No source changes shipped.** Per Principle #4, every commit ships
   green tests. Today's deliverable is this notes file + the ownership
   classification + the broader-team handoff doc, not source breakage.

## Ownership classification

| Crate | Version | Severity range of advisories | In `main` at same version? | Ownership |
|---|---|---|---|---|
| `wasmtime` | 17.0.3 | 1.8 – 9.0 | yes | **INHERITED** |
| `wasmtime-wasi` | 17.0.3 | 6.1 | yes | **INHERITED** |
| `wasmtime-jit-debug` | 17.0.3 | warn:unsound | yes | **INHERITED** |
| `aws-lc-sys` | 0.37.0 | 5.9 – 7.5 | yes | **INHERITED** |
| `rustls-webpki` | 0.103.9 | 8.7 + warns | yes | **INHERITED** |
| `rustls-pemfile` | 2.2.0 | warn:unmaintained | yes | **INHERITED** |
| `bytes` | 1.11.0 | 7.5 | yes | **INHERITED** |
| `tar` | 0.4.44 | 5.1 + warn | yes | **INHERITED** |
| `time` | 0.3.46 | 5.1 | yes | **INHERITED** |
| `hickory-proto` | 0.25.2 | 2.3 + warn | yes | **INHERITED** |
| `quinn-proto` | 0.11.13 | warn | yes | **INHERITED** |
| `lru` | 0.12.5 | warn:unsound | yes | **INHERITED** |
| `rand` | 0.8.5 / 0.9.2 | warn:unsound | yes | **INHERITED** |
| `atomic-polyfill` | 1.0.3 | 7.5 (unmaintained) | yes | **INHERITED** |
| `bincode` | 1.3.3 | warn:unmaintained | yes | **INHERITED** |
| `mach` | 0.3.2 | warn:unmaintained | yes | **INHERITED** |
| `fxhash` | 0.2.1 | warn:unmaintained | yes | **INHERITED** |
| `paste` | 1.0.15 | warn:unmaintained | yes | **INHERITED** |
| `core2` | 0.4.0 | warn:unmaintained | yes | **INHERITED** |
| `cap-primitives` | 2.0.2 | warn | yes | **INHERITED** |
| `objc2` and family (NEW from `elastos-vz`) | 0.6 / 0.3 | n/a | n/a | **INTRODUCED** — zero findings |
| `block2`, `dispatch2` (NEW from `elastos-vz`) | 0.6 / 0.3 | n/a | n/a | **INTRODUCED** — zero findings |

**Net new attack surface from our branch (per cargo-audit):** zero crates
with known advisories.

## Why this changes Phase 10 scope

The original Phase 10 plan implicitly assumed "Phase 10 includes fixing all
workspace CVEs." That was wrong on two grounds:

1. **Architectural correctness.** The vulnerable crates affect both Linux
   and Mac builds. Bumping `wasmtime`, refreshing the TLS chain, or
   replacing unmaintained crates is a runtime-wide concern. Shipping those
   fixes as part of a "Mac substrate" branch couples unrelated concerns
   and lands them in `main` under a misleading PR title.

2. **Ownership clarity.** Our branch's contract with `main` is *"swap the
   compute substrate on Mac without touching anything else."* The
   `check-linux-untouched.sh` script enforces exactly this property today.
   Bumping wasmtime would violate it (wasmtime is consumed by Linux's
   `elastos-compute` too) and break the clean Linux-untouched audit trail
   that makes this branch easy to review.

The right split:

- **THIS branch (`sash/local-test`) Phase 10** focuses only on
  Mac-substrate-scoped security work — see `PHASE_10_PLAN.md` for the
  re-scoped day-by-day.
- **A separate branch off `main`** (suggested name:
  `chore/runtime-cve-hygiene`) handles all 34 inherited vulnerabilities,
  owned by the runtime team, benefiting both platforms. The handoff
  document with full triage is `RUNTIME_CVE_HANDOFF.md` — hand this to
  the broader team.

## What we will and won't fix on this branch

| Vulnerability source | Fix here? | Why |
|---|---|---|
| Carrier-bridge framing parser bugs surfaced by fuzzing (Days 4-8) | **YES** | We co-own this code; we're the ones who will fuzz it; findings here are inherent to the Mac substrate's trust boundary. |
| `elastos-vz` itself findings (none today) | **YES** | New substrate, our code, our scope. |
| Mac-specific bootstrap / signing / notarization gaps | **YES** | Mac-only surfaces, our scope. |
| SIGINT graceful shutdown for `elastos run` | **YES** | Mac-substrate user-facing bug. |
| Test-binary auto-resign | **YES** | Mac-bootstrap gap. |
| All 34 inherited CVEs (wasmtime cluster, TLS chain, utility crates, unmaintained crates) | **NO** | Pre-exist on `main`, affect Linux too, fixing them on this branch couples concerns and breaks `check-linux-untouched.sh`. Handed off to broader team via `RUNTIME_CVE_HANDOFF.md`. |

## Triage decisions documented

For the team — when you read the structured table, you'll see "warn:" rows
with no CVSS score. These are advisories where the upstream RustSec author
chose not to assign a CVSS. We still treat them as actionable; they're just
not blockers in the same way the scored ones are.

### Why we're not pretending `cargo audit` results map 1:1 to risk

CVSS is a useful prioritization signal, not a perfect one. Some of our
findings are technically advisory matches but are unreachable in our
usage. We flagged these for the broader team in `RUNTIME_CVE_HANDOFF.md`
so they can decide accepted-risk vs fix:

- `cap-primitives` and `wasmtime` Windows-device-filenames advisories:
  neither platform we ship runs on Windows. Likely accepted-risk.
- `wasmtime` Winch-backend advisories: we use Cranelift, not Winch.
  Likely accepted-risk pending feature-flag verification.
- `wasmtime` aarch64 Cranelift advisories: **directly applicable to us**
  (Apple Silicon). Must fix via wasmtime bump on the runtime-hygiene
  branch.

## Linked artifacts

- `RUNTIME_CVE_HANDOFF.md` — the handoff document for the broader runtime
  team covering all 34 inherited vulnerabilities, with severity, our usage
  notes, and recommended remediation per crate cluster.
- `PHASE_10_PLAN.md` — re-scoped Mac-substrate-only security work.
- `BRANCH_SUMMARY.md` — security section updated to reflect the
  ownership split.

## Quotable Day 1 conclusion

> *"Cargo audit of `sash/local-test` found 34 vulnerabilities, all inherited
> from `main`, none introduced by the Mac substrate work on this branch.
> Phase 10 on this branch will focus on Mac-substrate-scoped security work
> (Carrier-bridge fuzzing, threat model, demo bugs, sign+notarize CI).
> The inherited workspace-wide CVEs are flagged to the broader runtime
> team via RUNTIME_CVE_HANDOFF.md, to be addressed on a separate branch
> off main where they benefit both Linux and Mac."*
