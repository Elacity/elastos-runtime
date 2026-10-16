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

The first honest Windows product should run the Linux Runtime locally inside
WSL2.

- Package the Runtime as a signed or importable ElastOS WSL distribution.
- Provide a small Windows launcher that starts, stops, updates, health-checks,
  and opens `http://localhost:61180/apps/home/`.
- Keep Runtime state in the Linux filesystem, not in mixed host paths.
- Enforce explicit storage budgets and cleanup.
- Keep localhost and passkey origin stable.
- Report Browser availability honestly. Edge is a shell surface for Home, not
  proof of a local isolated Browser capsule.

WSL2 is a host substrate, not a new capsule API and not a substitute for
capsule isolation. Capsules keep the same Runtime, Carrier, provider,
identity, and capability contracts.

Do not leak WSL paths, host ports, or shell commands into capsule, provider, or
object contracts.

## Linux build inside WSL2

This note is from the Linux installer and Runtime on `develop`. It was read
from code. It was not executed on a Windows machine or inside a WSL2 distro.

Inside a distro, `uname -s` is `Linux`. `scripts/install.sh` therefore selects
the Linux release for that architecture (`x86_64-linux` or `aarch64-linux`)
and writes the Linux data directory: `$XDG_DATA_HOME/elastos` or
`~/.local/share/elastos`. The published Linux binary is the musl target
(`x86_64-unknown-linux-musl` or `aarch64-unknown-linux-musl`), so the distro
glibc is not the loader. The same `elastos gateway --addr 127.0.0.1:8090`
command and `GET /healthz` body `OK` apply. `healthz` in
`elastos/crates/elastos-server/src/api/gateway_site.rs` returns that body.

The launcher keeps `HOME` and the data directory on the distro filesystem.
The host lock is an exclusive `flock` on `{data}/host-process.lock`. That lock
belongs on the Linux filesystem. A data directory on `/mnt/c` is a different
filesystem and is outside this install path.

`elastos gateway` leaves the browser closed. On Linux, `elastos home --browser`
tries `wslview` and then `xdg-open`. The Windows launcher opens
`http://127.0.0.1:8090/home/` itself and keeps that exact origin for the
passkey. `localhost` and `127.0.0.1` are different WebAuthn origins.

The Windows supervisor signals the Linux gateway process. `SIGTERM` to the
`wsl` parent, with the gateway left running inside the distro, skips the
Runtime's process-group shutdown. The launcher sends `SIGTERM` to the gateway
pid inside the distro, then waits until `/healthz` fails.

Home's HTTP gateway does not need `/dev/kvm` or systemd. Two later paths do:

- Local Browser and crosvm open `/dev/kvm` and fail closed when the device is
  absent. Nested virtualization is a separate Windows setting. Without it, the
  honest report is that the local Browser engine is unavailable and a remote
  engine is the viewer path.
- `scripts/install.sh` runs `scripts/browser-vm-linux-network.py` only when
  that helper is installed and `/dev/kvm` exists. The helper's setup writes
  systemd and udev units. A distro without systemd keeps the HTTP Home and
  leaves that Browser host setup incomplete.

Native `elastos.exe` is still absent. `install.sh` stops when `uname` is not
Linux or Darwin. A Windows launcher that wants this Runtime starts the distro
and runs the Linux binary there.

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
