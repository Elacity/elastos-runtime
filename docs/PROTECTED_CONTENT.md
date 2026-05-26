# Protected Content Provider

Protected content is Runtime-mediated. App, viewer, and content capsules ask to
open an object; they do not receive raw wallet, chain, IPFS, Elacity, or key
authority.

The contract is:

`capsule -> runtime capability -> elastos://drm/open -> drm-provider -> rights/key/decrypt providers`

## Current Slice

The repo now has the contract and fail-closed boundary, not production DRM:

- shared protected-content schemas in `elastos-common`
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

This is intentional. The first safe step is to make the authority boundary
unambiguous before adding dDRM contract reads or ElastOS dKMS.

PC2's dDRM contracts and WASM decrypt/render/media crates are useful
implementation references. They should enter Runtime only as provider-internal
backends behind `rights-provider`, `key-provider`, and `decrypt-provider`; they
must not give app or viewer capsules raw CEK, wallet, chain, IPFS, or Elacity
authority.

## Protected Object Shape

New protected objects should publish as sealed SmartWeb objects:

```json
{
  "schema": "elastos.sealed.object/v1",
  "payload_cid": "bafy...",
  "rights_policy_cid": "bafy...",
  "availability_receipt_cid": "bafy...",
  "key_envelope": {
    "scheme": "elastos-pq-hybrid-threshold-v0",
    "kid": "...",
    "wrapped_cek": "...",
    "policy_hash": "sha256:...",
    "algorithms": {
      "cipher": "aes-256-gcm",
      "signature": ["ed25519", "ml-dsa-65"],
      "kem": ["x25519", "ml-kem-768"],
      "share_scheme": "shamir-t-of-n"
    }
  },
  "viewer": {
    "required_interface": "elastos.viewer/document@1"
  }
}
```

`payload_cid` can be publicly reachable because protected payload bytes must be
encrypted before replication. Access is enforced by rights checks and key release,
not by hiding CIDs.

## Crypto Agility And dKMS Direction

FROST is a threshold Schnorr protocol, so it is classical ECC security, not a
post-quantum root. ElastOS may use FROST for short/medium-term receipt or cohort
signing, but new dKMS content must not depend on FROST as the long-term key
security foundation.

New protected content should use algorithm-agile sealed objects:

- Encrypt payload bytes with AES-256-GCM or ChaCha20-Poly1305.
- Split the AES-256 CEK into `t-of-n` shares.
- Wrap each share to an approved dKMS node with hybrid X25519 + ML-KEM-768.
- Sign release receipts with classical + PQ signatures where practical, starting
  with Ed25519 plus ML-DSA; use SLH-DSA for conservative hash-based signatures
  where size and speed are acceptable.
- Reconstruct the CEK only inside the key/decrypt provider boundary, then return
  scoped render/decrypt output to the viewer instead of raw CEKs.

Current EVM/BTC/ELA wallet proofs and dDRM chain state are still classical. They
are useful authorization inputs today, but they should not be the only permanent
identity or access root for long-lived encrypted assets.

References: [NIST PQC standards announcement](https://www.nist.gov/news-events/news/2024/08/nist-releases-first-3-finalized-post-quantum-encryption-standards),
[FIPS 203 ML-KEM](https://csrc.nist.gov/pubs/fips/203/final),
[FIPS 204 ML-DSA](https://csrc.nist.gov/pubs/fips/204/final),
[FIPS 205 SLH-DSA](https://csrc.nist.gov/pubs/fips/205/final),
and [RFC 9591 FROST](https://www.rfc-editor.org/rfc/rfc9591).

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

## Remaining Sequence

1. Wire real `elastos://drm/open` orchestration behind the declared sequence:
   content status/fetch, typed rights checks, key release, decrypt/render
   sessions, and signed release receipts.
2. Wire `key-provider` to an ElastOS PQ-hybrid threshold release backend.
3. Wire `decrypt-provider` to a real decrypt/render backend that keeps CEKs
   inside the provider boundary.
4. Wire real protected-content producers to the existing sealed-object publish
   contract after payload encryption, rights policy, availability receipt,
   provenance, key-envelope, and viewer-interface generation exist.
5. Add a permissioned ElastOS PQ-hybrid dKMS v0 for new content only.

No visible protected-content UI should ship before fail-closed provider tests and
capability-resource checks cover the full open path.

## Executable Proof

Run `scripts/protected-content-provider-contract-smoke.sh` after changing
protected-content provider capsules. It exercises the real provider binaries
over their JSON line protocol and verifies the current journey contract:

- status exposes blocked raw authority
- valid requests fail closed until backends are configured
- invalid raw-authority requests are rejected
- `drm-provider.open` reports the declared provider/runtime sequence
