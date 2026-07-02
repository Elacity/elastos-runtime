# State

Last updated: 2026-06-29 UTC

This file records public-safe current truth for the 0.5.0 candidate. Historical
local proof logs, private SSH aliases, tunnel ports, operator usernames, key
paths, worktree paths, and target backup paths are intentionally not tracked in
the public repository.

## Release Posture

- 0.5.0 is a review candidate, not a release tag.
- Source/review proof must cite concrete reusable commands: `git diff --check`,
  `node scripts/home-entropy-check.mjs`, `node scripts/browser-entropy-check.mjs`,
  `bash scripts/check-wci-alignment.sh`, `just candidate-command-audit`, and the
  touched-surface Rust/capsule tests.
- Target proof is operator-supplied. It must name target roles and exact command
  lines without committing private host aliases, SSH keys, tunnel ports, local
  data roots, or local worktree paths.
- Private proof logs stay outside this repo. Public release notes should claim
  only proof that can be reproduced from the reviewed source or from
  explicitly supplied target evidence.

## Browser Truth

- Browser architecture is coherent enough to preserve.
- The Browser objective still fails product audio proof and hash-bound manual UX evidence.
- Docker/Selkies is only `managed_baseline_not_final_product`.
- The hosted Selkies/GStreamer service is a managed baseline, not accepted as the final Browser.
- The current hosted baseline is single-session; active pages are a serialization blocker.
- This server is not a product native-browser proof target because it lacks a real host compositor/display, host audio service, and working network namespace support.
- Kasm Workspaces, BrowserBox, or KasmVNC cannot replace Selkies until the
  operator_control_socket not provisioned blocker is cleared and their
  operator prerequisites plus product-compositor evidence pass.
- Browser profile state is principal-owned and reset-scoped, but it still lacks
  protected/recoverable Browser profile storage.
- Browser VM Chromium profile disks are principal-owned and reset-scoped, but they are not protected principal-root envelopes or Recovery Kit-packaged state yet.
- Browser profile receipts must continue to report
  `storage_posture=principal_owned_reset_scoped_unprotected`.
- Principal-root object protection exists for selected Home/runtime state; this does not include Browser VM Chromium profile disks yet.

## Browser Provider Evidence

- `scripts/browser-provider-decision-report.mjs` summarizes supplied `hosted_bakeoff` and `native_preflight` artifacts and keeps generated placeholder configs out of operator instructions.
- `scripts/browser-provider-runbook.mjs` is read-only guidance. Its operator guidance is generated from the actual evidence and should not be treated as a deployment action.
- Current Browser runbooks must keep the stop condition visible: do not keep
  tuning the running Selkies baseline as product architecture.

## Mac VM Proof Boundaries

- The Mac VM acceptance chain recomputes the receipt SHA-256 from the receipt path and rejects auth setup receipts generated after the machine proof.
- The virtual-auth Browser setup path must drive the virtual-auth Browser open viewport by default.
- Profile reset proof must preserve `removed_profile_disk=true`.
- The virtual-auth credential store remains an owner-only local file.
- The handoff exits non-zero until the headed auth setup receipt is bound.
- `scripts/mac-source-home-restart.sh` remains the source-home restart/proof
  helper for macOS target evidence.

## Remote Carrier Exit Evidence

- Operator evidence must reject local redacted artifact hash mismatches.
- Operator evidence must reject local redacted artifacts that still contain private route material.
- Operator evidence must reject stale or route-mismatched hash-bound route-readiness reports.
- Operator evidence must reject stale local installed artifact readiness reports.
- Operator evidence must reject missing route principals.
- Operator evidence must reject local Browser machine-proof artifacts that do not cite the reviewed route target or target host.
- Operator evidence must reject weak evidence that does not cite the reviewed source/exit runtime DIDs and endpoints.
- Operator evidence must reject weak evidence that does not cite the reviewed principal/grant/target/Carrier stream/cleanup route nouns.
- Remote Carrier Exit readiness must remain hash-bound remote route readiness.
- Public-live update planning must stage candidate binaries in a server-side candidate directory before explicit install approval.

## Public Install Truth

- Public-install branch-binary smokes must pin the installer-selected components manifest.
- Public-install branch-binary smokes prevent source checkout `components.json` from leaking into installed-path proof.
- Public-install branch-binary smokes fail if the selected gateway lacks the current `home` setup profile.
- Branch-override public smokes require a staged or published 0.5.0-compatible
  manifest with the current `home` profile and checksummed artifacts.
- Source/local Carrier setup proof stays in `scripts/local-carrier-setup-smoke.sh`.
- source/local Carrier setup proof stays in `scripts/local-carrier-setup-smoke.sh`.
- Public install proof must require a staged or published 0.5.0-compatible manifest with the current `home` profile and checksummed artifacts.
- Set `ELASTOS_PUBLIC_INSTALL_FORCE_RELAY_ONLY=1` only when the publisher relay
  path itself is under review.
- Final public installed-path proof waits for publishing the 0.5.0
  binary/artifact set.

## Canonical Journey Proofs

These reusable proof paths carried over from the flint baseline and remain the
named acceptance commands for their journeys (the alignment gate pins them here
so the ledger and the proofs cannot drift apart):

- `scripts/local-identity-profile-smoke.sh` / `scripts/public-install-identity-smoke.sh`
  — DID/profile proof path (source-local and installed-path).
- `scripts/public-install-operator-smoke.sh` — installed-path operator-node
  status/update acceptance path.
- `scripts/audit-linux-runtime-portability.sh` — explicit public Linux runtime
  portability proof (with `just verify-release` as the release-trust gate).
- `scripts/protected-content-provider-contract-smoke.sh` — protected-content
  provider boundary proof over the real DRM/rights/key/decrypt JSON line
  protocols.

## Open Blockers

- Product Browser completion is not claimed.
- Manual installed-device checks on Mac and Linux/aarch64 targets are still
  required before release handoff.
- People remains a Home-owned window/target and must become its own capsule/app
  in a later slice.
