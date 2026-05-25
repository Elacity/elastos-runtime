# Phase 9 Day 1 — Mac source-checkout bootstrap for `elastos home`

> **Outcome (2026-05-26):** Brought `elastos home` up on a Mac
> source checkout without going through the trusted-source
> Carrier installer flow. Net: a single new dev-only script,
> `scripts/dev/mac-local-setup.sh`, that builds + stages the
> three host-prereq providers (`shell`, `localhost-provider`,
> `did-provider`), writes a stamped local `components.json`,
> and self-verifies the result by chaining into
> `elastos home --status --json`. After running it on a fresh
> Mac, `elastos home` (no flags) **launches the full TUI**:
> identity minted, managed-home runtime spawned, Carrier
> bootstrap ready, all eight UI sections rendering.
> Zero substrate change — Day 1 is bootstrap-only, lives
> entirely under `scripts/dev/`.
>
> **Anchor:** [`PHASE_6_PLAN.md`](PHASE_6_PLAN.md) (status
> banner), [`PHASE_8_DAY_8_NOTES.md`](PHASE_8_DAY_8_NOTES.md)
> (closed Phase 8: real ElastOS WASM capsule on Mac).

## 1. What we were solving

After Phase 8 closed, the Mac user could:

- `elastos run ubuntu-base` → real Ubuntu 22.04 LTS shell.
- `elastos run capsules/home` → 19-line WASM "home"
  capsule prints a launch banner and exits.

What the Mac user could **not** do is the actual end-user
experience: `elastos home` — the front-door TUI Linux users
get the moment they install the stamped publisher build.

A first empirical probe on this Mac (2026-05-26):

```text
$ elastos home
Error: HOME prerequisites not installed: localhost-provider, shell, did-provider

Run first:

  elastos setup

Then try again.
```

So the obvious next move was `elastos setup`. That bails too:

```text
$ elastos setup
ElastOS v0.2.0-dev — setup for darwin-arm64
Using default setup profile: home …

Components to install:
  - shell
  - localhost-provider
  - did-provider
  - webspace-provider
  - home-cli
  - home
  - system
  - documents
  - library
  - inbox

[install] shell …
  Resolving shell from elastos://artifact/shell-darwin-arm64...
  Trying elastos://artifact/shell-darwin-arm64 via trusted source over Carrier...
Error: Trusted source Carrier fetch failed for shell …
`elastos setup` installs first-party artifacts from a trusted source over Carrier.
For a published install, run the stamped installer first.
For a source checkout, create your own trusted source or add one with `elastos source add ...`.
```

In other words: on a fresh Mac with **no canonical install
at `~/Library/Application Support/elastos/`**, `elastos setup`
intentionally refuses to bootstrap itself — the trusted source
must already be known to the host. This is the right default
for end-users (refuse to fetch untrusted code) and the wrong
default for source-checkout dev (the source IS the truth).

The Linux escape hatch already exists:
`scripts/home-demo-local.sh` does exactly this kind of
bootstrap from a workspace. But it leans on Linux-only tools:

- `getent passwd "$(id -un)" | cut -d: -f6` — no `getent` on Mac.
- `sha256sum` — Mac ships BSD `shasum -a 256` (different output format).
- `stat -c '%s'` — Mac's BSD `stat` uses `-f '%z'`.
- It also calls `scripts/install.sh`, which fundamentally
  requires a pre-stamped publisher trusted source.

So `home-demo-local.sh` is not usable on Mac. We need a
small Mac-native equivalent.

## 2. Decision: write `scripts/dev/mac-local-setup.sh`

**Scope.** Build the **three host providers checked by the
home-prereq gate** (`shell`, `localhost-provider`,
`did-provider`), and write a local `components.json` whose
`darwin-arm64` platform entries carry the actual `sha256:`
checksums + file sizes of the staged binaries — which is
exactly what `verify_installed_component_binary` validates
before the runtime spawns each provider.

**Out of scope for Day 1.** The other 5 components in the
`home` profile (`webspace-provider`, `home-cli`, `home`,
`system`, `documents`, `library`, `inbox`) are
**optional** from the home-prereq perspective: the runtime
won't refuse to launch over them — they just show as
`[no] missing prerequisites` in the dashboard's Services
section. Day 2+ can extend the bootstrap to install them.

