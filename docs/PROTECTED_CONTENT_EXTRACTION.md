# Protected-content integration plan

Status: current source-only plan for the permissioned mint -> buy -> open ->
play path. Released 0.6 still uses the provisional `drm` / `rights` / `key` /
`decrypt` surface. The newer Runtime-owned path is inactive, not installed, and
not cut over.

## Why the Runtime / provider / custody / capsule split exists

The split is deliberate and remains the only acceptable architecture for this
work:

- One canonical path per operation. No dual decoder, no classical-only mint
  mode, no PQ-off default, no second coordinator.
- Fail closed. Missing receipts, missing KEM material, lost provider responses,
  caller-selected nodes, or ambiguous post-effect state are explicit failures.
- Capsules are never authority. They do not receive CEKs, raw shares, routes,
  hosts, IPs, ports, credentials, Wallet RPC, Chain RPC, Kubo/IPFS APIs, or
  bearer playback URLs.
- Runtime owns authenticated selection, lifecycle, audit, and settlement.
  Providers own typed operation semantics. Carrier only transports
  Runtime-selected endpoints.
- Encrypted objects stay on the normal content path. Rights, custody release,
  and decrypt stay behind typed providers.
- Keep the trusted core small. Do not grow public codec crates, Library routes,
  or provisional DTOs just to make inactive seams compile.

The first honest product proof remains one installed Runtime with two human
principals: creator mint/list, buyer purchase, buyer open/play. Content opens
only after the configured threshold of approved custody nodes contributes. Apps
never see the CEK. This is not a public dKMS claim, not a storage-market claim,
and not Browser completion.

## Source of truth

Published review stack. Use these exact reviewed sources; do not invent a
parallel stack.

| Branch | Commit | Tree | What it proves |
|---|---|---|---|
| `origin/feat/protected-content-contracts` | `1865595f3fdd46336526fd057ce7091c417c9811` | `5d70c98ba32496ce1f71cd0cacd67fc560961b40` | Canonical content identity, owner-bound rights grant, bounded authenticated release/contribution/terminal contracts, and threat model. |
| `origin/feat/protected-content-custody` | `29879b25d7c527aece3d3664222e6dd3b3957422` | `0ffd7f0282982c33b01f4e5c28f8db3aa7f4cdbe` | Custody envelopes, share sealing, exact-threshold reconstruction, policy/recipient authority, epochs/operations, and durable replay. |
| `origin/feat/protected-content-key-reconstruction` | `a8d0a9a6beff8b963e54f224af07205fddda0a76` | `5eff01c3380b5da4d0214430c0ebd6be60c7e2fe` | Authenticated contribution collection and the private threshold reconstruction bridge. |
| `origin/feat/protected-content-custody-provider` | `f7cd6c3dfe4fc3f6899c88af3ee4c082b49e3a49` | `b9b93460f66b94759100b5993887e24c599e27a8` | Object/pool/epoch/committee binding; one node, one sealed share; Runtime issuer pinning; owner-only node state; duplicate/conflict/restart; rights-gated release; encrypted contribution replay; bounded frames; strict shutdown. |
| `origin/feat/protected-content-wallet-rights` | `2c69d0c2af00f7050faa424d3f7d6f4e41a92a9a` | `73764e9aef98bf7ea7e24989ce051594a05a71ac` | Wallet signs exact canonical `RightsRequestV1`. Generic approval cannot substitute. |
| `origin/feat/protected-content-runtime` | `b00bfeeb894033559239b7b438b5558cab900b4d` | `7b8b1945471e86859b56e6175d8ba35330af5e48` | Private durable release journal and typed internal coordination. Persist before effects; record `provider_effect_started`; replay only exact terminals; ambiguous post-effect outcomes stay nonterminal. |
| `origin/feat/protected-content-rights` | `43a83e5bd405820713bb88d4e32950b5bfa26ccb` | `34e5bb5379268419ff8c5b4dc97cc0631d70c2b3` | Typed Chain rights evidence and rights evaluator. Capsule-facing old boolean rights rail is gone. Provisional `rights-provider` stays until cutover. |
| `origin/feat/protected-content-runtime-lifecycle` | `34465959` | `9f32e0b70c2c5311f49b61424e00ea9305af2ce6` | Published prefix for the inactive Runtime lifecycle line. The current `feat/protected-content-runtime-lifecycle` branch continues that source line with the creator-listing, purchase, viewer-lifecycle, unbound-KID, and closeout work described below. |

The current `feat/protected-content-runtime-lifecycle` source line now proves
the inactive Runtime-owned mint -> availability -> creator mint/list -> buy ->
open -> 2-of-3 release -> decrypt -> close path in source. That proof does not
change the installed product path.

