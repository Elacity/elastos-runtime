# Protected content

Protected content is Runtime-mediated. App, viewer, and content capsules ask to
open an object; they do not receive raw wallet, chain, IPFS, Elacity, or key
authority.

The protected-content source line is still not installed product behavior.
Released 0.6 contains the older provisional provider surface. Newer review
branches define the canonical source boundaries without changing the active
Library/open path:

- The canonical v1 published review stack begins with the
  `elastos-protected-content-contracts` branch, the companion
  `elastos-protected-content-custody` branch, and the published child
  `feat/protected-content-key-reconstruction` branch, documented in
  [Protected-content v1 contracts](PROTECTED_CONTENT_CONTRACTS_V1.md). That
  stack now defines canonical authority bindings, typed rights-policy and
  evidence contracts, Profile-signed recipient-key authorization, signed
  immutable custody epochs, a typed Runtime-to-release-node operation
  envelope, a local durable dual-key replay-claim store for custody nodes,
  a claim-gated node release path, and source-only custody helpers for
  custody-envelope provisioning, recipient-sealed node release, and
  threshold reconstruction inside the decrypt boundary for new content. That
  published ancestry does not yet wire in product Runtime orchestration,
  provider integration, Runtime-owned replay storage, recipient
  key-possession proof, or product playback.
- The current published source-only branch also includes
  `origin/feat/protected-content-custody-provider` at `f7cd6c3d`. Its ancestry
  adds the private provider protocol, authenticated payload sealing, local
  decrypt-output helper, object-bound custody-pool policy, one-node
  provisioning authority, expected Runtime issuer pinning, owner-only durable
  node-share storage, and the historical source-only `custody-provider`
  process. It proves one selected node stores one sealed share and releases
  only recipient-sealed contributions after signed rights validation. On the
  current `feat/protected-content-runtime-lifecycle` branch that provider is
  registered only on the inactive Runtime path. It is still not active,
  installed, or product-proven.
- The published source-only stack now continues through
  `origin/feat/protected-content-wallet-rights` at `2c69d0c2`,
  `origin/feat/protected-content-runtime` at `b00bfeeb`, and
  `origin/feat/protected-content-rights` at `43a83e5b`. Together they add
  Wallet-signed rights requests, private durable Runtime release coordination,
  typed chain policy/evidence, and the typed rights evaluator without changing
  the active installed product path.
- The current `feat/protected-content-runtime-lifecycle` branch continues the
  published `origin/feat/protected-content-runtime-lifecycle` source line from
  `34465959`. The inactive Runtime-owned mint -> availability -> creator
  mint/list -> buy -> open -> play -> close path is complete in source on the
  current branch: Runtime-owned mint durability, fresh pre-buy availability,
  verified creator mint/list binding, Runtime-owned buy with finalized access
  corroboration, durable viewer lifecycle, and the inactive combined mint ->
  buy -> open -> play -> close proof. Later closeout commits keep Base
  read-path truth and docs current without changing installed-product scope.
- The older installed/provider surface is the provisional
  `elastos_common::protected_content` DTO set plus the fail-closed
  `drm-provider`, `rights-provider`, `key-provider`, and `decrypt-provider`
  capsules. It is not current architecture and does not consume or prove the
  v1 contract. The inactive canonical mint/buy/open/play path is complete in
  source on `feat/protected-content-runtime-lifecycle`, but it is still not
  installed, packaged, configured, cut over, or product-ready. It must not
  remain as a parallel decoder or compatibility path.
- The published protected-content lifecycle branch currently ends at
  `origin/feat/protected-content-runtime-lifecycle` commit `34465959`; the
  current branch completes the source-only inactive path without changing
  installed product behavior.

## Canonical architecture

The intended protected-content path is:

`capsule -> Runtime coordinator -> chain policy/evidence -> custody providers -> decrypt-provider`

- Capsules own user workflow and request an action. They do not select
  providers, custody nodes, routes, or network locations.
- Runtime derives authority from the authenticated Profile, exact Wallet
  approval, session, object, and action. It selects providers, owns durable
  orchestration, and audits the result.
- Rights evaluation is node-local inside each selected custody provider.
  Runtime asks the existing `chain` provider for the configured policy body
  and later evidence. There is no rights-provider process.
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

