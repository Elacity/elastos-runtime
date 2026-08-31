# Scripts

The `scripts/` root contains commands that developers or operators invoke
directly. Subdirectories contain implementation helpers. A root script is not
automatically a stable end-user command.

## Main entry points

- `build.sh` builds Runtime and capsules.
- `install.sh` runs the signed installer.
- `setup-source-home.sh` builds and provisions a source Home. Full mode
  installs one stable Runtime under the platform data root and writes the
  versioned source-home installation receipt.
- `home-demo-local.sh` and `chat-demo-local.sh` start disposable local demos.
- `share-demo.sh` runs the focused sharing demo.
- `setup-crosvm.sh` installs VM prerequisites.
- `publish-release.sh` is the low-level release publisher.
- `vendor-walletconnect-adapter.sh` refreshes the pinned WalletConnect asset.

Use the `justfile` for repository gates:

```bash
just verify
just verify-release
```

`just verify` is the source gate. It runs documentation and product alignment,
versioning, WIT and template checks, Home and Browser entropy checks, command
audits, formatting, Clippy, Runtime workspace tests, own-workspace capsule
tests, and the separate Browser local-exit checks. `just verify-release` adds
browser-based UI source checks, local Carrier setup and Home front-door proofs.
Publishing trust and signer verification are separate release gates.

## Public-install proof

The three public-install wrappers cover separate installed paths:

- `public-install-identity-smoke.sh`: identity and profile
- `public-install-home-frontdoor-smoke.sh`: setup and Home
- `public-install-operator-smoke.sh`: installed operator and update commands

Set `ELASTOS_PUBLISHER_GATEWAY=<url>` to test a published candidate.

During candidate review, the identity and Home wrappers accept
`ELASTOS_BIN_OVERRIDE=<path-to-branch-elastos>` only when the gateway serves a
compatible manifest with the current `home` setup profile and checksummed
artifacts. They pin the installer-selected components manifest so source
checkout metadata cannot leak into installed-path proof. The operator wrapper
always uses installed binaries and does not accept the override.

Before a candidate gateway exists, use
`scripts/local-carrier-setup-smoke.sh`. Set
`ELASTOS_PUBLIC_INSTALL_FORCE_RELAY_ONLY=1` only for a stricter publisher
relay-health check.

The full release order and manual target pass are in the
[source integration and release checklist](../docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md).

## Focused proof

Use the runbook that owns the surface:

- [Browser capsule](../docs/BROWSER_CAPSULE.md)
- [Browser VM target](../docs/BROWSER_VM_TARGET.md)
- [Inspector testing](../docs/INSPECTOR_TESTING.md)
- [People and conversations](../docs/PEOPLE_CONVERSATIONS.md)
- [Protected content](../docs/PROTECTED_CONTENT.md)
- [Capsule authoring](../docs/CAPSULE_AUTHORING.md)

Common branch gates include:

- `auth-wallet-focus-smoke.sh` for passkey, Recovery Kit, Wallet, chain, and
  principal-bound launch checks
- `wallet-product-safety-smoke.sh` for product Wallet release safety
- `wallet-connector-transaction-smoke.mjs` for fake-DOM, fake-provider
  connector handoff source proof, not hosted Browser acceptance
- `protected-content-provider-contract-smoke.sh` as the fail-closed retirement
  guard for the provisional rights, key, decrypt, and DRM providers; it does not
  verify the canonical v1 custody path
- `people-conversations-local-smoke.sh` for profile, discovery, contacts, and
  Chat handoff
- `capsule-inspector-act-check.sh` for Inspector scope and Inbox approval
- `installed-provider-verify.sh` for an installed provider manifest and binary
- `source-home-capsule-inventory-smoke.py` for source-home capsule finalization
- `protected-content-installed-static-audit.py` for a bounded read-only audit
  of source identity, installed artifacts, private provider declarations, and
  operator prerequisites; `ready_for_active_proof` is not product readiness

`public-copy-entropy-check.mjs` checks selected public manifests, static HTML,
accessibility labels, and Home CLI command copy for Home, People, Spaces,
Services, and System. `check-wci-alignment.sh` owns canonical architecture terms
and retired product terms. Both run directly; this checkout has no separate
`terminology-lint` recipe.

## Browser capacity proof

The [Browser capsule](../docs/BROWSER_CAPSULE.md) and
[Browser VM target](../docs/BROWSER_VM_TARGET.md) own the contract. These two
proof modes are easy to confuse:

```bash
# Read the current capacity receipt without opening an engine session
HOME_VIRTUAL_AUTH_BROWSER_OPEN=0 HOME_VIRTUAL_AUTH_BROWSER_SUMMARY=1 \
  node scripts/home-passkey-virtual-auth-smoke.mjs

# Open a page and test active capacity
scripts/browser-session-capacity-smoke.sh
```

The active smoke opens a page, holds heartbeats for 30 seconds, confirms that an
extra open fails with `browser_capacity_unavailable`, closes the page, and
checks that capacity returns to its starting value. Override concurrency only
when the provider truthfully supports more active pages.

Hosted Browser service files under `scripts/system/` are proof and operator
packaging. They are not the product Browser path. Private SSH aliases, users,
ports, and data roots belong in local operator notes.

## Live and recovery helpers

`mac-source-home-restart.sh` and `linux-source-home-restart.sh` restart only the
stable source-home Runtime after they validate the installation receipt,
exact prior process, bounded rollback, and served artifact parity. Mac default
mode uses the existing installation; `--init` also requires current clean
source and artifact parity. Linux writes
`receipts/linux-source-home-restart.json` under the stable data root and has no
receipt-output argument.

`recovery-kit-live-smoke.sh` requires a signed Home or System session through
`ELASTOS_HOME_TOKEN`, a Cookie header, or a cookie jar. Export also requires a
fresh request-bound passkey token in
`ELASTOS_FRESH_PASSKEY_HOME_TOKEN`. Import into the same root is opt-in through
`ELASTOS_RECOVERY_KIT_IMPORT=1`.

## Subdirectories

- `build/`: build and staging helpers
- `fetch/`: asset and tool fetchers
- `fixtures/`: test-only proof fixtures
- `lib/`: shared shell and JavaScript helpers
- `system/`: installable operator-service files
- `dev/`: local development helpers outside the public command contract

## Placement rules

- Keep one canonical path per operation.
- Put directly invoked, reusable commands at the root.
- Put shared implementation in a named subdirectory.
- Keep installed mode explicit where a command supports both source and
  installed paths.
- Keep host-specific secrets and private maintenance commands outside the repo.
