# DEV_SETUP — the local dKMS / dDRM / commerce harness

Everything here is read off the scripts as they exist on this branch. Where a flag or an
env var is named, it is because the script reads it — nothing below is aspirational.

Start at [README.md](README.md) if you have not. This page is the *how do I run it*
companion; [RUN_E2E.md](RUN_E2E.md) is the operator runbook for a real publish-and-open.

---

## 1. Prerequisites

**Required for everything:** the pinned toolchain. `rust-toolchain.toml` declares channel
`1.89.0` with `rustfmt`, `clippy`, and the `wasm32-wasip1` target, so rustup installs the
wasm std for you — you do not need `rustup target add`.

**Per-path extras** (each is checked, and each failure mode is stated by the script that
needs it):

| Tool | Needed by | If missing |
|---|---|---|
| `wasmtime` | `scripts/ddrm-chain-smoke.sh` | the script exits 1 with an install hint; `ddrm-verify.sh` skips that gate clean |
| `ffmpeg` + `ffprobe` | media (video/audio) mint via `run-creator-gateway.sh` | the script WARNs and continues; media mint then fails (`media-provider` has no `ffmpeg_path`) |
| `ipfs` (kubo) | content publish via `run-creator-gateway.sh` | the script WARNs and continues; publish fails with no asset/metadata CID |
| `docker` | `scripts/dev/dkms-docker/up.sh` | hard fail (`FAIL: docker is required`) |
| `python3` | quorum descriptor assembly in several dev scripts | hard fail |
| `node` | the repo gates (`home-entropy-check.mjs`) | hard fail |

Nothing here needs a chain, an IPFS daemon, or a deployed node **until** you choose the
operator path in §3.

---

## 2. The cheapest proof: the smoke scripts

These build the real capsule binaries and drive them cross-process. No chain, no IPFS, no
node deployment, no network.

```bash
scripts/ddrm-consumer-smoke.sh      # drm/open → rights → key → decrypt, end to end
scripts/ddrm-producer-smoke.sh      # mint a CEK now, escrow it, recover it, re-seal, decrypt
```

`ddrm-consumer-smoke.sh` takes `--backend reference|dkms` (default `reference`), plus
`--threshold` and `--nodes N`. The named siblings are thin `exec` wrappers around exactly
those flags — read the wrapper's header comment for what each one proves:

| Script | What it drives |
|---|---|
| `ddrm-consumer-smoke.sh` | the in-runtime durable-key-store authority (`reference`) |
| `ddrm-consumer-dkms-smoke.sh` | `--backend dkms` — an external, secret-holding authority node |
| `ddrm-consumer-dkms-threshold-smoke.sh` | `--threshold` — a real **2-of-2** rail, XOR-split CEK, both shares unwrapped only inside the decrypt boundary |
| `ddrm-consumer-dkms-quorum-smoke.sh` | `--threshold --nodes 3` — a real **2-of-3** Shamir quorum over GF(256), with live node-kill failover |
| `ddrm-consumer-dkms-tcp-smoke.sh` | the 2-of-2 rail over **real TCP** with the mutually-authenticated sealed channel (plaintext recover refused, downgrade dropped, MITM tamper dropped) |
| `ddrm-producer-smoke.sh` | the producer half: mint → escrow → recover → re-seal → decrypt in one run |
| `ddrm-publish-smoke.sh` | `publish-provider` → `chain-provider`: one identity KID → contentId → mint calldata |
| `ddrm-market-smoke.sh` | publish → chain → `content-market`: the listing decodes back to the same content id |
| `ddrm-chain-smoke.sh` | all four chain providers under **wasmtime**, fail-closed |

**The standing gate before a rebase or PR** is `scripts/ddrm-verify.sh`, which aggregates
four checks in cost order: contract drift (`ddrm-drift-check.sh`), cross-impl parity
(`pc2-conformance.sh`, skips clean when the PC2 repo is absent), the test ladder with
expected counts (`ddrm-ladder-check.sh`), and the WASI smoke (`ddrm-chain-smoke.sh`, skips
clean without `wasmtime`). Set `DDRM_VERIFY_FAST=1` to skip the two heavy gates.

### 2.1 Known gap — `ddrm-runtime-open` live-lifecycle scenarios vs. the hardened node