`CustodyEnvelopeV1` is private owner-only Runtime open/provisioning material
stored on the inactive source path at
`protected-content/runtime-open/{mint}/envelope.bin`. It is separate from the
identity-only mint journal and from public metadata, and capsules cannot read
it. Runtime may carry the envelope as provisioning/open material, but it cannot
open the node-sealed shares inside it. Each selected custody provider persists
only its own raw share. Public metadata contains no shares; it contains only
bounded identities, threshold/epoch/pool facts, CEK commitment, and
signatures.

Raw CEK and private-key JSON vectors in historical branches are deterministic
test data only. Product operations, responses, logs, public metadata, and
durable product state must never contain raw CEKs.

Operator addresses and ports may exist only in private deployment
configuration. Contracts and capsules see Runtime-selected service or endpoint
identities, never raw topology. Carrier transports selected endpoint traffic
and does not grant rights or custody authority.

This architecture is not yet installed or wired into the active Library product
path. Source truth now includes contracts, custody behavior, authenticated
payload sealing, decrypt-boundary helpers, the custody provider registered
only on the inactive Runtime path, Wallet-rights signing, a private Runtime
coordination foundation, typed chain-rights evaluation, durable custody
provisioning, a private server content publish/status/refetch verifier that
returns signed provider/object/publisher-pinned availability evidence, and
Runtime-owned mint/buy/open/read/close seams on the current
`feat/protected-content-runtime-lifecycle` branch. The inactive Runtime
composite on that branch now proves the typed gateway publish/list/buy/open
path with real
protect, custody, and decrypt provider processes; deterministic Wallet, Chain,
and content fixtures still stand in for not-yet-installed authority surfaces.
The live
provisional `drm` / `rights` / `key` / `decrypt` product routes remain
unchanged.

Current source proof is layered. Runtime mint tests prove durable 2-of-3
custody provisioning and an exact signed, provider-pinned
content-availability decision before buy/open. The server adapter verifies the
existing generic content object and signed receipt; the typed combined proof
now covers creator publish -> fresh availability -> creator mint/list -> buyer
recheck and buy -> finalized access corroboration -> open -> 2-of-3 custody
release -> decrypt init + one segment read -> exact close. Lower-level Runtime
lifecycle and separate
decrypt-provider process tests prove PQ-hybrid contribution reconstruction,
exact CENC media reads, close replay, process restart, and old-handle absence.
Separate Runtime restart/replay tests prove persisted terminal replay and
retained nonterminal state after effect start. This remains source proof, not
installed/cut-over product evidence: real signed 2-of-3 operator custody
config, installer/ProviderRegistry packaging, installed inactive proof, and the
atomic cutover are still open.

## Provisional retirement surface

Released 0.6 still contains the provisional `elastos_common::protected_content`
DTOs and the fail-closed `drm-provider`, `rights-provider`, `key-provider`, and
`decrypt-provider` capsules. That old DRM/provider code remains installed and
source-visible only until the canonical Runtime path replaces it atomically. It
is not a second product architecture, and it is not evidence that the canonical
v1 path works.

The provisional provider smoke verifies only that this old surface rejects raw
authority and remains unavailable without configured backends. It must not be
used to claim custody, Runtime orchestration, decryption, or playback readiness.

PC2's dDRM contracts and WASM decrypt/render/media crates remain implementation
references. A later integration may use reviewed parts inside canonical
providers, but those parts must not create a second authority path.

## PR #15 disposition

The public, unmerged `feat/dkms-esp-port` / PR #15 tree is research and
behavior evidence only:

- Keep as research: threshold crypto, node-local custody, recipient-sealed
  contributions, CEK commitment, lifecycle scenarios, and fail-closed negative
  tests.
- Reimplement at the canonical boundary: per-node durable shard storage,
  DKG/rotation/re-share/revocation, pool/governance policy, provider roles, and
  Runtime-open scenarios.
- Reject from the product path: public aggregated `shares[]` metadata,
  capsule-owned authority, raw CEK operations, `rail_shim` and reference
  fallbacks, old `drm-provider` orchestration, direct TCP/IP/port topology in
  capsules or contracts, static authorization fallbacks, and the standalone
  harness as a product route.

PR #15's producer-smoke `escrow.json` is historical development evidence only.
It aggregates wrapped shares, so ElastOS must not adopt that fixture as
canonical metadata. The producer smoke writes and reloads
`cek_commitment_b64`; the missing-commitment writer/reloader inconsistency
belongs to the older Creator path.

Latest confirmed PR #15 comment truth for the Base read path:

- AuthorityGateway access reads use
  `hasAccessByContentId(address holder, bytes16 contentId) -> bool`.
