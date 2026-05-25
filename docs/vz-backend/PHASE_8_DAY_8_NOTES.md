# Phase 8 Day 8 — Real ElastOS capsule on Mac (standalone WASM lane)

**Status:** ✅ Complete. A real ElastOS WASM capsule (`home`) now boots,
runs, and exits cleanly on macOS — no `elastos serve` daemon, no Linux VM,
no Docker, no cross-compile toolchain. End-to-end native execution through
wasmtime via the elastos runtime.

```
$ elastos run capsules/home
[run] WASM capsule launching standalone (in-process; no `elastos serve` daemon detected)
home capsule launched: name=home id=wasm-805f3daf-e212-437e-94e9-986986824296 ts=1779733823
[run] WASM capsule 'home' exited
$ echo $?
0
```

This is the first ElastOS-shipped capsule (not Ubuntu, not a smoke
fixture) to run on a Mac through the same `elastos run <capsule>`
operator UX Linux users have. It caps Phase 8's "real workload runs"
mission: Day 6 made Ubuntu boot writably, Day 7 made the operator
interactive, Day 8 actually runs an ElastOS app.

---

## What shipped

### 1. Standalone WASM lane in `elastos-server`

`run_wasm_standalone` (`crates/elastos-server/src/run_cmd.rs`) mirrors
the Day-5 `run_microvm_standalone` pattern: when no `elastos serve`
daemon is writing the coords file, fall back to in-process wasmtime
execution rather than hard-failing on the `operator_runtime_coords()`
read. Dispatch lane (also in `run_cmd.rs`):

```rust
elastos_common::CapsuleType::Wasm => {
    if operator_runtime_available().await {
        return run_wasm_via_operator_runtime(&capsule_dir, capsule_args).await;
    }
    return run_wasm_standalone(&capsule_dir, capsule_args).await;
}
```

Three architectural choices made explicit in the comments:

1. **No bridge spawner.** The operator-runtime lane installs a
   `wasm_bridge_spawner` so SDK calls from the capsule round-trip to
   the daemon. Standalone leaves it unset → `WasmProvider::start`
   reads `use_bridge = bridge_spawner.is_some() == false` → inherited
   stdio, no plumbing. Sufficient for the v0.1 demo bar (the shipped
   WASM capsules have 19-line `main()`s that print a launch banner via
   `elastos_guest::CapsuleInfo::from_env()` and return; the richer
   browser surface is rendered by the daemon, not the WASM stub).
2. **Storage under `<data_dir>/storage`.** Same data dir Phase 7's
   `elastos doctor` inspects, same dir the setup loop installs to,
   same dir the standalone microvm lane uses. Single source of truth
   for "where ElastOS state lives" across all three Mac lanes
   (MicroVM, WASM, future Data).
3. **No raw mode.** The v0.1 standalone WASM capsules are one-shot
   prints, not interactive readers. The function explicitly documents
   "if/when a real interactive WASM capsule lands, this is the place
   to add a `_saved_termios` guard mirroring the operator lane" — so
   the next refactor has a labelled hook.

The existing `ScopedTerminalEnv::capture()` still runs so capsules
that inspect `ELASTOS_TERM_COLS` / `_ROWS` get sensible values.

### 2. JIT entitlements for Hardened Runtime

`scripts/dev/sign-elastos-vz/vz.entitlements.plist` gained three new
keys:

```xml
<key>com.apple.security.cs.allow-jit</key>
<true/>
<key>com.apple.security.cs.allow-unsigned-executable-memory</key>
<true/>
<key>com.apple.security.cs.disable-executable-page-protection</key>
<true/>
```

Without `allow-jit`, the sign script's `--options runtime` (Hardened
Runtime) flag makes macOS SIGKILL the process the first time wasmtime
calls `mprotect(PROT_EXEC)` or `mmap(..., MAP_JIT, ...)` — the
binary silently exited 137 with no error visible to the operator
because the kernel kills the process before stderr flushes. With all
three keys, wasmtime's W^X JIT codegen (MAP_JIT + pthread\_jit\_
write\_protect\_np) runs unblocked.

