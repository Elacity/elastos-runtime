# Protected-content extraction plan

Status: working plan for the permissioned mint → buy → open → play path.
This is not installed product truth. Released 0.6 still uses the provisional
`drm` / `rights` / `key` / `decrypt` surface.

Dirty `main` `TASKS.md` 0.7 text is unreviewed operator planning. Canonical
remaining work for this extraction is this document plus `TASKS.md` on
`feat/protected-content-runtime-lifecycle` (`c81ad819`), stacked on
`origin/feat/protected-content-rights`.

## Why this shape

Principles that decide every slice:

- One canonical path per operation. No dual decoder, no classical-only mint
  “until PQ is ready,” no PQ-off default, no second coordinator.
- Fail closed. Missing KEM, missing receipt, lost provider response, and
  caller-selected nodes are explicit failures, not fallbacks.
- Capsules are not authority. They never receive CEKs, shares, routes, hosts,
  IPs, ports, credentials, or bearer play URLs.
- Runtime owns selection, lifecycle, audit, and settlement. Providers own
  operation semantics. Carrier only transports Runtime-selected endpoints.
- Encrypted objects use the normal content path. Decrypt and rights stay behind
  typed providers.
- Small trusted core. Do not grow public codec crates or Library/DRM product
  routes to make an inactive seam compile.

The first honest product proof is one installed Runtime with two human
principals: publish, mint, list, buy, acquire, play. Content opens only at the
declared threshold of approved custody nodes. Apps never see the CEK. This is
not public dKMS, not a storage market, and not Browser completion. Current
source still pins one local Runtime device issuer; multi-Runtime issuer
admission and cross-Runtime protected-content exchange are later explicit
pre-public-network gates.

## Source of truth

Published review stack. Use these trees; do not invent a parallel stack.

| Branch | Commit | Tree | What it proves |
|---|---|---|---|
| `origin/feat/protected-content-custody-provider` | `f7cd6c3dfe4fc3f6899c88af3ee4c082b49e3a49` | `b9b93460f66b94759100b5993887e24c599e27a8` | Object/pool/epoch/committee binding; one node, one sealed share; Runtime issuer pinning; owner-only node state; duplicate/conflict/restart; rights-gated release; encrypted contribution replay; bounded frames; strict shutdown. Unregistered. |
| `origin/feat/protected-content-wallet-rights` | `2c69d0c2af00f7050faa424d3f7d6f4e41a92a9a` | `73764e9aef98bf7ea7e24989ce051594a05a71ac` | Wallet signs exact canonical `RightsRequestV1`. Generic approval cannot substitute. |
| `origin/feat/protected-content-runtime` | `b00bfeeb894033559239b7b438b5558cab900b4d` | `7b8b1945471e86859b56e6175d8ba35330af5e48` | Private durable release journal and typed internal coordination. Persist before effects; record `provider_effect_started`; replay only exact terminals; ambiguous post-effect outcomes stay nonterminal. |
| `origin/feat/protected-content-rights` | `43a83e5bd405820713bb88d4e32950b5bfa26ccb` | `34e5bb5379268419ff8c5b4dc97cc0631d70c2b3` | Typed Chain rights evidence and rights evaluator. Capsule-facing old boolean rights rail is gone. Provisional `rights-provider` stays until atomic cutover. |

The published Runtime seam is `elastos-protected-content-runtime`, not a
gateway-owned commerce workflow and not the old dirty server module.

Still not product-ready: the inactive source proof is now complete, but it is
not installed and not cut over. The active Library/Marketplace/runtime path
still uses provisional providers and DTOs. Current source already proves
producer-side CENC protection, provider/object/publisher-pinned availability,
alpha success plus beta terminal denial, wrong-recipient rejection,
wrong-object/media-binding rejection, encrypted-segment tamper rejection,
exact durable replay, explicit provider unregister/absence cleanup, and zero
unresolved release state through the inactive Runtime/provider path. Lower
layers already own restart/crash/cleanup. Packaging, Runtime custody
composition, the clear-media import boundary, and the Runtime-internal chain
policy resolver are complete on this line. The remaining work is Library
mint/list wiring, Marketplace buy / Library open, atomic cutover, and the
installed one-Runtime/two-principal acceptance proof.

## Evidence only — do not continue or merge

### `feat/protected-content-runtime-coordinator-v1`

