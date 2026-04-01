# ElastOS Runtime

Signed capsules, explicit capabilities, and sovereign local execution for humans and AI.

Pre-release and unstable. Verified on Linux `x86_64` and `aarch64`. Not for production or important workloads.

## Install

```bash
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash
elastos setup
elastos
```

This installs the signed `elastos` binary, provisions the core PC2 home profile through the trusted-source path, and opens the PC2 home surface.

## Build From Source

Requires Rust 1.89+.

```bash
cargo install just
just build
just test
```

Or manually:

```bash
cd elastos && cargo build --workspace --release
```

## Run

```bash
# Open the PC2 home surface
elastos

# P2P chat
elastos chat --nick alice

# One-time extras for direct share/open
elastos setup --with kubo --with ipfs-provider --with md-viewer

# Share a file over the IPFS-backed content path
elastos share README.md

# Preview a shared CID locally (or on another machine with the same extras)
elastos open elastos://<cid> --browser

# See all commands
elastos --help
```

Direct `share`/`open` are content-plane commands backed by `ipfs-provider` and `kubo`. They are not part of the default Carrier-only PC2 core profile.

Power-user paths such as `elastos run` require an explicit runtime and the correct working directory. See [docs/GETTING_STARTED.md](docs/GETTING_STARTED.md) for source builds, capsule development, and explicit runtime workflows.

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  Runtime (elastos binary) — minimal trusted base    │
│  Isolation · Signatures · Capabilities              │
└─────────────────────────────────────────────────────┘
                        │
┌─────────────────────────────────────────────────────┐
│  Shell (capsule with orchestrator capability)       │
│  Permission prompts · Capsule orchestration         │
└─────────────────────────────────────────────────────┘
                        │
┌─────────────────────────────────────────────────────┐
│  Capsules (sandboxed apps and providers)            │
│  WASM · microVM · data · zero ambient authority     │
└─────────────────────────────────────────────────────┘
```

The runtime is the small trusted base. Everything above it — including the shell — runs as a sandboxed capsule with explicit capability tokens. Humans and AI agents use the same capability model.

## What Works Today

- fresh install → setup → PC2 home
- native P2P chat, plus local/source proof for WASM chat interop
- signed publish, install, and update flow
- content sharing and local site hosting
- DID-backed identity across surfaces
- agent capsule with signed gossip and verified-only AI responses

Release-trust verification against the canonical publisher path is separate from local dev proof. See `state.md` and [docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md](docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md) for the current scope.

See [state.md](state.md) for the current product state.

## Runtime Classes

Every command has one runtime expectation. No command may hang.

| Class | Commands | Contract |
|---|---|---|
| Managed dashboard | `elastos`, `elastos pc2` | Auto-starts loopback runtime, renders PC2 |
| Managed user | `elastos chat` | Auto-starts local runtime after setup |
| No runtime | `elastos share`, `elastos open`, `elastos shares *`, `elastos attest`, `elastos update`, `elastos setup`, `elastos site *` | Runs direct |
| Operator | `elastos agent`, `elastos capsule`, `elastos run` | Requires explicit `elastos serve` |
| Starts own service | `elastos serve`, `elastos gateway`, `elastos site serve` | Starts its own daemon |

See [docs/COMMAND_MATRIX.md](docs/COMMAND_MATRIX.md) for the full contract.

## Repository Structure

```text
elastos-runtime/
├── elastos/               # Core runtime workspace (Rust)
│   └── crates/            # elastos-server, elastos-runtime, elastos-common, ...
├── capsules/              # User/provider/demo capsules
├── docs/                  # Architecture, guides, status
├── scripts/               # Build, publish, install, proof scripts
└── tests/                 # Integration tests
```

## Documentation

| Document | What |
|----------|------|
| [docs/GETTING_STARTED.md](docs/GETTING_STARTED.md) | Install, build, first runs |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Runtime design and trust boundaries |
| [docs/COMMAND_MATRIX.md](docs/COMMAND_MATRIX.md) | Runtime expectation per command |
| [docs/NAMESPACES.md](docs/NAMESPACES.md) | localhost:// and elastos:// namespace model |
| [docs/CARRIER.md](docs/CARRIER.md) | P2P transport model |
| [docs/SITES.md](docs/SITES.md) | Local site hosting and public exposure |
| [docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md](docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md) | Release-facing test matrix and manual runbook |
| [docs/GLOSSARY.md](docs/GLOSSARY.md) | Terminology |
| [PRINCIPLES.md](PRINCIPLES.md) | Guiding constraints |
| [ROADMAP.md](ROADMAP.md) | Forward plan |
| [TASKS.md](TASKS.md) | Open work |

## License

[MIT](LICENSE)
