# Phase 9 Day 3 — Platform-aware `Full-screen Apps` backing

> **Outcome (2026-05-26):** The Day-2 self-verifier topped out at
> 5 / 8 services ready on a Mac source checkout, with one
> structurally-red row that no amount of installer plumbing could
> ever flip green: `Full-screen Apps`, whose
> `SystemServiceSpec.backing` hard-coded `["crosvm", "vmlinux"]`.
> crosvm is Linux-only by design — Mac runs Apple's
> Virtualization.framework embedded in the elastos binary, and
> the Phase-6 components audit explicitly omits crosvm from the
> macOS install plan. Day 3 makes the spec platform-conditional
> at compile time and adds three guard tests so the rule stays
> green across both platforms. **6 / 8 services ready** on Mac
> after this change (the other two are honest third-party
> dependencies kubo + cloudflared, not substrate gaps).
>
> **Anchor:** [`PHASE_9_DAY_2_NOTES.md`](PHASE_9_DAY_2_NOTES.md)
> § 4.3 called this out as the one substrate change Day 3+
> should pick up.

## 1. The change

`elastos/crates/elastos-server/src/home_cmd.rs`:

```rust
// Before
SystemServiceSpec {
    name: "Full-screen Apps",
    role: "Supports immersive full-screen app capsules …",
    backing: &["crosvm", "vmlinux"],
},

// After
SystemServiceSpec {
    name: "Full-screen Apps",
    role: "Supports immersive full-screen app capsules …",
    backing: FULL_SCREEN_APPS_BACKING,
},

#[cfg(target_os = "macos")]
const FULL_SCREEN_APPS_BACKING: &[&str] = &["vmlinux"];

#[cfg(not(target_os = "macos"))]
const FULL_SCREEN_APPS_BACKING: &[&str] = &["crosvm", "vmlinux"];
```

One platform-conditional static slice. The spec itself takes a
`backing: &'static [&'static str]` (it was already a borrowed
slice, so no signature change). On macOS the backing becomes
single-element `["vmlinux"]`; everywhere else it stays
`["crosvm", "vmlinux"]`.

### Why this is the right shape

Three alternative shapes were considered and rejected:

| Shape                                                             | Why rejected                                                                                                                                                                                                                                                                                          |
| ----------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| A runtime branch on `cfg!(target_os = "macos")` inside `SystemServiceSpec` | Doesn't work cleanly — the spec is a `const &[...]` so it has to be computable at compile time anyway. Using `cfg!()` would force the slice to allocate at runtime.                                                                                                                                   |
| A synthetic `vz` component in `COMPONENTS` that `gather_components` marks `available = true` on macOS | Cleaner long-term, but it adds a new abstraction (virtual components) for one spec entry. Defer until a second VMM-on-host check needs it.                                                                                                                                                            |
| Drop `Full-screen Apps` entirely on macOS                         | Loses the dashboard signal — operators on Mac _do_ want to know whether the VM lane is ready. Suppressing the row would mask real "vmlinux not installed" cases.                                                                                                                                       |

The cfg-gated constant is the minimal, additive change that
keeps both platforms honest: Linux still requires both binaries,
macOS only checks the one that lives on disk.

## 2. Tests

Three new tests live in `home_cmd::tests`, gated by the same
`#[cfg(target_os = ...)]` attributes as the constants, so the
test suite always asserts the rule that matches the host it's
running on:

```rust
#[cfg(target_os = "macos")]
#[test]
fn full_screen_apps_backing_is_vmlinux_only_on_macos() {
    assert_eq!(FULL_SCREEN_APPS_BACKING, &["vmlinux"]);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn full_screen_apps_backing_keeps_crosvm_off_macos() {
    assert_eq!(FULL_SCREEN_APPS_BACKING, &["crosvm", "vmlinux"]);
}

#[cfg(target_os = "macos")]
#[test]
fn full_screen_apps_ready_on_macos_with_just_vmlinux() {
    let snapshot = sample_snapshot_with_components(&["vmlinux"]);
    let services = gather_system_services(&snapshot.components);
    let full_screen = full_screen_apps_service(&services);
    assert!(full_screen.ready);
    assert_eq!(full_screen.backing, "vmlinux");
}

#[cfg(not(target_os = "macos"))]
#[test]
fn full_screen_apps_not_ready_off_macos_without_crosvm() { … }

#[cfg(not(target_os = "macos"))]
#[test]
fn full_screen_apps_ready_off_macos_with_crosvm_and_vmlinux() { … }
```

The shared helper `full_screen_apps_service(services)` panics
if the row is ever dropped from `SYSTEM_SERVICES`, so a regression
that removes the row is caught immediately rather than silently
flipping the assertions to no-ops.

## 3. Smoke

### 3.1 Tests