- Tip: `18c266a246eaf079dd0535f044b9c838d1c09a1a` tree `74a038ac`
- Merge-base with rights: `3b07fd4d` (predates the published stack)
- Do not continue it. Do not cherry-pick it.

It mixes stale decrypt wire (`read_viewer_chunk`, plaintext chunks), public
chunk-payload types in the contracts crate, dependency drift, and a server
`protected_content_runtime` layout that does not exist on rights. It maps
custody toward the live `key` route. It does not solve identity-bound
reconciliation after `provider_effect_started`. Keep it as evidence for later
adapter negative-test ideas only, rewritten against the current crate.

### PR #15 / `feat/dkms-esp-port`

Mine, do not merge: PQ-hybrid `ddrm-envelope` crypto, threshold/negative tests,
CEK commitment, recipient-sealed contributions, node-local custody direction,
CENC/`cenc-core` play, owner-only access grants.

Reject: public `shares[]`, `rail_shim`, custom Carrier/TCP/WireGuard, WASI/
microVM product claims, `docs/dkms/**` as truth, PQ-off decrypt defaults,
`elastos-logger`, `act-emitter`, standalone harness, capsule-owned topology.

PR #15 already requires exactly one `covered_address` equal to the recovered
owner. v1 `RightsRequestV1` already rejects an attacker-signed victim wallet.
Keep both. Do not reintroduce a covered-address list.

### Sash PR #23

House URUX on `feat/home-agent-harness-rebuild`. `source-macos` failed. Not the
protected-content path. Wait.

## Ordered slices

The remaining A-F slices are coherent commit slices on the current working line
`feat/protected-content-runtime-lifecycle`, building forward from the
inactive proof at `8eff416e`. Do not reopen a third D branch or require a new
child branch per slice. Publication, repacking, or branch reshaping can be
decided only after the source line is accepted. Do not stack new work on
`main`, dirty `main`, or `coordinator-v1`.

### Current inactive proof coverage

The combined inactive source proof is complete at
`feat/protected-content-runtime-lifecycle` `8eff416e`:

- exact fixed-layout content availability publish/status/refetch/verify is
  covered by
  `elastos/crates/elastos-server/src/protected_content_runtime/tests.rs::
  protected_content_availability_publishes_status_refetches_and_verifies_exact_media`
  plus its negative companions;
- one real custody-provider process plus public provisioning receipt parsing is
  covered by
  `...::runtime_custody_registry_adapter_process_happy_path_uses_public_provision_receipt`;
- real three-node 2-of-3 release with the third node not invoked after
  threshold success is covered by
  `...::runtime_release_coordinator_process_two_of_three_success_stops_before_third_node`;
- real alpha success plus beta terminal denial through the same node-local
  rights boundary is covered by the existing custody process test surface; and
- the combined real protect -> mint -> availability -> buy -> release ->
  decrypt -> init/segment read -> close path is covered by
  `...::runtime_decrypt_registry_adapter_process_reconstructs_for_prepared_recipient_and_closes_cleanly`,
  including wrong-recipient rejection, wrong-object/media-binding rejection,
  encrypted-segment tamper rejection, exact durable replay from the same
  Runtime release journal, explicit provider unregister/absence cleanup,
  byte-exact clear recovery, and zero unresolved release state.

Restart/crash/cleanup are already owned by focused lower-layer tests:

- `capsules/custody-provider/tests/process.rs::
  custody_provider_process_provisions_releases_replays_after_restart_and_shuts_down`
- `capsules/protected-content-decrypt-provider/tests/process.rs::
  process_prepare_open_read_close_replay_and_restart_absence_flow`
- `elastos/crates/elastos-protected-content-runtime/src/journal.rs::
  durable_state_replays_only_persisted_terminal_result`
- `elastos/crates/elastos-protected-content-runtime/src/coordinator.rs::
  runtime_coordination_replays_terminal_without_dispatch`
- `elastos/crates/elastos-protected-content-runtime/src/mint.rs::
  restart_after_effect_started_stays_nonterminal`
- `elastos/crates/elastos-protected-content-runtime/src/mint.rs::
  custody_provisioned_replays_without_redispatch`

No separate pre-cutover live Profile/Wallet/Chain process harness is required
from current source truth. Focused Profile-signing, Wallet binding, Chain
evidence, and the integrated deterministic process path already cover those
source seams.

