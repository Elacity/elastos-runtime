# ElastOS Runtime

ElastOS is a local-first runtime for Apps and services. Runtime checks the
authority of each caller before allowing an effect. People sign in to Home
with passkeys.

For released versions, supported installation targets and known limitations,
see [state.md](state.md). A source checkout and a published installation have
separate artifact identities and verification records.

## Install the Linux preview

```bash
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash
export PATH="$HOME/.local/bin:$PATH"
elastos setup
elastos
```

Running `elastos` opens Home. The default setup installs Home and its core Apps.
You do not need a separate `elastos serve` process for this path.
Home is the user-facing front door to the managed Runtime.

Only one live host may own an ElastOS data home at a time. Stop Home before
using the separate operator runtime in the same home. See [Installing
ElastOS](docs/INSTALL.md) for profiles, updates, trust verification, and
operator setup.

macOS currently uses source-home staging rather than the public installer. See
the [Mac staging runbook](docs/MAC.md).

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

Runtime is the trusted core. Home and shells show state and collect intent, but
they cannot grant themselves authority. Executable capsules request effects
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

Use [state.md](state.md) as the authority for current behavior and known gaps.
It distinguishes implemented behavior from source-only paths and unverified
product claims. Browser source and proof tooling alone do not establish
complete Browser product support.

Use [TASKS.md](TASKS.md) for open work, [ROADMAP.md](ROADMAP.md) for future
direction, and [elastos/CHANGELOG.md](elastos/CHANGELOG.md) for release history.

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
- [State](state.md): verified behavior and known gaps
- [Principles](PRINCIPLES.md): decision constraints
- [Architecture](docs/ARCHITECTURE.md): trust and responsibility boundaries
- [Capsule authoring](docs/CAPSULE_AUTHORING.md): supported Component and web-projection paths

## License

[MIT](LICENSE)