**Anchor — where the prereq gate lives:**

```rust
// elastos/crates/elastos-server/src/runtime_control.rs
let mut missing = Vec::new();
for name in &["localhost-provider", "shell", "did-provider"] {
    if crate::binaries::find_installed_provider_binary(name).is_none() {
        missing.push(*name);
    }
}
if !missing.is_empty() {
    anyhow::bail!(
        "{} prerequisites not installed: {}\n\n\
         Run first:\n\n\
         \x20 elastos setup\n\n\
         Then try again.",
        surface_name.to_ascii_uppercase(),
        missing.join(", ")
    );
}
```

**Anchor — where verification lives:**

```rust
// elastos/crates/elastos-server/src/setup.rs
pub fn verify_installed_component_binary(
    data_dir: &Path,
    name: &str,
    path: &Path,
) -> anyhow::Result<String> {
    …
    let manifest_path = install_root.join("components.json");
    …
    let checksum = platform_info
        .checksum
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!(…))?;
    if !file_matches_checksum(path, checksum)? {
        anyhow::bail!(…);
    }
    Ok(checksum.to_string())
}
```

**`file_matches_checksum`** strips a `sha256:` prefix and
hex-compares the file hash. So our manifest must use the
`sha256:<hex>` shape, not a bare hex string.

## 3. The script

`scripts/dev/mac-local-setup.sh` (new). The three concrete
moves:

### 3.1 Build

`shell` and `localhost-provider` are members of the **elastos
workspace** at `elastos/Cargo.toml`, so `cargo build -p shell
-p localhost-provider --release --manifest-path
elastos/Cargo.toml` builds them into
`elastos/target/release/`.

`did-provider` is its **own** crate at
`capsules/did-provider/Cargo.toml` (with `[workspace]` at the
bottom to detach it from the parent elastos workspace), so
`cargo build -p did-provider --release --manifest-path
capsules/did-provider/Cargo.toml` builds it into
`capsules/did-provider/target/release/`.

### 3.2 Stage

Each binary is installed (with `install -m 0755`) at
`<data_dir>/bin/<name>`, where `<data_dir>` is what the runtime
resolves via `default_data_dir()` (see
`elastos-server/src/sources.rs`):

```rust
pub fn default_data_dir() -> PathBuf {
    if let Ok(override_dir) = std::env::var("ELASTOS_DATA_DIR") {
        if !override_dir.is_empty() {
            return PathBuf::from(override_dir);
        }
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("elastos")
}
```

On Mac that's `~/Library/Application Support/elastos`.

For each staged binary the script computes:

- `shasum -a 256 "$dest" | awk '{print $1}'` — sha256 hex.
- `stat -f '%z' "$dest"` — byte size.

These are the BSD-flavor commands; the Linux flow uses
GNU `sha256sum` / `stat -c`. Both produce the same logical
values; the script keeps to BSD-only here so it works
out-of-the-box on a clean macOS install.

### 3.3 Stamp the manifest

A tiny inline `python3` script reads the source-checkout
`components.json`, sets `external.<name>.platforms.darwin-arm64.checksum`
to `sha256:<hex>` and `…size` to the byte count for each of
the three providers, and writes the result to
`<data_dir>/components.json`.

`load_manifest()` in `elastos/crates/elastos-server/src/setup.rs`
resolves manifest paths in this order:

1. `<data_dir>/components.json` ← what we just wrote.
2. `<exe-dir>/components.json`.
3. `<repo-root>/components.json` (source-checkout fallback).

So `<data_dir>/components.json` takes priority and the runtime
sees our stamped local manifest.

### 3.4 Self-verify

After staging + stamping, the script chains into
`elastos home --status --json`, filters the JSON snapshot for
any system service whose `backing` is one of the three host
providers and whose `ready` is `false`, prints any failures,
and exits 1 if found. So the script either reports success
(`all three host providers report ready.`) or surfaces a
specific failure the operator can act on.

### 3.5 Idempotency

`install -m 0755 src dest` always re-copies, but since the
hash of the binary is fully determined by the build inputs,
re-running the script when the source hasn't changed produces:

- identical staged files,
- identical `STAGED_SHAS` / `STAGED_SIZES`,
- an identical stamped `components.json`.

