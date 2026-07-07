# ElastOS Runtime

Signed capsules, explicit capabilities, passkey-fronted accounts, and
sovereign local execution for humans and AI.

Pre-release and unstable. Verified primarily on Linux `x86_64` and `aarch64`.
Not for production or important workloads.

## Install

The public installer is the current Linux `x86_64`/`aarch64` preview. macOS uses
source-home staging for now; see [docs/MAC.md](docs/MAC.md).

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
- Hosted room lane on the installed path: `elastos setup --profile demo`, `elastos setup --profile operator`, `elastos serve`, then `elastos room open`.
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

Developer orientation:

- `state.md` is the factual current-state surface.
- `TASKS.md` is the open-work driver; the `Now` section is strict priority order.
- `ROADMAP.md` is direction, not proof.
- `docs/README.md` lists the active documentation set.
- `scripts/README.md` explains which scripts are public entrypoints and which are proof/operator helpers.

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
# then open http://localhost:8090/apps/chat-room/

# Operator peer control
elastos node info
elastos node status --peer <did:key:...>
```

No-runtime content-plane and site commands:

```bash
# One-time extras for direct share/open
elastos setup --with kubo --with ipfs-provider --with documents

# Share a file over the current content availability path
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

Direct `share`/`open` are current content-plane commands backed by `ipfs-provider` and `kubo`. They are not part of the default Carrier-only Home profile; the app-facing contract is `elastos://content/*`.

Power-user paths such as `elastos run` require an explicit runtime and the correct working directory. See [docs/GETTING_STARTED.md](docs/GETTING_STARTED.md) for source builds, capsule development, and explicit runtime workflows.

Current Apple silicon Mac staging is a source-home path, not the final public
Mac installer. See [docs/MAC.md](docs/MAC.md) for the verified Mac setup from
source checkout to `http://localhost:61180/apps/home/` and passkey sign-in.

The interactive product contract is narrower than the full command surface:

- first-class: `elastos`, `elastos home`, `Home -> System/Documents/Library/Inbox`
- demo profile: `Home -> Chat Room`, `Home -> GBA UCity`, and MyWebSite/public-edge helpers when their components are installed
- secondary shortcut: `elastos chat`
- secondary packaged path: `elastos capsule <name> --lifecycle interactive --interactive`
- operator/developer-only: `elastos agent`, `elastos node`, `elastos run`, non-interactive `elastos capsule`

See [docs/INTERACTIVE_RUNTIME_CONTRACT.md](docs/INTERACTIVE_RUNTIME_CONTRACT.md) for the exact runtime, TTY, and home/exit semantics.

## Architecture

```text
Home / System / Wallet / Browser
  -> Runtime principals, sessions, capabilities, audit
  -> Provider plane: content, wallet, chain, browser-engine, net, exit
  -> Carrier / local providers / explicit operator services
```

The runtime is the small trusted base: isolation, signatures, principal/session
binding, capability validation, object routing, and audit. Everything above it,
including Home, System, Wallet, Browser, apps, viewers, and providers, runs with
scoped authority. Humans and AI agents use the same capability model and the
same action contracts.

The current planning frame is four quadrants:

- **Home / PC2:** human front door, account UX, object browsing, capsule launch
- **Runtime:** trusted capability core and provider routing
- **Carrier:** authenticated object/message/stream plane
- **Blockchain:** wallet, DID/EID, node, provenance, and rights adapters

Normal capsules do not receive raw wallet RPC, node RPC, IPFS/Kubo APIs, browser
engine handles, private keys, or host internals. They ask Runtime for scoped
capabilities; providers perform the dangerous work.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md),
[docs/CARRIER.md](docs/CARRIER.md), and
[docs/DESIGN_SYSTEM.md](docs/DESIGN_SYSTEM.md).

## What Works Today

- fresh install → setup → Home
- passkey-first Home unlock, with first-account admin, admin-controlled guest
  enrollment, self-created guest accounts, sign-out, and per-principal Home
  state
- System account policy, background controls, full Recovery Kit import/export
  for the principal Home root plus recoverable built-in Wallet keys, and
  advanced runtime/network diagnostics
- one Wallet surface for passkey-managed accounts, MetaMask, Bitcoin, and the
  dormant WalletConnect connector path; Wallet owns account naming, defaults,
  balances, QR receive, approvals, and connector handoffs. WalletConnect stays
  hidden until operator-pinned Reown/AppKit config and local adapter assets are
  present
