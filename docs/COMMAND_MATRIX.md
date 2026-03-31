# Command Runtime Matrix

Every `elastos` command has exactly one runtime expectation. No command may hang.

## Runtime Classes

| Class | Description | Auto-start |
|-------|-------------|------------|
| **No Runtime** | Runs without a daemon. May spawn local helpers (IPFS bridge, etc.) | No |
| **Managed Dashboard Runtime** | Auto-starts/reuses a dedicated local runtime for the dashboard/home surface | Yes |
| **Managed User Runtime** | Auto-starts/reuses background runtime | Yes |
| **Operator Runtime** | Requires explicit `elastos serve` running | No |
| **Starts Own Service** | Starts its own daemon or service process | N/A |

## Command Classification

### No Runtime Required

| Command | Notes |
|---------|-------|
| `elastos --version` | |
| `elastos version` | |
| `elastos --help` | |
| `elastos setup` | Provisions components |
| `elastos update` | Carrier-only by default after install; explicit transport override required for web bootstrap/debug paths; unstamped installs fail fast with `No trusted source configured` |
| `elastos upgrade` | Alias for update |
| `elastos init` | Scaffolds capsule project |
| `elastos verify` | Checks signatures offline |
| `elastos sign` | Signs capsule offline |
| `elastos keys *` | Key management |
| `elastos source *` | Trusted source config |
| `elastos publish-release` | Spawns own pipeline |
| `elastos config *` | Local config file |
| `elastos emergency *` | Key rotation |
| `elastos share` | Bundles content + direct IPFS bridge; exits immediately. On a fresh installed layout, add the explicit extras first: `elastos setup --with kubo --with ipfs-provider --with md-viewer` |
| `elastos share --public` | Bundles content + direct IPFS bridge + direct tunnel-provider public edge; keeps the immediate public link alive until Ctrl+C |
| `elastos open` | Direct IPFS bridge + local web serve. On a fresh installed layout, add the explicit extras first: `elastos setup --with kubo --with ipfs-provider --with md-viewer` |
| `elastos shares *` | Local catalog + direct IPFS bridge |
| `elastos attest` | Provenance signing + direct IPFS bridge |
| `elastos site stage` | Stages a static site into `localhost://MyWebSite` |
| `elastos site path` | Prints the staged local root and filesystem path |
| `elastos site publish [--release <name>]` | Packages the current site root as an immutable CID-backed site bundle, prints `elastos://<cid>`, and can store a friendly named release alias under `localhost://ElastOS/SystemServices/Publisher/SiteReleases/...`. On a fresh installed layout, add `elastos setup --with kubo --with ipfs-provider` first |
| `elastos site releases` | Lists named site releases stored under `localhost://ElastOS/SystemServices/Publisher/SiteReleases/...` |
| `elastos site channels` | Lists promotion channels stored under `localhost://ElastOS/SystemServices/Edge/ReleaseChannels/...` |
| `elastos site activate [--release <name> | --channel <name>]` | Either publishes the current site root as a CID-backed bundle, activates an existing named release, or activates the release currently promoted to a channel, then signs it into Edge site-head state under `localhost://ElastOS/SystemServices/Edge/SiteHeads/...`. On a fresh installed layout, add `elastos setup --with kubo --with ipfs-provider` first when activation needs CID-backed publish/fetch support |
| `elastos site history` | Lists signed site-head activation snapshots from `localhost://ElastOS/SystemServices/Edge/SiteHistory/...` |
| `elastos site rollback [release-or-bundle-cid]` | Re-points the active site head to a previous published site bundle or named release snapshot and records a new rollback activation |
| `elastos site promote <channel> <release>` | Promotes a named release into an Edge release channel under `localhost://ElastOS/SystemServices/Edge/ReleaseChannels/...` |
| `elastos site bind-domain` | Writes a runtime-owned public-edge domain binding under `localhost://ElastOS/SystemServices/Edge/Bindings/...` |
| `elastos webspace list [path]` | Queries the dynamic `localhost://WebSpaces/<moniker>/...` resolver surface directly. Today `Elastos` exposes typed children such as `content`, `peer`, `did`, and `ai`; deeper `peer` / `did` / `ai` traversal fails closed until richer resolver semantics exist |
| `elastos webspace resolve <target>` | Resolves a mounted WebSpace moniker or deeper handle path into a typed local handle. Current contract: `resolve` is handle-only; `content/<cid>` resolves to a file endpoint, `peer/<id>`, `did/<did>`, and `ai/<backend>` resolve to one typed folder handle, and `_meta.json` is a metadata file view for `read` / `stat`, not another handle |
| `elastos run` (Data) | Power-user explicit path/CID launch. Data capsules are served in-process. |
| `elastos pc2 --status` | Host-side snapshot of the sovereign PC2 world |
| `elastos pc2 --json` | Machine-readable host-side snapshot of the sovereign PC2 world |

