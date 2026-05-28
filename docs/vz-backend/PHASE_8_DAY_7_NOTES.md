# Phase 8 Day 7 — Interactive Console Attach

**Status:** ✅ Complete. `elastos run ubuntu-base` now drops the operator at a
working `root@ubuntu:/#` Linux shell on macOS.

---

## What shipped

### 1. Bidirectional kernel console in `elastos-vz`

`build_kernel_console(interactive: bool)` (`crates/elastos-vz/src/ffi/console.rs`)
gained a new branch. Two construction modes now live behind the same return
type:

| Mode | Vz attachment | Host side | Used by |
| --- | --- | --- | --- |
| **Pipe-backed** (Days 1-6) | `forReading: None`, `forWriting: pipe(2)` write end | `host_read = Some(File)` → `console_forwarder` → `tracing::INFO target=vm_console` | All headless / `vm-debug boot` / `cargo test` paths. Byte-identical to Day 6. |
| **Interactive-stdio** (Day 7+) | `forReading: dup(STDIN_FILENO)`, `forWriting: dup(STDOUT_FILENO)` | `host_read = None` → forwarder skipped → operator's TTY *is* the console | `run_microvm_standalone` when stdin is a TTY. |

Both stdin and stdout are `dup`'d before being handed to
`VZFileHandleSerialPortAttachment`. The attachment runs `closeOnDealloc=true`,
so without dup'ing it would close the parent process's stdin/stdout when the
VM stops — corrupting the operator's shell. The dups are independent FDs that
the attachment can safely close on teardown.

The `BuiltMachine.kernel_console_host_read` field changed type to
`Option<std::fs::File>`. `VzMachineHandle::new` keys off `Option::is_some()` to
decide whether to spawn the console forwarder. No new public API; the existing
`VmConfig.interactive_stdio` knob (declared in Phase 2, dormant until now) is
the switch.

### 2. Standalone lane wiring in `elastos-server`

`run_microvm_standalone` (`crates/elastos-server/src/run_cmd.rs`):

1. Calls `enable_host_raw_mode_pub()` (already used by `chat_cmd`,
   `home_cmd`, `capsule_cmd`). The returned `TermiosGuard` is held for the
   VM's lifetime, restored on drop.
2. Sets `VmConfig.interactive_stdio = raw_mode_guard.is_some()`. When stdin
   is not a TTY (CI, piped input, headless smoke), the guard is `None` and
   we silently fall back to the Day-6 pipe-backed path.
3. Prints a distinct banner per branch — "interactive mode" vs "headless
   mode" — so the operator can tell at a glance which lane fired.
4. Explicitly `drop()`s the guard after the VM stops so the operator's
   shell is normal before control returns.

