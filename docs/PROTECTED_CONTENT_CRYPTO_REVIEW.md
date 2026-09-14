# Protected-content cryptographic review package

This document is the entry point for an external cryptographic reviewer who has
not seen this codebase. It states exactly which primitives the protected-content
path uses, which crate and version implements each one, what each construction
binds, what the system does and does not defend against, and which test pins
each claim. It is paired with a committed set of golden vectors
([`docs/audits/2026-09-protected-content-golden-vectors-v1.json`](audits/2026-09-protected-content-golden-vectors-v1.json))
and a replay test that regenerates them from the shipped code.

External cryptographic review remains open. Nothing here is a claim that the
suite has been reviewed; it is the material a reviewer needs in order to do
that review. No part of this document should be read as production
confidentiality evidence.

## 1. Scope and corrections

The scope is the confidentiality and key-custody path for protected content:
content sealing (media and non-media), threshold custody of the content key,
authorized key release, and the wallet signature that authorizes a rights
request. Authority signatures, rights-policy correctness, node admission and
rotation, operational custody governance, and the Runtime's own durable replay
storage are outside it.

Three things about this path are commonly misread, so they are corrected up
front.

**The `hpke` crate is a dependency in name only.** `elastos-protected-content-custody`
declares `hpke` and the crate graph shows it, but the only item imported from it
anywhere in the workspace is `hpke::rand_core` — the RNG trait bound, nothing
else. No HPKE encryption, key schedule, or ciphersuite is used. Verify with
`grep -rn 'hpke::' elastos/crates --include='*.rs'`: every hit is
`use hpke::rand_core::{...}`. The real key-encapsulation suite is the one named
by `CUSTODY_X_WING_AES256GCM_SUITE_ID_V1`,
`elastos-xwing-draft06-hkdf-sha256-aes256gcm/v1`: X-Wing draft-06 (ML-KEM-768
concatenated with X25519) to HKDF-SHA256 with no salt to AES-256-GCM. The
constants `STORED_SHARE_HPKE_INFO_V1` and `RELEASED_SHARE_HPKE_INFO_V1` keep an
`HPKE` spelling for wire compatibility; they are HKDF `info` labels, not HPKE
parameters.

**There are two content-encryption paths, not one.** Media is AES-128-CTR
CENC over fMP4; non-media objects use the newer `EPC1` chunked AEAD container,
which is AES-256-GCM. They share the same 32-byte content encryption key (CEK)
and the same custody, but the ciphers and their integrity properties differ.
Section 2 states both.

**A CENC `pssh` box now ships inside the public media file.** Protected fMP4
init segments carry a Protection System Specific Header for DRM system id
`b6e254ef-0dc5-47fe-94e7-0e72ed1dc7b0`, protection scheme
`cenc:elastos-pq-hybrid-threshold-v0`. Its `Data` payload is JSON describing
the scheme. It is a public descriptor by design and carries no key material —
see the threat model in section 3.

## 2. Suite card

Versions below are the resolved versions in `elastos/Cargo.lock`. Code
references name a module, not a line, because line numbers rot.

### 2.1 Key encapsulation — X-Wing draft-06

| | |
| --- | --- |
| Crate | `x-wing` 0.1.0 (pinned `=0.1.0`) |
| Construction | ML-KEM-768 (`ml-kem` 0.3.2) concatenated with X25519 (`x25519-dalek` 3.0.0) |
| Public key | 1216 bytes: 1184-byte ML-KEM-768 encapsulation key followed by the 32-byte X25519 public key |
| Ciphertext | 1120 bytes |
| Shared secret | 32 bytes |
| Secret key | a 32-byte seed; the decapsulation key is expanded from it |
| Code | `elastos-protected-content-custody/src/pq_hybrid.rs` |

Public keys are validated strictly on import, not merely length-checked: the
trailing X25519 component is rejected if it is low order, if it is a
non-canonical encoding with the high bit set, or if the two components appear
in the pre-draft-06 wire order. Pinned by
`pq_hybrid::tests::session_public_rejects_old_wire_order_and_invalid_final_x25519_component`
and `secrets::tests::recipient_public_key_rejects_invalid_xwing_bytes`.

