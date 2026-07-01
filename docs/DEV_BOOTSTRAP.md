# Dev Bootstrap — one command to boot the full browser shell

There is **one** command that builds every backend and launches the browser shell — Home,
Library, Connect wallet, Create, and **DDRM open (double-click → wallet-sign → play)** — on
macOS/arm64 or Linux:

```bash
# Open your OWN protected assets (sealed to your LIVE dKMS geo quorum): Carrier transport +
# wallet-signed chain rights. THIS is the everyday command.
ELASTOS_DKMS_CARRIER=1 scripts/dev/run-creator-gateway.sh
```

Then open **http://localhost:8090/apps/home/** — **`localhost`, not `127.0.0.1`** (WebAuthn
rejects bare-IP relying-party IDs). Opening a protected asset prompts **MetaMask to sign** an
access grant; your geo nodes verify the signature + your on-chain access and release the key
2-of-3.

> This is a **developer bootstrap**, not a production install. It builds from source and points
> the per-provider `ELASTOS_<NAME>_BIN` dev overrides at the local builds (see
> [why](#why-a-fresh-clone-needs-this)). Production installs verify a signed release manifest.

---

## Modes (pick the one that matches where your keys live)

`run-creator-gateway.sh` builds everything the same way; the env you pass selects the **dKMS
quorum** and the **rights gate**:

| You want to… | Command | Quorum | Rights |
|---|---|---|---|
| **Open your real assets** (sealed to the live geo nodes) | `ELASTOS_DKMS_CARRIER=1 scripts/dev/run-creator-gateway.sh` | LIVE 3 geo nodes over Carrier (no VPN) | `chain` (wallet-signed grant) |
| Mint/open **new local test assets** only | `scripts/dev/run-creator-gateway.sh` | throwaway local 2-of-3 on this box | `dev` (local attestation, no wallet) |
| Live nodes over the legacy WireGuard mesh | `ELASTOS_DKMS_REMOTE=1 scripts/dev/run-creator-gateway.sh` | LIVE geo nodes over dkms0 VPN | `chain` |

**Critical:** an asset can only be opened by the **same quorum it was minted to**. Your existing
assets are escrowed to your **live geo nodes** — the throwaway *local* quorum can never open them
(you'll see `0 of 3 … foreign/tampered escrow`). Use `ELASTOS_DKMS_CARRIER=1`.

Live-mode credentials must exist (they already do on this machine):
`~/.elastos-dkms/dkms-authority.carrier.json` and `~/.elastos-dkms/secrets/caller.seed`.

### Rights: `dev` vs `chain`
- `chain` (live-mode default) — the node-side trustless path: the browser runs the MetaMask
  grant flow, you **sign**, and each node verifies the wallet-signed `AccessGrantV1` + reads
  `hasAccessByContentId` from Base. Requires a **linked EVM wallet**. This is how real assets open.
- `dev` (local default) — derives a placeholder subject, no wallet. Works **only** against a
  local quorum whose nodes allow-list your caller seed; the **live nodes reject it**
  (`anonymous caller must present a wallet-signed access grant`).

Override with `ELASTOS_DDRM_RIGHTS=chain|dev|chain-mock`.

---

## Access model — how any user opens (there is no per-user allow-list)

The whole point of the dKMS quorum: **anyone can call it; no one gets a key unless their wallet
holds the asset's access token on-chain.** The gate is the smart contract, not an operator list.

Concretely, the node's trustless gate (`capsules/dkms-authority/src/node_chain.rs::authorize_access`):
1. **Any** caller connects (anonymous — the "millions-of-sovereign-runtimes" posture).
2. The caller presents a **wallet-signed `AccessGrantV1`**.
3. **Each node verifies the wallet signature itself and reads `hasAccessByContentId` from the
   contract itself**, then releases its 2-of-3 share. No enrollment, no trusted receipt.

So onboarding a new user/node needs **no per-user secret and no manual node step**:

| What the new user needs | Secret? | Where it comes from |
|---|---|---|
| App + one command | no | this repo |
| Quorum **descriptor** (node endpoints/pubkeys) | **no — public** | shipped/fetched/handed over (`dkms-authority.carrier.json`) |
| A caller identity (transport session key) | their own | generated locally (the local path already does `/dev/urandom`) — **not** shared |
| Access to the asset | — | **their own wallet holding the token on-chain** (the real gate) |

> **`DKMS_AUTHORITY_ALLOWED_CALLERS` is NOT the security boundary.** Since W3/D4 it is a *soft,
> optional* DoS/handshake knob. Left set, it refuses a *session* to unrecognized caller keys —
> which would force manual per-user enrollment and defeats the model. For the sovereign-runtime
> posture it must be **empty/unset** on the nodes; the wallet-signed grant + on-chain token then
> decide everything. Dropping it is explicitly safe (an anonymous caller can never forge
> `allowed:true` — recover always requires a valid grant). If your live nodes still have it set,
> that's a legacy holdover to clear (one env change + `systemctl restart dkms-authority` per node,
> done one at a time to keep 2-of-3 live).

## What the command builds

- **All wasm-guest capsule backends** (`wasm32-wasip1`) — auto-discovered from each
  `capsule.json`, so the tile list can't drift and 404 (Home, Library, Browser, Marketplace,
  Documents, Inbox, System, Wallet, Inspector, GBA, …).
- **The provider spine, with correct features** — `encrypt`(escrow), `media`, `publish`,
  `chain`, `wallet`, `ipfs`, `rights`(chain-rights), `decrypt`(rail-stream,rail-mint,pdf-render),
  `key-provider`(key-authority-ref), `object`, plus the `ddrm-media-authority` helper.
- **The dKMS open rail** — `dkms-authority`/`dkms-keygen` (local mode) **or** the
  `dkms-carrier-client` sidecar (live mode), a warm `key-provider` daemon, and the assembled
  OPEN descriptor + caller seed.
- **The gateway** (`elastos-server`) with `--features dev-modes` so any rights mode is selectable.

External tools it expects on `PATH` (WARN-only): `ffmpeg`/`ffprobe` (media), `ipfs`/kubo (content).

---

## Why a fresh clone needs this

The runtime resolves backends **from disk at launch**. A fresh clone ships almost none built,
and on macOS/arm64 the release manifest has **no platform entry** (`unknown-arm64`), so provider
verification fails closed. The escape hatch (`binaries.rs`,
`verify_component_binary_with_data_dir`): when `ELASTOS_<NAME>_BIN` points **exactly** at a
binary, it is trusted without manifest verification. This script builds every backend and sets
exactly those overrides. Providers whose `capsule.json` says `"type": "microvm"` still run as
**native subprocesses** here — no KVM or Apple Virtualization needed for Library/DDRM/wallet.

---

## Troubleshooting — error → cause → fix

| Symptom | Cause | Fix |
|---|---|---|
| `WASM file not found: …/<name>.wasm` | capsule wasm backend not built | rerun the command (it builds every wasm-guest capsule) |
| `no provider for scheme: object` / `wallet` | that native provider not built/wired | rerun the command (builds + sets `ELASTOS_<NAME>_BIN`) |
| `media-authority helper not found` | `ddrm-media-authority` not built | rerun the command |
| `wallet not linked: a chain rights check needs the principal's EVM address` | `chain` mode, no linked wallet | Connect wallet in the shell, then open (or use `ELASTOS_DDRM_RIGHTS=dev` for local assets) |
| `no platform entry for unknown-arm64` (provider skipped) | macOS release manifest has no arm64 entry | the `ELASTOS_<NAME>_BIN` override (set by the script) trusts the local build |
| `dKMS quorum open not configured (set ELASTOS_DDRM_QUORUM_OPEN_DESCRIPTOR)` | no quorum wired | run with `ELASTOS_DKMS_CARRIER=1` (live) or plain (local) — the script assembles the descriptor |
| `2-of-3 quorum NOT met — 0 of 3 … foreign/tampered escrow` | opening a live-quorum asset against the **local** quorum | use `ELASTOS_DKMS_CARRIER=1` (the asset is sealed to your geo nodes) |
| `anonymous caller must present a wallet-signed access grant` | live nodes hit in **dev** rights mode | use `chain` rights (the live-mode default; don't pass `ELASTOS_DDRM_RIGHTS=dev`) |
| `502 could not open owned asset from the dKMS quorum` | quorum recover failed | read the node reason in the gateway log (usually one of the two rows above) |
| `another ElastOS host already owns this data dir` | single host lock (one host per data dir) | quit ElastOS.app and/or `lsof -ti tcp:8090 \| xargs kill`, then rerun |
| WebAuthn "invalid domain" on login | passkeys reject IP RP-IDs | open `http://localhost:8090`, not `127.0.0.1` |

The launcher runs the gateway in the foreground and reaps the dKMS daemons/sidecar on exit
(`Ctrl-C`). To stop a backgrounded run: `pkill -f "elastos gateway"; pkill -f dkms-carrier-client`.

---

## The durable fix (beyond dev)

The permanent solution is publishing **arm64 (and other) platform entries in the signed release
manifest**, so install resolves and verifies these providers on every platform with no
`ELASTOS_<NAME>_BIN` override. Until then, this one command is the sanctioned local path.
