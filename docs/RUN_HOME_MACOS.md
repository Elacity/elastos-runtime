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

# 3. the gateway itself
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

### Overrides (optional)

| Env var | Purpose |
| ------- | ------- |
| `ELASTOS_DDRM_DECRYPT_BIN` | Path to the `rail-stream,rail-mint` decrypt-provider binary |
| `ELASTOS_DDRM_MEDIA_AUTHORITY_BIN` | Path to the `ddrm-media-authority` helper |
| `ELASTOS_DDRM_SAMPLE_VIDEO` | Use your own source clip instead of the synthesized one |

### dDRM troubleshooting

| Symptom | Cause | Fix |
| ------- | ----- | --- |
| `could not open owned media: … decrypt-provider did not configure` | decrypt-provider built as the fail-closed stub (no rail features) | Rebuild with `--features rail-stream,rail-mint` |
| `media-authority helper not found at …` | helper not built, or running a gateway from a different tree | Build step 2 above, run gateway from this repo, or set `ELASTOS_DDRM_MEDIA_AUTHORITY_BIN` |
| `could not open owned media: … ffmpeg` | `ffmpeg`/`ffprobe` not on `PATH` | `brew install ffmpeg` |
| Tile missing from launcher | gateway built/run from a different `capsules/` tree | Build + run the gateway from this repo on `feat/ddrm-home-playback` |

---

## Troubleshooting

| Symptom | Cause | Fix |
| ------- | ----- | --- |
| `This is an invalid domain.` on *Use passkey* | Page opened on `127.0.0.1` (bare IP) | Open `http://localhost:8090/apps/home/` instead |
| `request failed: 500 … trailing characters` after login | Corrupt `browser-state.json` (shared with Electron app) | Use the `fix/home-summary-resilience` binary; it resets corrupt state |
| `another ElastOS host already owns … host-process.lock` | Desktop app / another `serve`/`gateway` holds the lock | Quit `ElastOS.app`, `kill` the pid in `host-process.lock`, retry |
| `elastos-server` won't compile on macOS | Missing the crosvm Darwin gate | Build from `fix/home-summary-resilience` (includes `5b167f1`) |
| Passkey not offered on `localhost` | Existing passkey bound to the desktop app's origin | Create a fresh passkey on the `localhost` page (same identity vault) |

## Cleanup

```bash
# Stop the gateway (Ctrl-C if foreground, or kill the pid)
# Remove the build worktree when done:
git worktree remove /tmp/elastos-mac-home
```
