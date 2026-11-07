# Capsule init

A person creates a new capsule directory from the CLI and sees `capsule.json` on disk.

## Sub-features

- `init-create` runs `elastos init` in an empty directory.
- `init-manifest` leaves `capsule.json` in the new directory.

## How to get to it (user POV)

- Choose an empty working directory.
- Run `elastos init cloud-hello`.
- Open `cloud-hello/capsule.json`.

## Driving it with verify-elastos

Preconditions:

- `$BIN` is the release binary from `run.env`.
- The working directory is `$STATE/work` from that file, and `cloud-hello` does not already exist there.
- Do not run `elastos init` in the repository root.

- **Create.** Run `cd "$STATE/work"` and then `"$BIN" init cloud-hello`. The exit code is 0 and stdout contains `Created capsule 'cloud-hello'`.
- **Manifest.** `test -f "$STATE/work/cloud-hello/capsule.json"` succeeds.
- **Proof.** Copy `capsule.json` to `artifacts/verify-elastos/<run-id>/capsule.json`. The copy contains the capsule name.

## Gotchas

- `elastos init` writes into the current directory. A run from the repo root drops a capsule into the source tree.
- Init does not need `elastos serve`. Do not treat a running runtime as proof that init worked.
- If `cloud-hello` already exists, the command is not this recipe's empty-directory case. Report that precondition instead of reusing the directory.