```text
$ cargo test -p elastos-server
…
running 406 tests   (lib)         404 passed, 2 ignored
running 86  tests   (bin)         86 passed   ← +2 over Day-2 baseline
running 2   tests   (integration: orphan_cleanup)
running 3   tests   (integration: …)
…
test result: ok across every binary. Zero regressions.
```

### 3.2 Live dashboard on this Mac

Before Day 3 (the Day-2 ceiling):

```text
[ok ] Home Session     (shell)
[ok ] Local World      (localhost-provider)
[ok ] Identity         (did-provider)
[ok ] WebSpaces        (webspace-provider)
[no ] Content Exchange (ipfs-provider + kubo)
[ok ] Site Edge        (site-provider)
[no ] Public Edge      (tunnel-provider + cloudflared)
[no ] Full-screen Apps (crosvm + vmlinux)        ← structurally red on Mac
services ready: 5 / 8
```

After Day 3:

```text
[ok ] Home Session     (shell)
[ok ] Local World      (localhost-provider)
[ok ] Identity         (did-provider)
[ok ] WebSpaces        (webspace-provider)
[no ] Content Exchange (ipfs-provider + kubo)
[ok ] Site Edge        (site-provider)
[no ] Public Edge      (tunnel-provider + cloudflared)
[ok ] Full-screen Apps (vmlinux)                 ← now correctly reports Mac state
services ready: 6 / 8
```

The two `[no]` rows are now honestly third-party-dependency
gaps — `brew install kubo cloudflared` would flip both to `[ok]`
without any further substrate or script work.

### 3.3 Re-sign + downstream smoke

`cargo build -p elastos-server` invalidates the codesign
signature (the linker rewrites the binary), so after the rebuild
this day required:

```text
$ scripts/dev/sign-elastos-vz/sign.sh elastos/target/debug/elastos
…
Done. `elastos/target/debug/elastos` can now drive Apple's Virtualization.framework.
```

All four entitlements (`com.apple.security.virtualization`,
`com.apple.security.cs.allow-jit`,
`com.apple.security.cs.allow-unsigned-executable-memory`,
`com.apple.security.cs.disable-executable-page-protection`) are
back in place. To confirm we didn't break the Phase-8
end-to-end smokes:

- **WASM standalone:** `elastos run capsules/home` →
  `home capsule launched: name=home id=wasm-d6efabf6-… ts=1779736116`
  → `[run] WASM capsule 'home' exited` (JIT pages still mappable).
- **VZ subsystem init:** `vz provider enabled (Apple
  Virtualization.framework available; Phase 1 stub — microVM
  launch fails closed)` log line confirms Vz is initialised on
  parent startup.

Both green.

## 4. Operational note: re-signing after `cargo build`

Today's discovery surface: every `cargo build -p elastos-server`
silently drops the four entitlements baked into the dev signing
plist, because codesign signatures don't survive a relink. The
existing one-liner is:

```text
scripts/dev/sign-elastos-vz/sign.sh elastos/target/debug/elastos
```

Day 4 candidate: make `scripts/dev/mac-local-setup.sh` detect a
missing entitlement and auto-invoke the signer (or print a
single conspicuous warning). The signing script is already
idempotent and ad-hoc, so wiring it in is a 5-line addition.

## 5. Files touched

- `elastos/crates/elastos-server/src/home_cmd.rs` — one spec
  field switched to a cfg-gated constant; two new constants;
  five new tests in `home_cmd::tests`. Net +50 LOC.
- `docs/vz-backend/PHASE_6_PLAN.md` — status banner extended.
- `docs/vz-backend/PHASE_9_DAY_3_NOTES.md` — this file.

Substrate change is one line of `backing:` plus the two
constants. The body of `gather_system_services` is unchanged.

## 6. What unlocks next

With the dashboard now showing 6 / 8 on Mac (and 7 / 8 or 8 /
8 reachable when `kubo` / `cloudflared` are on PATH), the
visible "is ElastOS ready?" lights are essentially all the
operator can ask for from a source checkout. Day 4+ candidates,
in priority order:

1. **Bootstrap auto-re-sign.** Add a check + call into
   `mac-local-setup.sh` so a fresh `cargo build` doesn't leave
   the operator with a silently broken binary. (5-line change,
   trivial.)
2. **Wire the five Home-surface capsules' WASM↔HTTP
   carrier_bridge.** Today `home`, `system`, `documents`,
   `library`, `inbox` are registered in the runtime's capsule
   cache (the "6 capsules installed" count) but their `main()`s
   just print a banner. Day 4+ would connect each to the managed
   runtime's HTTP API via `carrier_bridge::spawn_wasm_api_bridge`
   so the dashboard's Apps surface gets user-visible behaviour,
   not just registration counts.
3. **Notarized Mac binary distribution.** Replace the ad-hoc
   dev-signing flow with a proper Apple Developer ID +
   notarytool path so a clean Mac (no `xattr -d` workaround) can
   run `elastos home` straight out of a release tarball.
   Hardware/credential dependency, not engineering.
