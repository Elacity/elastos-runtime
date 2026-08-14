# Protected content

Protected content is Runtime-mediated. App, viewer, and content capsules ask to
open an object; they do not receive raw wallet, chain, IPFS, Elacity, or key
authority.

The source tree currently has two protected-content layers:

- The canonical v1 source-only review stack is the
  `elastos-protected-content-contracts` crate plus the companion
  `elastos-protected-content-custody` crate, documented in
  [Protected-content v1 contracts](PROTECTED_CONTENT_CONTRACTS_V1.md). That
  stack now defines canonical authority bindings, typed rights-policy and
  evidence contracts, Profile-signed recipient-key authorization, signed
  immutable custody epochs, a typed Runtime-to-release-node operation
  envelope, a local durable dual-key replay-claim store for custody nodes,
  a claim-gated node release path, and source-only custody helpers for
  custody-envelope provisioning, recipient-sealed node release, and
  recipient-side threshold reconstruction for new content. It is not yet wired
  into Runtime orchestration, provider integration, Runtime-owned replay
  storage, recipient key-possession proof, decryption, playback, installation,
  or deployment.
- The current installed/provider surface is the older provisional
  `elastos_common::protected_content` DTO set plus the fail-closed
  `drm-provider`, `rights-provider`, `key-provider`, and `decrypt-provider`
  capsules. That provisional surface does not consume or prove the new v1
  contract. Future integration must replace it atomically, with no parallel
  decoder, fallback, or migration path.

The current provider chain is:

`capsule -> runtime capability -> elastos://drm/open -> drm-provider -> rights/key/decrypt providers`

## Current provider slice

The repo currently has the provisional fail-closed provider boundary, not
production DRM:

- shared provisional protected-content schemas in `elastos-common`
- `drm-provider` registered as `elastos://drm/*`
- `rights-provider` registered as `elastos://rights/*`
- `key-provider` registered as `elastos://key/*`
- `decrypt-provider` registered as `elastos://decrypt/*`
- `status` advertises the blocked raw-authority list and canonical open
  sequence, including Runtime-owned receipt and audit steps
- `open` validates sealed-object requests and fails closed with the same
  machine-readable required sequence until rights, key, and decrypt providers
  exist
- `open` rejects key envelopes without approved algorithm metadata
- `rights-provider` validates typed access/subscription questions and fails
  closed until a dDRM/chain policy backend is configured
- `chain-provider` exposes typed `has_access_by_content_id` reads that validate
  inputs and only call configured contract selectors
- `key-provider` validates key-release requests and algorithm-agile key
  envelopes, then fails closed until a dKMS backend is configured
- `decrypt-provider` validates scoped decrypt/render session requests and fails
  closed until decrypt/render backends are configured
- `content-provider` rejects incomplete `sealed` object publishes before IPFS:
  `sealed.json`, payload, rights policy, availability receipt, provenance, and
  approved key-envelope algorithms are required

This is intentional. The first safe steps are to make the authority and
custody boundaries unambiguous in the source-only v1 crates and keep the
current provider chain fail closed until the reviewed integration slice exists.

PC2's dDRM contracts and WASM decrypt/render/media crates are useful
implementation references. They should enter Runtime only as provider-internal
backends behind `rights-provider`, `key-provider`, and `decrypt-provider`; they
must not give app or viewer capsules raw CEK, wallet, chain, IPFS, or Elacity
authority.

## dDRM Decrypt Rail Options And Recommendation

The v0.4.0 provider chain intentionally proves the fail-closed sockets:

`drm-provider -> rights-provider -> key-provider -> decrypt-provider`

The remaining architecture decision is how the live CEK reaches the decrypt
boundary once real decryption is wired. The recommended default is a
sealed-material rail:

- Runtime orchestrates the normal provider chain through `drm`, `rights`, and
  `key`.
- `decrypt-provider` creates a per-session one-time public key for the decrypt
  sandbox.
- `key-provider` or the dKMS release backend seals the CEK to that one-time
  decrypt-session public key, using a separately reviewed decrypt-handoff
  suite.
- The decrypt step receives sealed material in the decrypt-session request,
  unwraps it inside the sandbox, decrypts/renders, zeroizes the live CEK, and
  returns only scoped output.
- `decrypt-provider` does not pull keys by making outbound capability calls.
  The component that briefly sees the live CEK must have the smallest possible
  authority surface.

