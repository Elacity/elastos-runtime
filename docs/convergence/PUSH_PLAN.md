# Reinstatement Push Plan — local branches → PRs

**Status:** Ready. Execute the moment GitHub push access is restored.
**Date:** 2026-06-09 (rebase measured against the RELEASED `v0.4.0` on Day 44)
**Base for every branch:** `v0.4.0` (`cae83c3c3`) — 0.4.0 is now released + tagged.

> ## ✅ 0.4.0 RELEASED + tagged `v0.4.0` (`cae83c3c3`) — alignment verified Day 44
>
> **0.4.0 has shipped** (tag `v0.4.0` = `cae83c3c3`; `origin/0.4.0` force-updated
> `67b7560a7` → `cae83c3c3`). It stopped moving — the rebase is now unblocked. The
> Day-44 alignment audit found:
>
> **The contract held byte-identical to what shipped.**
> `elastos-common/protected_content.rs` is **byte-identical** between
> `feat/decrypt-provider-cenc` and the released `v0.4.0`
> (`git diff feat/decrypt-provider-cenc:…/protected_content.rs v0.4.0:…` = 0 lines),
> and `scripts/ddrm-drift-check.sh` **PASSES against the released base** (13 consts /
> 10 structs / 1 fn / 10 fields). Our entire chain was built against exactly the
> contract that shipped.
>
> **Crown jewel validated green ON the released base (Day 44, content-overlay in a
> throwaway worktree at `v0.4.0`):** drift-check PASS; `decrypt-provider` `harden`=65
> + `pq-mldsa-hybrid`=37; `encrypt-provider`=13; `pc2-conformance.sh` byte-compatible.
> So our crypto core rebases cleanly onto `v0.4.0`.
>
> **Released v0.4.0 ships the providers as fail-closed SKELETONS** (`decrypt-provider`
> `open_session`/`render` return `not_configured` — no CEK rail). The rail decision
> (`DDRM_DECRYPT_RAIL.md` Q1/Q2/Q3) is **still the one true blocker**; our branch has
> the full in-VM engine + both Q2 answers pre-proven, ready to drop in behind it.
>
> **Measured rebase conflict surface** (real `git rebase --onto v0.4.0 42e4d7ffd`,
> Day 44 — note the cut point is `42e4d7ffd`, the parent of our first convergence
> commit, so Anders' shared `marketplace`/`library` commits are NOT replayed):
> - `decrypt-provider/src/main.rs` — **clean reconcile**: our engine replaces the
>   skeleton (resolve by taking our commit's content; `cenc/*` are new files).
> - `key-provider/src/main.rs` **+** `drm-provider/src/main.rs` — **genuine 3-way**:
>   WE evolved them (rights-receipt binding, WASI/test bar) AND Anders independently
>   refactored them in `v0.4.0` (key +67/−37, drm trimmed). **Needs Anders' intent**
>   to merge cleanly (see "for Anders" below).
> - `encrypt-provider/**` — **no conflict**: absent in `v0.4.0`, purely our new capsule.
> - `rights-provider` / `availability-provider` — we didn't touch `src` → **adopt
>   Anders' v0.4.0 versions**.
>
> **For Anders:** contract aligned perfectly — thank you. Two asks before we land the
> rebase: (1) your `v0.4.0` `key-provider` + `drm-provider` refactors overlap our
> changes — can you confirm intent / whether our versions supersede? (2) the
> decrypt rail (Q1/Q2/Q3 in `DDRM_DECRYPT_RAIL.md`) is the last blocker; our side has
> both signature answers (ML-DSA-65 + hybrid ECDSA+ML-DSA) pre-proven and the engines
> PC2-conformance-verified — ready to wire the day you pick the transport.

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
# Cut point = parent of our FIRST convergence commit, so Anders' shared
# marketplace/library commits are not replayed onto v0.4.0 (it already has them).
# For feat/decrypt-provider-cenc that is 42e4d7ffd (measured Day 44). For the other
# branches use: CUT="$(git merge-base v0.4.0 "$B")" if they share no Anders commits.
git rebase --onto v0.4.0 42e4d7ffd "$B"
# ...resolve conflicts (see churn points below), then:
scripts/ddrm-verify.sh                 # for the dDRM branch: must be ALL GATES PASS
#   (other branches: cargo build/test for the crate they touch — see per-PR plan)
git range-diff v0.4.0...@{-1} v0.4.0...HEAD               # confirm nothing dropped
```

**Branch order & expected conflict surface** (cross-checked against git Day 36):

| Order | Branch | ahead | conflict surface on rebase |
|---|---|---|---|
| 1 | `fix/crosvm-darwin-build` | 3 | none expected (platform-gating new files); **build-verified green on macOS Day 42** |
| 2 | `fix/home-summary-resilience` | 4 | stacked on #1 — rebase #1 first, then this onto it; **build-verified macOS Day 43** (own tests green; 4 microVM-launch tests are no-KVM env failures, pass on Linux CI) |
| 3 | `chore/bincode-2x` | 3 | **bincode call-sites** if the base touched serialization; keep `bincode::config::legacy()`, re-run the wire-format golden; **build-verified green macOS Day 43 (311 passed, byte-identity golden green)** |
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
- **Verified green on macOS (Day 42, Darwin 25.4.0 arm64):** `cargo test -p
  elastos-crosvm` = **18 passed / 0 failed** (incl. the fail-closed stub tests),
  the `elastos-crosvm` crate compiles **warning-free**, and `cargo build -p
  elastos-server` finishes clean. Branch is build-verified, not just authored —
  ready to push as-is once GitHub access returns.

### #2 `fix/home-summary-resilience`
- **What:** a corrupt/stale `browser-state.json` (cosmetic UI state) resets to
  default instead of fail-closing the whole Home summary (which blocked login).
- **Why:** non-authority UI convenience data must never lock a principal out of
  their desktop. Observed in the wild (passkey sign-in 500: trailing bytes after
  valid JSON from a non-atomic external writer).
- **Test plan:** passkey sign-in succeeds with a deliberately corrupted
  `browser-state.json`; warning logged; default state returned.
- **Verified on macOS (Day 43):** `cargo build -p elastos-server` clean; the
  branch's own `home_browser_state_*` tests pass (incl.
  `test_home_browser_state_resets_plaintext_for_protected_principal_root` — the
  reset-to-default path this fix generalizes). **Caveat:** 4 `home_launch` /
  `runtime_ensure` tests fail on macOS (`assert running==true`) — they require a
  live KVM microVM and fail **identically on `fix/crosvm-darwin-build`** (which
  lacks this fix), so they are an **environmental (no-KVM) limitation, not a
  regression**; they pass on Linux CI. Follow-up (own branch, not this one):
  `cfg(target_os="linux")`-gate the microVM-launch home tests so
  `cargo test -p elastos-server` is green on macOS dev machines too.
- **Quality note:** the exact *trailing-bytes-after-valid-JSON* parse-reset path
  (serde error → default) is covered by behavior parity with the tested
  unencrypted-reset path but has no *dedicated* unit test; deliberately NOT added
  here to keep this minimal reviewed fix unchanged before push (tracked as a
  follow-up).

### #3 `chore/bincode-2x`
- **What:** bincode 1.3 → 2.x using `bincode::config::legacy()` for capability
  tokens; golden + round-trip tests prove byte-identical wire format.
- **Why:** security debt Anders flagged; do it with explicit versioning, not a
  silent wire-format change.
- **Test plan:** `cargo test -p elastos-runtime` green; golden test asserts the
  1.3-era bytes decode and re-encode identically under 2.x.
- **Verified green on macOS (Day 43, Darwin 25.4.0 arm64):** full
  `cargo test -p elastos-runtime` = **311 passed / 0 failed**, including the two
  safety-critical tests — `token_wire_format_is_bincode_1x_legacy` (byte-for-byte
  pin to captured 1.3 output) and `token_round_trips_through_bincode_2x`. The
  capability-token wire format is provably **unchanged** by the 2.x migration —
  the one branch where silent serialization drift would have broken tokens. No
  churn risk found; ready to push as-is.

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
