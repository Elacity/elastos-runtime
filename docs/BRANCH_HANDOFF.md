# Branch handoff — `claude/branch-deep-audit-yiez86`

Hand-off note for merging this branch into `feat/ddrm-hardening-and-creator-parity` and
validating it live. Written at `5080468`. **Base:** `feat/ddrm-hardening-and-creator-parity`
(`1677023`) — branch is **22 ahead, 0 behind** the base, so the merge is a clean fast-forward
(no rebase needed unless the base has moved since).

## What this branch is

Deep-audit follow-through: closes reachable resource/replay/parser-DoS gaps (all **fail-closed**),
behavior-preserving perf, a real release profile, and build-visible audit ratchets. **+1.3k / −120
over 21 files, 13 new tests.** Nothing loosens a security boundary; every change either tightens a
fail-closed path or is provably byte-identical.

## Verification state — what's GREEN here vs. what to run LIVE

These ran clean in the dev container (the code-correctness half of `just verify-ci`):

| Gate | Result |
|---|---|
| `just alignment-check` (contract-drift, fail-closed) | ✅ OK |
| `cargo fmt --all -- --check` (workspace) | ✅ clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | ✅ clean |
| `cargo test --workspace` | ✅ **1503 passed, 0 failed**, 15 ignored |
| `just verify-capsules` (decrypt-provider / ddrm-envelope / media-authority / AV weld) | ✅ 146 / 76 / 15 / PASS |

**Run these LIVE (need a real box — not run here):** `just command-smoke` · `just
candidate-command-audit` (release-binary install audit) · `just home-frontdoor-smoke` ·
`just local-carrier-setup-smoke` (needs Carrier artifact access). Then the full `just verify`
on the merged tree.

## The four behavior-changing commits (focus the live review here)

The rest are perf (byte-identical) / docs / build. These four change runtime behavior — all
reviewed adversarially and found sound:

- **M3** (`fea728b`) — reject `trun/senc sample_count > 1<<20` **before** any allocation/loop;
  bound is ~1000× real fMP4 fragments. Pin: `decrypt-provider::cenc::mp4box::alloc_bound_tests`.
- **A7** (`edb02ec`) — a quorum **retry** sends a fresh per-request nonce re-signed by the cached
  session key (same wallet delegation/anchor), so the node's single-use replay guard doesn't reject
  a legit retry; fail-soft to the original. Wired at `viewer_open.rs:1173` + `:1399`. Pin:
  `access_grant::attempt_tests::retry_uses_freshly_regenerated_grant`.
- **B1** (`253d099` + `ce2bd96`) — WASM provider now enforces each capsule's **declared**
  `memory_mb` (clamped to `ELASTOS_WASM_MEMORY_CEILING_MB`, 8 GiB default) + table/instance caps,
  matching the crosvm path. Pin: `wasm::tests::declared_memory_is_honored_and_clamped_to_ceiling`,
  `over_budget_capsule_fails_closed_not_host_exhaustion`.
- **B2a** (`8bf1d29`) — engine runs with epoch interruption; `stop()` sets a per-instance
  `should_stop` + bumps the epoch so a runaway traps on its next backedge (a legit capsule with no
  stop signal runs untouched; the per-store flag prevents false-killing sibling capsules on a shared
  engine). Pin: `wasm::tests::runaway_capsule_is_terminable_via_stop_signal`.

## Audit ratchets (scope-out evidence for the external firm)

`elastos/crates/elastos-server/tests/ddrm_verdicts.rs` (11 verdicts, each pinned to a
test/CI-job/structural reason) · `elastos/crates/elastos-runtime/tests/capability_conformance.rs`
(`KNOWN_GAPS`) · `docs/PRE_AUDIT.md` (7/8 resolved) · `docs/AUDITOR_PACKET.md`. These let an
external review confirm the settled findings in minutes and bill for the hard crypto.

## Known follow-ups (NOT in this branch, by decision)

- **B2b — automatic no-progress kill:** deferred pending a policy call (long-running service vs
  one-shot command; `max_runtime`/`cpu_shares` semantics). B2a gives the operator a kill switch
  today; B2b would make it autonomous. See `docs/ROADMAP_TO_10.md`.
- **Per-launch user/policy memory *grant*** above the manifest declaration (lower priority).
- The roadmap's Decision ① (external crypto audit), ② (coordinated node redeploy), ③
  (permissionless track) are off-branch money/ops/protocol work.

## Suggested merge sequence

1. `git fetch && git checkout feat/ddrm-hardening-and-creator-parity && git merge --ff-only
   claude/branch-deep-audit-yiez86` (fast-forward; resolve nothing if base is unchanged).
2. Run full `just verify` on the merged tree + the LIVE smokes above.
3. Live-exercise on the real box: a WASM capsule open (B1 memory cap visible), a `stop()` on a
   busy capsule (B2a terminates it), and a dDRM open with a forced attempt-2 retry (A7).
