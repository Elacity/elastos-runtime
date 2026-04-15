# Getting Started with ElastOS Runtime

This guide has two paths:

- binary install if you want to use the current preview
- source build if you want to develop or inspect the runtime directly

## Binary Install

The canonical public install path is:

```bash
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash
elastos setup
elastos
```

After setup, `elastos` opens the sovereign PC2 home surface. From there you can launch chat and inspect your rooted localhost world. PC2 is the front door. No separate `elastos serve` terminal is needed for the normal user path.

### Choose One Lane Per Home

One ElastOS home may have only one live host owner at a time.

- PC2 lane:
  - `elastos setup` or `elastos setup --profile demo`
  - then `elastos`
- Operator lane:
  - `elastos setup --profile operator`
  - then `elastos serve`
- Hosted room/browser validation lane:
  - `elastos setup --profile demo`
  - `elastos setup --profile operator`
  - then `elastos serve`
  - then `elastos room open --addr 0.0.0.0:8090`
- Treat hosted room as a lane switch after PC2 checks, not as something that merges into a still-running `elastos` session in the same home.
- `elastos room open` is not a second host. It reuses the live `elastos serve` runtime and opens the room gateway through it.
- Do not run `elastos` and `elastos serve` side by side in the same home and expect them to merge. Stop one first or use separate homes if you intentionally need both.
- If `elastos serve` already owns the home, `elastos` will not start in that same home until you stop `serve`.

What this gives you today:

- a local PC2 home surface
- one-terminal native chat
- signed `elastos update`
- first-party Carrier-backed setup for the default PC2 core profile

Useful next commands after plain `elastos setup`:

```bash
elastos chat --nick alice
elastos update
```

If you want the same PC2 front door plus the broader demo/test surfaces:

```bash
elastos setup --profile demo
elastos
```

`setup --profile demo` adds more shipped surfaces. It does not change the single-host rule above.

For the hosted room browser path, `setup --profile demo` is the piece that installs the shipped room browser surface.

If you want direct share/open on top of the default PC2 core profile, add the explicit extras first:

```bash
elastos setup --with kubo --with ipfs-provider --with md-viewer
elastos share README.md
elastos open elastos://<cid>
elastos share --public README.md
```

Important boundary:

- `chat` is the only standalone managed user-runtime command
- `setup` stays first-party and Carrier-only by default
- direct share/open/site/public-edge tooling is explicit extra setup, not part of the default PC2 core profile
- `agent`, `capsule`, WASM/microVM `run`, and `room open` remain explicit operator-runtime surfaces

See [INTERACTIVE_RUNTIME_CONTRACT.md](INTERACTIVE_RUNTIME_CONTRACT.md) for the blessed interactive contract and [COMMAND_MATRIX.md](COMMAND_MATRIX.md) for the full command/runtime table.

## Interactive Path Status

Current status by intent:

- first-class public path:
  - `elastos`
  - `elastos pc2`
  - `PC2 -> Chat`
- secondary shortcut:
  - `elastos chat`
- secondary packaged surface path:
  - `elastos capsule <name> --lifecycle interactive --interactive`
- operator or developer-only:
  - `elastos agent`
  - `elastos run ...`
  - non-interactive `elastos capsule ...`

The public product contract is intentionally centered on the PC2 front door, not on direct packaged-surface launches.

## Direct Chat Shortcuts

If you want to jump straight into chat without going through PC2 home:

```bash
elastos chat --nick alice
```

`elastos chat` is a shortcut to the same native chat surface used from PC2. On the current native terminal path, `Esc` or `/home` returns to PC2; `/quit` exits to the invoking terminal.

There are also packaged chat-family paths, but they are secondary today rather than the main product contract:

```bash
elastos setup --profile chat
elastos capsule chat --lifecycle interactive --interactive --config '{"nick":"alice"}'
```

For the non-KVM packaged WASM variant:

```bash
elastos setup --profile demo
elastos capsule chat-wasm --lifecycle interactive --interactive --config '{"nick":"alice"}'
```

Important honesty rule for those packaged paths:

- when launched from PC2, they can return to PC2
- when launched directly from the terminal, both `/home` and `/quit` return to the invoking terminal
- they should not be documented as the boring default path unless they are explicitly surfaced and proven on the installed route

## Elastos Sites

The browser-facing local site root is:

```text
localhost://MyWebSite
```

Current status:

- this is now a real staged local root under the runtime data dir
- `elastos site ...` is the explicit site command surface
- `elastos open localhost://MyWebSite` serves the staged local site directly
- `localhost://Public/...` remains the shared-files root, not the site root

