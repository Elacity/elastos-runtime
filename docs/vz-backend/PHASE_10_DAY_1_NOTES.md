# Phase 10 Day 1 — CVE audit (findings + remediation plan)

> Status: **Audit complete.** Triage complete. Remediation broken into named
> sub-days because the scope is larger than a single day can honestly absorb.
> Predecessor: `PHASE_10_PLAN.md`.

## TL;DR

- Ran `cargo audit v0.22.1` against branch HEAD on the `elastos/` workspace.
- **34 vulnerabilities and 12 warnings** found.
- **2 are CVSS 9.0 (CRITICAL).** Both in `wasmtime 17.0.3`, both directly
  relevant to our usage (one is "Guest-controlled resource exhaustion in
  WASI implementations" — WASI is exactly what our WASM capsules run on).
- **14 of the 34 vulnerabilities are in `wasmtime 17.0.3`**, which is **28
  major versions behind** the current release (17 → 45). Fixing them
  requires a focused wasmtime-bump sub-phase (Day 1c-1f below) because
  the wasmtime API has been substantially redesigned across that span.
- `cargo update` (passive patch-level updates only) would close **14** of
  the 34 in one shot **but breaks the build**: `pkcs8 0.11.0-rc.11 → 0.11.0`
  is incompatible with `ed25519-dalek`'s current call site. That's its own
  named sub-day (Day 1b).
- Day 1 ships: this notes file, the structured triage table, and the
  scoped remediation plan. **No source bumps applied today** (every
  attempted bump either broke the build or required a multi-day migration).
  This is the right call per Principle #2 (honest gap reporting) and #4
  (every commit ships green tests).

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
4. **No source changes shipped.** Per Principle #4, every commit ships
   green tests. Today's deliverable is this notes file + the next-day
   plan, not breakage.

## Re-scoped remediation plan (sub-days inside the original Day 1 budget)

The original Phase 10 plan said "if any HIGH or CRITICAL, fix before
close-of-day." That's not honest with the actual scope. Re-scoping:

### Day 1a — TODAY: audit + triage + plan
This document. **DONE.**

### Day 1b — `cargo update` cascade fix
- Investigate the `pkcs8` RC → stable transition.
- Either bump `ed25519-dalek` to a release that supports stable `pkcs8`,
  or pin `pkcs8` to a version that doesn't trip the cascade.
- Re-run `cargo update`, verify build, re-run `cargo audit`.
- Expected close: **14 vulnerabilities** (assuming `cargo update` after the
  pin closes the same set we saw today).
- Effort: 0.5-1 day.

### Day 1c-1f — wasmtime 17 → 45 migration
- Read upstream wasmtime CHANGELOG for breaking changes across the 28
  major versions.
- Stand up a temporary `elastos-compute-wasmtime45` crate alongside the
  existing one; port the WASI host functions and store/instance API usage
  to the new surface.
- Verify the home/system WASM capsules still execute against the new host.
- Cut over `elastos-compute` to wasmtime 45; remove the temporary crate.
- Re-run `cargo audit`.
- Expected close: **15 vulnerabilities** (including both 9.0 criticals).
- Effort: 3-5 days.

### Day 1g — Targeted Cargo.toml bumps
- For each remaining advisory not in the wasmtime cluster, identify whether
  it's a direct dep (bump in our `Cargo.toml`) or transitive (force-resolve
  via `[patch]` or wait for upstream).
- Apply bumps; verify build; re-run audit.
- Expected close: most of the remaining ~5-7 advisories.
- Effort: 1 day.

### Day 1h — Unmaintained-crate replacements
- For each `warn:unmaintained`: identify a maintained replacement or
  evaluate accepted-risk.
- The `mach` and `atomic-polyfill` ones in particular are likely
  swappable for `mach2` and stable atomic primitives.
- Document each decision (replace / accept-with-rationale).
- Effort: 0.5-1 day.

### Day 1i — Re-audit + Day 1 closeout
- Re-run `cargo audit`.
- Target: **zero HIGH or CRITICAL**, all remaining advisories either fixed
  or carrying an explicit accepted-risk note signed off by the reviewer.
- Update this file with the close-out delta.
- Effort: 0.5 day.

**Total Day 1 sub-budget:** 6-9 dev-days. This pushes Phase 10's overall
calendar from 18 days to ~25 days. Honest re-scope, surfacing now rather
than at end of phase.

## Triage decisions documented

For the team — when you read the structured table, you'll see "warn:" rows
with no CVSS score. These are advisories where the upstream RustSec author
chose not to assign a CVSS. We still treat them as actionable; they're just
not blockers in the same way the scored ones are.

### Why we're not pretending `cargo audit` results map 1:1 to risk

CVSS is a useful prioritization signal, not a perfect one. Some of our
findings are technically advisory matches but are unreachable in our
usage:

- The `cap-primitives` Windows-device-filenames advisory: we don't run on
  Windows. **Will close as accepted-risk in Day 1h.**
- The `wasmtime` Windows-device-filenames advisory: same.
- The `wasmtime` Winch-backend advisories: we use Cranelift, not Winch.
  **Need to verify** in `elastos-compute/Cargo.toml` what wasmtime
  features we enable. If Winch is disabled, these are accepted-risk.
- The two `wasmtime` advisories scored 2.3 for aarch64 sandbox escape:
  **directly applicable to us** (we run on Apple Silicon aarch64). Must
  fix via the wasmtime bump.

Day 1h will produce a verified-applicable / accepted-risk classification
for every remaining row.

## Linked artifacts

- Updated `BRANCH_SUMMARY.md` security section with a note that
  cargo-audit Day 1 has been performed and the results are here.
- `PHASE_10_PLAN.md` retains the original day budget; this file is the
  authoritative re-scope record.

## What this means for the BRANCH_SUMMARY share

The `BRANCH_SUMMARY.md` already states "the substrate has never been
reviewed by an outside set of eyes" and "no `cargo audit` has been run."
Now that the audit has been run, the team should see the results. The
honest framing for them:

> *"We ran the audit. Found 34 vulnerabilities + 12 warnings. 2 are
> critical, both in our pinned wasmtime version which is 28 majors behind.
> The fix is a focused multi-day sub-phase; remediation plan is documented
> at PHASE_10_DAY_1_NOTES.md."*
