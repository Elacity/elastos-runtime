# Scripts

The `scripts/` tree is organized around one rule:

- the `scripts/` root contains the commands a developer or operator should type directly
- subdirectories contain lower-level support tooling

## Root Entry Points

Canonical user-facing entrypoints stay at the root:

- `agent.sh` — run the agent capsule
- `build.sh` — build runtime and capsules
- `chat.sh` — launch the chat demo
- `dev-sync-jetson.sh` — fast host-to-Jetson sync loop
- `gba.sh` — launch the GBA demo
- `install.sh` — signed installer
- `notepad.sh` — launch the notepad demo
- `pc2-demo-local.sh` — prepare and launch the local source-based PC2 demo in a clean temp home
- `public-gateway.sh` — launch the public gateway flow
- `publish-release.sh` — low-level release publisher
- `release-demo-gate.sh` — release acceptance helper
- `resolve-binary.sh` — shared binary resolver sourced by root launchers
- `setup-crosvm.sh` — install runtime VM prerequisites
- `share-demo.sh` — share project docs/content
- `status.sh` — inspect local build/runtime state

If a script is something a human is expected to type from docs, it belongs here.

## Support Subdirectories

- `build/` — lower-level build helpers
  - `build-rootfs.sh`
  - `build-vm-smoke-rootfs.sh`
  - `build-llama-server.sh`
  - `clean.sh`
- `fetch/` — asset/tool fetchers
  - `fetch-cloudflared.sh`
  - `fetch-model.sh`
- `ops/` — specialized operations and diagnostics
  - `jetson-gateway.sh`
  - `jetson-vm-diagnose.sh`
  - `host-doctor.sh`
  - `host-refresh.sh`
  - `host-reset.sh`
- `system/` — unit/service assets
  - `elastos-gateway.service`
  - `elastos-runtime.service`
  - `elastos-agent-codex.service`
  - runtime/agent wrapper scripts and env/policy examples

These scripts are intentionally less prominent. They support the canonical flows but are not the default starting point for new contributors.

## Design Rules

- One canonical path per operation.
- Root scripts should be obvious, stable entrypoints.
- Root repo launchers use the repo binary by default.
- Installed runtime mode must be explicit (`--installed`) where supported.
- Support scripts should be grouped by job, not historical accident.
- If a script is internal, its path should make that obvious.