This keeps the rights/key/decrypt separation while avoiding an outbound
authority grant to the highest-risk boundary. The current source-only custody
crate already defines canonical custody envelopes plus stored-share and
released-share sealing for new content, but the Runtime/provider handoff to a
decrypt boundary is still future integration work.

The new Profile-signed recipient-key authorization in the v1 contract is an
authorization object only. It binds one exact recipient public key and one
exact Runtime operation issuer for one binding/action/session/time window. It
does not prove X25519 secret-key possession. Actual holder-only possession
remains a later Runtime-owned invariant.

Other options were considered and rejected as the normal Runtime path:

| Option | Shape | Assessment |
|--------|-------|------------|
| Decrypt pulls keys | `decrypt-provider` calls `key-provider` or a key backend after authorization | Flexible, but grants outbound authority to the boundary that briefly holds live CEK. Keep only for controlled diagnostics or explicit capability-gated adapters. |
| One combined key/decrypt provider | Key release and decrypt/render run in one provider | Simpler CEK path, but collapses authority separation and increases blast radius. Useful for tests, not the target trust boundary. |
| Runtime relays raw CEK | Runtime receives CEK and passes it to decrypt | Not acceptable. Runtime would become a key-material holder instead of an orchestrator. |
| Runtime relays sealed material | Runtime passes a sealed envelope without being able to open it | Acceptable if the envelope is transcript-bound and Runtime never sees raw CEK. This is the practical form of the recommended rail. |
| dKMS direct-to-decrypt sealing | dKMS seals directly to the one-time decrypt-session key | Best target when available. `key-provider` brokers policy and receipts without seeing raw CEK. |
| Lit/Chipotle backend | Vendor-backed key release returns a CEK envelope, as PC2 does today | Useful compatibility backend only. The Runtime contract must remain backend-neutral and must also support an ElastOS-native dKMS path. |

Before live decrypt is enabled, the sealed material envelope must bind the full
transcript: principal, session, object, action, viewer interface, output kind,
expiry, release receipt hash, decrypt-session public key, envelope algorithm,
and provider identity. It must use nonce-safe authenticated encryption, signature
verification, replay rejection, short expiry, zeroization, and audit. PC2's
current `ddrm-decrypt` WASM pattern proves the containment invariant, but its
P-256 and Lit/Chipotle details are implementation references rather than
Runtime product truth.

## Current source-only custody helper

The `elastos-protected-content-custody` crate is a source-only helper for new
protected content:

- It provisions canonical custody envelopes from the reviewed v1 contract.
- It binds each custody manifest to a domain-separated CEK commitment, then
  checks that commitment after threshold reconstruction before returning an
  opaque content-key wrapper.
- It uses one pinned recipient-sealing suite for stored and released shares:
  RFC 9180 base mode X25519 + HKDF-SHA256 + AES-256-GCM.
- It rejects noncanonical and low-order X25519 contract key bytes as a stricter
  local canonical-identity rule before HPKE use. This is stronger than the RFC
  7748 primitive requirement; it is not claimed as an RFC mandate.
- It uses GF256 Shamir splitting through `vsss-rs` for new content only.
- Released terminal settlement and recipient reconstruction require exactly the
  bound threshold count. Required-plus-one contributions are rejected.
- It returns only opaque, redacted secret wrappers; it does not expose a
  capsule-visible raw-key API.
- It is source-only. There are no running custody nodes, Runtime/provider
  routes, Runtime-owned replay orchestration, recipient key-possession proof,
  decrypt/render product flow, installation, or deployment in this branch. The
  current local child line adds one node-local durable dual-key replay-claim
  store and the private claim-gated release transition only. That store adds a
  domain-separated integrity digest to detect same-length corruption or torn
  local state, but it does not defend against a malicious same-UID rewrite
  that can recompute the digest.

This helper does not make a PQ claim. PQ-hybrid custody, dKMS node lifecycle,
and product wiring remain future work. The current HPKE dependency is `hpke`
0.13, whose upstream documentation says it has not been formally audited. This
branch therefore makes no external cryptographic audit claim. The manifest
commitment only detects that threshold reconstruction produced the wrong key; it
does not identify the malicious node and it is not verifiable secret sharing.

## Current source-only operational contracts

The reviewed v1 contract stack now also defines the source-only operational
contract layer that later Runtime and custody-node integrations must consume:

