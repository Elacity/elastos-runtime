# Troubleshooting

Practical fixes for problems hitting a **local source checkout** (especially **macOS / darwin-arm64**)
when bringing up the Create-portal gateway, the dKMS quorum, and the VSCode debug launch.

Each entry is **Symptom → Cause → Fix**. Commands assume you run from the repo root
(`elastos-runtime/`). Set `export EMSDK_QUIET=1` first to silence the emsdk banner on every shell.

> **Start here for the happy path.** This file is for when something breaks. For the intended dev
> flow read these first:
> - [`docs/DEV_BOOTSTRAP.md`](docs/DEV_BOOTSTRAP.md) — the one-command dev gateway + the dKMS-mode env matrix.
> - [`docs/RUN_HOME_MACOS.md`](docs/RUN_HOME_MACOS.md) and [`docs/MAC.md`](docs/MAC.md) — the macOS run guides (source-home install, Browser VM).
> - [`docs/GETTING_STARTED.md`](docs/GETTING_STARTED.md) — first-time orientation.
> - [`README.md`](README.md) — repo overview & layout. (The wider workspace lives one level up; this
>   `elastos-runtime` repo is one of several independent git repos, each with its own tooling.)

---

## Prerequisites (macOS / darwin-arm64)

Install these before running anything below. The dev scripts probe for them and fail-closed if missing.

| Tool | Why | Install |
|---|---|---|
| **Rust** (via rustup) + `wasm32-wasip1` + `aarch64-unknown-linux-musl` targets | build providers, wasm app capsules, and the Browser VM guest (a Linux microVM) | `rustup target add wasm32-wasip1 aarch64-unknown-linux-musl` |
| **Node** (≥ 20) | Home shell tooling / smokes | nvm or `brew install node` |
| **Docker** | the local 3-node dKMS quorum (`scripts/dev/dkms-docker`) | Docker Desktop |
| **ffmpeg / ffprobe** | media mint (DASH packaging) | `brew install ffmpeg` |
| **kubo / ipfs** | `elastos://content` publish/fetch (installed automatically by `setup-source-home.sh`) | `brew install ipfs` (or auto) |
| **coturn** (optional) | Browser VM engine TURN relay (in-app native web rendering only) | `brew install coturn` |
| **e2fsprogs** (optional, `debugfs`) | Browser VM rootfs inspection (`mac-source-home-restart.sh` only) | `brew install e2fsprogs` |

`rust-lld` (bundled with the Rust toolchain) is used as the cross-linker for the musl guest — **no system
musl toolchain is required** on macOS.

---

## How the pieces fit (mental model)

**Three ways to run the gateway — use the right one:**

| Command | What it does | Providers | App capsules (browser/home) | Use when |
|---|---|---|---|---|
| `elastos gateway` (bare binary) | starts the server only | trusts **installed, signed** components (none on macOS → all skipped) | resolves from the **installed catalog** only | almost never on a source checkout |
| `scripts/dev/run-creator-gateway.sh` | builds providers, provisions dKMS, exports `ELASTOS_<NAME>_BIN` dev overrides, runs the gateway | **local builds** trusted via env overrides | resolves from the installed catalog (must be installed first) | day-to-day dev (this is what the **VSCode task** runs) |
| `scripts/setup-source-home.sh` | **builds + installs** providers, app capsules, and `components.json` into the data root (once) | **installs** them so even the bare binary verifies | **installs** browser/home/marketplace/… | first-time setup / after a fresh data root |

**Runtime resolution rule (important):** the gateway serves app capsules **only** from the *installed catalog*
(`<data_dir>/capsules/` + `components.json`), **never** from the repo `capsules/` dir. The repo dir
(`DEV_CAPSULES_ROOT`) is used only by `setup`/listing. → so "Browser capsule not found" always means the
catalog is empty/unpopulated (§6), not a code bug.

**Key paths (macOS):**
- Gateway data root: `~/Library/Application Support/elastos/` (from `dirs::data_dir()/elastos`; the launch
  script's `--data-dir` does **not** override the gateway binary's data root).
- Installed app-capsule catalog: `<data_dir>/capsules/`  ·  provider bins: `<data_dir>/bin/`  ·  manifest: `<data_dir>/components.json`
- Auth state (audit chain): `<data_dir>/ElastOS/System/Auth/auth-state.json` (the `elastos/ElastOS` nesting on
  case-insensitive macOS is expected, not a bug — app dir `elastos` + VFS root `ElastOS`).
