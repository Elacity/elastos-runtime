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

After setup, `elastos` opens the sovereign PC2 home surface. From there you can launch chat and inspect your rooted localhost world. PC2 is the front door, but home/app return behavior is still being tightened on some installed hosts. No separate `elastos serve` terminal is needed for the normal user path.

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

If you want direct share/open on top of the default PC2 core profile, add the explicit extras first:

```bash
elastos setup --with kubo --with ipfs-provider --with md-viewer
elastos share README.md
elastos open elastos://<cid>
elastos share --public README.md
```

Important boundary:

- `chat` is the only managed user-runtime command
- `setup` stays first-party and Carrier-only by default
- direct share/open/site/public-edge tooling is explicit extra setup, not part of the default PC2 core profile
- `agent`, `capsule`, and WASM/microVM `run` remain explicit operator-runtime surfaces

See [COMMAND_MATRIX.md](COMMAND_MATRIX.md) for the full command/runtime contract.

## Direct Chat Shortcuts

If you want to jump straight into chat without going through PC2 home:

```bash
elastos chat --nick alice
```

There is also a packaged IRC capsule path:

```bash
elastos setup --profile irc
elastos capsule chat --lifecycle interactive --interactive --config '{"nick":"alice"}'
```

For the non-KVM packaged WASM variant:

```bash
elastos setup --profile demo
elastos capsule chat-wasm --lifecycle interactive --interactive --config '{"nick":"alice"}'
```

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
- Linux with KVM for microVM work (`native`, `WSL2`, or `aarch64` such as Jetson)
- `just` recommended: `cargo install just`

No OpenSSL is required for the core runtime. Most crypto is pure Rust.

### Build

```bash
cargo install just
just build
just test
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

These commands require an explicit runtime:

```bash
elastos serve
elastos agent --backend codex
elastos capsule ...
elastos run ...
```

Rule:

- if a command is operator-runtime, it should fail fast and tell you to start `elastos serve`
- a chat-managed runtime does not satisfy operator-runtime commands

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

## Cross-Device Notes

The current preview is exercised on:

- Linux `x86_64`
- Linux `aarch64`
- WSL
- Jetson

Current honest proof scope is narrower than that platform list:

- the live public x86_64 outsider path is proven on the current release line
- Jetson/WSL `elastos -> PC2 -> Chat -> home` is still an open target-machine proof item

See [state.md](../state.md) for the factual current evidence level.

## Related Docs

- [state.md](../state.md)
- [COMMAND_MATRIX.md](COMMAND_MATRIX.md)
- [INSTALL.md](INSTALL.md)
- [ARCHITECTURE.md](ARCHITECTURE.md)
- [CARRIER.md](CARRIER.md)
