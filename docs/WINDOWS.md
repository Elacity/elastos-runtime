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
  Runtime, and opens one Home origin: `http://localhost:8090/home/`.
- The launcher keeps Runtime state on the distro filesystem.
- The launcher enforces explicit storage budgets and cleanup.
- The launcher keeps the localhost passkey origin stable.
- The launcher reports Browser availability honestly. Edge is a shell surface
  for Home. It is not proof of a local isolated Browser capsule.

WSL2 is a host substrate. It is not a new capsule API and it is not a
substitute for capsule isolation. Capsules keep the same Runtime, Carrier,
provider, identity, and capability contracts.

Capsule, provider, and object contracts stay on the Runtime surface. WSL paths,
host ports, and shell commands stay in the host launcher.

## Linux build inside WSL2

A Windows launcher runs the Linux Runtime inside a WSL2 distro. This section
cites the installer and gateway behavior that exists today, and it names the
gaps a launcher still owns.

Inside a distro, `uname -s` is `Linux`. `scripts/install.sh` `detect_platform`
selects `x86_64-linux` or `aarch64-linux`. `scripts/prepare-release-platform.sh`
builds those releases as `x86_64-unknown-linux-musl` or
`aarch64-unknown-linux-musl`.

`scripts/install.sh` `verify_signature` checks the Ed25519 signature on
`release-head.json` and `release.json` against `MAINTAINER_DID` before it
installs. `sha256_check` then checks the binary against the signed SHA-256.
`scripts/install-bootstrap-test.py` covers a tampered release
(`test_signature_and_binding_failures_reject_before_artifacts_in_populated_installation`),
a foreign trust anchor in that same test, and a binary whose bytes differ
(`test_corrupt_artifacts_fail_before_changes_then_clean_rerun_installs`,
which expects `SHA-256 mismatch`). This note does not claim the installer
refuses an older release that still carries a valid signature. That rollback
check is a gap.

The installer writes `$XDG_DATA_HOME/elastos` or `~/.local/share/elastos`.
The launcher keeps `HOME` and that data directory on the distro filesystem.
`install.sh` does not reject a data root under `/mnt`. That guard is a gap.

The one Home origin is `http://localhost:8090/home/`.

- `BROWSER_HOME_PORT` in `elastos/crates/elastos-server/src/update_controller.rs`
  is `8090`.
- `HOME_ROUTE` in `elastos/crates/elastos-server/src/api/gateway.rs` is
  `/home/`.
- `elastos home --browser` binds `localhost:8090` and opens the gateway Home
  URL (`gateway_entry.rs` `run_browser_home`). The gateway builds that URL as
  `{api}/home/` (`gateway_server.rs`).
- `/home`, `/apps/home`, and `/apps/home/` redirect to `/home/`
  (`redirect_home_root` in `api/browser_capsules.rs`).
- `elastos gateway` without `--addr` binds `127.0.0.1:8090` (`main.rs`, the
  `Gateway` command). A launcher that wants the localhost origin passes
  `--addr localhost:8090`.

The passkey relying party id for `http://localhost:8090/home/` is `localhost`.
WebAuthn scopes that id by host, so any port on `localhost` can request an
assertion for it. The Windows launcher opens only `http://localhost:8090/home/`.

`elastos gateway` leaves the browser closed. The Windows launcher owns the
browser-open path and opens the Home origin above. `elastos home --browser`
still calls `open_browser` in `main.rs`, which tries `wslview` and then
`xdg-open` with the URL it was given. This note does not make that command
refuse under WSL. A second open from that command is a gap.

The runtime update command is `elastos update` (`Commands::Update` in
`main.rs`). `scripts/install.sh` `show_ready` prints that command. A Windows
launcher calls `elastos update`. It does not ship a second updater. This note
does not define the stop signal. Issue #265 owns that path. `wsl --shutdown`
stops the distro without a Runtime stop.

The decision for WSL is that the installer skips
`scripts/browser-vm-linux-network.py` and prints that skip. This note claims
no Windows Browser engine. This note does not change `install.sh`. Today
`prepare_browser_linux_host` still runs that helper when the file is installed
and `/dev/kvm` exists, and the helper can write systemd and udev units. The
visible skip is open work on issue #265.

`install.sh` stops when `uname` is not Linux or Darwin. A Windows launcher
starts the distro and runs the Linux binary there.

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
- stable localhost and passkey origin
- honest Browser availability
- installed artifact receipt and source receipt