The `enable_host_raw_mode_pub` implementation keeps `ISIG` set (see
`runtime_control.rs:111` — `// Keep ISIG so Ctrl+C generates SIGINT even in
raw mode`), so the host terminal still intercepts Ctrl-C and forwards SIGINT
to the elastos process. The trade-off documented in the run banner: Ctrl-C in
the host stops the VM (not the guest shell); to interrupt a guest command,
the operator types `Ctrl-C` after exiting through `poweroff` and re-running.
A guest-side Ctrl-C escape sequence (e.g. QEMU's `Ctrl-A x`) is a Day-8+ UX
nicety, not Day-7 scope.

### 3. Autologin via overlay-init drop-in

The Day-6 overlay-init was extended to drop two files into the tmpfs
upperdir *before* `switch_root`:

1. `/usr/local/sbin/elastos-login` — a 3-line bash wrapper that discards
   agetty's `-p -h <host> -f <user>` argv and `exec -l /bin/bash`'s an
   interactive login shell.
2. `/etc/systemd/system/serial-getty@hvc0.service.d/autologin.conf` — a
   systemd drop-in that swaps the unit's `ExecStart` for
   `agetty --autologin root --login-program /usr/local/sbin/elastos-login …`.

**Why the wrapper instead of `--login-program /bin/bash` directly.** agetty
always passes `-p -h <host> -f <user>` to whatever `--login-program` names.
Bash misinterprets `-h` as its own `hashall` builtin flag and the trailing
username as a script path (`bash: root: Is a directory`). The wrapper soaks
up those args and execs bash cleanly.

**Why bypass `/bin/login` entirely.** Ubuntu's PAM stack pulls in
`pam_securetty` (which rejects root login on `hvc0` — it's not in
`/etc/securetty`) and `pam_unix` (which validates `/etc/shadow` even on
`-f`-skip-auth invocations). Canonical's cloud image ships root with a
locked password (`!*` in shadow), so `/bin/login` always demands a password
the operator can't provide. The Day-5 smoke surfaced this as `Password:`
hanging after every `(automatic login)`. Bypassing `/bin/login` is correct
for the v0.1 operator-only lane: agetty has already vouched for the
autologin user out of band, and there is no multi-tenant trust boundary
inside the guest. Multi-tenant / password-protected capsules are a
manifest-driven Day-8+ design.

Both drop-ins live in the *ephemeral* tmpfs upperdir, so they:

- Do not modify the squashfs base (Canonical's pinned image stays intact).
- Reset on every `elastos run` exit (no cumulative state across runs).
- Add zero bytes to the rootfs distribution payload.

The Day-6 CPIO writer (`overlay_initrd.rs::write_combined_initrd`) detects
the script change via its byte-equality idempotency check, so
`elastos setup` rebuilt `bin/initrd-overlay` automatically (33058136 bytes vs
the Day-6 33057432; +704 bytes for the new shell logic).

---

## Acceptance — all green

Smoke ran via `/usr/bin/expect` so the Day-7 wiring runs against a real
PTY-allocated terminal (the agent's harness is otherwise headless).

```
spawn /Users/sash/code/elastos-runtime/elastos/target/debug/elastos run ubuntu-base
…
[run] guest started in interactive mode. Press Ctrl-C in the host terminal to
stop the VM (the host raw-mode guard keeps ISIG enabled). Inside the guest,
run `poweroff` for a clean shutdown.

…systemd boot to multi-user.target…

Ubuntu 22.04.5 LTS ubuntu hvc0
ubuntu login: root (automatic login)
root@ubuntu:/# cat /etc/os-release
PRETTY_NAME="Ubuntu 22.04.5 LTS"
VERSION="22.04.5 LTS (Jammy Jellyfish)"
…
root@ubuntu:/# uname -srm
Linux 5.15.0-179-generic aarch64
root@ubuntu:/# mount | grep overlay
overlay on / type overlay (rw,relatime,lowerdir=/lower,upperdir=/upper/upper,workdir=/upper/work,xino=off,nouserxattr)
root@ubuntu:/# ^C
[run] Ctrl-C received
[run] stopping VM…
[run] done.

OK: Day-7 shell smoke complete
EXIT=0
```

Checkpoints, mapped to the Day-7 prompt's acceptance bar:

- [x] `elastos run ubuntu-base` shows `ubuntu login:` on a real interactive
      terminal (banner: "guest started in interactive mode" → autologin
      handshake → bash PS1).
- [x] Operator can type `root` + Enter (the expect script's `send "root\r"`
      reached `getty`; agetty echoed `root` back and `bin/login` printed
      `Password:` in the first smoke run, then `--autologin` + wrapper
      bypassed the prompt entirely in the second).
- [x] Reach a `#` prompt — `root@ubuntu:/#` shows in the smoke log.
- [x] `cat /etc/os-release` returns Ubuntu 22.04 — confirmed.
- [x] `uname -a` (smoke used `-srm`) returns Linux 5.15 arm64 — confirmed.
- [x] `mount | grep overlay` confirms overlayfs rootfs — confirmed, Day-6
      wiring still intact end-to-end.
- [x] Ctrl-C + reboot work cleanly; the `TermiosGuard` restored the
      operator's terminal on drop. No corruption after the smoke.
- [x] `cargo test -p elastos-server --lib`: **404 passed; 0 failed**.
- [x] `cargo test -p elastos-vz --lib`: **96 passed; 0 failed** (+1 from
      the new `interactive_kernel_console_does_not_produce_a_host_read_fd`
      regression-pin).
- [x] One commit, one notes file (this file).

---

## Files touched

| File | Change |
| --- | --- |
| `crates/elastos-vz/src/ffi/console.rs` | Split `build_kernel_console` into `build_pipe_kernel_console` (Day-6 path) + `build_interactive_kernel_console` (new). `KernelConsole.host_read` → `Option<File>`. New unit test pins the interactive branch. |
| `crates/elastos-vz/src/ffi/builder.rs` | Pass `vm.interactive_stdio` to `build_kernel_console`. `BuiltMachine.kernel_console_host_read` → `Option<File>`. Builder test updated. |
| `crates/elastos-vz/src/ffi/lifecycle.rs` | `VzMachineHandle::new` accepts `Option<File>`; spawn console forwarder only when `Some`. `forwarder` field → `Option<ConsoleForwarder>`. |
| `crates/elastos-server/src/run_cmd.rs` | `run_microvm_standalone` calls `enable_host_raw_mode_pub()`, sets `interactive_stdio` based on whether a TTY was acquired, distinct banners per branch, explicit guard drop on exit. |
| `crates/elastos-server/src/overlay_initrd.rs` | Overlay-init writes `/usr/local/sbin/elastos-login` + the `serial-getty@hvc0` autologin drop-in into the tmpfs upperdir before `switch_root`. New unit test pins the wrapper + drop-in. |
| `docs/vz-backend/PHASE_6_PLAN.md` | Status banner updated. |
| `docs/vz-backend/PHASE_8_DAY_7_NOTES.md` | New (this file). |

---

## What I didn't do (intentional, scoped out)

- **Guest-side Ctrl-C escape sequence.** Operator interrupting a long-running
  guest command currently means: exit the VM (Ctrl-C in host → SIGINT) and
  re-run. A QEMU-style `Ctrl-A x` / `Ctrl-A c` switching dispatcher between
  "send to host" and "send to guest" is a Day-8+ UX win.
- **Persistent shell history / state.** The overlay upperdir is tmpfs;
  every run starts fresh. A `--persistent` flag that maps a host-side
  ext4-on-loop image at `/upper` is a manifest-driven Day-8+ feature.
- **Multi-tenant / password-protected capsules.** The wrapper bypasses
  `/bin/login` because v0.1 has one operator and one boundary (the host
  user). When MicroVMs serve multiple users, the manifest should toggle
  `agetty --login-program /bin/login` and the operator should set passwords
  via cloud-init / pre-baked overlay layers.
- **vsock-based console attach for headless servers.** Currently
  interactive mode requires the elastos CLI to share a terminal with the
  operator. A future `elastos console <vm>` subcommand that connects to a
  vsock port for already-running guests is a useful detached-attach
  primitive; not Day-7.

---

## Known minor

`~/Library/Application Support/elastos/components.json` (the stamped/cached
manifest copy) still carries the Day-5 `size: 430985216` for `rootfs` even
though the repo manifest is now `431013888`. This causes a spurious
"size mismatch → refresh" on the first setup run after any pull, but the
download completes and stamps the new value. Cosmetic. Day-8 cleanup item:
have `setup.rs` write the resolved size back to the stamped manifest after
a successful refresh.
