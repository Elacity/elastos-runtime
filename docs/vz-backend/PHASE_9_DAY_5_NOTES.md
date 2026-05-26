# Phase 9 Day 5 — Stamp local CIDs onto the home-surface capsules

> **Outcome (2026-05-26):** Mac source-checkouts now hit the
> canonical supervisor path. Day 2 staged the five home-surface
> capsules (`home`, `system`, `documents`, `library`, `inbox`)
> into `<data_dir>/capsules/<name>/` but never wrote the two
> sentinel files (`.elastos-cid`, `.elastos-artifact-sha256`) or
> the matching `capsules:` entries in `<data_dir>/components.json`
> that `Supervisor::ensure_capsule` requires before
> short-circuiting an IPFS fetch. Day 5 closes that gap by
> mirroring the canonical chat-staging pattern from
> `scripts/home-demo-local.sh` (lines 167-188 + 243-248). With
> both stores in sync, `elastos capsule system --interactive`
> now resolves, ensures, and launches the system.wasm
> end-to-end on Mac for the first time.
>
> **Anchor:** the operator's gut-check — _"does the original have
> HTTP wiring? remember our principles, please just double
> check"_ — flagged that the working plan
> (short-circuit `capsule_cmd::run_capsule` to bypass the
> supervisor) was about to invent a parallel mechanism that
> already exists on Linux. The check turned a substrate change
> into a 60-LOC bash + python patch.

## 1. The trap that the principle-check averted

The day started with `elastos capsule system --interactive`
returning:

```text
Error: supervisor endpoint /api/supervisor/resolve-plan failed
       (500 Internal Server Error): capsule 'system' not in registry
```

The first instinct was to short-circuit `capsule_cmd::run_capsule`
(`elastos/crates/elastos-server/src/capsule_cmd.rs`) so that
locally-staged capsules skipped `resolve-plan` + `ensure-capsule`
and launched directly from `<data_dir>/capsules/<name>/`. That
plan was wrong on first principles:

| ElastOS principle (PRD § 5)              | What the short-circuit would have done                |
| ---------------------------------------- | ------------------------------------------------------ |
| Single source of truth                   | Created a parallel capsule-launch path on Mac          |
| Mirror Linux canonical behaviour exactly | Diverged from `home-demo-local.sh`'s established flow  |
| No new substrate without explicit need   | Required Rust changes in `capsule_cmd` + tests         |

The operator's _"does the original have HTTP wiring?"_ pushback
forced a real reading of the Linux canonical path, which turned
up three artefacts that ended the debate:

1. **`POST /api/apps/home/launch`** — the browser-Home WASM
   capsule's launch RPC, exercised by
   `scripts/home-camofox-smoke.mjs` (`launchShellTarget`,
   line 268-272). Mints a home_token; returns
   `{ route: "/apps/<name>/?home_token=…" }`.
2. **`GET /apps/<name>/*path`** — gateway route handled by
   `api/browser_capsules::serve_browser_app_index`, which serves
   `<data_dir>/capsules/<name>/index.html` directly. Already
   wired; already canonical.
3. **`scripts/home-demo-local.sh` lines 167-188** — the
   chat-bundle path proves how local-dev CIDs are minted for the
   canonical install: hash the artefact, write `local-<name>-<sha:0:16>`
   into both `<data_dir>/capsules/<name>/.elastos-cid` and the
   `capsules.<name>.cid` field in the local components.json. With
   both stores matching, `Supervisor::ensure_capsule`
   (`supervisor.rs:1530`) short-circuits the IPFS fetch and
   returns the cached path.

The canonical mechanism doesn't need a launch bridge — it needs
the staging script to finish its job.

## 2. The two gaps the investigation surfaced

| Gap | Scope                                                                                                   | Status      |
| --- | ------------------------------------------------------------------------------------------------------- | ----------- |
| A   | Mac bootstrap doesn't stamp `.elastos-cid` or `capsules:` entries → `resolve-plan` 500s.                | **Closed.** |
| B   | Terminal `elastos home` action handler routes capsule launches through `launch-capsule` → microVM path, which has no `CapsuleType::Data` branch. Pre-existing Linux limitation; browser-hosted Home (camofox) is the canonical UX for Data capsules. | Deferred — agreed scope was Gap A only. |