- typed wallet approvals through Wallet/Inbox, including managed EVM/BTC
  signing, external connector completion, and signed receipts without exposing
  private keys or raw wallet objects to apps
- typed `chain-provider` access for supported chain status, balances, proofs,
  transaction prepare/broadcast, and fail-closed local-node lifecycle status
- a Browser capsule proof that opens as a Home window and routes website access
  through the Runtime Browser/Net/Exit/Engine boundary instead of host iframes
  or app-level wallet injection
- a bounded protected-content proof for the known `ela.city` test path through
  Runtime Browser, including funded purchase/playback evidence, without claiming
  arbitrary protected-content readiness or completed dDRM/dKMS providers
- native P2P chat, plus local/source proof for WASM chat interop
- sovereign room membership/invite flow, hosted chat-room access under the explicit operator lane, and local cross-runtime Carrier room sync proof
- signed publish, install, and update flow
- operator-only remote node status and trusted-source update control over Carrier via `elastos node ...`
- explicit operator runtime prep via `elastos setup --profile operator`
- content sharing, content-availability manifests, and local site hosting through
  the `elastos://content/*` path, with `ipfs-provider` kept as a low-level
  backend
- device DID, passkey principals, wallet proof bindings, and Recovery Kit
  foundations without making wallet addresses the Runtime identity root
- agent capsule with signed gossip and verified-only AI responses
- Flint mandates: grant an AI agent scoped, expiring, revocable authority (a mandate, not
  your keys), watch and kill it from the Mandates shell app, let it spend real money under a
  durable spend cap on a payment rail (HTTPS adapter or on-chain DRM marketplace), and export
  a portable signed receipt verifiable off-box with `elastos verify-receipt` — see
  [docs/FLINT_MANDATE_ENGINE.md](docs/FLINT_MANDATE_ENGINE.md)

Important Browser status: the Browser ABI and hosted proof path are real, but
the final product browser is not complete. Stable arbitrary-site media, accepted
provider selection, protected/recoverable Browser profile storage, and
cross-platform native/microVM adapters remain open. See
[docs/BROWSER_CAPSULE.md](docs/BROWSER_CAPSULE.md)
and [docs/BROWSER_PROVIDER_BAKEOFF.md](docs/BROWSER_PROVIDER_BAKEOFF.md).

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
└── scripts/               # Build, publish, install, proof scripts
```

Rust tests live in the `elastos/` workspace. Product and release proof scripts
live under `scripts/`; they are listed in [scripts/README.md](scripts/README.md)
and the release-facing checklist.

For branch review, do not treat a green workspace test as sufficient by itself.
Use the relevant narrow provider/UI smokes from `TASKS.md` for the slice being
reviewed, especially Wallet, System, Browser ABI/provider, Browser proof tools,
and release/docs slices.

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
| [docs/CONTENT_AVAILABILITY.md](docs/CONTENT_AVAILABILITY.md) | SmartWeb content availability, IPLD-compatible manifests, and provider boundary |
| [docs/WALLET_PROVIDER.md](docs/WALLET_PROVIDER.md) | Wallet, account, approval, proof, and signer/provider boundary |
| [docs/CHAIN_PROVIDER.md](docs/CHAIN_PROVIDER.md) | Typed blockchain provider boundary |
| [docs/FLINT_MANDATE_ENGINE.md](docs/FLINT_MANDATE_ENGINE.md) | Flint — mandates for AI agents: scoped, revocable authority, spend-capped payments, portable signed receipts |
| [docs/DRM_MARKETPLACE_RAIL.md](docs/DRM_MARKETPLACE_RAIL.md) | The DRM marketplace payment rail (on-chain settlement under a mandate) |
| [docs/BROWSER_CAPSULE.md](docs/BROWSER_CAPSULE.md) | Browser/Net/Exit/Engine ABI and current proof boundary |
| [docs/BROWSER_PROVIDER_BAKEOFF.md](docs/BROWSER_PROVIDER_BAKEOFF.md) | Browser provider comparison and acceptance gates |
| [docs/SITES.md](docs/SITES.md) | Local site hosting and public exposure |
| [docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md](docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md) | Release-facing test matrix and manual runbook |
| [docs/GLOSSARY.md](docs/GLOSSARY.md) | Terminology |
| [PRINCIPLES.md](PRINCIPLES.md) | Guiding constraints |
| [ROADMAP.md](ROADMAP.md) | Forward plan |
| [TASKS.md](TASKS.md) | Open work |

## License

[MIT](LICENSE)
