# Running Home in the browser on macOS

This is the exact, repeatable procedure to bring up the **Home shell in a browser on
macOS** and sign in with a passkey. It exists because two non-obvious gotchas keep biting
us — the **single host lock** per data dir, and the **WebAuthn relying-party ID** rule.
Follow this and you will not hit "invalid domain" or "another ElastOS host already owns…"
again.

> Scope: this gets the **Home UI + passkey login + desktop** working on macOS today.
> Library / WebSpaces / IPFS-backed features still fail closed on this branch because their
> providers have no `unknown-arm64` build — that is the Apple Virtualization.framework work
> on `sash/local-test-v030` / `docs/vz-backend`, not covered here.

---

## TL;DR

```bash
# 1. Build the gateway binary from the branch that has BOTH macOS fixes
git worktree add /tmp/elastos-mac-home fix/home-summary-resilience
cargo build --manifest-path /tmp/elastos-mac-home/elastos/Cargo.toml -p elastos-server

# 2. Free the single host lock: quit the desktop app AND its managed-home runtime
osascript -e 'quit app "ElastOS"'
pkill -f 'runtime_kind.*managed-home' 2>/dev/null || true   # or kill the pid in host-process.lock

# 3. Launch the browser gateway against your REAL data dir
/tmp/elastos-mac-home/elastos/target/debug/elastos gateway --addr 127.0.0.1:8090
```

Then open **http://localhost:8090/apps/home/** — **not** `127.0.0.1` — and click *Use passkey*.

---

## Why these two gotchas exist

### 1. One host process per data dir (the lock)

The runtime allows exactly **one live host** per data dir. It is enforced by an exclusive
`flock` on `host-process.lock`, regardless of role — so `elastos serve` and `elastos
gateway` **cannot coexist** on the same data dir. See
`elastos/crates/elastos-server/src/host_lock.rs` (there is a test asserting a second lock is
rejected).

On macOS your data dir is:

```
~/Library/Application Support/elastos
```

The **ElastOS.app desktop** spawns a managed-home `serve` against that same dir and holds
the lock. To run the browser gateway you must release it first:

```bash
# See who holds it
cat "$HOME/Library/Application Support/elastos/host-process.lock"
# -> { "pid": <N>, "role": "serve", "addr": "127.0.0.1:<port>" }

# Quit the GUI...
osascript -e 'quit app "ElastOS"'
# ...the managed-home child often survives the GUI; stop it too:
kill <pid-from-lock>      # add -9 only if it ignores SIGTERM
```

A stale lock *file* left behind by a dead process is fine — `flock` is tied to the live
process, so the gateway re-acquires cleanly.

### 2. WebAuthn passkeys reject bare IPs ("This is an invalid domain")

The gateway derives the relying-party ID **dynamically from the page hostname**. Browsers
require a valid registrable domain (or `localhost`) for a relying-party ID and reject bare
IP literals **client-side, before the request reaches the server** — that's why this error
never shows up in the gateway log.

- ✅ `http://localhost:8090/apps/home/`  → RP ID `localhost` → passkeys work
- ❌ `http://127.0.0.1:8090/apps/home/`  → RP ID is an IP → **"This is an invalid domain."**

⚠️ The gateway's startup banner prints `Open: http://127.0.0.1:8090/`. **Ignore that line
for login** and use the `localhost` URL.

A passkey created inside the **ElastOS.app desktop** may be bound to a different origin, so
the browser might not offer it on `localhost`. If none is offered, just create a fresh
passkey on the `localhost` page — it's the same identity vault.

---

## Full procedure

### Prerequisite: the binary must have both macOS fixes

Branch **`fix/home-summary-resilience`** = the home-summary resilience fix (`35845b6`)
stacked on the crosvm Darwin build fix (`5b167f1`), on top of the 0.4.0 base. Both are
required:

