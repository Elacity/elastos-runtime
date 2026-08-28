# Local Home setup from a source checkout

Use this guide when you want to run Home from this source tree on one local
machine. For the published install path, use [INSTALL.md](INSTALL.md) and
[GETTING_STARTED.md](GETTING_STARTED.md). For Mac-specific staging details, use
[MAC.md](MAC.md).

## Scope

This guide covers one local source-home owner and one local browser Home.

It does not cover:

- the published installer path
- deployment or public hosting
- Browser VM acceptance proof
- manual partial setup that skips verified components or `components.json`

## Rules

1. Use the full source-home path: `scripts/setup-source-home.sh`.
2. Use one data root with one live host owner at a time.
3. Do not run `elastos gateway` and `elastos serve` against the same data root
   at the same time.
4. For local HTTP passkey use, the visible browser URL must use
   `http://localhost:PORT/...`.
5. Do not use `127.0.0.1`, a LAN IP, or a custom HTTP hostname for local
   passkey setup.
6. For a public or custom-domain Home, use its exact HTTPS origin. ElastOS does
   not require `localhost` globally.

## Choose your local values

Set these values before you start:

```bash
export GATEWAY_ADDR="localhost:61180"
export HOME_URL="http://localhost:61180/apps/home/"
```

`GATEWAY_ADDR` is the local listen address for `elastos gateway`.

`HOME_URL` is the browser URL you open.

You may choose a different loopback port if it is free. The Mac source-home
examples in this repository use `localhost:61180`. The agent smoke script
defaults to `localhost:8090`; that is only a script default, not product
authority.

## Know the two host lanes

Use `elastos gateway` when you want browser Home at `/apps/home/`.

Use `elastos serve` only for the separate operator lane.

`elastos serve` is not the browser Home front door. If you open `/apps/home/`
through that lane without the required token, the request fails.

See [COMMAND_MATRIX.md](COMMAND_MATRIX.md) for the full ownership rules.

## Preferred path: full source-home setup

`scripts/setup-source-home.sh` is the canonical setup path for a source
checkout. It builds the Runtime and providers, stages first-party capsules, and
stamps the installed `components.json`.

It does not complete readiness by itself. Current source says:

- artifacts install ends before readiness
- the platform restart path performs the offline principal-root readiness step
- direct gateway startup stays fail-closed when principal-root migration is not
  complete

For a local source-home without a signed collaboration startup profile, set:

```bash
export ELASTOS_COLLABORATION_STARTUP_MODE=isolated
```

### macOS source-home example

Use this pattern on Mac. For the full Mac runbook, Browser VM artifacts, and
restart helpers, read [MAC.md](MAC.md).

```bash
cd /path/to/elastos-runtime

export USER_HOME="$HOME"
export LOCAL_HOME_ROOT="$HOME/elastos-local-home"
export GATEWAY_ADDR="localhost:61180"
export HOME_URL="http://localhost:61180/apps/home/"

HOME="$LOCAL_HOME_ROOT" \
CARGO_HOME="$USER_HOME/.cargo" \
RUSTUP_HOME="$USER_HOME/.rustup" \
PATH="$USER_HOME/.cargo/bin:/opt/homebrew/bin:$PATH" \
ELASTOS_COLLABORATION_STARTUP_MODE=isolated \
scripts/setup-source-home.sh
```

Then use the platform restart helper:

```bash
scripts/mac-source-home-restart.sh \
  --test-home "$LOCAL_HOME_ROOT" \
  --addr "$GATEWAY_ADDR"
```

Then open:

```bash
open "$HOME_URL"
```

### Linux source-home example

Use this pattern on Linux:

```bash
cd /path/to/elastos-runtime

export LOCAL_HOME_ROOT="$HOME/elastos-local-home"
export XDG_DATA_HOME="$LOCAL_HOME_ROOT/xdg-data"
export GATEWAY_ADDR="localhost:61180"
export HOME_URL="http://localhost:61180/apps/home/"
export ELASTOS_COLLABORATION_STARTUP_MODE=isolated

scripts/setup-source-home.sh
scripts/linux-source-home-restart.sh \
  --home "$LOCAL_HOME_ROOT" \
  --xdg-data-home "$XDG_DATA_HOME" \
  --addr "$GATEWAY_ADDR"
```

Then open:

```bash
xdg-open "$HOME_URL"
```

## Direct source launch

If you need a direct source launch for diagnosis, use the built Runtime binary
from this checkout with the same source-home environment. Keep the platform
restart helpers above as the preferred readiness path.

On macOS:

```bash
HOME="$LOCAL_HOME_ROOT" \
./elastos/target/release/elastos gateway --addr "$GATEWAY_ADDR"
```

On Linux:

```bash
HOME="$LOCAL_HOME_ROOT" \
XDG_DATA_HOME="$XDG_DATA_HOME" \
./elastos/target/release/elastos gateway --addr "$GATEWAY_ADDR"
```

Use this only when you intentionally want the direct path. It does not replace
the platform restart helper and it does not perform the offline principal-root
readiness step.

## Sign in or create the first passkey

When Home first opens, create the first admin passkey.

For local HTTP development:

- use `http://localhost:PORT/apps/home/`
- keep `localhost` visible in the browser address bar
- do not swap the visible origin to `127.0.0.1`

For a public or custom-domain Home:

- use its exact `https://...` origin
- keep that exact HTTPS origin consistent for passkey use

## Agent or CI smoke path

The passkey virtual-auth smoke is an agent or test path. It is not the normal
user path.

Use the existing script only if your local test environment already has its
required runner available:

```bash
ELASTOS_BASE_URL="http://localhost:8090" \
HOME_URL="http://localhost:8090/apps/home/" \
HOME_VIRTUAL_AUTH_NAME="Local Source Home" \
node scripts/home-passkey-virtual-auth-smoke.mjs
```

This script requires a `localhost` Home origin for local HTTP passkey tests.
See [scripts/home-passkey-virtual-auth-smoke.mjs](../scripts/home-passkey-virtual-auth-smoke.mjs)
for its full environment surface.

## If something is wrong

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| Passkey creation fails on local HTTP | Visible origin is not `localhost` | Open `HOME_URL` with `http://localhost:PORT/...` |
| `/apps/home/` does not behave like browser Home | You started `elastos serve` instead of `elastos gateway` | Start `elastos gateway` for Home |
| `setup-source-home.sh` fails before staging collaboration | `ELASTOS_COLLABORATION_STARTUP_MODE` is missing | Set `ELASTOS_COLLABORATION_STARTUP_MODE=isolated` for local source-home setup |
| Home still is not ready after setup | `setup-source-home.sh` completed, but readiness step did not run | Run the platform source-home restart helper |
| One host refuses to start | Another host already owns the same data root | Stop the other host or use a different source-home root |

## Read next

- [GETTING_STARTED.md](GETTING_STARTED.md)
- [INSTALL.md](INSTALL.md)
- [MAC.md](MAC.md)
- [COMMAND_MATRIX.md](COMMAND_MATRIX.md)
