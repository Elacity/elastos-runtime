# Getting Started with ElastOS Runtime

This guide has two paths:

- binary install if you want to use the current preview
- source build if you want to develop or inspect the runtime directly

## Binary Install

The canonical public install path is the current Linux `x86_64`/`aarch64`
preview. macOS uses source-home staging for now; see [MAC.md](MAC.md).

```bash
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash
elastos setup
elastos
```

After setup, `elastos` opens Home. The default Home profile includes System,
People, Services, Browser, Wallet, Documents, Library, Marketplace, Archive, and
Inbox, plus access to your rooted localhost world. No separate `elastos serve`
terminal is needed for the normal user path.

### Choose One Lane Per Home

One ElastOS home may have only one live host owner at a time.

- Home lane:
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
- Treat hosted room as a lane switch after Home checks, not as something that merges into a still-running `elastos` session in the same home.
- `elastos room open` is not a second host. It reuses the live `elastos serve` runtime and opens the room gateway through it.
- Do not run `elastos` and `elastos serve` side by side in the same home and expect them to merge. Stop one first or use separate homes if you intentionally need both.
- If `elastos serve` already owns the home, `elastos` will not start in that same home until you stop `serve`.

What this gives you today:

- a local Home surface
- one-terminal native chat
- signed `elastos update`
- first-party Carrier-backed setup for the default Home profile, including
  System, People, Services, Browser, Wallet, Documents, Library, Marketplace,
  Archive, and Inbox

Useful next commands after plain `elastos setup`:

```bash
elastos chat --nick alice
elastos update
```

If you want the same Home front door plus the broader demo/test surfaces:

```bash
elastos setup --profile demo
elastos
```

`setup --profile demo` adds more shipped surfaces. It does not change the single-host rule above.

For hosted `chat-room` access, `setup --profile demo` installs the shipped `chat-room` web surface.

If you want direct share/open on top of the default Home core profile, add the explicit extras first:

```bash
elastos setup --with kubo --with ipfs-provider --with documents
elastos share README.md
elastos open elastos://<cid>
elastos share --public README.md
```

Important boundary:

- `chat` is the only standalone managed user-runtime command
- `setup` stays first-party and Carrier-only by default
- direct share/open/site/public-edge IPFS tooling is explicit extra setup, not
  part of the default Home profile
- `agent`, `capsule`, WASM/microVM `run`, and `room open` remain explicit operator-runtime surfaces

See [INTERACTIVE_RUNTIME_CONTRACT.md](INTERACTIVE_RUNTIME_CONTRACT.md) for the blessed interactive contract and [COMMAND_MATRIX.md](COMMAND_MATRIX.md) for the full command/runtime table.

## Interactive Path Status

Current status by intent:

- first-class public path:
  - `elastos`
  - `elastos home`
  - `Home -> System/Documents/Library/Inbox`
- secondary shortcut:
  - `elastos chat`
- secondary packaged surface path:
  - `elastos capsule <name> --lifecycle interactive --interactive`
- operator or developer-only:
  - `elastos agent`
  - `elastos run ...`
  - non-interactive `elastos capsule ...`

The public product contract is intentionally centered on Home, not on direct packaged-surface launches.

## Direct Chat Shortcuts

If you want to jump straight into chat without going through Home:

```bash
elastos chat --nick alice
```

`elastos chat` is a direct native chat shortcut. On the current native terminal path, `Esc` or `/home` returns Home; `/quit` exits to the invoking terminal.

There are also packaged chat-family paths, but they are secondary today rather than the main product contract:

```bash
elastos setup --profile chat
elastos capsule chat --lifecycle interactive --interactive --config '{"nick":"alice"}'
```

Important honesty rule for those packaged paths:

- when launched from Home, they can return Home
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
- If you want to wire in your own source manually, add one explicitly with `elastos source add ...`.
- If you want repo-native proof that the current checkout works, run `just local-carrier-setup-smoke` and `just home-frontdoor-smoke`.

Copying a raw source-built binary into `~/.local/bin` is not the canonical source-developer path.

### Mac source-home staging

For the current Apple silicon Mac path, use [MAC.md](MAC.md). The short version
is:

