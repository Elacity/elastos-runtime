# PC2 → ElastOS Runtime Convergence Plan

> Working plan. Not a release commitment. Direction here must pass the Planning
> Review Gate in [ROADMAP.md](../ROADMAP.md#planning-review-gate) before
> implementation. Volatile facts and proof transcripts belong in
> [state.md](../state.md), not here.

For why this work exists, see [pc2.net's ELASTOS_VISION.md](https://github.com/Elacity/pc2.net/blob/main/ELASTOS_VISION.md)
and this repo's [PRINCIPLES.md](../PRINCIPLES.md) and [ROADMAP.md](../ROADMAP.md).

## Why

[pc2.net](https://github.com/Elacity/pc2.net) is the current shipping
"Internet OS" product. It is a Puter-derived Node.js stack with wallet
auth, IPFS storage, SQLite, WASM execution, and a browser desktop. It
runs on macOS, Linux, Windows, Pi, Docker today. It is wrapped by
[elastos-launcher](https://github.com/Elacity/elastos-launcher) into a
one-click .dmg/.deb/.exe.

`elastos-runtime` is the target architecture: a minimal Rust trusted
core that gives every capsule its own isolation domain, capability
tokens, content-addressed identity, DID-anchored principals, and a
single namespace contract. Per [PRINCIPLES.md](../PRINCIPLES.md), it
delivers what pc2.net cannot retrofit:

- **No Ambient Authority** — capsules see capability-scoped operations,
  not raw filesystems or networks.
- **Small Trusted Core** — only isolation, signatures, capabilities, and
  `elastos://` live in the runtime; everything else is a capsule or
  provider.
- **Stable Identity Over Transport** — `localhost://...` and
  `elastos://...` are the real nouns; HTTP is edge transport.
- **One Canonical Path Per Operation** — no soft fallbacks; fail closed.

Convergence means: existing pc2.net features become capsules and
providers running on this runtime. The launcher wraps the runtime
instead of pc2-node. The Puter-derived desktop is progressively replaced
by the runtime-owned `home`/`system`/`library`/`documents`/`inbox`
surfaces under one capability model.

## Non-Goals

- This doc does **not** propose deprecating pc2.net before its features
  have working capsule equivalents.
- It does **not** propose folding pc2.net's ambient-authority Node
  process into the runtime. The trusted core stays small; pc2.net
  features arrive as capsules under the same capability checks every
  other capsule gets.
- It does **not** propose committing to a specific calendar.
  Sequencing is intent; gates are real.

## Convergence Direction (Already in This Repo's Docs)

This plan is the assembly of commitments that are already in the repo:

- `state.md` — *"The default Home path must remain a KVM-independent
  browser-hosted adapter so macOS and Windows stay in scope without
  pretending to offer Linux parity."* And: *"Linux is the truthful
  full-runtime baseline. macOS is not yet a truthful full runtime
  target on this branch."*
- `ROADMAP.md / Later` — *"Decide how much Puter-derived UI remains
  after the runtime-owned Home/System contract is stable."*
- `ROADMAP.md / Later — Cross-platform runtime and host adapters` —
  Server / Desktop / Mobile / Kiosk host adapter modes share one
  capsule contract; *"Capsules don't know which host adapter they're
  on."*
- `TASKS.md / Now / 1` — *"Keep the default Home path compatible with
  macOS and Windows by avoiding KVM-only assumptions. Remove remaining
  donor/KVM-only assumptions from scripts and runtime special cases."*

What this doc adds is sequencing, gates, and pc2.net-specific mapping.

## Capsule Inventory (Current Branch)

For Planning Review Gate "Boundary clarity". Every capsule.json in
this repo:

| Substrate | Capsule | Role | Cross-platform today? |
|---|---|---|---|
| `wasm` | `home` | shell | yes |
| `wasm` | `home-cli` | shell | yes |
| `wasm` | `system` | app | yes |
| `wasm` | `chat-room` | app | yes |
| `wasm` | `chat-wasm` | app | yes |
| `data` | `inbox` | app | yes |
| `data` | `library` | app | yes |
| `data` | `documents` | viewer | yes |
| `data` | `gba-emulator` | viewer | yes |
| `data` | `gba-ucity` | content | yes |
| `microvm` | `shell` | shell | Linux/KVM only |
| `microvm` | `localhost-provider` | provider | Linux/KVM only |
| `microvm` | `did-provider` | provider | Linux/KVM only |
| `microvm` | `webspace-provider` | provider | Linux/KVM only |
| `microvm` | `ipfs-provider` | provider | Linux/KVM only |
| `microvm` | `tunnel-provider` | provider | Linux/KVM only |
| `microvm` | `ai-provider` | provider | Linux/KVM only |
| `microvm` | `llama-provider` | provider | Linux/KVM only |
| `microvm` | `agent` | app | Linux/KVM only |
| `microvm` | `chat` | app | Linux/KVM only |
| `microvm` | `notepad` | app | Linux/KVM only |

The user-visible Home product (Home, System, Inbox, Library, Documents,
Chat Room, GBA, chat-wasm) is already either `wasm` or `data`. What
keeps the runtime Linux-only is **critical provider capsules** being
`microvm`-substrate, in particular `localhost-provider` (filesystem) and
`shell` (orchestrator).

## pc2.net Feature → Runtime Counterpart

Mapping at the *user-visible* level, with the substrate the runtime
currently uses.

| pc2.net feature | Runtime counterpart today | Gap to converge |
|---|---|---|
| Desktop UI (Puter fork) | `home` (wasm) + `system` (wasm) + browser-hosted `/apps/home/` | Home contract still maturing; Library/Documents/Inbox slices landing |
| Wallet authentication | `did-provider` + `elastos://did/` + capability tokens | Wallet/WebConnect step in [ROADMAP.md § Four-quadrant runtime balance](../ROADMAP.md#near-term-direction); DID and capability machinery are in the runtime; WebConnect-style wallet pairing flow is open work |
| File storage / file manager | `localhost://Users/self/...` via `localhost-provider`; `library` capsule for browsing | `localhost-provider` substrate (see §macOS Host Adapter) |
| IPFS storage | `elastos://...` + `ipfs-provider` (microvm) | `ipfs-provider` substrate; pin/CID semantics already match the trust model |
| Real-time sync | Carrier P2P (`elastos://peer/`) | Carrier carries presence, gossip, room sync today; richer object/sync model is open work |
| WASM execution | `elastos-compute` (Wasmtime) + capsule `type: wasm` | Already first-class; capsule contract is the substrate-agnostic boundary |
| AI chat | `ai-provider` + `llama-provider` (microvm) + `agent` capsule | Substrates; the runtime-side capability contract is already there |
| DApp store | `webspace-provider` (microvm) + `localhost://WebSpaces/Marketplace` (intended) | Substrate + Marketplace WebSpace is open work (see ROADMAP "marketplace is a WebSpace") |
| Tunneling (`.ela.city`) | `tunnel-provider` (microvm) — wraps cloudflared/sing-box paths | Substrate; the runtime needs the tunnel as a provider, not as a built-in protocol |
| Backup / restore | localhost-provider + content-addressed `elastos://...` | Out of scope for the runtime-as-trusted-core; sits as a capsule using the same provider plane |
| Auto-update | `elastos update` + trusted-source / signed-release pipeline | Already first-class in the runtime; the launcher's update flow can defer to it |
| Access control (wallet-based perms) | Capability tokens + browser-session principals + Inbox approval | Already first-class architecturally; UX coverage of every flow is open work |

The pattern: **the runtime contract is largely in place**. What is
not in place on every platform is the *substrate* that lets providers
run with isolation. That is the heart of the macOS work below.

## macOS Host Adapter — Sequenced Slices

The runtime should reach macOS as a first-class host adapter without
faking Linux/KVM parity. Per `ROADMAP.md` host-adapter direction, the
contract is identical across adapters; only how capsules are presented
changes.

### Slice A — Workspace builds on non-Linux (Layer 1)

**Status:** complete on this branch in [commit 1dc27b4](https://github.com/Elacity/elastos-runtime/commit/1dc27b4).

- `elastos-crosvm/src/network_stub.rs` mirrors the public surface of
  `network.rs` on non-Linux and fails closed in `setup()` with an
  explicit "requires Linux" error.
- `lib.rs` cfg-gates `mod network` to Linux vs. stub.
- Pre-existing `rootfs.rs` data-disk test cfg-gated to Linux only
  (shells out to `mkfs.ext4`).
- One `null()` → `null_mut()` fix in an `elastos-guest` test for macOS
  `openpty` signature.

**Verified on Apple Silicon M5 Pro / macOS 26.4.1:**

- `cargo build -p elastos-server --release` succeeds.
- `cargo check --workspace --all-targets` succeeds.
- `cargo test -p elastos-crosvm` → 18/18 pass.
- `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings`
  clean on the touched crates.
- `./elastos --version`, `--help`, `setup --list`, `identity show`,
  `config show`, `source list`, `source --help`, `serve --help` all run.
- `elastos serve` initializes data dir, capability signing key, CA cert,
  TLS leaf cert, IP detection, TLS enable, `documents-provider`
  registration; then fails closed with `Error: localhost-provider not
  installed`.
- `elastos setup` (default `home` profile) correctly emits `[skip]
  shell — not available for unknown-arm64`, `[skip] localhost-provider
  — not available …`, `[skip] did-provider …`, `[skip]
  webspace-provider …`. Setup logic already knows what is unavailable
  on this platform.

**Linux behavior is bit-identical:** the real `network.rs` is
unchanged.

### Slice B — Critical providers available on non-Linux (Layer 2)

**Status:** scope concretized; implementation open.

**Smallest shippable slice:** make `localhost-provider` available as an
**in-process provider** on non-Linux. The runtime already registers
`documents-provider` in-process during `elastos serve` startup; the
mechanism exists. `localhost-provider` is a pure-Rust crate (no
Linux-only deps in `Cargo.toml`) so it links cleanly.

**Boundary clarity:**

- This must not weaken the trusted-core contract. An in-process
  provider on the macOS host still runs *under* the same capability
  token validation, audit, and provider-registry routing every other
  provider gets. The change is *isolation substrate*, not *authority
  model*.
- On Linux, the microVM-backed `localhost-provider` capsule remains the
  default. Substrate selection is host-adapter policy, not capsule
  policy.

**Affected quadrants:**

| Quadrant | Change |
|---|---|
| PC2/Home | none in this slice — Home reaches `localhost-provider` through the same provider registry call regardless of where the provider runs |
| Runtime | substrate-selection policy: when `elastos_crosvm::is_supported()` returns false, fall back to the in-process provider variant for capsules tagged as such |
| Carrier | none in this slice |
| Blockchain | none in this slice |

**Proof path (must pass before declaring Slice B done):**

1. `elastos serve` binds `127.0.0.1:3000` on macOS without `Error:
   localhost-provider not installed`.
2. A browser request to `/apps/home/` returns the Home surface assets.
3. A Home `/api/provider/localhost/*` call returns a typed object from
   `~/Library/Application Support/elastos/...`.
4. Existing source-line tests (`cargo test -p elastos-server home --lib`
   etc.) still pass on Linux. The in-process variant must not regress
   the microVM-backed path.
5. On Linux, microVM-backed `localhost-provider` remains the default
   and `setup --list` does not change behavior.

**Entropy risk avoided:** do not introduce a third substrate name or a
parallel capsule manifest schema. The capsule continues to declare
itself once; substrate selection is a runtime-policy detail bound to
`is_supported()` on the chosen provider.

**Not in this slice:**

- `did-provider`, `webspace-provider`, `ipfs-provider`,
  `tunnel-provider`, `ai-provider`, `llama-provider`. Each of these
  should follow the same pattern after Slice B proves it on
  `localhost-provider`. Doing one at a time keeps regressions
  attributable.
- A WASM substrate variant of these providers. Possibly correct
  long-term, but bigger than Layer 2. Layer 2 should not block on it.
- An Apple Hypervisor.framework substrate. Out of scope here; tracked
  separately.

### Slice C — Platform identity on macOS

**Smallest shippable slice:** the runtime currently reports the macOS
platform as `unknown-arm64` (observable in `elastos setup --list`
output and `elastos setup` skip lines). The setup logic does the right
thing despite the unknown label, but the public surface should be
honest: `aarch64-apple-darwin` (and the Intel equivalent if/when
relevant).

**Proof path:** `elastos setup --list` reports `aarch64-apple-darwin`
on macOS; existing Linux platform IDs unchanged.

### Slice D — Source-line proof script for the macOS path

The repo has Linux smokes (`local-carrier-setup-smoke`,
`home-frontdoor-smoke`, etc.). Add a macOS-aware variant or extend the
existing scripts to detect `aarch64-apple-darwin` and run the subset
that has provider availability under Slice B.

**Not yet in scope** — this slice waits on Slice B + C to define what
"works on macOS" means as a proof contract.

## Launcher Convergence — Sequenced Slices

[elastos-launcher](https://github.com/Elacity/elastos-launcher) today
downloads pc2.net + Node 20 + WireGuard + AmneziaWG + sing-box, runs
`pc2-node`, and exposes a Power On / Power Off + Update UI.

### Launcher Slice 1 — `--backend elastos-runtime` opt-in

**Smallest shippable slice:** add a runtime backend choice to
elastos-launcher. Default stays `pc2-node`. Users can opt into
`elastos-runtime` after Slice B above lands. The launcher's start/stop
logic in `src/main/pc2Manager.ts` gains a sibling `elastosManager.ts`
that supervises `elastos serve` under the same lifecycle: install via
trusted source, start under PM2 or equivalent, status check, log view,
update via `elastos update`.

**Boundary clarity:** the launcher does not learn capsule semantics or
become a second policy plane. It runs a daemon and forwards UI to
`http://127.0.0.1:3000/apps/home/`. That keeps PRINCIPLES "HTTP Is Edge
Transport, Not Product Truth" honest.

**Affected quadrants:**

- PC2/Home: launcher gains a runtime-backend switch and learns to open
  `/apps/home/` instead of `:4200`
- Runtime: nothing changes; the launcher consumes the existing serve +
  trusted-source + update contracts
- Carrier: nothing changes
- Blockchain: nothing changes

**Proof path:** on macOS, launcher with `--backend elastos-runtime`
opens Home in the bundled browser/webview. Power On installs the
runtime via trusted source if missing, runs `elastos serve`, surfaces
status. Power Off stops the daemon cleanly. Switching back to
`pc2-node` is a one-click toggle.

### Launcher Slice 2 — install-parity check for the runtime backend

The launcher's existing "Install Parity Rule" (CONTRIBUTING.md) says
the GUI must install the same tool set as the terminal scripts. For
the runtime backend, the analogous rule is: the GUI must always run
the same `elastos setup --profile <p>` the terminal install would run,
with no GUI-only side-effects. Codify this as a unit test in the
launcher repo.

### Launcher Slice 3 — UX convergence

Once both backends ship features that map cleanly, the launcher's
"Open Cloud" button can target the runtime's Home surface by default
and keep pc2.net as a fallback during deprecation. The actual
decommissioning of pc2.net is `ROADMAP.md / Later` territory and is
out of scope for the launcher slice.

## Sequencing Summary

The order below is intent. Each step ends on a proof gate; no step
ships until its gate passes.

1. **Slice A** — workspace builds on non-Linux ✅ done
2. **Slice B** — in-process `localhost-provider` on non-Linux →
   `elastos serve` reaches a functional Home on macOS
3. **Slice C** — honest macOS platform identity in setup output
4. **Slice D** — macOS source-line smoke script reflecting the realised
   provider matrix
5. **Wallet-backed identity + WebConnect** ([ROADMAP.md § Near-term §
   Four-quadrant runtime balance](../ROADMAP.md)) — first cross-quadrant
   move. Independent of macOS work; both can proceed in parallel.
6. **Other microvm providers** (`did-`, `webspace-`, `ipfs-`,
   `tunnel-`, `ai-`, `llama-`) repeat the Slice B pattern one at a
   time
7. **Launcher Slice 1** — `--backend elastos-runtime` opt-in, lit once
   Slice B + C are real on macOS
8. **Spaces / network drives** ([ROADMAP.md § Near-term § Four-quadrant
   runtime balance](../ROADMAP.md))
9. **Capsule publish/install registry** ([ROADMAP.md § Near-term §
   Four-quadrant runtime balance](../ROADMAP.md))
10. **Launcher Slice 2 + 3** as the underlying capsule and registry
    contracts harden

Apple Hypervisor.framework substrate, dedicated browser capsule, and
deeper Puter-derived UI deprecation are explicitly later. They should
not distort the immediate sequence above.

## Decision Log Stub

Track decisions here when they pin down a previously open choice.

- **2026-05-21** — macOS port chosen over Linux-VM-only workflow.
  Driver: contributors on macOS, runtime's own commitment in `state.md`
  that the default Home path stays KVM-independent.
- **2026-05-21** — Slice B "smallest shippable" is in-process
  `localhost-provider` only, not WASM-substrate provider rework.
  Driver: matches existing `documents-provider` in-process mechanism,
  fewer new concepts.
