# Mac VZ — Anders review decisions (v0.3.1 review window)

> Source: Anders review reply, 2026-05-29.
> Branch under review: `sash/local-test-v030` (PR #3).
> These seven rulings are **authoritative**. The follow-up work tracked in
> `docs/mac-vz/v030-rebase/DAY_*` (the post-rebase day plan) executes against
> them. Do not reinterpret scope without a new ruling.

## Headline

macOS stays **browser-hosted** for now. Native Mac VZ / Linux-microVM parity is
**not in scope** for a v0.3.1 merge — it is gated behind real Apple Silicon
hardware operator-testing. PR #3 is held as a **parked v0.3.2+ branch**, not a
v0.3.1 merge candidate.

## The seven decisions

### 1. macOS scope
macOS stays browser-hosted. Native Mac VZ / Linux-microVM parity is not in scope
until we have real Apple Silicon hardware proof. Anders will not merge a native
substrate he cannot operator-test. (KVM is also not used in any meaningful way
yet.)

### 2. Landing strategy
Hold PR #3 as a draft / parallel v0.3.2+ branch. **Merge acceptance bar:**
- rebase onto current main,
- Linux CI green,
- Mac CI green **or** explicitly justified,
- real Apple Silicon **signed-build smoke**,
- **hunk-level review** of `carrier_bridge.rs`, `supervisor.rs`, `vm_provider.rs`,
- **no regression** to Linux / Home / Browser / Carrier paths.

### 3. SUN_LEN browser-test failures
Prefer **option (b)** — fix the Darwin socket path length properly. Do **not**
`cfg`-gate the tests away except as a temporary diagnostic step. Mac CI must not
stay informational once we claim Mac support.

### 4. components.json / darwin-arm64
**Defer** public `darwin-arm64` platform declarations until the capsule release
pipeline actually produces `darwin-arm64` artifacts. Dev/test metadata is fine,
but the **shipped registry must not advertise platforms we do not build**.

### 5. Principal-rooted localhost-fs scoping
If Mac VZ ships, it **must ship with principal-rooted scoping active, not
flat-rooted**. Wiring the fields through is good, but parity means the **same
isolation semantics as Linux**.

### 6. provider_call → carrier_invoke
Default stays **hard-reject**. Anders is not aware of published external guest
SDK consumers that still require `provider_call`; if any exist, **name them
explicitly**. **No silent warn-and-accept path** in the privileged bridge. If
compatibility is genuinely needed, build a **documented dev-only compatibility
adapter with a clear expiry** — never a quiet acceptance path.

### 7. Real-kernel VZ tests + codesigning
Keep codesigning as a **documented manual operator step** for now. Wire it into
CI only once we own Mac hardware / runners. Until then, Mac VZ merge-readiness
must include a **signed manual smoke artifact**.

## What this means for the follow-up day plan

| Decision | Follow-up action | Status |
|----------|------------------|--------|
| 1, 2 | Reframe PR #3 as parked v0.3.2-track; record acceptance bar | Day 1 |
| 5 | Verify Mac-path scoping enforcement (not just wiring) + parity test | Day 1 verify / Day 3 test |
| 3 | Fix Darwin runtime-stream socket path under `SUN_LEN`; un-gate 6 browser tests | Day 2 |
| 4 | Keep `darwin-arm64` out of shipped registry; separate dev/test metadata | Day 4 |
| 6 | Confirm hard-reject stays; audit for `provider_call` senders; name any | Day 4 |
| 7 | Document manual signing step; define signed-smoke merge artifact | Day 4 |

### Decision 5 — verification finding (Day 1)

Confirmed by inspection on `sash/local-test-v030`: the Mac launch path
(`supervisor.rs::start_capsule_vm_macos`) constructs `BridgeContext` with
`principal_id` and `data_dir` **byte-identically to the Linux launch path**
(`supervisor.rs::launch_capsule`). Both feed the **same** shared
`carrier_bridge::run_carrier_bridge_loop` dispatch, whose principal-aware
enforcement (`protected_principal_root_carrier_response`,
`principal_root_read_write_uri`, `scope_current_user_alias`) reads those fields.
The only Mac/Linux divergence in the context is `on_terminate` (a lifecycle
observer, unrelated to scoping). **Conclusion: scoping is already enforced at
parity, not flat-rooted.** Remaining work for decision 5 is therefore a
Mac-gated parity *test* asserting a capsule cannot escape its principal root on
the Mac path — not an implementation change.
