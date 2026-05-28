## Phase 3 Day 7 — `VZBridgedNetworkDeviceAttachment` entitlement gating

> Outcome log. Status: complete. The Day-2 unconditional bail
> for `permissions.guest_network: true` on macOS is replaced
> with a runtime check against the
> `com.apple.vm.networking` Apple entitlement. Signed binaries
> get a bridged virtio-net attachment; unsigned dev binaries
> fail closed with a typed error pointing at `docs/MAC.md`.
> Phase 3 fully wraps the "Mac substrate parity with Linux"
> promise: both NAT-only and routable-network capsules now
> have a clean, fail-closed path.

### Goal (recap)

Day 2 — when bridged-mode support was deferred — installed an
unconditional bail in `start_capsule_vm_macos`:

```text
"vz: capsule 'X' requests guest_network (TAP), but Vz bridged
 networking requires the `com.apple.vm.networking` Apple
 entitlement — deferred to Phase 3 Day 4+. NAT-only capsules
 launch normally on macOS; this capsule needs to drop
 `permissions.guest_network` or wait for the bridged-mode
 milestone."
```

That fail-closed was honest but coarse: it triggered on every
Mac launch of every `guest_network`-flagged capsule, including
the signed release binary where the entitlement *would* be
granted at runtime. Day 7 turns this into a runtime-conditional
decision: detect the entitlement, and route accordingly.

### What landed

1. **`elastos-vz/src/ffi/entitlement.rs`** (new).

   Raw FFI to two `Security.framework` and four
   `CoreFoundation` functions — no new workspace dependency.
   The check is:

   ```text
   SecTaskCreateFromSelf(null)
     → SecTaskCopyValueForEntitlement(task, CFSTR("com.apple.vm.networking"), &err)
     → CFGetTypeID(value) == CFBooleanGetTypeID() && CFBooleanGetValue(value) == 1
   ```

   Process invariant; cached behind `OnceLock<bool>`. The
   unsigned dev binary (every `cargo build` artifact, every
   CI runner) returns `false`. Tests inject a different value
   via a thread-local `EntitlementOverrideGuard` — RAII
   discipline, restored on drop, no `std::env` pollution.

   All FFI bindings have an explicit `SAFETY:` block. Each
   `CFCreate`/`CFCopy` is matched by `CFRelease` on every
   path including early returns; the `CFErrorRef` out-param
   is released defensively when the API populates it.

2. **`elastos-vz/src/ffi/network.rs::build_bridged_network`**
   (new helper).

   Builds a `VZVirtioNetworkDeviceConfiguration` backed by
   `VZBridgedNetworkDeviceAttachment.initWithInterface:`. The
   interface comes from `VZBridgedNetworkInterface.networkInterfaces`'s
   first element (typically `en0`); an empty list surfaces a
   typed `"no host interface available for bridging"` error.
   The MAC address is the deterministic
   `NetworkConfig.guest_mac` (matching the crosvm-side
   `NetworkConfig::new` hash derivation), so the capsule sees
   the same MAC across reboots and across substrates.

3. **`elastos-vz/src/ffi/builder.rs`** branches the network
   device on `vm.network`:

   ```rust
   let network = match vm.network.as_ref() {
       None => build_nat_network(),
       Some(net_cfg) => {
           if !has_vm_networking_entitlement() {
               return Err(format!(
                   "vz machine builder: capsule '{}' requested guest_network … but this binary lacks the `com.apple.vm.networking` Apple entitlement. …",
                   vm.vm_id
               ));
           }
           build_bridged_network(net_cfg).map_err(…)?
       }
   };
   ```

   Three paths, all exhaustive:
   - **`None`** — NAT, byte-identical to Day 2.
   - **`Some(_)` + entitled** — bridged, deterministic MAC.
   - **`Some(_)` + unentitled** — typed `Err(String)` naming
     the entitlement and the manifest field.

4. **`elastos-server/src/supervisor.rs`** at the Mac arm:
   - The unconditional bail block at L1303-1313 is gone.
   - `build_vm_config_for_mac` now sets
     `vm_config.network = Some(elastos_vz::NetworkConfig::new(&vm_id))`
     whenever `manifest.permissions.guest_network` is `true`,
     mirroring the Linux `with_network(NetworkConfig::new(&vm_id))`
     at supervisor.rs L1123. The Vz builder takes it from
     there.

   This is the same shape the Linux flow has — the supervisor
   *populates the network config*, the substrate-specific
   builder *decides what to do with it*.

### Tests (+8)

