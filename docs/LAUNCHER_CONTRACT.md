# Launcher Contract

A supervisor starts Home with the gateway command below. This repository owns the
gateway, the readiness probe, the passkey origin, and the exit codes. A desktop
launcher is a separate program. This document states the behavior the Runtime
implements today.

## Start

```bash
elastos gateway --addr localhost:8090
```

`--addr` is `host:port`. `elastos home` binds `localhost:8090` and opens the
Home URL below. The gateway command leaves the system browser closed. The
default `--addr` of the gateway command is `127.0.0.1:8090`, so a launcher
passes `--addr localhost:8090`.

For the host name `localhost`, the gateway listens on every loopback address
that a client can use for that name on the given port: each address that the
computer resolves for `localhost`, and also `127.0.0.1` and `::1`, which
browsers use for `localhost`. Each of these listeners serves the same Home.
When the computer has no IPv6, the gateway listens without `::1`. The gateway
exits 1 and its error names the address when another program already uses one
of these addresses, when it cannot listen on one of them for another reason,
or when `localhost` resolves to an address that is not loopback. For an IP
address in `--addr`, the gateway listens on that one address.

On Windows, Home runs in WSL2. This repository does not test how Windows
forwards `localhost` into WSL2; that is a gap in this contract.

`elastos serve` is the operator runtime. Its readiness route is `GET /api/health`.
A Home supervisor uses `gateway` and `/healthz`.

## Readiness and Home URL

When this gateway process holds the host lock `{data}/host-process.lock`
with role `gateway` or `gateway-public`:

```text
GET http://localhost:8090/healthz
```

returns HTTP 200 and the two bytes `OK`. Otherwise it returns HTTP 503: when
no process holds the lock, when another process holds it, when the file at
that path was removed or replaced after the gateway took its lock, and when
the gateway has released it. A gateway that `elastos serve` starts keeps the
`serve` lock, so its `/healthz` returns 503.

What proves ownership: the process that owns the listening socket answers,
and it checks its own state. It holds an exclusive lock (`flock`) on an open
file, `{data}/host-process.lock` names that same file now (the same device
and inode), and a new open of that file cannot take the lock. The JSON record
in the file is not proof. Any process that runs as the same user can rewrite
it, so `/healthz` does not read it, and a gateway does not skip taking the
lock because the record names its pid.

The process that answers on the port gives this reply about its own data
directory. If an ElastOS gateway for another data directory already listens on
the port, that gateway also returns `OK`. The reply does not name the process.
Before it trusts `OK`, a supervisor confirms that the child it started is still
running and that `{data}/host-process.lock` records the pid of that child with
role `gateway`. That record is a hint, not proof, for the reason above. A reply
that names the lock owner is a gap in this contract.

The Home origin is:

```text
http://localhost:8090/home/
```

`localhost` is the one Home origin. Other documents cite this section for the
Home address. When the gateway listens on `localhost`, a `GET` or `HEAD`
request for a Home page (`/`, `/home`, `/home/`, `/apps/home`, or
`/apps/home/`) whose `Host` is a loopback IP address, such as `127.0.0.1` or
`[::1]`, gets HTTP 308. Its `Location` is the same path and query at
`http://localhost:<port>`. `/healthz`, `/api/` routes, Home assets, and app
routes answer on every listening address without a redirect. A gateway with an
IP address in `--addr` serves its Home pages at that address.

The passkey relying party id is `localhost`. Registration and sign-in use that
origin. An IP-literal origin, including `http://127.0.0.1:8090`, is refused
with HTTP 403. For a loopback address, the reply tells the person to open
`http://localhost:<port>/home/`.

Each release takes the relying party id from the host of the page, and browsers
refuse an IP address as a relying party id. A passkey from an earlier release
therefore names the host where it was made. Sign-in accepts that passkey at
the same host on any port, and refuses it at every other host.

## Data directory and lock

| Item | Path |
| --- | --- |
| Linux data directory | `$XDG_DATA_HOME/elastos`, or `~/.local/share/elastos` |
| macOS data directory | `~/Library/Application Support/elastos` |
| Host lock | `{data}/host-process.lock` |

The lock record stores `pid`, `role`, and `addr`. `addr` is the `--addr`
value as given, for example `localhost:8090`. The record is for people and
tools to read; the `flock` on the file is the lock. A second live host for the
same data directory exits 1. Its error contains `another ElastOS host already owns`.

## Stop

The gateway handles `SIGTERM` and `SIGINT` by signalling its process group,
waiting, and then signalling that group again. Exit 75 comes from
`std::process::exit` in the binary watch and skips that handler.

While the gateway is running, a replaced installed binary or a changed binary
hash makes that process exit 75. The install transaction replaces the binary
by renaming the staged file onto the installed path. On Linux that running
path then ends in ` (deleted)`, and the watch exits 75. A supervisor starts
`elastos gateway --addr localhost:8090` again.

A moved path, a path that cannot be read, and a file that could not be
re-verified wait for one more sample. A replacement on that next sample exits
75. A second sample with the same failure exits 1. A different failure on
that next sample, or a move to a different path, waits for one more sample
instead. The message names
`elastos gateway --addr localhost:8090` as the recovery command. A supervisor
leaves that process stopped.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | The command finished. |
| 1 | The command failed. This includes a second host for the same data directory and a binary watch that confirmed a move, an unreadable path, or a failed re-verify. A supervisor leaves the gateway stopped. |
| 2 | The `elastos` command line is invalid. |
| 75 | The running gateway exited because the installed binary was replaced or its hash changed. A supervisor starts the gateway again. |

A supervisor uses these exit codes. It does not parse English sentences from
stdout. Exit 0 covers more than one successful update outcome. A later issue
owns a machine-readable update status.

Intel Mac's release id is `x86_64-darwin`. This contract does not publish a
download for that id.
