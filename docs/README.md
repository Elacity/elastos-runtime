# Docs

Reference documentation that used to live at the repo root now lives here.

## Canonical Current Docs

These are the main active docs for outside readers and contributors.

- [../README.md](../README.md) — repo front door
- [../PRINCIPLES.md](../PRINCIPLES.md) — proof discipline and runtime authority constraints
- [state.md](../state.md) — factual current-state summary
- [GETTING_STARTED.md](GETTING_STARTED.md) — developer build/run path
- [COMMAND_MATRIX.md](COMMAND_MATRIX.md) — command/runtime contract
- [RUNTIME_REPO_USER_STORY_CHECKLIST.md](RUNTIME_REPO_USER_STORY_CHECKLIST.md) — release-facing test matrix and manual runbook
- [SITES.md](SITES.md) — current site and public-edge model
- [INSTALL.md](INSTALL.md) — signed install and update flow

Planning and truth surfaces outside `docs/`:

- [../TASKS.md](../TASKS.md)
- [../ROADMAP.md](../ROADMAP.md)
- [../state.md](../state.md)

## Deep Reference

- [ARCHITECTURE.md](ARCHITECTURE.md) — system design and crate/layer structure
- [OVERVIEW.md](OVERVIEW.md) — high-level repo/system summary
- [CAPSULE_MODEL.md](CAPSULE_MODEL.md) — supplemental capsule/runtime terminology note
- [CAPSULE_AUTHORING.md](CAPSULE_AUTHORING.md) — role, ABI, authority, manifest, template, and verification guide for capsule authors
- [../templates/capsules/README.md](../templates/capsules/README.md) — canonical capsule scaffolds kept executable by repository gates
- [../elastos/wit/elastos-bus-v1.wit](../elastos/wit/elastos-bus-v1.wit) — minimal `elastos:bus@v1` WIT world for product capsules
- [NAMESPACES.md](NAMESPACES.md) — `elastos://`, `localhost://`, and principal-root namespace rules
- [CARRIER.md](CARRIER.md) — supplemental Carrier framing note
- [PEOPLE_CONVERSATIONS.md](PEOPLE_CONVERSATIONS.md) — People, pairing, guest, and conversation model
- [CONTENT_AVAILABILITY.md](CONTENT_AVAILABILITY.md) — IPLD, CID sync, availability receipts, and SmartWeb content-plane direction
- [ARCHIVE_POLICY.md](ARCHIVE_POLICY.md) — Archive dependency, release, and generic-family enablement policy
- [PC2_CONVERGENCE.md](PC2_CONVERGENCE.md) — current translation of useful PC2 patterns into Runtime provider/capsule boundaries
- [RUNTIME_FRAMEWORK_RESEARCH.md](RUNTIME_FRAMEWORK_RESEARCH.md) — research notes for runtime framework choices that are not current commitments
- [ESP_V0.md](ESP_V0.md) — shell protocol descriptor for existing projection and consent routes
- [CAPSULE_INTERFACE_CONTRACT.md](CAPSULE_INTERFACE_CONTRACT.md) — shared web/CLI/fact/affordance/gate/audit contract for capsule projections
- [HOME_SHELL_HOST_CONTRACT.md](HOME_SHELL_HOST_CONTRACT.md) — runtime-owned Home front-door contract for unlock, active shell selection, root mounting, child intents, and recovery
- [INTERACTIVE_RUNTIME_CONTRACT.md](INTERACTIVE_RUNTIME_CONTRACT.md) — interactive runtime/session contract
- [CAPSULE_INSPECTOR.md](CAPSULE_INSPECTOR.md) — Runtime-owned live object mirror, gate preview, and Inbox-gated act path
- [INSPECTOR_TESTING.md](INSPECTOR_TESTING.md) — local Inspector testing path
- [CHAIN_PROVIDER.md](CHAIN_PROVIDER.md) — typed chain-provider boundary and current blockchain-quadrant slice
- [WALLET_PROVIDER.md](WALLET_PROVIDER.md) — wallet proof, account-link, typed-signing, and transaction authority boundary
- [BROWSER_CAPSULE.md](BROWSER_CAPSULE.md) — Browser/Net/Exit/Engine ABI, product rule, and current proof boundary
- [BROWSER_VM_TARGET.md](BROWSER_VM_TARGET.md) — Browser VM target, guest, helper, and media contract
- [BROWSER_PROVIDER_BAKEOFF.md](BROWSER_PROVIDER_BAKEOFF.md) — hosted/native browser-provider comparison and acceptance gates
- [MAC.md](MAC.md) — macOS staging, source-home, and Browser VM notes
- [DESIGN_SYSTEM.md](DESIGN_SYSTEM.md) — Home and app visual design tokens/conventions
- [RIGHTS_PROVIDER.md](RIGHTS_PROVIDER.md) — typed protected-content rights questions and fail-closed policy boundary
- [KEY_PROVIDER.md](KEY_PROVIDER.md) — protected-content key release and PQ-hybrid envelope boundary
- [DECRYPT_PROVIDER.md](DECRYPT_PROVIDER.md) — protected-content decrypt/render session boundary
- [PROTECTED_CONTENT.md](PROTECTED_CONTENT.md) — sealed objects, DRM provider boundary, and protected-content sequence
- [GLOSSARY.md](GLOSSARY.md) — vocabulary only

These should stay narrower than the canonical current docs. If they repeat the same story in different words, they should be merged or shortened rather than expanded.

## Release and Versioning

- [VERSIONING.md](VERSIONING.md) — runtime release versioning policy
- [SHARE_VERSIONING.md](SHARE_VERSIONING.md) — share lifecycle and versioning model
