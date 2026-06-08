# Reinstatement Push Plan — local branches → PRs

**Status:** Ready. Execute the moment GitHub push access is restored.
**Date:** 2026-06-08
**Base for every branch:** `origin/0.4.0` (all are clean descendants — verified).

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
| 1 | `fix/crosvm-darwin-build` | 1 | fix(crosvm): compile on non-Linux hosts so 0.4.0 builds/runs on macOS | — |
| 2 | `fix/home-summary-resilience` | 2 | fix(home): reset corrupt browser-state instead of failing the home summary | #1 (stacked) |
| 3 | `chore/bincode-2x` | 1 | chore(runtime): migrate bincode 1.3 → 2.x with wire-format compat tests | — |
| 4 | `chore/carrier-iroh-upgrade` | 1 | docs(carrier): iroh/Hickory upgrade decision memo + correct audit.toml rationale | — |
| 5 | `feat/decrypt-provider-cenc` | 11+ | feat(ddrm): decrypt-provider cenc engine, chain providers proven, rail spec + alignment | — |

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
- Branch is a clean descendant of `origin/0.4.0` (rebase if main has moved).
- PR body: 1–3 bullet summary + the test plan above.

## After Anders' answers land
- **dDRM rail (Option A + tier):** wire `envelope::ecdh_unwrap` + `cenc::process`
  in decrypt-provider; align `decrypt-provider/capsule.json` type per his tier
  call. Adds to PR #5 (or a follow-up).
- **Carrier toolchain:** if he approves MSRV 1.91, convert #4 from a memo into the
  real iroh 1.0 migration on a fresh branch.