The script doesn't try to short-circuit the build — `cargo
build` is the idempotency layer there, and a no-op cargo build
on this Mac takes ~0.1 s.

## 4. Smoke

### 4.1 Before

```text
$ elastos/target/debug/elastos home
Error: HOME prerequisites not installed: localhost-provider, shell, did-provider

Run first:

  elastos setup

Then try again.
```

### 4.2 First run of `mac-local-setup.sh`

```text
$ scripts/dev/mac-local-setup.sh
[mac-local-setup] repo:      /Users/sash/code/elastos-runtime
[mac-local-setup] data-dir:  /Users/sash/Library/Application Support/elastos
[mac-local-setup] platform:  darwin-arm64

[mac-local-setup] building shell (manifest=…/elastos/Cargo.toml)
    Finished `release` profile [optimized] target(s) in 0.08s
[mac-local-setup] building localhost-provider (manifest=…/elastos/Cargo.toml)
    Finished `release` profile [optimized] target(s) in 0.06s
[mac-local-setup] building did-provider (manifest=…/capsules/did-provider/Cargo.toml)
    Finished `release` profile [optimized] target(s) in 0.05s

[mac-local-setup] staged …/elastos/bin/shell
  sha256: 06d4089b88b9f5620506e4031d244d6819d3ab3248bd436c3da2b612681f990b
  size:   2988016
[mac-local-setup] staged …/elastos/bin/localhost-provider
  sha256: a00408f8c64e8e4f0b2b3d8466833de6fa64811d1db8dabb3ff919bab12f4ecf
  size:   762880
[mac-local-setup] staged …/elastos/bin/did-provider
  sha256: 5263872ec9a1399534b5777de281ce311de8fa2bc9ee505aa7231da1ed7d1d26
  size:   699616

[mac-local-setup] wrote …/elastos/components.json

[mac-local-setup] verifying via: elastos home --status --json
[mac-local-setup] all three host providers report ready.

[mac-local-setup] OK
```

### 4.3 `elastos home --status`

```text
ElastOS Home
  Version:   0.2.0-dev
  …
System
  …
  Services:
    Home Session       [ok]  installed
      backing: shell
    Local World        [ok]  installed
      backing: localhost-provider
    Identity           [ok]  installed
      backing: did-provider
    WebSpaces          [no]  missing prerequisites
    Content Exchange   [no]  missing prerequisites
    Site Edge          [no]  missing prerequisites
    Public Edge        [no]  missing prerequisites