| Test | File | What it proves |
|---|---|---|
| `entitlement_check_returns_false_for_unsigned_dev_binary` | `elastos-vz/src/ffi/entitlement.rs` | The dev-build invariant: the runtime check returns `Ok(false)` on every unsigned binary CI produces. Establishes the floor. |
| `override_for_testing_round_trips_true_and_false` | `elastos-vz/src/ffi/entitlement.rs` | Both override branches reach the entitlement-check call site. Bypass + production path both reachable in tests. |
| `override_guard_restores_prior_state_on_drop` | `elastos-vz/src/ffi/entitlement.rs` | Nested overrides honour drop order — inner drop reveals outer state. Guards against accidental leaks across parallel tests. |
| `builder_surfaces_typed_error_when_entitlement_absent_and_network_requested` | `elastos-vz/src/ffi/builder.rs` | With override = `false` and `vm.network = Some(_)`, the builder returns `Err(String)` containing `com.apple.vm.networking`, `guest_network`, and `vm_id`. |
| `builder_attaches_bridged_attachment_when_entitlement_present` | `elastos-vz/src/ffi/builder.rs` | With override = `true` and `vm.network = Some(_)`, the builder either produces a configuration with exactly one network device, OR (if the host genuinely has no bridge-capable interfaces) surfaces the `no host interface` error — both are correct fail-closed. |
| `builder_ignores_entitlement_when_capsule_uses_nat_only` | `elastos-vz/src/ffi/builder.rs` | NAT-only capsules (`vm.network = None`) succeed regardless of override state. Guards against an "entitlement controls all networking" regression. |
| `build_vm_config_for_mac_routes_guest_network_capsule_into_vm_config_network` | `elastos-server/src/supervisor.rs` (macOS-gated) | Manifest with `permissions.guest_network: true` produces `vm_config.network = Some(NetworkConfig)` with the expected 172.16.x.x/30 shape, deterministic MAC. |
| `build_vm_config_for_mac_leaves_network_none_when_guest_network_not_requested` | `elastos-server/src/supervisor.rs` (macOS-gated) | Mirror invariant: NAT-only capsules leave `vm_config.network = None`. Guards against an accidental "always-on bridged" regression. |
| `start_capsule_vm_macos_fails_closed_when_guest_network_lacks_entitlement` (renamed from the Day-2 `fails_closed_when_manifest_requests_tap_network`) | `elastos-server/src/supervisor.rs` (macOS-gated) | The end-to-end supervisor path: `guest_network: true` on an unentitled CI host produces an `Err`, never a silent success. Exact error surfaces depending on what else is missing (kernel/rootfs vs entitlement). |

Workspace total: **1033 green on Mac** (was 1025 end of Day 6;
+8 net). Clippy `-D warnings` clean. `cargo fmt` clean.
`scripts/check-linux-untouched.sh bcf5a0a` clean.

### What's still not Linux-parity

Day 7 closes the *core* "Mac substrate has both NAT and
bridged paths" promise. The remaining items before Phase 3
fully signs off:

- **Signed-build smoke harness.** The "entitlement-present"
  path is unit-tested via the override; an actual end-to-end
  test requires a Developer-ID-signed binary with the
  entitlement and a corresponding provisioning profile.
  That's release-engineering territory, not coding work.
- **Manifest-driven interface selection.** Today
  `pick_first_bridged_interface` picks `VZBridgedNetworkInterface.networkInterfaces[0]`
  unconditionally. A future `NetworkConfig.bridge_interface`
  field could let manifests pin to `en1` etc. Phase 4 polish.
- **Multi-VM concurrent boot stress.** Each `VzProvider::load`
  creates a fresh dispatch queue but the supervisor has never
  been stressed with N>1 simultaneous Mac launches. Phase 4
  territory.

### Files touched

| File | Change |
|---|---|
| `elastos/crates/elastos-vz/src/ffi/entitlement.rs` | New module. Raw Security.framework + CoreFoundation FFI, cached entitlement check, thread-local override for tests. |
| `elastos/crates/elastos-vz/src/ffi/mod.rs` | `pub(crate) mod entitlement;` |
| `elastos/crates/elastos-vz/src/ffi/network.rs` | New `build_bridged_network` helper + `pick_first_bridged_interface`. |
| `elastos/crates/elastos-vz/src/ffi/builder.rs` | Three-way network device branch + three new tests. |
| `elastos/crates/elastos-server/src/supervisor.rs` | Removed unconditional bail; `build_vm_config_for_mac` now sets `vm_config.network = Some(_)` for `guest_network: true` manifests. Updated Day-2 fail-closed test for Day-7 contract + two new tests. |
| `docs/vz-backend/PHASE_3_DAY_7_NOTES.md` | This file. |
| `docs/vz-backend/PLAN.md` | Phase 3 status header → Day 7 complete. |
| `docs/MAC.md` | Capability matrix updated with the signed-vs-unsigned outcome table. |

### Linux-untouched gate

`scripts/check-linux-untouched.sh bcf5a0a` is **green**. Zero
diff in `elastos-crosvm/`, `elastos-runtime/`,
`elastos-common/`, `elastos-compute/`. The Linux launch path
keeps using its `with_network(NetworkConfig::new(&vm_id))`
shape against the Linux `NetworkConfig` type — the Mac code
above is symmetrical but operates on the Vz `NetworkConfig`
re-export.

### Phase 4 handoff

Phase 3 closes here. Phase 4 territory:

- Multi-VM concurrent boot stress + dispatch queue contention
  audit (the per-`VzProvider` serial queue is the obvious
  bottleneck).
- `NetworkConfig.bridge_interface` manifest field +
  interface-name routing.
- Persistent block device sharing across capsules
  (`VZSharedDirectoryConfiguration` evaluation; out of scope
  if Carrier's existing virtio-blk path suffices).
- Mac-side release-engineering: Developer ID signing
  pipeline, entitlement plist, notarization.
