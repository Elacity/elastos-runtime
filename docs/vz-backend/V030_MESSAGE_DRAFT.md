# Message draft to Anders — paste-ready (v0.3.0 rebase complete)

> Tone target: deferential to his time, evidence-led, no asks. Three
> short paragraphs. Pointer to the full memo for anyone who wants
> depth. Trim/edit as needed before sending.

---

**Subject:** Mac VZ branch refreshed against v0.3.0 main — ready for v0.3.1 review

Anders, the Mac VZ branch (`sash/local-test`, the one with the Apple
`Virtualization.framework` substrate) is now refreshed against v0.3.0
main on PR #3 / `sash/local-test-v030`. Four-day rebase wraps up
today; same shape as the CVE rebase from last month — Day 1 baseline,
Day 2 conflict-free pieces, Day 3 the big three-way merge
(supervisor / vm-provider / carrier-bridge), Day 4 sign-off (the
`carrier_invoke` ABI logic merge, fuzz harness restoration, gate
re-baseline, workspace walk-through). Daily notes are in
`docs/mac-vz/v030-rebase/DAY_{1,2,3,4}.md`.

Three things worth knowing for the v0.3.1 review window:

1. **The branch is CI-green on Linux and clean under
   `cargo clippy --workspace --tests -- -D warnings` /
   `cargo fmt --all -- --check` on both Linux and Mac.** All 60
   supervisor unit tests, 24 carrier-bridge unit tests (incl. the new
   v0.3.0 principal-aware ones), 108 elastos-vz unit tests pass on
   Mac. The `linux-untouched.yml` gate is re-baselined to the Day-4
   HEAD so future commits keep being checked against "no protected
   crate touched beyond what the rebase already shipped."

2. **Mac CI has 6 known failures, all in
   `gateway_browser_route_tests::test_browser_*`, none of them rebase
   regressions.** Concrete root cause is captured in
   `docs/mac-vz/v030-rebase/DAY_3.md`: v0.3.0's
   `gateway_browser_stream::browser_runtime_stream_socket_path`
   builds the runtime stream socket under
   `std::env::temp_dir().join("elastos-browser-streams")`, which on
   macOS resolves under `/var/folders/<XX>/<YY>/T/...` — the
   resulting absolute path exceeds Darwin's 104-byte `sun_path`
   limit. Linux's `sun_path` is 108 bytes and stays under. This is a
   v0.3.0-on-Mac platform issue in the test path that pre-dates our
   branch; flagging here for project-level decision (cfg-gate to
   Linux / shorten the socket path on Darwin / accept Mac CI as
   informational).

3. **`carrier_bridge.rs` was the genuinely tricky merge.** Both sides
   landed roughly 1000 lines of changes in different directions —
   v0.3.0 added the `carrier_invoke` ABI migration (replacing the
   old `provider_call` request type) plus principal-aware
   localhost-fs scoping; the Mac VZ archive added byte-budgeted
   framing, the socketpair-based `spawn_carrier_bridge_on_stream`
   entry point, and `BridgeContext.on_terminate` lifecycle
   observability. Day 3 layered the wrong way (archive base + v0.3.0
   fields), which left the bridge speaking the old `provider_call`
   ABI while the guest already spoke `carrier_invoke` — Linux CI
   stayed green only because no test exercises a real socketpair end
   to end. Day 4 inverted the strategy (v0.3.0 logic base + Mac VZ
   framing/lifecycle on top); the relevant 24 unit tests now exercise
   the v0.3.0 principal-aware paths. Worth a sanity-check pass on
   that file in particular if anything looks off in v0.3.1 review.

Full memo with file-by-file conflict shape, the test diff, and the
known-issues catalog (incl. the SUN_LEN root cause and the
`components.json` gap) lives in
`docs/mac-vz/v030-rebase/DAY_4.md` on `sash/local-test-v030`. Branch
state is stable and CI-green; a follow-up note with the open
v0.3.1-shape decisions (scope, landing strategy, the 6 SUN_LEN
failures, principal-rooted scoping on Mac, etc.) goes out separately
so you can reply per-question at your own pace.

