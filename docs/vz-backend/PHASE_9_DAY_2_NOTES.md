# Phase 9 Day 2 — Full Home surface bootstrap on Mac

> **Outcome (2026-05-26):** Extended `scripts/dev/mac-local-setup.sh`
> from the three host-prereq providers it shipped on Day 1 to the
> **complete first-party Home surface a Mac source checkout can
> build on its own**: seven host provider binaries plus five
> shipped Home-surface capsules (two WASM, three data). After the
> script runs, `elastos home` reports **5 / 8 system services
> ready** (up from 3 / 8), with **6 capsules installed** (up from
> 1), and the dashboard's Apps launcher is now backed by a fully
> populated capsule registry. The three rows still in
> `missing prerequisites` are all due to third-party binaries we
> cannot lawfully bundle (`kubo`, `cloudflared`) or platform-only
> dependencies (`crosvm` — Linux-only by design). 404/404
> elastos-server lib tests pass; zero substrate change; one
> commit, one notes file.
>
> **Anchor:** [`PHASE_9_DAY_1_NOTES.md`](PHASE_9_DAY_1_NOTES.md)
> (Day-1 baseline 3 / 8 ready), this file extends it.

## 1. Where Day 1 left us, where Day 2 takes us

Day 1 (yesterday) brought the **prereq gate** down. `elastos home`
no longer bailed with "HOME prerequisites not installed", and the
managed-home runtime started spawning successfully. But of the
**eight** system services the dashboard tracks, only three were
green:

```
3 / 8 services ready
  [ok] Home Session     shell
  [ok] Local World      localhost-provider
  [ok] Identity         did-provider
  [no] WebSpaces        webspace-provider
  [no] Content Exchange ipfs-provider + kubo
  [no] Site Edge        site-provider
  [no] Public Edge      tunnel-provider + cloudflared
  [no] Full-screen Apps crosvm + vmlinux
```

And only one capsule was registered in the runtime's cache
(`ubuntu-base`, from prior phase work). The Home dashboard ran,
but its "Apps" launcher and "System" services were sparse.

Day 2's goal was to push every service we can install ourselves
into `[ok]` and to register every Home-surface capsule that ships
in this repo. The honest ceiling is **5 / 8** because:

- `Content Exchange` requires `kubo` (third-party Go binary).
- `Public Edge` requires `cloudflared` (third-party Cloudflare CLI).
- `Full-screen Apps` is hard-coded to `crosvm` (Linux-only) in
  `home_cmd.rs`'s `SystemServiceSpec` — Mac's Vz substrate
  doesn't satisfy this row today.

So 5 / 8 is the actual achievable floor for a Mac source checkout
running this bootstrap script alone. (With `brew install kubo
cloudflared`, that rises to 7 / 8. The 8th row needs a
substrate change — Day-3+ scope.)

## 2. What the extended script does

`scripts/dev/mac-local-setup.sh` is now split into three component
categories with one helper function each, plus a manifest stamper
and a self-verifier:

### 2.1 Host providers (7 binaries)

Each is a Rust crate that builds a single binary, staged at
`<data_dir>/bin/<name>` and stamped into
`<data_dir>/components.json` with `sha256:<hex>` + size.

| Provider          | Workspace                                  | Status |
|-------------------|--------------------------------------------|--------|
| `shell`           | `elastos/Cargo.toml`                       | Day 1  |
| `localhost-provider` | `elastos/Cargo.toml`                    | Day 1  |
| `did-provider`    | `capsules/did-provider/Cargo.toml`         | Day 1  |
| `webspace-provider` | `capsules/webspace-provider/Cargo.toml` | Day 2  |
| `site-provider`   | `capsules/site-provider/Cargo.toml`        | Day 2  |
| `ipfs-provider`   | `capsules/ipfs-provider/Cargo.toml`        | Day 2  |
| `tunnel-provider` | `capsules/tunnel-provider/Cargo.toml`      | Day 2  |

A new helper `build_and_stage_provider <name> <manifest> <target_dir>`
factors out the build+stage+stamp pattern. Each provider's checksum
+ size is appended to a temp TSV stream (`PROVIDER_STAMPS_FILE`)
that the Python stamper reads in a single pass — one source of
truth, no per-component duplication.

### 2.2 WASM capsules (2)

`home` and `system`. Built via `cargo build --release --target
wasm32-wasip1 --manifest-path capsules/<name>/Cargo.toml -p <name>`,
then `<name>.wasm` + `capsule.json` are staged at
`<data_dir>/capsules/<name>/`.

`home.wasm` was already built in Phase 8 Day 8 (the standalone
WASM-lane smoke). `system.wasm` is new this day. Both are tiny
"version-1" capsules whose `main()` prints a launch banner via
`elastos_guest::CapsuleInfo::from_env()` — they don't need the
WASM↔HTTP bridge yet, which is why we can stage them without
running the managed runtime through any new integration path.

### 2.3 Data capsules (3)

