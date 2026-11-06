---
name: verify-elastos
description: "Drive a source-built ElastOS operator runtime the way an operator does: isolated data home, shell attach, and localhost file save. Use when proving runtime, profile, or capsule-init behavior in this repo."
disable-model-invocation: true
---

# Verify ElastOS

The primary surface for this skill is the source-built operator runtime and the `elastos` CLI. A person reaches files through a shell attach session and the `localhost://Public` root. Profile nickname and capsule init are CLI entry points on an isolated data home. Home in the browser is a separate product path. This skill does not open it and does not claim a passkey sign-in.

Two runs can proceed together. Each run has its own data home, port, and pid. One data home has one live host. Do not point a second `elastos` command that owns a runtime at a home this skill is serving.

## Launch

From the repo root, with `elastos/target/release/elastos` and `elastos/target/release/localhost-provider` already built (`just build`):

```bash
chmod +x .cursor/skills/verify-elastos/scripts/*.sh
ENV="$(.cursor/skills/verify-elastos/scripts/launch.sh)"
```

`launch.sh` creates `$HOME/.local/share/elastos-verify-runs/<run-id>`, stages `localhost-provider` with a `sha256:` checksum in that home's `components.json`, and starts `elastos serve` on a free loopback port. Ready means `GET /api/health` returns `{"status":"ok",...}` within 30 seconds. The script prints the `run.env` path.

## Doctor

Run this before a drive whenever the runtime looks wrong:

```bash
.cursor/skills/verify-elastos/scripts/doctor.sh "$ENV"
```

Doctor passes only when the recorded pid is alive, its command is this run's `elastos serve --addr 127.0.0.1:<port>`, that pid owns the port, and the health version matches `elastos --version`.

## Drive

Recipes live in `features/`. Start from the baseline in `features/README.md`. The harness is the scripts in `scripts/`. Stable handles are HTTP paths and CLI subcommands, not screen coordinates.

The localhost file recipe is:

```bash
.cursor/skills/verify-elastos/scripts/drive-localhost-file.sh "$ENV"
```

A client attach session that writes `Public/verify-note.txt` receives 403. A write with no Authorization header receives 401. A shell attach session receives 200, and a following GET returns the same bytes. The bytes are also on disk at `<data>/Public/verify-note.txt`.

## Evidence

Write proof under `artifacts/verify-elastos/<run-id>/`. Keep the command result and the stored bytes together. `localhost-file.txt` records the status codes, the write response, the read body, the disk path, and whether the disk bytes match. `serve.log` is the runtime log. `run.env` records pid, port, and paths. Do not copy `runtime-coords.json` or session tokens into the evidence.

A pass exercises the real attach and storage routes. It does not call `/api/test/create-session`.

## Cleanup

```bash
.cursor/skills/verify-elastos/scripts/cleanup.sh "$ENV"
```

Cleanup reads `run.env`, and kills that pid only when `/proc/<pid>/cmdline` is still this run's serve command. It also stops that pid's direct children, then deletes the run's data home. It leaves `artifacts/verify-elastos/<run-id>/` in place.

## Helpers

- `.cursor/skills/verify-elastos/scripts/launch.sh [run-id]`
- `.cursor/skills/verify-elastos/scripts/doctor.sh <run.env>`
- `.cursor/skills/verify-elastos/scripts/drive-localhost-file.sh <run.env>`
- `.cursor/skills/verify-elastos/scripts/cleanup.sh <run.env>`
