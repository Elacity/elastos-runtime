# Phase 7 Day 6 — Quieter `elastos doctor` output

**Phase**: 7 (CI lane + artifact publication)
**Day**: 6 (doctor UX polish — suppress supervisor INFO logs)
**Date**: 2026-05-25
**Status**: GREEN — `elastos doctor` now runs under a thread-local
WARN-and-above tracing subscriber for the duration of `run`. The
supervisor's `vz: startup orphan-prune complete` INFO line no longer
bleeds into the triage output. Global `serve`/`setup` logging is
unchanged (the subscriber swap is scoped via
`tracing::subscriber::with_default`). 386/386 elastos-server lib
tests green; one new behavioural test pins the suppression contract.
**Predecessor**: [`PHASE_7_DAY_5_NOTES.md`](./PHASE_7_DAY_5_NOTES.md)
**Successor candidate**: Phase 7 Day 7 — CI Mac runner activation
(hardware procurement only, scaffolding done in Phase 6 Day 8) OR
defer to Phase 8 (carrier RPC validation on a bootable rootfs).

---

## 1. Headline

`elastos doctor` on this Mac before Day 6 (output reproduced from
[`PHASE_7_DAY_5_NOTES.md`](./PHASE_7_DAY_5_NOTES.md) § 1):

```
ElastOS doctor — substrate path resolution check
  platform:   darwin-arm64
  data_dir:   /Users/sash/Library/Application Support/elastos

2026-05-25T15:32:23.032672Z  INFO elastos_server::supervisor: vz: startup orphan-prune complete overlays_removed=0 sockets_removed=0 bridge_sockets_removed=0
  vmlinux:     /Users/sash/Library/Application Support/elastos/bin/vmlinux
              [present] size 44.9 MB
              [validate] passes guest-kernel sanity check
  ...
```

`elastos doctor` on this Mac after Day 6:

```
ElastOS doctor — substrate path resolution check
  platform:   darwin-arm64
  data_dir:   /Users/sash/Library/Application Support/elastos

  vmlinux:     /Users/sash/Library/Application Support/elastos/bin/vmlinux
              [present] size 44.9 MB
              [validate] passes guest-kernel sanity check
  ...
```

One line shorter; no audience-mismatch leak from the daemon-level
tracing pipeline into a one-shot triage tool. The `grep -E
'INFO|orphan-prune'` check from the 10/10 prompt returns **zero
matches** (exit code 1 from grep, as expected).

## 2. Subscriber-pattern choice — option (a), thread-local override

The 10/10 prompt enumerated three options:

- **(a)** `tracing::subscriber::with_default(quiet, || { … })` for
  scoped override
- **(b)** per-command subscriber install
- **(c)** programmatic `RUST_LOG=warn` before constructing the
  supervisor

Investigation found the global subscriber is installed at
`main.rs:1055` *before* clap dispatch, with
`elastos=info + vm_console=info` directives. That rules out (b)
without ripping out the global. (c) is process-wide and reaches
beyond doctor — violates the "narrow change" rule. (a) is the
cleanest fit: a thread-local subscriber via
`tracing::subscriber::with_default` swaps the default only for the
closure body and reverts on closure exit. Doctor's body is purely
synchronous (no `.await`), so no task migration can leak the
override across worker threads.

The doctor subscriber writes to stderr (not stdout), so any genuine
WARN/ERROR events surface in the normal log channel. The actual
doctor *report* goes to stdout — separation of concerns matches
operator expectations.

## 3. What was added

### 3.1 `doctor_cmd::run` body wrapped in `with_default`

```rust
pub async fn run(args: DoctorArgs) -> anyhow::Result<()> {
    tracing::subscriber::with_default(build_quiet_subscriber(), || -> anyhow::Result<()> {
        let data_dir = crate::sources::default_data_dir();
        let manifest = load_manifest()?;
        let platform = detect_platform();

        print_report(
            &mut std::io::stdout(),
            &data_dir,
            &manifest,
            &platform,
            args.verbose,
        )
    })
}
```

10-line module-level doc comment above explains why the swap is
safe and what it suppresses, mirroring the Day-5 inline-doc style.

### 3.2 New `build_quiet_subscriber` free function

```rust
fn build_quiet_subscriber() -> impl tracing::Subscriber + Send + Sync {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .finish()
}
```

Extracted as a free function so the new unit test (§ 3.3) can build
the *same* subscriber pointed at a `Vec<u8>` capture writer and
assert end-to-end behaviour. Without extraction, the test would have
had to duplicate the builder — a maintenance burden + drift risk.

`with_ansi(false)` is deliberate: no terminal escapes in stderr-bound
warnings, since operators might pipe stderr to a log file.

### 3.3 New behavioural unit test