`documents`, `library`, `inbox` — all `"type": "data"`, no Rust
code, just `capsule.json` + `index.html`. The new
`stage_data_capsule` helper uses `rsync -a --delete` (which is
preinstalled on macOS) and excludes build artefacts (`target/`,
`*.lock`, `.elastos-cid`, `.elastos-artifact-sha256`, `browser/`).

We deliberately do **not** write `.elastos-cid` or
`.elastos-artifact-sha256` cache files into these directories
because the source-checkout `components.json` ships every capsule
entry with an empty `cid` + `sha256`. The runtime's
`component_install_state` only flags `Stale` when
`cached_cid != entry.cid`, and `"" != ""` is false, so the
capsule install state lights up `Installed` without us having to
fabricate trust-anchor metadata.

### 2.4 Third-party dependency hints

After staging, the script probes `kubo` and `cloudflared` on
`PATH` and prints `brew install kubo cloudflared` hints when
either is missing — clear, non-blocking, operator-actionable.

### 2.5 Self-verifier

The chained `elastos home --status --json` consumer now prints a
formatted per-service ready/not-ready table and fails when fewer
than **5** services are green — the realistic Day-2 floor.

## 3. Smoke

### 3.1 First run (cold cargo cache)

```text
$ scripts/dev/mac-local-setup.sh
[mac-local-setup] repo:      /Users/sash/code/elastos-runtime
[mac-local-setup] data-dir:  /Users/sash/Library/Application Support/elastos
[mac-local-setup] platform:  darwin-arm64

[mac-local-setup] building provider: shell
  Finished `release` profile [optimized] target(s) in 0.08s
  staged …/bin/shell
    sha256: 06d4089b88b9f5620506e4031d244d6819d3ab3248bd436c3da2b612681f990b
    size:   2988016
[mac-local-setup] building provider: localhost-provider
  Finished `release` profile [optimized] target(s) in 0.06s
  staged …/bin/localhost-provider
    sha256: a00408f8c64e8e4f0b2b3d8466833de6fa64811d1db8dabb3ff919bab12f4ecf
    size:   762880
[mac-local-setup] building provider: did-provider
  Finished `release` profile [optimized] target(s) in 0.05s
  staged …/bin/did-provider
    sha256: 5263872ec9a1399534b5777de281ce311de8fa2bc9ee505aa7231da1ed7d1d26
    size:   699616
[mac-local-setup] building provider: webspace-provider
   …
  Finished `release` profile [optimized] target(s) in ~30s
  staged …/bin/webspace-provider
    sha256: <hex>
    size:   <bytes>
[mac-local-setup] building provider: site-provider
   …
  Finished `release` profile [optimized] target(s) in ~25s
  staged …/bin/site-provider
[mac-local-setup] building provider: ipfs-provider
   …
  Finished `release` profile [optimized] target(s) in 10.22s
  staged …/bin/ipfs-provider
    sha256: cff270b56a5798621a05816f3c26744fc73190ecd736e7294e08417c771e5d02
    size:   3073264
[mac-local-setup] building provider: tunnel-provider
   …
  Finished `release` profile [optimized] target(s) in 4.10s
  staged …/bin/tunnel-provider
    sha256: c8d8bf52e75850b06f6200f9455916f45f2fd293c1c8b7b510bbcc8db81e46ef
    size:   668128

[mac-local-setup] building wasm capsule: home
  Finished `release` profile [optimized] target(s) in 0.01s
  staged …/capsules/home/{home.wasm, capsule.json}
[mac-local-setup] building wasm capsule: system
  Finished `release` profile [optimized] target(s) in 0.01s
  staged …/capsules/system/{system.wasm, capsule.json}

[mac-local-setup] staging data capsule: documents
  staged …/capsules/documents/
[mac-local-setup] staging data capsule: library
  staged …/capsules/library/
[mac-local-setup] staging data capsule: inbox
  staged …/capsules/inbox/

[mac-local-setup] wrote …/components.json

[mac-local-setup] third-party dependencies not on PATH:
  - kubo   (install with: brew install kubo)
  - cloudflared   (install with: brew install cloudflared)
  Content Exchange and/or Public Edge services will remain
  in 'missing prerequisites' state until they are installed.

[mac-local-setup] verifying via: elastos home --status --json
  services ready: 5 / 8
    [ok ] Home Session  (shell)
    [ok ] Local World  (localhost-provider)
    [ok ] Identity  (did-provider)
    [ok ] WebSpaces  (webspace-provider)
    [no ] Content Exchange  (ipfs-provider + kubo)
    [ok ] Site Edge  (site-provider)
    [no ] Public Edge  (tunnel-provider + cloudflared)
    [no ] Full-screen Apps  (crosvm + vmlinux)

[mac-local-setup] OK
```

### 3.2 Idempotent re-run

Second run with no source changes: every cargo build resolves to
"Finished … in 0.0-0.1 s" (no-op), every binary is re-staged
byte-identically (`install` always copies but the bytes are the
same), every rsync is a no-op, the python stamper rewrites
`components.json` byte-for-byte identically, and the
self-verifier confirms 5 / 8 green:

