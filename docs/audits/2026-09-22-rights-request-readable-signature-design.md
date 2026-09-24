# A rights request a person can read — signature format design

Date: 2026-09-22. Status: **implemented**; the security review is still owed.
The design below is what shipped, with the differences from the first draft
recorded at the end.
Scope: the wallet signature over `RightsRequestV1` on the protected-content
open path. The custody ceremony, the release operation and the decrypt
provider are unchanged.

## Why

Opening a protected item asks the owner to sign a rights request. Today that
request is presented as its raw canonical bytes, hex-encoded for transport and
decoded by the wallet for display. What a person sees is roughly two kilobytes
beginning `elastos.protected-content.rights-request/v1` and continuing in
replacement characters.

That is not a correctness defect. The bytes the wallet signs are exactly the
bytes the Runtime verifies, and the signature is sound — an open on 2026-09-22
completed through this path. It is a consent defect. An approval screen exists
so a person can decide, and nobody can decide about a screen of binary. The
honest description of today's prompt is that it asks for trust rather than
informed agreement.

The goal is to show what matters and keep binding everything.

## What the request actually contains

```rust
pub struct RightsRequestV1 {
    binding: ProtectedContentBindingV1,   // the bulk: encrypted-content
                                          // identity, key envelope, custody
                                          // pool identity
    action: RightsActionV1,
    recipient: RecipientKeyIdentityV1,
    issued_at: u64,
    expires_at: u64,
    replay_nonce: ReplayNonce16,
}
```

One field is large and none of it is meaningful to a reader.

## Why the prompt is binary at all

The handoff sends the canonical bytes as `0x`-prefixed hex, and a wallet
**decodes** `0x` input before it displays and signs it. So the wallet is
showing the decoded bytes, which is why a dozen SHA-256 digests arrive on
screen as replacement characters.

A message that is not `0x`-prefixed is treated as UTF-8 text and rendered
verbatim. That single fact decides the design: the digests can be readable hex
simply by making the signed message readable text.

## The recommended shape: a readable signed message

EIP-191 personal_sign over plain text, the same construction SIWE uses, which
every wallet renders natively:

```
ElastOS — open protected content

Action:    View
Item:      <title from the verified listing>
Account:   0x1f2e…9ab
Expires:   2026-09-22 13:31 UTC

Content:   0x8f3a…c21d
Envelope:  0x44b1…07ef
Session:   0x9d0c…3a55
Nonce:     0x1693e993…
```

Four lines a person can check, and the mandatory digests present as hex rather
than mojibake. No typed-data encoder, no ABI encoding, no new dependency. The
digests do not become more trustworthy by being displayed — nobody verifies a
hash by eye — but they stop being noise a reader has to scroll past.

### The trap this shape introduces

Plain text has no structure, so a field carrying a newline can forge another
line. A title of `x\nAccount: 0xattacker` reads as an extra field. SIWE solves
this with a strict grammar; here the rule is that the title is rejected, not
escaped, when it carries a control character, and that it comes only from
verified listing state. That check is small and must not be skipped.

## The alternative: EIP-712 typed data

EIP-712 typed data, where the fields a person needs are fields a person reads,
and everything else is bound by hash rather than displayed:

| field | shown as | purpose |
| --- | --- | --- |
| `action` | "View" | what is being authorised |
| `item` | the listing's own title | what it is being authorised over |
| `account` | `0x…` | which account authorises it |
| `expiresAt` | a time | how long the authorisation lives |
| `binding` | `0x…` | keccak of the canonical binding |
| `recipient` | `0x…` | keccak of the recipient identity |
| `nonce` | `0x…` | the replay nonce |

Seven named rows in place of two kilobytes. Four carry meaning; three are
hashes that keep the binding complete.

Typed data is structurally unambiguous, so it needs no grammar rule for
injection. That is its advantage over the readable message above. It costs a
typed-data encoder on the producer and both verifiers, and the readable message
reaches the same outcome for a reader, so it is recorded here as the option to
take if the injection rule proves awkward rather than as the default.

An EIP-712 domain pinning the name and chain id is included, so a signature
produced for one deployment cannot be replayed against another. The canonical
identities already make that hard; the domain makes it structural.

## The rule that governs this change

**Whatever is displayed must also be signed.** A field shown but not bound lets
the prompt state one thing while the signature authorises another, which is
worse than today's unreadable prompt, because it looks trustworthy. So `item`
sits inside the signed struct, and its value is derived from the verified
listing rather than from anything a caller supplies.

The companion rule: whichever shape is chosen, the fields it commits to must
cover exactly what the current canonical encoding covered. A field that
silently leaves the signed material has widened what a signature authorises.
This is the part that needs a reviewer rather than an author.

## Verification follows the format, by construction

This is the load-bearing part of the design, and the reason the change is
tractable rather than fragile.

The signature is recovered in exactly two places today, and both already derive
what they hash from one definition — `RightsRequestV1::canonical_bytes()`:

| site | what it does | reached by |
| --- | --- | --- |
| `capsules/wallet-provider/src/approval.rs:779` | `ethereum_signed_message_hash(canonical_bytes)`, recover, compare to the account | the wallet, at completion, before it stores a signed result |
| `elastos-protected-content-contracts/src/rights.rs::verify_unclaimed` | `recover_wallet(request.canonical_bytes())`, compare to `binding.wallet()` | the Runtime coordinator **and every custody node** |