```

3 / 8 services ready, exactly the three the script provisions.
The other 5 are `[no] missing prerequisites` — Day-1 scope,
no runtime startup blocker.

### 4.4 `elastos home` (full TUI)

Running the full dashboard (with `q` piped on stdin so it
renders into the line-dashboard branch and accepts the quit
intent):

```text
[2J[HElastOS Home
A small-device home for people, spaces, apps, and system trust.
Version: runtime 0.2.0-dev  home 0.1.0-dev  installed (none)

Now
  User:      sash
  Nick:      sash
  Identity:  did:key:z6Mki45sT…jrjTnNWogWx3oXqHEq
  Network:   Carrier bootstrap ready; waiting for another participant
  MyWebSite: not staged locally
  Spaces:    MyWebSite, Public, Local, WebSpaces
  Capsules:  1 installed / 0 running
  Source:    no trusted source configured

Start Here
  1. Chat [ready]
     Open native chat, send a message, and return here when you exit.
     elastos chat
  2. MyWebSite [blocked]
     …
  3. Updates [blocked]
     …

Needs Attention
  MyWebSite is empty. Stage a local directory with `elastos site stage <dir>`.
  No trusted release source is configured yet, so update flows stay manual.

Inbox
  Attention: 0 waiting / 0 unread

People
  You        sash
  Nick       sash
  Identity   did:key:z6Mki45sT…jrjTnNWogWx3oXqHEq
  Network    Carrier bootstrap ready; waiting for another participant
  Profile    ready
  Chat       ready
  Peers      0 endpoints reachable
  Ticket     pmrgk3teobxws3tuomrd…itun5ygsyzchjxhk3dmpu

Spaces
  MyWebSite  not staged locally
  Public     0 shared channels ready to open
  Local      scratch space for temporary work and session state
  WebSpaces  named handles into content, peers, identity, and AI

Apps
  Communication:
    Chat [ready]
    Chat Room [idle]

System
  Runtime    managed-home
  Identity   ready
  Trust      no trusted source configured
  Updates    blocked (no trusted source configured yet; …)
  Inbox      0 attention · 0 unread
  Services   3 / 8 ready
  Roots      ElastOS · PC2Host

  Services ready: 3 / 8

Choose an action number, `r` to refresh, `q` to exit Home, `?` for help.
Select action (number, r refresh, q exit, ? help):
```

That's the **full ElastOS Home TUI** running on a Mac source
checkout — not a stub, not a fixture, the same surface a
Linux user gets after the stamped install.

What this proves end-to-end on Mac:

- The three host providers (Rust binaries) build, stage, and
  pass `verify_installed_component_binary`.
- The managed-home runtime spawns as a child `elastos serve`
  process from `runtime_control::ensure_managed_runtime` and
  the parent successfully attaches to it.
- The HTTP API the Home dashboard polls (snapshot writer,
  intent reader) is reachable.
- The Carrier P2P stack bootstraps (`Carrier bootstrap ready;
  waiting for another participant`).
- The `did:key:` identity is minted by the staged
  `did-provider`.
- The localhost virtual filesystem (`localhost://MyWebSite`,
  `localhost://Public`, etc.) is served by the staged
  `localhost-provider`.
- The Phase-8 JIT entitlements + Vz entitlements coexist on
  the dev-signed binary without conflict — the home managed
  runtime never needs Vz at idle, but it doesn't crash
  either.

### 4.5 Idempotency

Second run with no source changes:

```text
$ scripts/dev/mac-local-setup.sh
…
[mac-local-setup] building shell …
    Finished `release` profile [optimized] target(s) in 0.08s
…
[mac-local-setup] staged …/bin/shell
  sha256: 06d4089b88b9f5620506e4031d244d6819d3ab3248bd436c3da2b612681f990b  ← unchanged
  size:   2988016
…
[mac-local-setup] all three host providers report ready.

[mac-local-setup] OK
```

Same checksums, same sizes, manifest re-written byte-for-byte
identically.

## 5. Regression coverage

```text
$ cd elastos && cargo test -p elastos-server --lib
…
test result: ok. 404 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 1.34s
```

404/404, no regressions. The script writes only into
`<data_dir>/` (the user's macOS data dir), so the test
suite (which sandboxes via tempdirs and `ELASTOS_DATA_DIR`)
is unaffected.

Also confirmed Phase 8 Day 7 + Day 8 smokes are still
green — `elastos run capsules/home` (standalone WASM lane)
and the Ubuntu VM smoke remain unchanged; the debug binary
is still signed with all four entitlements
(`com.apple.security.virtualization` +
`com.apple.security.cs.allow-jit` +
`com.apple.security.cs.allow-unsigned-executable-memory` +
`com.apple.security.cs.disable-executable-page-protection`).

## 6. Mental-model anchors for the next day

- **Day-1 scope was the prereq gate.** Only three providers
  block `elastos home` startup. The other five
  `[no] missing prerequisites` (webspace-provider,
  ipfs-provider, site-provider, tunnel-provider) are feature
  gates, not startup gates.
- **The capsule providers shipping with the OS** (home,
  system, documents, library, inbox) are WASM capsules that
  the runtime downloads via the trusted-source flow at
  setup time. On Mac we built one of them (`home.wasm`) in
  Phase 8 Day 8 by hand; the other four would need the same
  treatment to light up the full Home surface. That's Day-2+
  work — a generic "build the in-repo WASM capsules and stage
  them under `<data_dir>/capsules/<name>/`" extension to
  this same script.
- **The trusted-source story is intentionally absent here.**
  This script is **dev-only**. The end-user install path is
  the stamped `install.sh` Carrier flow, which doesn't need
  any of this. A production-shaped equivalent would be a
  publisher-DID-signed `darwin-arm64` release of these three
  binaries served through the same Carrier source as the
  Linux ones.

## 7. Files touched

- `scripts/dev/mac-local-setup.sh` — new, +200 lines,
  +x mode.
- `docs/vz-backend/PHASE_6_PLAN.md` — status banner extended.
- `docs/vz-backend/PHASE_9_DAY_1_NOTES.md` — this file.

Zero substrate code touched.
