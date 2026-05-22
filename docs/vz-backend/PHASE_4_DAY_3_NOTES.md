## Phase 4 Day 3 — Cross-VM provider RPC stress + capability-bridge dispatch race audit

> Outcome log. Status: complete. Day 1 audited the *configuration*
> concurrency surface (per-VM dispatch queue + CID allocator).
> Day 2 audited the *I/O* concurrency surface (Carrier-bridge
> multiplexing under N>1). Day 3 closes the third and final
> concurrency triangle: the *runtime side* of the bridge — the
> dispatch path from `handle_request` into the
> `ProviderRegistry`, into the per-`VmCapsuleProvider`
> `VmRawBridge`, and through into the per-VM connection back out
> to a sibling guest. The audit's purpose is to *prove* the
> existing primitives compose safely under N microVMs without
> editing the runtime crate. Zero `elastos-runtime` changes
> landed; every new test lives in `elastos-server`.

### Cross-VM RPC dispatch — shared-state touches

The full path traversed by one `provider_call` request line:

```
guest writes JSON to /dev/hvc1
  └─> run_carrier_bridge_loop (per-VM tokio::spawn, Day 2 audit)
        └─> handle_request (per-line, no shared state of its own)
              ├─> bridge_ctx.capability_manager.validate(...)
              │     └─> CapabilityManager  (Arc, internal RwLocks; Day 3 test §5)
              │           ├─ CapabilityStore::is_epoch_valid    (atomic read)
              │           ├─ CapabilityStore::is_token_revoked  (RwLock<HashSet>)
              │           ├─ CapabilityStore::try_use_token     (atomic CAS, only if max_uses)
              │           └─ AuditLog::emit                     (Mutex<VecDeque>, append-only)
              └─> bridge_ctx.provider_registry.send_raw(scheme, &req)
                    ├─ providers: RwLock<HashMap<&str, Arc<dyn Provider>>>  (read-clone-drop)
                    └─> VmCapsuleProvider::send_raw  (per-provider Arc)
                          └─ spawn_blocking → VmRawBridge::send_raw_blocking
                                └─ io: Mutex<Option<VmIo>>   (serializes per-VM)
                                      └─ blocking write/read on single OwnedFd
```

| Layer | Shared state | Lock | Granularity | Held across await? |
|---|---|---|---|---|
| `ProviderRegistry` | `providers`, `sub_providers` | `tokio::sync::RwLock<HashMap<…>>` | Whole map | **No** (read → clone `Arc` → drop guard, then await) |
| `VmCapsuleProvider` | `bridge: Arc<VmRawBridge>` | none (Arc clone is lock-free) | Per provider instance | n/a |
| `VmRawBridge` | `io: Option<VmIo>` | `std::sync::Mutex` | Per provider VM (one bridge instance) | n/a — bridge runs inside `spawn_blocking` |
| `CapabilityManager` | `store`, `audit_log`, `metrics` | each carries its own internal lock | granular per sub-state | **No** (validator reads; only `try_use_token` mutates and uses atomic CAS) |
| `PendingRequestStore` | `requests`, `session_requests` | `tokio::sync::RwLock<HashMap<…>>` each | Whole map per kind | **No** (each call acquires, mutates, releases before awaiting peer state) |

### Audit finding: there is no request-id allocator

The Day 3 prompt asks us to audit a "`VmCapsuleProvider`
request-id allocator". The audit conclusion is that **no such
allocator exists**:

- `VmRawBridge::send_raw_blocking` writes one JSON request line,
  then reads one JSON response line, all under
  `self.io.lock()`. Request and response are paired by **strict
  order** through a single connection.
- This is correct: the `Mutex` enforces single-in-flight per
  provider VM, so pairing-by-order is unambiguous.
- N concurrent callers against ONE `VmCapsuleProvider` queue at
  the Mutex (serial RPCs per VM). N callers against M providers
  see M-way parallelism (one independent Mutex + connection per
  provider).
- An `AtomicU64` allocator would only be necessary if multiple
  in-flight requests could share a connection. That would
  require additional response-routing machinery in the bridge,
  which is explicitly **not** the current design. If we ever
  switch to that model the allocator would land in the same PR.

This is documented here rather than tested because a "1000
parallel atomics increment" test against a non-existent counter
is not a useful gate. The behaviour that actually matters —
"60 RPCs through 3 consumers × 2 providers never cross-talk" —
is what the new test in `vm_provider.rs` proves directly.

