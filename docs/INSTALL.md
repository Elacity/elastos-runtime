# Installing ElastOS

## Canonical Install (bootstrap from the publisher URL, then Carrier)

The public installer is the current Linux `x86_64`/`aarch64` preview. macOS uses
source-home staging for now; see [MAC.md](MAC.md).

```bash
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash
elastos setup
elastos
```

After setup, `elastos` opens Home. The default Home profile includes System,
People, Services, Browser, Wallet, Documents, Library, Marketplace, Archive, and
Inbox without requiring users to learn runtime nouns first. Direct
`elastos chat` remains a shortcut and auto-starts a local runtime — no separate
`elastos serve` terminal needed. Subsequent runs reuse the running runtime
automatically.

- The gateway-hosted installer carries the maintainer DID, signed release head,
  and publisher discovery metadata automatically.
- The web installer is a one-time bootstrap. After install, first-party
  `setup` and `update` use the trusted source over Carrier by default.
- Users should not need to know a HEAD CID to install or update normally.
- Native chat does not require crosvm, vmlinux, kubo, or sudo.

Source checkout note:

- A GitHub clone gives you source plus `components.json`. It does not stamp a trusted source into `sources.json`.
- Source-built binaries can inspect setup profiles from the checkout, but `elastos setup` still needs either a published install or an explicitly created/added trusted source.
- For a concrete source-checkout `source add` example, see [GETTING_STARTED.md](GETTING_STARTED.md#source-built-trusted-source-example).

`elastos setup` is intentionally narrow:

- it provisions the core Home profile; native chat is part of the runtime binary
- it does not silently provision every share/site/operator dependency
- broader surfaces require explicit extras or an operator profile

Useful extras:

```bash
# content-backed share/open with the local Kubo backend
elastos setup --with kubo --with ipfs-provider --with documents

# local site serving / browser preview helper
elastos setup --with site-provider

# ephemeral public site serving
elastos setup --with site-provider --with tunnel-provider --with cloudflared

# CID-backed site publish / activate on a fresh install
elastos setup --with kubo --with ipfs-provider
```

## Manual Install (explicit operator/debug installer bootstrap)

```bash
EXPLICIT_GATEWAY=https://publisher.example.com

# Fetch the published installer bundle through one explicitly chosen IPFS gateway.
curl -fsSL "${EXPLICIT_GATEWAY}/ipfs/<INSTALLER_CID>/install.sh" | bash

# Or with explicit trust anchors
curl -fsSL "${EXPLICIT_GATEWAY}/ipfs/<INSTALLER_CID>/install.sh" | bash \
  -s -- --head-cid <HEAD_CID> --maintainer-did <DID>

elastos setup
```

Use this only when the canonical bootstrap publisher URL is unavailable or when
you are doing release/debug work. It is not the preferred user workflow and
should not be the default path you hand to external testers. The operator path
is explicit on purpose: choose one gateway, know why you are using it, and do
not silently switch transports.

## Jetson (aarch64)

Prerequisites: Jetson Linux.

```bash
# Install (auto-detects aarch64)
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash

# Setup (provisions the core Home profile — no crosvm/vmlinux needed for native chat)
~/.local/bin/elastos setup

# Open Home
~/.local/bin/elastos

# Or jump straight to chat
~/.local/bin/elastos chat --nick jetson

# Check for updates
~/.local/bin/elastos update --check
```

Native chat does not require KVM, crosvm, vmlinux, or sudo. For microVM
capsules, those are provisioned by setup but not required for the native chat path.

Browser VM target maintenance is an operator path, not a normal user install
step. On an already-provisioned Jetson Browser VM target, check for drift first:

```bash
cd <target-source-checkout>
HOME=<target-home> \
XDG_DATA_HOME=<target-xdg-data-home> \
scripts/browser-vm-target-refresh.sh --verify-only
```

If drift is reported and the source checkout is the reviewed release branch,
refresh Browser VM helpers without a full Rust/WASM build:

```bash
HOME=<target-home> \
XDG_DATA_HOME=<target-xdg-data-home> \
scripts/browser-vm-target-refresh.sh

HOME=<target-home> \
XDG_DATA_HOME=<target-xdg-data-home> \
scripts/linux-source-home-restart.sh \
  --home <target-home> \
  --xdg-data-home <target-xdg-data-home> \
  --addr <target-loopback-addr> \
  --json-out <target-elastos-data-dir>/logs/gateway-restart.json
```

This refresh path updates installed Browser VM scripts plus guest script files
inside existing initrd/rootfs artifacts. If the guest-control bridge binary has
already been built for the Linux guest architecture, pass
`--guest-control-bridge-bin <path>` to refresh it inside the rootfs with backup
and verification. Refresh-only is not sufficient for package/dependency changes,
Chromium wrapper changes, Selkies Python package patches, or other complete
guest image changes; rebuild/restage the Browser VM rootfs, then run
`scripts/browser-vm-artifact-preflight.sh` before claiming runtime parity.

Then prove source/install/artifact/runtime parity from the reconciliation host:

```bash
scripts/jetson-browser-runtime-audit.mjs \
  --host <target-host> \
  --user <target-user> \
  --data-dir <target-elastos-data-dir> \
  --source-dir <target-source-checkout> \
  --require-parity
```

Use full `elastos setup` or `scripts/setup-source-home.sh` only when you are
building/provisioning the runtime or changing broader VM artifacts such as the
kernel, crosvm/VZ supervisor, native proxy binary, Chromium wrapper, Selkies app
patch, PipeWire/WirePlumber/GStreamer dependency set, or guest image contract.
After full source-home setup on a Linux target, run
`scripts/linux-source-home-restart.sh` before testing the target. Setup can
replace the gateway binary, and the old live gateway intentionally exits when it
detects that stale executable state.

## Updating

```bash
elastos update                          # Canonical path (Carrier P2P discovery)
elastos update --check                  # Check only, don't install
elastos update --head-cid <cid>         # Manual/operator override
elastos update --no-p2p --gateway <url> # Operator escape hatch (not the canonical path)
```

`elastos update` should discover newer signed releases through the trusted source
relationship established at install time. Explicit gateway and HEAD CID flags are
operator/debug tools, not the primary product path.

## Handoff Verification

Normal users do not need these commands. Use them before handing a branch,
published candidate, or target device to another tester:

```bash
# Current checkout: source/review proof
git diff --check
node scripts/home-entropy-check.mjs
node scripts/browser-entropy-check.mjs
bash scripts/check-wci-alignment.sh

# Current checkout: installed-style command behavior in a clean home
just candidate-command-audit

# Current 0.5.0 candidate through the canonical public installer/source path.
# Requires a staged or published 0.5.0-compatible manifest with the current
# home profile and checksummed artifacts.
ELASTOS_PUBLISHER_GATEWAY=<candidate-url> \
ELASTOS_BIN_OVERRIDE="$PWD/elastos/target/release/elastos" \
  bash scripts/public-install-identity-smoke.sh
ELASTOS_PUBLISHER_GATEWAY=<candidate-url> \
ELASTOS_BIN_OVERRIDE="$PWD/elastos/target/release/elastos" \
  bash scripts/public-install-home-frontdoor-smoke.sh

# Source/local Carrier setup proof before a candidate gateway exists
scripts/local-carrier-setup-smoke.sh

# Final public install path after publishing 0.5.0
bash scripts/public-install-identity-smoke.sh
bash scripts/public-install-home-frontdoor-smoke.sh

# Stricter Carrier relay-only setup path, when publisher relay health is under review
ELASTOS_PUBLIC_INSTALL_FORCE_RELAY_ONLY=1 bash scripts/public-install-home-frontdoor-smoke.sh

# Candidate publisher/gateway after staging a 0.5.0 artifact set
ELASTOS_PUBLISHER_GATEWAY=<candidate-url> bash scripts/public-install-home-frontdoor-smoke.sh

# Target closeout from the operator host when a Home-authorized Browser page is active
scripts/jetson-browser-runtime-audit.mjs \
  --host <target-host> \
  --user <target-user> \
  --data-dir <target-elastos-data-dir> \
  --source-dir <target-source-checkout> \
  --require-parity \
  --min-active-crosvm-seconds 3600
```

The manual installed-device check is still separate: on each target host, run
`elastos setup`, open `elastos`, visit System, Documents, Library, Inbox,
People, and Services, launch and close at least one app, and return Home
cleanly.
Source-home and seed-node proofs do not replace this installed-path check.

## Publisher Notes

This document is for install and update behavior, not the internal release ceremony.

- The canonical public gateway is `https://elastos.elacitylabs.com`.
- Published installers are stamped so `elastos update` can discover newer signed releases without manual flags.
- Release and ceremony scripts are internal maintainer tooling and are not part of the public install contract.

## What Gets Installed

These are the default paths when XDG variables are unset. Runtime data honors `XDG_DATA_HOME`.

| Path | Description |
|------|-------------|
| `~/.local/bin/elastos` | Runtime binary |
| `${XDG_DATA_HOME:-~/.local/share}/elastos/components.json` | Capsule registry |
| `${XDG_DATA_HOME:-~/.local/share}/elastos/sources.json` | Trusted source config (for updates) |

`elastos setup` installs the components selected by the active profile. The
default `home` profile installs the Home front door and the visible Home
surfaces: System, People, Services, Browser, Wallet, Documents, Library,
Marketplace, Archive, and Inbox. People is Home-owned state and UI, not a
separate capsule. The profile also installs the local provider components needed
by those surfaces, including DID, webspace, wallet/chain, Browser Engine, Net,
and Exit providers. Demo chat-room, GBA, public-edge tunnel/cloudflared, IPFS/
Kubo, and protected-content DRM providers are installed only when you choose a
broader profile or add them with `--with`.

## Policy Capsule

The Home/orchestrator capsule enforces capability policy. The default is secure:

- **With terminal** (interactive): `cli` mode — operator approves/denies each request
- **Without terminal** (daemon): `agent` mode — policy-file rules, built-in defaults cover standard capsules

Custom policy files can live at `~/.local/share/elastos/policy.json`, or you can point to another file with `ELASTOS_POLICY_FILE`.

## Trust Model

All artifacts are signed with Ed25519. The installer verifies:

1. `release-head.json` signature against the maintainer DID
2. `release.json` signature against the same DID
3. Binary and components.json SHA-256 checksums

Gateways are transport only — signatures are the trust anchor.