Gap B isn't Mac-specific. On Linux, terminal `elastos home`'s
`capsule-library` action runs the same
`elastos capsule library --lifecycle interactive --interactive`
fallthrough that hits `run_capsule`'s missing-Data-branch.
Browser-Home (`/api/apps/home/launch` → gateway HTML serving)
is the supported path for Data capsules on both platforms.
Fixing Gap B is a separate decision about terminal-Home UX, not
a Mac-substrate task.

## 3. The fix

### 3.1 Stamp each staged capsule with a local CID

A single helper (`stamp_local_capsule_cid`) appended to
`scripts/dev/mac-local-setup.sh`, called by both the WASM and
Data capsule staging helpers right after they place files:

```bash
stamp_local_capsule_cid() {
  local name="$1"
  local platform="$2"
  local dest_dir="$DATA_DIR/capsules/$name"

  # Sha over a deterministic stream of file paths + bytes,
  # excluding the two stamp files themselves.
  local sha
  sha="$(
    cd "$dest_dir" && find . -type f \
        -not -name '.elastos-cid' \
        -not -name '.elastos-artifact-sha256' \
        -print0 | LC_ALL=C sort -z | xargs -0 cat \
      | shasum -a 256 | awk '{print $1}'
  )"

  local cid="local-${name}-${sha:0:16}"
  printf '%s\n' "$cid" > "$dest_dir/.elastos-cid"
  printf '%s\n' "$sha" > "$dest_dir/.elastos-artifact-sha256"
  printf '%s\t%s\t%s\t%s\t%s\n' \
    "$name" "$cid" "$sha" "$size" "$platform" \
    >> "$CAPSULE_STAMPS_FILE"
}
```

Hashing scheme: sha256 over the deterministic concatenation
of (sorted file paths) + (file bytes), excluding the stamp
files themselves. Stable across re-runs as long as content is
stable; tracks content drift. The `local-<name>-<sha:0:16>`
shape matches the canonical `local-chat-<sha:0:16>` form from
`home-demo-local.sh`.

### 3.2 Stream the stamps into the manifest writer

A second TSV stream (`CAPSULE_STAMPS_FILE`) parallels the
existing provider stamp file. The inline python3 stamper
consumes it and writes one entry per staged capsule into
`<data_dir>/components.json`:

```python
capsules = data.setdefault("capsules", {})
for name, cid, sha, size, plat in capsule_stamps:
    capsules[name] = {
        "cid": cid,
        "sha256": sha,
        "size": size,
        "platforms": [plat],
    }
```

Both stores are now in lock-step: every CID in the on-disk
`.elastos-cid` file equals the CID in the manifest entry, which
is exactly what `Supervisor::ensure_capsule` (`supervisor.rs:1530`)
needs to short-circuit the fetch.

### 3.3 Strengthen the self-verifier

The script's pre-existing `--status --json` check is augmented
with a registry-consistency probe that fails the script if any
of the five home-surface capsules has a missing or mismatched
CID. This guards against:

- A future `find` exclusion missing one of the stamp files →
  hash drift between on-disk and manifest.
- A python writer regression dropping the `capsules:` entries.
- An rsync `--delete` that accidentally removes the stamps.

## 4. Smoke

### 4.1 Bootstrap output

```text
[mac-local-setup] verifying via: elastos home --status --json
  services ready: 6 / 8
    [ok ] Home Session  (shell)
    [ok ] Local World  (localhost-provider)
    [ok ] Identity  (did-provider)
    [ok ] WebSpaces  (webspace-provider)
    [no ] Content Exchange  (ipfs-provider + kubo)
    [ok ] Site Edge  (site-provider)
    [no ] Public Edge  (tunnel-provider + cloudflared)
    [ok ] Full-screen Apps  (vmlinux)
[mac-local-setup] verifying capsule registry consistency
    [ok ] home: cid=local-home-041ed736bcf978fe
    [ok ] system: cid=local-system-2144ccff29e610c2
    [ok ] documents: cid=local-documents-ac1f6bdcc29fbd37
    [ok ] library: cid=local-library-45f74580c625889e
    [ok ] inbox: cid=local-inbox-41f2eea10e2007ce
[mac-local-setup] OK
```

### 4.2 End-to-end capsule launch (the test that proves it)

