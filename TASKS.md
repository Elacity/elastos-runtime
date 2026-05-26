# Tasks

Open work only. Completed work belongs in
[elastos/CHANGELOG.md](elastos/CHANGELOG.md). Verified current truth belongs in
[state.md](state.md).

Operating principle: one canonical path per operation and clear failure when a path is not yet ready.

Guiding-star constraints live in [PRINCIPLES.md](PRINCIPLES.md).

Do not add new product surface area until the `Now` section is materially tighter.

## Now

### 0. Capsule authority and Carrier boundary
- [ ] Make the capsule contract explicit and test-enforced: app/viewer/content capsules must not get direct filesystem, IPFS, database, raw network, or capsule-to-capsule authority; all effects must go through runtime-issued capability calls over the Carrier/provider plane.
- [ ] Make local provider calls and remote Carrier calls share one capsule-facing contract so capsules do not branch on "local" versus "network"; only the runtime/provider layer chooses the transport.
- [ ] Treat HTTP/browser routes as host adapters only. A browser-hosted capsule may use an HTTP endpoint to reach the local runtime, but the product contract must remain `capsule -> runtime capability -> Carrier/provider -> object/service`.
- [ ] Add a fail-closed check that normal app capsules stay Carrier-only by default: no `guest_network`, no host execution, no raw off-box transport, and no provider-specific protocol knowledge in app UI.
- [ ] Keep provider capsules as explicit exceptions with narrow manifests, capability schemas, audit events, and user/operator-visible reason strings.
- [ ] Keep Browser on one Browser/Net/Exit ABI: Browser UI asks Runtime to open a page, Runtime routes through Browser Engine Adapter + Net + Exit providers, and candidate-specific details stay behind provider contracts.
- [ ] Treat Selkies as the current self-hosted baseline, not the acceptance answer; do not spend more branch time tuning Selkies as the product path unless the structured `next_action` explicitly says that is the next gate.
- [ ] Freeze new Browser provider implementation on this branch after the current proof gates; the next Browser path should be a native Chromium/CEF-style adapter or an operator-owned hosted candidate proven through the same Browser/Net/Exit ABI.

### 1. Home environment
- [ ] Keep `home` as the browser capsule ID and `home-cli` as the terminal capsule ID; visible product language is `Home`.
- [ ] Extend the runtime-owned Home contract beyond identity + app catalog: Library browsing, runtime health, capability prompts, and attach/focus semantics.
- [ ] Expand `System` beyond identity + app inventory into a real system surface.
- [ ] Prove one truthful `Home -> System -> app -> focus/close -> Home` manual loop, then decide the first non-browser attachment contract.
- [ ] Keep the default Home path compatible with macOS and Windows by avoiding KVM-only assumptions.
- [ ] Remove remaining donor/KVM-only assumptions from scripts and runtime special cases.
- [ ] Replace the current `route + attach_kind` launch payload with a runtime-issued launch grant that is transport-agnostic and non-delegatable.
- [ ] Add an explicit runtime/manifest exposure contract for Home, gateway, and shared surfaces so internal-only and external-only objects do not depend on name-based filtering.

### 2. Home front-door boringness
- [ ] Prove one boring installed `elastos -> Home -> app -> Home` path on Jetson and WSL.
- [ ] Keep tightening dashboard navigation, return-home behavior, and single-owner TTY/session rules until target-machine proof is boring.
- [ ] Keep unfinished surfaces out of the main live path unless they launch from Home and return cleanly.
- [ ] Rehearse and simplify the Home/People/Spaces/System story so the front door feels useful without internal-runtime narration.
- [ ] Promote local Appearance state into a DID-anchored profile/settings object that syncs through Carrier/provider policy and projects back into `localhost://ElastOS/System/Appearance/...` per trusted device.
- [ ] Keep `Apps` as the public catalog term and `capsules` as the internal/runtime term; do not expose both as competing public nouns.
- [ ] Keep `Settings` and `Storage` as sections inside `System`, not as separate top-level ontology.
- [ ] Decide the explicit home-return contract for native and non-native chat surfaces.
- [ ] Split Home surfaces cleanly into launchable apps, site/share actions, and support assets instead of mixing them in one Apps list.
- [ ] Keep only shipped, installable, launchable, and useful items in `Apps`; demote or hide unfinished catalog-only entries until they earn real Home actions.
- [ ] Make `MyWebSite` useful from Home with a real local preview path plus a first-class `Go public` action, not just long notices.
- [ ] Make `setup --profile demo` install the app capsules Home honestly advertises, or stop advertising them there.
- [ ] Make `GBA UCity` launch cleanly from Home on the installed path, not only from local source proof.
- [ ] Decide and vendor the mobile/WebView GBA engine path: either a non-threaded mGBA WASM artifact, a different single-threaded emulator core, or a native/mobile host adapter. Until then, fail fast on missing WebAssembly threads instead of hanging.
- [ ] Decide whether `Chat WASM` is a real Home-visible app on supported hosts or a developer-only surface; then make Home match that truth.
- [ ] Decide whether blocked apps should be hidden entirely from the main Apps surface or moved into an explicit install/setup section.

