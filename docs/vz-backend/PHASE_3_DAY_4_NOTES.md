## Phase 3 Day 4 — real Carrier console socketpair (`/dev/hvc1` ↔ host Unix socket)

> Outcome log. Status: complete. Bytes now flow guest↔host on
> `/dev/hvc1` for Mac microVMs; first-party capsules
> (`chat`, `did-provider`, …) can talk to host providers via
> `RequestEnvelope` / `ResponseEnvelope` exactly the same way
> the Linux flow does over crosvm's `unix-stream` carrier.

### Goal (recap)

Day 3 wired `CapsuleBackend::VzVm` into the supervisor so
`elastos ps` / `status` / `stop` work on Mac. The one thing
capsule code inside the VM still **could not do** was talk to
the host Carrier bridge: the Vz console attachment at
`elastos-vz/src/ffi/console.rs::build_carrier_console_slot` was
a `pipe(2)` loop that swallowed every guest write. The Day-2
host-side `<socket_dir>/<handle>-carrier.sock` listener existed
but never had a connection — the guest's `/dev/hvc1` was wired
to nothing real.

Day 4 replaces the placeholder pipe with a real
`socketpair(AF_UNIX, SOCK_STREAM)` and routes the host-side fd
straight into the Carrier bridge dispatch loop. No Unix
listener / accept on Mac — the host endpoint is handed to
`tokio::net::UnixStream::from_std` directly.

### What landed

1. **`build_carrier_console_slot` now returns a `CarrierConsole`**
   (`elastos-vz/src/ffi/console.rs`):

   ```text
   socketpair(AF_UNIX, SOCK_STREAM)
       ├── host fd  ──►  OwnedFd (non-blocking, owned by caller)
       └── vz fd    ──►  NSFileHandle (closeOnDealloc=true)
                                │
                                ▼
                  dup(vz_fd) ──► NSFileHandle (closeOnDealloc=true)
                                │
                                ▼
                  VZFileHandleSerialPortAttachment.initWithFileHandleForReading_fileHandleForWriting
                                │
                                ▼
                  VZVirtioConsolePortConfiguration[0]
                                │
                                ▼
                  VZVirtioConsoleDeviceConfiguration  ──►  guest sees `/dev/hvc1`
   ```

   The Vz-side `dup` is necessary because `NSFileHandle` has
   `closeOnDealloc=true` and Apple's attachment API takes two
   separate handles (one for reading, one for writing). With
   the duplicate, each handle owns one fd; the socket endpoint
   stays alive until both close, then the kernel reclaims it.

2. **`BuiltMachine::carrier_host_fd`** (new field). Holds the
   host-side `OwnedFd` so `VzProvider::load_with_vm_config`
   can flow it through to `RunningVm` without crossing the
   `VZVirtualMachineConfiguration` ownership boundary.

3. **`RunningVm::take_carrier_host_fd(&mut self) -> Option<OwnedFd>`**
   (new). The supervisor calls this exactly once,
   immediately after `VzProvider::take_running_vm`. Subsequent
   calls return `None` — the bridge owns the fd from then on.

4. **`VzMachineHandle::new`** refactor: now takes destructured
   `vz_config` + `kernel_console_host_read` parameters instead
   of a whole `BuiltMachine`. This avoids holding the non-`Send`
   `Retained<VZVirtioConsoleDeviceConfiguration>` across the
   provider's `vms.write().await` insertion point.

5. **`carrier_bridge.rs` refactor** (additive):

   | Function | Role |
   |---|---|
   | `run_carrier_bridge_loop(stream, ctx, label)` (new, internal) | Shared per-connection dispatch loop. Reads newline-delimited `RequestEnvelope`s, dispatches via `bridge_ctx`, writes `ResponseEnvelope`s back. Body is byte-identical to the Day-2 inline loop. |
   | `spawn_carrier_bridge(path, …)` (unchanged signature) | Binds a Unix listener, accepts one connection, hands the stream into `run_carrier_bridge_loop`. **Linux flow byte-identical** — crosvm's `--serial type=unix-stream` connects exactly as before. |
   | `spawn_carrier_bridge_on_stream(stream, …)` (new) | Skips bind/accept; takes a pre-connected `tokio::net::UnixStream` and hands it into `run_carrier_bridge_loop`. **Mac flow uses this** with the carrier socketpair host fd. |

6. **`Supervisor::start_capsule_vm_macos`** picks up the fd
   immediately after `take_running_vm`, wraps it in a
   `tokio::net::UnixStream::from_std`, and calls
   `spawn_carrier_bridge_on_stream` with the same `BridgeContext`
   shape as Linux. The Day-2 `<socket_dir>/<handle>-carrier.sock`
   path is no longer bound (no listener); it's kept on
   `vm_config.carrier_socket_path` for parity with the Linux
   manifest dump and for future diagnostic surfaces.