- dKMS docker public artifacts (descriptor + caller seed the gateway consumes): `scripts/dev/dkms-docker/shared/`

**Key env vars** (the VSCode task sets these — see `.vscode/tasks.json`):
- `ELASTOS_DDRM_RIGHTS` = `dev` | `chain-mock` | `chain` — the rights gate mode.
- `ELASTOS_DKMS_REMOTE=1` + `ELASTOS_DKMS_CARRIER=1` — seal/recover against the LIVE geo nodes over Carrier (vs a local quorum).
- `ELASTOS_DKMS_REMOTE_DESCRIPTOR` / `ELASTOS_DKMS_REMOTE_CALLER_SEED` — point at `scripts/dev/dkms-docker/shared/*`.
- `ELASTOS_<NAME>_BIN` — dev override pointing at a locally-built provider binary (bypasses the signed-manifest check).

**Startup guards you may trip on an old data root** (all deliberate, all fail-closed): the audit-chain guard
(§3), the principal-root object-size guard (§4), and the caller-policy guard (§1). See
[`docs/AUTH_AUDIT_CHAIN.md`](docs/AUTH_AUDIT_CHAIN.md) for the auth/audit model.

---

## Getting a working dev gateway on macOS — the short path

If you just want the whole thing running, do these once, in order:

```bash
# 0. one-time Rust cross-target for the Browser VM guest helpers (Linux microVM)
rustup target add aarch64-unknown-linux-musl

# 1. install the source-home app surface + providers into your data root
#    (populates ~/Library/Application Support/elastos/{capsules,bin,components.json})
./scripts/setup-source-home.sh

# 2. (only if your data root predates the audit-chain / principal-root hardening — see §3, §4)
#    migrate the audit chain, and move any >16 MiB library object out of the root

# 3. bring up the dKMS quorum (3 local docker nodes) — needed for the rights/open path
cd scripts/dev/dkms-docker && ./up.sh up && cd -

# 4. launch the gateway (this is what the VSCode "bring up gateway (debug)" task runs)
ELASTOS_DKMS_CARRIER=1 ELASTOS_DKMS_REMOTE=1 \
  ELASTOS_DKMS_REMOTE_DESCRIPTOR="$PWD/scripts/dev/dkms-docker/shared/dkms-authority.carrier.json" \
  ELASTOS_DKMS_REMOTE_CALLER_SEED="$PWD/scripts/dev/dkms-docker/shared/caller.seed" \
  ELASTOS_DDRM_RIGHTS=chain \
  ./scripts/dev/run-creator-gateway.sh --addr 127.0.0.1:8090
# then open http://localhost:8090/apps/home/   (localhost, NOT 127.0.0.1 — WebAuthn rejects bare IPs)
```

The sections below explain each failure you may hit along the way.

---

## 1. dKMS docker nodes crash-loop: "both are set — refusing to start"

**Symptom** — `./scripts/dev/dkms-docker/up.sh up` starts the 3 nodes, then they exit(1) and restart forever:

```
dkms-authority: cannot start — $DKMS_AUTHORITY_ALLOWED_CALLERS and $DKMS_AUTHORITY_ALLOW_ANONYMOUS=1
are both set — refusing to start (choose an allow-list OR explicit anonymous, not both)
```

**Cause** — This is the DKMS-8 fail-closed caller-policy guard working correctly. `up.sh` allow-lists the
runtime caller (`DKMS_AUTHORITY_ALLOWED_CALLERS` via `.env`), while the container entrypoint *also* set
`DKMS_AUTHORITY_ALLOW_ANONYMOUS=1` — a contradictory policy. The dev tooling wasn't reconciled with the
mutual-exclusion the hardening enforces.

**Fix** — Fixed in `scripts/dev/dkms-docker/entrypoint.sh`: the mesh now runs **allow-list only**
(production-like) and never sets `ALLOW_ANONYMOUS`. The entrypoint is **baked into the image**
(`COPY` in the Dockerfile), so you must **rebuild**:

```bash
cd scripts/dev/dkms-docker
./up.sh up          # `up.sh up` runs `docker compose build`, so it rebuilds with the fixed entrypoint
# (a bare `docker compose up` keeps the old baked-in entrypoint and will still crash-loop)
```

---

## 2. dKMS docker: descriptor assembly fails with "node 0 ... missing public identity"