## Current inactive source path

Current source proof already covers:

- creator-side CENC protection;
- exact provider/object/publisher-pinned signed availability verification;
- Runtime-owned creator mint/list settlement from immutable terms;
- Runtime-owned buy with finalized multi-source access corroboration;
- durable viewer open/read/close lifecycle;
- real protect, custody, and decrypt provider-process integration on the typed
  inactive path;
- wrong-recipient rejection;
- wrong-object and wrong-media-binding rejection;
- encrypted-segment tamper rejection;
- exact durable replay;
- explicit provider unregister/absence cleanup; and
- zero unresolved Runtime/provider state in the inactive combined proof.

Lower-layer restart/crash/cleanup ownership remains in the existing focused
tests:

- `capsules/custody-provider/tests/process.rs::custody_provider_process_provisions_releases_replays_after_restart_and_shuts_down`
- `capsules/protected-content-decrypt-provider/tests/process.rs::process_prepare_open_read_close_replay_and_restart_absence_flow`
- `elastos/crates/elastos-protected-content-runtime/src/journal.rs::durable_state_replays_only_persisted_terminal_result`
- `elastos/crates/elastos-protected-content-runtime/src/coordinator.rs::runtime_coordination_replays_terminal_without_dispatch`
- `elastos/crates/elastos-protected-content-runtime/src/mint.rs::restart_after_effect_started_stays_nonterminal`
- `elastos/crates/elastos-protected-content-runtime/src/mint.rs::custody_provisioned_replays_without_redispatch`

The inactive combined source proof is currently exercised by:

- `elastos/crates/elastos-server/src/protected_content_runtime/tests.rs::protected_content_availability_publishes_status_refetches_and_verifies_exact_media`
- `elastos/crates/elastos-server/src/protected_content_runtime/tests.rs::runtime_custody_registry_adapter_process_happy_path_uses_public_provision_receipt`
- `elastos/crates/elastos-server/src/protected_content_runtime/tests.rs::runtime_release_coordinator_process_two_of_three_success_stops_before_third_node`
- `elastos/crates/elastos-server/src/protected_content_runtime/tests.rs::runtime_decrypt_registry_adapter_process_reconstructs_for_prepared_recipient_and_closes_cleanly`

This is source proof only. It is not installed-product proof, not live
deployment evidence, and not a confidentiality claim for the active product.

## Private material and leak boundaries

`CustodyEnvelopeV1` is current source-only inactive Runtime open/provisioning
material stored owner-only at
`protected-content/runtime-open/{mint}/envelope.bin`. It is private
open/provisioning material, separate from the identity-only mint journal and
separate from public metadata. Capsules cannot read it. Runtime cannot open the
node-sealed shares inside it. Each selected custody provider persists only its
own raw share. Runtime and public state never persist a raw CEK or raw share.

Public metadata contains bounded identities, threshold/epoch/pool facts, CEK
commitment, and signatures only. Product operations, responses, logs, public
metadata, and durable product state must never contain raw CEKs, raw shares,
clear-media bytes, private routes, RPC topology, or credentials.

## Source-only helper and contract facts

Current source also defines these exact source-only helper and contract facts:

- `elastos-protected-content-custody` provisions canonical custody envelopes,
  binds them to a domain-separated CEK commitment, and uses the pinned
  PQ-hybrid confidentiality suite
  `elastos-xwing-draft06-hkdf-sha256-aes256gcm/v1`. Missing either KEM fails
  closed. There is no classical-only product envelope, no PQ-off default, and
  no dual decoder.
- Recipient key authorization is a Profile-signed authorization object only. It
  binds one exact provider-generated recipient public key to one exact Runtime
  operation and one exact time window. It does not prove secret-key possession.
  Possession remains inside the decrypt boundary.
- The decrypt boundary reconstructs and uses the CEK only inside the scoped
  session, then zeroizes it.
- Typed Chain rights evidence uses exact finalized observations over explicit
  protected-content RPC sources. There is no caller-supplied rights fact, no
  generic fallback to `network.rpc_url`, and no topology exposure.

## Evidence-only branches and rejected drafts

### `feat/protected-content-runtime-coordinator-v1`

Do not continue it and do not cherry-pick it. It mixes stale decrypt wire,
public plaintext chunk types, dependency drift, and an obsolete server module
shape. Keep it only as evidence for later negative-test ideas rewritten against
the current crate surface.

### PR #15 / `feat/dkms-esp-port`

Keep as research only:

