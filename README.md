# ElastOS Runtime

Signed capsules, explicit capabilities, and sovereign local execution for humans and AI.

Pre-release and unstable. Verified on Linux `x86_64` and `aarch64`. Not for production or important workloads.

## Install

```bash
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash
# Core Home front door only
elastos setup

# Same front door, broader demo/test surfaces
elastos setup --profile demo

elastos
```

This installs the signed `elastos` binary.

- `elastos setup` provisions the core Home front door.
- `elastos setup --profile demo` provisions the broader demo/test surface, including the hosted `chat-room` web surface.
- `elastos setup --profile operator` prepares the explicit operator lane used by `elastos serve`, `elastos node ...`, `elastos agent`, and `elastos run`.
- Hosted `chat-room` access currently needs both: `setup --profile demo` installs the shared web surface, and `setup --profile operator` prepares the explicit runtime lane that `elastos room open` reuses.

Then `elastos` opens Home.

## Choose A Lane

One ElastOS home may have only one live host owner at a time.

- Home lane: `elastos setup` or `elastos setup --profile demo`, then `elastos`.
- Operator lane: `elastos setup --profile operator`, then `elastos serve`.
- Hosted room/browser lane on the installed path: `elastos setup --profile demo`, `elastos setup --profile operator`, `elastos serve`, then `elastos room open`.
- `elastos room open` is not a second host. It reuses the live `elastos serve` runtime and opens the room gateway through it.
- `elastos` and `elastos serve` are not two parallel entrypoints for the same home. Stop one before starting the other, or use separate homes if you intentionally need both.

## Build From Source

Requires Rust 1.89+.

```bash
cargo install just
just build
just test
just verify          # source-local gate
just verify-release  # canonical publisher gate
```

Or manually:

```bash
cd elastos && cargo build --workspace --release
```

Source-built setup notes:

- The built binary is a source artifact, not a self-contained install.
- When you run the built binary from the repo checkout, `elastos setup --list` can read the repo `components.json`.
- `elastos setup` still needs a trusted source in `~/.local/share/elastos/sources.json` before it can fetch first-party artifacts.
- For published-install behavior, use the installer in [docs/INSTALL.md](docs/INSTALL.md).
- For source proof of the current checkout, use `just local-carrier-setup-smoke` and `just home-frontdoor-smoke`.
- For one concrete `source add` example, see [docs/GETTING_STARTED.md](docs/GETTING_STARTED.md#source-built-trusted-source-example).

## Run

Normal user lane:

```bash
# Open Home
elastos

# P2P chat
elastos chat --nick alice
```

Explicit operator lane:

```bash
# Start the explicit runtime owner
elastos serve

# Sovereign room status and control
elastos room show
elastos room pending
elastos room approve
elastos room open --addr 0.0.0.0:8090
# then open http://127.0.0.1:8090/apps/chat-room/

# Operator peer control
elastos node info
elastos node status --peer <did:key:...>
```

No-runtime content-plane and site commands:

```bash
# One-time extras for direct share/open
elastos setup --with kubo --with ipfs-provider --with documents

# Share a file over the IPFS-backed content path
elastos share README.md

# Preview a shared CID locally (or on another machine with the same extras)
elastos open elastos://<cid> --browser

# See all commands
elastos --help
```

Important:

- `elastos room show` works without a live runtime, but `elastos room open` requires a running `elastos serve` in the same home plus the hosted `chat-room` web surface from `elastos setup --profile demo`.
- `elastos` is the Home front door. It does not currently attach to an already-running operator runtime in the same home.
- The hosted room route is `/apps/chat-room/`. `/apps/room/` is not a public route. Inside Home, that same surface stays under Home-scoped authority; outside Home it uses browser-session capability policy.

Direct `share`/`open` are content-plane commands backed by `ipfs-provider` and `kubo`. They are not part of the default Carrier-only Home profile.

Power-user paths such as `elastos run` require an explicit runtime and the correct working directory. See [docs/GETTING_STARTED.md](docs/GETTING_STARTED.md) for source builds, capsule development, and explicit runtime workflows.

The interactive product contract is narrower than the full command surface:

- first-class: `elastos`, `elastos home`, `Home -> System/Documents/Library/Inbox`
- demo profile: `Home -> Chat Room`, `Home -> GBA UCity`, and MyWebSite/public-edge helpers when their components are installed
- secondary shortcut: `elastos chat`
- secondary packaged path: `elastos capsule <name> --lifecycle interactive --interactive`
- operator/developer-only: `elastos agent`, `elastos node`, `elastos run`, non-interactive `elastos capsule`

See [docs/INTERACTIVE_RUNTIME_CONTRACT.md](docs/INTERACTIVE_RUNTIME_CONTRACT.md) for the exact runtime, TTY, and home/exit semantics.

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  Runtime (elastos binary) — minimal trusted base    │
│  Isolation · Signatures · Capabilities              │
└─────────────────────────────────────────────────────┘
                        │
┌─────────────────────────────────────────────────────┐
│  Home / policy capsule with orchestrator capability │
│  Permission prompts · App orchestration             │
└─────────────────────────────────────────────────────┘
                        │
┌─────────────────────────────────────────────────────┐
│  Capsules (sandboxed apps and providers)            │
│  WASM · microVM · data · zero ambient authority     │
└─────────────────────────────────────────────────────┘
```

The runtime is the small trusted base. Everything above it, including Home and System, runs as sandboxed capsules with explicit capability tokens. Humans and AI agents use the same capability model and the same action contracts. See [docs/DESIGN_SYSTEM.md](docs/DESIGN_SYSTEM.md) for the current first-party surface palette and interaction rules.

## What Works Today

- fresh install → setup → Home
- native P2P chat, plus local/source proof for WASM chat interop
- sovereign room membership/invite flow, hosted chat-room access under the explicit operator lane, and local cross-runtime Carrier room sync proof
- signed publish, install, and update flow
- operator-only remote node status and trusted-source update control over Carrier via `elastos node ...`
- explicit operator runtime prep via `elastos setup --profile operator`
- content sharing and local site hosting
- DID-backed identity across surfaces
- agent capsule with signed gossip and verified-only AI responses

Release-trust verification against the canonical publisher path is separate from local dev proof. See `state.md` and [docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md](docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md) for the current scope.

See [state.md](state.md) for the current product state.

## Runtime Classes

Every command has one runtime expectation. No command may hang.

| Class | Commands | Contract |
|---|---|---|
| Managed dashboard | `elastos`, `elastos home` | Auto-starts or reuses the managed Home runtime for the first-class Home front door |
| Managed packaged interactive | `elastos capsule <name> --lifecycle interactive --interactive` | Secondary packaged path; reuses a compatible active runtime or the managed Home runtime when needed |
| Managed user | `elastos chat` | Native chat shortcut; reuses a healthy managed Home runtime first, otherwise managed chat runtime |
| No runtime | `elastos share`, `elastos open`, `elastos shares *`, `elastos attest`, `elastos update`, `elastos setup`, `elastos site *` | Runs direct |
| Operator | `elastos room open`, `elastos agent`, non-interactive `elastos capsule`, `elastos run` | Requires one explicit live runtime owner per home (`elastos serve`) |
| Starts own service | `elastos serve`, `elastos gateway`, `elastos site serve` | Starts its own daemon |

See [docs/INTERACTIVE_RUNTIME_CONTRACT.md](docs/INTERACTIVE_RUNTIME_CONTRACT.md) for the interactive contract and [docs/COMMAND_MATRIX.md](docs/COMMAND_MATRIX.md) for the full command/runtime table.

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
| [docs/INSTALL.md](docs/INSTALL.md) | Install, update, and trust model details |
| [docs/INTERACTIVE_RUNTIME_CONTRACT.md](docs/INTERACTIVE_RUNTIME_CONTRACT.md) | Blessed interactive runtime, TTY, and home/exit model |
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