Encapsulation uses `encapsulate_deterministic` with randomness drawn from the
caller's RNG, so a seeded RNG makes the whole seal reproducible. That is what
makes the end-to-end golden vector in section 5 possible.

### 2.2 Key derivation — HKDF-SHA256, no salt

| | |
| --- | --- |
| Crate | `hkdf` 0.12.4 over `sha2` 0.10.9 |
| Salt | none (`Hkdf::<Sha256>::new(None, shared_secret)`) |
| IKM | the 32-byte X-Wing shared secret |
| Output | a 32-byte AES-256 wrap key |
| Code | `pq_hybrid.rs`, `derive_wrap_key` |

The `info` string is domain-separated and length-prefixed, built as:

```text
"elastos.protected-content.xwing-draft06.hkdf-sha256/v1"
  || be16(len(suite_id)) || suite_id
  || be32(len(aad))      || aad
```

where `suite_id` is `elastos-xwing-draft06-hkdf-sha256-aes256gcm/v1` and `aad`
is the bound AAD of section 2.4. Because the AAD is inside the `info`, a wrap
key derived for one binding cannot be reached from another.

### 2.3 Share wrapping — AES-256-GCM

| | |
| --- | --- |
| Crate | `aes-gcm` 0.10.3 (with `aes` 0.8.4, `ghash` 0.5.1, `zeroize` on) |
| Key | the 32-byte HKDF output |
| Nonce | 12 bytes, drawn from the caller's RNG per seal |
| Plaintext | a 32-byte share or content key; ciphertext plus tag is 48 bytes |
| AAD | the bound AAD of section 2.4 |
| Envelope | 1180 bytes: 1120-byte X-Wing ciphertext, 12-byte nonce, 48-byte wrapped share |
| Code | `pq_hybrid.rs`, `share_wrap.rs` |

Nonces are random, not counter-derived. The wrap key is unique per
encapsulation, so nonce reuse across seals under the same key does not arise in
this construction.

### 2.4 What binds what

Every seal in this system is bound to the exact situation it was made for. The
AAD passed to AES-256-GCM is always `info || aad`, where `info` is a fixed
label naming the use and `aad` is a canonically encoded contract body.

| Seal | `info` label | Bound AAD |
| --- | --- | --- |
| Stored custody share | `elastos.protected-content.stored-share/v1` | `StoredShareAadV1`: the custody manifest hash and the exact node identity (node public key, node custody public key, share coordinate) |
| Released contribution | `elastos.protected-content.released-share/v1` | `ReleasedShareAadV1`: release-request hash, protected-content binding, node decision hash, node identity, recipient key identity |
| Possession challenge | `elastos.protected-content.recipient-possession/v1` | the possession transcript below |
| Decrypt-session CEK wrap | `elastos.protected-content.decrypt-session-cek/v1` | the caller's session transcript |

Canonical encodings are domain-separated: every contract body is encoded as its
domain string, a zero byte, then its fields, with byte strings and strings
length-prefixed with a big-endian `u16`. Decoding re-encodes and rejects any
input that is not the canonical form.

**Possession transcript.** `possession_transcript_v1` concatenates:

```text
"elastos.protected-content.recipient-possession-transcript/v1"
  || profile_public_key (32)
  || runtime_session_binding_digest (32)
  || canonical_bytes(recipient_key_identity)
  || request_hash (32)
```

**Runtime session binding, v3.** The `runtime_session_binding` digest above is
SHA-256 over:

```text
"elastos.protected-content.runtime-session-binding.v3"
  || be32(len) || principal_id
  || be32(len) || profile_did
  || be32(len) || executable_actor
  || be32(len) || launch_id
  || be32(len) || proof_binding_id
  || be32(len) || session_id
  || be32(len) || grant_id
  || be32(len) || mint_id
```

