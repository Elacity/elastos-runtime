---
name: verify-localhost-authority
description: "Prove a client attach session cannot write or read localhost Public files without a capability, while a shell session can. Use when a change touches attach, auth middleware, or localhost storage checks."
disable-model-invocation: true
---

# Localhost write authority

A client session must not write or read a `localhost://Public` file unless it presents a capability token. A shell session may write that file with no capability token. A wrong attach secret creates no session. The harm is a client, or a caller with no session, storing or reading a person's Public file.

This check runs the release binary. No Kani or CBMC toolchain is installed for Rust 1.91 in this repo, so the check does not use a hand model. It rebuilds `elastos-server` and then executes every combination of the branch inputs the code actually switches on.

## When a change touches this

Run the check when a diff touches any of these:

- `elastos/crates/elastos-server/src/api/handlers/attach.rs` function `attach`
- `elastos/crates/elastos-server/src/api/middleware.rs` functions `auth_middleware` and `extract_bearer_token`
- `elastos/crates/elastos-server/src/api/handlers/storage.rs` functions `write_file`, `read_file`, and `enforce_capability`
- `elastos/crates/elastos-runtime/src/session/mod.rs` function `is_shell`

The code was last confirmed against commit `324bc2bc2dc3543c680d4be5469567956e1c9c94`. Move that commit forward only after you read those functions at the new commit and the check still describes them. If those files changed after that commit, treat a pass from the old commit as stale.

## Run

From the repo root:

```bash
chmod +x .cursor/skills/verify-localhost-authority/scripts/check.sh
.cursor/skills/verify-localhost-authority/scripts/check.sh
```

The command needs `elastos/target/release/localhost-provider` from `just build`. It rebuilds the `elastos` binary, starts one isolated runtime, writes `artifacts/verify-localhost-authority/<run-id>/matrix.txt`, and stops that runtime. A line that starts with `FAIL` is a miss. Exit code 0 means every row matched.

## How to read a miss

Read the `FAIL` line. The name is the step, in the words of the code:

- `attach-wrong-*`, `attach-empty-*`, and `attach-suffix-*` must be 403 `invalid attach secret`. `attach` compares the secret with `!=` and returns before `create_session`.
- `attach-correct-shell` must be session type `shell`. Any other scope, including `operator`, must be session type `capsule`.
- `write-no-auth` and `write-empty-bearer` must be 401 from `auth_middleware` when the bearer token is absent.
- `write-bad-bearer` must be 401 `Invalid or expired session token`.
- `write-client`, `write-operator`, and `read-client` must be 403 `missing X-Capability-Token`. `enforce_capability` returns that error when `is_shell` is false and the header is absent.
- `write-client-bad-capability` must be 403 `invalid token`.
- `write-shell` must be 200, `read-shell` must return `Release checklist`, and `disk-public-file` must find those bytes at `Public/verify-note.txt`.

A `FAIL` on `write-client` or `read-client` with status 200 means the capability check no longer stops a client. That is a product bug, not a modeling error. A `FAIL` that says the runtime never became healthy, or `doctor` would reject the pid, is a harness failure. Fix the harness, not the expected status.

Do not drop a row to make the check pass.

## What the check covers

The bound is the finite control matrix:

- secrets: the real secret, empty, `not-the-secret`, and the real secret plus one trailing `x`
- scopes: `shell`, `client`, and `operator`
- writes and reads: no Authorization header, an empty bearer, an unknown bearer, a client token, an operator token, a client token with `X-Capability-Token: not-a-capability`, and a shell token

It does not build a valid capability token, and it does not check token cryptography, checksum verification of provider binaries, or Home passkey sign-in. A pass is this matrix on this binary, not the whole runtime.

## Keep the check honest

After an intended change to the covered functions, read the new code, run the check, and update the sync commit in this file only when the rows still describe that code. Point a later drift repair at `/maintain-formal-verification-skill`.
