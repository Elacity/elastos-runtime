# ElastOS verification map

This directory is the maintained source for verifying operator and CLI behavior of ElastOS. Read the index before driving the app, then use the matching feature file as the recipe.

## Baseline preconditions

- Build `elastos/target/release/elastos` and `elastos/target/release/localhost-provider` with `just build`.
- Launch with `.cursor/skills/verify-elastos/scripts/launch.sh` so the run has its own data home and port.
- Run `doctor.sh` on that `run.env` and require the recorded pid, port, and version.
- Never drive a runtime this verification run did not start.
- Set `XDG_DATA_HOME` to the run's `xdg` directory for every `elastos` CLI command. An unset value uses the operator's own Home.

## Driving conventions

- Start every recipe from the baseline state unless its preconditions say otherwise.
- Treat every command as literal. Keep quoted names and flags unchanged.
- HTTP actions use the paths in the feature file and the token from `POST /api/auth/attach`.
- CLI actions use `$BIN` from `run.env` with the run's `XDG_DATA_HOME`.
- Record the feature ID and entry point with every artifact.
- Report an unreachable path with the attempted command and the unmet precondition.
- Do not report a skipped entry point as verified through a different path.

## Proof and skip reporting

- Capture the user action and the resulting state, not only the final response.
- HTTP proof includes the status and a second read of the same resource.
- CLI proof includes the command, stdout, stderr, and exit code.
- Mutation proof includes the bytes on disk or a second CLI read.
- Leave proof under `artifacts/verify-elastos/<run-id>/`. Cleanup does not remove it.

## Feature entry contract

Each feature file starts with an H1 title and one paragraph. It then uses exactly four H2 sections in this order.

1. `Sub-features`
2. `How to get to it (user POV)`
3. `Driving it with verify-elastos`
4. `Gotchas`

## Features

- [Localhost file](./localhost-file.md) covers shell save, client refusal, and a second read of `localhost://Public/verify-note.txt`.
- [Operator attach](./operator-attach.md) covers health, a matching attach secret, and a rejected secret.
- [Profile nickname](./profile-nickname.md) covers setting a nickname and reading it back from the CLI.
- [Capsule init](./capsule-init.md) covers creating a capsule directory from `elastos init`.
