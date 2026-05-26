# Phase 10 — Mac Security Hardening + Release Polish

> Status: **Day 1 in progress** — CVE audit running, plan committed.
> Branch: `sash/local-test`.
> Predecessor: `PHASE_9_SIGNOFF.md` (engineering milestone signed off).
> Outcome target: a `sash/local-test` head state that a reasonable engineering
> lead would sign off as a public **Mac alpha** release.

## Why this phase exists

Phase 9 closed the engineering milestone: the substrate works, the bootstrap
works, the end-to-end paths work, all five sign-off smoke tests are green,
and the validation has been performed live on Apple Silicon. **This is
sufficient for internal use and dogfooding. It is not sufficient for public
release.**

The branch ships **7,724 LOC of new substrate code** in `elastos-vz` that has
never been reviewed by anyone outside the people who wrote it, plus a
parser (the Carrier-bridge framing) sitting directly on the host-guest
trust boundary that has never been fuzz-tested, plus 158 files of new
dependency churn that has never been CVE-scanned. Phase 10 closes those
gaps in a defined, time-boxed way.

## Scope

**In scope:**

- The Mac compute substrate (`elastos/crates/elastos-vz/`).
- The Mac bootstrap (`scripts/dev/mac-local-setup.sh`, `scripts/release-mac.sh`).
- The trust-boundary parsers (Carrier-bridge framing in `elastos-server/src/carrier_bridge.rs`).
- The release CI lane (sign + notarize + smoke).
- The two operator-facing demo bugs identified during the Phase 9 walkthrough
  (SIGINT-graceful-shutdown, test-binary auto-resign).

**Out of scope:**

- New substrate features (no new VM types, no new networking modes, no new device passthrough).
- New capsule types or new Home-surface apps.
- Performance work (deferred to a separate Phase 11 if needed).
- Home shell UI work, including the "Launch microVM" affordance gap from Phase 9.
- Intel-Mac validation (deferred; needs hardware access).
- Cross-platform rootfs builds (deferred; out of substrate scope).

## Success criteria (the Phase 10 sign-off matrix)

| # | Criterion | Evidence |
|---|---|---|
| S1 | `cargo audit` reports zero HIGH or CRITICAL advisories against branch HEAD | Captured output in `PHASE_10_DAY_1_NOTES.md` (and re-run in `PHASE_10_SIGNOFF.md`) |
| S2 | Written threat model exists, enumerates every trust boundary, names enforcement mechanism for each, names "what would constitute a break" | `MAC_THREAT_MODEL.md` |
| S3 | Carrier-bridge framing parser has a cargo-fuzz harness checked in, seed corpus committed, runs cleanly for at least 24 wall-clock hours on a developer machine | `elastos/crates/elastos-server/fuzz/` + `PHASE_10_DAY_8_NOTES.md` |
| S4 | `elastos run` responds to SIGINT with graceful Vz `provider.stop()` (no SIGKILL needed) | Regression test + commit |
| S5 | `cargo test -p elastos-vz` succeeds in a fresh checkout without manual `sign.sh` invocation | Bootstrap re-runs auto-resign on test binaries; verified by CI |
| S6 | External security review of `elastos-vz` complete; reviewer's findings either fixed or explicitly accepted with rationale | Remediation log in `PHASE_10_DAY_15_NOTES.md` |
| S7 | GitHub Actions lane on Mac runner: builds release artifact, applies entitlements, codesigns with Developer ID, notarizes, runs full Phase 9 smoke matrix; failure blocks merge | `.github/workflows/mac-release-smoke.yml` green |
| S8 | Phase 9 5-test smoke matrix reruns green at Phase 10 close (regression check) | `PHASE_10_SIGNOFF.md` |

## Day-by-day plan

### Day 1 — CVE audit

- Install `cargo-audit --locked`.
- Run against the workspace from branch HEAD.
- For every finding, capture: advisory ID, severity, affected crate, our usage
  (direct dependency or transitive), upstream fix availability, our chosen action.
