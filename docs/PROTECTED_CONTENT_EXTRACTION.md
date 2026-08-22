# Protected-content extraction plan

Status: working plan for the permissioned mint → buy → open → play path.
This is not installed product truth. Released 0.6 still uses the provisional
`drm` / `rights` / `key` / `decrypt` surface.

Dirty `main` `TASKS.md` 0.7 text is unreviewed operator planning. Canonical
remaining work for this extraction is this document plus `TASKS.md` on
`origin/feat/protected-content-rights` and its children.

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

The 0.7 goal is one permissioned journey across two fresh Runtimes: publish,
mint, discover, buy, acquire, play. Content opens only at the declared
threshold of approved custody nodes. Apps never see the CEK. This is not public
dKMS, not a storage market, and not Browser completion.

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

Still not product-ready: not installed; inactive `custody` is registered and
reconcile is identity-bound; rights evaluation can invoke `chain` through the
registry without replacing live `rights`; share wrap is PQ-hybrid on this
unpublished tree; recipient possession and decrypt-session wrap are on this
tree; the Runtime mint journal commits 2-of-3 PQ-hybrid envelopes on this
unpublished tree only; lower-level buy/open/read/close seams are on this tree,
and records only provider/object/publisher-pinned, identity-only content
availability evidence after a private server publish/status/refetch check of
the fixed CENC directory before buy; active
Library still uses provisional providers; one process-backed inactive Runtime
success path already exists with three independently addressed custody-provider
processes, one decrypt-provider process, deterministic Wallet fixtures, and
deterministic Chain evidence fixtures; live Wallet/Chain process integration,
the two-principal matrix, cutover, UI, and installed proof remain open.

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
adapter negative-test *ideas* only, rewritten against the current crate.

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

Each slice is one unpublished `feat/protected-content-*` child of the previous
reviewed tip, starting from `origin/feat/protected-content-rights` `43a83e5b`.
Do not stack on `main`, dirty `main`, or `coordinator-v1`. Dependent-merge
warning applies until the parent is on `upstream/0.7-dev`.

### 0. Inactive Runtime integration — next slice

- Branch name: `feat/protected-content-runtime-integration`
- Base: `origin/feat/protected-content-rights` at `43a83e5b`
- Local `feat/protected-content-runtime-integration` continues this slice on
  the same base. Do not open a third D branch. Do not import `coordinator-v1`.

Goal: inactive Runtime provider lifecycle, registration, routing, audit, and
exact identity-bound reconciliation after provider effects. Use existing typed
Wallet, Chain evidence, rights evaluator, custody-provider, and Runtime
coordinator crates. A typed internal provider adapter is allowed only if it is
the same seam and does not create a second route.

Keep journal and reconciliation in `elastos-protected-content-runtime`. Server
may register and invoke through `ProviderRegistry` and scan unresolved journal
ids. Server must not become a second coordinator or grow public codec types.

Non-goals: mint UI, playback UI, atomic cutover, provider rewrites, public
codec-crate expansion, install/deploy, PR #15 replay, Iroh/Carrier, fallback,
dual authority, capsule-selected topology.

Success:

- Runtime registers and owns selected protected-content providers on an
  inactive seam. Canonical custody uses reserved name `custody`, never live
  `key`.
- Capsules cannot mint `elastos://custody/...` capability.
- Persist operation state before provider effects.
- Record `provider_effect_started` before the first effectful provider call.
- Crash or lost response after effect remains durable and nonterminal. No
  settlement from time, path absence, provider absence, or fallback.
- Reconciliation finishes only through an exact identity-bound provider
  receipt/result.
- Rights denial is terminal and calls no custody provider.
- Wrong provider result, node set, threshold, issuer, operation hash, receipt,
  or stale result fail closed.
- Exact terminal replay returns the stored result. Nonterminal replay does not
  silently redispatch.
- Responses and logs expose no CEK, raw share, route, endpoint, host, IP, port,
  path, or credential.
- Audit records the Runtime-owned sequence without leaking the above.

Stop: no safe lifecycle seam; reconciliation needs new public contract types;
must modify active Library/DRM/key/decrypt routes; caller-supplied provider
selection; Carrier changes required; unexpected dirty worktree or remote ref.

