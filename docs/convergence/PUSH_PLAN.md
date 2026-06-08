# Reinstatement Push Plan — local branches → PRs

**Status:** Ready. Execute the moment GitHub push access is restored.
**Date:** 2026-06-09 (rebase recipe verified against git reality on Day 36)
**Base for every branch:** `origin/0.4.0`.

> ## ⚠️ Base moved — rebase before pushing (recipe is button-press below)
>
> Anders **force-pushed `origin/0.4.0`** (`42e4d7ffd` → `67b7560a7`), redoing
> commits as warned. Our branch has therefore **diverged** from `origin/0.4.0`
> (verified Day 36: `git merge-base --is-ancestor origin/0.4.0 feat/decrypt-provider-cenc`
> → **not an ancestor**; merge-base is `589092b95`, with **3** base commits since).
> Do **not** rebase until 0.4.0 stops moving — then run §"Rebase recipe" below.
>
> **The contract converged — still zero type drift (re-verified Day 36).**
> `elastos-common/protected_content.rs` is **byte-identical** between
> `feat/decrypt-provider-cenc` and `origin/0.4.0`
> (`git diff origin/0.4.0..feat/decrypt-provider-cenc -- …/protected_content.rs` = 0
> lines). The redone base independently added the exact types our providers were
> built against (`RightsDecisionReceiptV1`, `KeyReleaseRequestV1.rights_receipt`,
> typed `DecryptSessionRequestV1.release_receipt`, `ReleaseReceiptV1.session_id/action`),
> plus the PQ-negotiation surface (`KeyEnvelopeAlgorithmsV1`,
> `validate_protected_content_key_envelope_algorithms`, the `DEFAULT_*` algorithm
> sets). `scripts/ddrm-drift-check.sh` now pins **all** of these (13 consts / 10
> structs / 1 fn / 10 fields), so the rebase is a button-press verification, not an
> archaeology dig.

## Rebase recipe (run when 0.4.0 settles)

**Pre-flight (once):**
```bash
git fetch origin 0.4.0
scripts/ddrm-verify.sh                 # gate must be GREEN on the current tip first
```

**Per branch** — rebase onto the fresh `origin/0.4.0`, then re-verify. Because the
base was force-pushed, use `--onto` with the *current* merge-base (not a hard-coded
parent), so only our own commits replay:

```bash
B=feat/decrypt-provider-cenc           # repeat for each branch in the push order
git branch -f "backup/${B##*/}-prerebase" "$B"          # safety snapshot
git rebase --onto origin/0.4.0 "$(git merge-base origin/0.4.0 "$B")" "$B"
# ...resolve conflicts (see churn points below), then:
scripts/ddrm-verify.sh                 # for the dDRM branch: must be ALL GATES PASS
#   (other branches: cargo build/test for the crate they touch — see per-PR plan)
git range-diff origin/0.4.0...@{-1} origin/0.4.0...HEAD   # confirm nothing dropped
```

**Branch order & expected conflict surface** (cross-checked against git Day 36):

| Order | Branch | ahead | conflict surface on rebase |
|---|---|---|---|
| 1 | `fix/crosvm-darwin-build` | 3 | none expected (platform-gating new files) |
| 2 | `fix/home-summary-resilience` | 4 | stacked on #1 — rebase #1 first, then this onto it |
| 3 | `chore/bincode-2x` | 3 | **bincode call-sites** if the base touched serialization; keep `bincode::config::legacy()`, re-run the wire-format golden |
| 4 | `chore/carrier-iroh-upgrade` | 3 | docs/audit.toml only — none expected |
| 5 | `feat/decrypt-provider-cenc` | 39 | `capsules/{decrypt,key,drm}-provider/src/main.rs` only — see below |

**Known churn points (resolution = "keep both", no type reconciliation needed):**
- **dDRM providers** (`capsules/{decrypt,key,drm,rights}-provider`): conflicts arise
  only because the base lacks *our additions* (cenc/envelope/rights-binding/seam/
  consumer contract). Take the base's structure + re-apply our additions. The
  contract types are identical, so there is **no type reconciliation** — confirm with
  `scripts/ddrm-drift-check.sh` (PASS) immediately after resolving.