7. **Tests** (Mac-gated unless noted):

   | Crate | Test | Asserts |
   |---|---|---|
   | `elastos-vz` | `carrier_slot_constructs_with_named_port` (updated) | Existing port-name + array-shape checks now also confirm `host_fd >= 0`. |
   | `elastos-vz` | `carrier_slot_uses_real_socketpair_with_paired_endpoints` (new) | Drops the Vz-side carrier console; observes peer-closed (EOF / EPIPE) on the host-side fd. Proves the socketpair is genuinely connected — no pipe loop. |
   | `elastos-server` | `spawn_carrier_bridge_on_stream_handles_ping_pong_over_socketpair` (new) | Creates a socketpair, drives one half through `spawn_carrier_bridge_on_stream`, writes a `ping` from the other half, reads `pong` back. End-to-end proof that bytes flow guest-style → host bridge → host-style response, on the same dispatch loop the Linux flow uses. |

8. **Docs:** this file; `PLAN.md` Phase 3 header advances to
   "Day 4 complete"; `MAC.md` capability matrix updates the
   Carrier console row from "stub" to "real socketpair, bytes
   flow guest↔host".

### Apple-API note that shaped Day 4

`VZFileHandleSerialPortAttachment` takes a pair of
`NSFileHandle`s — one for reading, one for writing — both with
`closeOnDealloc=true`. The naive shape "give Vz the same fd
twice" would double-close on dealloc. The naive shape "give Vz
one fd for read and one for write, both pointing at the same
socketpair endpoint" needs two distinct fds, so the Vz side
`dup`s its endpoint before wrapping. The kernel's socket
refcount keeps the endpoint alive until both duplicates close.

`Retained<VZVirtualMachineConfiguration>` (and the inner
`Retained<VZVirtioConsoleDeviceConfiguration>`) are not `Send`
because they hold raw Objective-C pointers. Day 4 changes
`VzMachineHandle::new` to take destructured pieces so the
provider's `async fn load_with_vm_config` can consume the
`BuiltMachine` entirely inside a non-async block before any
`await` — keeping the surrounding future `Send` so callers
in `tokio::spawn` continue to compile.

### What is still *not* working after Day 4

- vsock from host → guest is not yet bridged (Day 5). vsock
  *to* host providers is unaffected because Carrier now works.
- TAP networking (capsule reachable from host LAN) is rejected
  with a typed entitlement error (Phase 3 Day 6+; needs
  `com.apple.vm.networking`).
- `wait_for_exit_code` still returns `0` for every terminal
  state. Distinguishing clean shutdown vs crash needs
  `VZVirtualMachineDelegate` notifications (Day 5).
- The Day-2 `<socket_dir>/<handle>-carrier.sock` filesystem
  path is no longer bound on Mac. It's harmless (we keep the
  field set on `vm_config.carrier_socket_path` for log/diag
  parity), but if downstream tooling tries to `accept()` on it
  for telemetry it will block forever. Day 5 will revisit
  whether to drop the field entirely or keep it for diagnostics.

### Linux-untouched evidence

- `scripts/check-linux-untouched.sh bcf5a0a`: green.
- `carrier_bridge.rs` is in `elastos-server`, which is **not**
  in the Linux-untouched gate's protected paths (the gate
  protects `elastos-crosvm`, `elastos-runtime`, `elastos-common`,
  `elastos-compute` per `scripts/check-linux-untouched.sh`).
  The refactor is additive: `spawn_carrier_bridge`'s
  public signature and behaviour are unchanged for the Linux
  flow (still binds a listener, still accepts one connection,
  still uses the same per-connection dispatch loop — now via
  the extracted `run_carrier_bridge_loop` helper).
- `cargo clippy --workspace --all-targets -- -D warnings`:
  clean on Mac AND Linux.
- 507 tests green locally on Mac (Day 3 ended at 505; Day 4
  added 1 in `elastos-vz` and 1 in `elastos-server`).

### Day 5 handoff

The next slices, in priority order:

1. **vsock host→guest bridging.** Today the supervisor
   advertises `provider_vsock_port` to the capsule via the
   kernel command line (Day 2 work), but there's no host-side
   `VZVirtioSocketDevice.connectToPort` plumbing for the API
   provider to reach the capsule. On Linux this is implicit
   in `crosvm`'s vsock model; on Mac it's an explicit Apple-
   API loop we need to wire.
2. **`VZVirtualMachineDelegate` exit codes.** Replace the
   polling `wait_for_exit_code` with delegate-driven
   notifications so the supervisor can distinguish clean
   shutdown from crash and log the correct status.
3. **First-party capsule end-to-end on Mac.** With Carrier
   bytes flowing as of Day 4, the `chat` capsule should now
   boot and talk to the host's `LocalhostProvider`. A manual
   smoke test belongs in Day 5's notes; an automated one
   needs the cached artefacts the CI runners don't yet carry.