- threshold crypto;
- node-local custody;
- recipient-sealed contributions;
- CEK commitment;
- lifecycle scenarios; and
- fail-closed negative tests.

Reimplement at canonical boundaries:

- per-node durable shard storage;
- DKG/rotation/re-share/revocation;
- pool/governance policy;
- provider roles; and
- Runtime-open scenarios.

Reject from the product path:

- public aggregated `shares[]` metadata;
- capsule-owned authority;
- raw CEK operations;
- `rail_shim` and reference fallbacks;
- old `drm-provider` orchestration;
- direct topology in capsules or contracts;
- static authorization fallbacks; and
- the standalone harness as a product route.

PR #15 already requires exactly one `covered_address` equal to the recovered
owner. `RightsRequestV1` already rejects an attacker-signed victim wallet. Keep
both. Do not reintroduce a covered-address list.

## Confirmed deployed-contract facts

Latest confirmed PR #15 review facts for the Base read path:

- AuthorityGateway access reads use
  `hasAccessByContentId(address holder, bytes16 contentId) -> bool`.
- The exact bytes16 value is the CENC KID and remains separate from the full
  encrypted-content identity.
- AuthorityGateway resolves that KID through
  `CentralStorage.ipReference(bytes16)`.
- Unknown/unbound KIDs revert with exact custom error
  `UnboundContentId(bytes16)` / selector `0xcad88223`.
- Bound KIDs without access return `false`.

All previously open questions are closed by the ELACITY-2296 on-chain
verification (2026-08-28, read-only probes against the deployed Base 8453
proxies; `v3-drm-protocol` is the authoritative contracts repo — the older
`drm-contracts` deployment is deprecated and must not be used for ABI or
authorization conclusions):

- KID binding is `CentralStorage.bindIP(bytes16 contentId, address channel,
  uint256 tokenId)`, callable only by acknowledged protocol contracts
  (`whitelistOnly`; unauthorized callers revert
  `UnrecognizedContractError(address)` / `0x552b3ecd`). Its sole caller is
  `AssetFactory.registerNewAsset` inside the mint flow; each binding emits
  `IPBound(bytes16 indexed, address indexed, uint256 indexed)` (topic0
  `0x69eafb02…`). Verified live: `acknowledged(AssetFactory) == true`,
  `acknowledged(<random EOA>) == false`, simulated EOA `bindIP` reverts, and
  97 `IPBound` events exist on the deployed `CentralStorage`.
- Canonical purchase state DOES require Runtime to issue
  `AuthorityGateway.buyAccess`: access resolves only through ERC-1155
  `ACCESS_TOKEN` (id 1) / `DISTRIBUTION_RIGHT` (id 3) balances on the
  per-asset operative — moved only by the buyAccess trade path — or an active
  channel subscription. ABIs confirmed: native
  `buyAccess(address seller, address ledger, uint256 tokenId, uint256
  _quantity, uint256 _pricePerToken)` payable = `0xf7580ad9`; ERC-20 adds
  `address _payToken` (non-payable, rejects the zero sentinel) =
  `0x0ede2294`, and requires a prior `approve` to the per-operative
  `paymentProcessor()` (`0xf1c6bdf8`), never to the gateway. The pinned
  `AssetCreated` topic0 `0xc0a995e4…dba46` is the EventHub shape (3 indexed
  topics); `mint.asset_created_emitter` must be the EventHub proxy, not the
  channel.
- Deployed contracts store a single boolean per `(holder, contentId)` — no
  per-right state exists on-chain. The two token ids `checkAccess` reads are
  not consumption rights: `ACCESS_TOKEN` (id 1) carries playback/read access
  (and by extension download); `DISTRIBUTION_RIGHT` (id 3) is the right to
  sell/distribute the access token. `View` / `Download` (and any per-right
  granularity) live exclusively in the signed Runtime rights policy.
- Bound-KID proof (Base 8453, gateway
  `0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D`): KID
  `0x2c27d859924f93f14aa8071f7ba8192e` → allowed wallet (minter)
  `0x34DAF31B99B5A59cEB18E424Dbc112FA6e5f3Dc3` returns `true`; random wallet
  `0x1111…1111` returns `false`; an unbound KID reverts with
  `0xcad88223` + KID payload. Note: access can also be satisfied by channel
  token-ownership gates (`TokenAccessRegistered`), so an "open" channel can
  legitimately return `true` for arbitrary wallets — deny proofs must use a
  gated channel.