The `--backend dkms` smoke variants above drive the `ddrm-runtime-open` orchestrator. Its main
verify-mode **rail** (the happy-path open) is already migrated to the hardened node: node identity is
created **offline** by the `dkms-authority provision` subcommand (`provision_dkms_node`) *before* a
**load-only** daemon is started — the daemon never creates or selects an identity over the wire
(DKMS-7).

Its **adversarial-probe and live-lifecycle scenarios are NOT yet migrated** and are RED against the
hardened node. They are opt-in (verify mode + a `dkms` backend + a `--threshold`/`--nodes` config),
and are **not** part of `just verify-capsules`, which is green. The remaining migration, per
`scripts/dev/ddrm-runtime-open/src/main.rs` (the `PENDING MIGRATION` banner above
`fn dkms_node_adversarial_probe`):

1. **Wire `op:init` is refused** on every transport. ~25 call sites still send `{"op":"init"}`.
   The ones that hit an already-provisioned running daemon (`dkms_node_adversarial_probe`,
   `dkms_malformed_frame_is_refused`, `dkms_tcp_channel_adversarial_gates`) simply drop the init and
   keep the `hello` round-trip. The ones that read `(vk, recipient)` from the init response
   (`dkms_threshold_probe`, the rotation/quorum/DKG scenarios) must instead take identity from
   `provision_dkms_node`.
2. **Unprovisioned `start_dkms_daemon`.** Scenarios that spawn their own daemons (e.g.
   `dkms_threshold_probe`) point at a keystore path that was never provisioned, so a load-only daemon
   now fails at startup. Each such spawn needs a preceding `provision_dkms_node`.
3. **v1 lifecycle auths are rejected.** The rotation/revocation, quorum-rotation,
   quorum-reconfigure, and DKG contribute/install/attest scenarios build v1
   `reshare_aad`/`dkg_aad`-style authorizations; the hardened node verifies operator auth only
   against the v2 `ddrm_envelope::lifecycle` canonical manifest digest (DKMS-5). Re-encode with the
   shared v2 encoder.

This is a comprehensive rewrite of the live-lifecycle driver (~15 scenario functions) that can only
be verified end-to-end under the full offline orchestration `scripts/ddrm-consumer-smoke.sh` drives.
It is intentionally deferred; the security gate does not depend on it.

---

## 3. The full operator path: `run-creator-gateway.sh`

```bash
./scripts/dev/run-creator-gateway.sh
# then open http://localhost:8090/apps/home/
```

Use `localhost`, **not** `127.0.0.1` — WebAuthn rejects bare IPs, so the money verbs'
passkey step-up cannot complete on an IP origin.

### What the script actually does, in order

1. **Checks external tools** — `ffmpeg`/`ffprobe` and `ipfs`. Both are WARN-and-continue,
   not fatal.
2. **Builds the provider binaries** with their canonical feature sets:
   `encrypt-provider --features escrow`, `media-provider`, `publish-provider`,
   `chain-provider`, `wallet-provider`, `ipfs-provider`,
   `rights-provider --features chain-rights`,
   `decrypt-provider --features rail-stream,rail-mint,pdf-render`,
   `key-provider --features key-authority-ref`, `dkms-authority`, `dkms-keygen`,
   `object-provider`. It then **discovers** every wasm-guest app capsule (any
   `capsule.json` whose entrypoint ends in `.wasm` next to a `Cargo.toml`) and builds it
   for `wasm32-wasip1 --release` — no hardcoded list, so a new capsule is picked up
   automatically and a capsule that fails to build fails *closed* (its tile will not open)
   instead of aborting the launch.
3. **Builds the gateway** (`elastos-server`) `--features dev-modes`, and the
   `scripts/dev/ddrm-media-authority` helper.
4. **Provisions the quorum** — see §3.2.
5. **Exports the `ELASTOS_*_BIN` dev overrides** so the gateway trusts the locally built
   capsules without an installed signed manifest, then launches
   `elastos gateway --addr 127.0.0.1:8090` in the **foreground** (not `exec`) so the dKMS
   daemons it spawned are reaped on exit.

### 3.1 Flags and the build profile

```
scripts/dev/run-creator-gateway.sh [--addr 127.0.0.1:8090] [--data-dir DIR] [--remote] [--carrier]
```

- `--addr` / `--data-dir` — also accept the `--flag=value` form.
- `--remote` — seal and recover against the **live** 3 geo nodes instead of the local
  quorum (same as `ELASTOS_DKMS_REMOTE=1`). Bare `--remote` uses the **deprecated**
  WireGuard `dkms0` mesh transport and requires this machine to be on that VPN.