(`derive_runtime_custody_session_binding` in
`elastos-server/src/protected_content_runtime.rs`). Every field is
length-prefixed, so no two distinct field tuples share a preimage. The v3
change from v2 is that `executable_actor` is the verified capsule the caller
was actually launched as, rather than a hard-coded player id — which is what
lets a second viewer (the object reader) exist at all.

**EPC1 chunk AAD.** Each sealed chunk of a non-media object is bound to its
header and its index:

```text
"elastos.protected-content.payload.chunk-aad/v1" || 0x00
  || encoded_header_bytes
  || be64(chunk_index)
```

The per-chunk nonce is the header's 12-byte base nonce with bytes 4..12 XORed
with the big-endian chunk index, so the nonce is unique for every chunk index
under the header's size ceiling
(`payload::tests::payload_nonces_are_unique_for_every_valid_chunk_index_under_the_maximum_framed_bound`).

**Content key commitment.** `SHA-256("elastos.protected-content.content-key-commitment/v1" || 0x00 || cek)`,
carried in the public custody manifest. Reconstruction compares the
reconstructed key against it in constant time before the key is used.

**Encrypted content identity.** `SHA-256(framed_ciphertext)` together with the
framed byte length, carried in the signed custody manifest. This is the handle
that ties a staged file to an envelope.

**Terminal receipt digest.** The receipt statement is canonically encoded under
`elastos.protected-content.terminal-receipt-statement/v1` and covers the exact
release-request hash, the binding, the outcome, and, for each contributing
node, the node public key, decision hash, contribution hash, and contribution
commitment. It is signed with Ed25519 (`ed25519-dalek` 2.2.0) by a
Runtime-selected issuer key. It is result evidence, not a portable grant.

### 2.5 Threshold secret sharing — Shamir over GF(256)

| | |
| --- | --- |
| Crate | `vsss-rs` 6.0.1, `Gf256` |
| Parameters | 2-of-3 for the shipped committee shape; the threshold is carried in the manifest, not hard-coded in the primitive |
| Share wire form | 33 bytes: a one-byte non-zero share coordinate followed by the 32-byte share value |
| Randomness | a `rand` 0.10 `CryptoRng` (system RNG in production) |
| Code | `provision.rs` (`split_content_key`), `reconstruct.rs` (`Gf256::combine_bytes`) |

This is plain Shamir, not verifiable secret sharing. A node that returns a
wrong share is detected — reconstruction fails the content-key commitment — but
it is not identified, and there is no proof of correct sharing. That limitation
is deliberate and is also recorded in
[`docs/PROTECTED_CONTENT_CONTRACTS_V1.md`](PROTECTED_CONTENT_CONTRACTS_V1.md).

### 2.6 Content encryption

**Non-media objects — `EPC1`.** A framed container: the magic `EPC1`, a
big-endian `u16` header length, the encoded header (schema
`elastos.protected-content.payload/v1`, suite `aes-256-gcm-chunked/v1`, content
type, plaintext byte count, 12-byte base nonce, content-key commitment), then
one AES-256-GCM chunk per 1 MiB of plaintext. Each chunk carries its own
16-byte tag and the AAD of section 2.4. The object's identity is
`SHA-256` over the whole framed file together with its length. Code:
`elastos-protected-content-custody/src/payload.rs`.

**Media — CENC fMP4.** Suite id `cenc-fmp4-aes128ctr/v1`. The 128-bit CENC key
is `SHA-256("elastos.protected-content.cenc-aes128-key/v1" || cek)` truncated to
16 bytes; samples are encrypted with AES-128-CTR (`ctr` 0.9.2 over `aes` 0.8.4)
using 8- or 16-byte IVs, full-sample or subsample. Code:
`elastos-protected-content-custody/src/cenc.rs`, `protect.rs`, `play.rs`.