### 3. Release / install / update coherence
- [ ] Lock interactive-launch, stale-runtime, and stale-support-asset regressions with explicit coverage.
- [ ] Extend outsider proof beyond local x86_64 until Jetson/WSL evidence is equally solid.
- [ ] Keep `scripts/public-install-identity-smoke.sh` in scope as the DID-backed People/profile contract for public install proof.
- [ ] Keep `scripts/public-install-operator-smoke.sh` and `scripts/public-install-home-frontdoor-smoke.sh` in scope as installed public front-door/operator proof.
- [ ] Keep `scripts/audit-linux-runtime-portability.sh` in scope as the public Linux runtime portability proof.

### 4. Truth surfaces and anti-drift
- [ ] Remove duplicated volatile facts such as scattered versions, metrics, and proof transcripts from durable docs.
- [ ] Keep `PRINCIPLES.md`, docs, and command surfaces aligned through fail-closed checks instead of periodic prose cleanup.
- [ ] Encode the proof-first and command-surface guardrails in durable repo docs so agents do not keep reinventing launch models or overstating proof.
- [ ] Add a future-work review gate before implementation: every plan must name the smallest shippable slice, affected quadrant(s), capsule-authority boundary, verification command, and entropy risk being avoided.
- [ ] Reject plans that add public UI, protocol bridges, provider behavior, or blockchain hooks before the underlying principal, capability, package, or space contract is explicit and testable.

### 5. Site / publication surface
- [ ] Keep `MyWebSite`, publication, channels, activation, and rollback on one coherent local-first path.
- [ ] Evolve site/publication state toward cleaner resolver-owned system-service objects.
- [ ] Make the combined publish + host refresh + live deployment ceremony deterministic and easy to verify.

## Next

### Four-quadrant runtime balance
- [ ] Balance the next phase across the four ElastOS quadrants instead of over-investing in one layer:
  1. **PC2/Home**: user front door, object browser, install/launch UX, spaces and people views
  2. **Runtime**: trusted core, principals, sessions, package verification, interface contracts, capability routing
  3. **Carrier**: authenticated object/message/stream transport, discovery, sync, replication, content delivery
  4. **Blockchain**: DID/EID, wallet signing, provenance anchors, publisher identity, optional receipts/licensing
- [ ] Use this order for future plan reviews: first prove the runtime contract, then expose the PC2/Home UX, then route through Carrier/provider transport, then add blockchain anchoring only where identity/provenance/approval needs it.
- [ ] Build wallet-backed identity + WebConnect as the first balancing move:
  1. PC2: first-class connect/login/pair device flow
  2. Runtime: principal/session model bound to DID/wallet identity and app-scoped capabilities
  3. Carrier: authenticated device pairing and session attach transport
  4. Blockchain: wallet-provider plus DID/EID signing and approval hooks
- [ ] Build Spaces/network drives as the second balancing move:
  1. PC2: `People`, `Spaces`, and shared-drive browsing without exposing transport names as product truth
  2. Runtime: mount records, object heads, ACLs, watch/sync APIs, and resolver-owned WebSpace traversal
  3. Carrier: discovery, sync, replication, shared-state updates, and content delivery
  4. Blockchain: optional ownership/provenance anchors for space heads and published shares
- [ ] Build capsule publish/install registry as the third balancing move:
  1. PC2: install/pin/unpin UX, trusted/untrusted publisher state, and app catalog actions that all work
  2. Runtime: signed bundle identity, whole-package verification, interface/version contracts, install receipts, update policy
  3. Carrier: package/update distribution and peer discovery for trusted sources
  4. Blockchain: publisher identity, provenance receipts, and optional license/payment hooks without making token mechanics the core model
- [ ] Do not prioritize rich DRM economics, DeFi/BtcFi, Android box specifics, or literal Capsule-NFT mechanics before the package identity, principal, space, and provider contracts are real.

### Runtime primitives missing for the PC2 world-computer model
- [ ] Replace hardcoded `Users/self` assumptions with first-class principals: user DID, device DID, personas, agents, active session, and capability tokens bound to principal + capsule + session.
- [ ] Add authenticated Carrier envelopes as the default application contract: sender DID, object identity, signature, capability context, replay protection, and verified delivery status. Keep raw gossip/transport as an explicit unsafe/provider-level lane.
- [ ] Add a real WebSpace mount/object model: mount table, resolver selection, object heads, local cache, access policy, sync cursors, and typed viewer resolution.
- [ ] Add signed package identity for every installable capsule: manifest hash, full bundle hash/Merkle root, publisher DID, signature chain, interface descriptors, and install/update receipts.
- [ ] Add an interface registry primitive: signed interface descriptors, semantic versions, required/provided capability schema, compatibility resolution, and fail-closed launch when required interfaces are missing.
- [ ] Add wallet/EID/chain providers behind the runtime boundary. The runtime should expose capability-gated signing, approval, credential, and provenance operations; it should not embed chain business logic.
- [ ] Keep network-drive/provider operating systems outside the trusted core. The runtime owns verification, capability routing, and audit; provider capsules/services own Telegram/Nostr/Matrix/Facebook/IPFS/Carrier-specific behavior.

