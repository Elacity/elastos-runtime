# Protected-content v1 contracts

Status: source-only authority, operational-contract, and canonical-wire
foundation. The normative authority types are in the
`elastos-protected-content-contracts` crate, and the companion source-only
custody helper lives in `elastos-protected-content-custody`. This document
describes the reviewed contract layer; it does not claim running rights
providers, custody nodes, content availability, decryption, playback, or
product integration.

## Authority boundaries

1. A Profile and passkey identify the person. The v1 contract binds the
   collaboration Profile as one canonical Ed25519 `did:key` public key.
2. A Wallet signature authorizes one bounded rights request for the Wallet in
   the protected-content binding. It does not prove that the right exists;
   each release node evaluates the bound policy. Wallet does not replace
   Profile identity.
3. Runtime derives authority, selects providers, owns lifecycle, owns its
   orchestration replay, chooses terminal-receipt issuers, and audits. Each
   custody node owns the exact local atomic dual-key replay claim before it
   can act.
4. Provider contracts define local and remote rights and custody operations.
5. Carrier transports traffic only to endpoints selected by Runtime. It grants
   no authority and is absent from these types.
6. Each dKMS node evaluates the immutable rights policy independently and signs
   its own decision before it can sign a contribution.
7. Capsules own UI and workflows. The capsule-visible terminal result contains
   authenticated hashes and references, not provider-private contributions or
   route selection.

## Exact protected-content binding

`ProtectedContentBindingV1` is repeated unchanged through the authority chain.
It binds all of the following:

- ciphertext SHA-256 and exact byte length;
- key-envelope SHA-256 and exact byte length;
- rights-policy SHA-256 and exact byte length;
- canonical Profile Ed25519 public key;
- recovered Wallet address;
- non-secret, opaque 32-byte Runtime session binding;
- node-set identity, threshold, and custody-epoch identity.

`RightsPolicyIdentityV1` identifies the exact immutable policy each node must
evaluate. The policy body belongs to the rights provider. Its identity contains
no chain URL, RPC endpoint, network locator, or transport selection.

Runtime creates a fresh opaque binding and associates it with the verified
session for the bounded operation lifetime. The binding is not the Runtime
session ID, a hash supplied by the caller, a session cookie, a launch token, a
capability token, or another bearer credential.

Profile authority is encoded as 32 Ed25519 public-key bytes. DID text is only a
projection through the repository's shared strict codec. The codec accepts only
canonical `did:key:z...` text with the Ed25519 multicodec prefix; arbitrary DID
methods, alternate spellings, noncanonical Edwards encodings, and weak
Ed25519 public keys fail. Node and receipt-issuer authority uses that same
shared validator at construction.

The custody helper applies a stricter local canonical-identity rule to the
final X25519 component embedded in X-Wing draft-06 public-key bytes than RFC
7748 requires at the primitive boundary. Contract node custody keys and
recipient public keys use X-Wing draft-06 (ML-KEM-768 followed by X25519), and
the embedded X25519 component must use one exact canonical encoding and reject
low-order points. Frozen HPKE field/domain names retain their existing meaning
only where that separate recipient-release surface still uses them.

## Canonical wire form

JSON serialization is an inspection projection and is never signed or hashed.
Provider-private `RecipientSealedContributionV1`,
`NodeContributionStatementV1`, and `SignedNodeContributionV1` do not implement
Serde serialization because they contain sealed contribution bytes. Their only
wire form is the canonical binary contract. Every authoritative value uses the
crate's single binary encoder:

- ASCII domain tag followed by `00`;
- one fixed field order and one versioned domain per type;
- unsigned big-endian integers;
- fixed-width digests, keys, addresses, and nonces;
- two-byte lengths for bounded byte strings and nested values;
- explicitly bounded one-byte list counts where a type contains a list;
- no maps, optional fields, aliases, defaults, unknown fields, or trailing
  bytes.

Strict decode re-encodes and requires byte equality. SHA-256 covers the full
domain-separated canonical bytes. Golden encoding, hash, Wallet signature,
node signature, and terminal issuer signature vectors are tests in the crate.