AES-128-CTR provides **no integrity**: there is no per-sample tag, as the CENC
standard intends. Integrity on the media path comes from elsewhere — every
staged segment is checked against its expected `ciphertext_sha256` and byte
length, and the fMP4 layout is validated, before a single byte is decrypted
(`validate_staged_segment_v1` in
`elastos-protected-content-provider-contracts/src/media.rs`). A reviewer should
treat "media sample integrity" as a property of the staged-file identity check,
not of the cipher.

### 2.7 CENC `pssh` descriptor

Built and parsed only in
`elastos-protected-content-provider-contracts/src/pssh.rs`. System id
`b6e254ef-0dc5-47fe-94e7-0e72ed1dc7b0`, protection scheme
`cenc:elastos-pq-hybrid-threshold-v0`, payload schema
`elastos.protected-content.cenc-pssh-data/v1`. The `Data` payload is JSON with
a fixed field set: the two suite ids, the 16-byte content access id (which is
also the `tenc` default KID), and the custody pool, epoch, and
committee-authorization identity digests with their canonical byte lengths.
Payloads above 4096 bytes are refused rather than parsed, and the constant
fields are checked on every parse.

### 2.8 Wallet signature — EIP-191

| | |
| --- | --- |
| Crate | `k256` 0.13.4 (`ecdsa`), Keccak-256 from `sha3` |
| Prefixed hash | `Keccak256("\x19Ethereum Signed Message:\n" || len(message) || message)` (`elastos_auth::ethereum_signed_message_hash`) |
| Signature | 65 bytes: `r || s || v`, where `v` is the raw recovery id 0 or 1, not 27 or 28 |
| Malleability | a signature whose `s` is not already low-S is rejected before recovery (`signature.normalize_s().is_some()` is an error) |
| Address | the low 20 bytes of `Keccak256` over the uncompressed public key minus its `0x04` prefix |
| Code | `elastos-protected-content-contracts/src/rights.rs`, `recover_wallet` |

## 3. Threat model

This is a statement of what each adversary can and cannot do, with the test
that pins each claim. Where no test pins a claim, that is said outright rather
than implied. A test list is not a threat model; the list of rejected inputs in
[`docs/PROTECTED_CONTENT_CONTRACTS_V1.md`](PROTECTED_CONTENT_CONTRACTS_V1.md)
is a companion to this section, not a substitute for it.

### 3.1 Assets

1. The 32-byte content encryption key for each protected object.
2. The plaintext content itself — fMP4 samples, or the plaintext of an `EPC1`
   object.
3. Custody node secrets: the 32-byte X-Wing seed each node wraps its share to,
   and the node's Ed25519 signing key.
4. The operation-scoped recipient secret and the decrypt-session secret.
5. Node-local stored share records on disk.

Not assets of this system: the user's wallet private key (held by the wallet,
never by the Runtime), and the public descriptors — custody manifest,
`pssh` payload, terminal receipt — which are designed to be readable.

### 3.2 Principals and trust boundaries

| Principal | Holds | Trusted for |
| --- | --- | --- |
| Protect provider (sandboxed) | the CEK for the duration of one sealing session | sealing correctly; it is not trusted after it provisions |
| Runtime | session bindings, committee selection, mint journal | admitting viewers, issuing the release operation; it holds no playback custody envelope or sealed-share bytes |
| Custody node (one of three) | one Shamir share, sealed to its own X-Wing key | contributing its own share under an authorized decision |
| Decrypt provider (sandboxed) | the recipient secret behind an opaque handle | reconstructing and re-wrapping; the CEK never leaves it in the clear |
| Viewer capsule | rendered output for one launch | nothing about authorization; every session field it sends is overwritten |

The boundaries that matter:

- **Viewer capsule to Runtime gateway.** The viewer is a sandboxed capsule
  reached through the gateway provider proxy. It cannot name its own session.
- **Runtime to sandboxed providers.** Process boundary; providers receive only
  what a Runtime-issued request declares.
- **Runtime to custody nodes.** A signed release operation in, a signed node
  contribution out. Nodes are mutually distrusting.