- The exact bytes16 value is the CENC KID and remains separate from the full
  `EncryptedContentIdentityV1`.
- AuthorityGateway resolves that KID through
  `CentralStorage.ipReference(bytes16)`.
- Unknown/unbound KIDs revert with exact custom error
  `UnboundContentId(bytes16)` / selector `0xcad88223`; bound KIDs without
  access return `false`.

Still open from that same review:

- the exact KID-binding write operation and authorization path; do not treat
  `bindIP` as verified truth;
- whether canonical deployed purchase state requires Runtime to call
  `buyAccess`, plus the exact ABI/receipt/event proof for that path;
- whether `View` / `Download` remain only signed Runtime policy actions for
  the deployed Elacity flow; and
- one known bound KID plus one allowed and one denied wallet against the
  reviewed deployed proxy.

## Source-only sealed decrypt handoff

The intended handoff keeps live key material out of Runtime. Runtime binds an
authorized decrypt session to the exact object, action, recipient, rights
evidence, custody epoch, expiry, and provider identity. Custody nodes return
recipient-encrypted contributions. Runtime relays those opaque contributions to
the authorized decrypt boundary. The decrypt boundary reconstructs and uses the
CEK only inside that scoped session, then zeroizes it. It does not make outbound
calls to obtain broader authority.

The new Profile-signed recipient-key authorization in the v1 contract is an
authorization object only. It binds one exact provider-generated,
operation-scoped PQ-hybrid recipient public key and one exact Runtime operation
issuer for one binding/action/session/time window. It does not prove
PQ-hybrid secret-key possession. The decrypt provider retains the matching
secret behind an opaque handle and requires a PQ-hybrid challenge/response
against that exact public key before reconstruction; no Profile seed enters
Runtime, custody, or decrypt-provider contracts. The crate-public reconstruct
path returns the CEK only inside a PQ-hybrid decrypt-session wrap. The inactive
source path now assembles the signed Runtime release operation and signs the
exact recipient-key authorization through the existing Profile authority
surface, but that path is still not installed or cut over. Profile
authorization alone is still not possession, and this branch still makes no
active-product confidentiality claim.

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
  `elastos-xwing-draft06-hkdf-sha256-aes256gcm/v1` (X-Wing draft-06:
  X25519 + ML-KEM-768, HKDF-SHA256, AES-256-GCM). This is a confidentiality
  suite only; authority signatures remain Ed25519/classical.
- It rejects truncated or X25519-only wrap public keys before seal or unseal.
  This is a local canonical-identity rule, not an RFC 9180 HPKE product path.
- The decrypt provider generates each operation-scoped recipient secret and
  retains it behind an opaque handle. The authenticated Profile signs the exact
  public-key authorization; no Profile seed enters Runtime, custody, or the
  decrypt-provider contracts. Reconstruction requires holder-only PQ-hybrid
  possession. The crate-public reconstruct path wraps the CEK to a
  decrypt-session key; payload decrypt keeps the CEK inside this crate.
- It uses GF256 Shamir splitting through `vsss-rs` for new content only.
- Released terminal settlement and recipient reconstruction require exactly the
  bound threshold count. Required-plus-one contributions are rejected.
- It returns only opaque, redacted secret wrappers; it does not expose a
  capsule-visible raw-key API.
- The published custody-provider branch includes source-only staged
  payload sealing inside this same custody crate. That is intentional: keeping
  CEK generation, commitment, payload encryption, custody-envelope
  provisioning, and zeroization in one crypto boundary is safer than exposing
  or duplicating CEK APIs across crates. The sealing API writes only to a
  staging sink and returns canonical metadata only after both ciphertext
  staging and custody provisioning succeed; callers must discard staged output
  after any error.
- The published custody-provider branch also includes source-only staged
  decrypt output inside that custody boundary. It accepts the exact
  encrypted-content identity plus the existing authenticated release inputs,
  reconstructs the CEK only inside custody, verifies the full framed
  ciphertext identity before any plaintext write, authenticates each chunk
  before staging plaintext, and returns only bounded plaintext metadata after
  full success. On any error it returns no success metadata and callers must
  discard staged plaintext output. It does not yet add provider wire, Runtime
  orchestration, viewer streaming, product integration, installation, or
  deployment.