- `--carrier` — reach the live nodes by `did:key` over Carrier/iroh (implies `--remote`;
  same as `ELASTOS_DKMS_CARRIER=1`). No VPN, no mesh. **This is the preferred live path.**

Flags override the matching env when set; the env still works when the flag is absent.

`ELASTOS_BUILD_PROFILE=release` builds every provider and the gateway optimized. Worth
doing: the provider bridge is serial, so one slow debug provider stalls the rest — release
providers run roughly 10–30× faster.

### 3.2 How the quorum gets provisioned

**Local (the default, zero-config).** If `<data-dir>/dkms/quorum.json` and
`quorum-nodes.json` are absent, the script calls
`scripts/dev/ddrm-provision-quorum.sh <data-dir>/dkms`, which stands up three long-lived
`dkms-authority` identities, each with its own durable master-seed store, and writes:

- `quorum.json` — the **public-only** descriptor (verifying + recipient keys). The Create
  portal reads this and seals each minted CEK share to those recipient pubkeys. No secret
  material; safe to publish.
- `quorum-nodes.json` — the **operator-private** sidecar (per-node store path + intended
  socket endpoint). File paths, not secrets; the master seed lives only inside each store.

Minting needs only the public descriptor — sealing is pure local crypto and no node need be
running. The daemons matter only for **recovery**. So the script then starts the three
daemons on their durable stores, mints a fresh per-boot caller identity
(`dkms-keygen derive-vk`) that the nodes allow-list, and assembles
`quorum-open.json` — a v2 descriptor carrying each node's live endpoint — which serves both
the mint seal and the `key-provider` recover. If any of that fails it prints a WARN and
**mint still works**; only opening a dKMS asset is disabled (it 503s).

It also spawns one **warm `key-provider` daemon** on a Unix socket
(`<quorum-dir>/key-provider.sock`) so opens after the first reuse the live node handshake
sessions. `KEY_PROVIDER_LISTEN` is passed inline to that child only — never exported — so a
fallback-spawned `key-provider` still runs in plain stdio mode. If the socket never appears
the socket env is simply not advertised: opens fall back to per-open spawn. **Latency only,
never access.**

**Remote.** With `--remote`/`--carrier` the script reads
`~/.elastos-dkms/dkms-authority.carrier.json` (Carrier) or `dkms-authority.v2.json`
(legacy mesh) plus `~/.elastos-dkms/secrets/caller.seed`, overridable via
`ELASTOS_DKMS_REMOTE_DESCRIPTOR` and `ELASTOS_DKMS_REMOTE_CALLER_SEED`. Both files must
exist or the script hard-fails. Remote mode **defaults `ELASTOS_DDRM_RIGHTS=chain`** — it is
the live-rail proof. In Carrier mode it also builds and starts the
`dkms-carrier-client` sidecar on `127.0.0.1:9444` (`DKMS_CARRIER_CLIENT_ADDR`), pre-warmed
with the node `did:key`s so the first open lands on a warm path.

### 3.3 Node-side trustless authorization (chain mode only)

When `ELASTOS_DDRM_RIGHTS=chain`, each local daemon is handed its **own** read-only Base
capability — `DKMS_CHAIN_RPC_POOL`, `DKMS_RIGHTS_CONTRACT`, `DKMS_RIGHTS_SELECTOR`,
`DKMS_CHAIN_ID` — so it authorizes a wallet-signed `AccessGrantV1` itself, by verifying the
signature and reading `hasAccessByContentId` from Base. That is a faithful local proxy for
the sovereign quorum, no mesh needed. In `dev` / `chain-mock` these stay unset and the
enrolled-caller path is used instead (the browser MetaMask grant flow is only offered in
chain mode; `prepare-grant` 400s otherwise).

**Owner-only entitlement binding (DKMS-1).** The node binds the on-chain check to the wallet
identity that actually signed the delegation: it normalizes and queries **exactly `owner_address`**
(`covered_addresses = [owner]`, `MAX_COVERED_ADDRESSES = 1`). A wallet signature over a list is not
proof the signer controls every listed address, so a **multi-address v1 grant fails closed** — it is
not silently honored. The sidecar's grant builder already emits the safe owner-only default, so
existing owner-only v1 grants stay valid; covering an additional distinct address would require a
separately-versioned relation proof (not shipped). EOA and EIP-1271 owners follow the same rule, and
both `hasAccessByContentId` and the EIP-1271 read fail closed on RPC disagreement or insufficient
reachability (DKMS-2) rather than trusting a single endpoint.