- **Public distribution.** The sealed object, the protected fMP4 (including its
  `pssh` box), and the custody manifest are all public artifacts.

### 3.3 A compromised viewer capsule

**Can.** See whatever the decrypt boundary hands it for the session it was
actually launched into. Compromising the renderer of content the user is
already authorized to view does not need any cryptographic break; that content
is already in front of the user.

**Cannot: name a different session.** The gateway strips `launch_id`,
`proof_binding_id`, `session_id`, `grant_id`, `executable_actor`,
`wallet_request_hex` and `wallet_response_hex` out of the viewer's request body
and re-injects them from the verified home launch token. Pinned by
`test_library_provider_runtime_custody_viewer_ops_require_player_launch_token`
in `elastos-server`'s gateway tests.

**Cannot: pose as a different viewer.** Protected viewer operations are refused
unless the launch token's `executable_actor` equals its selected resource and is
one of the two admitted viewer capsule ids. Same test.

**Cannot: reach the content key.** Reconstructed CEK bytes leave the custody
crate only inside a PQ-hybrid decrypt-session wrap, and `ContentEncryptionKeyV1`
exposes no raw-byte accessor across the crate's public API at all. Pinned by
`possession::tests::decrypt_session_wrap_hides_cek_and_rejects_wrong_session`
and `payload::tests::payload_debug_and_public_outputs_do_not_expose_content_key_bytes`.

**Cannot: replay a decrypt-session wrap into another session.** The wrap's AAD
is the session transcript; unwrapping under a different transcript or a
different session secret fails closed. Same `decrypt_session_wrap_...` test, and
`possession::tests::possession_fails_closed_on_wrong_transcript`.

**Not pinned.** There is no unit test asserting the v3 session-binding preimage
against a fixed expected digest; the binding's collision resistance rests on
the length-prefixed encoding being read correctly, and on SHA-256. A reviewer
should treat the v3 preimage as reviewed by inspection only.

### 3.4 A compromised single custody node

**Can.** Read its own share in the clear. Refuse to contribute, which denies
service for that object if the committee then falls below threshold. Emit a
signed contribution carrying whatever bytes it likes.

**Cannot: reconstruct the content key alone.** 2-of-3 Shamir over GF(256): one
share is information-theoretically independent of the secret. Pinned by
`reconstruct::tests::insufficient_shares_fail_before_open` and
`reconstruct::tests::authenticated_operation_insufficient_shares_fail_before_open`,
and, at the primitive level, by the single-share check inside
`shamir_gf256_2_of_3_splits_and_recombines_as_recorded` in the golden-vector
replay.

**Cannot: substitute a wrong share undetected.** The reconstructed key is
compared against the manifest's content-key commitment in constant time.
Pinned by
`reconstruct::tests::malicious_signed_wrong_share_is_rejected_by_content_key_commitment`
and its `authenticated_operation_` twin. *Detection only* — the system does not
identify which node cheated, and this is not verifiable secret sharing.

**Cannot: use another node's stored share.** The stored-share AAD binds the
custody manifest hash and that node's exact identity, so a share moved to
another node fails to open. Pinned by
`release::tests::release_reaches_hpke_failure_for_request_bound_to_wrong_node_aad_stored_share`
and `node_share::tests::node_local_share_rejects_selected_node_substitution`.

**Cannot: reuse a contribution across requests or recipients.** The released
share's AAD binds the release-request hash, the binding, the decision hash and
the recipient key identity. Pinned by
`reconstruct::tests::wrong_request_contribution_is_rejected`,
`reconstruct::tests::wrong_recipient_secret_is_rejected`, and
`reconstruct::tests::authenticated_operation_wrong_terminal_release_request_is_rejected`.

**Cannot: be impersonated by a Runtime-side substitution.** A claimed operation
addressed to a different node, a wrong custody secret, or a wrong signing key
all fail before any decapsulation. Pinned by
`release::tests::release_rejects_claimed_operation_for_a_different_node`,
`release::tests::release_rejects_wrong_custody_secret`, and
`release::tests::release_rejects_wrong_signing_key`.