- Custody-pool policy keeps three truths separate: a permissioned signed
  custody pool for node eligibility, each immutable per-object custody
  committee in `SignedCustodyEpochV1`, and later Runtime route resolution
  keyed by `node_public_key`. Validation requires a caller-supplied trusted
  policy authority, exact signed pool, exact signed custody epoch, a second
  signed committee authorization that binds the exact pool snapshot to the
  exact epoch identity, and a caller-supplied expected
  `CustodyCommitteeAuthorizationIdentityV1`. Pool membership binds only node
  signing keys, node custody keys, opaque operator identities, opaque
  failure-domain identities, approved suites, validity, and revocation. It
  does not sign service IDs, routes, URLs, hostnames, sockets, ports,
  WireGuard, ALPN, or other topology. The source policy is fixed 2-of-3 across
  distinct operators and failure domains, with no fallback or silent
  substitution. For protected-content release, `failure_domain_id` must include
  the chain-observation backend as well as the custody operator lane; two
  committee members that depend on one RPC operator/backend are not distinct
  failure domains. A protected object must later commit the exact pool identity,
  exact epoch identity, and exact committee-authorization identity it was
  minted against; Runtime open must use those object-bound identities rather
  than any "latest pool" view. The pool authority is permissioned, not a
  decentralized consensus system.

The published ancestry includes classical HPKE/X25519 helper material as source
history only; it is not the current source-only mint/open tree and it is not a
product mint path. The current protected-content review tree uses
`elastos-xwing-draft06-hkdf-sha256-aes256gcm/v1` for PQ-hybrid share-wrap
confidentiality and requires both `x25519` and `ml-kem-768`. Missing either KEM
fails closed. There is no classical-only product envelope, no PQ-off default,
and no dual decoder. Node and recipient wrap identities carry hybrid public
keys before mint. Authority signatures on this current review tree remain
classical and are not claimed quantum-safe. External review remains required
before public dKMS claims. The manifest commitment detects that threshold
reconstruction produced the wrong key; it does not identify the malicious node
and it is not verifiable secret sharing.

## Current source-only operational contracts

The reviewed v1 contract stack now also defines the source-only operational
contract layer that later Runtime and custody-node integrations must consume:

- `RightsPolicyBodyV1`, `RightsEvaluationEvidenceRequestV1`, and
  `RightsEvaluationEvidenceV1` provide one narrow typed EVM policy/evidence
  shape grounded in the current `chain-provider`
  `has_access_by_content_id` method only: exact Wallet-derived subject,
  exact full encrypted-content identity, one distinct 16-byte Base access
  content ID, signed `RightsActionV1`, `chain_id`, contract bytes, selector,
  ABI identity, one finalized-only observation rule, and exact finalized
  block/result evidence. There is no free-form contract right string, and these
  contracts do not carry RPC URLs, provider labels, or routes.
- The current review chain-rights/evaluator line tightens this into a source-only
  typed evidence path: evidence is acquired for the exact Runtime release
  operation, verifies live chain id, uses exact finalized block hash plus
  finalized block number, rejects selector/method mismatches, applies bounded
  freshness, and redacts upstream provider failures. Each custody node resolves
  that evidence on its own node-host Runtime through 2-5 explicit
  protected-content RPC sources, requiring two exact finalized matches with no
  fallback to the general network RPC URL. This corroboration reduces one-source
  risk but is not consensus proof. The evaluator obtains evidence through a
  trusted source handle rather than accepting arbitrary caller-supplied rights
  facts.
- The current review Wallet-rights line adds one dedicated Wallet operation that signs
  exact canonical `RightsRequestV1` bytes for the selected active EVM account
  through the existing verified Wallet invocation context. It carries no
  duplicate object, pool, epoch, committee, Profile, or session fields outside
  the canonical request.
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
- The source-only custody-provider capsule proves one configured node, one
  selected sealed share per object, expected Runtime issuer validation,
  local-node validation, owner-only durable node-share storage, exact duplicate/
  conflict/restart behavior, signed-rights-gated release, exact encrypted
  contribution replay, bounded provider frames, redacted diagnostics, and clean
  shutdown. In the published custody-provider branch it was not registered by
  Runtime; the current lifecycle branch registers it only on the inactive path.
  It is still not installed, deployed, or product proven. It carries no CEK,
  raw share, provider route, endpoint, IP address, port, Carrier topology, or
  credential in provider responses.