### Installed prerequisites

Before cutover work can honestly pass installed proof, the operator must supply:

1. one signed permissioned custody profile with the exact pool, epoch,
   committee authorization, three distinct node/provider identities, three
   owner-only state roots, 2-of-3 threshold, expected Runtime issuer, and
   lifecycle config; this is signed policy and process identity only, not a
   physical/operator-independence claim; and
2. one configured test Chain network plus the rights/purchase contract binding,
   exact method/selector, two funded test accounts, and the existing Wallet
   approval/transaction coordinator and Chain evidence path. Deterministic
   fixtures in source tests are not a product claim for this prerequisite.

### Slice A — package and provision internal providers

Entry:

- combined inactive source proof is green at `8eff416e`;
- no live route is changed; and
- disk stays above 15%.

Bounded work and file groups:

- add one packaged binary per protected-content provider kind (protect,
  custody, decrypt) to the existing component/install/source-home packaging
  path;
- keep custody node instances as owner-only process state/config selected by
  later inactive Runtime-owned call sites, not as separate packaged
  components;
- stage built-artifact identity and package metadata for those binaries without
  implying a live install, signed custody profile installation, or cutover;
- update provider manifests/lockfiles and install/source-home scripts; and
- touch only the packaging surfaces that must know these internal providers
  exist.

Likely files:

- `components.json`;
- provider manifests and lockfiles;
- install/source-home scripts; and
- focused provider/install tests.

Exact exit:

- `protected-content-protect-provider`, three custody-provider instances, and
  `protected-content-decrypt-provider` are packageable/provisionable through the
  existing component/install/source-home packaging path;
- the new protected-content provider set is internal and inactive only; and
- built/staged artifact identity plus package metadata are provable for Runtime,
  `components.json`, and the provider binaries, while custody instance
  startup/selection remains deferred to later inactive creator/open call sites.

Focused verification:

- provider packaging tests;
- staged-artifact identity and metadata checks; and
- leak scans showing no CEK/share/topology/path/credential exposure.

### No separate Slice B service/facade

The reviewed minimal path rejects a standalone protected-content product
service/facade. Do not add a new service, trait, DTO, registry, supervisor,
coordinator, journal, or a leaf-adapter-only production commit with no real
caller.

Instead:

- protect invocation lands with its first real inactive Library creator
  publish/mint/list caller;
- decrypt invocation lands with its first real inactive Library open/viewer
  caller; and
- existing `RuntimeMintCoordinator`, `bind_buy`,
  `RuntimeReleaseCoordinator`, `prepare_recipient`, `open_viewer_session`,
  `read_viewer_media_part`, `close_viewer_session`, `ProviderRegistry`,
  Wallet/Chain adapters, Profile authority, and the existing journals remain
  the composition seams that those call sites invoke directly.

The decrypt fixture decision-window repair is complete at `0a01ec3d`. It
changed tests only and proved the signed node-decision window must stay inside
the exact release-operation/request window.

### Pre-C0 — Runtime-owned custody composition

Status: complete at `554058cc`, `2064b556`, and `d9c4bef3`.

Entry:

- Slice A is complete/packageable;
- current inactive source proof is green; and
- routes remain inactive.

Bounded work and file groups:

- load and validate an operator-provided signed custody pool, epoch, and
  committee authorization;
- keep `NodePublicKey -> provider adapter/transport + owner-state-root`
  mapping private to Runtime;
- require exactly three distinct node keys, custody keys, operators, failure
  domains, owner-state roots, and provider adapter identities;
- use one Runtime `ProviderRegistry` with at most one local `custody` route and
  Runtime-selected per-node `ProviderInvocationTransport`;
- provide an owner-only Runtime mint journal root; and
- adapt `RuntimeMintCoordinator` signing so the live device signing key signs
  exact statements.

Exact exit:

- Runtime-owned custody composition exists without a second service, facade,
  coordinator, registry type, supervisor, journal type, route, or app; and
- capsules and signed custody contracts still see no route/topology.

Focused verification:

- focused Runtime custody-composition tests;
- identity-only journal assertions; and
- leak scans for route/topology/secret persistence.

### Pre-C1 — accepted clear-media input

Status: complete at `5fae54a2`. Library validates the directory and then
fails closed with the inactive-boundary error. Protect/mint dispatch is
Slice C.

Entry:

- Pre-C0 is green; and
- routes remain inactive.

Bounded work and file groups:

- define the protected creator input as one Library directory containing one
  validated clear fMP4 init and ordered clear fMP4 segments in one exact local
  layout;
- reuse the existing clear fMP4 parser surfaces for validation;
- keep plain `Publish` plain and separate; and
- replace/delete `protected_content_fixture` by giving the existing `Publish`
  operation one canonical explicit protection mode such as
  `protection: "runtime_custody"`, where absence remains ordinary plain
  publish.

Exact exit:

- protected mint makes no arbitrary-file claim;
- no transcoding is introduced;
- no clear bytes are sent to the content provider; and
- no legacy boolean, fallback, fixture field, or dual write remains.

Focused verification:

- focused Library/content protected-input tests;
- clear-layout validation assertions; and
- searches proving the fixture switch is gone from the production path.

### Pre-C2 — Runtime-internal chain policy resolver

Status: complete at `c81ad819`.

Runtime asks the existing `chain` provider for the exact configured
`RightsPolicyBodyV1` for one content identity and action. The operation is
denied as a capsule capability. There is no rights-provider process and no
caller-supplied policy body.

Exact exit:

- one configured policy source per action;
- Runtime recomputes the policy identity and rejects mismatch; and
- later Library mint can use this seam instead of a fixture policy.

Focused verification:

- `capsules/chain-provider` `resolve_protected_content_policy_*`;
- `runtime_resolve_rights_policy_recomputes_identity_and_rejects_mismatch`; and
- `provider_resource` denial of `chain.resolve_protected_content_policy`.

### Slice C — wire Library creator import/mint/list

Entry:

- Slice A is complete/packageable;
- current inactive source proof plus deterministic signed custody / Chain
  fixture seams are green;
- Pre-C0, Pre-C1, and Pre-C2 are green; and
- Slice E has not cut over. Provisional `drm` / `rights` / `key` / `decrypt`
  remain the live open/share path.

"Inactive" here means no product cutover: do not remove the provisional
surface, do not expose `protect` as a capsule capability, and do not accept
fixture authority. It does **not** mean Library keeps the Pre-C1 bail.
`protection.mode = "runtime_custody"` must actually run protect → mint →
verified availability and fail closed when operator composition, the device
key, protect, chain policy, or content availability is missing.

Bounded work and file groups:

- register `protect` on the same `ProviderRegistry` Library already uses;
- deny `protect` as a capsule capability, same as `custody` and
  `chain.resolve_protected_content_policy`;
- after `validate_runtime_custody_publish_input`, load Pre-C0 composition
  (fail closed if absent), protect the clear init/segments, resolve the
  Pre-C2 View policy for the new encrypted content identity, mint through
  `RuntimeMintCoordinator`, publish only the encrypted fixed-layout tree,
  and persist identity-only Library/mint facts;
- keep plain `Publish` plain;
- do not invent a second registry, coordinator, journal, facade, Create, or
  Store capsule.

Likely files:

- `elastos/crates/elastos-server/src/server_infra.rs`;
- `elastos/crates/elastos-server/src/provider_resource.rs`;
- `elastos/crates/elastos-server/src/library.rs`;
- `elastos/crates/elastos-server/src/protected_content_runtime.rs`; and
- focused creator tests.

Exact exit:

- the existing Library capsule drives creator import/mint/list through
  protect -> RuntimeMintCoordinator -> verified availability;
- only encrypted fixed-layout output is published;
- Library records/list/status expose identity and availability facts only;
- Runtime persists identity-only mint state and no CEK/raw share/sealed-share/
  ciphertext/clear-media bytes; real signed operator config is required for
  product execution and deterministic fixtures remain test-only; and
- no Create or Store capsule is introduced.

Focused verification:

- creator mint/list tests through the existing Library publish/status path;
- fail-closed tests without composition, device key, protect, or policy;
- availability/object-identity assertions; and
- journal and publish-record leak assertions.

Source proof is on this local branch. Named tests live with the Library
publish suite. Slice E has still not cut over.

### Slice D — wire Marketplace buy and Library open/viewer

Entry:

- creator mint is green on Library; Slice E has not cut over.

Bounded work and file groups:

- wire the existing Marketplace listing/detail/buy flow to the existing Wallet
  approval/transaction coordinator and Chain evidence path;
