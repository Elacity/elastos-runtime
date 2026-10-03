# Installing ElastOS

## Install from the publisher

The installer looks up signed releases for Linux x86_64/aarch64 and macOS Apple silicon. Intel Mac and other OS families fail closed. The signed release determines which binaries exist for a given platform.

This checkout's installer combines installation, Home setup, and launch into one
command:

```bash
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash
```

That complete flow becomes available at this URL when the candidate installer
and signed release are published. Developers can use the [Mac source
guide](MAC.md) for source-home staging and Browser VM work.

The candidate installer detects the platform (Linux `x86_64`/`aarch64` or
macOS Apple silicon), verifies the signed release, installs Runtime, and
fetches the Home profile from the trusted publisher. It
starts Home at `http://localhost:8090/home/` and opens your browser. Keep the
terminal open while you use Home; Ctrl+C stops it. With no interactive terminal,
setup completes and prints the full command for opening Home later. The installer
uses the installed binary's full path, so editing PATH is optional.

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

### Recover an interrupted update

If Runtime reports an interrupted command-line update, run `elastos update`
again before starting Home. Runtime owns the saved transaction and verifies its
files before recovery. Keep the installation files and data in place.

A Home with an installed update controller keeps its controller Runtime and
private receipt in the data directory. If restart recovery requires that
controller, set `ELASTOS_RECOVERY_DATA` to the data directory shown in the local
message and run:

```bash
"${ELASTOS_RECOVERY_DATA}/update-controller/runtime" __update-controller \
  --receipt "${ELASTOS_RECOVERY_DATA}/update-controller/receipt.json"
```

The controller verifies its signed Runtime, retained launch settings and saved
release before it starts Home. Keep this terminal open. Initial startup has a
120-second limit; update and rollback startup have a 30-second limit. A startup
failure names the private `update-controller/runtime.log` file to inspect
before trying again. The receipt keeps Home paths, locale, desktop opener settings and Runtime launch
bindings. It also retains the configured Wallet price API key, which Wallet reads
from the environment. Other provider and Browser settings stay in their installed
private configuration files. Keep the receipt's owner-only permissions and
share only a safe error summary.

Home reuses a verified controller when its signed Runtime already matches.
If a new controller cannot fit, ordinary Home can open with Home update controls
unavailable.
Free disk space before updating. An interrupted update or uncertain controller
ownership keeps its recovery step.

These overrides are for operators:

```bash
elastos update --head-cid CID
elastos update --no-p2p --gateway GATEWAY_URL
```

Replace `CID` and `GATEWAY_URL` with the source values.

## Compare and change the release signer

Compare an announced release DID with the staged root DID in the repository
[README](../README.md#install-from-the-publisher) before installing or changing
a source. The source owner can re-trust an existing source explicitly:

```sh
elastos source add --name EXISTING_SOURCE --publisher NEW_DID
```

Runtime asks for the complete new DID. Entering another value cancels the
change. This step keeps the source's channel, install path, Carrier ticket and
gateways. After confirmation, Runtime accepts release signatures from the new
DID and refuses signatures from the former DID.

An existing Home that keeps its old source pin refuses a release signed by the
new maintainer with a signer-mismatch error. That Home continues to trust the
old key and remains exposed if a copy of that key exists. The owner must compare
the public DID and complete the explicit re-trust step before its next update.

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
