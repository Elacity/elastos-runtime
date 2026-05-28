## Phase 4 Day 2 — Real-kernel multi-VM harness + Carrier-bridge multiplex audit

> Outcome log. Status: complete. Day 1 proved the Vz substrate
> *configures* N concurrent VMs safely (per-VM dispatch queue,
> race-free CID allocator, reaper isolation). Day 2 promotes
> the real-kernel concurrent boot test from `#[ignore]` to an
> auto-discovering visible-skip and audits the second
> previously-unexamined concurrency surface: the supervisor's
> Carrier bridge dispatch loop when N VMs simultaneously feed
> bytes to `/dev/hvc1`.

### Kernel/rootfs fixture decision

**Decision:** auto-discover from the supervisor's canonical
install paths, with env-var overrides for non-standard
locations. Do NOT regenerate or stage fixtures at test time
(that would touch the Linux-untouched perimeter).

Discovery order:

| Resource | Override env var | Canonical fallback |
|---|---|---|
| Kernel | `ELASTOS_VZ_TEST_KERNEL` | `~/.local/share/elastos/bin/vmlinux` |
| Rootfs | `ELASTOS_VZ_TEST_ROOTFS` | `~/.local/share/elastos/capsules/*/rootfs.ext4` (first match) |

When either is missing, the test `eprintln!`s a clear "skipping
— no X found" message and returns `Ok(())`. CI logs show the
skip explicitly (it's NOT `#[ignore]`d). Promoting to a
required gate is a separate runner-fleet task: it needs an
Apple-Silicon GitHub Actions runner with the kernel + at least
one capsule pre-installed, which is out of scope for Day 2.

Why this specifically: the supervisor's own
`VzConfig::default()` already resolves to
`~/.local/share/elastos/bin/vmlinux` (see
`elastos-vz/src/config.rs::VzConfig::new`), so the test's
discovery just mirrors what production does. No new artefact
location, no test-only path. The same applies to the rootfs:
`capsule_dir.join("rootfs.ext4")` is the supervisor's wiring
verbatim.

### Carrier-bridge ownership + lock surfaces (audit)

| Surface | Owner | Lifecycle | Lock granularity |
|---|---|---|---|
| `spawn_carrier_bridge_on_stream` `JoinHandle` | DETACHED (`tokio::spawn`, return value discarded) | Bridge dispatch loop runs until socket EOF / read-error / write-error; supervisor does NOT track the handle. | None — single-task ownership of the `UnixStream`. |
| `BridgeContext::provider_registry` | `Arc<ProviderRegistry>` cloned per bridge | Lives as long as the registry | Registry's internal locking (read-mostly for `has_provider`, `resolve`); contended only on capability mutations (rare). |
| `BridgeContext::capability_manager` | `Arc<CapabilityManager>` cloned per bridge | Process-lifetime | Internal locking. |
| `BridgeContext::pending_store` | `Arc<elastos_runtime::capability::pending::PendingRequestStore>` cloned per bridge | Process-lifetime | Internal locking (concurrent capability approval polls). |
| `BridgeContext::capsule_id` | Per-bridge `String`, owned | Per-bridge | None — owned. |
| Per-bridge `UnixStream` | Owned by the bridge task (split into reader + writer) | Closes on bridge exit | None — single-task ownership. |

**Audit conclusion: no architecture changes required for N
bridges.** The detached-spawn model is correct under N>1
because:

1. Each bridge owns its own `UnixStream` — no shared I/O.
2. The shared `BridgeContext` only carries `Arc`-clones for
   read-mostly state behind internal locks; the bridge does
   not hold those locks across `await` points.
3. Socket-driven termination means dropping the
   `RunningCapsule` (which drops `carrier_host_fd`) makes the
   guest endpoint unreachable, the host endpoint's read
   returns `Ok(0)` (EOF), and the loop breaks cleanly. No
   `JoinHandle::abort()` is needed.
4. The supervisor's `running` RwLock is NOT touched by any
   bridge — bridges talk only to providers/capabilities, never
   to the supervisor's running-map.

The original Day-2 plan asked about per-bridge `JoinHandle`
tracking. The audit shows that adding a `Vec<JoinHandle>` per
supervisor would be **net negative**: it would create a new
race surface (the supervisor's `running` map and the handle
vec falling out of sync) while solving nothing — the bridge
already terminates cleanly without explicit aborts.

### What landed

1. **`elastos-vz/tests/concurrent_launch.rs`** — promoted
   `concurrent_load_with_real_kernel`:
   - Drops `#[ignore]`.
   - Adds `discover_kernel()` and `discover_rootfs()` helpers
     honouring `ELASTOS_VZ_TEST_KERNEL` /
     `ELASTOS_VZ_TEST_ROOTFS` overrides with fallback to the
     supervisor's canonical paths.
   - Returns `Ok(())` with a visible `eprintln!` when
     discovery fails or `is_supported()` is false. CI logs
     the skip; no silent passes.
   - On a developer machine with the kernel + rootfs
     installed, the test spawns three concurrent VMs through
     ONE `VzProvider`, asserts every `CapsuleHandle::id` is
     distinct.

2. **`elastos-server/src/carrier_bridge.rs`** — three new
   tests in the existing `mod tests`:
   - `build_socketpair()` helper: factors out the
     `socketpair(AF_UNIX, SOCK_STREAM)` setup used by the
     existing Day-4 ping/pong test (the existing test was
     not refactored, only the new tests use the helper, to
     keep the Day-4 test diff-stable).
   - `ping_bridge()` helper: round-trips a ping request and
     parses the JSON pong; 2-second timeout returns `None`
     so callers can distinguish "alive but slow" from
     "dead".
   - `three_concurrent_carrier_bridges_isolate_per_capsule`:
     three bridges share ONE `Arc<ProviderRegistry>`; each
     responds to its OWN ping (request-id echo proves no
     cross-bridge contamination).
   - `dropping_one_carrier_endpoint_terminates_only_that_bridge`:
     three bridges; pre-shutdown pings on alpha + charlie
     succeed; drop bravo's guest endpoint; alpha + charlie
     continue serving while bravo's loop exits on the next
     read EOF.

3. **`elastos-server/src/supervisor.rs`** — new test:
   - `reap_dead_capsules_does_not_starve_concurrent_readers`:
     three capsules (alpha Running, bravo Stopped, charlie
     Running); a "reader" task holds `running.read().await`
     for 200ms; reaper waits the expected ~150ms for the
     read to release; ONLY bravo is removed post-reap. The
     reader's snapshot taken under the held read lock is
     identical to its post-sleep snapshot (proves the
     RwLock keeps the writer-side reaper blocked while the
     read is held — no partial-mutation observed by
     concurrent readers).

### Hard constraints satisfied

- **Linux launch path byte-identical.**
  `scripts/check-linux-untouched.sh bcf5a0a` green. No
  protected-crate diff.

- **Exhaustive match arms.** `cargo clippy --workspace
  --all-targets -- -D warnings` clean on Mac. No new
  `match` over `CapsuleBackend` — the bridge tests use
  `socketpair` directly; the reaper race test mutates the
  existing `synthetic_vzvm_running_capsule` helper output
  via the same `if let CapsuleBackend::VzVm(...)` pattern
  Day 1 introduced.

- **Both `RUST_TEST_THREADS` values pass.** Verified for the
  new tests under `--test-threads=1` and `--test-threads=4`.

- **Visible skip, not silent pass.** The promoted real-kernel
  test prints to stderr on the early-return path; CI logs
  show the reason.

- **No new external dependencies.**

### Out of scope (intentional)

- Cross-VM resource pressure (CPU pinning, memory headroom
  under M concurrent VMs).
- Carrier bridge wire-format upgrades.
- Apple Silicon GitHub Actions runner provisioning (separate
  infra task).
- Promoting `concurrent_load_with_real_kernel` from a
  visibly-skipping test to a required gate — that requires
  the runner-fleet work above.

### Lessons / observations

- The "detached `tokio::spawn` for bridges" pattern is the
  right one for socket-driven lifecycles. Tracking handles
  in a `Vec<JoinHandle>` would have been a Day-2 refactor
  that *added* race surface without buying anything. The
  audit's value was confirming this with a test rather than
  taking it on faith.

- The reaper race test is the kind of test that's easy to
  write *and easy to write wrong*. The first cut had the
  reader sleep BEFORE acquiring the lock, which made the
  test pass by accident (reader never had the lock when
  reaper ran). The current shape — acquire lock first, then
  sleep — exercises the actual contention surface.

- `read_line` returns `Ok(0)` on EOF and `Err` on broken-
  pipe; both terminate the bridge loop. The shutdown test
  triggers the EOF path (clean `drop` of the peer), which
  is what the supervisor's clean-shutdown path also does.
  The broken-pipe path (kill -9 of the guest while
  mid-message) is covered by the write-error branch in the
  loop, which exits the same way.