V1 bounds encrypted content at 2^50 bytes, policy and envelope descriptions at
1 MiB each, recipient-sealed contributions at 16 KiB, node sets at 16 members,
Wallet requests at five minutes, and release/decision/contribution/terminal
windows at one minute. Child windows must remain inside their parent windows.
The terminal receipt must also remain inside every contribution window it
settles, so its issuer cannot extend a node's signed approval.
All `issued_at`, `expires_at`, and verification `now` values are unsigned Unix
seconds in UTC.

## Wallet rights request and audit receipt

`RightsRequestV1` binds the protected-content binding, one action, the exact
recipient key identity, issue and expiry times, and a 16-byte replay nonce. The
Wallet recovered from the signature must equal the Wallet in the protected
content binding. Runtime must also compare the signed recipient with the key it
received from the decrypt provider for the authenticated Profile/session. That
operation-scoped key is generated fresh by the provider; Runtime receives only
its public key and identity, and a caller cannot replace either the owner Wallet
or recipient after the signature.

Runtime must create the session binding, request time window, and fresh replay
nonce, then ask Wallet Provider to sign those exact canonical bytes. A capsule
may request an action, but it does not choose replay or session authority.

`RecipientKeyIdentityV1` contains a bounded encryption-suite identifier and the
SHA-256 identity of the exact recipient public-key bytes under that suite. It
contains no endpoint, device, route, or Carrier identity. The contract binds
the selected key and suite; the custody integration must enforce a reviewed
suite allowlist, carry the exact public key to each selected node, require
`matches_public_key` before encryption, and prove key ownership.

The signature is EIP-191 over the request's canonical bytes. Its 65th byte is
canonically `0` or `1`. Values `27` and `28` are rejected at construction and
canonical decode; v1 has no compatibility fallback. Current Wallet Provider
surfaces still need a typed protected-content integration that verifies an
externally completed signature, canonicalizes its recovery byte to `0` or `1`,
and only then constructs `WalletSignedRightsRequestV1`.

Verification requires an `AtomicReplayClaimer`. Its insert-if-absent key is the
domain-separated exact authority scope plus nonce, not the request hash. Times
are intentionally outside that scope, so changing expiry cannot reuse a nonce.
There is no verification overload that omits replay enforcement. The crate has
only a private in-memory test implementation; Runtime/node durable storage is
integration work.

`SignedRightsReceiptV1` is authenticated preliminary audit evidence. Even an
authentic `Allowed` receipt is not key-release authority. The release verifier
does not accept this receipt type. A forged or unsigned allow therefore has no
path into release verification.

## Typed policy and recipient-key authorization

`RightsPolicyBodyV1` is the v1 immutable policy body. It is intentionally
narrow and grounded in the current reviewed rights-provider surface:

- one exact `content_id`;
- one exact required `RightsActionV1`;
- one exact EVM right argument string sent as the third
  `has_access_by_content_id(content_id, subject, right)` ABI argument;
- one exact subject source: the Wallet address from
  `ProtectedContentBindingV1`;
- one exact EVM `chain_id`;
- one exact 20-byte EVM contract address;
- one exact 4-byte function selector;
- one exact ABI identity:
  `HasAccessByContentIdStringAddressString`;
- one exact bounded observation/finality rule:
  `RightsObservationFinalityV1::min_confirmations`.

This is not a generic policy language, opaque byte blob, or arbitrary map.
It is the smallest reviewed source-only policy shape that matches the current
typed `chain-provider` `has_access_by_content_id` path. The signed policy now
explicitly maps one product `RightsActionV1` to one exact contract right string,
so two nodes cannot claim the same policy identity while sending different ABI
right arguments. Production-approved chain ids, contract addresses, selectors,
ABI fixtures, and right strings remain open review inputs, but they are
required typed policy fields now, not ambient provider configuration.

`RightsEvaluationEvidenceRequestV1` is the matching typed evidence request. It
pins the exact `ProtectedContentBindingV1` and `RightsPolicyIdentityV1`. A node
cannot claim to evaluate a different policy or a different Wallet/content/
session binding than the one named in the request.

`RightsEvaluationEvidenceV1` is the matching typed evaluation result. It binds
the same exact binding and policy identity plus:

- the exact Wallet subject address derived from the binding;
- the exact observed chain id;
- the exact observed block number and block hash;
- the exact head block number used for the finality rule; and
- the exact `has_access` result.

Verification rejects a different Wallet subject, different chain, or
insufficient confirmation depth before any provider integration exists. RPC
URLs, provider labels, transport routes, and endpoint identity remain outside
the contract. Because the evidence binds the exact `RightsPolicyIdentityV1`, it
indirectly pins the full reviewed policy body: contract, selector, ABI, exact
right argument, content id, and finality rule.

`SignedRecipientKeyAuthorizationV1` is a Profile-signed recipient-key
authorization. It binds all of the following:

- the exact `ProtectedContentBindingV1`;
- the exact requested action;
- the exact X-Wing draft-06 PQ-hybrid recipient public-key bytes;
- the exact `RecipientKeyIdentityV1`;
- the exact Runtime application-operation issuer key;
- the shared opaque Runtime session binding;
- one bounded issue/expiry window.

Verification compares the recipient bytes to `RecipientKeyIdentityV1`, the
Profile signer to the binding Profile, the Runtime issuer to the signed issuer,
and the window to the parent binding/action/session context. This object proves
authorization only. It does not prove that the decrypt provider retains the
matching secret behind its opaque handle. The provider generates that fresh
operation-scoped X-Wing draft-06 key; no Profile seed enters protected-content
contracts or provider calls. Holder-only possession is a PQ-hybrid
challenge/response against that exact authorized public key, and the
crate-public reconstruct path returns the CEK only inside a decrypt-session
wrap.

## Custody epoch and Runtime release operation

`SignedCustodyEpochV1` is the immutable signed custody epoch. Its statement
binds:

- the exact epoch issuer key;
- the exact approved recipient/stored-share/released-share suite ids;
- the exact threshold;
- the exact ordered node list of:
  - Ed25519 node signing key,
  - X-Wing draft-06 node custody public key (ML-KEM-768 followed by X25519),
  - deterministic share coordinate.

The node list is canonicalized by node signing key, and the coordinates are
reassigned deterministically from that order. `KeyEnvelopeIdentityV1` now pins
the resulting `CustodyEpochIdentityV1`, so the epoch signature is not
self-authorizing and an existing envelope cannot silently inherit a different
issuer, node set, threshold, or suite.

`SignedRuntimeReleaseOperationV1` is the typed application-authenticated
Runtime-to-release-node envelope above Carrier transport authentication. It
binds:

- the exact canonical Wallet-signed rights request bytes and derived hash;
- the exact canonical key-release request bytes and derived hash;
- the exact recipient public-key bytes;
- the exact signed recipient-key authorization;
- the exact typed policy body, policy identity, and evidence request;
- the exact signed custody epoch and bound key-envelope identity;
- the exact Runtime application-operation issuer;
- one bounded audit request id and issue/expiry window.

Verification proves those bindings and requires the Profile-signed
recipient-key authorization to authorize the Runtime issuer. Carrier endpoint
authentication remains transport evidence only. This source-only contract does
not implement provider routing or Runtime durable replay state. Verification
returns an authenticated replay-pending surface only: it proves signatures and
exact bindings, exposes the nested request hashes and replay-claim keys, and
does not expose actionable `VerifiedRightsRequestV1` or
`VerifiedKeyReleaseRequestV1` values. The companion source-only custody helper
uses those exact claim keys for one local dual-key atomic claim transition. It
can persist the exact recipient-encrypted node contribution with that claim and
replay only that result after restart. Runtime durable replay and orchestration
remain later work. A crash or storage failure after the claim becomes durable
but before its result becomes durable is fail closed and requires a fresh
Runtime release operation; there is no operation-resume journal for that state.

## Node decision and contribution

`KeyReleaseRequestV1` binds the same protected-content identity, original
Wallet request hash, action, signed recipient, shorter child window, and a
separately claimed release nonce. It stays inside the verified Wallet request
window.