**Not defended.** Two colluding nodes out of three reconstruct the key. That is
the stated threshold, not a defect — but it means the security of an object is
exactly the security of the weakest two of its three custodians, and the
committee shape is therefore a deployment decision, not a cryptographic one.

### 3.5 A replayed release operation

**Can.** Resend an identical claim and receive the identical stored
contribution. This is intended: the node-local store is idempotent so that a
crashed or retried release does not consume a second authorization.

**Cannot: produce a second, different effect.** The durable store claims both
replay keys atomically and replays only the exact persisted result. Pinned by
`replay_store::tests::durable_store_replays_exact_contribution_after_restart_without_new_effects`,
`replay_store::tests::durable_store_rejects_mismatched_retry_without_mutating_stored_result`,
and `replay_store::tests::durable_store_rejects_partial_overlap_without_claiming_new_key`.

**Cannot: succeed after a claim with no result.** A claim that was recorded but
never completed fails closed rather than being treated as an approval. Pinned by
`replay_store::tests::durable_store_claim_without_result_fails_closed`.

**Cannot: outlive its window.** Pinned by
`reconstruct::tests::authenticated_operation_expiry_is_rejected` and
`release::tests::release_rejects_expires_at_equal_to_now`.

**Cannot: be forged by tampering with the store on disk.** Corrupt, truncated,
oversized, same-length-corrupted, forged-digest, symlinked and hard-linked
store states are all refused. Pinned by
`replay_store::tests::durable_store_rejects_corrupt_truncated_and_oversized_state`,
`replay_store::tests::durable_store_rejects_same_length_state_corruption_and_forged_digest`,
and `replay_store::tests::durable_store_rejects_symlinked_loose_mode_and_hard_linked_paths`.

**Not pinned here.** This is the *node-local* replay store. Runtime-side
durable replay storage is explicitly out of scope for the current contracts, so
a replay defence at the Runtime tier is not claimed and no test pins one.

### 3.6 A tampered staged file

**Can.** Corrupt, truncate, extend, reorder or splice the bytes of a staged
ciphertext on disk, and edit the public `pssh` descriptor inside a protected
media file.

**Cannot: have a tampered `EPC1` object decrypt.** Every chunk is AES-256-GCM
with the header and chunk index in its AAD, and the framed file as a whole is
bound by its `SHA-256` and length in the signed manifest. Pinned by
`payload::tests::payload_rejects_header_chunk_tag_order_duplication_splice_length_and_type_tampering`,
`payload::tests::payload_rejects_truncation_and_trailing_bytes`, and
`payload::tests::chunk_decrypter_round_trips_and_rejects_reordered_chunks`.

**Cannot: substitute a different object or key.** Pinned by
`payload::tests::payload_rejects_wrong_key_and_wrong_commitment`, and by
`payload::tests::decrypt_output_rejects_wrong_ciphertext_before_any_plaintext_write`,
which also establishes that no plaintext is written before the check fails.

**Cannot: have a tampered media segment decrypt.** Each staged segment is
verified against its expected `ciphertext_sha256` and byte length, and the fMP4
layout is validated, before any AES-128-CTR keystream is applied. Pinned by
`play::tests::playback_helper_fails_closed_for_changed_source_wrong_session_and_bad_lengths`.

**Cannot: gain anything from the `pssh` box.** The box is a public JSON
descriptor of the scheme that ships inside a public media file. It contains no
key material, no share, and no secret — only the two suite ids, the content
access id a consumer presents when it asks for a key, and the custody pool,
epoch and committee-authorization identity digests that say which quorum can
answer. Editing it cannot produce a key; a consumer that follows an edited
descriptor is redirected to a quorum that will not authorize it. Pinned by
`pssh::tests::build_then_parse_round_trips_and_the_payload_carries_no_secret`,
`pssh::tests::payload_validation_pins_every_constant_and_rejects_bad_hex`, and
`pssh::tests::a_foreign_system_id_is_never_read_as_ours` in
`elastos-protected-content-provider-contracts`.

