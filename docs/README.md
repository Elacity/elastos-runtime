# Docs

Reference documentation that used to live at the repo root now lives here.

## Canonical Current Docs

These are the main active docs for outside readers and contributors.

- [../README.md](../README.md) — repo front door
- [state.md](../state.md) — factual current-state summary
- [GETTING_STARTED.md](GETTING_STARTED.md) — developer build/run path
- [RUN_HOME_MACOS.md](RUN_HOME_MACOS.md) — repeatable runbook for the browser Home shell + passkey login on macOS (host lock + WebAuthn gotchas)
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
- [PRINCIPLES_CONFORMANCE.md](PRINCIPLES_CONFORMANCE.md) — audit of where code holds/breaks PRINCIPLES.md, ranked improvement areas, and an investigated-but-not-a-defect list
- [adr/0001-extract-app-and-service-logic-from-trusted-core.md](adr/0001-extract-app-and-service-logic-from-trusted-core.md) — ADR: shrink the trusted core by moving app/service logic into its capsules (Principle 5)
- [CAPABILITY_AUDIT.md](CAPABILITY_AUDIT.md) — capability-conformance audit: enforcement architecture, what's proven, and the gaps (machine-checked in the `capability_conformance` test harness)
- [SECURITY_AUDIT.md](SECURITY_AUDIT.md) — adversarial security audit: crypto correctness, identity binding, secrets hygiene, memory safety; findings + what's sound
- [CONFIDENTIAL_COMPUTE.md](CONFIDENTIAL_COMPUTE.md) — TEE / hardware-enclave (SEV-SNP, TDX, remote attestation) opportunity, runtime-wide wedges, and a phased architecture scaffold (forward design; nothing implemented yet)
- [CAPSULE_MODEL.md](CAPSULE_MODEL.md) — supplemental capsule/runtime terminology note
- [CARRIER.md](CARRIER.md) — supplemental Carrier framing note
- [CONTENT_AVAILABILITY.md](CONTENT_AVAILABILITY.md) — IPLD, CID sync, availability receipts, and SmartWeb content-plane direction
- [ARCHIVE_POLICY.md](ARCHIVE_POLICY.md) — Archive dependency, release, and generic-family enablement policy
- [PC2_CONVERGENCE.md](PC2_CONVERGENCE.md) — current translation of useful PC2 patterns into Runtime provider/capsule boundaries
- [CHAIN_PROVIDER.md](CHAIN_PROVIDER.md) — typed chain-provider boundary and current blockchain-quadrant slice
- [WALLET_PROVIDER.md](WALLET_PROVIDER.md) — wallet proof, account-link, typed-signing, and transaction authority boundary
- [BROWSER_CAPSULE.md](BROWSER_CAPSULE.md) — Browser/Net/Exit/Engine ABI, product rule, and current proof boundary
- [BROWSER_PROVIDER_BAKEOFF.md](BROWSER_PROVIDER_BAKEOFF.md) — hosted/native browser-provider comparison and acceptance gates
- [RIGHTS_PROVIDER.md](RIGHTS_PROVIDER.md) — typed protected-content rights questions and fail-closed policy boundary
- [KEY_PROVIDER.md](KEY_PROVIDER.md) — protected-content key release and PQ-hybrid envelope boundary
- [DECRYPT_PROVIDER.md](DECRYPT_PROVIDER.md) — protected-content decrypt/render session boundary
- [PROTECTED_CONTENT.md](PROTECTED_CONTENT.md) — sealed objects, DRM provider boundary, and protected-content sequence
- [GLOSSARY.md](GLOSSARY.md) — vocabulary only

These should stay narrower than the canonical current docs. If they repeat the same story in different words, they should be merged or shortened rather than expanded.

## Release and Versioning

- [VERSIONING.md](VERSIONING.md) — runtime release versioning policy
- [SHARE_VERSIONING.md](SHARE_VERSIONING.md) — share lifecycle and versioning model