- wire the existing Library open/viewer flow to the existing Runtime release /
  decrypt path; and
- keep all viewer output on the single Runtime-selected protected-content path.

Likely files:

- Marketplace and Library protected-content route surfaces;
- direct existing protected-content composition seams in server/runtime;
- transaction-effect lookups and viewer open/read/close surfaces; and
- focused buy/open tests.

Exact exit:

- buyer is denied before purchase;
- the exact buy uses the real Wallet approval flow and real Chain result;
- the existing Marketplace listing/detail/buy flow and Library open/viewer
  flow compose the canonical protected-content seams directly; and
- no second authority path, route, or viewer is introduced.

Focused verification:

- inactive buyer/open tests using the real product adapters;
- denial-before-purchase, replay, and leak assertions; and
- no carrier/topology/public-secret exposure.

### Slice E — atomic cutover

Entry:

- Slices A-D are green while still inactive.

Bounded work and file groups:

- activate the new product route in one commit;
- remove the old provisional `drm` / `rights` / `key` / old `decrypt` startup,
  DTO, provider-resource/catalog, build/install/component/WCI/test/doc
  surfaces in that same commit; and
- keep no old/new protected-content route active together.

Likely files:

- `elastos/crates/elastos-server/src/server_infra.rs`;
- `elastos/crates/elastos-server/src/provider_resource.rs`;
- `elastos/crates/elastos-server/src/library.rs`;
- `elastos/crates/elastos-server/src/content.rs`;
- `elastos/crates/elastos-server/src/api/gateway_capsule_catalog/read_model.rs`;
- `elastos/crates/elastos-common/src/protected_content.rs`;
- component/install/publish surfaces; and
- old provider tests/docs.

Exact exit:

- the new product route is activated in one commit; and
- the old provisional `drm` / `rights` / `key` / old `decrypt` startup, DTO,
  provider-resource/catalog, build/install/component/WCI/test/doc surfaces are
  removed in the same commit.

Focused verification:

- absence searches proving the old path is removed;
- route/resource/provider registration tests;
- no fallback, compatibility decoder, dual write, dual authority, or second
  journal/registry/supervisor/coordinator; and
- cutover-set artifact parity checks.

### Slice F — minimum UI and installed one-Runtime/two-principal acceptance

Entry:

- atomic cutover is source-green.

Bounded work and file groups:

- keep the minimum UI on the existing Library, Marketplace, Wallet/Inbox, and
  one Runtime-selected viewer surfaces only;
- add the installed acceptance script and any direct test/support wiring needed
  to prove the product path; and
- avoid broad UI redesign, new capsules, or alternate viewers.

Likely files:

- the existing Library/Marketplace/Wallet/Home/browser proof surfaces;
- install/proof scripts;
- final protected-content docs; and
- focused installed acceptance tests or proof scripts.

Exact exit:

- one installed Runtime proves artifact parity for Runtime, `components.json`,
  and all provider binaries;
- creator imports one clear fMP4 asset, mints it, obtains verified
  availability, and lists it;
- buyer is denied before purchase, then uses the real Wallet approval flow and
  real Chain result;
- the listing becomes owned/available to the buyer;
- Library open drives 2-of-3 release -> viewer init/segment read -> close;
- wrong-object rejection, tampered-segment rejection, exact replay, Runtime
  restart, provider cleanup, and zero unresolved journals/sessions all hold;
  and
- no CEK/raw share/clear-media durability, topology/credential leakage, or
  bearer play URL appears in product-visible surfaces.

Focused verification:

- the installed one-Runtime/two-principal acceptance script; and
- focused negative/restart/cleanup assertions not already owned by lower layers.

### UI mapping for the first product proof

Minimum UI only:

- existing Library capsule for creator import/mint/list and open;
- existing Marketplace for listing/detail/buy;
- existing Wallet/Inbox approval flow; and
- one Runtime-selected viewer.

Do not invent Create or Store capsules, a second viewer, or a second
protected-content route.

### Acceptance matrix

The installed same-Runtime acceptance must prove:

- artifact parity for Runtime, `components.json`, and all provider binaries;
- creator imports one clear fMP4 asset, mints it, obtains verified
  availability, and lists it;