**Weaker than it looks.** On the media path the tamper-evidence is a hash
comparison of the staged file, not an authenticated cipher. If a deployment
ever decrypts a media segment that has not been through
`validate_staged_segment_v1`, that segment has no integrity protection at all.
The `EPC1` path does not have this shape: its integrity is in the cipher.

## 4. Build card

| | |
| --- | --- |
| Toolchain | Rust 1.91.0, pinned in `rust-toolchain.toml` (`rustfmt`, `clippy`) |
| Workspace | `elastos/Cargo.toml`, resolver 2, one lockfile at `elastos/Cargo.lock` |
| Crate under review | `elastos/crates/elastos-protected-content-custody`, which is `#![forbid(unsafe_code)]` |

Direct cryptographic dependencies of the custody crate, as resolved:

```text
aes 0.8.4            aes-gcm 0.10.3       ctr 0.9.2
ed25519-dalek 2.2.0  ghash 0.5.1          hkdf 0.12.4
hpke 0.13.0          sha2 0.10.9          subtle 2.6.1
vsss-rs 6.0.1        x-wing 0.1.0         zeroize 1.9.0
```

reached transitively: `ml-kem` 0.3.2, `x25519-dalek` 3.0.0, `sha3` 0.12.0 (via
`x-wing`), `elliptic-curve` 0.14.1 (via `vsss-rs`), `k256` 0.13.4 (rights).

Three RNG generations coexist because the dependencies require it: `rand` 0.9
for anything behind the `hpke::rand_core` bound, `rand` 0.10 for `vsss-rs`, and
`rand_core` 0.6 as a transitive floor. Production code draws from the operating
system in both generations.

Reproduce:

```bash
cd elastos
cargo tree -p elastos-protected-content-custody --edges normal --depth 1
cargo metadata --locked --format-version 1 > /dev/null   # lockfile is authoritative
cargo build -p elastos-protected-content-custody
cargo test  -p elastos-protected-content-custody
cargo clippy -p elastos-protected-content-custody --all-targets -- -D warnings
```

## 5. Golden vectors

The committed vectors are
[`docs/audits/2026-09-protected-content-golden-vectors-v1.json`](audits/2026-09-protected-content-golden-vectors-v1.json),
schema `elastos.protected-content.golden-vectors/v1`.

Regenerate:

```bash
cargo run -p elastos-protected-content-custody --example golden_vectors
```

Replay:

```bash
just test-crate elastos-protected-content-custody --test golden_vectors
```

Every vector is produced by the code that ships — this crate's public API, or
the exact dependency version this crate links. Nothing is re-implemented for
the purpose of generating a vector, because a vector produced by a second
implementation would only test that second implementation. The replay test also
fails if the schema string changes, so the file's contract with any external
consumer cannot move silently.

| Family | What it pins | Why it is here |
| --- | --- | --- |
| `xwing_decaps` | X-Wing draft-06 decapsulation: seed, encapsulation key, ciphertext, shared secret | The KEM is the root of the whole suite; recording one deterministic encapsulation means replay only has to decapsulate |
| `hkdf_sha256_no_salt` | HKDF-SHA256 in no-salt mode | Entry 0 is RFC 5869 test case A.3, so a reviewer can check the primitive against a published vector without trusting this repository |
| `shamir_gf256_2_of_3` | `vsss-rs` `Gf256` split and combine, in the crate's own 33-byte share wire form | Pins the threshold parameters and the share encoding, and asserts that a single share does not reconstruct |
| `eip191_recover` | The EIP-191 prefixed hash, low-S rejection, recovery and address derivation | The wallet signature is the only classical-authority step in the confidentiality path |
| `decrypt_session_keygen` | `mint_decrypt_session_from_seed`: labelled SHA-256 over the seed, then an X-Wing key | Pins the derivation a decrypt session's identity depends on, recording only the public key |
| `recipient_key_identity` | Recipient public key and the canonically encoded `RecipientKeyIdentityV1` it binds to | This identity is what an authorization names, so a change to it changes who a release can be answered to |
| `possession_challenge_seal` | The whole seal end to end: transcript preimage, X-Wing encapsulation, the HKDF `info` construction, AES-256-GCM over the bound AAD, and the canonical envelope | The one byte-for-byte vector over shipped code. Any change to the transcript layout, the `info` encoding, the AAD binding or the envelope framing moves these bytes |
| `epc1_framed_chunk_ranges` | `EPC1` framing arithmetic: 1 MiB plaintext chunks, a 16-byte tag per chunk, contiguous ranges | The object container is the newest path; this pins its geometry with no key material involved |

