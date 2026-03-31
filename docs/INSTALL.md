# Installing ElastOS

## Canonical Install (bootstrap from the publisher URL, then Carrier)

```bash
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash
elastos setup
elastos
```

After setup, `elastos` opens the sovereign PC2 home surface. From there you can
launch chat and the other local surfaces without learning runtime nouns first.
Direct `elastos chat` remains a shortcut and auto-starts a local runtime — no
separate `elastos serve` terminal needed. Subsequent runs reuse the running
runtime automatically.

- The gateway-hosted installer carries the maintainer DID, signed release head,
  and publisher discovery metadata automatically.
- The web installer is a one-time bootstrap. After install, first-party
  `setup` and `update` use the trusted source over Carrier by default.
- Users should not need to know a HEAD CID to install or update normally.
- Native chat does not require crosvm, vmlinux, kubo, or sudo.

`elastos setup` is intentionally narrow:

- it provisions the core PC2/chat profile
- it does not silently provision every share/site/operator dependency
- broader surfaces require explicit extras or an operator profile

Useful extras:

```bash
# direct share/open
elastos setup --with kubo --with ipfs-provider --with md-viewer

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

# Setup (provisions the core PC2/chat prerequisites — no crosvm/vmlinux needed for native chat)
~/.local/bin/elastos setup

# Open PC2 home
~/.local/bin/elastos

# Or jump straight to chat
~/.local/bin/elastos chat --nick jetson

# Check for updates
~/.local/bin/elastos update --check
```

Native chat does not require KVM, crosvm, vmlinux, or sudo. For microVM
capsules, those are provisioned by setup but not required for the native chat path.

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

Capsules are **not** pre-installed. They are downloaded on-demand by the
supervisor when a command needs them (e.g., `elastos chat` downloads
chat + its provider dependencies automatically).

## Shell Policy

The shell capsule enforces capability policy. The default is secure:

- **With terminal** (interactive): `cli` mode — operator approves/denies each request
- **Without terminal** (daemon): `agent` mode — policy-file rules, built-in defaults cover standard capsules

Custom policy files can be placed at `~/.config/elastos/runtime-policy.json`.

## Trust Model

All artifacts are signed with Ed25519. The installer verifies:

1. `release-head.json` signature against the maintainer DID
2. `release.json` signature against the same DID
3. Binary and components.json SHA-256 checksums

Gateways are transport only — signatures are the trust anchor.