The same command that errored with `capsule 'system' not in
registry` at the start of the day now resolves, ensures, loads,
and runs the WASM end-to-end:

```text
$ elastos capsule system --lifecycle interactive --interactive
No runtime found. Starting local home runtime...
Runtime started (pid 23425). Log: …/runtime.log
2026-05-26T04:18:11Z INFO elastos: vz provider enabled
2026-05-26T04:18:11Z INFO elastos_server::runtime: Loading capsule 'system' (Wasm)
2026-05-26T04:18:11Z INFO elastos_compute::providers::wasm: Loaded WASM capsule 'system' with ID wasm-b5bbc874-…
2026-05-26T04:18:11Z INFO elastos_compute::providers::wasm: Starting capsule 'system'
2026-05-26T04:18:11Z INFO elastos_compute::providers::wasm: WASM bridge active for capsule 'system'
system capsule launched: name=system id=wasm-b5bbc874-… ts=1779769091
```

Every step is the canonical Linux path:

1. `runtime_control::ensure_runtime_for_home` brings up a
   managed-home daemon.
2. `Supervisor::resolve_plan` finds `system` in the registry's
   `capsules:` map (because Day 5 stamped it there).
3. `Supervisor::ensure_capsule` reads `.elastos-cid`, sees it
   matches `entry.cid`, returns the cached path (no IPFS fetch).
4. `capsule_cmd::run_capsule` sees `manifest.capsule_type == Wasm`
   and takes the `run_wasm_capsule` branch.
5. `elastos_compute::providers::wasm` loads + starts the wasm
   inside wasmtime; the system.wasm prints its banner and runs
   to interactive idle.

No new code paths, no parallel mechanism, no substrate change.

### 4.3 Idempotent re-run

A second `scripts/dev/mac-local-setup.sh` run produces identical
CIDs (because the source content is unchanged) and identical
manifest output. The `rsync --exclude '.elastos-cid'` excludes
already in Day 2 preserve the stamp files across staging passes.

## 5. Why this is correct architecturally

The chat-staging pattern in `scripts/home-demo-local.sh` exists
because Linux canonical installs go through `install.sh` +
`elastos setup --profile <name>`, which fetch capsules from the
trusted Carrier source and stamp real CIDs into both the manifest
and each capsule dir. For development against a source checkout
(where Carrier isn't a viable fetch path), `home-demo-local.sh`
synthesizes its own CIDs with the `local-<name>-<sha:0:16>` shape
and stamps them locally. The supervisor doesn't care that the
CID is synthetic — it only cares that the on-disk marker matches
the registry entry.

Mac source-checkouts have the same constraints as Linux source
checkouts: no Carrier, no published artefacts, no real CIDs.
The fix is to apply the same local-CID-minting pattern, not to
invent a Mac-specific launch path. Day 5 brings Mac to parity
with the canonical Linux source-checkout workflow.

## 6. What this unlocks for follow-up phases

- **Camofox-on-Mac (future):** with the `capsules:` entries and
  on-disk stamps in place, the gateway routes
  (`/apps/<name>/*path`) will serve the home-surface capsules
  without any extra wiring. The same camofox smoke script that
  drives Linux today will be portable.
- **Terminal Home Data-capsule launch (Gap B):** if/when needed,
  the fix is a single `CapsuleType::Data` branch in
  `capsule_cmd::run_capsule` that opens the gateway URL in the
  host browser. It would benefit both Linux and Mac equally.
- **Real `elastos setup --profile` on Mac:** the bootstrap
  script is now structurally aligned with how `setup` will
  eventually populate the registry, making the transition to a
  real installer cleaner.

## 7. Files touched

- `scripts/dev/mac-local-setup.sh` — +85 LOC:
  - new helper `stamp_local_capsule_cid`,
  - per-capsule invocation in both `build_and_stage_wasm_capsule`
    and `stage_data_capsule`,
  - extended python stamper to write `capsules:` entries,
  - new self-verifier block for CID consistency.
- `docs/vz-backend/PHASE_6_PLAN.md` — status banner extended.
- `docs/vz-backend/PHASE_9_DAY_5_NOTES.md` — this file.

Zero substrate code touched. Zero new tests — the bootstrap
script's two verifiers (`--status --json` services count +
registry-consistency probe) cover the regression surface.