---

# Follow-up message — questions for Anders (paste-ready)

> Send this as a separate message after the status note above, or
> together as a second block. Numbered so Anders can reply per item.

**Subject:** Mac VZ rebase — open v0.3.1-shape decisions

Anders, on top of the "rebase complete" note, here are the genuine
open decisions where I'd value your call before / during the v0.3.1
review window. Numbered so you can reply by number; happy to take any
of these in a sync if that's faster than async.

### Strategy / scope

1. **macOS scope for v0.3.1.** Is native Linux-microVM parity on Mac
   (the `Virtualization.framework` substrate this branch ships) in
   scope for v0.3.1, or does the v0.3.1 macOS story stay strictly
   browser-hosted per ROADMAP? If browser-hosted only, the
   `elastos-vz` crate can be hard-cfg-gated and we cut a lot of test
   surface. If native parity is the goal, PR #3 needs to land on main.

2. **Landing strategy for PR #3.** Assuming native parity is in scope:
   do you want PR #3 (`sash/local-test-v030`) to merge into main as
   part of v0.3.1, or held as a parallel branch through v0.3.1 and
   folded in at v0.3.2? If merge — what's your acceptance bar
   (CI all-green incl. Mac, code review depth, real-hardware smoke
   on a signed build, all of the above)?

### Concrete fixes

3. **The 6 Mac-CI `gateway_browser_route_tests::test_browser_*`
   failures.** Root cause is v0.3.0's runtime stream socket path
   exceeding Darwin's 104-byte `sun_path` limit. Three options:
   - **(a)** `cfg`-gate the affected tests to Linux only — cheapest,
     defers the real fix.
   - **(b)** shorten the runtime stream socket path on Darwin to fit
     under `SUN_LEN` — correct fix, ~½ day.
   - **(c)** accept Mac CI as informational, don't gate merges on it.

   Preference?

4. **`components.json` and `darwin-arm64`.** v0.3.0 main ships zero
   `darwin-arm64` capsule platform declarations. Do you want this
   filled in for v0.3.1 (requires capsule-release-pipeline work to
   actually produce darwin-arm64 binaries), or stays Linux-only with
   `darwin-arm64` deferred to v0.3.2+?

5. **Principal-rooted localhost-fs scoping on the Mac path.** Day 4
   wired the `BridgeContext` fields through `start_capsule_vm_macos`
   (`principal_id`, `data_dir`). Is the v0.3.1 plan to ship Mac VZ
   with principal-rooted scoping **active at parity with Linux**, or
   to stay **flat-rooted** at v0.3.1 with the fields just preserving
   forward compatibility for v0.3.2+?

### Compatibility

6. **`provider_call` → `carrier_invoke` ABI migration.** v0.3.0 main
   hard-rejects the legacy `provider_call` request type. Are there
   any external / published guest SDK consumers still sending
   `provider_call`, or is the population fully migrated? If not fully
   migrated, do we want a deprecation period (warn + accept) before
   the hard reject lands?

7. **Real-kernel `elastos-vz` boot tests + codesigning.**
   `single_vm_boots_to_userspace` and `concurrent_load_with_real_kernel`
   require the `com.apple.security.virtualization` entitlement, so the
   test binaries need codesigning via `scripts/dev/sign-elastos-vz/`.
   Wire that into the standard dev loop (CI hook + pre-test signing
   step), or leave as a manual operator pre-step documented in
   `docs/dev/testing.md`?

I'll be on `#elastos-runtime` if any of these is faster as a quick
back-and-forth. Day-4 sign-off + branch state is in
`docs/mac-vz/v030-rebase/DAY_4.md` on `sash/local-test-v030` if you
want context for any specific item.
