# ElastOS Runtime

ElastOS is a local-first runtime for Apps and services. Runtime checks the
authority of each caller before allowing an effect. People sign in to Home
with passkeys.

For release history, see [elastos/CHANGELOG.md](elastos/CHANGELOG.md).
[GitHub issues](https://github.com/Elacity/elastos-runtime/issues) own current
acceptance and verification. A source checkout and a published installation have
separate artifact identities and verification records.

## Current isolation and target boundary

First-party apps run as web projections in the browser's opaque sandboxed
frames. Runtime checks their capability tokens before it performs an effect.
Home can currently obtain every app's capability, so a compromised Home can
reach those apps' authority. The target limits Home to delegation and gives each
app a separate, revocable capability. The WASM Component authoring path runs in
Wasmtime with memory and fuel limits and Runtime Bus hostcalls.

Providers run as native operating-system processes with the Runtime user's
rights. Only the model provider is partly confined. The trusted shell helper
also runs as a native host process. The web Terminal is disabled by default;
host developer mode and closed guest registration are required to enable it.
An enabled Terminal runs commands with the host user's rights.

The seed operator can read hosted data, wallet keys and recovery material.
Passkeys control sign-in; stored data and keys remain accessible to the Runtime
account and root while Home is locked. Protection against hosted operators and
root, and against other software or OS users while a self-hosted Home is locked,
is the target of [hosted protection](https://github.com/Elacity/elastos-runtime/issues/209)
and [locked Home protection](https://github.com/Elacity/elastos-runtime/issues/210).
An unlocked self-hosted Home trusts its owner and their host software. Recovery
from a stolen device or profile key requires a new identity.

The [isolation plan](https://github.com/Elacity/elastos-runtime/issues/173)
records the remaining gates. Source checks describe this source tree. Accepted
installed proof binds the exact Runtime, components and app assets to the
journeys tested on that device.

## Install from the publisher

The public installer looks up signed releases for Linux x86_64/aarch64 and macOS Apple silicon. Intel Mac and other OS families fail closed.

The staged release root DID is
`did:key:z6MkgwHd2BCWe1jHMXPiR6H1q1RFPcv1YzhMbK5G1kBarbfe`. Before a staged
install, compare this DID with the `MAINTAINER_DID` value in the frozen installer.
Promotion to the live installer requires staged install and update acceptance
and operator approval.

```bash
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash
```

The source installer installs Runtime, sets up the Home profile, and opens Home.
The published installer and signed release have their own acceptance evidence
in GitHub issues. Keep
the terminal open while you use Home. Add `$HOME/.local/bin` to PATH when you
later run `elastos` from a new shell. You do not need a separate `elastos serve`
process for this path. Home is the user-facing front door to the managed
Runtime.

Only one live host may own an ElastOS data home at a time. Stop Home before
using the separate operator runtime in the same home. See [Installing
ElastOS](docs/INSTALL.md) for profiles, updates, trust verification, and
operator setup. See the [Mac staging runbook](docs/MAC.md) for source-home
staging and Browser VM work.

## Build from source

The workspace requires Rust 1.91 or newer.

```bash
cargo install just
just build
just test
```

A source build does not create a complete install. Use [Getting
started](docs/GETTING_STARTED.md) for trusted-source setup, running a source
build, and capsule development. Run `just verify` before handing off a change.

## System model

Runtime is the trusted core. Home and shells show state and collect intent.
The target limits their authority to Runtime-approved delegation. Executable
capsules request effects
through typed Runtime resources. Components use ElastOS Bus. Web projections
use narrow, capsule-scoped Runtime adapters. Both enter Runtime's authority and
routing boundary. Runtime handles core operations directly and selects a
provider for provider-backed effects. Carrier is the endpoint-authenticated
off-box transport for routes that leave the node, not the capsule API.

Self-contained host commands and explicit operator commands use their
documented paths inside the `elastos` binary. They are outside the capsule
effect path. The [architecture](docs/ARCHITECTURE.md) defines the trust
topology. The [command matrix](docs/COMMAND_MATRIX.md) defines command
ownership.

ElastOS keeps three concepts separate:

- objects are a person's documents, media, identities, sites, and other things
- Digital Capsules are complete, portable signed packages
- spaces are the rooted namespaces where objects and services resolve

Public product surfaces use "Apps." Runtime and developer documents use
"capsules." Directories under `capsules/` and `templates/` are source packages.
They become Digital Capsules only when completely packaged and signed. Runtime
admission is a separate, node-local verification decision. See the
[principles](PRINCIPLES.md) and
[architecture](docs/ARCHITECTURE.md) for the full model.

## Status and verification

Use [GitHub issues](https://github.com/Elacity/elastos-runtime/issues) for current
behavior, known gaps and proof. [Install and update #89](https://github.com/Elacity/elastos-runtime/issues/89)
owns installed release acceptance. A source checkout and a published install
have separate artifact identities; source checks alone do not prove installed
product or Browser support.

[ROADMAP.md](ROADMAP.md) links planned work;
[elastos/CHANGELOG.md](elastos/CHANGELOG.md) records release history.

For command ownership across Home and operator lanes, see the [command
matrix](docs/COMMAND_MATRIX.md).

## Repository layout

```text
elastos-runtime/
├── elastos/       # Rust runtime workspace
├── capsules/      # First-party capsule source packages and projections
├── docs/          # Guides, contracts, architecture, and runbooks
└── scripts/       # Build, verification, release, and operator tools
```

## Read next

- [Getting started](docs/GETTING_STARTED.md): install, build, and create a capsule
- [Documentation map](docs/README.md): complete guide and contract index
- [Isolation plan](https://github.com/Elacity/elastos-runtime/issues/173): current boundaries and acceptance gates
- [Principles](PRINCIPLES.md): decision constraints
- [Architecture](docs/ARCHITECTURE.md): trust and responsibility boundaries
- [Capsule authoring](docs/CAPSULE_AUTHORING.md): supported Component and web-projection paths

## License

[MIT](LICENSE)
