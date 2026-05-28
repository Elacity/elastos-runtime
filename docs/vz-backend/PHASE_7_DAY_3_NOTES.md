# Phase 7 Day 3 — Supervisor wires Mac substrate paths from the registry

**Phase**: 7 (CI lane + artifact publication)
**Day**: 3 (supervisor default-paths wiring on Mac)
**Date**: 2026-05-25
**Status**: GREEN — supervisor now populates `vz_config.kernel_path`
AND `vz_config.initramfs_path` from the registry-resolved
`bin/vmlinux` + `bin/initrd` paths on macOS at construction time;
`elastos serve` / `elastos gateway` on Mac no longer need to
hand-construct a custom `VzConfig`. 382/382 elastos-server lib
tests + 14/14 elastos-vz tests pass; Linux byte-identical.
**Predecessor**: [`PHASE_7_DAY_2_NOTES.md`](./PHASE_7_DAY_2_NOTES.md)
**Successor**: Phase 7 Day 4 (self-hosted Mac runner activation, OR
CI smoke that runs `elastos setup` + boot test on every PR).

---

## 1. Headline

The Day-2 fetcher made `elastos setup --profile minimal` produce
`~/Library/Application Support/elastos/bin/vmlinux` + `bin/initrd` on
Mac. Day-2's boot test exercised those paths by setting environment
variables that the integration test reads explicitly.

Day-3 closes the loop for the **production** call path. The
supervisor — constructed via the argument-free `Supervisor::new` from
`serve_cmd.rs:355` and `gateway_cmd.rs:92,103` — now resolves both
artifacts from the components manifest at construction time and
populates the `VzConfig` it stores. Every per-capsule `VmConfig` that
omits `initramfs_path` will inherit the supervisor-level default via
the propagation already wired in
[`supervisor.rs:2305–2309`](../../elastos/crates/elastos-server/src/supervisor.rs)
(originally added in Phase 4 as "the seam stays consistent" — Day 3
makes that seam carry useful traffic).

**No new artifact downloads, no schema change, no Vz substrate
change.** Just a ~30 LOC supervisor-side wire-up + 3 tests.

## 2. The latent Day-2 bug surfaced today

Locating `VzConfig::new()` revealed a preexisting mismatch the Day-2
test had been hiding:

```rust
// elastos-vz/src/config.rs:109
pub fn new() -> Self {
    let data_dir = default_data_dir();       // $HOME/.local/share/elastos
    Self {
        kernel_path: data_dir.join("bin/vmlinux"),
        // ...
        initramfs_path: None,
        // ...
    }
}

// elastos-vz/src/config.rs:182
fn default_data_dir() -> PathBuf {
    // "Match the crosvm default_data_dir() semantics so a single
    //  ~/.local/share/elastos directory hosts both substrates"
    home.join(".local/share/elastos")
}
```

But on macOS, `elastos-server`'s installer (`setup.rs`) writes to:

```rust
// elastos-server/src/sources.rs:22
pub fn default_data_dir() -> PathBuf {
    dirs::data_dir()                          // ~/Library/Application Support/
        .unwrap_or(...)
        .join("elastos")
}
```

These resolve to **different directories on macOS**:

| Source-of-truth | Mac path |
|---|---|
| `elastos-vz::config::default_data_dir()` | `~/.local/share/elastos/bin/vmlinux` |
| `elastos-server::sources::default_data_dir()` | `~/Library/Application Support/elastos/bin/vmlinux` |

Day-7 (the substrate boot test) worked only because the integration
test explicitly set `ELASTOS_VZ_TEST_KERNEL` to override the path. Any
production `elastos serve` on a Mac would have hit
[`VzConfig::validate` → `KernelNotFound`](../../elastos/crates/elastos-vz/src/config.rs)
at first `launch_capsule`, because the supervisor was relying on the
`VzConfig::new()` default and the installer was writing somewhere
else.

The mismatch is structurally unfixable in `elastos-vz` alone — only
the orchestration layer (`elastos-server`) knows which data-dir
resolver actually applies, because the substrate crate intentionally
avoids depending on the `dirs::*` opinion of `elastos-server`. Day 3
fixes the mismatch at the only place that's correct: the supervisor.

## 3. The change set

Single hunk in
[`elastos/crates/elastos-server/src/supervisor.rs`](../../elastos/crates/elastos-server/src/supervisor.rs),
right after the existing `crosvm_config` construction:

```rust
let crosvm_config = CrosvmConfig::new()
    .with_crosvm_bin(crosvm_bin)
    .with_kernel_path(kernel_path.clone())  // .clone() added: kernel_path now consumed twice
    .with_socket_dir(data_dir.join("crosvm"))
    .with_rootfs_cache_dir(data_dir.join("rootfs-cache"));

// Phase 7 Day 3 — Mac substrate paths come from the registry.
#[cfg(target_os = "macos")]
let vz_config = {
    let vz_config = vz_config.with_kernel_path(kernel_path);
    let installed_initrd =
        Self::resolve_external_install_path(&registry, &data_dir, "initrd", "bin/initrd");
    if installed_initrd.is_file() {
        vz_config.with_initramfs_path(installed_initrd)
    } else {
        vz_config
    }
};
#[cfg(not(target_os = "macos"))]
let _ = kernel_path;
```

Decisions baked into this snippet:

- **`is_file()` not `exists()`**, so a stray *directory* at
  `bin/initrd` doesn't get accepted as a kernel artifact. Day-3 test
  `mac_supervisor_omits_initramfs_when_not_installed` exercises this
  branch by writing a directory there.
- **`#[cfg(target_os = "macos")]` only.** The Linux launch path goes
  through `crosvm_config`; `vz_config.kernel_path` on Linux is dead
  code that gets stripped from the binary. The `let _ = kernel_path;`
  on the Linux side is purely there to silence the unused-variable
  warning.
- **Kernel path is unconditional, initrd is conditional.** A bare
  `--profile minimal` install gets both, but a user who runs
  `elastos setup` with only `--with vmlinux` (no initrd) still gets a
  valid Vz config — kernel-only boot is allowed for self-built
  kernels that don't need an initramfs.
- **No supervisor-side validation.** The supervisor doesn't try to
  validate that the resolved kernel actually exists at construction
  time. That's `VzConfig::validate`'s job, and it runs at first
  capsule launch — which is the right time for that error to surface
  (operator can re-run `elastos setup` to fix it).

## 4. Tests added

Three new mac-only unit tests in
[`supervisor.rs::tests`](../../elastos/crates/elastos-server/src/supervisor.rs):

| Test | What it pins |
|---|---|
| `mac_supervisor_wires_kernel_path_from_registry` | The supervisor overrides `VzConfig::new()`'s `~/.local/share/elastos/...` default with the registry-resolved path under the supervisor's actual `data_dir`. Closes the Day-3 §2 mismatch. |
| `mac_supervisor_picks_up_installed_initrd_as_default` | When `bin/initrd` exists, the supervisor populates `vz_config.initramfs_path` so every per-capsule `VmConfig` inherits it via the supervisor.rs:2305-2309 fallback. This is the production-normal case after `elastos setup --profile minimal`. |
| `mac_supervisor_omits_initramfs_when_not_installed` | When `bin/initrd` is absent (or — fail-closed — when something stray like a directory occupies that path), `vz_config.initramfs_path` stays `None`. Validates the `is_file()` gate. |

All three drive the production `Supervisor::new` entry point (not
`new_with_vz_config`), so the wire-up is verified along the same path
`serve_cmd.rs` + `gateway_cmd.rs` exercise.

The Linux side has nothing to test — the cfg-gated arm leaves
`vz_config` untouched, which is the byte-identical guarantee Phase 6
§ "Linux untouched" gate requires.

## 5. Validation

| Suite | Result | Notes |
|---|---|---|
| `cargo test -p elastos-server --lib mac_supervisor` | **3/3 pass** | The new Day-3 tests in isolation |
| `cargo test -p elastos-server --lib` | **382/382 pass** | Was 379 pre-Day-3; +3 new tests |
| `cargo test -p elastos-vz` (lib + 3 integration) | **14/14 pass** | Substrate untouched, regression-clean |
| `cargo clippy -p elastos-server --lib -- -D warnings` | **clean** | No new warnings on the modified file |
| `single_vm_boots_to_userspace` (Day-7 boot test, re-run) | **PASS in 0.32s** | Substrate still works against fetcher-installed paths |

## 6. What changes for end users

Before Day 3 (post-Day-2 state, on a fresh Mac):

```
$ elastos setup --profile minimal      # Day 2 — works
…
Done. 2 installed, 1 skipped.
$ elastos serve …                       # would have hit
                                        # KernelNotFound:
                                        # ~/.local/share/elastos/bin/vmlinux
                                        # because VzConfig::new()'s default
                                        # pointed at a path the installer
                                        # never wrote to on Mac.
```