```bash
rustup target add wasm32-unknown-unknown
export MAC_TEST_HOME="$HOME/elastos-mac-test-home"
export USER_HOME="$HOME"

HOME="$MAC_TEST_HOME" \
CARGO_HOME="$USER_HOME/.cargo" \
RUSTUP_HOME="$USER_HOME/.rustup" \
PATH="$USER_HOME/.cargo/bin:/opt/homebrew/bin:$PATH" \
scripts/setup-source-home.sh
```

Then start Home at `http://localhost:61180/apps/home/`. Browser VM setup also
requires reviewed Linux guest artifacts (`vmlinux`, `initrd`, `rootfs.ext4`,
and `browser-vm-rootfs-manifest.json`) installed into the Mac source-home data
dir; `setup-source-home.sh` refreshes existing VM artifacts but does not create
that bootable Linux guest image on macOS.

### Source-built trusted source example

If you already control a trusted source runtime, add it explicitly before running `setup`:

```bash
elastos source add \
  --name local-dev \
  --publisher <did:key:...> \
  --connect-ticket <ticket> \
  --publisher-node-id <node-id> \
  --install-path ~/.local/bin/elastos

elastos source show
elastos setup --profile operator
```

What those values mean:

- `--publisher` is the trusted publisher DID for the source you intend to follow.
- `--connect-ticket` and `--publisher-node-id` come from the source runtime you control.
- `--install-path` should point at the `elastos` binary you want future updates to target.

Important boundary:

- `source add` adds an existing trusted source. It does not create one from the checkout.
- Today the supported source-local proof path is still `just local-carrier-setup-smoke` or `just home-frontdoor-smoke`.
- Reusing someone else's live `sources.json` is not the general contract. Add a source you control, or use the stamped installer path.

## First Source Runs

### Native chat

```bash
./scripts/chat.sh --nick alice
```

The source-side scripts are developer/demo entrypoints. They are not the public
install contract. The `demo` and `full` profiles install the conditional GBA
engine, viewer, and uCity content capsule together. The viewer uses Runtime for
ROM and save authority. Use `just gba` to build and verify that path; launch
games from Home or Library.

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
- The browser-hosted adapter it exposes comes from `setup --profile demo`.
- The canonical local route is `http://localhost:8090/apps/chat-room/`.

Rule:

- if a command is operator-runtime, it should fail fast and tell you to start `elastos serve`
- a chat-managed runtime does not satisfy operator-runtime commands
- `room open` is the explicit room/browser helper on top of `elastos serve`; it is not a separate host lane

### Browser Proof Lane

The Browser capsule is currently a Runtime Browser proof, not a completed
general-purpose browser. It should be tested through the Browser/Net/Exit/Engine
contract and the slice-specific smokes named in `TASKS.md`; do not validate it
by opening a host iframe or a raw external browser tab and calling that done.

For developer review, the important distinction is:

- Browser UI receives a page/display session and input routes.
- Runtime owns Net/Exit validation, wallet mediation, chain reads, and audit.
- Operator Browser service scripts live in `scripts/system/` and are proof
  packaging for the current hosted baseline, not the final architecture.

## Capsule Development

Read [Capsule Authoring](CAPSULE_AUTHORING.md) and use the checked-in
[`templates/capsules`](../templates/capsules/README.md) for web projections,
viewer/content pairs, and provider contracts. Create a minimal Component
capsule with:

```bash
./elastos/target/release/elastos init my-capsule
cd my-capsule
../scripts/build-component-capsule.sh .
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
- providers expose scoped actions like storage, DID, peer, content, chain, and AI
- low-level IPFS/Kubo access stays behind provider/system-service boundaries
- Carrier owns networking semantics; capsules do not get raw networking by default

## Installed-Host Notes

The current preview is exercised on Linux `x86_64` and `aarch64`.

Current honest proof scope is narrower than that platform list:

- the live public x86_64 outsider path is proven
- full installed `elastos -> Home -> System/Documents/Library/Inbox -> Home` is still a manual target-machine acceptance item on additional hosts
- installed hosted chat-room validation is still a manual operator-lane acceptance item

See [state.md](../state.md) for the factual current evidence level.

## Related Docs

- [state.md](../state.md)
- [COMMAND_MATRIX.md](COMMAND_MATRIX.md)
- [INSTALL.md](INSTALL.md)
- [ARCHITECTURE.md](ARCHITECTURE.md)
- [CARRIER.md](CARRIER.md)
