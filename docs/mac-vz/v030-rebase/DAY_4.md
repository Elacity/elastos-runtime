# Day 4 — Mac VZ rebase onto v0.3.0 main (sign-off)

> Branch: `sash/local-test-v030` (PR #3, draft)
> Date: 2026-05-28
> Phase: **Sign-off** — close out the carrier-bridge merge, re-baseline
> the protected-paths gate, walk the full Mac VZ surface, and prepare
> the Anders handoff for v0.3.1 review.

## Goal

Take the Day-3 commit (`f97203f`, supervisor + vm-provider + carrier-
bridge reconciled) the rest of the way:

1. Land the v0.3.0 principal-aware logic into `carrier_bridge.rs` that
   Day 3 deferred — `scope_current_user_alias`,
   `protected_principal_root_carrier_response`,
   `principal_root_read_write_uri`, `rooted_localhost_fs_path`, plus
   the full `carrier_invoke` request-type dispatch (Day 3 still spoke
   the old `provider_call` ABI inherited from the archive).
2. Walk every file Mac VZ touched on the archive and confirm none was
   lost or accidentally Linux-gated during the rebase.
3. Re-baseline `.github/workflows/linux-untouched.yml`'s
   `VZ_BACKEND_BASELINE` to the Day-4 HEAD so future commits are gated
   against "rebase complete" rather than "rebase mid-flight".
4. Run the cross-platform smoke helpers (`scripts/lib/`) on Mac and
   confirm no regression vs the archive's pre-rebase behavior.
5. Sign off.

## What landed

### Day 4 commit 1 — `feat(mac-vz): day 4 — reconcile carrier_bridge.rs v0.3.0 principal-aware logic`

The big one. Day 3 had to defer this because the archive's
`carrier_bridge.rs` predates v0.3.0's `carrier_invoke` ABI migration
and principal-aware localhost-fs scoping; landing it verbatim on top
of v0.3.0 main left the bridge speaking the **old** `provider_call`
request type while the guest in `elastos-guest::runtime` already
speaks the **new** `carrier_invoke` type. Linux CI was green only
because no end-to-end guest↔host bridge test exercises a real
socketpair — the unit tests cover the bridge logic in isolation, and
no test on Day 3 actually fed `carrier_invoke` to the bridge.

Strategy: **invert the Day 3 reconcile**. Take v0.3.0 main's
`carrier_bridge.rs` as the **logic base** and re-layer the Mac VZ
framing/lifecycle additions on top, instead of the archive base + v0.3.0
fields.

v0.3.0 main logic (now present, was missing on the branch after Day 3):

| Symbol | Purpose |
|--------|---------|
| `carrier_invoke` request type | Replaces `provider_call`; matches the guest ABI |
| `carrier_invoke_dispatch` | Parses `{uri, operation, body, token}` into `{scheme, operation, resource, request}` |
| `protected_principal_root_carrier_response` | Intercepts read/write requests for principal-rooted localhost paths and short-circuits the response from the host's protected-storage helpers (no provider round-trip) |
| `principal_root_read_write_uri` | Detects `Users/<principal>/...` storage URIs in request bodies and returns the rooted form |
| `request_content_bytes`, `apply_read_window` | Read/write request helpers |
| `provider_ok_result`, `provider_error_result`, `carrier_error_response` | Response shape helpers |
| `scope_current_user_alias` | Rewrites `Users/self/...` → `Users/<principal>/...` (Phase 8 Day 6) |
| `is_unscoped_current_user_alias` | Sentinel for "principal context required" |
| `provider_scheme_for_carrier_uri` | URI-scheme → provider-scheme routing |
| `wallet_signature_parts_from_uri` | Wallet-resource URI parsing |
| `is_runtime_control_request` | Rejects raw runtime-control surfaces (including the legacy `provider_call`) — the only remaining guest↔host ABI is `carrier_invoke` |

All 24 v0.3.0 unit tests for the above now pass (incl.
`carrier_invoke_dispatch_*`, `handle_request_rejects_old_provider_call_shape`,
`handle_request_uses_protected_principal_root_object_for_users_self_writes`).

Mac VZ surface preserved on top:

- `CARRIER_MAX_LINE_BYTES` constant + `CarrierFrameError` enum +
  `parse_carrier_line` pub fn for the fuzz harness (Phase 10 Day 4-8).
- `read_line_byte_budgeted` + `drain_to_newline` byte-budgeted line
  reader (Phase 10.5 M1).
- `BridgeContext.on_terminate: Option<Arc<Notify>>` field (Phase 4 Day
  6).
- `spawn_carrier_bridge_on_stream` socketpair entry point (Phase 3 Day
  4).
- Shared `run_carrier_bridge_loop` extracted from the path-based
  bind/accept flow so both Linux and Mac use the same dispatch loop.
- `on_terminate.notify_waiters()` fires on every loop exit (EOF, read
  error, write error, oversized-line teardown, accept failure).

### Day 4 commit 2 — `chore(mac-vz): day 4 — restore carrier-bridge fuzz harness lost in rebase`

Walk-through against `archive/sash/local-test` revealed the
carrier-bridge fuzz harness was the only Mac VZ surface that didn't
make it through. Re-staged verbatim:

- `elastos-server/fuzz/.gitignore`
- `elastos-server/fuzz/Cargo.toml` (declares its own `[workspace]` —
  uses nightly for `libfuzzer-sys` without disturbing the parent
  stable toolchain)
- `elastos-server/fuzz/dict/carrier_bridge_framing.dict`
- `elastos-server/fuzz/fuzz_targets/carrier_bridge_framing.rs`
- `elastos-server/fuzz/corpus/carrier_bridge_framing/*.{json,empty,
  spaces,blank-lines,oversized,truncated-utf8,...}` (24 seed inputs)

The harness consumes the public surface preserved on Day 4's
`carrier_bridge.rs` reconcile (`parse_carrier_line`,
`CarrierFrameError`, `CARRIER_MAX_LINE_BYTES`). Asserts the framing
parser never panics, oversized inputs short-circuit with
`LineTooLarge`, and every byte slice yields `Ok` or `Err`. Runnable
locally via `cargo +nightly fuzz run carrier_bridge_framing`.

### Day 4 commit 3 — `chore(ci): re-baseline linux-untouched gate to Day-4 HEAD`

Updated `.github/workflows/linux-untouched.yml`'s
`VZ_BACKEND_BASELINE` from `ded1333` (Day 2) to `65f5f05` (Day 4
fuzz-restore commit, the rebase's final-state HEAD). Day 3 + Day 4
commits only touch `elastos-server` (NOT protected), so the gate
keeps enforcing "no future commit modifies `elastos-crosvm` /
`elastos-runtime` / `elastos-common` / `elastos-compute` beyond what
the rebase already shipped."

## Workspace walk-through

For every file Mac VZ touched in the archive (`5b90ea8` →
`sash/local-test`), confirmed presence + content on
`sash/local-test-v030`:

- **86 paths** touched in the archive (60 once we exclude the
  fuzz-corpus binary blobs).
- **0 source files lost** in the rebase (the only initial gap was
  the fuzz harness, fixed in Day 4 commit 2).
- **`elastos-vz/` (26 files)**: all preserved. Three diffs vs the
  archive: `provider.rs` / `vm.rs` / `config.rs` each got a single
  `+ authority: None` line in their in-module unit-test
  `CapsuleManifest` initializer to match v0.3.0's schema bump. Pure
  schema-bump, no Mac-substrate change.
- **`elastos-server/` Mac VZ surfaces**: `doctor_cmd.rs`,
  `vm_debug_cmd.rs`, `overlay_initrd.rs` all present.
  `supervisor.rs`, `vm_provider.rs`, `setup.rs`, `runtime_control.rs`,
  `home_cmd.rs`, `run_cmd.rs`, `main.rs` all reconciled with both
  Mac VZ work and v0.3.0 changes layered.
- **Tests (8 files)**: `capability_concurrency`, `common/mod`,
  `vz_chat_interop_smoke`, `vz_home_frontdoor_smoke`,
  `vz_perf_harness`, `vz_shutdown_semantics`, `vz_supervisor_smoke`,
  `vz_supervisor_startup_orphan_cleanup` all present, all linking
  against the rebased symbols, all updated for v0.3.0's `authority:
  None` / `principal_id: None` / `data_dir: None` schema additions.

## Local validation

```text
$ cargo check --workspace --tests
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.20s

$ cargo clippy --workspace --tests -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.27s

$ cargo fmt --all -- --check
(silent — clean)

$ cargo test -p elastos-server --lib carrier_bridge::
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 630 filtered out

$ cargo test -p elastos-server --lib supervisor::
test result: ok. 60 passed; 0 failed; 0 ignored; 0 measured; 594 filtered out

$ cargo test -p elastos-vz --lib
test result: ok. 108 passed; 0 failed; 0 ignored; 0 measured

$ cargo test -p elastos-server --lib
test result: FAILED. 646 passed; 6 failed; 2 ignored; 0 measured
  — same 6 pre-existing Mac SUN_LEN failures documented in DAY_3.md;
    no Day 4 regressions.

$ cargo test --workspace --tests --no-fail-fast
  — every package green except:
    1. elastos-server::gateway_browser_route_tests::test_browser_*
       (6, all SUN_LEN)
    2. elastos-vz::concurrent_launch::{single_vm_boots_to_userspace,
       concurrent_load_with_real_kernel} — pre-existing, requires
       Apple `com.apple.security.virtualization` entitlement (cargo
       test binaries are unsigned by default; needs codesigning via
       scripts/dev/sign-elastos-vz/, see docs/MAC.md). Not a rebase
       regression — same failures on archive/sash/local-test.

$ bash scripts/lib/cross-platform-test.sh
cross-platform.sh: 47 passed, 0 failed

$ bash scripts/lib/runtime-cleanup-test.sh
runtime-cleanup.sh: 5 passed, 0 failed
```

## CI signal

| Check | Day 3 | Day 4 (expected) |
|-------|-------|------------------|
| `Linux-untouched gate (Vz backend)` | green | green (gate re-baselined) |
| `CI` (Linux build + tests) | green | green (Day 4 only touches non-protected `elastos-server` + new fuzz crate that is its own workspace, untouched by the parent build) |
| `Mac Vz CI (Phase 5+ Apple Silicon)` | 6 SUN_LEN failures | 6 SUN_LEN failures (no new regressions, no fixes — out of scope) |

## Known issues (carried forward, **NOT** rebase regressions)

1. **6 Mac CI gateway_browser SUN_LEN failures.** Documented in
   DAY_3.md with a concrete diagnostic. v0.3.0 main builds the runtime
   stream socket under `std::env::temp_dir().join(
   "elastos-browser-streams")`, which on macOS resolves under the
   per-user `/var/folders/<XX>/<YY>/T/...` private temp tree. The
   absolute path exceeds Darwin's 104-byte `sun_path` limit. **Fix
   would** shorten the socket path on Darwin (e.g. hashed-filename
   truncation or an env-var override). Out of scope for the rebase.
2. **`components.json` lacks `darwin-arm64`.** v0.3.0 main's
   `components.json` has no Mac platforms declared for any capsule.
   `bash scripts/lib/components-json-verify.sh` fails on the rebased
   branch with `[Class A] external.shell.platforms missing keys:
   ['darwin-arm64']` (and ~30 similar). The archive had partial Mac
   integration here; v0.3.0 main removed it. Project-level capsule
   release pipeline work, not rebase work.
3. **Two `elastos-vz` real-kernel boot tests** require the Apple
   `com.apple.security.virtualization` entitlement and only pass on a
   codesigned dev build. Standard pre-condition for running real
   `Vz` from an unsigned `cargo test` binary; ignored locally
   today, will pass once `scripts/dev/sign-elastos-vz/` is part of
   the dev loop.

## What does **not** carry forward

The Day 1–3 deferred lists are now empty:

- ~~`carrier_bridge.rs` v0.3.0 principal-aware logic merge.~~ **Done
  Day 4 commit 1.**
- ~~Workspace walk-through.~~ **Done — only the fuzz harness was
  missing.**
- ~~Re-baseline `linux-untouched.yml`.~~ **Done — `65f5f05`.**
- ~~Cross-platform smoke run.~~ **Done — helpers green.**

## Next

- Compose the Anders message for v0.3.1 review handoff
  (`docs/vz-backend/V030_MESSAGE_DRAFT.md`). This is the final Day 4
  task before sign-off.
- Push Day 4 commits to origin, confirm CI mirrors the local pattern
  (Linux green, Mac CI 6 SUN_LEN failures).
- PR #3 stays draft until Anders confirms the rebase landing strategy
  for v0.3.1.