### 3.4 AV forensic watermarking

The script enables per-buyer forensic variants (`ELASTOS_AV_VARIANTS=1`) and persists one
bias master per data dir at `<quorum-dir>/av-bias.master`, exported as
`ELASTOS_AV_MASTER_B64`. It must be stable across restarts and identical on the mint and
serve sides, or the manifest's bias commitment will not match and the open honestly falls
back to the single encode. Forensic marking activates only on the wallet-grant (**chain**)
open path.

---

## 4. A real 3-node quorum on one machine: `scripts/dev/dkms-docker`

```bash
cd scripts/dev/dkms-docker
./up.sh            # build + start + mint the caller identity + assemble the descriptor
./up.sh down       # stop, KEEP the volumes (master seeds survive; same identities on restart)
./up.sh destroy    # stop AND delete the volumes (a brand-new quorum next time)
```

Three isolated containers, each one full quorum node: `dkms-authority` (the secret holder)
plus a `dkms-carrier-node` iroh bridge, each with its own private `/data` volume for the
master seed and carrier identity — the same shape as the three live geo nodes. The image is
built from the repo root but copies in only the standalone crates and their path deps, never
the whole workspace.

`up.sh` mints the runtime **caller** identity once (`dkms-keygen keygen --role caller`) and
allow-lists its VK on all three nodes via `.env`; the seed at `shared/caller.seed` is
secret. It waits for each node to publish its identity and `did:key`, assembles
`shared/dkms-authority.carrier.json` (2-of-3, `carrier:did:key:…` endpoints), asserts the
three identities are distinct, and then computes and pins the **node-set id**
(`DKMS_AUTHORITY_NODE_SET_ID_B64` — base64 SHA-256 over `t` and the ordered node VKs).
That pin is mandatory in a release-built node: without it, every grant-authorized recover
fails closed. It recreates the nodes only when the pin actually changed.

It prints the four exports that point a runtime at the result:

```bash
export ELASTOS_DKMS_REMOTE=1
export ELASTOS_DKMS_CARRIER=1
export ELASTOS_DKMS_REMOTE_DESCRIPTOR="…/shared/dkms-authority.carrier.json"
export ELASTOS_DKMS_REMOTE_CALLER_SEED="…/shared/caller.seed"
./scripts/dev/run-creator-gateway.sh
```

Each node's `dkms-authority` is also mapped to `127.0.0.1:{1,2,3}9443` **purely for
host-side debugging** — the runtime reaches nodes over Carrier `did:key`, not those ports.
The live nodes expose 9443 only on their private mesh, never publicly.

**Local Carrier without Docker:** `scripts/dev/dkms-local-carrier-up.sh up|down` puts
Carrier bridges in front of the *existing* durable node stores, so the quorum identities
stay exactly the ones already sealed to and only the transport front changes. It writes to
`~/.elastos-dkms/` (descriptor, per-bridge seeds for stable `did:key`s across restarts,
logs, pidfile) — which is what `run-creator-gateway.sh --carrier` reads.

---

## 5. Dev-mode caveats — read before you trust a local result

The whole local harness is built `--features dev-modes`. That feature is **off by default**
(`default = []` in `elastos/crates/elastos-server/Cargo.toml`), so a plain
`cargo build --release` is secure by construction. What it re-enables:

- **`ELASTOS_DDRM_SUBJECT` is a dev-build pin, and a release binary handed it refuses to
  boot.** It forces *every* principal's on-chain rights check to one operator-chosen
  wallet, so aiming it at a wallet holding the access token unlocks the asset for the whole
  node. The honouring branch is `#[cfg(feature = "dev-modes")]`; the release guard is
  `enforce_release_build_rights_safety` in
  `elastos/crates/elastos-server/src/api/rights_authority.rs`, called at boot from
  `gateway_cmd.rs`. The same guard rejects `ELASTOS_DDRM_RIGHTS=dev|chain-mock` in a release
  build — in a release build `chain` is the only selectable rights mode.