- The current `origin/feat/protected-content-runtime` branch adds private durable operation state and
  typed internal coordination over existing Wallet, rights, and custody
  provider contracts. It persists before provider effects, records
  effect-started state before the first effectful call, stores exact terminal
  results for replay, and leaves ambiguous post-dispatch outcomes durable and
  nonterminal. `feat/protected-content-runtime-lifecycle` registers inactive
  `custody` through `ProviderRegistry` / `ProviderBridge`, derives one stable
  owner-only inactive custody state root under the Runtime data root, rejects
  missing or unsafe custody paths before spawn, scans unresolved journal ids,
  and settles only from exact identity-bound receipts after
  `provider_effect_started`. Rights evaluation
  acquires Chain evidence by invoking existing `chain` /
  `protected_content_rights_evidence` through `ProviderRegistry`; it does not
  replace live `rights`. Its Runtime mint tests distinguish custody
  provisioning from signed content availability, then exercise the inactive
  test-provider buy/open composite. This is still not
  installed product behavior, Library cutover, or a production confidentiality
  claim.

This operational layer is still source-only. Inactive `ProviderRegistry`
wiring and recipient key-possession proof exist on this branch, but there is no
active provider-registry cutover, installed product flow, or production
confidentiality claim. The provisional `key-provider` and `rights-provider`
remain the active registered product path until the atomic cutover.

### dKMS placement and transport

The typed key-release contract does not change with placement. Runtime selects
the route after authorization; the caller does not name a peer, host path,
backend, or transport. The selected provider may use an internal compatibility
backend, but that backend does not become part of the capsule contract:

- same-node key or dKMS implementations use a private Runtime-owned adapter;
- a first-party production dKMS hop that crosses an ElastOS machine boundary
  uses Carrier as the canonical off-box transport;
- vendor or compatibility transports remain provider-internal and must not
  create a second capsule contract;
- Carrier endpoint authentication identifies the transport peer but does not
  authorize key release, so the dKMS protocol still verifies node authority,
  rights binding, encryption, signatures, freshness, replay, and threshold
  policy end to end;
- raw CEKs never enter Runtime, Carrier, an ordinary app, or a viewer. Prefer
  direct sealing to a one-time decrypt-session key; and
- an all-on-one-machine quorum is a development or contract-test topology, not
  proof of independent operators, failure domains, or distributed custody.

Current EVM/BTC/ELA wallet proofs and dDRM chain state are still classical. They
are useful authorization inputs today, but they should not be the only permanent
identity or access root for long-lived encrypted assets.

References: [NIST PQC standards announcement](https://www.nist.gov/news-events/news/2024/08/nist-releases-first-3-finalized-post-quantum-encryption-standards),
[FIPS 203 ML-KEM](https://csrc.nist.gov/pubs/fips/203/final),
[FIPS 204 ML-DSA](https://csrc.nist.gov/pubs/fips/204/final),
[FIPS 205 SLH-DSA](https://csrc.nist.gov/pubs/fips/205/final),
and [RFC 9591 FROST](https://www.rfc-editor.org/rfc/rfc9591).

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

The architecture, published stack, open installed prerequisites, atomic
cutover, and acceptance requirements live in the
[Protected-content integration plan](PROTECTED_CONTENT_EXTRACTION.md).

Before a source-only child branch is used as a parent, its branch ancestry and
focused gates still need review/publish hygiene. The source-only inactive path
is complete on the current `feat/protected-content-runtime-lifecycle` branch;
the remaining order is:

1. Publish and review the current source-only branch without widening the
   installed product path.
2. Prove the remaining deployed Base facts: the exact KID-binding write
   operation and authorization, the exact `AuthorityGateway.buyAccess`
   ABI/receipt/event and whether Runtime must issue it for canonical purchase
   state, the deployed `View` / `Download` policy semantics, and one known
   bound KID with allowed and denied wallets.
3. Provision the installed prerequisites: one signed owner-only 2-of-3 custody
   composition, Chain config and funded accounts, and packaged protect + three
   custody instances + decrypt through the existing installer / `ProviderRegistry`
   path. No new route, provider, app, authority, or fallback path is added
   here.
4. Run the installed inactive proof with three real replicas and repair after
   one replica is lost, still without cutover.
5. Land one atomic cutover that removes the provisional
   `drm` / `rights` / `key` / `decrypt` authority with no fallback, dual route,
   or compatibility decoder.
6. Prove the installed one-Runtime / two-principal mint -> buy -> play
   acceptance path.

PQ external review remains open before public cryptographic claims. Later
public-network gates, multi-Runtime issuer admission, and cross-Runtime
protected-content exchange remain separate post-cutover decisions.

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