- If any HIGH or CRITICAL: resolve before close-of-day (via dependency bump
  or explicit accepted-risk note with reviewer sign-off).
- **Deliverable:** `PHASE_10_DAY_1_NOTES.md`.

### Day 2-3 — Threat model

- Enumerate trust boundaries:
  - Host operator ↔ supervisor process.
  - Supervisor ↔ Apple Vz framework.
  - Guest VM ↔ Carrier bridge.
  - Capsule ↔ capsule (must enforce: **not possible directly**).
  - Operator ↔ runtime API.
  - Upstream component registry ↔ runtime.
- For each boundary: attacker capability on the untrusted side, mechanism that
  enforces the boundary, concrete event that would constitute a break.
- **Deliverable:** `docs/vz-backend/MAC_THREAT_MODEL.md`.

### Day 4-8 — Carrier-bridge fuzz harness

- Stand up `cargo-fuzz` for the Carrier-bridge framing parser.
- Seed corpus: existing test fixtures + Carrier messages captured during a
  managed Home run.
- Run continuously on a developer machine; triage each finding as a
  separate fix commit with regression test.
- **Deliverable:** `elastos/crates/elastos-server/fuzz/` (harness, corpus, dictionary), plus `PHASE_10_DAY_8_NOTES.md` with findings.

### Day 9-10 — Demo-bug fixes

- **SIGINT graceful shutdown for `elastos run`.** Add a signal handler that
  invokes `provider.stop()` and waits for the Vz machine to enter `Stopped`
  before exiting. Regression test verifies no `Virtualization.VirtualMachine`
  process survives a SIGINT-then-wait sequence.
- **Test-binary auto-resign in `mac-local-setup.sh`.** After `cargo build` /
  `cargo test --no-run`, locate every binary under `target/*/deps/elastos_vz*`
  and re-apply the entitlement plist, mirroring the existing Day-4 auto-resign
  for the main binary.
- Each fix in its own commit.

### Day 11-15 — External security review window

- Hand off branch + threat model + `BRANCH_SUMMARY.md` + Day-1 audit + Day-8
  fuzz findings to the external security reviewer.
- Agent work pauses except for: (1) answering reviewer clarification
  questions, (2) applying agreed remediations, (3) keeping CI green.
- **Deliverable:** `PHASE_10_DAY_15_NOTES.md` — remediation log with each
  finding, decision, and resolution commit.

### Day 16-17 — Release CI lane

- GitHub Actions workflow `mac-release-smoke.yml` runs on every PR touching
  `elastos-vz/`, `release-mac.sh`, or the entitlements plist.
- Steps: checkout → `release-mac.sh` (signs + notarizes) → drop signed binary
  into a fresh `mac-local-setup` flow → execute the Phase 9 5-test smoke matrix.
- Failure blocks merge.
- **Deliverable:** workflow file checked in and green on a non-trivial PR.

### Day 18 — Sign-off

- Rerun Phase 9's 5-test smoke matrix (regression check).
- Confirm `cargo audit` still zero criticals.
- Confirm fuzz harness has run cleanly for at least N hours since the
  reviewer's last remediation.
- Confirm reviewer sign-off attached.
- **Deliverable:** `PHASE_10_SIGNOFF.md`.

## Principles to uphold (carried from prior phases)

1. **No parallel mechanisms.** If `cargo-audit`, `cargo-fuzz`, or any
   upstream tool exists for a problem, use it. Do not invent Mac-only
   equivalents.
2. **Honest gap reporting.** If a day can't be completed in its window,
   document the deferral and the rationale rather than half-shipping.
3. **Substrate stays minimal.** Phase 10 must not grow `elastos-vz` LOC by
   more than **10%** (i.e., +772 LOC) without an explicit scope-change
   conversation with the operator.
4. **Every commit ships green tests.** No "WIP, will fix in next commit"
   commits.
5. **Anchor everything in evidence.** Every claim in `PHASE_10_*` notes must
   either link to a commit, a test output, or a captured tool log.

## Rolling status

- **Day 1** — in progress. CVE audit installing + running. Plan committed.
