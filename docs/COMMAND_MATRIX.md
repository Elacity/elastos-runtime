# Command runtime matrix

This file classifies the documented `elastos` CLI command families by runtime
behavior. See
[Interactive runtime contract](INTERACTIVE_RUNTIME_CONTRACT.md) for terminal
input, exit, and return behavior. CLI help remains the source of truth for the
complete command and flag inventory.

## Runtime classes

| Class | Meaning |
| --- | --- |
| Self-contained | Does not attach to an existing local Runtime host. It may still read local state, use Carrier, spawn a verified provider, or start an embedded preview. |
| Managed user lane | Reuses only the Runtime kinds named for that command, or starts the named managed Runtime with a bounded policy. The user does not start `elastos serve`. |
| Operator lane | Requires a live operator runtime started with `elastos serve`. The command fails if that runtime is absent or incompatible and never widens a managed policy. |
| Service | Starts a long-lived server or gateway and remains active until stopped. |

One ElastOS data home has one live host owner. `elastos serve` and
`elastos gateway` acquire that ownership directly. A managed user runtime owns
it when it starts. A command must not silently start a competing host.

## Front doors and execution

| Command | Class | Actual behavior |
| --- | --- | --- |
| `elastos`, `elastos home` | Managed user lane | Opens Home. `elastos home` starts or reuses the managed Home runtime. It does not attach to an operator runtime that already owns the same home. |
| `elastos home --status`, `elastos home --json` | Self-contained | Reads and prints a Home-state probe. It does not start Home. |
| `elastos chat` | Managed user lane | Reuses a healthy, version-matched managed Home runtime. Otherwise it starts a separate managed chat runtime with its narrower policy, or reuses that runtime when it is already active. It does not attach to an operator runtime. |
| `elastos agent` | Operator lane | Attaches to the operator runtime. This applies to local, Venice, and Codex backends. |
| `elastos run <path>` or `elastos run --cid <cid>`, `type=data` | Self-contained | Materializes the capsule, creates an in-process Runtime and preview server, opens the browser, and remains active until stopped. It does not attach to `elastos serve`. |
| `elastos run <path>` or `elastos run --cid <cid>`, Component | Operator lane | Accepts only `type=wasm` with `runtime_abi="elastos.component/v1"`. It attaches to the operator runtime for Bus authorization, then executes the Component locally. |
| `elastos run <path>` or `elastos run --cid <cid>`, microVM | Operator lane | Uses the operator runtime supervisor to launch and stop the microVM. |
| `elastos run`, other manifest types or no manifest | Self-contained | Falls through to the local runner. This is not a supported 0.6 authoring path. A non-Component WASM manifest is rejected. |
| `elastos capsule <name>` with `--lifecycle interactive` or `--interactive` | Managed user lane | Reuses an active operator, managed Home, or managed Chat runtime recorded for the same data home. If none is recorded, it starts the managed Home runtime. Either option selects the interactive runtime path; `--interactive` also sets the capsule launch flag. |
| `elastos capsule <name>` without an interactive option, Component | Operator lane | Resolves and installs through the operator runtime, then executes the Component locally with Runtime Bus authorization and waits for it. |
| `elastos capsule <name>` without an interactive option, other supported types | Operator lane | Resolves, installs, launches, and waits through the operator supervisor. |
| `elastos serve` | Service | Starts the operator runtime and writes operator runtime coordinates. `--capsule` and `--cid` add an initial capsule launch. |
| `elastos gateway` | Service | Starts the direct gateway host. It does not reuse `elastos serve`; it fails if another host owns the same home. `--public` also starts the public tunnel path. |

## Local administration and release

All commands in this table are self-contained unless the note names a remote
target.