`allow-unsigned-executable-memory` + `disable-executable-page-protection`
are belt-and-braces for the older `mprotect`-based JIT paths some
wasmtime configurations still take; setting them explicitly avoids
the Day-8-smoke debug loop where the first signing pass only added
`allow-jit` and the process still SIGKILL'd at the next codegen step.

The MicroVM/Vz lane is unaffected — Vz doesn't touch host JIT, and
the new entitlements are additive (the existing
`com.apple.security.virtualization` still grants Vz access).
Day-7 VM smoke re-ran against the freshly resigned binary post-Day-8;
boot reached `Ubuntu 22.04.5 LTS ubuntu hvc0` exactly as before.

Plist syntax constraint: Apple's AMFI parser rejects HTML-style
comments interleaved between dict key/value pairs with
`AMFIUnserializeXML: syntax error near line N`. Two iterations during
the smoke surfaced this. The final plist keeps comments out of the
file body entirely (they live in the sign script's docs + this notes
file).

### 3. `home.wasm` staged in the capsule directory

`capsule.json` declares `"entrypoint": "home.wasm"`, which
`WasmProvider::load` resolves as `capsule_dir.join(entrypoint)`. The
build produces `capsules/home/target/wasm32-wasip1/release/home.wasm`
(65 542 bytes). For Day 8 the artefact was copied to
`capsules/home/home.wasm` so `elastos run capsules/home` works
verbatim without configuring `entrypoint` to point into `target/`.

Building the wasm: `cargo build --release --target wasm32-wasip1` from
`capsules/home/`. The `wasm32-wasip1` rustup target is already
installed on this dev box; if not, `rustup target add wasm32-wasip1`
gets it. Zero Docker, zero cross-compile toolchain.

---

## Acceptance — all green

```
$ elastos run capsules/home
2026-05-25T18:30:23 INFO elastos::run_cmd: Running capsule from: ../capsules/home
2026-05-25T18:30:23 INFO elastos: vz provider enabled (Apple Virtualization.framework available)
[run] WASM capsule launching standalone (in-process; no `elastos serve` daemon detected)
2026-05-25T18:30:23 INFO elastos_server::runtime: Loading capsule 'home' (Wasm)
2026-05-25T18:30:23 WARN elastos_server::runtime: Signature verification skipped (no trusted keys configured)
2026-05-25T18:30:23 INFO elastos_compute::providers::wasm: Loaded WASM capsule 'home' with ID wasm-805f3daf-e212-437e-94e9-986986824296
2026-05-25T18:30:23 INFO elastos_compute::providers::wasm: Starting capsule 'home'
home capsule launched: name=home id=wasm-805f3daf-e212-437e-94e9-986986824296 ts=1779733823
[run] WASM capsule 'home' exited
$ echo $?
0
```

Checkpoints, mapped to the Day-8 prompt's acceptance bar:

- [x] `elastos run capsules/home` runs natively on Mac without an
      `elastos serve` daemon — confirmed.
- [x] Capsule prints its launch line (`home capsule launched: name=home
      id=... ts=...`) — confirmed.
- [x] Clean exit, `[run] WASM capsule 'home' exited`, `$? == 0`.
- [x] Operator-runtime lane preserved — dispatch falls through to it
      when `operator_runtime_available()` is true. Source review +
      no test deletions confirm.
- [x] `home.wasm` built natively on Mac via
      `cargo build --target wasm32-wasip1 -p home --release`. Zero
      Docker, zero cross-compile toolchain, zero new system deps.
- [x] Day-7 VM lane regression-checked: same binary still boots Ubuntu
      to `Ubuntu 22.04.5 LTS ubuntu hvc0` (`/tmp/p8d8-vm-smoke.log`).
- [x] `cargo test -p elastos-server --lib`: **404 passed; 0 failed**.
- [x] `cargo test -p elastos-vz --lib`: **96 passed; 0 failed**.
- [x] `cargo test -p elastos-compute --lib`: **4 passed; 0 failed**.
- [x] One commit, one notes file (this file).

---

## Files touched

