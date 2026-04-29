# Scripts

The `scripts/` tree is organized around one rule:

- the `scripts/` root contains top-level commands a developer or operator may run directly
- subdirectories contain lower-level support tooling

Not every root script is a stable end-user entrypoint. Some root scripts are
explicit proof, smoke, audit, or release helpers and should be documented as
such.

## Root Entry Points

Top-level directly-invoked entrypoints stay at the root:

- `agent.sh` — run the agent capsule
- `build.sh` — build runtime and capsules
- `chat.sh` — launch the chat demo
- `gba.sh` — launch the GBA demo
- `install.sh` — signed installer
- `notepad.sh` — launch the notepad demo
- `home-demo-local.sh` — prepare and launch the local source-based Home demo in a clean temp home
- `publish-release.sh` — low-level release publisher
- `resolve-binary.sh` — shared binary resolver sourced by root launchers
- `setup-crosvm.sh` — install runtime VM prerequisites
- `share-demo.sh` — share project docs/content

If a script is something a human is expected to type from docs, it belongs here.

## Root Proof Helpers

Proof, smoke, and audit helpers also currently live at the root. Common examples:

- `command-smoke.sh`
- `installed-command-audit.sh`
- `local-carrier-chat-smoke.sh`
- `local-carrier-setup-smoke.sh`
- `home-frontdoor-smoke.sh`
- `system-camofox-smoke.sh`
- `chat-room-gateway-camofox-smoke.sh`
- `chat-room-session-reuse-camofox-smoke.sh`
- `chat-room-guest-identity-camofox-smoke.sh`
- `chat-room-runtime-activity-smoke.sh`
- `public-install-identity-smoke.sh`
- `public-install-operator-smoke.sh`
- `public-install-home-frontdoor-smoke.sh`

These are review and release helpers, not automatically part of the stable
end-user command contract. The `public-install-*.sh` helpers can target a
published candidate gateway by setting `ELASTOS_PUBLISHER_GATEWAY=<url>`.

## Support Subdirectories

- `build/` — lower-level build helpers
  - `build-rootfs.sh`
  - `build-vm-smoke-rootfs.sh`
  - `build-llama-server.sh`
  - `clean.sh`
- `fetch/` — asset/tool fetchers
  - `fetch-cloudflared.sh`
  - `fetch-model.sh`
- `lib/` — shared shell helpers sourced by top-level proof and release scripts
  - `runtime-cleanup.sh`

Any deeper deployment or helper assets should stay out of the public root-script story unless they are part of the shipped public contract.

## Design Rules

- One canonical path per operation.
- Root scripts should be obvious, stable entrypoints.
- Root repo launchers use the repo binary by default.
- Installed runtime mode must be explicit (`--installed`) where supported.
- Support scripts should be grouped by job, not historical accident.
- If a script is internal, its path should make that obvious.
