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
Home URL below. The gateway command leaves the system browser closed.

`elastos serve` is the operator runtime. Its readiness route is `GET /api/health`.
A Home supervisor uses `gateway` and `/healthz`.

## Readiness and Home URL

When this gateway process is the recorded owner of `{data}/host-process.lock`,
with role `gateway` or `gateway-public` and `pid` equal to its process id:

```text
GET http://localhost:8090/healthz
```

returns HTTP 200 and the two bytes `OK`. A different pid, a missing lock, or a
lock that is not JSON returns HTTP 503. A gateway that `elastos serve` starts
keeps the `serve` record, so its `/healthz` returns 503. The probe does not
treat another process on the port as this Home.

The Home origin is:

```text
http://localhost:8090/home/
```

The passkey relying party id is `localhost`. Registration and sign-in use that
origin. An IP-literal origin, including `http://127.0.0.1:8090`, is refused.

## Data directory and lock

| Item | Path |
| --- | --- |
| Linux data directory | `$XDG_DATA_HOME/elastos`, or `~/.local/share/elastos` |
| macOS data directory | `~/Library/Application Support/elastos` |
| Host lock | `{data}/host-process.lock` |

The lock record stores `pid`, `role`, and `addr`. A second live host for the
same data directory exits 1. Its error contains `another ElastOS host already owns`.

## Stop

The gateway handles `SIGTERM` and `SIGINT` by signalling its process group,
waiting, and then signalling that group again. Exit 75 comes from
`std::process::exit` in the binary watch and skips that handler.

While the gateway is running, the binary watch exits 75 when the installed
binary is replaced, when its path changes, or when the watch cannot read it.
The install transaction replaces that binary by renaming the staged file onto
the installed path. On Linux that running path then ends in ` (deleted)`.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | The command finished. |
| 1 | The command failed. A second host for the same data directory is one case. |
| 2 | The `elastos` command line is invalid. |
| 75 | The running gateway's binary watch exited. |

A supervisor uses these exit codes. It does not parse English sentences from
stdout. Exit 0 covers more than one successful update outcome. A later issue
owns a machine-readable update status.

Intel Mac's release id is `x86_64-darwin`. This contract does not publish a
download for that id.