| Commit    | Fix | Without it |
| --------- | --- | ---------- |
| `5b167f1` `fix(crosvm)` | Gates Linux-only TAP/ioctl networking behind `target_os="linux"` + a fail-closed non-Linux stub | `elastos-server` won't **build** on macOS |
| `35845b6` `fix(home)`   | Resets corrupt `browser-state.json` to default instead of erroring | Login appears to fail with **`500 … trailing characters`** on `GET /api/apps/home/summary` |

> The `500 … trailing characters` is a separate failure from "invalid domain": it's the
> Electron desktop app writing `browser-state.json` non-atomically while sharing the data
> dir, leaving a valid JSON object followed by trailing bytes. The resilience fix tolerates
> it. (`browser-state` is cosmetic UI state only — no authority — so resetting it is safe.)

Build via a worktree so the dDRM branch stays untouched:

```bash
git worktree add /tmp/elastos-mac-home fix/home-summary-resilience
cargo build --manifest-path /tmp/elastos-mac-home/elastos/Cargo.toml -p elastos-server
# binary: /tmp/elastos-mac-home/elastos/target/debug/elastos
```

### Steps

1. **Release the host lock** (see gotcha #1): quit `ElastOS.app` and `kill` the managed-home
   pid from `host-process.lock`. Confirm ports are free:
   ```bash
   lsof -nP -iTCP:8090 -sTCP:LISTEN || echo "8090 free"
   ```
2. **Launch the gateway** against your real data dir (default on macOS — do **not** override
   `HOME`):
   ```bash
   /tmp/elastos-mac-home/elastos/target/debug/elastos gateway --addr 127.0.0.1:8090
   ```
   Healthy startup loads your existing signing key, CA/TLS, brings the carrier online, and
   loads the shell capsule.
3. **Open** http://localhost:8090/apps/home/ and click **Use passkey**. You should land on
   the desktop.
4. **Sanity probes** (optional):
   ```bash
   curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8090/apps/home/          # 200
   curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8090/api/apps/home/summary # 200
   ```

### Expected (harmless) startup warnings on macOS

These are the providers with no `unknown-arm64` build on this branch. Home + login still
work; the corresponding features fail closed:

```
WARN object-provider binary is not installed; Library object operations will fail closed
WARN Skipping webspace-provider … no platform entry for unknown-arm64
WARN Skipping ipfs-provider … no platform entry for unknown-arm64
WARN content-block-graph-provider binary is not installed; arbitrary DAG repair will fail closed
```

To get those working on macOS you need the Apple Virtualization.framework backend
(`sash/local-test-v030`, see `docs/vz-backend/`), which is out of scope for this runbook.

---

## Playing an owned video (dDRM viewer seam)

The **Owned Video** tile in the launcher plays a real CENC-encrypted clip end-to-end
through the local dDRM rail: the gateway spawns a `ddrm-media-authority` helper (the
"local test KMS" adapter), which CENC-packs the clip, launches a **separate**
`decrypt-provider` boundary, and seals the CEK to that boundary's in-VM session key.
The browser only ever receives already-decrypted segment bytes — the CEK never reaches
the helper, the gateway, or the player.

> ⚠️ **You must build and run the gateway from THIS repo / branch, not the `/tmp`
> worktree above.** The dev-tree capsule path is baked in at **compile time**
> (`DEV_CAPSULES_ROOT = <repo>/capsules`, see `capsule_inventory.rs`). A gateway built
> elsewhere serves a different `capsules/` tree and won't have the tile, the
> `/media/open` route, or the helper.

### Build the three binaries (from the repo root)

```bash
# 1. decrypt boundary — MUST include the rail features, or it builds as the
#    canonical fail-closed stub and refuses to play.
cargo build --manifest-path capsules/decrypt-provider/Cargo.toml \
  --features rail-stream,rail-mint

# 2. local key-authority helper (isolated PQ-crypto workspace)
cargo build --manifest-path scripts/dev/ddrm-media-authority/Cargo.toml

# 3. rights-provider capsule WITH the chain-rights dev profile (the live-chain gate)
cargo build --manifest-path capsules/rights-provider/Cargo.toml --features chain-rights

# 4. chain-provider capsule (the REAL on-chain ownership read; needed for chain modes)
cargo build --manifest-path capsules/chain-provider/Cargo.toml

# 5. the gateway itself
cargo build --manifest-path elastos/Cargo.toml -p elastos-server
```

`ffmpeg` + `ffprobe` must be on `PATH` (the helper synthesizes the test clip). On
macOS: `brew install ffmpeg`.

### Run + play

```bash
# release the host lock (see gotcha #1), then:
./elastos/target/debug/elastos gateway --addr 127.0.0.1:8090
```

Open **http://localhost:8090/apps/home/**, sign in, and click the **Owned Video** tile.
A player window opens and the clip plays.

> ✅ **`feat/ddrm-home-playback` now carries the home-summary resilience fix too.** It
> was ported onto this branch (mirrors `35845b6`), so the gateway you build from THIS
> repo for dDRM work also tolerates a corrupt `browser-state.json` — you no longer need
> the `/tmp` worktree just to avoid the `500 … trailing characters` login failure.

### Live-chain rights gate (Library opens)

Opening an owned object from the **Library** (`POST /api/viewers/open`) now runs a
live-chain authorization gate **before** anything is sealed or decrypted, exactly as
Anders specified — the DECISION lives in the `rights-provider` capsule, not the gateway:

1. The gateway resolves + reads the object inside the principal's own root (ownership
   gate), then builds a typed on-chain ownership attestation (`ChainAccessAttestationV1`).
   On this host that attestation is a **dev local-attestation** (stands in for a
   `chain-provider.has_access_by_content_id` read against the Base contract).
