# Tasks

Open work only.
Completed milestones belong in [elastos/CHANGELOG.md](elastos/CHANGELOG.md).
Verified current truth belongs in [state.md](state.md).

Operating principle: one canonical path per operation, no silent fallbacks, and clear failure when a path is not yet ready.

Guiding-star constraints live in [PRINCIPLES.md](PRINCIPLES.md).

Do not add new product surface area until the `Now` section is materially tighter.

## Now

### 1. Chat / IRC contract freeze
- [ ] Define one explicit executable-capsule runtime contract covering identity bootstrap, capability acquisition, Carrier access, localhost access, interactive TTY ownership, and home/exit signaling.
- [ ] Converge `chat`, `pc2`, `agent`, `shell`, and attached WASM flows toward one blessed runtime/provider access path.
- [ ] Decide the fate of the attached WASM bridge contract instead of leaving mixed doctrine in source and docs.
- [ ] Standardize interactive launch ownership across PC2, native chat, WASM chat, and microVM launch.
- [ ] Add a proof matrix for the standardized contract across native chat, WASM IRC, microVM IRC, agent, and one simple localhost app.
- [ ] Either prove the new per-client session identity work in the shared chat layer or replace it with a simpler invariant; do not keep source-only complexity that still fails the proof matrix.
- [ ] Restore and lock a proof matrix for native/native, native/WASM, and native/microVM chat-family interop on both same-host and cross-host paths.

### 2. PC2 front-door boringness
- [ ] Prove one boring installed `elastos -> PC2 -> app -> home` path on Jetson and WSL.
- [ ] Keep tightening dashboard navigation, return-home behavior, and single-owner TTY/session rules until target-machine proof is boring.
- [ ] Keep unfinished surfaces out of the main live path unless they launch from PC2 and return cleanly.
- [ ] Rehearse and simplify the Home/People/Spaces/System story so the front door feels useful without internal-runtime narration.
- [ ] Decide the explicit home-return contract for native and non-native chat surfaces.
- [ ] Split PC2 surfaces cleanly into launchable apps, site/share actions, and support assets instead of mixing them in one Apps list.
- [ ] Keep only shipped, installable, launchable, and useful items in `Apps`; demote or hide `Markdown Viewer`, `Notepad`, `Codex`, and any other catalog-only entries until they earn real PC2 actions.
- [ ] Make `MyWebSite` useful from PC2 with a real local preview path plus a first-class `Go public` action, not just long notices.
- [ ] Make `setup --profile demo` install the app capsules PC2 honestly advertises, or stop advertising them there.
- [ ] Make `GBA UCity` launch cleanly from PC2 on the installed path, not only from local source proof.
- [ ] Decide whether `Chat WASM` is a real PC2-visible app on supported hosts or a developer-only surface; then make PC2 match that truth.
- [ ] Decide whether blocked apps should be hidden entirely from the main Apps surface or moved into an explicit install/setup section.

### 3. Release / install / update coherence
- [ ] Lock interactive-launch, stale-runtime, and stale-support-asset regressions with explicit coverage.
- [ ] Extend outsider proof beyond local x86_64 until Jetson/WSL evidence is equally solid.
- [ ] Keep `scripts/public-install-update-smoke.sh` in scope as the stamped-install update proof.
- [ ] Keep `scripts/public-install-identity-smoke.sh` in scope as the DID-backed People/profile contract for public install proof.
- [ ] Keep `scripts/public-linux-runtime-portability-smoke.sh` in scope as the public Linux runtime portability proof.

### 4. Truth surfaces and anti-drift
- [ ] Remove duplicated volatile facts such as scattered versions, metrics, and proof transcripts from durable docs.
- [ ] Keep `PRINCIPLES.md`, docs, and command surfaces aligned through fail-closed checks instead of periodic prose cleanup.
- [ ] Encode the proof-first and command-surface guardrails in durable repo docs so agents do not keep reinventing launch models or overstating proof.

### 5. Site / publication surface
- [ ] Keep `MyWebSite`, publication, channels, activation, and rollback on one coherent local-first path.
- [ ] Evolve site/publication state toward cleaner resolver-owned system-service objects.
- [ ] Make the combined publish + host refresh + live deployment ceremony deterministic and easy to verify.

## Next

### WebSpace / WCI contract
- [ ] Expand the current `webspace-provider` slice into fuller resolver outputs and deeper typed traversal.
- [ ] Clarify the relationship between rooted localhost paths, `elastos://...`, and mounted WebSpace views without freezing syntax too early.
- [ ] Define the CAS object model so paths stay the comfort layer rather than the real identity model.

### Collaboration and messaging
- [ ] Earn IRC only as an explicit packaged path with honest runtime prerequisites and proof.
- [ ] Build toward a first-class collaboration provider instead of letting compatibility bridges define the architecture.

### Operator and audit hardening
- [ ] Keep `verify`, `command-smoke`, `installed-command-audit`, and related gates honest and fail-closed.
- [ ] Continue the systematic crate audit through the remaining runtime crates.

### 6. Dead code cleanup
- [ ] Re-audit `provider/registry.rs` from current source, not from the stale dead-code list that existed before the 2026-03-31 cleanup. Only remove API surface that is now proven unused on the installed path.
- [ ] Continue the crate-by-crate orphaned-code audit with the same fail-closed rule: delete only after proving the installed path does not use it.

## Later

- [ ] Define the browser host-adapter model without faking Linux parity.
- [ ] Decide the longer-term operator packaging path for Codex and related AI/agent surfaces.
- [ ] Add a hosted-key AI provider behind a stable runtime contract.
- [ ] Explore stronger attestation, encrypted capsules, and protected-content flows after the local runtime contract is stable.
- [ ] Consider renaming `elastos-server` crate to `elastos-cli`. It is the CLI binary + all commands, not just a server. The current name misleads new developers about what the crate does.