**Symptom** — `up.sh up` reaches "assemble the CARRIER v2 descriptor" and aborts:

```
AssertionError: node 0 provision output missing public identity
```

**Cause** — DKMS-7 moved identity creation to the offline `provision` subcommand, which prints the seal keys
as a **flat** top-level JSON object. The descriptor-assembly step still read them nested under a `data`
envelope (the old wire-`init` response shape).

**Fix** — Fixed in `scripts/dev/dkms-docker/up.sh` (reads `.seal_verifying_key_b64` at the top level, accepting
either shape). If you see this on an old checkout, `git pull`/rebuild. The provisioning output contract is
documented in `docs/dkms/deploy/RUNBOOK.md` §6.

---

## 3. Gateway won't start: "unchained auth state is unsupported"

**Symptom** — the gateway prints its banner then exits:

```
Error: unchained auth state is unsupported; preserve and back up the existing data root,
then use a fresh data root; no automatic migration or offline migration script is provided
```

**Cause** — A pre-existing **audit-chain hardening** guard (commit `fix(auth): harden audit and launch
authority`). Your data root has auth state (principals/sessions) created **before** the audit-chain feature,
so it has no tamper-evident audit chain. A *lossless* migration is impossible by design (retroactively signing
the old audit log would forge tamper-evidence).

**Fix** — Use the offline migration (preserves identities/passkeys, discards the un-chained audit log, starts a
fresh chain, backs up first). **Run with the gateway stopped:**

```bash
./elastos/target/release/elastos migrate-audit-chain --dry-run   # preview: shows kept vs discarded counts
./elastos/target/release/elastos migrate-audit-chain             # do it (writes auth-state.json.pre-audit-migrate-<ts>.bak)
```

The migration keeps `principals` / `sessions` / `principal_root_protections` and only resets the audit log.
The backup file lets you restore the pre-migration state for inspection.

→ **Docs:** [`docs/AUTH_AUDIT_CHAIN.md`](docs/AUTH_AUDIT_CHAIN.md) (the audit-chain model & why the guard fails closed); `elastos migrate-audit-chain --help`.

---

## 4. Gateway won't start: "declared principal-root object exceeds 16 MiB"

**Symptom** — after clearing §3, the gateway exits with:

```
Error: declared principal-root object exceeds 16 MiB
```

**Cause** — A pre-existing **plaintext-root migration** readiness guard caps declared principal-root objects at
16 MiB per file (64 MiB total). Your library root has a file larger than 16 MiB.

**Fix** — Find and move the oversize object out of the data root (recoverably):

```bash
ROOT="$HOME/Library/Application Support/elastos"
find "$ROOT" -type f -size +16M -exec ls -lh {} \;      # locate the offender(s)
# move each out to a backup, preserving the relative path, e.g.:
#   mv "$ROOT/Users/<principal>/Pictures/<big>.ddrm"  ~/elastos-oversize-backup/...
```

Restore later if/when the object size cap is raised. (Objects declared as NotFound are skipped by the check.)

---

## 5. Gateway runs but every provider is skipped: "no platform entry for darwin-arm64"

**Symptom** — running `elastos gateway` directly:

```
WARN elastos::server_infra: Skipping chain-provider due to verification failure:
  cannot verify installed component 'chain-provider' at .../bin/chain-provider: no platform entry for darwin-arm64
WARN ... encrypt-provider binary is not installed; the Create portal mint path will fail closed
... (every provider skipped)
```

**Cause** — The **bare binary** only trusts **installed, signed** components. macOS has no published signed
manifest, so all providers fail verification. The dev path builds providers locally and either installs them
(via a source-home `components.json` manifest) or points the gateway at them with `ELASTOS_<NAME>_BIN` overrides.

**Fix** — Don't run the bare binary. Either:
- Run **`./scripts/dev/run-creator-gateway.sh`** (it builds providers and exports the `ELASTOS_<NAME>_BIN`
  dev overrides so the gateway trusts local builds — this is what the VSCode task uses), **or**
- Run **`./scripts/setup-source-home.sh`** once (see §6) to install the providers + `components.json` into the
  data root so even the bare binary can verify them.

---

## 6. Home page shows "Browser capsule not found" (empty app catalog)

**Symptom** — `http://localhost:8090/apps/home/` loads the nav chrome but the content is:

```
Browser capsule not found
```