`KeyReleaseRequestV1` is not a Runtime signature or a bearer grant. A remote
node must receive the canonical Wallet request and release request through the
typed `SignedRuntimeReleaseOperationV1` envelope. The node must still verify
the nested requests and claim both replay keys atomically in its own durable
store before it acts. Carrier endpoint authentication alone does not prove who
authored the application request.

Each node creates `SignedNodeRightsDecisionV1` after independently evaluating
the exact rights-policy identity. The signed statement binds the release and
Wallet request hashes, complete protected-content binding, action, node public
key, decision, provider evidence hash, and child window.

`SignedNodeContributionV1` nests that signed node decision. Verification
requires `Allowed`, verifies membership in the bound node set, and verifies the
contribution with the same node key. A denied decision, decision from another
node, wrong node set, wrong threshold, or escaped child window fails.

The contribution payload is provider-private
`RecipientSealedContributionV1`: exact recipient key identity plus bounded
opaque bytes. The contract requires that recipient to equal the signed release
recipient and authenticates the bytes and commitment. The contract type itself
does not implement encryption. The companion source-only
`elastos-protected-content-custody` crate implements X-Wing draft-06 PQ-hybrid
recipient-sealed contributions and threshold reconstruction, including a
manifest-bound reconstructed-key commitment check that detects a wrong
reconstructed key. It strictly validates the embedded X25519 component. It
does not identify the malicious node, it is not verifiable secret sharing, it
is not yet wired into Runtime/provider/product flows, and it has not received
an external cryptographic audit. No production confidentiality claim follows
from this contract type alone.

## Terminal result

`SignedTerminalReceiptV1` is the capsule-visible result. Its statement contains
the exact release-request hash and binding, outcome, and unique node keys,
decision hashes, contribution hashes, and contribution commitments. It does not
contain the provider-private contribution bytes.

The statement names an Ed25519 issuer and is signed. Verification requires the
Runtime-selected expected issuer, the verified release request, and the exact
set of verified contribution references. Every verified contribution must
retain and match the exact release-request hash and recipient. A successful
result requires exactly the bound threshold. Required-plus-one released
contributions are rejected, not treated as extra success evidence.
Contributions from another request or recipient cannot be reused. The receipt
is audit/result evidence, not a portable grant that can authorize another
release.

## Threat model and fail-closed rules

Tests reject wrong Wallet, attacker-signed victim address, wrong content,
policy, evidence request, node set, threshold, Profile, session, action,
request hash, issuer, recipient, recipient-key identity, Runtime issuer,
custody epoch, approved suite, expiry, replay, and post-signature mutation.
They also reject a forged allow receipt, nonce reuse after changing fields,
child lifetime escape, a denied or cross-node decision, cross-request
contribution reuse, duplicate node/decision/contribution/commitment evidence,
insufficient threshold, required-plus-one released evidence, malformed
canonical input, noncanonical Wallet/Profile encodings, and noncanonical or
low-order X25519 contract key encodings.

These contracts plus the source-only `elastos-protected-content-custody` helper
provide one bounded node-local owner-only dual-key replay store. It binds the
store to one node, privately gates release on the exact claim, persists the
exact recipient-encrypted contribution, and replays only that result. They do
not solve malicious custody nodes, rights-policy correctness, Runtime durable
replay storage, full operational custody state, recovery from a durable claim
without a result, issuer-key lifecycle, node admission/rotation/recovery,
Library list/open/play, rendering, or product workflow safety. Share wrap on
this unpublished tree is `elastos-xwing-draft06-hkdf-sha256-aes256gcm/v1`:
X-Wing draft-06 with X25519 + ML-KEM-768, HKDF-SHA256, and AES-256-GCM.
This is PQ-hybrid confidentiality only; authority signatures remain
Ed25519/classical and full PQ authorization is a pre-activation decision. The
Runtime mint journal can commit 2-of-3 PQ-hybrid envelopes without claiming
content availability or a catalog path. Buy/open remains blocked until Runtime
verifies the existing content provider's exact signed availability receipt.
Remaining inactive e2e and cutover work is tracked in `TASKS.md`.

The parent branch's provisional `elastos_common::protected_content` DTOs are
not this canonical contract. Integration must replace that surface atomically
after independent review. It must not add parallel decoders, migration adapters,
or compatibility fallbacks.
