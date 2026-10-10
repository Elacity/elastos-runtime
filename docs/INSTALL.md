# Installing ElastOS

## Current isolation and target boundary

First-party apps run as web projections in the browser's opaque sandboxed
frames. Runtime checks each app's signed launch token and actor before it performs an effect.
Home can currently obtain every app's capability, so a compromised Home can
reach those apps' authority. The target limits Home to delegation and gives each
app a separate, revocable capability. The WASM Component authoring path runs in
Wasmtime with memory and fuel limits and Runtime Bus hostcalls.

Providers run as native operating-system processes with the Runtime user's
rights. Only the model provider is partly confined. The trusted shell helper
also runs as a native host process. The web Terminal is disabled by default;
host developer mode and closed guest registration are required to enable it.
An enabled Terminal runs commands with the host user's rights.

The [data, keys and backups](#data-keys-and-backups) section explains current
host access and the protection target for hosted and locked self-hosted Homes.

The [isolation plan](https://github.com/Elacity/elastos-runtime/issues/173)
records the remaining gates. Source checks describe this source tree. Accepted
installed proof binds the exact Runtime, components and app assets to the
journeys tested on that device.

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
Linux/crosvm Browser has a separate host network setup path that requires
administrator access. The installed host adapter determines the launch-time
privileges. The default Home profile omits `crosvm` and `vmlinux`. This release
publishes them for no platform, so microVM capsules are not available in it.

The [Browser VM target](BROWSER_VM_TARGET.md) documents the target contract and
maintenance boundary. [Scripts](../scripts/README.md) maps the executable proof
commands.

Browser VM target maintenance is an operator path.
Refresh-only is not sufficient for package/dependency changes: rebuild the target and run
`scripts/browser-vm-artifact-preflight.sh`, including the complete
PipeWire/WirePlumber/GStreamer dependency set, before installation.

## How updates reach your Home

Updates are pull only. Nothing is pushed to your Home. The publisher's seed
keeps the signed release, and your Home asks for it.

**When Home checks.** Home checks only while its System page is open. System
asks every 5 seconds while it is visible (every 2 seconds while Home restarts)
and every 30 seconds while it is hidden, and the Runtime reuses a check that is
less than 30 seconds old. Home does not check when the Runtime starts, on a
timer or when System is closed. Home itself shows no update badge yet; open
System to see an update.

**What a check does.** The Runtime opens one Carrier connection to the trusted
source saved in `sources.json` during install. It asks for the latest signed
release head, fetches the head and the release by CID (a source that gives no
separate head CID sends the head file by name), and checks both signatures, the
release hash, the channel and the version order. An older
release is refused. The `gateways`, `discovery_uri` and `ipns_name` fields in
`sources.json` are shown but not used to find updates. If the Carrier
connection fails, the check fails.

**What System shows.** System shows the new version and its release notes.

**Approve and restart.** You approve the update with your passkey for that
exact release. The Runtime downloads the binary and components by CID over
Carrier (a component listed without a CID comes by its release name), stages
everything beside the running Home and restarts Home. A download fails only
after 30 seconds without data.

**If it fails.** If the new release does not start, the previous release is
restored.

**Terminal.** From a terminal, check or install in place:

```bash
elastos update --check
elastos update
```

`elastos upgrade` does the same as `elastos update`. HTTP is used only when you
pass `--gateway` or `--no-p2p`, and by the first download in `install.sh`.

Runtime keeps the verified signed pair in `installation/release-head.json`
and `installation/release.json` within its data directory. Publisher owns its
separate publication files. For an older installation, Runtime migrates the
signed pair after it verifies the trusted source, binary, components and installed
support under the installation lock. An interrupted transaction completes its
original recovery before that migration. If the saved pair requires repair,
keep the files in place and follow Runtime's operator repair step.

Running `install.sh` again uses the same installation lock and journal. Before it
stops Runtime, it restores an interrupted install and refuses an older release,
another channel, a pending Home update, a second writer and an installed Runtime
without a readable `sources.json`. Until an interrupted install is restored,
Home does not start and asks you to run `install.sh` again.

How releases get to the seed is in
[VERSIONING.md](VERSIONING.md#publishing-a-release).

### Undo an update

Use Undo if an update causes a problem and you need the previous release. Undo
is in the terminal only. Before an update, run `elastos source show` and save
the full `Head CID:` value. That command shows the current head; after the
update, it shows the new head. If it shows `unknown`, get the previous signed
head CID from the publisher.

To restore the previous release, run:

```bash
elastos update --rollback-to <previous head CID>
```

Replace `<previous head CID>` with the saved CID. A plain update to an older
release is refused. Undo needs the network: it fetches the changed parts of the
previous release again by CID from the trusted source, so the source must be
reachable. Undo keeps your identity, accounts, and user data, including data
written after the update.

For a legacy Home, the first update to a new publisher key uses the installer's
re-trust step. After that step, Undo accepts only heads signed by the current
trusted key. A head signed by the former key is refused and the installation
stays unchanged. Ask the publisher for a previous release signed by the current
key if you need to recover across that first update.

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

The maintainer release DID is
`did:key:z6MkgwHd2BCWe1jHMXPiR6H1q1RFPcv1YzhMbK5G1kBarbfe`. Compare the
installer's `Maintainer DID:` line with the complete DID in the repository
[README](../README.md#install-from-the-publisher).

An existing Home with the old source pin refuses a release signed by the new
maintainer. On the Runtimes that still trust the old key, `elastos update`
reports one line:

```text
Error: Signer DID mismatch: trusted set = ["<old DID>"], got did:key:z6MkgwHd2BCWe1jHMXPiR6H1q1RFPcv1YzhMbK5G1kBarbfe
```

Newer Runtimes add a second line: `This release is signed by a new publisher
key. Run the publisher's install.sh to trust it; this installation was not
changed.`

The trusted set shows your Home's current DID. To re-trust once, run the
publisher's installer over the existing installation, using the same command
as a fresh install:

```sh
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash
```

The installer trusts the new DID, installs the release and keeps your existing
identity, accounts and user files. Later updates accept release signatures from
the new DID and refuse signatures from the former DID. Homes that already trust
this DID can use normal updates.
Until you re-trust, Home still trusts the old key and remains exposed if a copy
of that key exists.

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
from the version label or a successful setup. [Install/update acceptance](https://github.com/Elacity/elastos-runtime/issues/89)
owns exact public-manifest parity evidence.

## Data, keys and backups

Runtime stores keys next to the data they protect in its data home. A full
backup of that home contains all keys, including wallet keys. Protect the backup
with the same care as the running Home. A Recovery Kit is sensitive recovery
material; keep it under your own control.

Use hosted accounts only for public demos. Keep wallets, private data and
recovery material on your own device until
[hosted protection](https://github.com/Elacity/elastos-runtime/issues/209) passes
its acceptance gate. The seed operator can read hosted data, wallet keys and
recovery phrases stored on the seed. Host-side encryption with keys stored
beside the data leaves those secrets accessible to that operator.

Passkeys control sign-in. Locking Home stops UI use; the Runtime account and root
retain access to stored data and keys. The protection target covers hosted
operators and root, and other software or OS users while a self-hosted Home is
locked. An unlocked self-hosted Home trusts its owner and their host software.
Recovery can restore lost access with valid recovery material. A stolen device
or profile key requires a new identity.

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