### WebSpace / WCI contract
- [ ] Expand the current `webspace-provider` slice into fuller resolver outputs and deeper typed traversal.
- [ ] Clarify the relationship between rooted localhost paths, `elastos://...`, and mounted WebSpace views without freezing syntax too early.
- [ ] Define the CAS object model so paths stay the comfort layer rather than the real identity model.
- [ ] Keep capsule execution substrate (`type`), product role (`shell`/`app`/`viewer`/`provider`/`content`), and launch exposure as separate runtime concepts instead of letting one field imply the others.
- [ ] Document and enforce the object/capsule/space split consistently across UI copy, manifests, runtime docs, and shell/catalog surfaces.

### Collaboration and messaging
- [ ] Earn IRC only as an explicit packaged path with honest runtime prerequisites and proof.
- [ ] Build toward a first-class collaboration provider instead of letting compatibility bridges define the architecture.

### Documents and Library
- [ ] Add import/fork flows for immutable `elastos://<cid>` document revisions through the same provider contract.
- [ ] Unify the markdown packaging model so local documents, viewer/editor content, and `elastos share` do not keep using three different markdown stories.
- [ ] Decide the first collaborative document core intentionally; prefer a Rust/WASM CRDT evaluation (`Yrs` first, `Automerge` second) over ad hoc editor glue or a direct port of external JS products.
- [ ] Keep keystroke-level local editing local-first and low-latency; Carrier should carry remote sync/share/collaboration updates, not gate every same-runtime write.
- [ ] Keep the remaining implementation order explicit:
  1. add import/fork/open flows for `elastos://<cid>` revisions through the same provider contract
  2. unify local documents, viewer/editor content, and `elastos share` under one markdown packaging story
  3. add collaboration, comments, and presence on top of the provider/session contract instead of baking sync assumptions into the editor UI

### Inbox
- [ ] Add Inbox coverage for every first-party approval/action flow that can be initiated by a human or an agent.

### Human/agent parity and design system
- [ ] Extract the shared capsule token block into a versioned support asset once capsule packaging can import shared CSS without coupling style to runtime authority.

### Trusted content and access rights
- [ ] Define one trust contract for installable capsules and published objects: DID/CID/hash/signature identify the thing before it is opened, executed, or decrypted.
- [ ] Design sealed-content packaging so encrypted payloads are normal for protected apps, media, and documents instead of special-case add-ons.
- [ ] Introduce an ElastOS-native access/decryption provider analogous to a Lit-style policy gate: authorize by DID, entitlement, and capability, then issue short-lived decryption access.
- [ ] Keep rights evaluation in the provider plane, not in per-app bespoke license logic or gateway-only checks.
- [ ] Keep Carrier responsible for encrypted blob transport and remote update delivery, not for replacing capability and policy enforcement.
- [ ] Prove one honest protected-content flow end to end: resolve object by stable identity, verify trust material, authorize access, decrypt for the rightful user, open in the correct viewer/app, and fail closed for everyone else.

### Operator and audit hardening
- [ ] Keep `verify`, `command-smoke`, `installed-command-audit`, and related gates honest and fail-closed.
- [ ] Continue the systematic crate audit through the remaining runtime crates.

### 6. Dead code cleanup
- [ ] Re-audit `provider/registry.rs` from current source, not from the stale dead-code list that existed before the 2026-03-31 cleanup. Only remove API surface that is now proven unused on the installed path.
- [ ] Continue the crate-by-crate orphaned-code audit with the same fail-closed rule: delete only after proving the installed path does not use it.

## Later

- [ ] Decide how much Puter-derived UI remains after the runtime-owned Home/System contract is stable.
- [ ] Define the browser host-adapter model without faking Linux parity.
- [ ] Introduce the dedicated browser capsule only after the runtime launch/object contract is stable; it should be one app with outbound network capability, not the platform.
- [ ] Decide the longer-term operator packaging path for Codex and related AI/agent surfaces.
- [ ] Add a hosted-key AI provider behind a stable runtime contract.
- [ ] Lift the first in-process rights/decryption provider into its own first-party provider capsule once the contract is stable enough to stop changing every week.
- [ ] Consider renaming `elastos-server` crate to `elastos-cli`. It is the CLI binary + all commands, not just a server. The current name misleads new developers about what the crate does.
