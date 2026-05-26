# Command Runtime Matrix

Every `elastos` command has exactly one runtime expectation. No command may hang.

For the narrower interactive front-door contract, see [INTERACTIVE_RUNTIME_CONTRACT.md](INTERACTIVE_RUNTIME_CONTRACT.md).

Single-host rule: one ElastOS home may have one live host owner at a time. Managed dashboard paths own the managed Home lane, `elastos serve` owns the explicit operator lane, and `elastos room open` is the explicit helper that reuses the live operator lane instead of starting a second host.

## Runtime Classes

| Class | Description | Auto-start |
|-------|-------------|------------|
| **No Runtime** | Runs without a daemon. May call explicit provider/helper components such as `ipfs-provider` when installed. | No |
| **Managed Dashboard Runtime** | Auto-starts/reuses a dedicated local runtime for the dashboard/home surface | Yes |
| **Managed User Runtime** | Auto-starts/reuses background runtime | Yes |
| **Operator Runtime** | Requires explicit `elastos serve` running | No |
| **Service Entrypoint** | Starts or attaches to an explicit service surface | N/A |

## Interactive Surface Status

Not every interactive command is equally product-facing.

- First-class public front door:
  - `elastos`
  - `elastos home`
  - `Home -> System/Documents/Library/Inbox`
- Secondary supported shortcut:
  - `elastos chat`
- Secondary packaged surface path:
  - `elastos capsule <name> --lifecycle interactive --interactive`
- Operator or developer surfaces:
  - `elastos agent`
  - `elastos run ...`
  - non-interactive `elastos capsule ...`

The detailed runtime/TTY/home contract for those paths lives in [INTERACTIVE_RUNTIME_CONTRACT.md](INTERACTIVE_RUNTIME_CONTRACT.md).

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
| `elastos verify` | Checks local capsule signatures offline; provenance CID reads use `elastos://content/*` |
| `elastos sign` | Signs capsule offline |
| `elastos keys *` | Key management |
| `elastos source *` | Trusted source config |
| `elastos publish-release` | Spawns own pipeline |
| `elastos config *` | Local config file |
| `elastos emergency *` | Key rotation |
| `elastos room show|pending|seed|invite-*|accept-*|approve|deny|reset` | Local sovereign room control, summary, pending browser-access review, and signed invite/accept envelope flow. `room show/pending/approve/deny` can review and resolve browser access from the CLI without a live runtime. |
| `elastos node info` | Local operator-node identity and route snapshot; no local runtime required |
| `elastos node peer add|list|remove` | Local operator peer config; no local runtime required |
| `elastos node status --peer <did>` | Source-side operator command; the target peer should be prepared with `elastos setup --profile operator` and running explicit `elastos serve` |
| `elastos node room * --peer <did>` | Source-side explicit remote room control over Carrier. Reads room state, reviews pending browser access requests, approves/denies them, and starts/reuses the remote room gateway. The target peer must explicitly allow `room.read`, `room.approve`, `room.deny`, and/or `room.open`. |
| `elastos node update --peer <did> --check` | Source-side operator command; the target peer should be prepared with `elastos setup --profile operator` and running explicit `elastos serve` |
| `elastos node update --peer <did> --apply --yes` | Source-side operator command; mutating remote trusted-source update; target restart is still manual and the target peer should be prepared with `elastos setup --profile operator` |
| `elastos share` | Host-side content bundle published through `elastos://content/*`; exits immediately. On a fresh installed layout, add the explicit extras first: `elastos setup --with kubo --with ipfs-provider --with documents` |
| `elastos share --public` | Host-side content bundle published through `elastos://content/*` plus explicit tunnel-provider public edge; keeps the immediate public link alive until Ctrl+C |
| `elastos open` | Host-side data-capsule materialization through `elastos://content/*` + local web serve. Release object CIDs are opened as verified terminal metadata summaries, not launchable capsules. On a fresh installed layout, add the explicit extras first: `elastos setup --with kubo --with ipfs-provider --with documents` |
| `elastos shares *` | Local catalog plus content-provider channel-head reads/writes |
| `elastos attest` | Provenance signing and optional share digest fetch through `elastos://content/*` |
| `elastos site stage` | Stages a static site into `localhost://MyWebSite` |
| `elastos site path` | Prints the staged local root and filesystem path |
| `elastos site publish [--release <name>]` | Packages the current site root through `elastos://content/*` as an immutable CID-backed site bundle, prints `elastos://<cid>`, and can store a friendly named release alias under `localhost://ElastOS/SystemServices/Publisher/SiteReleases/...`. On a fresh installed layout, add `elastos setup --with kubo --with ipfs-provider` first |
| `elastos site releases` | Lists named site releases stored under `localhost://ElastOS/SystemServices/Publisher/SiteReleases/...` |
| `elastos site channels` | Lists promotion channels stored under `localhost://ElastOS/SystemServices/Edge/ReleaseChannels/...` |
| `elastos site activate [--release <name> | --channel <name>]` | Either publishes the current site root through `elastos://content/*`, activates an existing named release, or activates the release currently promoted to a channel, then signs it into Edge site-head state under `localhost://ElastOS/SystemServices/Edge/SiteHeads/...`. On a fresh installed layout, add `elastos setup --with kubo --with ipfs-provider` first when activation needs CID-backed publish/fetch support |
| `elastos site history` | Lists signed site-head activation snapshots from `localhost://ElastOS/SystemServices/Edge/SiteHistory/...` |
| `elastos site rollback [release-or-bundle-cid]` | Re-points the active site head to a previous published site bundle or named release snapshot and records a new rollback activation |
| `elastos site promote <channel> <release>` | Promotes a named release into an Edge release channel under `localhost://ElastOS/SystemServices/Edge/ReleaseChannels/...` |
| `elastos site bind-domain` | Writes a runtime-owned public-edge domain binding under `localhost://ElastOS/SystemServices/Edge/Bindings/...` |
| `elastos webspace list [path]` | Queries the dynamic `localhost://WebSpaces/<moniker>/...` resolver surface directly. Today `Elastos` exposes typed children such as `content`, `peer`, `did`, and `ai`; deeper `peer` / `did` / `ai` traversal fails closed until richer resolver semantics exist |
| `elastos webspace resolve <target>` | Resolves a mounted WebSpace moniker or deeper handle path into a typed local handle. Current contract: `resolve` is handle-only; `content/<cid>` resolves to a file endpoint, `peer/<id>`, `did/<did>`, and `ai/<backend>` resolve to one typed folder handle, and `_meta.json` is a metadata file view for `read` / `stat`, not another handle |
| `elastos run` (Data) | Power-user explicit path/CID launch. Data capsules are served in-process. |
| `elastos home --status` | Host-side snapshot of Home state |
| `elastos home --json` | Machine-readable host-side snapshot of Home state |

