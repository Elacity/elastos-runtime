# Ready for Cursor — the local/KVM lane checklist

Everything that can be built in the cloud is done and pushed to
`claude/keep-consent-architecture-0fz0ll`. This file is the clean pickup list for
when you're at a machine with **Cursor + `/dev/kvm` + `CAP_NET_ADMIN`** (a real
Linux host with hardware virtualization and network-admin rights). Each item is
fully designed; only the hardware-dependent build + tests remain.

## Before you start (verify the branch is green in ~2 min)

```bash
cd elastos
cargo test  -p elastos-server -p elastos-runtime -p elastos-common --lib
cargo clippy -p elastos-server -p elastos-runtime --lib --tests -- -D warnings
cargo fmt -p elastos-server -p elastos-runtime -- --check
bash ../scripts/check-wci-alignment.sh
```
All four should pass. (Server lib ~876 tests, runtime ~345, common ~95.)

## 1. W1b — the real egress firewall  ← highest leverage

**Design:** `docs/W1B_EGRESS_FIREWALL.md` (complete — read it first).
**Why it matters:** converts the reach *model* (already built + tested) into
packet-level *enforcement* — the literal "we can prove the agent stayed in its
lane" proof (EU AI Act containment). This is the single biggest remaining piece.

Build steps (all specified in the design doc):
- A per-TAP `nftables` chain derived from the capsule's `EgressReach`
  (`None`=drop-all, `Allowlisted`=allow-resolved-only, `Open`=allow+tagged-hot),
  installed right after the TAP hook at `supervisor.rs:1115` (`guest_network`).
- An `EgressDenied` signed audit event on the flight recorder for every drop.
- Teardown-on-reap (keyed by `vm-<name>` — mirror the BUG-2/3 leak discipline).
- The DNS proxy (host-scoped allowlist vs IP filter) — proxy primary, IP-set interim.
- The 5 tests in the design doc, incl. the **compromised-guest backstop** (a guest
  bypassing the provider SDK is still dropped by the kernel chain).

## 2. BUG-2 / BUG-3 / BUG-7 — VM-lifecycle leaks

**Spec:** `docs/KNOWN_GAPS.md` ("Performance + bugs" section).
These need a real crosvm boot to test, so they're KVM-lane:
- **BUG-2:** per-launch `-carrier.sock` + detached bridge accept-loop leaked on
  teardown → store + abort the `JoinHandle`, remove the sock.
- **BUG-3:** boot-failure orphan: overlay + sockets + task leaked when `vm.start()`
  errors before the running-map insert → cleanup on the error path.
- **BUG-7:** reap treats the Carrier backend as unconditionally alive → real liveness.
(BUG-1, BUG-5, BUG-6, BUG-8 are already CLOSED in-cloud; BUG-4 mechanism is closed
with 3 real-provider migrations.)

## 3. W5b — the visual Svelte projection shell  (browser lane, not KVM)

**Spec:** `docs/ESP_SHELL_UI.md` (complete — 8 component contracts, each mapped to a
proven `elastos/esp/*.ts` projection + its schema tag, under the no-authority-in-the-
view invariant). The headless model + logic are all built and tested in-cloud;
only the live Svelte paint + a visual snapshot test remain.

## The standing discipline (keep it)

- One gated chunk at a time; commit only when `cargo test` + `clippy -D warnings` +
  `rustfmt --check` + the alignment ratchet are green.
- Verify-first: read the real code before changing an enforcement path; never
  loosen a capability action without confirming the op's true behavior.
- Fail-closed; name every deferral with a ratchet or a privilege requirement.
- The G3b conformance ratchet (`all_provider_manifests_preview_actions_match_verb_map_or_tracked`)
  will fail the build if any preview/enforce drift sneaks in — keep it green.

## When ready to merge

Open a PR from `claude/keep-consent-architecture-0fz0ll`. The campaign is ~42
commits: the W0–W7 ESP core, the consent + signed-receipt flow, the EU AI Act
audit artifact, the security/bug hardening (AUD-1..5 + BUG-1/4/5/6/8), the BUG-4
`DidNotAct` refund mechanism on 3 real providers, and the G3b universal
preview==enforce conformance pin (drift can no longer hide).
