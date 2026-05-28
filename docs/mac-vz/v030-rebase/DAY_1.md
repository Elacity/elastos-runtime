# Mac VZ Branch Rebase onto v0.3.0 — Day 1 of 4

**Branch**: `sash/local-test-v030`
**Base**: `chore/runtime-cve-hygiene-v030` @ `a591fda` (= PR #2 HEAD = v0.3.0 main + CVE fixes)
**Archive of old Mac VZ branch**: tag `archive/local-test-pre-v030-rebase` (pushed to origin)
**Draft PR**: [#3](https://github.com/Elacity/elastos-runtime/pull/3) — marked DO NOT REVIEW until Day 4
**Day 1 result**: zero new commits on top of PR #2's HEAD; CI baseline only

---

## TL;DR

Day 1 of the Mac VZ rebase is **deliberately a no-op for code**. It's the
same trick we used for the CVE rebase: spend the first day setting up
infrastructure (new branch, archive tag, draft PR, CI baseline) before
any reconciliation work starts. This means Day 1's diff against the
target base is **empty**, and CI on the Day 1 push must match PR #2's
green CI exactly. If it doesn't, something is wrong with the baseline
itself.

| Metric | Today |
|---|---|
| Files changed (vs PR #2 HEAD) | 0 |
| Rust source edited | 0 lines |
| New commits | 0 |
| `cargo audit` | 3 vulns / 4 warnings (inherited from PR #2) |
| Mac VZ work ported | 0% (starts on Day 2) |
| Linux CI checks | pending (should match PR #2) |

---

## Why we chose this base

Three options were considered:

1. **Branch off `chore/runtime-cve-hygiene-v030` (PR #2's branch).** ← chosen.
   Mac branch inherits the CVE fixes as baseline. End state: Mac branch is
   simultaneously v0.3.0-current AND CVE-clean (3 vulns / 4 warnings). No
   re-doing of the CVE work. If Anders looks at `sash/local-test-v030`
   and `cargo audits` it, the numbers match what PR #2 shows.
2. **Branch off `origin/main` directly.** Cleaner reviewable diff (Mac VZ
   only) but Anders would see audit regression vs PR #2 — defeats the
   point of doing the CVE work first.
3. **Wait for PR #2 to merge to main first.** Cleanest history but
   blocked on Anders reviewing PR #2.

Option 1 is the only one that doesn't waste work and doesn't block on
Anders' review schedule. Cost: the rebased Mac VZ branch's PR will
contain *both* the CVE work and the Mac VZ work in the diff against
main. Mitigation: explicit framing in PR #3 that PR #2 is the canonical
review surface for the CVE bits, PR #3 is the Mac VZ work *on top of*
those CVE fixes. Documented inline in the PR description.

---

## What's on the new branch right now

Same set of commits as PR #2:

```
a591fda  docs(cve): day 4 — sign-off doc for v0.3.0 rebase, ready for v0.3.1 review
5894621  docs(cve): day 3 rebase notes — rustls-pemfile closed, 3/4 final state
1b87aae  chore(cve): day 3 — axum-server 0.7→0.8 + rustls-pemfile removal (3 vulns / 4 warnings)
... (back through the rest of PR #2 history) ...
8acb72d  fix(ci): satisfy clippy in home realtime tests       ← v0.3.0 main
```

Zero Mac VZ work. That's not a bug — that's the point. Mac VZ work
starts landing on Day 2.

---

## Conflict map (input to Days 2–4)

Discovered from `git diff $(merge-base origin/main sash/local-test)..sash/local-test`:

| File | v0.3.0 churn | Mac VZ churn | Day to reconcile | Severity |
|---|---:|---:|---|---|
| `elastos-server/src/carrier_bridge.rs` | +1002/−135 | +917/−58 | Day 4 | **HARDEST — both sides rewrote heavily** |
| `elastos-server/src/supervisor.rs` | +148/−88 | +3800/−81 | Day 3 | High volume but mostly orthogonal additions |
| `elastos-server/src/vm_provider.rs` | +23/−9 | +544/−1 | Day 2 | Small v0.3.0 delta, large Mac additions |
| `elastos-server/src/runtime.rs` | +27/−8 | +5/0 | Day 2 | Tiny — easy |
| `elastos-server/src/doctor_cmd.rs` | 0 | +758 | Day 2 | None — Mac side only |
| `elastos-server/src/overlay_initrd.rs` | 0 | +555 | Day 2 | None — Mac side only |
| `elastos-server/src/vm_debug_cmd.rs` | 0 | +524 | Day 2 | None — Mac side only |
| `elastos-compute/src/providers/wasm.rs` | +40/−11 | 0 | (already done by PR #2) | None |
| `elastos-vz/**` (new crate) | — | NEW | Day 2 | None |
| `elastos-server/tests/vz_*.rs` | — | NEW | Day 2 | None |
| `scripts/lib/cross-platform*.sh` | — | NEW | Day 2 | None |
| `scripts/release/*.sh` | — | NEW | Day 2 | None |
| `scripts/dev/*` | — | NEW | Day 2 | None |
| `state.md`, `docs/vz-backend/*` | — | NEW | Day 4 (final) | None |

Total Mac VZ side surface: 204 files / +55,816 / −300 lines since the
merge-base.

---

## Plan for Days 2–4

### Day 2: Port the conflict-free pieces (low risk, high volume)

Items here have no v0.3.0 conflict at all (or trivially small ones), so
they're "copy from archive, paste onto new branch" operations:

- Entire `elastos-vz/` crate (new — no conflict possible)
- `elastos-crosvm` cfg-gating for non-Linux hosts (Mac portability fix)
- `doctor_cmd.rs` (+758 Mac side, v0.3.0 untouched)
- `overlay_initrd.rs` (+555 Mac side, v0.3.0 untouched)
- `vm_debug_cmd.rs` (+524 Mac side, new file)
- `vm_provider.rs` (small v0.3.0 delta — apply Mac additions on top)
- `runtime.rs` tiny touch (+5 lines on Mac side)
- All new test files: `tests/vz_shutdown_semantics.rs`, `tests/vz_perf_harness.rs`, `tests/concurrent_launch.rs`
- All new scripts: `scripts/lib/cross-platform*.sh`, `scripts/release/*.sh`, `scripts/dev/*`, `scripts/measure-*.sh`, `scripts/release-mac.sh`
- Single Day 2 commit (or 2–3 logically grouped commits if it's hard to
  reason about as one). Linux CI must stay green; Mac VZ CI will start
  running once Mac source files appear on the branch.

### Day 3: Reconcile `supervisor.rs`

Mostly Mac VZ additions in different code paths from v0.3.0's. Strategy:

1. Copy `supervisor.rs` from `archive/local-test-pre-v030-rebase`.
2. Identify v0.3.0's specific edits (only +148/-88 lines vs the
   merge-base) and layer them on top.
3. Verify the four-direction reconciliation (a) Mac VZ supervisor APIs
   are intact, (b) v0.3.0's Mac-port-irrelevant changes are preserved,
   (c) `vz_stubs::VzConfig` Linux side still has all the with_*
   builders Mac VZ supervisor.rs depends on, (d) tests still pass.
4. Single commit. Linux CI green required.

### Day 4: Reconcile `carrier_bridge.rs` (the hardest day)

v0.3.0 added +1002/-135 lines for **Carrier rooms** (multi-peer
messaging system). Mac VZ added +917/-58 lines for **FIFO carrier
transport** (WASM capsule bridge using FIFOs after the wasmtime 17→36
removed `WasiCtx::insert_file`).

These two changes are **at different layers** of the file — one is
about peer-to-peer routing, the other is about local capsule-host IPC.
They should be composable, not in tension. Strategy:

1. Read both versions carefully to confirm the layering hypothesis.
2. Take v0.3.0's `carrier_bridge.rs` as the base (because PR #2 has
   already validated it on green CI).
3. Identify the specific FIFO-transport additions on the Mac VZ side
   and graft them onto v0.3.0's structure.
4. **Escalate to user if reconciliation isn't converging within ~2
   hours.** This is the day where the plan most likely needs adjustment.

After carrier_bridge.rs lands: write SIGNOFF.md (modeled on PR #2's
SIGNOFF.md), flip PR #3 to Ready for Review, and the Mac VZ branch is
fresh against v0.3.0.

---

## Verification record (Day 1)

Day 1's only verification is "the new branch matches PR #2 HEAD and CI
is green." Documented as it lands:

- Branch HEAD: `a591fda` (= PR #2 HEAD; verified via `git log` after checkout)
- `git diff origin/main..sash/local-test-v030 --stat`: matches PR #2's
  diff exactly (CVE work only, zero Mac VZ work)
- Draft PR #3 opened: ✓
- Linux CI on PR #3: pending — outcome will be appended once it lands

---

## Decisions logged (Day 1)

1. **Branch off PR #2 (Option 1 above), not main directly.** Avoids
   regressing the audit numbers on the rebased Mac VZ branch.
2. **Open draft PR #3 for CI signal**, same `[DRAFT — DO NOT REVIEW]`
   framing as PR #2 used. Gets us the CI gate without inviting review.
3. **Document everything inline in the per-day notes** (as we did for
   the CVE rebase). Same `docs/cve-hygiene/v030-rebase/` pattern but
   under `docs/mac-vz/v030-rebase/` to keep the two efforts cleanly
   separated for future readers.
4. **Don't touch `archive/local-test-pre-v030-rebase`.** It's the
   immutable record of where the v0.2.0-based Mac VZ work ended. Same
   policy as `archive/runtime-cve-hygiene-v020-base`.