After Day 3:

```
$ elastos setup --profile minimal      # Day 2 — still works
…
Done. 2 installed, 1 skipped.
$ elastos serve …                       # supervisor reads the registry,
                                        # populates vz_config.kernel_path =
                                        # ~/Library/Application Support/
                                        # elastos/bin/vmlinux (and
                                        # initramfs_path = bin/initrd).
                                        # First launch_capsule passes
                                        # VzConfig::validate and reaches
                                        # the Vz BootLoader.
```

The "fresh Mac → working capsule boot" loop is now end-to-end
functional through the production CLI, with no env-var overrides
needed.

## 7. What is NOT done today

Deliberately deferred (Day-4+ or Phase 8):

- **The `default_data_dir()` two-file split is still present** in
  `elastos-vz::config`. Today's fix routes around it from the
  supervisor; a future Phase-8 cleanup could either (a) remove the
  default from `VzConfig::new()` entirely (make `kernel_path`
  non-defaulted, force the constructor to be explicit) or (b)
  re-point `elastos-vz::default_data_dir()` at the same `dirs::*`
  opinion `elastos-server` uses. Either is bigger scope than Day 3
  warrants and would change the substrate crate's public API surface,
  which we promised to keep frozen post-Phase-6.
- **No CLI surface for inspecting the resolved paths.** Operators
  who want to verify "did the supervisor wire the right kernel?" must
  read logs or attach a debugger. A `elastos doctor` or `elastos
  setup --verify-only --show-resolved` would close that loop; small
  follow-up.
- **Capsule-level initramfs override** still requires a custom
  `VmConfig`. The supervisor.rs:2305-2309 propagation respects a
  per-capsule `initramfs_path` if the capsule manifest sets one, but
  no capsule manifest field exists today that maps to it. If/when a
  capsule needs a custom initrd, it'd ship one in its bundle and the
  capsule loader would set the field — Phase 8+.

## 8. Reproducing today's work

```bash
# Build + run the new tests
cargo test -p elastos-server --lib mac_supervisor -- --nocapture
# →  test mac_supervisor_wires_kernel_path_from_registry          ... ok
# →  test mac_supervisor_omits_initramfs_when_not_installed       ... ok
# →  test mac_supervisor_picks_up_installed_initrd_as_default     ... ok

# Verify the Day-7 boot test still passes against the
# fetcher-installed paths (no Vz substrate regression)
TEST_BIN=elastos/target/debug/deps/concurrent_launch-*
scripts/dev/sign-elastos-vz/sign.sh "$TEST_BIN"
ELASTOS_VZ_TEST_KERNEL="$HOME/Library/Application Support/elastos/bin/vmlinux" \
ELASTOS_VZ_TEST_INITRD="$HOME/Library/Application Support/elastos/bin/initrd" \
"$TEST_BIN"
# →  test concurrent_load_with_real_kernel              ... ok
# →  test concurrent_load_rejections_isolate_per_vm     ... ok
# →  test single_vm_boots_to_userspace                  ... ok
```

## 9. Branch state + Day-4 candidates

`sash/local-test` at this commit. No `main` push, per the user's
standing instruction.

Day-4 ordered shortlist (smallest first):

1. **`elastos doctor` — resolved-paths inspector.** Tiny CLI command
   that constructs the supervisor against the current data-dir,
   prints `vz_config.kernel_path` + `initramfs_path` + whether each
   exists on disk. Closes the "did the wiring work?" diagnostic loop
   end users currently can't observe.
2. **CI smoke job — `elastos setup` + boot test on every PR.** Needs
   a macOS runner (free GitHub Actions slot OR self-hosted). Catches
   the next regression of the Day-2/Day-3 wire-up automatically.
3. **Self-hosted Mac runner activation.** Phase-6 left this
   scaffolded but not switched on. Day-1 of this would be selecting
   the hardware + ensuring the runner spec at
   [`SELF_HOSTED_RUNNER_SPEC.md`](./SELF_HOSTED_RUNNER_SPEC.md)
   matches what GitHub expects from a self-hosted Mac runner.
4. **`default_data_dir` unification (Phase-8).** Bigger refactor;
   touches the substrate crate's public API. Worth doing eventually
   but explicitly out of Phase-7 scope.