**Cause** — At runtime the gateway serves app capsules only from the **installed catalog**
(`~/Library/Application Support/elastos/capsules/` + a components manifest), never from the repo `capsules/`
dir. On a fresh/old data root that catalog is empty, so `browser`/`home` don't resolve. `run-creator-gateway.sh`
builds *wasm-guest* capsules but does **not** install the static app-shell capsules.

**Fix** — Run the source-home installer. It builds and installs the full app surface into the data root:

```bash
rustup target add aarch64-unknown-linux-musl   # prerequisite — see §7
./scripts/setup-source-home.sh                 # installs browser, home, marketplace, wallet, library, ... + components.json
```

After it completes, `data_dir/capsules/` contains `browser`, `home`, `home-cli`, `home-gui`, `marketplace`,
`wallet`, `library`, `chat-room`, etc., and the home page resolves the browser capsule (HTTP 200).

> `elastos setup --profile demo` does **not** work on a source checkout — it fetches signed artifacts from a
> trusted source over Carrier (`No trusted source configured`). `setup-source-home.sh` is the source-checkout
> equivalent that builds+installs locally.

→ **Docs:** [`docs/BROWSER_CAPSULE.md`](docs/BROWSER_CAPSULE.md), [`docs/CAPSULE_MODEL.md`](docs/CAPSULE_MODEL.md) (capsule model & resolution), [`docs/DEV_BOOTSTRAP.md`](docs/DEV_BOOTSTRAP.md) (dev run flow).

---

## 7. `setup-source-home.sh` fails: "can't find crate for `core` ... aarch64-unknown-linux-musl"

**Symptom** —

```
[setup-source-home] build Browser VM guest relay helpers
error[E0463]: can't find crate for `core`
  = note: the `aarch64-unknown-linux-musl` target may not be installed
  = help: consider downloading the target with `rustup target add aarch64-unknown-linux-musl`
```

**Cause** — The Browser VM guest relay helpers cross-compile for a **Linux microVM** (`aarch64-unknown-linux-musl`),
even on macOS. That Rust target's std wasn't installed. The script already uses `rust-lld` as the cross-linker
on Darwin, so no system musl toolchain is needed — only the target's std.

**Fix** —

```bash
rustup target add aarch64-unknown-linux-musl
./scripts/setup-source-home.sh   # re-run
```

---

## 8. `setup-source-home.sh` exits 1 on the Browser VM engine (turnserver / VM rootfs)

**Symptom** — near the end:

```
{"schema":"elastos.setup-source-home.browser-artifacts/v1","ok":true,..., "missing":3}
turnserver was not found; install coturn or set ELASTOS_BROWSER_VM_TURN_PROGRAM ...
SETUP_HOME_EXIT: 1
```

**Cause** — This is **only** the optional **Browser VM engine** (native in-page web rendering inside a Linux
microVM): `turnserver`/coturn isn't installed and the VM rootfs artifacts are missing. The **core install
already succeeded** (app capsules, providers, `components.json`) — Home runs fine without it.

**Fix** — Ignore it unless you need the in-app Browser to render real web pages. For that:

```bash
brew install coturn
# then run the browser-rootfs setup (see scripts/setup-source-home-browser-artifacts.sh)
```

---

## 9. VSCode: "The command for input 'gatewayPid' didn't output any results"

**Symptom** — launching the **"ElastOS: attach to running gateway"** debug config pops a modal
that blocks the launch.

**Cause** — The `gatewayPid` input runs `pgrep -f 'target/debug/elastos gateway'`. When no gateway is running
(it crashed on a guard, or you launched attach before the gateway bound), `pgrep` returns empty and the
`tasks-shell-input` extension treats empty output as "no results" and hard-fails.

**Fix** — Two parts:
- The **root cause** is almost always that the gateway isn't running — fix that first (§1–§8). Once a gateway
  is up, `gatewayPid` resolves and attach works (debug build → lldb has symbols).
- The input command in `.vscode/launch.json` was made robust: it **polls ~15s** for the gateway PID (handles the
  race where the gateway is still binding) and emits a `0` fallback if none is found — so the input never triggers
  the blocking modal. When there's truly no gateway, the *attach* then fails cleanly in the Debug Console
  (`no process with PID 0`) instead of a modal that aborts the launch.

**VSCode launch flow, once the environment is set up (§5–§8):**
1. Run the task **"elastos: bring up gateway (debug)"** → gateway comes up on `:8090`.
2. Open **http://localhost:8090/apps/home/** (`localhost`, not `127.0.0.1`).
3. Run **"ElastOS: attach to running gateway"** → `gatewayPid` resolves → lldb attaches.

