## Phase 3 Day 6 — `MacVsockDial` provider-bridge integration

> Outcome log. Status: complete. The Day-5 vsock dial primitive
> is now plumbed end-to-end through the supervisor's
> `ProviderRegistry`. Capsules running inside a Mac microVM can
> not only **call out** to host providers via `/dev/hvc1`
> (Day 4) but can now **be reached by** other capsules' provider
> calls — closing Phase 3's core promise that ElastOS's capsule
> isolation and routing semantics hold on Mac with no Apple
> entitlements and no Linux-side changes.

### Goal (recap)

Day 5 added the FFI primitive (`RunningVm::connect_vsock` →
Apple's `VZVirtioSocketDevice.connectToPort:`) but stopped short
of wiring it into the supervisor's API provider plumbing. As of
Day 5:

- `elastos-server/src/vm_provider.rs::try_vsock_connect` still
  called `socket(libc::AF_VSOCK, SOCK_STREAM, 0)` — Linux-only.
  On Mac this surfaces as `EAFNOSUPPORT`.
- `start_capsule_vm_macos` set `provider_route = None`
  unconditionally because the Linux helper expects a TAP IP, and
  TAP is gated by Apple entitlements (Phase 3 Day 7+).

Net effect: a Mac capsule like `localhost-provider` could boot,
but no sibling capsule could reach its provider endpoint —
`localhost://Users/self/Documents/foo.txt` from `chat` would
fail with `EAFNOSUPPORT` at the bridge.

Day 6 closes that gap end-to-end without disturbing the Linux
launch path.

### What landed

1. **`elastos-server/src/vm_provider.rs::MacVsockDial` type
   alias.**

   ```rust
   pub type MacVsockDial = Arc<
       dyn Fn(u32) -> Pin<Box<dyn Future<Output = std::io::Result<OwnedFd>> + Send>>
           + Send + Sync,
   >;
   ```

   This is the canonical "object-safe async function" pattern
   used elsewhere in the workspace. No new dependencies — the
   `Pin<Box<dyn Future …>>` plumbing rides on tokio/futures
   types already in the dep graph.

2. **`VmRawBridge::new_with_vsock_dialer`** sibling constructor.
   `VmRawBridge` gains a `mac_vsock_dialer: Option<MacVsockDial>`
   field. The Linux call site keeps using `VmRawBridge::new`,
   which sets the field to `None` — the wire diff against Day 5
   is zero on Linux.

3. **`VmRawBridge::try_connect_once` branches on the dialer.**
   When `mac_vsock_dialer.is_some()`, the bridge takes a new
   `try_mac_vsock_dial` path:

   ```rust
   fn try_mac_vsock_dial(&self, dialer: MacVsockDial, port: u32)
       -> Result<VmIo, ProviderError>
   {
       let owned_fd = tokio::runtime::Handle::current()
           .block_on(dialer(port))?;
       // wrap fd into File, try_clone for writer half, stash raw
       // fd for wait_for_response()'s poll(), exactly like the
       // AF_VSOCK arm.
   }
   ```

   `Handle::current().block_on` is safe here because the bridge
   is always invoked from inside `tokio::task::spawn_blocking`
   (the `Provider::send_raw` boundary) — blocking-pool threads
   are outside the runtime worker pool, so `block_on` does not
   panic.

   We route on closure presence, not `cfg!(target_os)`, so
   tests on either platform can inject a fake dialer to
   exercise the bridge without touching the kernel.

4. **`VmCapsuleProvider::new_with_vsock_dialer`** convenience
   constructor that forwards to the new bridge constructor.
   Used only from the Mac supervisor arm.

5. **`Supervisor::register_provider_route_with_vsock_dialer`**
   sibling of `register_provider_route`. Builds the
   `VmCapsuleProvider` via the new constructor instead of the
   Linux one; otherwise byte-identical (same
   `parse_provider_route_from_provides` parser, same
   `register_sub_provider` / `register` calls, same logging
   shape). `cfg(target_os = "macos")` so it doesn't bloat the
   Linux binary.

6. **`start_capsule_vm_macos` builds the dialer closure and
   wires the route.** Before inserting the new `RunningCapsule`,
   if `manifest.provides.is_some()`:

   ```rust
   let running_weak = Arc::downgrade(&self.running);
   let handle_for_dialer = handle.clone();
   let dialer: crate::vm_provider::MacVsockDial = Arc::new(move |port| {
       let running_weak = running_weak.clone();
       let handle = handle_for_dialer.clone();
       Box::pin(async move {
           let Some(running) = running_weak.upgrade() else {
               return Err(io::Error::new(io::ErrorKind::NotConnected,
                   "supervisor running map has been dropped"));
           };
           let map = running.read().await;
           let Some(rc) = map.get(&handle) else {
               return Err(io::Error::new(io::ErrorKind::NotConnected,
                   format!("capsule handle '{handle}' is no longer running")));
           };
           match &rc.backend {
               CapsuleBackend::VzVm(vm) => vm.connect_vsock(port).await
                   .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e.to_string())),
               _ => Err(io::Error::new(io::ErrorKind::Unsupported, …)),
           }
       })
   });
   let provider_route = self
       .register_provider_route_with_vsock_dialer(name, manifest.provides.as_deref(),
           handle.clone(), launch_config_for_route, dialer)
       .await;
   ```

   The `Weak<RwLock<HashMap<…>>>` is the key safety property:
   the closure does not keep the supervisor's running map
   alive, and on every dial it re-resolves the live
   `RunningCapsule` for the handle. A torn-down VM cleanly
   surfaces `io::ErrorKind::NotConnected` rather than dialing
   freed memory.

7. **Launch-config preservation.** `build_vm_config_for_mac`
   consumes its `launch_config: Value` to bake boot args (mirror
   of the Linux flow at L1015–L1024). Day 6 clones `config`
   before that call so the cloned copy can be passed to the
   provider-route registration — the Linux arm at L1203 does
   exactly the same dance.

### Tests

Five new tests (all in `elastos-server`):

| Test | File | What it proves |
|---|---|---|
| `vm_capsule_provider_uses_mac_dialer_when_present` | `src/vm_provider.rs` | Bridge sends `init`+`read` over a dialer-provided socketpair and gets a JSON response back. End-to-end JSON envelope round-trip without touching the kernel's vsock. |
| `vm_capsule_provider_propagates_dialer_errors` | `src/vm_provider.rs` | A dialer that fails with `NotConnected` surfaces through the connect-retry loop; the bridge's `io` guard stays empty so the next request retries the dialer instead of reusing a half-built `VmIo`. |
| `mac_vsock_dialer_takes_priority_over_af_vsock_path` | `src/vm_provider.rs` | Even when `guest_host` parses as a numeric CID (`"42"`), the dialer short-circuits — the bridge's error mentions `"mac vsock dial"` and never `"vsock connect to CID"`. Defensive: no silent fallthrough to `AF_VSOCK`. |
| `register_provider_route_with_vsock_dialer_attaches_provider_to_registry` | `src/supervisor.rs` (macOS-gated) | Supervisor helper registers a real `VmCapsuleProvider` for `localhost://` and the registry can resolve the scheme. Validates the registration path without booting a Vz VM. |
| `mac_vsock_dialer_closure_returns_not_connected_when_handle_is_missing` | `src/supervisor.rs` (macOS-gated) | Exercises both fall-throughs in the dialer-closure shape baked into `start_capsule_vm_macos`: weak-upgrade succeeds + map miss → `NotConnected("no longer running")`; weak-upgrade fails (map dropped) → `NotConnected("running map has been dropped")`. |

Workspace test count: **351 green on Mac** (was 346 at end of
Day 5; +5 new tests). Clippy `-D warnings` clean. `cargo fmt`
clean. `scripts/check-linux-untouched.sh bcf5a0a` clean.

### What's still not Linux-parity

Day 6 closes the *core* "inter-capsule provider RPC works on
Mac" promise. The remaining gaps before Phase 3 fully wraps:

- **`VZBridgedNetworkDeviceAttachment`** for capsules that
  declare `permissions.guest_network`. Entitlement-gated; the
  Mac supervisor still bails out with a typed error directing
  the user to drop `guest_network` or wait for Phase 3 Day 7+.
- **End-to-end smoke harness.** The five tests above prove the
  individual seams but a full
  `localhost-provider` ↔ `chat` round-trip is still a manual
  test today — neither CI runner has signed Vz binaries.
  Documented in `docs/MAC.md`; the supervisor logs each step
  (`register_provider_route_with_vsock_dialer` → bridge dial →
  `RunningVm::connect_vsock` → bytes flow) for human-driven
  verification.
- **Multi-VM concurrent boot.** Each `VzProvider::load` creates
  a fresh dispatch queue but the supervisor has never been
  stressed with N>1 simultaneous Mac launches. Phase 4
  territory.

### Files touched

| File | Change |
|---|---|
| `elastos-server/src/vm_provider.rs` | New `MacVsockDial` type alias, `VmRawBridge::new_with_vsock_dialer` + `try_mac_vsock_dial`, `VmCapsuleProvider::new_with_vsock_dialer`. Plus three new tests. |
| `elastos-server/src/supervisor.rs` | New `register_provider_route_with_vsock_dialer` (Mac-only), `start_capsule_vm_macos` builds the dialer closure and threads it through, clones `launch_config` before the consuming call. Plus two new Mac-gated tests. |
| `docs/vz-backend/PHASE_3_DAY_6_NOTES.md` | This file. |
| `docs/vz-backend/PLAN.md` | Phase 3 status header → Day 6 complete. |
| `docs/MAC.md` | Capability matrix updated: capsule code on Mac is now both a provider client AND a provider server. |

### Linux-untouched gate

`scripts/check-linux-untouched.sh bcf5a0a` is **green**. No
files in `elastos-crosvm/`, `elastos-runtime/`,
`elastos-common/`, or `elastos-compute/` were modified. The
Linux launch path's only contact with the new code is via the
public `VmCapsuleProvider::new` (unchanged signature) and
`Supervisor::register_provider_route` (unchanged signature) —
Linux passes no dialer and routes through the existing
`socket(AF_VSOCK,…)` path byte-for-byte.

### Day 7 handoff

Next milestone: `VZBridgedNetworkDeviceAttachment` for capsules
that need `guest_network`. This is entitlement-gated; the
deliverable shape is (a) detect the entitlement at runtime,
(b) accept the `permissions.guest_network` manifest flag when
present, (c) keep the typed fail-closed error when absent.
After Day 7 the Phase 3 "Mac substrate parity with Linux for
NAT-only capsules" promise is fully signed off.
