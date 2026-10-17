# Windows strategy

This document records the current Windows truth and the next honest product
direction. It does not claim that native Windows support exists today.

## Current truth

- Native Windows is not an accepted Runtime target.
- The experimental Windows checks on the Home review branch exposed Unix-only
  assumptions. Windows remains outside the accepted platform matrix.
- The first confirmed portability blocker is overly broad non-WASI Unix code in
  `elastos-guest`, including termios, serial, PTY, and FIFO paths.
- That is the first blocker, not the only blocker. Unix sockets, permissions,
  signals, process groups, PTYs, and Browser isolation also need explicit host
  adapters.

Windows support must keep the same Runtime, Carrier, provider, identity, and
Browser contracts. Capsules must not branch on Windows or learn host topology.

## First Windows product: WSL2

The first honest Windows product runs the Linux Runtime locally inside WSL2.
The executed proof for that product is
[issue #265](https://github.com/Elacity/elastos-runtime/issues/265).

- Package the Runtime as a signed or importable ElastOS WSL distribution.
- A Windows launcher starts, stops, updates, and health-checks the Linux
  Runtime, and opens one Home origin. The
  [launcher contract](LAUNCHER_CONTRACT.md#readiness-and-home-url) owns that
  URL.
- The launcher keeps Runtime state on the distro filesystem.
- The launcher enforces explicit storage budgets and cleanup.
- The launcher keeps the passkey origin stable.
- The launcher reports Browser availability honestly. Edge is a shell surface
  for Home. It is not proof of a local isolated Browser capsule.

WSL2 is a host substrate. It is not a new capsule API and it is not a
substitute for capsule isolation. Capsules keep the same Runtime, Carrier,
provider, identity, and capability contracts.

Capsule, provider, and object contracts stay on the Runtime surface. WSL paths,
host ports, and shell commands stay in the host launcher.

## Linux build inside WSL2

A Windows launcher runs the Linux Runtime inside a WSL2 distro. This section
records what the installer and Runtime do there today. Each statement cites
the code. If a line number moves, use the named function. Rust paths are
relative to `elastos/crates/elastos-server/src/`.

### Platform

Inside a distro, `uname -s` is `Linux`. `detect_platform`
(`scripts/install.sh:317-336`) selects `x86_64-linux` or `aarch64-linux`.
`scripts/prepare-release-platform.sh:107-108` builds those releases for
`x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`.
`installer_data_dir` (`scripts/install.sh:300-315`) stops on a system name
that is not `Linux` or `Darwin`. A Windows launcher starts the distro and runs
the Linux installer and binary there.

### Home origin

The [launcher contract](LAUNCHER_CONTRACT.md#readiness-and-home-url) is the
one source for the Home URL that a launcher opens and for the passkey origin.
This note does not repeat that value. It records what the Runtime code serves:

- `elastos home --browser` binds the host name `localhost` at
  `BROWSER_HOME_PORT` (`gateway_entry.rs:17-31`,
  `update_controller.rs:2288-2289`).
- The gateway keeps the host name that it was given in its URL
  (`api/gateway_server.rs:96-101`) and adds `/home/`
  (`api/gateway_server.rs:117`). `HOME_ROUTE` is `/home/`
  (`api/gateway.rs:440`).
- `/home`, `/apps/home`, and `/apps/home/` redirect permanently to `/home/`
  (`api/gateway.rs:1222-1235`, `redirect_home_root` in
  `api/browser_capsules.rs:58-64`).
- After an update restart, Runtime prints the Home URL with the host name
  `localhost` (`update_controller.rs:1049`). The update controller accepts a
  ready Home at `BROWSER_HOME_PORT` and `/home/` on `localhost`, `127.0.0.1`,
  or `[::1]` (`approved_home_url`, `update_controller.rs:2291-2299`).
- `elastos gateway` is a hidden operator command. Without `--addr` it binds
  `127.0.0.1` at the same port (`main.rs:438-441`).

`localhost` and `127.0.0.1` are different browser origins. A passkey that is
registered on one origin does not sign in on the other. The desktop launcher
and the launcher contract do not use the same host name for the Home origin
today. That is an open gap. The team decides the canonical origin, and this
note does not choose one.

`docs/INSTALL.md` describes `elastos home --browser`. The source-build guides
`docs/MAC.md` and `docs/HOME_LOCAL_SETUP.md` use a developer gateway on a
different port. That is a developer lane, not the installed product.

`elastos home --browser` opens a browser itself. `browser_home_ready`
(`gateway_entry.rs:33-40`) prints the URL and calls `open_browser`
(`main.rs:2268-2283`). On Linux, `open_browser` tries `wslview` and then
`xdg-open`. It skips the open after an update restart
(`ELASTOS_UPDATE_RESTART=1`). There is no flag that stops this open, so a
launcher that also opens Home makes a second tab. That is a gap.

### Release verification

The installer trusts one maintainer key. The publish step writes that
`did:key` into `install.sh` as `MAINTAINER_DID`
(`scripts/install.sh:45-48`). `ELASTOS_MAINTAINER_DID` or `--maintainer-did`
replaces it (`scripts/install.sh:45`, `scripts/install.sh:1128`). The
installer stops when the value is empty or is not a `did:key`
(`scripts/install.sh:1167-1172`). The signature check is only as strong as the
`install.sh` copy that carries this key. A Windows launcher must check the
SHA-256 of `install.sh` against its own signed record before it runs it, and
must not set `ELASTOS_MAINTAINER_DID`.

Before the installer changes any file, it does these checks:

1. `verify_signature` (`scripts/install.sh:678-825`) checks the Ed25519
   signature over the domain and the canonical payload. It also checks that
   the signer is the pinned `MAINTAINER_DID`. The installer runs it on
   `release-head.json` (`scripts/install.sh:1286`) and on `release.json`
   (`scripts/install.sh:1313`).
2. `validate_release_identity` (`scripts/install.sh:827-863`) checks that the
   signed head names the SHA-256 of the exact `release.json` bytes.
3. `sha256_check` (`scripts/install.sh:356-364`) checks the downloaded binary
   against the SHA-256 in the signed `release.json`
   (`scripts/install.sh:1354`).

These tests in `scripts/install-bootstrap-test.py` feed a bad input and check
that the installer stops and changes nothing:

- `test_signature_and_binding_failures_reject_before_artifacts_in_populated_installation`
  (line 1235): a release with one changed field, a head bound to another
  release, and a foreign trust anchor. The first and last expect
  `Signature verification FAILED`.
- `test_corrupt_artifacts_fail_before_changes_then_clean_rerun_installs`
  (line 991): a truncated binary and a binary with different bytes. Both
  expect `SHA-256 mismatch`.
- `test_altered_message_signature_and_key` (line 215) and
  `test_binding_precedes_artifacts` (line 731) cover the Ed25519 check and
  the head-to-release binding.

On an existing installation, the release writer refuses a release that is
older than the installed release, and a release from another channel
(`install_release`, `update.rs:505-513`). The Rust test
`older_installer_after_update_is_refused_without_changes` (`update.rs:5067`)
covers the older release. A new installation has no installed release to
compare. It accepts any release that the pinned key signed. A launcher that
needs a version floor on a new installation must supply it. That is a gap.

### Data location

The installer writes `$XDG_DATA_HOME/elastos` or `~/.local/share/elastos`
(`installer_data_dir`, `scripts/install.sh:300-315`). The launcher keeps
`HOME` and that data directory on the distro filesystem. `install.sh` does not
reject a data directory under `/mnt`. That guard is a gap.

### Update path

The signed release head names its channel. The installer reads it, accepts
`stable`, `canary`, or `jetson-test` (`scripts/install.sh:84`,
`scripts/install.sh:1290-1293`), and records it as the channel of the trusted
source in `sources.json` (`scripts/install.sh:1406-1425`). The publisher
gateway that the installer reads sets which channel a new installation gets.

- `elastos update` (`Commands::Update`, `main.rs:550-570`) is the update
  command. `show_ready` prints it after installation
  (`scripts/install.sh:885-904`).
- `elastos update` refuses a signed release that is older than the installed
  release (`update.rs:1233-1237`). The release writer keeps the installed
  version as a floor (`update.rs:505-512`; test
  `force_keeps_signed_installed_version_as_downgrade_floor`,
  `update.rs:4081`). This note does not describe a rollback path.
- `verify_source_channel` (`update.rs:170-183`) accepts only the subscribed
  channel. A `canary` subscription also accepts `stable` releases from the
  same publisher.
- `elastos source switch-channel <channel>` changes the subscription
  (`sources.rs:465-475`, `sources.rs:713-719`).
- A second run of `install.sh` also updates. It uses the same release writer
  and the same refusals as above.

A Windows launcher calls `elastos update`. It does not ship a second updater.
This note does not define the stop signal. Issue #265 owns that path.
`wsl --shutdown` stops the distro without a Runtime stop.

### Browser on WSL2

Runtime offers Browser only on `darwin-arm64` (`browser_available`,
`setup/browser_config.rs:166-168`). On every Linux host, WSL2 included,
Browser fails closed:

- `elastos setup` removes the Browser host files that a release installs
  (`retire_host`, `setup/browser_config.rs:300-355`). The removed files
  include `scripts/browser-vm-linux-network.py`. When a Browser capsule
  folder exists, setup also writes an empty Engine selection and prints
  `Browser is not available on this computer yet.`
  (`setup/browser_config.rs:301-306`, `setup/browser_config.rs:352-354`).
  Without that folder, setup prints no Browser line.
- The Home catalog leaves Browser out
  (`api/gateway_capsule_catalog/read_model.rs:44`,
  `api/browser_capsules.rs:431-433`).
- The installer runs `elastos setup` before its Linux host step
  (`scripts/install.sh:950-952`). `prepare_browser_linux_host`
  (`scripts/install.sh:908-937`) returns when the helper file is absent
  (`scripts/install.sh:914`). Thus the installer asks for no `sudo`, and no
  systemd unit, udev rule, TAP device, or KVM access rule is written.
- The Rust test `linux_setup_installs_no_browser_host_helpers_or_image`
  (`setup/browser_config.rs:808`) checks this for `linux-amd64` and
  `linux-arm64`.

This result does not depend on `/dev/kvm`. This note claims no Windows Browser
Engine.

If an earlier release ran the root helper on a host, `retire_host` removes
only the files in the data directory. The root-owned copy in
`/usr/local/lib/elastos`, the systemd unit, the udev rule, and the KVM access
rule stay until an administrator runs the helper `remove` action. That is a
gap.

## Later native Windows direction

Native Windows remains later host-adapter work behind the same Runtime and
Browser contracts:

- Unix sockets to Windows named pipes with strict ACLs
- PTY to ConPTY
- process groups and signals to Job Objects and Windows control events
- Unix file permissions to Windows ACLs
- reviewed WHPX or Hyper-V adapters where host virtualization is required
- a Browser adapter such as WebView2 or CEF behind the existing Browser, Net,
  and Exit ABI, with OS-level network-denial proof

Native Windows must not introduce a second Runtime authority model, a second
provider contract, or an unrestricted browser fallback.

## Acceptance for the first WSL proof

Before Windows support is called product-ready, prove:

- a fresh Windows installation without developer tools
- install, start, stop, restart, update, and Home open
- Recovery Kit, Profile, Wallet, People, and Chat
- same-device and cross-device Carrier behavior where applicable
- bounded disk usage and explicit cleanup
- no orphan processes after stop
- stable Home and passkey origin
- honest Browser availability
- installed artifact receipt and source receipt
