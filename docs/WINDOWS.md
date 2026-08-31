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
