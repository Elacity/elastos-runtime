# Phase 10 — Mac Security Hardening + Release Polish

> Status: **Day 1 DONE — re-scoped per ownership findings** (see below).
> Branch: `sash/local-test`.
> Predecessor: `PHASE_9_SIGNOFF.md` (engineering milestone signed off).
> Sibling: `RUNTIME_CVE_HANDOFF.md` (broader-team handoff for inherited CVEs).
> Outcome target: a `sash/local-test` head state that a reasonable engineering
> lead would sign off as a public **Mac alpha** release **assuming the
> parallel `chore/runtime-cve-hygiene` branch off `main` lands first or
> alongside.**

## Re-scope after Day 1 (May 26, 2026)

Day 1's CVE audit revealed that **all 34 vulnerabilities found by
`cargo audit` are inherited from `main` — zero were introduced by this
branch's Mac substrate work.** (Full classification in
`PHASE_10_DAY_1_NOTES.md`.) This means the "fix workspace CVEs" work is
not legitimately on this branch — it's a runtime-wide concern affecting
both Linux and Mac, handed off to the broader team via
`RUNTIME_CVE_HANDOFF.md`.

**Phase 10 on `sash/local-test` is therefore re-scoped to only the
security work that is legitimately Mac-substrate-scoped.** Calendar drops
from ~25 days back to ~14 days. The scope below reflects this.

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

**In scope (Mac-substrate-specific security work only):**

- The Mac compute substrate (`elastos/crates/elastos-vz/`) — external
  code review target.
- The Mac bootstrap (`scripts/dev/mac-local-setup.sh`, `scripts/release-mac.sh`).
- The trust-boundary parsers (Carrier-bridge framing in
  `elastos-server/src/carrier_bridge.rs`) — we co-own this; we're the
  heaviest user; fuzzing it benefits the substrate's threat model.
- The release CI lane (sign + notarize + smoke).
- The two operator-facing demo bugs identified during the Phase 9 walkthrough
  (SIGINT-graceful-shutdown, test-binary auto-resign).
- A written Mac substrate threat model documenting the trust boundaries
  unique to the Vz / entitlement / NAT-network surface.

**Out of scope on this branch (handed off via `RUNTIME_CVE_HANDOFF.md`):**

- **All 34 inherited workspace CVEs.** Per Day-1 ownership analysis, every
  vulnerable crate exists on `main` at the same version. The fixes
  (notably `wasmtime` 17→45 and the TLS chain refresh) affect both Linux
  and Mac and should be on a `chore/runtime-cve-hygiene` branch off
  `main`, not coupled into a Mac-substrate branch.

**Out of scope on this branch (deferred):**

- New substrate features (no new VM types, no new networking modes, no new device passthrough).
- New capsule types or new Home-surface apps.
- Performance work (deferred to a separate Phase 11 if needed).
- Home shell UI work, including the "Launch microVM" affordance gap from Phase 9.
- Intel-Mac validation (deferred; needs hardware access).
- Cross-platform rootfs builds (deferred; out of substrate scope).

## Success criteria (the Phase 10 sign-off matrix — re-scoped)