- Creator royalty: `0x3b6` = 950 ERC-1155 `ROYALTY_SHARE` (id 2) units.
  1000 units exist per asset — creator 950 (95%) plus the protocol owner's
  `CentralStorage.protocolShares()` = `(50, 0xdB0E70C1…)` (5%), both
  observed live on a deployed operative's balances.

## Installed prerequisites

Before installed proof can honestly pass, the operator must supply:

1. one signed owner-only three-node 2-of-3 custody composition at
   `{data_dir}/protected-content/custody-composition.json`, with signed pool,
   epoch, committee authorization, expected policy authority, expected
   committee identity, and exactly three node routes keyed by node public key
   and owner-state root;
2. one private multi-source Chain configuration on the existing chain-provider
   path, including the exact protected-content policy source and the exact
   finalized read/evidence configuration;
3. packaged protect + custody + decrypt provider registration through the
   existing component/install/source-home path;
4. one installed three-replica availability proof, plus repair-after-one-loss
   proof; and
5. funded test accounts for the existing Wallet approval/transaction
   coordinator and Chain evidence path.

Deterministic source fixtures do not satisfy any of those installed
prerequisites.

## Remaining order

The remaining work is now straightforward:

1. publish and review the current source-only branch without widening installed
   behavior;
2. prove the remaining deployed-contract facts listed above;
3. package and register protect + custody + decrypt on the existing installer /
   ProviderRegistry path with real signed custody composition and private
   multi-source Chain config;
4. run the installed inactive proof with three real replicas, including
   repair-after-one-loss;
5. land one atomic cutover that removes the provisional `drm` / `rights` /
   `key` / `decrypt` authority with no fallback, dual route, compatibility
   decoder, dual write, or dual authority; and
6. prove the installed one-Runtime / two-principal mint -> buy -> play
   acceptance path.

Later public-network gates remain separate from this first product path:
multi-Runtime issuer admission, cross-Runtime protected-content exchange,
public-pool governance, and any operator-independence claim stronger than
signed configured facts plus installed proof.

## Atomic cutover

The cutover must happen in one commit:

- activate the Runtime-owned protected-content route;
- remove the provisional `drm` / `rights` / `key` / old `decrypt` startup,
  DTO, provider-resource, catalog, build, install, component, WCI, test, and
  doc surfaces in that same commit; and
- keep no fallback, no compatibility decoder, no dual route, no dual write, no
  dual authority, and no second registry, supervisor, coordinator, or journal.

## Installed acceptance

Installed same-Runtime acceptance must prove:

- artifact parity for Runtime, `components.json`, and all provider binaries;
- creator imports one clear fMP4 asset, protects it, obtains verified
  availability, mints it, and lists it;
- buyer is denied before purchase;
- buyer uses the real Wallet approval flow and real Chain result;
- the listing becomes available to the buyer without mutating immutable creator
  listing bytes;
- Library open drives 2-of-3 release -> viewer init/segment read -> close;
- wrong-object/media-binding rejection;
- tampered encrypted-segment rejection;
- exact replay and durable restart behavior;
- provider cleanup and zero unresolved Runtime/provider journals or sessions;
- no CEK, raw share, ciphertext, clear-media, topology, credential, or bearer
  playback URL leakage; and
- installed proof with one Runtime and two distinct principals.

## Hard stops

Stop rather than invent around the problem if any remaining step requires:

1. publishing clear media before protection or sending clear bytes to the
   content provider on the protected path;
2. keeping fixture authority in production creator, purchase, or open paths;
3. letting a single node or single route masquerade as a 2-of-3 custody set;
4. using test keys or deterministic signed fixtures in production;
5. changing a frozen public protected-content contract merely for routing;
6. exposing Carrier or topology in a capsule or public contract;
7. migration, fallback, dual authority, or dual write;
8. a second provider registry, supervisor, coordinator, or journal;
9. route/path/host/port/credential, CEK, share, ciphertext, or clear-media
   bytes entering Runtime or Library journals; or
10. proceeding without the signed custody profile or Chain contract config, or
    continuing below 10% free disk.

## Verification commands

Use narrow gates for touched surfaces. Do not claim installed readiness from
source tests alone.

```bash
git diff --check
node scripts/home-entropy-check.mjs
(cd elastos && cargo fmt --all -- --check)
(cd elastos && cargo test -p elastos-protected-content-runtime -- --nocapture)
(cd elastos && cargo test -p elastos-protected-content-rights -- --nocapture)
(cd elastos && cargo test -p elastos-server protected_content_runtime -- --nocapture)
```

Add only the focused tests required by the touched surface. Scan outputs for
CEK/share/topology/fallback leakage. Do not push, merge, install, or deploy
without an explicit ask after showing the exact diffs and verification results.