| Command family | Included commands | Notes |
| --- | --- | --- |
| CLI information | `elastos --help`, `elastos --version`, `elastos version` | Reads no runtime coordinates. |
| Setup | `elastos setup`, including `--list`, profiles, `--with`, and `--without` | Installs or lists components and external dependencies. |
| Project creation | `elastos init <name>`, `elastos init <name> --type content` | Creates files in the current directory. |
| Configuration | `elastos config show`, `elastos config set` | Reads or writes local configuration. |
| Identity | `elastos identity show`, `elastos identity nickname get`, `elastos identity nickname set` | Reads or updates the local DID-backed profile. `nickname set` prompts only when no value is supplied and a TTY is available. |
| TLS | `elastos tls trust`, `elastos tls regen` | Prints trust instructions or regenerates the local leaf certificate. |
| Emergency | `elastos emergency rotate` | Attempts to persist a new signing key. The active Runtime changes only after restart, and current persistence failure is not fail-closed. See the [security finding](../SECURITY.md#capability-state-and-key-rotation-are-not-restart-safe). |
| Trusted sources | `elastos source add`, `elastos source list`, `elastos source show`, `elastos source switch-channel`, `elastos source verify` | Manages local trusted-release source state. |
| Updates | `elastos update`, `elastos upgrade` | `upgrade` dispatches to the same update handler. Discovery may use Carrier or explicit gateways, but no local runtime is required. |
| Offline principal-root maintenance | hidden `elastos principal-root-migrate`, hidden `elastos principal-root-upgrade` | Operates on an explicit data directory. The Runtime must be offline and the command requires explicit backup inputs. |

## Trust, content, and publishing

| Command family | Class | Included behavior |
| --- | --- | --- |
| `elastos keys generate`, `elastos keys node-id` | Self-contained | `generate` writes a keypair. `node-id` loads or creates the local Runtime device identity and prints its DID. |
| `elastos sign` | Self-contained | Validates and signs a local capsule manifest and entrypoint. |
| `elastos verify` | Self-contained | Verifies a local capsule signature. With `--cid`, it fetches and verifies provenance through the content provider. |
| `elastos sign-payload` | Self-contained | Reads bytes from stdin and writes a domain-separated Ed25519 signature and signer DID as JSON. |
| `elastos publish <path>` | Self-contained | Validates the manifest, checks that the resolved entrypoint path exists, then publishes through the content provider. A microVM uses the explicitly installed local `ipfs-provider`. [Capsule authoring](CAPSULE_AUTHORING.md#publish-with-the-right-gate) owns the exact validation limits. |
| `elastos publish-release` | Self-contained | Runs the signed release pipeline. Dry-run and preflight modes do not publish. Public URL options may start their own gateway and tunnel step. |
| `elastos share <path>` | Self-contained | Publishes a file or directory, provenance, and a signed channel head unless disabled by flags. |
| `elastos share <path> --public` | Self-contained | Adds an immediate tunnel and remains active until interrupted. It is not a Runtime host. |
| `elastos content publish-object`, `elastos content repair-worker`, `elastos content status` | Self-contained | Each command starts the installed content and IPFS provider path directly. |
| `elastos open <uri>` | Self-contained | Materializes shared content. A release object prints a verified summary; launchable content starts an embedded local server and remains active until stopped. |
| `elastos attest` | Self-contained | Creates provenance for a CID and may fetch the share digest through the content provider. |
| `elastos shares list`, `elastos shares history`, `elastos shares delete-local`, `elastos shares archive`, `elastos shares unarchive`, `elastos shares revoke`, `elastos shares set-did`, `elastos shares head` | Self-contained | Manages the local share catalog and signed channel heads. |

## Site and WebSpace commands

| Command family | Class | Included commands |
| --- | --- | --- |
| Site state | Self-contained | `elastos site stage`, `elastos site path`, `elastos site publish`, `elastos site releases`, `elastos site channels`, `elastos site history`, `elastos site activate`, `elastos site rollback`, `elastos site bind-domain`, `elastos site promote` |
| Site serving | Service | `elastos site serve` starts the local or ephemeral static-site service. |
| WebSpace inspection | Self-contained | `elastos webspace mounts`, `elastos webspace adapters`, `elastos webspace health`, `elastos webspace list`, `elastos webspace resolve`, `elastos webspace head`, `elastos webspace cache-status`, `elastos webspace sync-status` |
| WebSpace mutation | Self-contained | `elastos webspace register-adapter`, `elastos webspace unregister-adapter`, `elastos webspace check-adapter`, `elastos webspace mount`, `elastos webspace unmount`, `elastos webspace index`, `elastos webspace refresh`, `elastos webspace cache`, `elastos webspace sync`, `elastos webspace fork` |

The WebSpace rows list the complete current family. These commands operate on
local mount, adapter, index, cache, and sync state. A configured resolver may
still be unavailable; the command must report that state rather than invent a
local result.

## Room and remote node commands

| Command family | Class | Included behavior |
| --- | --- | --- |
| Local room state | Self-contained | `elastos room show`, `elastos room pending`, `elastos room seed`, `elastos room invite`, `elastos room invite-export`, `elastos room invite-import`, `elastos room accept`, `elastos room accept-export`, `elastos room accept-import`, `elastos room approve`, `elastos room deny`, `elastos room reset` |
| Local room gateway | Operator lane | `elastos room open` requires the room capsule and a live operator runtime. It asks that runtime to start the room gateway. |
| Local node state | Self-contained | `elastos node info`, `elastos node peer add`, `elastos node peer list`, `elastos node peer remove` |
| Remote node reads | Self-contained locally | `elastos node status`, `elastos node room show`, `elastos node room pending`. The target peer needs a reachable operator runtime and the matching allowlisted action. |
| Remote node control | Self-contained locally | `elastos node room approve`, `elastos node room deny`, `elastos node room open`, `elastos node update --check`, `elastos node update --apply --yes`. The target operator runtime enforces its allowlist. |

Command classification does not grant capsule authority. Host-side provider
commands are explicit CLI tools; App and viewer effects still cross Runtime
capability, provider, and audit boundaries.

## Rules

1. Every command must complete, time out, or fail clearly.
2. A self-contained command does not read or create Runtime coordinates unless
   its row says otherwise.
3. `elastos home --status` and `--json` never start Home.
4. A managed user command starts or reuses only the Runtime kind named in its
   row.
5. An operator command fails when the operator Runtime is absent; it does not
   widen or replace a managed policy.
6. Host-side provider bridge commands are explicit operator tooling, not app-capsule authority.
7. `elastos run` is the explicit path for arbitrary path or CID input. Data runs
   locally; executable Component and microVM paths use the operator lane.
8. `elastos` and `elastos serve` are separate lanes for one data home and must
   not own it concurrently.

## Future: changing command ownership

Move a command between classes only with an explicit policy change, matching
tests, and an update to this matrix. Do not widen a managed Runtime merely to
make one command convenient.