### Per-bridge vs. per-provider state — design check

| Concept | Per-VM-bridge | Per-provider-VM | Why |
|---|---|---|---|
| `UnixStream` (carrier socket) | ✔ | — | Each microVM has its own `/dev/hvc1` <-> host socketpair (Day 2). |
| `BridgeContext` (capability_manager, provider_registry, pending_store) | shared `Arc` | shared `Arc` | These are *runtime-wide* singletons by design. Internal locking is granular. |
| `VmRawBridge::io` | — | ✔ (one Mutex per provider VM) | A single TCP/vsock connection cannot multiplex; the Mutex enforces it. |
| Pending-request bookkeeping | — | — | `PendingRequestStore::requests` keys are request UUIDs, not VM IDs; no per-bridge partitioning. |

The audit verifies these are the *intended* boundaries and the
new tests check that they hold under N>1.

### What landed

1. **Multi-bridge × multi-provider stress test** in
   `elastos-server/src/vm_provider.rs`:
   `cross_vm_rpc_dispatch_isolates_per_provider_under_n_consumers`.
   Two synthetic provider VMs on socketpairs (alpha, bravo) +
   three consumer tasks issuing 10 RPCs to each = 60 total
   round-trips. Each request carries a unique `nonce`; the
   synthetic VM echoes back a `{provider, nonce}` payload.
   Test assertions:
   - Every consumer's response carries the right provider
     marker → no cross-provider routing leak.
   - Every consumer's response carries the right nonce → no
     order-pairing leak inside one provider's Mutex.
   - Each provider served exactly 30 requests → no losses, no
     extras.

2. **Pending-request store concurrency test** in new file
   `elastos-server/tests/capability_concurrency.rs`:
   `pending_store_resolves_100_concurrent_requests_without_loss`.
   100 distinct sessions create 100 pending requests in
   parallel; half grant, half deny in parallel; final read-back
   asserts exactly 50 Granted + 50 Denied with zero losses and
   zero double-resolves.

3. **Capability-verify load test** in the same file:
   `capability_validate_under_1000_parallel_calls_does_not_serialize`.
   10 tokens, overlapping resource patterns, 1000 parallel
   `validate` calls. Loose acceptance threshold: <5s wall-clock
   on a multi-threaded Tokio runtime. The point isn't speed —
   it's catching a regression where any future change to
   `validate` would degrade the path from read-mostly to
   globally-serialized.

4. **Documentation**:
   - This file (`PHASE_4_DAY_3_NOTES.md`).
   - `docs/vz-backend/PLAN.md` Phase 4 status header bumped to
     Day 3.
   - `docs/MAC.md` capability matrix updated to note "cross-VM
     provider RPC under N concurrent microVMs" as audited.

### Linux-untouched and CI gates

- `scripts/check-linux-untouched.sh bcf5a0a` — green. The
  protected paths (crosvm, runtime, common, namespace, storage,
  compute, tls, identity) carry zero modifications. All edits
  are confined to `elastos-server/src/vm_provider.rs`,
  `elastos-server/tests/capability_concurrency.rs` (new), and
  the docs.
- `cargo clippy --workspace --all-targets -- -D warnings` —
  clean on macOS, expected clean on Linux (no platform-specific
  edits this day).
- `cargo fmt --all -- --check` — clean.
- All new tests pass under `RUST_TEST_THREADS=1` and
  `RUST_TEST_THREADS=4`.
- Stress tests' wall-clock: the RPC stress completes in ~10ms
  on dev hardware; the capability-validate load test completes
  in ~30ms; both are well within the CI budget.
- No new external dependencies.

### Carry-overs into Day 4+

- The host bridge's "one Mutex per provider VM" model means N
  consumer microVMs calling the SAME provider VM see serialized
  RPCs at the bridge. If a future capsule pattern needs
  fan-out-then-fan-in inside a single provider, the bridge would
  need either (a) multiplexing with a real request-id
  allocator, or (b) provider-side parallel handlers behind its
  own queue. Today's design assumes provider VMs are
  cheap-to-launch and operators dial up parallelism by adding
  provider VMs, not by demuxing inside one. That trade-off is
  documented; Phase 4 Day 4+ does NOT need to revisit it.
- Apple-runner CI (real Vz boots in the cross-VM stress test) is
  still a separate provisioning task. The current synthetic-VM
  test fully exercises the host-side dispatch graph; only the
  guest-kernel side is mocked.