| File | Change |
| --- | --- |
| `elastos/crates/elastos-server/src/run_cmd.rs` | Add `run_wasm_standalone` (in-process wasmtime, no bridge spawner, no raw mode, storage at `<data_dir>/storage`). Modify Wasm dispatch to gate on `operator_runtime_available()` and fall back to standalone — mirrors the Day-5 MicroVM fallback. |
| `scripts/dev/sign-elastos-vz/vz.entitlements.plist` | Add `com.apple.security.cs.allow-jit` + `allow-unsigned-executable-memory` + `disable-executable-page-protection`. Strip in-dict comments (AMFI parser rejects them). |
| `capsules/home/home.wasm` | Staged 65 542-byte WASM artefact at the capsule root so `entrypoint: "home.wasm"` resolves verbatim. |
| `docs/vz-backend/PHASE_6_PLAN.md` | Status banner updated. |
| `docs/vz-backend/PHASE_8_DAY_8_NOTES.md` | New (this file). |

---

## What I didn't do (intentional, scoped out)

- **Standalone-WASM bridge.** Capsules that make SDK calls today
  (identity, IPFS, signed messaging, provider registry) still require
  the daemon, because standalone mode leaves the bridge spawner
  unset. A future "in-process bridge" with a local-only provider
  registry would let standalone serve richer capsules; not Day-8
  scope. The 19-line shipped capsules don't need it.
- **`elastos run home` (name-only, no path).** That UX requires the
  capsule to be installed under `<data_dir>/capsules/home/` so the
  Day-5 `resolve_capsule_by_name` fallback resolves. Day 8 ships the
  `capsules/home/` repo-relative path; pre-stamping a capsule install
  into the data dir is a setup-loop concern.
- **Build automation for `home.wasm`.** Right now a contributor
  manually runs `cargo build --target wasm32-wasip1 --release` then
  `cp target/wasm32-wasip1/release/home.wasm home.wasm`. Wiring this
  into `elastos setup` or a `make capsules` target is post-Phase-8.
- **Other WASM capsules.** `home-cli`, `chat-room`, `chat-wasm`,
  `system` all share the same pattern. Once their `.wasm` is built
  they should "just work" via the same standalone lane, but each
  needs its `entrypoint` artefact staged. Verifying every one of
  them is regression coverage, not Day-8 scope.
- **MicroVM capsule on Mac.** The other big v0.1 demo would be
  building one of the MicroVM capsules (`shell`, `agent`,
  `ipfs-provider`) into a rootfs and booting it through the Day-5
  standalone microvm lane. Blocked on cross-compile toolchain (musl,
  aarch64-linux-gnu, mke2fs) — Linux-only build pipeline. Phase 9
  scope or a separate distribution-only effort.

---

## Phase 8 retrospective

| Day | Headline | Substrate state |
| --- | --- | --- |
| 1 | Rootfs strategy decided: Canonical squashfs | Pure planning, zero code |
| 2 | `components.json` rootfs entry; setup downloads it | `elastos setup` installs the rootfs |
| 3 | `elastos doctor` reports the rootfs row | Triage tool covers full substrate |
| 4 | Integration tests platform-aware; first real Mac boot | `concurrent_load_with_real_kernel` + `single_vm_boots_to_userspace` green on Mac |
| 5 | `elastos run ubuntu-base` boots to systemd userspace | Standalone MicroVM lane lit; capsule.json auto-generated |
| 6 | Writable tmpfs overlay; clean boot to `ubuntu login:` | Overlay-init module + custom CPIO concatenation |
| 7 | Interactive console: `root@ubuntu:/#` shell | Bidirectional Vz kernel console + autologin |
| 8 | Real ElastOS WASM capsule runs on Mac | Standalone WASM lane lit; both Mac lanes (VM + WASM) operator-ready |

Phase 8 closed. The user's original goal — "i just need it working on
mac" — is met for both the VM-shaped workloads (Day 7) and the
WASM-shaped workloads (Day 8). Remaining Mac-substrate gaps are
distribution polish (signed builds, brewfile, persistent overlay),
not capability gaps.