---

## 10. `--locked` build fails in a capsule: "lock file needs to be updated"

**Symptom** —

```
error: the lock file .../capsules/encrypt-provider/Cargo.lock needs to be updated but --locked was passed
```

**Cause** — A capsule's checked-in `Cargo.lock` pinned a stale workspace crate version
(`elastos-common 0.5.0` while the workspace is `0.6.0`).

**Fix** — Regenerate/commit the lockfile (`cargo update -p elastos-common --manifest-path
capsules/encrypt-provider/Cargo.toml`, or just `cargo build` without `--locked`). This specific one is already
synced in-tree.

---

## The Create → mint → open flow (dDRM / dKMS end-to-end)

This branch (`feat/dkms-esp-port`) is a **deliberate breaking change** from the legacy runtime to the
**ESP (Elastos Shell Protocol)** model. Several Create-portal and asset-open failures traced to **one
recurring pattern**: a component still speaking the *legacy* contract that ESP changed underneath it.
§11–§16 document each, in the order you hit them going mint → open. When debugging this flow, turn on
the per-stage logging described in [§17](#17-debugging-the-createopen-flow--logs--instrumentation) — every
section below was diagnosed from those log lines, not guesswork.

> **The single most important invariant:** a dKMS asset can only be opened by the **exact quorum
> node-set it was sealed to at mint time**. Docker `up.sh destroy` (or any volume recreation) rolls the
> node identities and **orphans every previously-minted asset**. To test the open path, **mint a fresh
> asset against the currently-running node-set** — see §11.

---

## 11. Opening a protected asset fails: "could not open … from the dKMS quorum" / all nodes reject `bad_node_set`

**Symptom** — double-clicking an owned `.ddrm` shows a lock + a 502 after ~9s. The docker node logs show
the recover reaching all three nodes and each rejecting it:

```
-> recover error code=access_denied (access grant rejected: bad_node_set)
2-of-3 quorum NOT met — only 0 of 3 secret-holders served [node A: … bad_node_set; node B: … bad_node_set; …]
```

**Cause** — **not** a transport, rights, or grant failure (rights pass, the grant assembles, the handshake
succeeds). The asset's CEK shares were sealed to a **different quorum node-set** than the one currently
running. Each node enforces a cross-quorum replay defense: the grant's declared node-set id must equal the
node's own pinned `DKMS_AUTHORITY_NODE_SET_ID_B64`. A prior `up.sh destroy` (or a fresh clone) rolled the
docker node master seeds, so older assets are sealed to a now-dead node-set.

**Fix / how to confirm** — This is expected for orphaned assets; there is no way to recover shares from a
node-set whose seeds are gone. To verify the open rail itself is healthy, **mint a fresh asset against the
running node-set and open that** — it recovers cleanly (`recover ok` ×3). The gateway now proves this
host-side *before* spawning a doomed recover: `open_quorum` in
[`viewer_open.rs`](elastos/crates/elastos-server/src/api/viewer_open.rs) derives both node-set ids and logs a
verdict —

```
dKMS node-set precheck: MATCH   node_set=…  — asset is recoverable by the running quorum
dKMS node-set precheck: MISMATCH — asset sealed to node_set=X but the running quorum is node_set=Y …
dKMS node-set evidence: asset node-vk fps=[…] vs running quorum node-vk fps=[…]   (DISJOINT ⇒ dead quorum)
```

- To keep a stable node-set across restarts, bring the mesh down with `up.sh down` (preserves the named
  volumes / seeds), **not** `up.sh destroy` (wipes them).

---

## 12. Create portal: wallet & channel pickers stay empty ("No launch capability")

**Symptom** — the Create portal renders, but the **Wallet** dropdown never fills (stuck on
"Connect a wallet on Base…") and **Channel** stays "Select a wallet first…". A breakpoint in the frame's
`loadWallets()` never hits, and the gateway shows **zero** `/api/apps/creator/wallet` (or `/status`)
requests.

**Cause** — `loadWallets()` only runs if `preflight()` returns true, and `preflight()` bails at its very
first line — `if (!homeToken) …` — because `homeToken` is `null`. The ESP shell delivers the launch
capability in the URL **fragment** (`…#home_token=…`, so it never reaches a server / access log), but
`creator.js` read it from the **query string** (`?home_token=`). It was the *only* capsule of ~19 still
reading the query; every other one (`elacity-player`, `ddrm-viewer`, `browser`, …) reads the fragment.

**Fix** — [`capsules/creator/browser/creator.js`](capsules/creator/browser/creator.js) now reads the
fragment, matching every other capsule:

```js
function fragmentValue(name) {
  return new URLSearchParams(window.location.hash.replace(/^#/, "")).get(name);
}
const homeToken = fragmentValue("home_token");   // was: query("home_token") → searchParams
```

Browser capsule assets are served `no-store` from the **installed** copy
(`<data_dir>/capsules/creator/browser/…`), so after editing the repo file, sync it to the installed path
and bump the `?v=` cache-buster in `index.html` (a hard refresh alone is enough since `no-store` disables
caching — but the version bump is belt-and-suspenders).

---

## 13. Create portal / mint fails: `wallet: unknown variant 'accounts' (or 'request_signature')`

**Symptom** — with the token fixed (§12), `/api/apps/creator/wallet` is now called but 502s, and the mint's
"Sign with wallet" step errors, both with a wallet-provider deserialization error:

```
wallet: unknown variant `accounts`, expected one of `init`, `status`, `wallet_contract`, `shutdown`
```

**Cause** — The wallet provider was migrated to a **v2 (`WalletContract`) protocol**: real operations
(`ListAccounts`, `RequestApproval`, `ListApprovals`) go through a runtime-signed `WalletContract` envelope,
and the top-level ops are only `init`/`status`/`wallet_contract`/`shutdown`. The entire creator
money-path (`creator.rs`) still hand-built the **legacy v1** shapes (`{op:"accounts"}`,
`{op:"request_signature"}`, `{op:"approval_requests"}`), which the v2 provider rejects.

**Fix** — Migrated the whole creator money-path to v2 in
[`creator.rs`](elastos/crates/elastos-server/src/api/creator.rs), routed through **one seam** so no caller
hand-builds a wallet request:

- `wallet_invoke(registry, authority, op)` — the single dispatch over `RuntimeWalletAdapter` (the one place
  that speaks v2).
- `resolve_owner_account` → `ListAccounts`; `resolve_mint_tx_hash` → `ListApprovals`.
- `request_transaction_approval(…)` — the single `RequestApproval` dispatch shared by create-channel, mint,
  and enable-trading (was three copy-pasted blocks).
- Each handler mints a `RuntimeWalletAuthority` from its creator launch token
  (`require_home_launch_token_binding` → `runtime_wallet_authority`); the `authority` carries the principal,
  so the redundant `principal_id` params and the duplicate token-validation gates were removed.

---

## 14. Mint "Sign with wallet" fails: "Wallet connector returned an invalid typed handoff: missing field `gas`"

**Symptom** — after §13, the sign step reaches the wallet connector (MetaMask), which then errors:

```
request failed: 400 Bad Request  Wallet connector returned an invalid typed handoff: missing field `gas`
```

**Cause** — Another legacy→ESP mismatch, exposed once the sign path finally reached the connector. The ESP
connector handoff requires a **fully-specified** transaction: `HomeWalletConnectorTransaction`
([`gateway_home_wallet_connector.rs`](elastos/crates/elastos-server/src/api/gateway_home_wallet_connector.rs))
and the browser validator (`validTransaction` in
[`home-wallet-connector-host.js`](capsules/home/browser/home-wallet-connector-host.js)) both require
`gas`/`gasPrice`/`nonce` as hex. But the creator built a **gas-less** intent (the legacy "let MetaMask
estimate" approach), and the wallet provider forwards those fields only when present.

**Fix** — The chain-provider already has a `prepare_transaction` op that populates `nonce`/`gas_price`/
`gas_limit` via RPC into the exact intent shape. `request_transaction_approval` in
[`creator.rs`](elastos/crates/elastos-server/src/api/creator.rs) now re-materializes the caller's
`{from,to,value,data}` through it before the `RequestApproval` — one central place, so every money-path
transaction is fully specified before the handoff.

> `prepare_transaction` runs `eth_estimateGas`; if the mint would revert on-chain (e.g. channel not created
> yet), estimateGas errors there — a real on-chain condition, surfaced clearly instead of a cryptic handoff.

---

## 15. Opening a recovered asset fails: "object session unavailable: 401" / `authority context mismatch`

**Symptom** — a **fresh** asset (§11) recovers successfully (`node-set precheck: MATCH`, `recover ok` ×3,
the viewer loads), then the viewer shows a lock:

```
Could not open the protected asset: object session unavailable: 401
```

and the gateway log names the reason exactly:

```
authorize_object_session: launch-token rejected for viewer='ddrm-viewer' … (401): home launch token authority context mismatch
```

**Cause** — The manifest gate
([`gateway_home_token.rs`](elastos/crates/elastos-server/src/api/gateway_home_token.rs), the
`authority context mismatch` check) requires the token's `launch_context.authority_actor` to be in the
active auth-session **grant's `apps`** list. That list is the **shells only** —
`["home", "system"]` ([`auth_gateway.rs`](elastos/crates/elastos-server/src/api/auth_gateway.rs)) — never a
viewer capsule. But the open path minted the viewer token via `direct("ddrm-viewer")`, whose
`authority_actor` **is** `"ddrm-viewer"` → not in the grant → 401. (Proof it's this: the creator app works
because it's minted via `projection`, whose `authority_actor` is `"home"`, which *is* in the grant.)

**Fix** — All four viewer-token mint sites (object + media) in
[`viewer_open.rs`](elastos/crates/elastos-server/src/api/viewer_open.rs) now use
`issue_home_projection_launch_token_with_context` (authority_actor=`home`, executable_actor=the viewer) —
the same **projection** path Home uses for every other app. `executable_actor` still scopes the token to the
viewer (the manifest's app check passes); only the authority is now `home`, which the grant admits.

---

## 16. (Infra, prerequisite to §11) dKMS recover never completes: node rejects the `init` handshake / RPC 429s

**Symptom** — before §11 could even surface `bad_node_set`, the recover 502'd earlier: the key-provider's
wire `init` was rejected by the DKMS-7-hardened node, and/or the on-chain rights check flapped with HTTP
`429` from `mainnet.base.org`.

**Cause & Fix** — two independent infra issues:
- **`init` handshake** — DKMS-7 nodes are provisioned **offline** and reject a wire `init`
  ("init is not a network operation"). The key-provider
  ([`key-provider/src/main.rs`](capsules/key-provider/src/main.rs)) no longer sends it — it goes straight
  from connect+preamble to the identity/hello handshake.
- **RPC 429s** — [`chain-provider/src/backends.rs`](capsules/chain-provider/src/backends.rs) got
  retry-with-backoff on transient errors (`evm_rpc_pool_call`, 429/5xx/transport), and the reliable
  `https://base-rpc.publicnode.com` is pinned as the default `ELASTOS_CHAIN_BASE_RPC`.

---

## 17. Debugging the Create/open flow — logs & instrumentation

Every failure above was diagnosed from **per-stage gateway logs**. The `.vscode` debug task
(`elastos: bring up gateway (debug)`) is wired to write them to a file and use the right log level:

- `RUST_LOG=info,elastos_server=debug,elastos_server::api::viewer_open=trace,elastos_server::api::media_authority=trace,elastos::provider::latency=error`
- the gateway's full output is `tee`'d to **`~/Library/Application Support/elastos/dkms/gateway.log`**.

**Log surfaces to correlate (a single open touches all of them):**

| Log | Path | Shows |
|---|---|---|
| Gateway | `~/Library/Application Support/elastos/dkms/gateway.log` | node-set precheck, rights decision, `creator_*`, `authorize_object_session`, `home_launch` route, `serve_browser_capsule` |
| dKMS nodes | `~/Library/Application Support/ElastOS/dkms/docker-logs/node-{0,1,2}.log` | `hello ok` / `recover ok` / `bad_node_set` (set `DKMS_AUTHORITY_LOG_LEVEL=debug`) |
| Warm key-provider daemon | `~/Library/Application Support/elastos/dkms/key-provider-daemon.out` | dial/session/recover per node |
| Carrier sidecar | `~/Library/Application Support/elastos/dkms/carrier-client.{out,err}` | did:key warm connections over iroh/QUIC |

**High-signal lines to grep** (added as diagnostics this cycle):

```bash
grep -nE "node-set precheck|node-set evidence|creator_status|creator_wallet|home_launch:|serve_browser_capsule|authorize_object_session" \
  "$HOME/Library/Application Support/elastos/dkms/gateway.log"
```

- `creator_status: READY` + `creator_wallet: … returned N account(s)` → the Create portal preflight + wallet
  list both worked (§12/§13).
- `home_launch: target=… route=…#home_token=<N chars>` → proves the launch token is delivered (in the
  **fragment**) for a given app (§12).
- `serve_browser_capsule: app=… path=…` → proves which capsule assets the browser actually fetched.

**Meta-lesson for this branch:** when a Create/open step fails, first ask *"what legacy contract did ESP
change here?"* — token location (query→fragment), wallet protocol (v1→v2), transaction completeness
(estimate→fully-specified), and launch-token authority (direct→projection) were all the same shape of bug.

---

## Further reading — documentation map

**Dev setup & running Home**
- [`docs/DEV_BOOTSTRAP.md`](docs/DEV_BOOTSTRAP.md) — the one-command dev gateway; the dKMS-mode/rights env matrix.
- [`docs/RUN_HOME_MACOS.md`](docs/RUN_HOME_MACOS.md) · [`docs/MAC.md`](docs/MAC.md) — macOS source-home install + Browser VM run guides.
- [`docs/GETTING_STARTED.md`](docs/GETTING_STARTED.md) — first-time orientation.
- [`docs/DEBUG.md`](DEBUG.md) *(repo root `DEBUG.md`)* — debugging policy; how to attach/inspect.
- [`README.md`](README.md) · [`AGENTS.md`](AGENTS.md) — repo overview & contributor guide (per-repo tooling differs across the wider workspace).

**Capsules & the Home shell** (the "Browser capsule not found" / app-catalog world — §5, §6)
- [`docs/CAPSULE_MODEL.md`](docs/CAPSULE_MODEL.md) · [`docs/CAPSULE_AUTHORING.md`](docs/CAPSULE_AUTHORING.md) — the capsule model & how they're built.
- [`docs/BROWSER_CAPSULE.md`](docs/BROWSER_CAPSULE.md) — the Browser capsule architecture.
- [`docs/HOME_SHELL_HOST_CONTRACT.md`](docs/HOME_SHELL_HOST_CONTRACT.md) — the Home shell host contract.
- [`GLOSSARY.md`](GLOSSARY.md) — canonical terms.

**Browser VM engine** (the optional native-render microVM — §7, §8)
- [`docs/BROWSER_VM_TARGET.md`](docs/BROWSER_VM_TARGET.md) — the per-launch Browser VM target contract.
- [`docs/BROWSER_PROVIDER_BAKEOFF.md`](docs/BROWSER_PROVIDER_BAKEOFF.md) — engine options / trade-offs.

**Auth & audit chain** (the "unchained auth state" guard — §3)
- [`docs/AUTH_AUDIT_CHAIN.md`](docs/AUTH_AUDIT_CHAIN.md) — how auth state becomes audit-chain protected and why the guard fails closed.
- `elastos migrate-audit-chain --help` — the offline migration tool used in §3.

**dKMS quorum** (the docker mesh & the rights/open path — §1, §2)
- [`docs/dkms/DEV_SETUP.md`](docs/dkms/DEV_SETUP.md) — the local dKMS / dDRM harness (docker mesh, `up.sh`, env passthrough).
- [`docs/dkms/RUN_E2E.md`](docs/dkms/RUN_E2E.md) — end-to-end dKMS flow.
- [`docs/DKMS_NODE_PROVISIONING.md`](docs/DKMS_NODE_PROVISIONING.md) · [`docs/DKMS_OVER_CARRIER.md`](docs/DKMS_OVER_CARRIER.md) — node layout & Carrier transport.
- [`docs/dkms/deploy/RUNBOOK.md`](docs/dkms/deploy/RUNBOOK.md) — **production** quorum deploy + the provisioning output contract (§6 of the runbook).
- [`docs/dkms/SECURITY_MODEL.md`](docs/dkms/SECURITY_MODEL.md) — the dKMS trust model.

**Architecture**
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) · [`docs/ARCHITECTURE_MAP.md`](docs/ARCHITECTURE_MAP.md) — system architecture.
- [`docs/CARRIER.md`](docs/CARRIER.md) — the Carrier P2P/gossip transport.
- [`docs/CHAIN_PROVIDER.md`](docs/CHAIN_PROVIDER.md) · [`docs/DECRYPT_PROVIDER.md`](docs/DECRYPT_PROVIDER.md) — provider internals.

> Line numbers drift; when a section here cites code, trust the symbol/path over the exact line.
> If a doc and the code disagree, the code (`capsules/`, `elastos/crates/elastos-server/src/`) is authoritative.