- **The managed-account autosign path is `dev-modes` only.** `ELASTOS_DDRM_BUY_SIGN=wallet`
  lets the runtime self-sign a buy (`api/buy_authority.rs`, `api/wallet_signer.rs`); it also
  applies to `chain-mock`. A release build never self-signs — it broadcasts an
  externally-signed tx (`ELASTOS_DDRM_BUY_SIGNED_TX`) or hands back the unsigned
  transaction for an external wallet.
- **`key-provider` is deliberately NOT built with `dev-modes`** — it keeps
  `--features key-authority-ref`. The production dKMS quorum path is compiled while the
  forgeable `reference` backend stays fenced out at selection, so the local gateway
  exercises the *same* key-release posture the production build ships.
- **`dkms-authority` hard-forbids `dev-modes` in a release build.** The dev harness builds it
  with `dev-modes` only in the debug profile; under `ELASTOS_BUILD_PROFILE=release` it is
  built production-posture.
- **Rights mode must match how the asset was minted, and the quorum rail must match where it
  was sealed.** A mismatch fails closed — it never silently downgrades. The symptom→cause
  table is in [../DKMS_OVER_CARRIER.md](../DKMS_OVER_CARRIER.md).

A local green result therefore proves the *rail*, not the *posture*, unless you ran it
`ELASTOS_DDRM_RIGHTS=chain` against a real wallet.

---

## 6. Environment surface (what the harness sets, and what you can pin)

| Variable | Meaning |
|---|---|
| `ELASTOS_BUILD_PROFILE` | `debug` (default) or `release` for every provider + the gateway |
| `ELASTOS_<NAME>_PROVIDER_BIN` | per-provider dev trust override; bypasses signed-manifest verification for that exact path |
| `ELASTOS_DDRM_DECRYPT_BIN` | the decrypt binary for the media-authority playback rail (same binary as `ELASTOS_DECRYPT_PROVIDER_BIN`, second lookup) |
| `ELASTOS_MEDIA_PROVIDER_CONFIG` | JSON `{ffmpeg_path, scratch_dir}`; unset ⇒ `package` fails closed |
| `ELASTOS_DDRM_RIGHTS` | `dev` (default in the harness) / `chain-mock` / `chain` |
| `ELASTOS_CHAIN_BASE_RPC` | Base RPC; defaulted to `https://mainnet.base.org` in chain mode |
| `ELASTOS_DDRM_RIGHTS_CONTRACT` / `_SELECTOR` / `ELASTOS_DDRM_CHAIN_ID` | override the Base AuthorityGateway defaults (`0x09dBe796…`, `0x54d42821`, `8453`) |
| `ELASTOS_DKMS_QUORUM_DESCRIPTOR` | the public descriptor the Create portal seals to |
| `ELASTOS_DDRM_QUORUM_OPEN_DESCRIPTOR` / `_CALLER_SEED_B64` | the open-side descriptor + the caller secret the nodes allow-list |
| `ELASTOS_DDRM_KEY_PROVIDER_BIN` / `_SOCKET` | the key-provider the open helper drives; socket = the warm daemon |
| `ELASTOS_DKMS_REMOTE` / `ELASTOS_DKMS_CARRIER` | live-quorum mode and its transport |
| `ELASTOS_DKMS_REMOTE_DESCRIPTOR` / `_CALLER_SEED` | override the `~/.elastos-dkms/` defaults |
| `DKMS_CARRIER_CLIENT_ADDR` | Carrier sidecar listen address (default `127.0.0.1:9444`) |
| `ELASTOS_AV_VARIANTS` / `ELASTOS_AV_MASTER_B64` | forensic variant switch + the shared bias master |

Node-side (read by `dkms-authority` itself): `DKMS_AUTHORITY_LISTEN`,
`DKMS_AUTHORITY_KEY_STORE`, `DKMS_AUTHORITY_ALLOWED_CALLERS`,
`DKMS_AUTHORITY_NODE_SET_ID_B64`, `DKMS_AUTHORITY_OPERATOR_VK`, `DKMS_CHAIN_RPC_POOL`,
`DKMS_RIGHTS_CONTRACT`, `DKMS_RIGHTS_SELECTOR`, `DKMS_CHAIN_ID`.

---

## 7. macOS vs Linux — what is actually different

**Both are supported**, and `run-creator-gateway.sh` says so in its own header. The
differences that bite:

- **Data dir.** macOS `~/Library/Application Support/elastos`; elsewhere
  `${ELASTOS_DATA_DIR:-~/.elastos}`. The quorum lands in `<data-dir>/dkms`.
- **No signed manifest on macOS.** The platform key resolves to `unknown-arm64`, which has
  no manifest entry, so fail-closed verification would reject providers that have no Rust
  source crate in this repo (`localhost`, `did`, `site`, `tunnel`, `webspace`) and the
  desktop `shell`. The script points `ELASTOS_<NAME>_BIN` / `ELASTOS_SHELL_BIN` at the
  installed Mach-O arm64 binaries under `<data-dir>/bin` so the managed child runtime
  (`elastos serve`, spawned without `env_clear`) inherits that explicit dev trust.
- **bash 3.2.** macOS still ships it, so the dev scripts avoid `mapfile` and use
  `set -u`-safe empty-array expansion. Keep new script code at that level.
- **Isolation substrate.** Linux gets the `crosvm` microVM on `/dev/kvm`; on macOS
  `crosvm::is_supported()` is `false` and microVM launch **fails closed** — the dDRM
  providers run as host subprocesses over stdio JSON, a legitimate host adapter with the
  identical typed request/receipt contract. `vz` (Apple Virtualization.framework) is the
  probed path to parity; `scripts/dev/mac-vz-feature-check` is the feasibility probe. Full
  table in [ARCHITECTURE.md §3](ARCHITECTURE.md#3-isolation-substrates--macos-vz-vs-linux-crosvm).
- **Docker.** `dkms-docker` builds a Linux image either way; on macOS it runs inside Docker
  Desktop's VM. Because the runtime reaches the nodes over Carrier `did:key` rather than
  published ports, nothing about that indirection needs extra wiring.
- **CI.** `just verify-ci` is the full `verify` minus `local-carrier-setup-smoke`, which a
  stock GitHub runner cannot reach; that one is covered on a Carrier-capable Linux box.

---

## 8. Gates to run before you push

```bash
just alignment-check                  # scripts/check-wci-alignment.sh — contract drift, fail-closed
node scripts/home-entropy-check.mjs   # includes "markdown local links must resolve"
scripts/ddrm-verify.sh                # the dDRM contract + ladder + WASI aggregate
just verify                           # the full pre-commit gate (fmt, clippy -D warnings, workspace tests, smokes)
```

`just verify-capsules` covers the dDRM capsule crates the workspace gate does not reach —
`decrypt-provider` (`rail-stream,rail-mint,pdf-render,pq-envelope`), `ddrm-envelope`
(`access-grant,av-variants`), `ddrm-media-authority` — under the **same** canonical feature
sets `run-creator-gateway.sh` builds, plus the AV forensic cross-language weld
(`tools/av-forensics/test_canonical.py`, pure stdlib).

If you touch the money path, note that `verify-ci` also runs
`cargo test -p elastos-server --lib --features dev-modes`: the buy-path ratchets are
`#[cfg(feature = "dev-modes")]`, and a ratchet the gate never compiles cannot ratchet.

---

## 9. When it does not work

| Symptom | Cause |
|---|---|
| WebAuthn / passkey step-up never prompts | you opened `127.0.0.1` instead of `localhost` |
| `WARN: <port> is in use` | the ElastOS desktop app or a prior gateway holds the host lock — `osascript -e 'quit app "ElastOS"'`, or `lsof -ti tcp:8090 \| xargs kill` |
| mint works, opening a minted asset 503s | the recovery quorum did not come up; check `<data-dir>/dkms/daemon.log` and the `quorum OPEN disabled` WARN |
| media mint fails, non-media mints fine | `ffmpeg`/`ffprobe` not on PATH ⇒ no `ELASTOS_MEDIA_PROVIDER_CONFIG` ⇒ `package` fails closed |
| publish yields no CID | `ipfs` (kubo) not on PATH |
| first open takes ~30 s on the live rail | a cold cross-continent dial; the Carrier sidecar pre-warm and the warm key-provider daemon are the fix — check both came up |
| a release binary refuses to boot | the rights-safety guard: you set `ELASTOS_DDRM_SUBJECT`, or a non-`chain` `ELASTOS_DDRM_RIGHTS`, in a release build. Working as designed |
| rights or quorum mismatch | see the symptom→cause table in [../DKMS_OVER_CARRIER.md](../DKMS_OVER_CARRIER.md) |