2. It spawns the real `rights-provider` (built with `--features chain-rights`) and asks it
   to `decide_access_from_chain`. The capsule binds the attestation to the request and
   mints a signed `RightsDecisionReceiptV1`.
3. **Deny ⇒ `403` and nothing is sealed.** Allow ⇒ the receipt's hash is welded into the
   decrypt-transcript AAD (`ddrm-envelope`), so the seal is cryptographically bound to
   THIS rights decision — a seal made under one decision cannot be replayed under another
   (the AEAD open fails closed at the decrypt boundary).

The on-chain ownership answer comes from one of three sources, selected by
`ELASTOS_DDRM_RIGHTS` (the gateway itself NEVER does chain RPC — the real
`chain-provider` capsule does):

| `ELASTOS_DDRM_RIGHTS` | Ownership source | Use |
| --------------------- | ---------------- | --- |
| `dev` (default) | Local attestation: owned unless the CID is in `ELASTOS_DDRM_DENY_CIDS` | Offline work, no chain |
| `chain-mock` | REAL `chain-provider` `eth_call` against an in-process JSON-RPC mock (no network) | Prove owned→opens / not-owned→fail-closed locally on a Mac |
| `chain` | REAL `chain-provider` `eth_call` against the configured Base RPC + contract | Production: actual access-token ownership |

- **dev fail-closed:** list a CID in `ELASTOS_DDRM_DENY_CIDS` → that open returns `403`.
- **chain-mock:** set `ELASTOS_DDRM_RIGHTS=chain-mock`; `ELASTOS_DDRM_CHAIN_ACCESS=denied`
  flips the mock to not-owned so the open fails closed — the calldata is still really
  ABI-encoded, sent, and decoded through the real `chain-provider`. `…=owned` forces owned;
  `…=ledger` answers from the local owned-token ledger (see the buy flow below).
- **chain:** set `ELASTOS_DDRM_RIGHTS=chain` plus `ELASTOS_CHAIN_BASE_RPC`,
  `ELASTOS_DDRM_RIGHTS_CONTRACT`, and `ELASTOS_DDRM_RIGHTS_SELECTOR`. The `subject` is the
  signed-in principal's linked EVM wallet (or `ELASTOS_DDRM_SUBJECT` override); chain mode
  with no linked wallet fails closed (`403 link an EVM wallet…`).