- buyer is denied before purchase;
- buyer uses the real Wallet approval flow and real Chain result;
- the listing becomes owned/available to the buyer;
- Library open drives 2-of-3 release -> viewer init/segment read -> close;
- wrong-object/media-binding rejection;
- tampered encrypted-segment rejection followed by a valid exact read on the
  original segment;
- exact replay and durable restart behavior;
- provider cleanup and zero unresolved Runtime/provider journals or sessions;
  and
- no CEK, raw share, clear-media durable persistence, topology, credential, or
  bearer play URL leakage.

### Known first boundaries

The first likely implementation boundaries on this line are:

- exact internal provider service names must be admitted by the existing
  `ProviderRegistry` allow-list before inactive wiring can call them;
- the old live decrypt scheme collides with the new decrypt path, so two
  authorities must not be parallel-registered under the live route; the new
  adapter is tested inactive first, then the old registration is swapped out
  atomically in Slice E;
- the custody-provider process-test wall-clock coupling is closed:
  `capsules/custody-provider/tests/process.rs` now uses the canonical
  60-second release-request maximum for valid operations, creates the
  success-path release request immediately before the
  contribution/replay/restart-replay phase, and creates a fresh exact request
  for the later wrong-signing phase instead of carrying one request across
  unrelated work. Verified evidence:
  `custody_provider_process_provisions_releases_replays_after_restart_and_shuts_down`,
  `elastos-protected-content-contracts::authority_tests::release_stays_inside_wallet_authority_window`,
  and
  `elastos-protected-content-contracts::rights::wrong_policy_and_expiry_fail_before_replay_claim`
  all passed, with separate expiry-negative coverage preserved and no sleeps,
  retries, or production clock changes;
- `Library` / `content` still depend deeply on the old protected-content DTO
  path and are the largest replacement surface;
- elastos-server process proofs require
  `ELASTOS_TEST_CUSTODY_PROVIDER_BIN`,
  `ELASTOS_TEST_PROTECT_PROVIDER_BIN`, and
  `ELASTOS_TEST_DECRYPT_PROVIDER_BIN` pointed at binaries built from this
  tree; stale local `target/` copies fail `InvalidOutput`; and
- installed acceptance is blocked unless the signed custody profile and the
  configured Chain contract plus two funded test accounts already exist.

### Functional completion

Functional completion for this line is one installed Runtime with two
principals passing the full acceptance above.

### Release review

Release gates after functional completion are:

- independent review of the PQ-hybrid share-wrap / recipient-wrap /
  authority-composition design;
- focused full gates on the cutover tree; and
- commit/review hygiene for the cutover series.

### Later pre-public-network gates

Later gates, not first cutover blockers:

- multi-Runtime issuer admission and cross-Runtime protected-content exchange;
- public pool governance; and
- any operator-independence claim stronger than signed configured claims plus
  the installed proof.

### Hard stop conditions

Stop rather than inventing around the problem if cutover work requires:

1. publishing clear media before protection or sending clear bytes to the
   content provider on the protected path;
2. keeping a fixture field or fixture authority in the production creator path;
3. letting a single node or single route masquerade as a real 2-of-3 custody
   set;
4. using test keys or deterministic signed fixtures in production;
5. changing a frozen public contract merely for routing;
6. exposing Carrier/topology in a capsule or public contract;
7. migration, fallback, dual authority, or dual write;
8. a second provider registry, supervisor, coordinator, or journal;
9. route/path/host/port/credential, CEK, share, ciphertext, or clear-media
   bytes entering Runtime or Library journals; or
10. proceeding without the signed custody profile or Chain contract config,
    starting Slice E cutover work early, or continuing below 15% free disk.

### Estimate

Plan for 4-7 focused working days plus review and the installed proof cycle.
This is not “two small tasks.”

## Verification

Narrow gates per slice. Do not full-workspace build unless the focused
dependency requires it and free disk stays above 15%.

```bash
git diff --check
node scripts/home-entropy-check.mjs
(cd elastos && cargo fmt --all -- --check)
(cd elastos && cargo test -p elastos-protected-content-runtime -- --nocapture)
(cd elastos && cargo test -p elastos-protected-content-rights -- --nocapture)
(cd elastos && cargo test -p elastos-server protected_content_runtime -- --nocapture)
```

Add focused tests only for touched crates. Scan responses/logs for
CEK/share/topology/fallback leakage. Do not push, merge, install, or deploy
without an explicit ask after showing log, diffstat, counts, and verification.
