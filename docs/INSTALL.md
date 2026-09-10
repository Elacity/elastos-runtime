# Installing ElastOS

## Install from the publisher

The public endpoint currently serves the older 0.1.2 Linux preview. Its
installer prints separate setup commands. The 0.7.1 installer in this checkout
combines installation, Home setup, and launch into one command:

```bash
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash
```

That complete flow becomes available at this URL when the candidate installer
and signed release are published. macOS Apple silicon downloads are still in
preparation; developers can use the [Mac source guide](MAC.md).

The candidate installer detects the platform, verifies the signed release,
installs Runtime, and fetches the Home profile from the trusted publisher. It
opens Home in the terminal. On a headless connection, setup completes and prints
the installed binary's full path for opening Home later. The installer uses that
path directly, so editing PATH is optional.

For automated provisioning or a different setup profile, use `--install-only`:

```bash
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash -s -- --install-only
```

`ELASTOS_INSTALL_ONLY=1` also selects bootstrap only. Test scripts use this
setting to own setup, launch, and cleanup; older installers ignore it and already
stop after bootstrap.

The current default Home exposes System, People, Services, Browser, Wallet,
Documents, Library, Marketplace, Archive, and Inbox. People is installed as a
separate app capsule; Home presents it but does not own its state or authority.

Native Windows is not an accepted install target today. The current product
direction is a local Linux Runtime inside WSL2 with a small Windows launcher.
See [WINDOWS.md](WINDOWS.md) for the current Windows strategy and the later
native-adapter boundary.

The installer URL bootstraps trust once. Later first-party setup and update
operations use the trusted Carrier source by default. Users do not manage a
release-head CID or gateway on the normal path.

The signed manifest determines which setup profiles are available:

- `elastos setup` installs the core Home profile.
- `elastos setup --profile demo` adds demo Apps and supporting tools.
- `elastos setup --profile operator` prepares the separate runtime used by
  `serve`, remote node control, agents, WASM or microVM `run`, and
  non-interactive capsule work.

Data `run` needs no Runtime, and interactive packaged capsules use managed Home.
Only one live host may own an ElastOS data home at a time. Do not run Home and
the operator runtime against the same home at the same time. See the
[command matrix](COMMAND_MATRIX.md) for each command lane.

### Source checkout note

A clone contains source code and `components.json`. It does not create a
trusted source relationship. A binary built from the checkout can read its
setup profiles, but it still needs a trusted source before fetching published
components.

See [Getting started](GETTING_STARTED.md#source-built-trusted-source-example)
for an explicit `source add` example. Use the public installer when testing the
published install path.

## Optional components

The Home profile in this checkout includes Documents, Library, Kubo and the
IPFS provider. Running an additional setup command for those components repeats
the default selection. A published release uses its own signed manifest.

Add site-serving tools when operating a website:

```bash
# Local site preview
elastos setup --with site-provider

# Ephemeral public site edge
elastos setup --with site-provider --with tunnel-provider --with cloudflared
```

### Content commands for operators

The current `elastos share` and CID-backed site commands use the Runtime
content provider with a local IPFS backend. Kubo and the IPFS provider remain
dependencies of that CLI path. A reduced installation needs those components;
opening a shared document also needs Documents.

The release plan's Content/Carrier journey has separate acceptance checks.
Installing the CLI backend establishes its prerequisites; cross-Runtime content
delivery needs target proof. See [Sites](SITES.md) for the site commands and
[Content capsule distribution](CONTENT_CAPSULE_DISTRIBUTION.md) for the content
contract.

## Setup and content Get are different operations

`elastos setup` is an operator/bootstrap path for installing the selected
Runtime profile from a trusted release. It is not the product contract for a
Home content catalog.

Downloadable games, GGUF models, and similar data should be published as signed
content capsules identified by the CID of their complete bundle. Home `Get`
will request a typed Runtime operation that verifies, fetches, pins, and admits
that exact capsule through the content and availability providers. A service
offer is needed only for a running provider capability, not for the content
package itself.

Until that Get contract is implemented and verified, raw `url` entries and
setup-only model downloads remain operator provisioning details. They must not
be projected as remotely installable Home catalog items. See
[Content capsule distribution](CONTENT_CAPSULE_DISTRIBUTION.md).

## Manual bootstrap

Use an explicit gateway only when the publisher URL is unavailable or when
testing release infrastructure:

```bash
EXPLICIT_GATEWAY=https://publisher.example.com

# Use the installer's published trust anchors.
curl -fsSL "${EXPLICIT_GATEWAY}/ipfs/INSTALLER_CID/install.sh" | bash

# Or supply the trust anchors explicitly.
curl -fsSL "${EXPLICIT_GATEWAY}/ipfs/INSTALLER_CID/install.sh" | bash \
  -s -- --head-cid HEAD_CID --maintainer-did MAINTAINER_DID

```

Replace the uppercase placeholders with the publisher's values. Use one gateway
and explicit trust anchors. Do not fall back to unrelated transports or
gateways.

## Jetson

The installer detects Linux `aarch64`:

```bash
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash
```

Native Home and chat run without KVM, crosvm, a guest kernel, Kubo, or `sudo`.
The default Home profile omits `crosvm` and `vmlinux`. Use an explicit profile
or source-home provisioning for microVM and Browser VM work.

The [Browser VM target](BROWSER_VM_TARGET.md) documents the target contract and
maintenance boundary. [Scripts](../scripts/README.md) maps the executable proof
commands.

Browser VM target maintenance is an operator path.
Refresh-only is not sufficient for package/dependency changes: rebuild the target and run
`scripts/browser-vm-artifact-preflight.sh`, including the complete
PipeWire/WirePlumber/GStreamer dependency set, before installation.

## Update

```bash
elastos update
elastos update --check
```

`elastos update` discovers newer signed releases through the trusted source
created during install.

These overrides are for operators:

```bash
elastos update --head-cid CID
elastos update --no-p2p --gateway GATEWAY_URL
```

Replace `CID` and `GATEWAY_URL` with the source values.

## Installed files

When XDG variables are unset, the default paths are:

| Path | Purpose |
| --- | --- |
| `~/.local/bin/elastos` | Runtime binary |
| `~/.local/share/elastos/components.json` | Installed component registry |
| `~/.local/share/elastos/sources.json` | Trusted update sources |

The publisher's signed manifest controls what `elastos setup` installs. Run
`elastos setup --list` to inspect the selected manifest's current profiles and
components before installation. The installed `components.json` records what
the selected profile installed. Do not infer parity with this development tree
from the version label or a successful setup. [state.md](../state.md) records
whether exact public-manifest parity evidence has been accepted.

## Capability policy

Runtime validates and enforces capability tokens. The built-in shell evaluates
the local approval policy for interactive and operator flows:

- In `cli` mode, the terminal asks the operator to approve or deny each request.
- In `agent` mode, the shell applies the policy file and built-in defaults.

The default policy file is
`~/.local/share/elastos/policy.json`. Set `ELASTOS_POLICY_FILE` to use a
different operator-managed file.

## Trust model

The installer verifies:

1. the `release-head.json` signature against the maintainer DID
2. the `release.json` signature against the same DID
3. the binary and `components.json` SHA-256 checksums

Gateways transport bytes. Signatures, hashes, and the maintainer DID establish
trust.

For release and target handoff commands, see the
[runtime user story checklist](RUNTIME_REPO_USER_STORY_CHECKLIST.md) and
[scripts index](../scripts/README.md). Those commands are outside the normal
install path.