Verify the chain path end to end (real chain-provider + mock + real rights-provider):

```bash
cargo test -p elastos-server rights_authority::tests -- --include-ignored
```

### Buy flow — put an access token in the wallet (`POST /api/market/buy`)

A Library open that is **denied** (no access token yet) is recoverable: the buy flow
assembles a `buyAccess` transaction, signs + broadcasts it, and the rights gate then
reads the new ownership. The Home shell does this automatically — a rights-denied open
calls `/api/market/buy { uri }` and retries the open once, so a click goes
**denied → buy → owned → plays**. The gateway invents NO contract semantics: the
`buyAccess` selector + value are operator-pinned config (like the `has_access`/`mint`
selectors), never a guessed signature.

Three modes (shared with `ELASTOS_DDRM_RIGHTS`):

- **dev** — records the purchase in the local owned-token ledger and returns a synthetic
  tx hash. Offline; no chain, no signing.
- **chain-mock** — assembles the calldata and broadcasts a representative signed tx
  through the REAL `chain-provider.broadcast_transaction` op against an in-process RPC
  mock (the production broadcast path runs), then records the purchase in the ledger.
  Pair with the rights gate's `ELASTOS_DDRM_CHAIN_ACCESS=ledger` so a fresh object is
  **not owned → 403**, and **owned → opens** right after the buy. Fully offline.
- **chain** — assembles the `{ to, value, data }` against the configured Base contract.
  With **`ELASTOS_DDRM_BUY_SIGN=wallet`** (recommended) the gateway sources real nonce/gas
  via `chain-provider.prepare_transaction`, signs inside **`wallet-provider`** with a
  managed secp256k1 account (the key never leaves the capsule), and broadcasts the signed
  bytes through the real chain-provider — genuinely live, no external signer. Absent that
  opt-in it broadcasts an **externally-signed** tx (`ELASTOS_DDRM_BUY_SIGNED_TX`), or —
  absent one — returns the assembled unsigned tx (HTTP `409`). Ownership is then read back
  from `hasAccessByContentId`, never the ledger.

> **Runtime signing (`ELASTOS_DDRM_BUY_SIGN=wallet`)** also works in **chain-mock**: the
> wallet capsule signs a well-formed buyAccess tx and the *genuine* signed bytes are
> broadcast through the in-process RPC mock — proving the whole `prepare → sign → broadcast`
> rail offline, with the key contained in `wallet-provider`. The managed account is the
> authoritative buyer, so ownership is recorded under its address.

Prove the whole offline loop (denied → buy → allowed) against the real capsules:

```bash
cargo build --manifest-path capsules/chain-provider/Cargo.toml
cargo build --manifest-path capsules/rights-provider/Cargo.toml --features chain-rights
cargo test -p elastos-server buy_then_open_loop -- --ignored
```

Prove the **real signing rail** offline (wallet capsule signs, chain-provider broadcasts):

```bash
cargo build --manifest-path capsules/wallet-provider/Cargo.toml
cargo build --manifest-path capsules/chain-provider/Cargo.toml
cargo test -p elastos-server chain_mock_wallet_signs -- --ignored
```

### Overrides (optional)