| # | Criterion | Evidence |
|---|---|---|
| S1 | CVE audit performed against branch HEAD; every finding classified by ownership (introduced / inherited / shared); inherited findings handed off to broader team | `PHASE_10_DAY_1_NOTES.md` + `RUNTIME_CVE_HANDOFF.md` (**DONE**) |
| S2 | Written Mac substrate threat model exists, enumerates every Mac-specific trust boundary (Vz, entitlements, Carrier bridge framing, NAT network, supervisor↔Vz IPC), names enforcement mechanism for each, names "what would constitute a break" | `MAC_THREAT_MODEL.md` |
| S3 | Carrier-bridge framing parser has a cargo-fuzz harness checked in, seed corpus committed, runs cleanly for at least 24 wall-clock hours on a developer machine | `elastos/crates/elastos-server/fuzz/` + `PHASE_10_DAY_8_NOTES.md` |
| S4 | `elastos run` responds to SIGINT with graceful Vz `provider.stop()` (no SIGKILL needed) | Regression test + commit |
| S5 | `cargo test -p elastos-vz` succeeds in a fresh checkout without manual `sign.sh` invocation | Bootstrap re-runs auto-resign on test binaries; verified by CI |
| S6 | External security review of **new `elastos-vz` LOC + Carrier-bridge framing on this branch** complete; reviewer's findings either fixed or explicitly accepted with rationale | Remediation log in `PHASE_10_DAY_13_NOTES.md` |
| S7 | GitHub Actions lane on Mac runner: builds release artifact, applies entitlements, codesigns with Developer ID, notarizes, runs full Phase 9 smoke matrix; failure blocks merge | `.github/workflows/mac-release-smoke.yml` green |
| S8 | Phase 9 5-test smoke matrix reruns green at Phase 10 close (regression check) | `PHASE_10_SIGNOFF.md` |
| S9 | `RUNTIME_CVE_HANDOFF.md` acknowledged by broader team; either `chore/runtime-cve-hygiene` branch underway or explicit acceptance that public-alpha ships with the inherited CVEs documented | Acknowledgement note from runtime team owner |

## Day-by-day plan (re-scoped — Mac-substrate-only)

### Day 1 — CVE audit + ownership classification — **DONE (ca7476c + this re-scope)**

- Installed `cargo-audit v0.22.1`. Ran against workspace.
- 34 vulnerabilities + 12 warnings found; **all 34 classified as INHERITED**
  from `main`.
- Handoff doc written for broader team: `RUNTIME_CVE_HANDOFF.md`.
- **Deliverable:** `PHASE_10_DAY_1_NOTES.md` + `RUNTIME_CVE_HANDOFF.md`.

### Day 2-3 — Mac substrate threat model

- Enumerate Mac-specific trust boundaries:
  - Host operator ↔ supervisor process.
  - Supervisor ↔ Apple Vz framework (entitlement boundary).
  - Guest VM ↔ Carrier bridge (virtio-console).
  - Capsule ↔ capsule (must enforce: **not possible directly** — NAT-only network + bridge mediation).
  - Operator ↔ runtime API (local gateway on 127.0.0.1).
  - Upstream component registry ↔ runtime (IPFS pull paths).
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

### Day 11-13 — External security review window (re-scoped)

- Review scope: **new substrate code only** — `elastos/crates/elastos-vz/`
  (~7,724 LOC) + Carrier-bridge framing parser (the trust-boundary code
  we co-own). Not the entire workspace.
- Hand off branch + threat model + `BRANCH_SUMMARY.md` + Day-1 ownership
  classification + Day-8 fuzz findings to the external security reviewer.
- Agent work pauses except for: (1) answering reviewer clarification
  questions, (2) applying agreed remediations, (3) keeping CI green.
- **Deliverable:** `PHASE_10_DAY_13_NOTES.md` — remediation log with each
  finding, decision, and resolution commit.

### Day 14 — Release CI lane

- GitHub Actions workflow `mac-release-smoke.yml` runs on every PR touching
  `elastos-vz/`, `release-mac.sh`, or the entitlements plist.
- Steps: checkout → `release-mac.sh` (signs + notarizes) → drop signed binary
  into a fresh `mac-local-setup` flow → execute the Phase 9 5-test smoke matrix.
- Failure blocks merge.
- **Deliverable:** workflow file checked in and green on a non-trivial PR.

### Day 15 — Sign-off

- Rerun Phase 9's 5-test smoke matrix (regression check).
- Confirm `RUNTIME_CVE_HANDOFF.md` acknowledged by runtime team owner.
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

- **Day 1** — DONE (ca7476c + re-scope commit). CVE audit complete; 34/34
  vulnerabilities classified as INHERITED from `main`; handoff doc written
  for broader team; this plan re-scoped accordingly. Calendar shrunk from
  ~25 days to ~14 days.
- **Day 2-15** — pending operator sign-off to proceed.
