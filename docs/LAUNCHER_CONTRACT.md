# Launcher contract

This is the stable interface a desktop launcher uses to install, start, watch,
stop, and update the ElastOS Runtime. The launcher owns the supervisor. The
Runtime owns the process, the data directory, the host lock, and the signed
update.

The checked behavior for readiness is `test_healthz_200` in
`elastos/crates/elastos-server/src/api/gateway_tests/site_publication.rs`.
The supervised start entry is `run_gateway` in
`elastos/crates/elastos-server/src/gateway_entry.rs`.

## Install

The launcher runs the publisher installer and stops before setup:

```bash
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash -s -- --install-only
```

`ELASTOS_INSTALL_ONLY=1` selects the same stop. The git copy of
`scripts/install.sh` still contains placeholder trust anchors. The launcher
runs the script the publisher URL serves, which carries the stamped anchors.

That command installs the Runtime binary and the trusted source, then returns.
It leaves Home setup and the browser to the launcher.

The launcher then runs:

```bash
elastos setup
```

Setup installs the Home profile. `elastos setup --isolated`, chosen before the
first start, keeps this Home out of the shared Community room.

The signed release head names the channel. Allowed names are `stable`,
`canary`, and `jetson-test`. The stable channel is the publication from
`main`. The installer records that channel in the trusted source. Later
updates follow the saved source. The launcher does not pass a channel flag.

The installer accepts Linux x86_64, Linux aarch64, and macOS Apple silicon
(`aarch64-darwin`). On any other operating system or architecture it exits 1.
Intel Mac is outside this install set. That host has one name pair. The
release platform id is `x86_64-darwin`. The package key is `darwin-amd64`.
`scripts/release-platforms.json` maps `darwin-amd64` to `x86_64-darwin`. The
launcher calls the same host `darwin-x64`. The signer admits `x86_64-linux`,
`aarch64-linux`, and `aarch64-darwin`. It refuses `x86_64-darwin` in
`scripts/release-signer.py` until one signed Intel artifact exists. This tree
has no Intel download. See [Mac notes](MAC.md) for the source-home path on
Apple silicon.

## Start

The launcher starts Home with:

```bash
elastos gateway --addr 127.0.0.1:8090
```

`--addr` (`-a`) is `host:port`. The default is `127.0.0.1:8090`. The launcher
owns the address it passes and uses that same address for every probe and for
the Home URL.

`elastos gateway` leaves the system browser closed. `elastos home --browser`
is a different command: it binds `localhost:8090` and opens the URL with
`open` on macOS, or `wslview` and then `xdg-open` on Linux. The launcher uses
`gateway`.

`elastos serve` is the operator runtime. Its readiness route is
`GET /api/health`. The launcher does not start `serve` against the same data
directory, and it does not use `/api/health` as the Home probe.

`--public` starts a tunnel and is outside this contract.

On startup the gateway prints:

```text
[gateway] ElastOS <version> starting on <addr>
```

That line is progress. Readiness is the HTTP probe below.

## Readiness and Home URL

When the gateway is listening:

```text
GET http://127.0.0.1:8090/healthz
```

returns HTTP 200 and the body bytes `OK` (three bytes, no trailing newline).
`test_healthz_200` locks that response.

The launcher opens:

```text
http://127.0.0.1:8090/home/
```

`/home/` is `HOME_ROUTE`. `/home` and `/apps/home/` redirect to it. The page
is the Home capsule installed by `elastos setup`. The launcher opens this URL
in its own browser after `/healthz` returns `OK`.

A source-home staging script may bind another port. The launcher uses the
address it passed to `--addr`.

## Data directory and binary

| Item | Path |
| --- | --- |
| Binary | `~/.local/bin/elastos`, or the directory passed as `--install-dir` |
| Linux data directory | `$XDG_DATA_HOME/elastos`, or `~/.local/share/elastos` |
| macOS data directory | `~/Library/Application Support/elastos` |
| Trusted source | `{data}/sources.json` |
| Installed components | `{data}/components.json` |
| Host lock | `{data}/host-process.lock` |

The Runtime resolves the data directory with the platform data directory
(`dirs::data_dir()`), then `elastos`. The installer uses the same split:
`~/Library/Application Support/elastos` on macOS, and
`$XDG_DATA_HOME/elastos` or `~/.local/share/elastos` on Linux.

The launcher keeps one data directory for the Home it supervises. It leaves
any other product directory in place.

Coordinate files under the data directory (`runtime-coords.json`,
`gateway-runtime-coords.json`, `home-runtime-coords.json`) hold a local
`attach_secret`. The launcher log is the gateway process stdout and stderr.
The launcher leaves those coordinate files out of the log.

`elastos version` prints `ElastOS Runtime v<version>`.

## Single instance

The gateway takes an exclusive lock on `{data}/host-process.lock` and writes
the pid, the role `gateway`, and the bind address. A second live host for the
same data directory exits 1. Its error contains:

```text
another ElastOS host already owns
```

The launcher shows that error. It leaves the process that holds the lock
running.

## Stop

The launcher sends `SIGTERM` to the gateway process.

`elastos gateway` puts itself in its own process group. On `SIGTERM` or
`SIGINT` the Runtime prints `Shutting down...`, signals that group with
`SIGTERM`, waits 15 seconds, then signals the group with `SIGKILL`.

The launcher waits until `GET /healthz` fails. A stop that begins with
`SIGKILL` to the parent skips the handler, so child processes can remain.
Exit status after a requested stop is the signal status. The launcher treats
a dead `/healthz` as stopped.

While the gateway is running, a replaced installed binary makes that process
exit 75. The same exit covers a moved executable path and a file that changed
and then could not be verified. The launcher starts
`elastos gateway --addr 127.0.0.1:8090` again so the new binary takes the lock.
A process that cannot read its own executable path exits 1. The launcher
leaves that process stopped.

## Update and rollback

```bash
elastos update --check
elastos update --yes
elastos update --rollback-to <previous head CID> --yes
```

There is no `elastos upgrade` command. `--check` and a completed install both
exit 0. When the saved release is current, stdout contains
`Installed release is up to date.` When a newer signed release exists,
stdout contains `Run `elastos update` to install.` The launcher reads that
text. The exit code stays 0 in both cases.

`--yes` skips the confirmation prompt. The launcher passes `--yes` on every
install and rollback. A declined prompt also exits 0 and prints
`Update cancelled.`

`--rollback-to` selects that signed head and admits an older release. The
launcher saves the current head CID from `elastos source show` (`Head CID:`)
before an update, and passes that CID to `--rollback-to`. A plain update of
an older release exits 1 and tells the operator to use the rollback command.

`--force` repairs an invalid installed version. It is a separate flag from
rollback.

Update and rollback use the trusted source over Carrier. They replace the
installed binary and `components.json` after signature and hash checks.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | The command finished. This includes `update --check` with or without a newer release, a declined update prompt, and `install.sh --help`. |
| 1 | The command failed. This includes the installer `die` path (unknown option, unsupported platform, failed check), a held host lock, a missing trusted source, a failed signature check, an older release without `--rollback-to`, and a running gateway whose executable path cannot be read. |
| 2 | The `elastos` command line is invalid. Clap reports a usage error with exit 2. |
| 75 | The running gateway exited because the installed binary was replaced, its path moved, or the file on disk could not be verified after a change. The launcher starts the gateway again. |

`install.sh` reports a failed check with exit 1.