| Env var | Purpose |
| ------- | ------- |
| `ELASTOS_DDRM_DECRYPT_BIN` | Path to the `rail-stream,rail-mint` decrypt-provider binary |
| `ELASTOS_DDRM_MEDIA_AUTHORITY_BIN` | Path to the `ddrm-media-authority` helper |
| `ELASTOS_DDRM_SAMPLE_VIDEO` | Use your own source clip instead of the synthesized one |
| `ELASTOS_RIGHTS_PROVIDER_BIN` | Path to the `chain-rights` rights-provider binary (the live-chain gate) |
| `ELASTOS_DDRM_DENY_CIDS` | Comma-separated content CIDs the dev attestation should DENY (fail-closed testing) |
| `ELASTOS_DDRM_RIGHTS` | Ownership source: `dev` (default), `chain-mock`, or `chain` |
| `ELASTOS_CHAIN_PROVIDER_BIN` | Path to the chain-provider binary (chain modes) |
| `ELASTOS_CHAIN_BASE_RPC` | Base RPC URL (`chain` mode) |
| `ELASTOS_DDRM_RIGHTS_CONTRACT` | Rights/AuthorityGateway contract address (`chain` mode) |
| `ELASTOS_DDRM_RIGHTS_SELECTOR` | `hasAccessByContentId` 4-byte selector, e.g. `0x........` (`chain` mode) |
| `ELASTOS_DDRM_RIGHTS_NETWORK` / `ELASTOS_DDRM_CHAIN_ID` | Network id (default `base`) / chain id (default `8453`) |
| `ELASTOS_DDRM_SUBJECT` | Pin the on-chain wallet `subject` (else the principal's linked EVM account) |
| `ELASTOS_DDRM_CHAIN_ACCESS` | `chain-mock` only: `denied` (not-owned) / `owned` / `ledger` (owned-token ledger) |
| `ELASTOS_DDRM_OWNED_LEDGER` | Path to the local owned-token ledger (buy flow); default `<temp>/elastos-ddrm-owned-tokens.json` |
| `ELASTOS_DDRM_BUY_SELECTOR` / `ELASTOS_DDRM_BUY_TO` / `ELASTOS_DDRM_BUY_VALUE` | Operator-pinned `buyAccess` selector / contract / payable value |
| `ELASTOS_DDRM_BUY_SIGN` | `wallet` → sign the buy inside `wallet-provider` with a managed account (key never leaves the capsule); else use `ELASTOS_DDRM_BUY_SIGNED_TX` |
| `ELASTOS_DDRM_BUY_SIGNED_TX` | `chain` mode (no runtime signing): an externally-signed buy tx to broadcast |
| `ELASTOS_WALLET_PROVIDER_BIN` | Path to the `wallet-provider` binary (runtime signing) |
| `ELASTOS_DDRM_WALLET_BASE` | Where `wallet-provider` keeps its managed-key store; default `$HOME/.elastos-ddrm-wallet` |

### dDRM troubleshooting

| Symptom | Cause | Fix |
| ------- | ----- | --- |
| `could not open owned media: … decrypt-provider did not configure` | decrypt-provider built as the fail-closed stub (no rail features) | Rebuild with `--features rail-stream,rail-mint` |
| `rights provider unavailable; cannot authorize open` (503) | `rights-provider` not built with `chain-rights`, or wrong path | Build step 3 above, or set `ELASTOS_RIGHTS_PROVIDER_BIN` |
| `no valid access token for this content (rights provider denied)` (403) | No access token for this `(content_id, subject)` — the dev deny list, the chain-mock ledger, or the real chain says no | Expected fail-closed behaviour; the Home shell auto-buys + retries, or call `POST /api/market/buy { uri }` |
| `link an EVM wallet to buy access` (403) | Buy attempted in a chain mode with no linked wallet | Link an EVM account, or set `ELASTOS_DDRM_SUBJECT` |
| `live buy needs a signature …` (409) | `chain` mode with no runtime signing and no `ELASTOS_DDRM_BUY_SIGNED_TX` | Opt into runtime signing with `ELASTOS_DDRM_BUY_SIGN=wallet`, or sign the returned `unsigned_tx` externally and resubmit via `ELASTOS_DDRM_BUY_SIGNED_TX` |
| `wallet-provider not found at …` | runtime signing opted in but the wallet capsule isn't built | `cargo build --manifest-path capsules/wallet-provider/Cargo.toml`, or set `ELASTOS_WALLET_PROVIDER_BIN` |
| `media-authority helper not found at …` | helper not built, or running a gateway from a different tree | Build step 2 above, run gateway from this repo, or set `ELASTOS_DDRM_MEDIA_AUTHORITY_BIN` |
| `could not open owned media: … ffmpeg` | `ffmpeg`/`ffprobe` not on `PATH` | `brew install ffmpeg` |
| Tile missing from launcher | gateway built/run from a different `capsules/` tree | Build + run the gateway from this repo on `feat/ddrm-home-playback` |

---

## Restarting the node cleanly (and recovering from the passkey 500)

Use this whenever you rebuild the gateway or hit a stale process. It is the exact loop
that avoids the two recurring failures: a held host lock, and the `500 … trailing
characters` login error.

```bash
# 1. Free port / host lock — kill any gateway already on 8090
lsof -ti tcp:8090 | xargs kill 2>/dev/null; sleep 1

# 2. (Re)build the gateway from THIS repo/branch (feat/ddrm-home-playback)
cargo build --manifest-path elastos/Cargo.toml -p elastos-server

# 3. Relaunch against the real data dir
./elastos/target/debug/elastos gateway --addr 127.0.0.1:8090 > /tmp/elastos-gateway.log 2>&1 &

# 4. Probe — both must be 200
curl -s -o /dev/null -w "home:    %{http_code}\n" http://localhost:8090/apps/home/
curl -s -o /dev/null -w "summary: %{http_code}\n" http://localhost:8090/api/apps/home/summary
```

### The passkey "500 … trailing characters" — what it is and how to clear it

Your passkey login *succeeds*; the failure is the **next** call,
`GET /api/apps/home/summary`, which Home renders onto the sign-in card so it looks like
login failed. The cause is `browser-state.json` (cosmetic UI state — recent targets +
window layout, **no authority**) left as *valid JSON followed by trailing bytes* by the
**Electron desktop app rewriting it non-atomically** while sharing this data dir.

There are two layers of defense — you want both:

1. **The durable fix (in the binary).** Build/run a gateway that carries the resilience
   fix — now on **both** `fix/home-summary-resilience` *and* `feat/ddrm-home-playback`.
   On a corrupt/mismatched `browser-state.json` it logs a warning and resets to default
   instead of 500-ing, so login can never be blocked by this file again. Our own writer
   uses `atomic_write`, so the runtime never creates the corruption — only the desktop
   app does.

2. **Manual reset (if you're on an older binary, or want to force-clear).** The file
   regenerates on next write, so deleting it is safe:

   ```bash
   # find + remove any corrupt browser-state.json under the data dir
   find "$HOME/Library/Application Support/elastos" -name browser-state.json -print -delete
   ```

> Rule of thumb: if sign-in shows `500 … trailing characters`, you are either running a
> gateway *without* the resilience fix, or the desktop app is actively fighting the
> gateway for the data dir. Run the fixed binary (step 2 of the restart loop) and quit
> `ElastOS.app` so only one host owns the data dir.

---

## Troubleshooting

| Symptom | Cause | Fix |
| ------- | ----- | --- |
| `This is an invalid domain.` on *Use passkey* | Page opened on `127.0.0.1` (bare IP) | Open `http://localhost:8090/apps/home/` instead |
| `request failed: 500 … trailing characters` after login | Corrupt `browser-state.json` (Electron app rewrites it non-atomically) | Run a binary with the resilience fix (`feat/ddrm-home-playback` or `fix/home-summary-resilience`); or `find "$HOME/Library/Application Support/elastos" -name browser-state.json -delete`. See *Restarting the node cleanly* above |
| `another ElastOS host already owns … host-process.lock` | Desktop app / another `serve`/`gateway` holds the lock | Quit `ElastOS.app`, `kill` the pid in `host-process.lock`, retry |
| `elastos-server` won't compile on macOS | Missing the crosvm Darwin gate | Build from `fix/home-summary-resilience` (includes `5b167f1`) |
| Passkey not offered on `localhost` | Existing passkey bound to the desktop app's origin | Create a fresh passkey on the `localhost` page (same identity vault) |

## Cleanup

```bash
# Stop the gateway (Ctrl-C if foreground, or kill the pid)
# Remove the build worktree when done:
git worktree remove /tmp/elastos-mac-home
```