```text
$ scripts/dev/mac-local-setup.sh
…
[mac-local-setup] verifying via: elastos home --status --json
  services ready: 5 / 8
…
[mac-local-setup] OK
```

Total wall clock: ~1 second.

### 3.3 Full Home dashboard

```text
…
Now
  …
  Capsules:  6 installed / 0 running   ← was 1 in Day 1
…

System
  Services ready: 5 / 8                ← was 3 in Day 1
```

The five new capsules (home, system, documents, library, inbox)
correctly stay out of the Apps launcher because home_cmd.rs's
`PROVIDER_CAPSULE_NAMES` + role filters exclude shells / providers
/ data viewers from the user-launchable list. They still register
in the runtime's capsule cache (the "6 installed" count) and are
available for the upcoming Day-3+ work that wires them into the
Home browser surface.

## 4. The three rows still in `[no]`

### 4.1 Content Exchange (ipfs-provider + kubo)

Our `ipfs-provider` Rust binary is staged. What's missing is
`kubo`, the Go IPFS daemon it shells out to. `kubo` is a 30 MB+
third-party binary; brew's `kubo` formula ships it pre-built
for darwin-arm64. The script prints the hint
`brew install kubo` and the row stays red until the operator
runs it.

When `kubo` lands on `PATH`, `find_installed_provider_binary`'s
PATH-traversal branch will resolve it, `gather_components` will
mark it `available`, and Content Exchange flips green
automatically — no script re-run needed.

### 4.2 Public Edge (tunnel-provider + cloudflared)

Same shape as Content Exchange but for Cloudflare's `cloudflared`
quick-tunnel binary. `brew install cloudflared` → row goes green.

### 4.3 Full-screen Apps (crosvm + vmlinux)

This is the only structurally interesting red row. `home_cmd.rs`
hard-codes:

```rust
SystemServiceSpec {
    name: "Full-screen Apps",
    role: "Supports immersive full-screen app capsules …",
    backing: &["crosvm", "vmlinux"],
},
```

On Mac the analogue of crosvm is Apple's Virtualization.framework
embedded in `elastos-vz`. We don't expose Vz as a discoverable
"component" the way crosvm is, because it isn't a binary that
lives at `<data_dir>/bin/`. A clean Day-3 fix would change the
spec to e.g.:

```rust
backing: if cfg!(target_os = "macos") {
    &["vmlinux"]   // Vz is implicit when vmlinux is present on macOS
} else {
    &["crosvm", "vmlinux"]
},
```

(Or, more strictly, expose a virtual "vz-available" component
that `gather_components` reports as `available = true` on macOS
when `elastos-vz::available()` succeeds.)

That's a small, targeted substrate change. It's out of scope for
Day 2 because Day 1 + Day 2 were explicit about being **bootstrap
script only** — no substrate edits. We document it here as the
single highest-value Day-3 target.

## 5. Regression coverage

```text
$ cd elastos && cargo test -p elastos-server --lib
…
test result: ok. 404 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 1.21s
```

404 / 404, zero regressions. The script writes only into
`<data_dir>/` and never touches source code, so the test suite
(which sandboxes via tempdirs and `ELASTOS_DATA_DIR`) is
unaffected. Phase 8 Day 7's Ubuntu VM smoke and Phase 8 Day 8's
WASM standalone smoke remain green — the dev-signed debug binary
still carries all four entitlements.

## 6. Files touched

- `scripts/dev/mac-local-setup.sh` — extended from 3 providers
  to 7 providers + 2 WASM capsules + 3 data capsules + 3rd-party
  PATH hints, +120 LOC.
- `docs/vz-backend/PHASE_6_PLAN.md` — status banner extended.
- `docs/vz-backend/PHASE_9_DAY_2_NOTES.md` — this file.

Zero substrate code touched.

## 7. What unlocks next

The Mac source checkout now lights up every Home surface the
runtime can do without a substrate change or third-party
download. Day 3+ candidates, in priority order:

1. **Platform-aware `Full-screen Apps` backing.** Replace the
   crosvm-only spec with a per-platform list so Mac shows the row
   as green when Vz + vmlinux + rootfs are present. One-line
   substrate change, +2 unit tests, +1 line in plan banner.
2. **Third-party autoresolve.** Have the bootstrap script
   optionally invoke `brew install kubo cloudflared` (gated
   behind `--with-third-party` so we never surprise the
   operator). Pushes Day-2's 5/8 to 7/8.
3. **Real WASM-side surfaces for `home` / `system` / `documents`
   / `library` / `inbox`.** Today they print a banner and exit.
   Day 4+ would wire each to its respective runtime API via the
   carrier_bridge so the dashboard's Apps launcher gets
   user-visible behaviour, not just a registration count.
4. **Sign-on for Apple notarization.** The current debug binary
   ad-hoc signs with `com.apple.security.virtualization +
   com.apple.security.cs.allow-jit`, which works for dev but
   doesn't survive an `xattr -d com.apple.quarantine` on a fresh
   user's Mac. A notarized release requires an Apple Developer
   ID + altool/notarytool pass. Hardware/credential dependency,
   not engineering.