**A deliberate gap.** There is no keyed `EPC1` chunk vector. Producing one
needs a `ContentEncryptionKeyV1` built from known bytes, and this crate does not
expose raw content-key bytes across its public API — which is itself a
property worth keeping. Widening the API to generate a vector would weaken the
thing the vector is meant to reassure a reviewer about. The keyed behaviour is
pinned instead by the crate's in-module tests, named in section 3.6.

The `possession_challenge_seal` and `shamir_gf256_2_of_3` vectors are
reproducible because the generator drives a seeded RNG. Those seeds are test
seeds and carry no production secret.

## 6. Open items for the external reviewer

1. **The whole suite is unreviewed.** No external cryptographic review of this
   construction has been performed. This document exists to make one possible.
2. **X-Wing draft-06.** The construction is a draft, and `x-wing` 0.1.0 is a
   pre-1.0 crate pinned to an exact version. A reviewer should confirm the
   implementation matches the draft it claims, including the concatenation
   order and the encapsulation-key validation, and should form a view on the
   draft-to-final migration path.
3. **Shamir is not verifiable.** A wrong share is detected by the content-key
   commitment but not attributed. Whether attribution is needed at 2-of-3, and
   what a VSS migration would cost, is an open design question.
4. **Two colluding custody nodes recover the key.** The threshold is the whole
   confidentiality argument against custodians. Whether 2-of-3 is the right
   shape, and how independence between node operators is actually enforced, is
   a deployment question a reviewer should press on.
5. **Media sample integrity.** AES-128-CTR CENC has no per-sample
   authentication. Integrity depends entirely on the staged-segment hash check
   running before decryption on every path. A reviewer should look for any path
   that decrypts without it.
6. **CENC key derivation truncates.** The 128-bit CENC key is a truncated
   SHA-256 over a label and the 256-bit CEK. A reviewer should confirm the
   separation between this derived key and the CEK's other uses is what is
   intended.
7. **AEAD state zeroization is partial.** This crate zeroizes CEK bytes and
   plaintext chunk buffers, and the `aes` crate's `zeroize` support clears
   round keys where it implements them, but the composite `Aes256Gcm`/GHASH
   state has no complete public zeroization contract across backends — notably
   the AArch64 PMULL POLYVAL path. The code keeps cipher lifetimes short and
   makes no stronger claim. Whether that is sufficient is a reviewer question.
8. **Authority signatures remain classical.** Confidentiality is PQ-hybrid;
   Ed25519 and secp256k1 authority signatures are not. Full post-quantum
   authorization is an open pre-activation decision.
9. **`hpke` is a dependency carrying no use.** It is linked only for
   `rand_core`. Likewise `rand_chacha` is declared by the custody crate and
   imported nowhere. Both should be removed or justified; a reviewer should not
   have to work out that the HPKE in the dependency graph is not load-bearing.
10. **The v3 session-binding preimage has no fixed-digest test.** It is
    reviewed by inspection only. See section 3.3.
11. **Randomness sourcing.** Nonces, encapsulation randomness and Shamir
    coefficients all come from the operating system RNG in production, across
    two `rand` generations. A reviewer should confirm no seeded or fallback RNG
    can reach a production path.