### Managed Dashboard Runtime

| Command | Notes |
|---------|-------|
| `elastos` | Default user entrypoint. Opens Home with no subcommand. |
| `elastos home` | Explicit Home entrypoint. Auto-starts/reuses a dedicated managed Home runtime on loopback, renders the local Home surface, and returns Home after launched actions exit. |
| `elastos capsule <name> --lifecycle interactive --interactive` | Interactive packaged capsule path. Reuses a compatible active runtime when one is already running; otherwise uses the managed Home runtime. |

### Managed User Runtime (auto-start)

| Command | Policy needed | Notes |
|---------|---------------|-------|
| `elastos chat` | peer, did, `Users/<principal-root>/.AppData/LocalHost/Chat` | Native Carrier chat only. First tries to reuse a healthy managed Home runtime; otherwise starts/reuses a managed chat runtime on loopback. Packaged full-screen chat and `chat-wasm` surfaces launch through `elastos capsule ...`, not `elastos chat`. |

### Operator Runtime (requires `elastos serve`)

| Command | Notes |
|---------|-------|
| `elastos agent` | Shell/supervisor orchestration via forward_to_shell; chat-managed runtime does not satisfy this |
| `elastos room open` | Requires a running operator runtime in the same home. Reuses the live `elastos serve` runtime and opens the hosted room gateway through it; it does not start a second host. The browser-hosted adapter it exposes comes from `elastos setup --profile demo`. |
| `elastos run` (MicroVM) | Supervisor capsule launch; chat-managed runtime does not satisfy this |
| `elastos run` (WASM) | Attaches to running runtime for provider bridge; chat-managed runtime does not satisfy this |
| `elastos capsule` (non-interactive) | Supervisor capsule management that is not an interactive packaged app surface still requires `elastos serve` |

### Service Entrypoint

| Command | Notes |
|---------|-------|
| `elastos serve` | Starts the runtime daemon |
| `elastos gateway` | Starts a direct gateway service when no local operator runtime already owns the home; otherwise reuses the running local operator runtime authority |
| `elastos site serve` | Starts a direct static site service in local or ephemeral mode |

## Rules

1. **No command may hang.** If a path cannot complete, it must timeout or fail fast.
2. If a command is "No Runtime" — it never reads runtime-coords.json.
3. `elastos home --status` and `--json` are host-side probes; they never auto-start a runtime.
4. If a command is "Managed Dashboard Runtime" — it auto-starts if prerequisites are met, reuses its dedicated managed dashboard runtime if running, and keeps the user model centered on Home rather than raw runtime nouns. Launched actions should return to the same Home session automatically.
5. If a command is "Managed User Runtime" — it auto-starts if prerequisites met, reuses if running.
6. If a command is "Operator Runtime" — it fails fast with:
   ```
   This command requires a running runtime.

     elastos serve

   Then run this command again.
   ```
7. Host-side provider bridge commands are explicit operator tooling, not app-capsule authority. Normal app/viewer/content capsules still go through runtime capabilities and provider/Carrier contracts.
8. `elastos run` is the explicit power-user path for arbitrary path/CID capsules. Data capsules run in-process; WASM and MicroVM paths require a running operator runtime.
9. `elastos` and `elastos serve` are different lanes for the same home. They do not currently merge into one shared live session. `elastos room open` is the one explicit helper that reuses the live operator lane.

## Future: Expanding Managed Runtime

To move `agent` into the managed user runtime:
1. Define the required policy additions explicitly.
2. Prove the shell/supervisor orchestration works with that policy.
3. Update this matrix.
4. Do not widen the policy speculatively.