### Managed Dashboard Runtime

| Command | Notes |
|---------|-------|
| `elastos` | Default user entrypoint. Opens the sovereign PC2 home surface with no subcommand. |
| `elastos pc2` | Explicit alias for the sovereign PC2 home surface. Auto-starts/reuses a dedicated managed `pc2` runtime on loopback, renders the local `pc2` WASM capsule, and returns home after launched actions exit. |
| `elastos capsule <name> --lifecycle interactive --interactive` | Packaged installed capsule path. Reuses an existing runtime when one is already active; otherwise auto-starts/reuses the managed `pc2` runtime so packaged surfaces like `chat` and `chat-wasm` can launch without `elastos serve`. |

### Managed User Runtime (auto-start)

| Command | Policy needed | Notes |
|---------|---------------|-------|
| `elastos chat` | peer, did, `Users/self/.AppData/LocalHost/Chat` | Native Carrier chat only. Starts/reuses a managed chat runtime on loopback. Packaged IRC/WASM surfaces launch through `elastos capsule ...`, not `elastos chat`. |

### Operator Runtime (requires `elastos serve`)

| Command | Notes |
|---------|-------|
| `elastos agent` | Shell/supervisor orchestration via forward_to_shell; chat-managed runtime does not satisfy this |
| `elastos run` (MicroVM) | Supervisor capsule launch; chat-managed runtime does not satisfy this |
| `elastos run` (WASM) | Attaches to running runtime for provider bridge; chat-managed runtime does not satisfy this |
| `elastos capsule` (non-interactive) | Supervisor capsule management that is not an interactive packaged app surface still requires `elastos serve` |

### Starts Own Service

| Command | Notes |
|---------|-------|
| `elastos serve` | Starts the runtime daemon |
| `elastos gateway` | Starts a direct gateway service |
| `elastos site serve` | Starts a direct static site service in local or ephemeral mode |

## Rules

1. **No command may hang.** If a path cannot complete, it must timeout or fail fast.
2. If a command is "No Runtime" — it never reads runtime-coords.json.
3. `elastos pc2 --status` and `--json` are host-side probes; they never auto-start a runtime.
4. If a command is "Managed Dashboard Runtime" — it auto-starts if prerequisites are met, reuses its dedicated managed dashboard runtime if running, and keeps the user model centered on PC2/home rather than raw runtime nouns. Launched actions should return to the same home session automatically.
5. If a command is "Managed User Runtime" — it auto-starts if prerequisites met, reuses if running.
6. If a command is "Operator Runtime" — it fails fast with:
   ```
   This command requires a running runtime.

     elastos serve

   Then run this command again.
   ```
7. `elastos run` is the explicit power-user path for arbitrary path/CID capsules. Data capsules run in-process; WASM and MicroVM paths require a running operator runtime.

## Future: Expanding Managed Runtime

To move `agent` into the managed user runtime:
1. Define the required policy additions explicitly.
2. Prove the shell/supervisor orchestration works with that policy.
3. Update this matrix.
4. Do not widen the policy speculatively.
