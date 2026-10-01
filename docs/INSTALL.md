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
components before installation. The installer and updater retain the verified
publisher manifest; setup can stamp installed file metadata into its local
registry. Profile membership alone therefore does not establish which files
are installed. Exact artifact and manifest checks establish installation
parity; the owning issue records the accepted proof.

## Proposed complete release layout

This contract is proposed for installation-layout review. It describes the
intended Unix installer and updater behavior; the current installation still
replaces the binary, manifest and support assets in sequence. Filesystem
migration and installed acceptance require a reviewed implementation.

Runtime will select one verified release set for each Home launch. The installer,
updater and offline migration will use one installation coordinator. Home keeps
ownership of identity, passkeys, configuration and user data throughout an
update.

### Release and Home roots

Let `DATA` be the existing platform application-data directory: on Linux,
`${XDG_DATA_HOME:-$HOME/.local/share}/elastos`; on macOS,
`$HOME/Library/Application Support/elastos`. Let `BIN` be the existing installed
command path, including an explicit installer `--install-dir` or trusted
source's `install_path`. Both paths are absolute and remain stable.

| Proposed path | Ownership and purpose |
| --- | --- |
| `DATA/install/sets/<set-id>/` | One immutable, verified release set |
| `DATA/install/active` | Relative symlink to the selected set |
| `DATA/install/staged` | Relative symlink to the single prepared set, when present |
| `DATA/install/rollback` | Relative symlink to one verified prior set during a named update window |
| `DATA/install/transaction.json` | Owner-only commit and recovery record |
| `DATA/installation.lock` | Shared coordinator lock for install, update and migration |
| `BIN` | Stable launcher for this Home's selected Runtime |
| `DATA/sources.json` | Home-owned trust and transport configuration |
| `DATA/backups/<migration-id>/` | Separate, bounded protected-root migration backup |

A set contains Runtime under `bin/elastos`, the verified `components.json`,
the selected profile's provider binaries and complete first-party capsule
bundles, and its release-owned support files. It also contains the consumed
signed `release-head.json`, `release.json`, and any manifest-pinned catalog,
under `metadata/`, including the exact signed-input components bytes when setup
materializes a local registry. A receipt binds their hashes, platform, selected
profile, artifact member paths and installed version. Its `set-id` is the SHA-256 of
that receipt's canonical artifact inventory. A changed profile creates a new
set rather than modifying the active set.

All set members and pointer targets stay within `DATA/install`. Archive
admission checks paths, links and hashes before extraction becomes eligible for
activation. Manifest installation paths such as `bin/<provider>` and
`capsules/<app>` are relative to the selected set. Home-owned caches, downloaded
content, model inventory, keys, policy and provider configuration remain in
their existing data locations. Only the release-owned files listed in the
receipt enter a set.

The Home's `ElastOS/SystemServices/Publisher` directory keeps its own publishing
output. Consumed release metadata belongs to the set's `metadata/` directory.
The installer and updater will stop writing consumed releases into Publisher.

### Command and component resolution

The launcher at `BIN` resolves `active` once, checks the set receipt, and executes
that set's Runtime with the existing Home data root. Runtime pins this same set
root for manifest, provider, capsule and helper lookup until the process ends.
Checksum verification uses the pinned set's manifest and artifact receipts:
archive hashes bind fetched packages, and member hashes bind extracted files.
All component lookups use the pinned set for the life of the process.

This requires explicit changes to the existing `binaries.rs` provider lookup and
`setup.rs` manifest/install-path resolution. Source checkout and explicit
operator override paths keep their own provenance and verification. A release
set admits members only through its receipt. Existing data-root component
paths are legacy inputs for migration, rather than fallback release members.

For a legacy installation, the coordinator first imports only the current
Runtime and release-owned component files into a verified legacy set. It pins
that set as active, then atomically replaces `BIN` with the launcher. A refusal
before this preparation completes preserves the old executable and Home.
Custom command paths can be on another filesystem: launcher preparation uses
a temporary file beside `BIN`, while the release commit stays on the data
filesystem. A rerun reads the transaction and resumes the same set.

### Lock order and commit

The lock order is installation lock, host-process lock, then protected-object
mutation lock. Each host startup takes the installation lock while it resolves
and pins a set and acquires `host-process.lock`; it then releases the
installation lock. A running host submits update work to the coordinator and
releases its host ownership before offline commit. It keeps using its pinned
set while download and verification are in progress.

The coordinator holds the installation lock through preparation and commit.
It verifies the complete selected set and writes a durable `prepared` record
before requesting the existing host to stop. It checks release of host
ownership and cleanup of owned children before the offline phase. Automatic
restart is a separate host lifecycle operation.

The current protected-root migration helper acquires the host-process lock
itself and requires a backup directly beneath `DATA/backups`. The proposed
coordinator acquires host ownership once for migration and commit. Implementation
must give that helper an internal entry point which accepts the already-held
host guard; its standalone CLI entry keeps its own acquisition. Both then take
the protected-object mutation lock. This preserves the existing lock boundary
and avoids recursive host locking. Migration backup size and recovery are
admitted before the helper mutates Home data. A failed migration restores or
recovers its own journaled data before an older Runtime can run again.

With host ownership held, the coordinator flushes the verified set and writes
the previous and next set IDs into the transaction. It creates a replacement
relative symlink beside `active`, then uses one same-filesystem atomic rename
to select the new set and flushes the parent directory. This pointer rename is
the release commit and selects Runtime and all release members together.

The active receipt is authoritative for the installed release. The coordinator
then reconciles only installed-version/head fields in `sources.json`, preserving
Home trust and transport edits, and marks the transaction committed. Every
install, update and host startup first recovers an unfinished transaction under
the installation lock. Recovery after pointer commit keeps the selected Runtime,
manifest and installed-version record consistent.

### Interruption, disk and retention

Before commit, interruption leaves the prior active set selected. A retry
checks the staged receipt and resumes verified work or removes only its partial
set. After pointer commit, recovery validates the active receipt, completes
source-record reconciliation and accounts for migration recovery before host
startup. An invalid committed set keeps Home stopped and gives the operator a
specific recovery action; rollback also requires compatible or restored Home
data. Reverting a pointer alone does not undo a data migration.

Space admission covers downloads, extraction, the selected set and any
migration backup on each affected volume. It preserves at least 10% free space,
or the operator's higher reserve. The same reserve is checked during staging
and before commit. Low space stops preparation while the active set remains
usable.

Retention permits one active set, one staged set and one verified rollback set
for a named update window. The transaction records each set's owner, byte size
and cleanup condition. A further update waits until the prior window closes.
Migration backups have separate ownership and expiry; release cleanup preserves
Home data and Publisher output. Completion requires exact installed journey,
interruption/retry and low-space evidence before this layout becomes the
installation contract.

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
