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
- A Windows launcher must start, stop, update, and health-check the Linux
  Runtime, and open `http://localhost:8090/home/`.
- Keep Runtime state in the Linux filesystem, not in mixed host paths.
- Enforce explicit storage budgets and cleanup.
- Keep localhost and passkey origin stable.
- Report Browser availability honestly. Edge is a shell surface for Home, not
  proof of a local isolated Browser capsule.

WSL2 is a host substrate, not a new capsule API and not a substitute for
capsule isolation. Capsules keep the same Runtime, Carrier, provider,
identity, and capability contracts.

Capsule, provider, and object contracts stay on the Runtime surface. WSL paths,
host ports, and shell commands stay in the host launcher.

## Linux build inside WSL2

A Windows launcher must run the Linux Runtime inside a WSL2 distro. This
section states the installer behavior that exists today and the launcher
behavior a future Windows program must provide. An executed Windows proof is
an owning issue.

Inside a distro, `uname -s` is `Linux`. `scripts/install.sh` selects the Linux
release for that architecture (`x86_64-linux` or `aarch64-linux`). It verifies
the release signature and hash before it installs the binary. It writes the
Linux data directory: `$XDG_DATA_HOME/elastos` or `~/.local/share/elastos`.
The published Linux binary is the musl target
(`x86_64-unknown-linux-musl` or `aarch64-unknown-linux-musl`).

A Windows launcher must keep `HOME` and the data directory on the distro
filesystem. It must start Home with `elastos gateway --addr localhost:8090`
and open one origin, `http://localhost:8090/home/`. The passkey relying party
id for that origin is `localhost`.

`elastos gateway` leaves the browser closed. That is the one browser-open
path for this host: the Windows launcher opens the Home origin above.
`elastos home --browser` on Linux tries `wslview` and then `xdg-open`. A
Windows launcher must not open a second origin through that command.

A Windows launcher must stop the gateway by a method the owning issue
specifies, and it must confirm the process has exited. This note does not
define a signal path.

The HTTP gateway is the Home path inside the distro. A local Browser engine
is a separate host decision. This note claims no Windows Browser engine.
`scripts/install.sh` runs `scripts/browser-vm-linux-network.py` when that
helper is installed and `/dev/kvm` exists. That helper writes systemd and
udev units.

`install.sh` stops when `uname` is not Linux or Darwin. A Windows launcher
must start the distro and run the Linux binary there.

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