Current commands:

```bash
elastos site stage ./my-site
elastos site path
elastos site publish --release weekend-demo
elastos site releases
elastos site promote live weekend-demo
elastos site channels
elastos site activate --channel live
elastos site history
elastos site rollback weekend-demo
elastos site bind-domain example.com
elastos site serve --mode local
elastos site serve --mode ephemeral
elastos open localhost://MyWebSite
```

For CID-backed site publish/activation, add the explicit extras first:

```bash
elastos setup --with kubo --with ipfs-provider
```

Current gateway modes above that local site root:

- local
  - your own static IP / domain / reverse proxy
- ephemeral
  - temporary public edge such as `cloudflared`
- supernode / active proxy
  - higher-availability hosted front door for the same local or replicated site (later)

See [SITES.md](SITES.md) for the contract and current implementation status.

## Source Build

### Prerequisites

- Rust 1.89+ via [rustup.rs](https://rustup.rs)
- Git
- Linux with KVM for microVM work on supported hardware
- `just` recommended: `cargo install just`

No OpenSSL is required for the core runtime. Most crypto is pure Rust.

### Build

```bash
cargo install just
just build
just test
just verify
just verify-release
```

Or manually:

```bash
cd elastos
cargo build --workspace --release
cd ..
```

Verify the built binary:

```bash
elastos/target/release/elastos --version
```

### Source-built setup notes

The built binary is not a self-contained install.

- Run it from the repo checkout if you want `elastos setup --list` to read the repo `components.json`.
- `elastos setup` still requires a trusted source before it can fetch first-party artifacts.
- A GitHub checkout gives you source plus the manifest. It does not stamp `sources.json` for you.
- If you want published-install behavior in a clean home, use the installer path from [INSTALL.md](INSTALL.md).
- If you want to wire in your own source, start with `elastos source add --help`.
- If you want repo-native proof that the current checkout works, run `just local-carrier-setup-smoke` and `just pc2-frontdoor-smoke`.

Copying a raw source-built binary into `~/.local/bin` is not the canonical source-developer path.

## First Source Runs

### Native chat

```bash
./scripts/chat.sh --nick alice
```

### Notepad demo

```bash
./scripts/notepad.sh
```

### GBA demo

```bash
./scripts/gba.sh
```

These source-side scripts are developer/demo entrypoints. They are not the public install contract.

## Operator Runtime

These commands belong to the explicit operator lane:

```bash
elastos setup --profile operator
elastos serve
elastos room open --addr 0.0.0.0:8090
elastos node info
elastos agent
elastos capsule ...
elastos run ...
```

Hosted room note:

- `elastos room open` needs the explicit operator runtime from `setup --profile operator`.
- The browser surface it exposes comes from `setup --profile demo`.
- The canonical local route is `http://127.0.0.1:8090/apps/room-browser/`.

Rule:

- if a command is operator-runtime, it should fail fast and tell you to start `elastos serve`
- a chat-managed runtime does not satisfy operator-runtime commands
- `room open` is the explicit room/browser helper on top of `elastos serve`; it is not a separate host lane

## Capsule Development

Create a new capsule:

```bash
./elastos/target/release/elastos init my-capsule
cd my-capsule
rustup target add wasm32-wasip1
cargo build --release
cp target/wasm32-wasip1/release/my-capsule.wasm .
../elastos/target/release/elastos run .
```

Content capsule scaffold:

```bash
./elastos/target/release/elastos init my-docs --type content
cd my-docs
../elastos/target/release/elastos share .
```

## Capability Model

Capsules and agents start with no ambient authority. Access is granted through explicit capability tokens.

Current trust model:

- runtime validates capabilities
- providers expose scoped actions like storage, DID, peer, IPFS, and AI
- Carrier owns networking semantics; capsules do not get raw networking by default

## Installed-Host Notes

The current preview is exercised on Linux `x86_64` and `aarch64`.

Current honest proof scope is narrower than that platform list:

- the live public x86_64 outsider path is proven
- full installed `elastos -> PC2 -> Chat -> home` is still a manual target-machine acceptance item on additional hosts
- installed hosted room-browser validation is still a manual operator-lane acceptance item

See [state.md](../state.md) for the factual current evidence level.

## Related Docs

- [state.md](../state.md)
- [COMMAND_MATRIX.md](COMMAND_MATRIX.md)
- [INSTALL.md](INSTALL.md)
- [ARCHITECTURE.md](ARCHITECTURE.md)
- [CARRIER.md](CARRIER.md)