- **`encrypt-provider` → `elastos-common`:** reconciled on Day 39 — its sealed
  **output** now uses the shared `SealedObjectV1`/`KeyEnvelopeV1`, so on rebase it
  shares the same contract-conflict surface as the other providers (resolve "keep
  both", then `ddrm-drift-check.sh` PASS). Its **input** `SealRequest` stays local
  (no shared seal-request type), so that file region won't conflict on type grounds.
- **bincode 2.x:** if the new base changed any capability-token serialization, keep
  the `legacy()` config and re-run the round-trip golden before pushing.

A safety backup of an early pre-rebase tip is kept at
`backup/decrypt-provider-cenc-preD17`; each rebase also snapshots
`backup/<branch>-prerebase` per the recipe above.

While GitHub access is suspended, all work has been committed to isolated local
branches, each scoped to one reviewable concern. This is the exact order and
shape to land them as small PRs without re-thinking. No branch depends on the
network; each pushes with `git push -u origin <branch>`.

## Push order & PR mapping

Order is chosen so the macOS build fix lands first (it unblocks building/running
0.4.0 on macOS, which the other branches benefit from), then independent hygiene,
then the larger dDRM feature.

| # | Branch | Ahead | PR title | Depends on |
|---|---|---|---|---|
| 1 | `fix/crosvm-darwin-build` | 3 | fix(crosvm): compile on non-Linux hosts so 0.4.0 builds/runs on macOS | — |
| 2 | `fix/home-summary-resilience` | 4 | fix(home): reset corrupt browser-state instead of failing the home summary | #1 (stacked) |
| 3 | `chore/bincode-2x` | 3 | chore(runtime): migrate bincode 1.3 → 2.x with wire-format compat tests | — |
| 4 | `chore/carrier-iroh-upgrade` | 3 | docs(carrier): iroh/Hickory upgrade decision memo + correct audit.toml rationale | — |
| 5 | `feat/decrypt-provider-cenc` | 39 | feat(ddrm): decrypt-provider cenc engine, chain providers proven, rail spec + alignment | — |

> Ahead-counts re-measured against the force-pushed `origin/0.4.0` on Day 36
> (`git rev-list --count origin/0.4.0..<branch>`); they include the divergence from
> the rewritten base and will collapse to the intended-commit count after rebase.

Notes:
- **#2 is stacked on #1** (it contains the crosvm commit). Either land #1 first
  then rebase #2 onto main, or open #2 against #1's branch. Same commit hash, so
  it merges cleanly.
- **#4 is documentation-only** (ADR + audit.toml comment); the two Hickory CVEs
  stay scoped-ignored pending the toolchain-floor decision. Safe to land anytime.
- **#5 is the big one** — split is optional (see below).

## Per-PR summary & test plan

### #1 `fix/crosvm-darwin-build`
- **What:** `cfg(target_os = "linux")`-gate the TAP/`network` module; add
  `network_stub.rs` that fails closed off-Linux; gate the `mkfs.ext4` test.
- **Why:** lets `elastos-server` build/run on macOS for local dev; no behaviour
  change on Linux (production microVM networking path unchanged).
- **Test plan:** Linux CI green (no functional delta). macOS: `cargo build -p
  elastos-server` succeeds; `elastos gateway` serves Home at `localhost:8090`.

### #2 `fix/home-summary-resilience`
- **What:** a corrupt/stale `browser-state.json` (cosmetic UI state) resets to
  default instead of fail-closing the whole Home summary (which blocked login).
- **Why:** non-authority UI convenience data must never lock a principal out of
  their desktop. Observed in the wild (passkey sign-in 500: trailing bytes after
  valid JSON from a non-atomic external writer).
- **Test plan:** passkey sign-in succeeds with a deliberately corrupted
  `browser-state.json`; warning logged; default state returned.

### #3 `chore/bincode-2x`
- **What:** bincode 1.3 → 2.x using `bincode::config::legacy()` for capability
  tokens; golden + round-trip tests prove byte-identical wire format.
- **Why:** security debt Anders flagged; do it with explicit versioning, not a
  silent wire-format change.
- **Test plan:** `cargo test -p elastos-runtime` green; golden test asserts the
  1.3-era bytes decode and re-encode identically under 2.x.

### #4 `chore/carrier-iroh-upgrade`
- **What:** decision memo (`CARRIER_IROH_UPGRADE.md`) + corrected `audit.toml`
  rationale. No dependency change.
- **Why:** closing both Hickory advisories needs hickory ≥ 0.26.1 → iroh 1.0-rc
  (MSRV 1.91 > pinned 1.89). That is an operator toolchain-floor decision; this PR
  records the evidence and keeps `cargo audit` green via visible ignores.
- **Test plan:** `cargo audit` green (ignores documented); no build delta.

### #5 `feat/decrypt-provider-cenc`
- **What:** vendored `cenc` decrypt engine; the four dDRM providers
  (drm/rights/key/decrypt) brought to a wasm-built, WASI-smoke-proven, fail-closed
  bar; cross-provider contract-seam tests; the ECDH envelope rail captured as a
  tested spec (`envelope.rs`); status + alignment docs.
- **Why:** the dDRM crown jewel, contract-first and ahead of Anders' mainline
  sequence so it is ready when he opens the track.
- **Test plan:** `cargo test` green per provider; `scripts/ddrm-chain-smoke.sh`
  all four providers PASS under wasmtime. See `DDRM_STATUS.md`.
- **Optional split** (if Anders prefers smaller units): (5a) decrypt-provider +
  cenc + envelope spec; (5b) key/rights/drm provider hardening + seam tests;
  (5c) smoke runner + docs.

## Pre-push checklist (per branch)
- `git log --oneline origin/0.4.0..<branch>` shows only the intended commits.
- No secrets / no `build/` or `scripts/dev/` local artifacts staged.
- Branch is a clean descendant of `origin/0.4.0` (rebase per the recipe if it moved;
  `git merge-base --is-ancestor origin/0.4.0 <branch>` should succeed post-rebase).
- For `feat/decrypt-provider-cenc`: `scripts/ddrm-verify.sh` = ALL GATES PASS.
- PR body: 1–3 bullet summary + the test plan above.

## After Anders' answers land
- **dDRM rail (Option A + tier):** wire `envelope::ecdh_unwrap` + `cenc::process`
  in decrypt-provider; align `decrypt-provider/capsule.json` type per his tier
  call. Adds to PR #5 (or a follow-up).
- **Carrier toolchain:** if he approves MSRV 1.91, convert #4 from a memo into the
  real iroh 1.0 migration on a fresh branch.