The third site is the producer: `capsules/wallet-provider/src/crypto.rs:196`
builds the connector handoff from the same canonical bytes.

So the change introduces one function in the contracts crate —

```rust
impl RightsRequestV1 {
    /// The exact text a wallet is asked to sign.
    pub fn signing_message(&self, item: &str) -> Result<String, ContractError>;
    /// The digest a wallet produces and every verifier recovers against.
    pub fn signing_hash(&self, item: &str) -> Result<[u8; 32], ContractError>;
}
```

— and all three sites call `signing_hash()` instead of hashing bytes
themselves. Producer and both verifiers then cannot disagree, because there is
no second definition to drift from. That is the same discipline that
`expected_runtime_custody_viewer_capsule` gives the kind-to-viewer check, and
the absence of it is what let the marketplace parser and its producer diverge
for twenty-four days.

`item` has to reach the request itself rather than being passed alongside it,
since a verifier must reproduce the exact message. The cleanest form is a
`RightsRequestV2` carrying the title as a bound field, which also gives the
version bump the cutover needs, and which is where the control-character rule
is enforced once rather than at each caller.

## Scope

1. `RightsRequestV1` → `V2` with the bound title, and `signing_message` /
   `signing_hash` beside it. The title is refused, not escaped, when it carries
   a control character.
2. The wallet provider's handoff, so the connector sends the readable message
   rather than a `0x`-prefixed hex string, and its completion branch, so it
   recovers against `signing_hash`.
3. `verify_unclaimed`, so the Runtime and the nodes recover the same way.
4. **A custody image rebuild.** The nodes verify the wallet signature through
   the shared crate, so they must ship the new format together with the host.
   `deploy/custody-host/Dockerfile` builds `elastos-server` and the providers
   from source.
5. A coordinated cutover. Signatures in the old format stop verifying, so host,
   capsules and custody image move together — the same one-revision rule the
   2026-09-21 install followed.

## For the reviewer

- Does the signed message commit to exactly what the canonical encoding
  covered? Name any field that changed side.
- Can any value reaching the message carry a newline, a colon or a control
  character, and is it refused rather than escaped?
- Is `item` derived only from verified listing state on every path that can
  reach it?
- Does the message name this deployment, so a signature cannot be replayed
  against another, without weakening the existing identity binding?
- Do producer and both verifiers reach `signing_hash` with no remaining
  byte-hashing path? A grep for `ethereum_signed_message_hash` in the rights
  path should return nothing after the change.
- Is there a window where a node on the old format meets a host on the new one,
  and does it fail closed?

## Related

- [Protected content](../PROTECTED_CONTENT.md)
- [Protected-content crypto review](../PROTECTED_CONTENT_CRYPTO_REVIEW.md)
- The dkms line carries a full EIP-712 implementation at
  `capsules/wallet-provider/src/crypto/evm/typed_data.rs`; this branch already
  has the equivalent machinery, used today for `browser_typed_data_sign`.


## What shipped, and how it differs from this design

Implemented 2026-09-22. The readable message was taken and EIP-712 was not.

**The title was dropped.** The design put the listing's title in the signed
message and then needed a rule to stop a title forging a line. Leaving it out
removes the rule and the risk together: every field in the shipped message is
an enum, a number or a hash, so no caller-supplied text reaches it and there is
nothing to escape or refuse. `RightsRequestV2` was therefore unnecessary; the
format changed without a new version of the structure.

The message that ships:

```
ElastOS

Open protected content.

Action:  View
Account: 0x4a62316623ad457f02cdc5d997ded67a383ec569
Expires: 2000000180 (unix seconds)
Request: 0xec40121ab038cfea46e2d243360a851a6e77c505f9d379cc844da2e52388d753
```

`Request` is the canonical hash of the whole request, and the three readable
lines are inside it, so none of them can be altered without changing it. The
expiry is a labelled epoch rather than a formatted date, because formatting one
would mean a date dependency in the crate every custody node links.

**One production path was missed by the design and caught by a test.** Managed
accounts sign in-process through `sign_managed_approval`, which the design's
three-site list did not name. An existing test asserts that managed and
external accounts produce identical signed results, and it failed until that
path also used `signing_message`. The three-site list should have been four.

**Golden vectors moved, twice.** The committed vectors in
`canonical_signature_golden_vectors` pin the wallet signature bytes and the
rights request hash, and both changed with the format; the release operation
hash moved with them because it covers the signed request. They moved a second
time when the word "(unix seconds)" was added, which is the clearest possible
demonstration that the message *is* the signature. Every other identity vector
is untouched, which is the evidence that only the presentation changed and not
what is bound. A reviewer of #48 should treat the new values as new evidence
rather than corrected ones.

**The cutover proved itself.** Four server tests failed after the change
because they launch the real provider binaries, which still linked the old
contract. Rebuilding protect, decrypt and custody made them pass. The same
applies to `elastos-custody-host`: the nodes verify through the shared crate,
so the image must be rebuilt from this revision before an installed run.

Verified: 3,215 workspace tests pass with 0 failures; wallet-provider 97;
`cargo clippy --workspace --all-targets -D warnings` clean; workspace and
wallet-provider fmt clean. A new test pins the prompt text itself, since
changing a word there changes every signature the product produces.

Still owed: the security review named above, and an installed run on a rebuilt
custody image.
