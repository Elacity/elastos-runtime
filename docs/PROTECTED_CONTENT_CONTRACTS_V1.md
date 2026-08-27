# Protected-content v1 contracts

Status: source-only contract foundation. The normative implementation is the
`elastos-protected-content-contracts` crate. This document describes that
review candidate; it does not claim running rights providers, key custody,
content availability, decryption, playback, or product integration.

## Authority boundaries

1. A Profile and passkey identify the person. The v1 contract binds the
   collaboration Profile as one canonical Ed25519 `did:key` public key.
2. A Wallet signature authorizes one bounded rights request for the Wallet in
   the protected-content binding. It does not prove that the right exists;
   each release node evaluates the bound policy. Wallet does not replace
   Profile identity.
3. Runtime derives authority, selects providers, owns lifecycle, performs the
   mandatory atomic replay claim, chooses terminal-receipt issuers, and audits.
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
- node-set identity and threshold.

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
selected for the authenticated Profile and session. A caller cannot replace
either the owner Wallet or recipient after the signature.

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

## Node decision and contribution

`KeyReleaseRequestV1` binds the same protected-content identity, original
Wallet request hash, action, signed recipient, shorter child window, and a
separately claimed release nonce. It stays inside the verified Wallet request
window.

`KeyReleaseRequestV1` is not a Runtime signature or a bearer grant. A remote
node must receive the canonical Wallet request and release request through a
typed, application-authenticated Runtime-to-provider operation. The node must
verify both requests and claim replay in its own durable store before it acts.
Carrier endpoint authentication alone does not prove who authored the
application request. That operation envelope is integration work and is not
defined by this source-only crate.

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
recipient and authenticates the bytes and commitment. It does not implement
encryption and cannot prove those bytes were cryptographically sealed. That
proof belongs to the deferred custody implementation and external
cryptographic review. No production confidentiality claim follows from this
source-only type.

## Terminal result

`SignedTerminalReceiptV1` is the capsule-visible result. Its statement contains
the exact release-request hash and binding, outcome, and unique node keys,
decision hashes, contribution hashes, and contribution commitments. It does not
contain the provider-private contribution bytes.

The statement names an Ed25519 issuer and is signed. Verification requires the
Runtime-selected expected issuer, the verified release request, and the exact
set of verified contribution references. Every verified contribution must
retain and match the exact release-request hash and recipient. A successful
result requires the bound threshold. Contributions from another request or
recipient cannot be reused. The receipt is audit/result evidence, not a
portable grant that can authorize another release.

## Threat model and fail-closed rules

Tests reject wrong Wallet, attacker-signed victim address, wrong content,
policy, node set, threshold, Profile, session, action, request hash, issuer,
recipient, expiry, replay, and post-signature mutation. They also reject a
forged allow receipt, nonce reuse after changing fields, child lifetime escape,
a denied or cross-node decision, cross-request contribution reuse, duplicate
node/decision/contribution/commitment evidence, insufficient threshold,
malformed canonical input, and noncanonical Wallet/Profile encodings.

These contracts do not solve malicious custody nodes, rights-policy correctness,
key-share generation/storage/rotation, contribution encryption, durable replay
storage, issuer-key lifecycle, revocation, availability, decryption, rendering,
or product workflow safety. They provide bounded statements for those systems
to implement and audit. Remaining custody and product work is tracked in
`TASKS.md`.

The parent branch's provisional `elastos_common::protected_content` DTOs are
not this canonical contract. Integration must replace that surface atomically
after independent review. It must not add parallel decoders, migration adapters,
or compatibility fallbacks.
