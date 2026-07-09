# Mandate & Receipt Wire Format — v1 (Sprint 49)

The portable formats behind Flint's "Grant → Act → Prove" loop, specified to the byte so an
independent party can (a) verify a `MandateReceipt` with no Flint code — completely for
`capability`-scope receipts and for `contiguous` receipts composed of the §5 events (other event
variants' serialized shapes live in the code, not yet in this spec), PROVIDED the implementation
uses a byte-compatible JSON encoder (§2's canonicalization rules; a mismatched encoder produces
false NEGATIVES, never forgeries) — and (b) construct a signed intent a Flint runtime will
accept. Everything here is FROZEN: verification re-serializes and
re-hashes these shapes, so any change is a new versioned schema, never an edit. A conformance
ratchet (`primitives::audit::tests::the_wire_format_matches_spec_mandate_v1`) pins this document
to the code.

Primitives: SHA-256; ed25519 (RFC 8032) verified STRICTLY (canonical S, small-order keys
rejected — two conforming verifiers must never disagree on a malleated signature); base64
(standard alphabet, padded, canonical trailing bits) for signatures; lowercase hex for hashes and
keys (verifiers SHOULD reject non-lowercase — the §4 binding signs the literal ASCII bytes);
JSON with the field names and order given here (serde-emitted; order is declaration order).
Domain separation between the three signature contexts (§2 record, §4 binding, §7 intent) rests
on the domain strings INSIDE each preimage — not on message length (two of the three signed
values are 32 bytes).

## 1. Trust model (read first)

- **Self-asserted signer.** A receipt verifies against the ed25519 key *it carries*
  (`signer_public_key_hex`). Anyone can mint a key and fabricate a receipt that verifies against
  itself. A consumer MUST pin the expected issuer key out-of-band (the `--signer` argument /
  `expected_signer_hex`); an unpinned verification is a structural check, **not a trust decision**
  (exit code 3, never 0).
- **Tamper-evident, not tamper-proof.** The format detects edits, interior drops, additions, and
  reorders by a HOLDER in transit (including of the settlement reference) — with ONE exception:
  a `contiguous` receipt WITHOUT a `set_binding` does not detect records truncated off the END
  together with the binding field (linkage fixes the interior, not the end; demand a bound
  receipt if end-truncation matters). It does not bind the key-holding ISSUER: a compromised
  runtime can sign a fabricated or selective set.
- **As-of-export, not as-of-now.** A receipt (and its §4 binding) fixes the set *at export
  time*. A holder possessing an earlier honestly-signed export (e.g. pre-revoke) can present it
  and every check passes. A consumer needing currency MUST demand a fresh export; the format
  carries no freshness anchor.
- **Completeness bounds.** A `contiguous` receipt proves an unbroken run but records truncated off
  the END need an external head anchor to detect. A `capability` receipt proves no *holder* altered
  the set (§4) but cannot prove the issuer omitted nothing at export.

## 2. The chained record

One audit record as it appears in a receipt (and on disk, one JSON object per line):

```json
{
  "seq":         1,                  // u64, monotonic, first record = 1
  "prev_hash":   "<64 hex>",         // prior record_hash; genesis = 64 zeros
  "event":       { … },              // the audited event (§5)
  "record_hash": "<64 hex>",         // see below
  "alg":         "ed25519",          // or "none" (unsigned logs; receipts REQUIRE ed25519)
  "sig":         "<base64>"          // ed25519 over the 32 raw record_hash bytes; "" iff alg=none
}
```

**Record hash preimage** (concatenation, no separators):

```
record_hash = SHA-256( "elastos.runtime/audit-chain/v1"   // domain, ASCII, no NUL
                     ‖ seq  as 8-byte big-endian
                     ‖ prev_hash as 32 raw bytes
                     ‖ event_json )
```

`event_json` is the event's canonical serde-JSON serialization. Verifiers MUST re-serialize the
*deserialized* event and hash those bytes (never the bytes as received) — this is the one
canonicalization recipe, shared by the on-disk chain walker and the receipt verifier.

**Record signature:** ed25519 over the 32 raw bytes of `record_hash` (not the hex string).

## 3. The mandate receipt document

```json
{
  "schema": "elastos.mandate_receipt/v1",       // verifier fail-closes on any other value
  "signer_public_key_hex": "<64 hex>",          // the issuing runtime's ed25519 key (§1 caveat)
  "scope": { "kind": "contiguous" }             // or:
           { "kind": "capability", "token_id": "<token>" },
  "records": [ ChainedRecord, … ],              // ascending seq; records[0] = the mandate grant
  "set_binding": "<base64>"                     // §4; REQUIRED for capability scope, else optional
}
```

An absent `scope` key means `contiguous` (legacy). By convention `records[0]` is the
`capability_grant` (the mandate) and the rest are the acts taken under it — but verifiers MUST
NOT require the grant at index 0 (the capability rule is "exactly one grant", position-free).

## 4. The set binding

The issuer's signature fixing the exact ordered record set — what stops a holder trimming a use or
revoke in transit from a `capability` receipt (whose membership is otherwise a keyless filter).
Ed25519 over:

```
binding_message = "elastos.runtime/mandate-receipt-set/v1"        // domain, ASCII
                ‖ scope_tag                                        // 1 byte: 0=contiguous, 1=capability
                ‖ [ len(token_id) as 8-byte BE ‖ token_id bytes ]  // capability scope only
                ‖ record_count as 8-byte big-endian
                ‖ concat( record_hash as the 64 ASCII hex bytes, in order )
```

Note the record hashes enter as their 64-char ASCII hex (fixed width — position + count make the
encoding unambiguous), not as raw bytes.

## 5. Mandate-relevant events

Events are a serde `AuditEvent` enum, INTERNALLY tagged: the variant name rides a `"type"` field,
FIRST, followed by the variant's fields in declaration order (snake_case). Because `event_json`
is hashed (§2), the byte encoding is normative: COMPACT JSON (no whitespace), keys in exactly the
order shown, serde_json string escaping (control chars escaped; `/` NOT escaped; non-ASCII as raw
UTF-8, never `\uXXXX`). The three events the mandate loop uses, as
byte-exact templates (`timestamp` is `{"unix_secs":u64,"monotonic_seq":u64}` — wall clock plus a
monotonic counter that survives clock steps):

```json
{"type":"capability_grant","timestamp":{…},"token_id":"…","capsule_id":"…","resource":"…","action":"…","expiry":null}
{"type":"capability_use","timestamp":{…},"token_id":"…","capsule_id":"…","resource":"…","action":"…","success":true}
{"type":"capability_revoke","timestamp":{…},"token_id":"…","reason":"…"}
```

- **`capability_grant`** — the mandate itself. `expiry` is a nullable timestamp (literal `null`
  when unset); optional `responsible_entity` (the accountable legal-entity DID) is APPENDED after
  `expiry` when set and ABSENT — not null — when unset (a frozen rule so pre-S32 chains
  re-verify).
- **`capability_use`** — one act. Optional `rail_ref` is APPENDED after `success` when set,
  ABSENT for non-rail acts: the external settlement reference for money acts (currently
  `drm:tx=<hash>;op=…;tid=…;price=…;tok=…` or `erc20:tx=<hash>;to=…;amount=…;tok=…` — see
  SPEC-market-provider-v1 §4; the key lists are informative — a receipt verifier treats
  `rail_ref` as an opaque string).
- **`capability_revoke`** — the kill switch: exactly `type`, `timestamp`, `token_id`, `reason`
  (there is NO `capsule_id` on a revoke).

Exact field lists are pinned by the conformance ratchet; any addition must be
`#[serde(default, skip_serializing_if = …)]`-append-only so old chains re-serialize byte-identically.

## 6. Verification algorithm

Given a receipt and an optional pinned signer key, compute the verdict:

1. `schema == "elastos.mandate_receipt/v1"` and `records` non-empty, else INVALID (hard error).
2. Decode `signer_public_key_hex` → 32-byte ed25519 key, else INVALID.
3. For each record, in order:
   a. **Linkage** (records after the first): `prev_hash == prior.record_hash` and
      `seq == prior.seq + 1`, else `chain_linkage_ok = false`.
   b. **Hash**: re-serialize `event`, recompute §2's preimage, compare to `record_hash`, else
      `hashes_ok = false`.
   c. **Signature**: `alg` MUST be `"ed25519"`; verify `sig` over the *recomputed* hash bytes —
      NEVER over the claimed `record_hash` (verifying the claimed bytes would let a tampered
      event ride a self-consistent hash+sig pair) — else `signatures_ok = false`.
4. **Scope rule** (`scope_ok`):
   - `contiguous`: `chain_linkage_ok` (nothing interior dropped).
   - `capability`: every record's event carries the scope's `token_id` AND exactly one record is a
     `capability_grant` AND `seq` is strictly ascending.
5. **Set binding** (`set_binding_ok`): a PRESENT `set_binding` MUST verify over §4's message for
   BOTH scopes ("optional" governs only whether ABSENCE is permitted — a present-but-stale
   binding is INVALID, never skipped). Absence: REQUIRED for `capability` scope (absent/null ⇒
   false); permitted for `contiguous` (absent AND null both accepted — Flint emits `null` for
   unsigned contiguous exports).
   Edge behaviors an implementation must reproduce: an undecodable `prev_hash` fails BOTH
   `hashes_ok` and `signatures_ok` for that record and processing continues; a record whose event
   cannot be re-serialized is a hard INVALID verdict (early error); `record_hash`/`prev_hash` hex
   case is tolerated when decoding in step 3, but §4 signs the LITERAL ASCII bytes of
   `record_hash` as stored — a case change breaks the set binding.
6. `structurally_valid = hashes_ok ∧ signatures_ok ∧ scope_ok ∧ set_binding_ok` — note linkage
   participates only THROUGH `scope_ok` (it IS the contiguous scope rule; a capability receipt is
   non-contiguous by design and reports `chain_linkage_ok` informationally).
7. `authenticated = structurally_valid ∧ (a pinned key was provided ∧ it matches the receipt's
   key — compared trimmed, ASCII case-insensitively)`.
8. Informational: `starts_at_genesis` (records[0] has `seq == 1` and an all-zero `prev_hash`) —
   the front-truncation signal for contiguous receipts; N/A to capability scope.

**Exit codes** (the `elastos verify-receipt` contract, scriptable): `0` AUTHENTIC (pinned +
matched + structurally valid) · `1` INVALID (any structural check failed — tampered/forged) ·
`3` VALID-BUT-UNAUTHENTICATED (structurally sound, no/mismatched pin — NOT a trust decision) ·
`4` COULD-NOT-EVALUATE (unreadable file / malformed JSON / bad pin argument — deliberately distinct
from 1 so "couldn't read it" is never mistaken for "forged").

## 7. The signed intent (agent → runtime)

What an agent signs to act under a mandate. The `schema` field's exact value is
`"elastos.intent.declaration.v1"` (dots, not slashes — it enters the signature preimage):

```json
{
  "schema": "elastos.intent.declaration.v1", "intent_id": "…", "capsule": "vm-<name>", "method_id": "runtime.pay",
  "input_hash": "…", "resource": "…", "action": "…", "standing_grant_id": "<mandate token>",
  "declared_at": {"unix_secs": u64, "monotonic_seq": u64}, "signer": "<64 hex>", "signature": "<base64>"
}
```

**Signature preimage** — ed25519 over `SHA-256` of:

```
  "elastos.intent.declaration.v1\0"                       // domain, WITH trailing NUL
‖ for each of [schema, intent_id, capsule, method_id, input_hash,
               resource, action, standing_grant_id, signer]:
      len(field) as 8-byte LITTLE-endian ‖ field bytes
‖ len(declared_at_json) as 8-byte little-endian ‖ declared_at_json
```

where `declared_at_json` is the timestamp's serde-JSON encoding
(`{"unix_secs":…,"monotonic_seq":…}` — its field order is therefore frozen:
`the_signature_preimage_timestamp_encoding_is_frozen` pins that segment, and the known-answer
ratchet `the_intent_preimage_digest_is_frozen` pins the WHOLE preimage — domain, field order, and
the little-endian prefixes — as one fixed digest). Note the
length prefixes here are LITTLE-endian, unlike §2/§4 — an implementation must not assume one
endianness across domains.

The runtime enforces, before executing: signature validity (strict); `signer` matching the
mandate's bound agent key WHEN the mandate bound one (an unbound legacy mandate is
capsule-string-scoped only); the mandate's scope/expiry/revocation; replay (the `intent_id`
inside a bounded time window); and per-mandate rate + spend budgets. Separately, the
signature-derived key `flint-<signature>` is the PAYMENT idempotency key
(SPEC-market-provider-v1 §2) — distinct from the replay key.

## 8. Versioning

- Serialized shapes in §2–§5 and §7 are FROZEN — verification re-serializes them. New fields must
  be optional AND absent-when-unset (`skip_serializing_if`) so historical bytes are reproduced.
- Any breaking change mints a new schema tag (`…/v2`); verifiers fail closed on unknown tags.
- The three domain strings (§2, §4, §7) are part of the format; they never change within v1.

## Version history

- **v1 (Sprint 49):** initial publication, extracted from the shipped implementation
  (`elastos-runtime::primitives::audit`, `capability::intent`) and pinned by the conformance
  ratchet.