- `RightsPolicyBodyV1`, `RightsEvaluationEvidenceRequestV1`, and
  `RightsEvaluationEvidenceV1` provide one narrow typed EVM policy/evidence
  shape grounded in the current `chain-provider`
  `has_access_by_content_id` method only: exact Wallet-derived subject,
  `chain_id`, contract bytes, selector, ABI identity, `content_id`, one exact
  contract right string, the product action that policy maps to that right,
  one bounded confirmation rule, and exact observed block/result evidence.
  They do not carry RPC URLs, provider labels, or routes.
- `SignedRecipientKeyAuthorizationV1` lets the authenticated Profile authorize
  one exact recipient public key and one exact Runtime application-operation
  issuer for one protected-content binding, action, session, and time window.
- `SignedCustodyEpochV1` signs one immutable custody epoch over the exact node
  signing keys, node custody keys, deterministic coordinates, threshold,
  approved suites, and issuer key. Existing envelopes cannot silently inherit a
  new epoch.
- `SignedRuntimeReleaseOperationV1` is the typed application-authenticated
  Runtime-to-release-node envelope above Carrier transport evidence. It binds
  the exact Wallet request, exact release request, exact recipient bytes and
  authorization, exact policy and evidence request, exact custody epoch, exact
  Runtime issuer, one audit id, and one bounded window. Verification returns an
  authenticated replay-pending value only. It exposes exact request hashes and
  replay claim keys for the node-local atomic durable claim step, but it does
  not expose actionable verified requests before that claim succeeds. A crash
  or storage failure after the node-local claim succeeds but before
  contribution settlement is fail closed and currently requires a fresh
  Runtime release operation; there is no durable operation-resume journal yet.

This operational layer is still source-only. There is no provider registry
cutover, Runtime-owned replay store, recipient key-possession proof, node
lifecycle service, installed product flow, or production confidentiality claim
in this branch.

## Provider Boundary

Normal capsules must not see:

- raw CEKs
- wallet RPC or private keys
- arbitrary chain RPC
- Kubo/IPFS APIs
- Elacity SDK or pinning credentials

The provider plane should expose typed questions instead:

- `elastos://drm/open`
- `elastos://rights/access/has_access_by_content_id`
- `elastos://rights/subscription/is_subscription_active`
- `elastos://rights/content/can_stream`
- `elastos://rights/content/can_download`
- `elastos://chain/<network>/rights/has_access_by_content_id`
- `elastos://key/release`
- `elastos://decrypt/session/open`
- `elastos://decrypt/render`

## Remaining sequence

A. Finish the source-only review gate: professional external cryptographic and
   contract review, plus any additional grounded HPKE wrapper known-answer
   coverage that a reviewed upstream vector path can support.
B. Continue the source-only custody-node operations and durable node-state
   layer around the reviewed contracts. The current local child line adds the
   durable dual-key replay-claim boundary and the claim-gated node-release
   transition. Remaining work is node admission/rotation/revocation/recovery,
   issuer-key lifecycle, operational audit retention, and durable
   post-claim operation recovery.
C. Replace the provisional `elastos_common::protected_content` DTO/provider
   surface atomically with Runtime-owned orchestration over the reviewed v1
   contract: Wallet integration, recipient key generation and possession proof,
   typed rights checks, rights-bound key release, release-receipt-bound
   decrypt/render sessions, sealed decrypt material, and one source allow/deny
   proof with no fallback path.
D. Prove the installed end-to-end path: real protected-content producers,
   installed provider/runtime flow, decrypt/render handoff, and final product
   evidence.

Only after A-D should ElastOS decide whether a permissioned PQ-hybrid dKMS
layer should replace the pinned source-only helper for new content.

Visible protected-content UI may ship only as a disabled/read-only readiness
rail until fail-closed provider tests and capability-resource checks cover the
full open path. The current Library rail can show Provider Chain/status
receipts and a disabled `Encrypted recipients` option, but it must not claim
production encrypted-recipient sharing, dDRM completion, or generic decrypt/
render readiness.

## Executable Proof

Run `scripts/protected-content-provider-contract-smoke.sh` after changing
protected-content provider capsules. It exercises the real provider binaries
over their JSON line protocol and verifies the current provisional provider
journey, not the canonical v1 contract crate:

- status exposes blocked raw authority
- valid requests fail closed until backends are configured
- invalid raw-authority requests are rejected
- `drm-provider.open` reports the declared provider/runtime sequence