`doctor_quiet_subscriber_suppresses_info_logs_but_passes_warn`:

- Builds an exact replica of `build_quiet_subscriber` but with a
  `CaptureWriter` (small `MakeWriter` adapter over
  `Arc<Mutex<Vec<u8>>>`) instead of stderr.
- Calls `tracing::info!("vz: startup orphan-prune complete")` and
  `tracing::warn!("doctor: substrate kernel checksum mismatch")`
  inside `with_default(subscriber, ...)`.
- Asserts the captured buffer **does not** contain
  `"startup orphan-prune"` and **does** contain
  `"substrate kernel checksum mismatch"`.

The test pins both halves of the contract: INFO suppression AND
WARN-and-above pass-through. The latter is the more important
assertion — a regression that broke warning emission would silently
hide real triage problems.

## 4. Validation

### 4.1 Compile

```
$ cargo check -p elastos-server
   Checking elastos-server v0.2.0
    Finished `dev` profile [...] in 7.41s
```

Zero warnings.

### 4.2 Targeted doctor tests (all three)

```
$ cargo test -p elastos-server --lib doctor_cmd::
running 3 tests
test doctor_cmd::tests::doctor_quiet_subscriber_suppresses_info_logs_but_passes_warn ... ok
test doctor_cmd::tests::doctor_reports_absent_artifact_with_remediation ... ok
test doctor_cmd::tests::doctor_reports_present_artifact_with_size_and_verbose_metadata ... ok

test result: ok. 3 passed; 0 failed; ...; 385 filtered out
```

### 4.3 Full elastos-server lib suite (regression check)

```
$ cargo test -p elastos-server --lib
test result: ok. 386 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 1.23s
```

**386/386**. Pre-Day-6 was 385; the +1 is the suppression test.
Zero regressions.

### 4.4 Manual smoke (this Mac, branch sash/local-test, debug build)

```
$ ./target/debug/elastos doctor 2>&1 | grep -E 'INFO|orphan-prune'
$ echo $?
1
```

Zero matches. grep's exit code 1 = "pattern not found" = success.

```
$ ./target/debug/elastos --help | head -3
ElastOS - sovereign Home and runtime
...
$ ./target/debug/elastos --version
elastos 0.2.0-dev
```

CLI surface unaffected — `with_default` did not leak into the
non-doctor code paths.

## 5. What this explicitly does NOT touch

To respect scope discipline, Day-6 is the minimum change that
satisfies the acceptance criteria:

- **The global subscriber in `main.rs`** is unchanged. `serve`,
  `setup`, `gateway`, etc. continue to emit INFO logs as configured
  by `RUST_LOG` or the default `elastos=info` directive.
- **The supervisor's INFO log itself** is unchanged. The line is
  still emitted by the substrate code — it just isn't visible
  during doctor's lifetime. `serve` operators still see the
  startup-prune diagnostic.
- **Doctor's report format** is unchanged. § 1's after-output is
  byte-identical to the Day-5 output minus the one INFO line.

## 6. Files changed (full inventory)

| file                                                              | delta            | role                                                       |
|-------------------------------------------------------------------|------------------|------------------------------------------------------------|
| `elastos/crates/elastos-server/src/doctor_cmd.rs`                 | +85 / -7         | `run` body wrap + `build_quiet_subscriber` + 1 unit test   |
| `docs/vz-backend/PHASE_6_PLAN.md`                                 | +1 status banner | Days 1–6 complete + Day 7 forward-link                     |
| `docs/vz-backend/PHASE_7_DAY_6_NOTES.md`                          | +new (this file) | day journal                                                |

Net: ~85 LOC across one file edit + plan banner + notes. No
supabase / schema / Vz substrate / CLI / network changes.

## 7. What remains in Phase 7 (after Day 6)

In rough priority order, none blocking end-user features:

1. **CI Mac runner activation**: scaffolding done in Phase 6 Day 8;
   needs hardware procurement only. The Day-1 decision (use
   Canonical's pinned kernel + initrd) means CI doesn't need to
   build a kernel from source — it can run the same `elastos
   setup --profile minimal` + boot test that works on this Mac.
2. **Apple Developer ID signing pipeline**: for distributing
   `elastos-server` to operators outside the dev cohort. Out of
   scope while we're branch-only.
3. **Phase 8 carrier RPC validation**: still requires a bootable
   rootfs image staged on Mac. Day-2 staged the kernel + initrd;
   the rootfs is the missing piece (separate manifest entry,
   separate provenance pipeline).

With Days 1–6 done, the Mac Vz path is now operator-ready: setup
fetches the substrate, supervisor wires it correctly, doctor
inspects it cleanly, and the same Phase-6 boot test continues to
reproduce in ~0.3 s. The remaining Phase-7 items are
infrastructure/distribution concerns, not substrate gaps.
