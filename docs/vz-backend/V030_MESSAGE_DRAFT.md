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
`docs/mac-vz/v030-rebase/DAY_4.md` on `sash/local-test-v030`. **No
action requested — both branches are stable, CI-green, draft PR's,
awaiting your v0.3.1 review window.**