This slice is implemented on `feat/protected-content-runtime-integration`:
inactive `custody` registration, journal unresolved scan, identity-bound
`reconcile`, hash-only audit records, the node-local rights path that
invokes existing `chain` / `protected_content_rights_evidence` through
`ProviderRegistry`, PQ-hybrid share wrap, recipient possession plus
decrypt-session wrap, the Runtime mint journal with 2-of-3 provision, and
buy/open/read/close seams. Runtime mint tests now prove durable custody
provisioning, a private server publish/status/refetch availability verifier,
and one separate inactive test-provider mint -> availability -> buy -> open ->
read -> close composition.
Separate lower-level lifecycle and decrypt-provider process tests
prove PQ-hybrid contribution reconstruction,
exact CENC media reads, close replay, restart, and old-handle absence.
Separate Runtime restart/replay tests prove persisted terminal replay and
retained nonterminal state after effect start. The inactive server path now
also proves one process-backed alpha success plus beta terminal denial journey
with real Runtime mint/release coordination, three distinct custody-provider
processes, one protect-provider process, and one decrypt-provider process:
mint -> availability -> buy -> open -> init/segment read -> close for alpha,
plus fail-closed beta denial, wrong-recipient rejection, encrypted-segment
tamper rejection, and zero unresolved release state. That success path uses
`ContentAvailabilityTestProvider`, deterministic Wallet
request/response/purchase-effect fixtures, `ProcessChainEvidenceProvider`
fixtures, and a directly constructed signed release operation; separate focused
tests prove the passkey-bound Profile authorization/signing seam and Runtime
release-operation assembly. It does not itself prove a live Profile -> Wallet
-> Runtime process chain or a real Chain provider process. Provisional
`rights` stays until atomic cutover. The next source task is narrower: extend
that process-backed proof to the remaining combined-path two-principal
negative/restart/crash matrix beyond the now-proven success + denial paths.
Only after that passes: atomic cutover. Do not import `coordinator-v1`.

### 1. PQ-hybrid envelope — before any mint

Share wrap on this unpublished tree uses
`elastos-xwing-draft06-hkdf-sha256-aes256gcm/v1`: X-Wing draft-06
(`ml-kem-768` plus `x25519`) with HKDF-SHA256 and AES-256-GCM. This is a
source-only permissioned draft, not an RFC-based product claim. Missing either
KEM component fails closed. Node and recipient wrap identities are hybrid
public keys, not X25519-only. Authority signatures on this tree remain
classical and are not claimed quantum-safe. External cryptographic review is
still required. This is still not a product mint path. Do not list or open
product Library objects until inactive e2e is proven. The unpublished mint
journal can commit PQ-hybrid envelopes without making those objects a live
catalog path.

Mine remaining `ddrm-envelope` negative tests as needed. Do not mint
classical-only objects. Do not keep a PQ-off default or dual decoder.
External audit remains required before public dKMS claims. Permissioned 0.7
still uses this profile from the first mint.

### 2. Recipient possession and decrypt-session wrap

Profile signature authorizes one public key; it is not possession. The decrypt
provider generates a fresh operation-scoped PQ-hybrid recipient key and retains
its secret behind an opaque handle. The Profile must sign authorization for that
exact public key, binding, action, recipient identity, Runtime issuer, and time
window; no Profile seed enters Runtime, custody, or decrypt-provider contracts.
The provider requires a PQ-hybrid challenge/response against that exact public
key before reconstruction, and the public reconstruct path returns the CEK only
inside a PQ-hybrid decrypt-session wrap. The server has a narrow passkey-bound
Profile authority seam that signs only the canonical recipient authorization,
then assembles and verifies the separately device-signed Runtime release
operation. It exposes neither Profile seed nor private key and owns no replay;
the Runtime release journal remains the sole replay and settlement owner.
Focused wiring tests cover the protected Profile, proof binding, existing device
key, output verification, and duplicate side-effect freedom. This is still not
a mint, list, or open path.

### 3. Mint journal, 2-of-3

On this unpublished tree the Runtime-owned producer journal binds one media
flow: encrypted-content identity, PQ-hybrid envelope identity, pool, epoch,
committee, node set, 2-of-3 threshold, CEK commitment, and policy. It
provisions one sealed share per selected node through existing custody-provider
contracts and does not persist CEKs or share bytes. Custody provisioning is not
availability. The Runtime records an identity-only verified availability fact
only after the private server adapter publishes the fixed CENC descriptor/init/
indexed-segment directory, reads the existing `elastos://content` provider's
signed receipt, and refetches the generic object. The result is pinned to the
selected provider, object, and publisher identities and canonical protected
CENC object: exact media identity, policy, replica requirement, and freshness. Custody
threshold is separate from availability replica policy. Partial provision is a durable terminal abort. Restart replays
exact terminals; uncertain post-effect state stays nonterminal. First-release orphan policy is bounded retention: accepted shares
stay unreachable by any valid release until a separately reviewed retirement
operation exists. The first proof uses three distinct node identities, three
owner-only state roots, and distinct operators/failure-domain claims. A
one-node path is rejected. Those signed configured claims are not physical
independence proof; the process proof supplies distinct processes, provider
identities, and owner-only state roots. This is still not product list/open/play
and does not replace live `key`/`rights`/`drm`/`decrypt`.

