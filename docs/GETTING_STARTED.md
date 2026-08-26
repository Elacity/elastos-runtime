# Getting started with ElastOS Runtime

## Install the Linux preview

The public binary installer is the current Linux `x86_64`/`aarch64` preview.
macOS uses source-home staging; see the [Mac runbook](MAC.md).

```bash
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash
export PATH="$HOME/.local/bin:$PATH"
elastos setup
elastos
```

After setup, `elastos` opens Home. The
[installation guide](INSTALL.md#installed-files) explains how to inspect the
selected signed manifest and installed component registry. A public install
receives only the components in that manifest. Do not infer that it matches this
0.6 development tree. [state.md](../state.md) records whether exact
publication-parity evidence has been accepted. The normal Home path does not
need a separate `elastos serve` process.

The current default Home exposes System, People, Services, Browser, Wallet,
Documents, Library, Marketplace, Archive, and Inbox.

Only one live host may own an ElastOS data home at a time. Do not run
`elastos` and `elastos serve` against the same home at the same time. The
[command matrix](COMMAND_MATRIX.md) shows whether a command owns a Runtime,
reuses one, or needs none.

Useful next commands:

```bash
elastos chat --nick alice
elastos update --check
```

The default setup installs the core Home profile. Add content publishing tools
only when you need them:

```bash
elastos setup --with kubo --with ipfs-provider --with documents
elastos share README.md
elastos open elastos://CID
```

Replace `CID` with the value returned by `elastos share`.

The demo profile adds more Apps and tools. The operator profile supports
`serve`, remote node control, agents, WASM or microVM `run`, and non-interactive
capsule work. Data `run` and interactive packaged capsules use the lanes in the
command matrix.
[Installing ElastOS](INSTALL.md) documents profiles, trust, and updates.
[Sites](SITES.md) documents local site staging and publication.

## Build from source

Prerequisites:

- Rust 1.91 or newer
- Git
- `just`, installed with `cargo install just`
- Linux with KVM only when working on crosvm or microVM paths

Build and test:

```bash
cargo install just
just build
just test
```

After the build, check the release binary:

```bash
elastos/target/release/elastos --version
```

A source-built binary is not a self-contained install. The checkout provides
source code and `components.json`, but it does not add a trusted publisher to
the user's `sources.json`. Run source commands from the repository root so the
binary can find repository assets.

For local source proof without changing a normal user home:

```bash
just local-carrier-setup-smoke
just home-frontdoor-smoke
```

These commands are developer checks. They do not prove the public installer
path. Use [Installing ElastOS](INSTALL.md) when testing the signed installer.

Before handing off a change, run `just verify`. It runs the full repository
gate, including lint and workspace tests.

### Source-built trusted source example

If you operate a trusted release source, add it explicitly:

```bash
ELASTOS_DEV_DATA_HOME="$HOME/.local/share/elastos-dev"

XDG_DATA_HOME="$ELASTOS_DEV_DATA_HOME" \
./elastos/target/release/elastos source add \
  --name local-dev \
  --publisher did:key:PUBLISHER_KEY \
  --connect-ticket CONNECT_TICKET \
  --publisher-node-id PUBLISHER_NODE_ID \
  --install-path "$PWD/elastos/target/release/elastos"

XDG_DATA_HOME="$ELASTOS_DEV_DATA_HOME" \
./elastos/target/release/elastos source show

XDG_DATA_HOME="$ELASTOS_DEV_DATA_HOME" \
./elastos/target/release/elastos setup

XDG_DATA_HOME="$ELASTOS_DEV_DATA_HOME" \
./elastos/target/release/elastos
```

Replace the uppercase placeholders with values from the source you control.
`source add` follows an existing source; it does not create a publisher from a
checkout. The separate data root isolates this run from the installed Home. Do
not copy another installation's `sources.json` into it.

## macOS source staging

Mac source staging is an operator workflow, not a public install. Home-only
staging requires an isolated source home, the offline principal-root readiness
step, and the platform restart script. Browser VM guest artifacts are required
only for Browser proof. Follow [MAC.md](MAC.md) rather than starting the gateway
by hand.

## Create a capsule

The supported WASM executable is a Component that uses
`elastos.component/v1` and `elastos:bus@v1`:

```bash
./elastos/target/release/elastos init my-capsule
```

See [Capsule authoring](CAPSULE_AUTHORING.md) for roles, manifests, templates,
build commands, and verification. Product Components do not receive WASI,
environment variables, host files, raw sockets, or direct provider authority.
This path is currently proved by the conformance fixture and authoring
template; shipped first-party UI Apps remain web projections.

## Operator paths

`elastos serve`, remote node control, WASM or microVM `run`, non-interactive
capsule execution, Browser target maintenance, and release handoff are operator
work. Data `run` needs no Runtime, while interactive packaged capsules use the
managed Home lane. See the command matrix for these paths:

- [Command matrix](COMMAND_MATRIX.md) for runtime ownership
- [Interactive runtime contract](INTERACTIVE_RUNTIME_CONTRACT.md) for terminal
  and Home return behavior
- [Scripts](../scripts/README.md) for proof and operator tooling
- [Runtime user story checklist](RUNTIME_REPO_USER_STORY_CHECKLIST.md) for
  release evidence

## Read next

- [Repository README](../README.md)
- [Architecture](ARCHITECTURE.md)
- [Glossary](GLOSSARY.md)
- [State](../state.md)
