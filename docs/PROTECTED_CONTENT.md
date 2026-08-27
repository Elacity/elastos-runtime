# Protected content

Protected content is Runtime-mediated. App, viewer, and content capsules ask to
open an object; they do not receive raw wallet, chain, IPFS, Elacity, or key
authority.

The source tree currently has one canonical source-only review stack and one
provisional retirement surface:

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
- The older installed/provider surface is the provisional
  `elastos_common::protected_content` DTO set plus the fail-closed
  `drm-provider`, `rights-provider`, `key-provider`, and `decrypt-provider`
  capsules. It is not current architecture and does not consume or prove the
  v1 contract. The Runtime integration stage must remove it atomically. It must
  not remain as a parallel decoder or compatibility path.

## Canonical architecture

The intended protected-content path is:

`capsule -> Runtime coordinator -> rights-provider -> custody providers -> decrypt-provider`

- Capsules own user workflow and request an action. They do not select
  providers, custody nodes, routes, or network locations.
- Runtime derives authority from the authenticated Profile, exact Wallet
  approval, session, object, and action. It selects providers, owns durable
  orchestration, and audits the result.
- `rights-provider` evaluates the exact approved policy through typed Chain
  evidence. It does not release content keys.
- Each selected custody provider independently verifies the exact Runtime
  operation and rights evidence, then returns only a recipient-encrypted
  contribution and an authenticated receipt.
- `decrypt-provider` is the only boundary that may reconstruct and briefly hold
  a live CEK. It returns only scoped output or an opaque session handle and
  zeroizes the CEK when the session ends.
- Carrier transports endpoint traffic selected by Runtime. Carrier is not
  Profile, rights, custody, or key authority.

Runtime may relay recipient-encrypted contributions or sealed material that it
cannot open. Runtime and capsules must never receive raw CEKs or custody shares.
Capsules must also never receive provider routes, endpoint DIDs, IP addresses,
ports, credentials, Wallet RPC, Chain RPC, Kubo/IPFS APIs, or Elacity SDK
authority.

This architecture is not yet installed or wired into Runtime. The current
branch is source-only contracts and custody behavior.

## Provisional retirement surface

Released 0.6 still contains the provisional `elastos_common::protected_content`
DTOs and the fail-closed `drm-provider`, `rights-provider`, `key-provider`, and
`decrypt-provider` capsules. They remain only as a disabled retirement surface
until the canonical Runtime path replaces them. They are not a second product
architecture and are not evidence that the canonical v1 path works.

The provisional provider smoke verifies only that this old surface rejects raw
authority and remains unavailable without configured backends. It must not be
used to claim custody, Runtime orchestration, decryption, or playback readiness.

PC2's dDRM contracts and WASM decrypt/render/media crates remain implementation
references. A later integration may use reviewed parts inside canonical
providers, but those parts must not create a second authority path.

## Future sealed decrypt handoff

The intended handoff keeps live key material out of Runtime. Runtime binds an
authorized decrypt session to the exact object, action, recipient, rights
evidence, custody epoch, expiry, and provider identity. Custody nodes return
recipient-encrypted contributions. Runtime relays those opaque contributions to
the authorized decrypt boundary. The decrypt boundary reconstructs and uses the
CEK only inside that scoped session, then zeroizes it. It does not make outbound
calls to obtain broader authority.

The new Profile-signed recipient-key authorization in the v1 contract is an
authorization object only. It binds one exact recipient public key and one
exact Runtime operation issuer for one binding/action/session/time window. It
does not prove X25519 secret-key possession. Actual holder-only possession
remains a later Runtime-owned invariant.

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
  custody crate includes one node-local durable dual-key replay-claim store. It
  privately gates release on the exact claim, persists the exact encrypted node
  contribution, and replays only that result after restart. A durable claim
  without a stored result fails closed. The store adds a domain-separated
  integrity digest to detect same-length corruption or torn local state, but it
  does not defend against a malicious same-UID rewrite that can recompute the
  digest.

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

## Capsule boundary

Normal capsules must not see:

- raw CEKs
- wallet RPC or private keys
- arbitrary chain RPC
- Kubo/IPFS APIs
- Elacity SDK or pinning credentials
- custody shares or node contributions
- provider routes, endpoint DIDs, IP addresses, or ports

In the intended architecture, capsules request an object action through Runtime
and receive a typed terminal result or scoped output handle. Runtime will select
and coordinate the typed rights, custody, and decrypt operations.

## Remaining sequence

A. Finish the source-only review gate: professional external cryptographic and
   contract review, plus any additional grounded HPKE wrapper known-answer
   coverage that a reviewed upstream vector path can support.
B. Continue the source-only custody-node operations and durable node-state
   layer around the reviewed contracts. The published custody branch adds the
   durable dual-key claim boundary, claim-gated node release, and exact encrypted
   result replay. Remaining work is node admission, rotation, revocation,
   recovery, issuer-key lifecycle, operational audit retention, and recovery
   from a claim that completed before its result became durable.
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

## Provisional retirement guard

Run `scripts/protected-content-provider-contract-smoke.sh` only when changing
the provisional provider capsules. It exercises those old binaries over their
JSON line protocol. It is a fail-closed retirement guard, not verification of
the canonical Runtime, rights, custody, or decrypt architecture:

- status exposes blocked raw authority
- valid requests fail closed until backends are configured
- invalid raw-authority requests are rejected
- the provisional `drm-provider.open` reports its old declared sequence