### 4. Buy / open / read / close

Wallet signs the exact approved action. Chain supplies typed rights evidence
through the existing durable transaction coordinator. Runtime coordinates
private reconstruction and scoped viewer output. No bearer `play_url`. Home
launch tokens stay HTTP-edge credentials. Play uses CENC/`cenc-core` behind
decrypt after the PQ-hybrid CEK exists only there.

On this unpublished tree: Runtime `bind_buy` rejects a custody-provisioned mint
without that exact signed availability evidence, then accepts the exact
Wallet/Chain-bound purchase after it is recorded. An inactive test-provider
composition proves mint -> availability -> buy -> open -> init/segment read ->
close. A separate process-backed success path now proves the same
mint -> availability -> buy -> open -> init/segment read -> close sequence for
one principal through three real custody-provider processes, one real
protect-provider process, and one real decrypt-provider process, plus
wrong-recipient rejection, bit-mutated encrypted-segment rejection, byte-exact
clear init/segment recovery, and zero unresolved release state. That proof
still uses deterministic Wallet
request/response/purchase-effect fixtures, `ProcessChainEvidenceProvider`
fixtures, and a directly constructed signed release operation; the passkey-
bound Profile authorization/signing seam and Runtime release-operation
assembly are proved separately. The lower-level `open_viewer_session` contract still requires an exact buy
receipt, typed decrypt contract, and opaque handle; bearer `play_url` and Home
launch tokens are rejected. CENC AES-128-CTR runs only after PQ-hybrid
decrypt-session CEK unwrap in the decrypt-provider process path. Live
`decrypt` is unchanged. The full two-principal negative/restart/crash matrix
and atomic cutover remain separate gates.

### 5. Process-backed inactive e2e, then atomic cutover

Runtime now loads a pinned signed pool, epoch, and committee, calls
`validate_custody_epoch_against_pool_at`, and resolves exactly those three
approved node identities to Runtime-selected provider instances in canonical
committee order. Provider candidates carry only node/custody keys, an opaque
owner-state-root commitment, and an in-memory provider reference; signed pool
operator/failure-domain claims remain policy, not physical-independence proof.
There is no election, random or "latest" selection, topology, or caller-supplied
node list.

Rights evaluation is now node-local on the inactive path. The node-hosted
provider retains its own signing key, evaluates the immutable rights request
through its node-local provider/chain context, and returns only the exact
signed node decision. Runtime selects the node through the ProviderRegistry but
never loads, receives, or passes a node signing key. This uses the existing
`custody` route and ProviderRegistry plane, not a second rights route,
registry, supervisor, or transport contract.

Next, keep the current Runtime-selected rights/custody/protect/decrypt path
and deterministic Chain evidence, then extend the combined process proof only
for the remaining matrix that is not already owned by focused lower-layer
tests: full two-principal coverage beyond the now-proven success + denial
paths, wrong object, and restart/crash/cleanup. If a later gate needs a live
Profile/Wallet/Chain process proof, add it explicitly rather than inferring it
from the current fixture-led success path. The configured
operator/failure-domain claims do not prove physical independence; the process
proof must establish distinct processes, provider identities, and owner-only
state roots. The first permissioned proof pins one local Runtime device
operation issuer; multi-Runtime issuer admission remains a later
pre-public-network gate.
Then delete provisional `elastos_common::protected_content`, `drm-provider`,
`key-provider`, and provisional decrypt in one slice. No compatibility decoder.

On this unpublished tree the layered proof already exists in parts: durable
custody provisioning, provider-pinned availability evidence, an inactive
test-provider composite, lower-level Runtime lifecycle checks, a
decrypt-provider process path with PQ-hybrid reconstruction and CENC reads,
restart/replay semantics, a real producer-side CENC protection process path,
one process-backed inactive Runtime success path, and a separate second-
principal denial proof through the same real custody-provider processes. What
remains is the rest of the combined two-principal negative/restart/crash
matrix and atomic cutover. Live `decrypt` is unchanged.

### 6. Minimum UI and installed two-principal proof

Create, Store listing/detail/buy, Library open, Wallet/Home approval, one
Runtime-selected viewer. After the authority path is green. Not PR #23.

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
