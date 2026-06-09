# dDRM chain — status & review package

**Branch:** `feat/decrypt-provider-cenc` (based on `origin/0.4.0`, **~67 commits**, tip Day-61 Phase C — the on-chain producer step now has a home: a new fail-closed `publish-provider` capsule ASSEMBLES the content mint — binding `contentId == bytes16 KID` (PC2 `kidToContentId`, no hash), deriving `tokenURI = {metadataCid}/metadata.json`, and emitting a typed *unsigned* `UnsignedMintV1` (`mint(string,uint16,bytes,bytes)`) for `chain-provider` to ABI-encode+broadcast and `wallet-provider` to sign — holding NO chain-RPC and NO wallet key itself (publish=13). Day 60 took the producer half cross-binary: `encrypt-provider` (feature `escrow`) `seal_inline` mints a CEK *now* + emits the SEALED escrow blob; `key-provider` (`release_from_escrow_ref`) recovers + re-seals it; `ddrm-producer-smoke.sh` drives `encrypt → key → decrypt` so a video sealed *now* decrypts *now*, no raw CEK/plaintext on any wire, no golden). **0.4.0 released — crypto core verified green on the released `v0.4.0`; rebase surface measured (see `PUSH_PLAN.md`). Anders confirmed the rail (Day 45); the decrypt boundary now implements his ENTIRE decrypt-side spec — Option A push-in (`rail-live`), full-transcript binding (`rail-bind`), in-sandbox key mint+publish (`rail-mint`), short-expiry + scoped CEK-free audit (`rail-audit`) — consolidated into the suite-tagged `SealedDecryptMaterialV1` drop-in (`rail-material`). Remaining work is upstream only (contract merge needs push; dKMS sealing needs Anders).**

> **📦 Day 49 — consolidated `SealedDecryptMaterialV1` (drop-in contract shape, LANDED).**
> The carrier is now a single backend-neutral, **suite-tagged** envelope — dKMS-native
> PQ-hybrid vs P-256/Lit compat is a FIELD, not a fork. The canonical op
> `OpenSessionV1` routes by `suite` into the audited/expiry-enforcing transcript-bound
> path; the compat suite is rejected on the product path and an unknown suite fails
> closed (`rail-material`=65). `DDRM_DECRYPT_RAIL.md` now carries the **verbatim
> additive `DecryptSessionRequestV1` delta** so Anders can lift it directly. This is
> the last clearly-ours decrypt-boundary task: the boundary is **complete**; what
> remains is upstream — fold the envelope into the shared `elastos-common` contract
> (needs push access) and the dKMS-direct sealing producer (needs Anders).

> **⛓️ Day 61 — `publish-provider`: the on-chain content mint, assembled fail-closed (Phase C, LANDED).**
> Days 58–60 built the producer's *crypto* half (mint→escrow→recover→re-seal→decrypt);
> Day 61 starts the producer's *on-chain* half — the step that registers content so the
> consumer chain (`has_access_by_content_id`) can answer for it. Audited PC2 first
> (`pc2-node/data/test-apps/elacity-creator/app.js`) and mirrored its REAL shapes: the
> on-chain `contentId` **is** the KID (`kidToContentId`, app.js:1568 — `0x` + 32 lowercase
> hex, **no hash, no truncation**; the legacy hash-derived id was deliberately removed),
> mint is `mint(string _uri, uint16 opType, bytes opRawData, bytes sellRawData)` on the
> creator's Channel (app.js:4948) with `_uri = {metadataCid}/metadata.json` (app.js:4946),
> and `opType ∈ {FREE=0, BUY_ONCE=1, BUY_AND_RESELL=2}`. New `publish-provider` capsule:
> `PreparePublish` validates a `PublishRequestV1`, binds **`content_id == bytes16 KID`**
> (closing producer→chain→consumer identity end to end), derives the tokenURI the PC2 way,
> and emits a typed **`UnsignedMintV1`** + a `PublishReceiptV1` whose status is `prepared`
> (never `published`) and which names the two providers that must finish the loop. It holds
> **no** chain-RPC and **no** wallet key — `opRawData`/`sellRawData` stay STRUCTURED so the
> EVM specialist (`chain-provider`) owns the ABI encoding and `wallet-provider` signs:
> the runtime's "core injects capabilities" pattern. Fail-closed: a non-`bytes16` KID, a
> paid listing with no price, a free listing carrying sale terms, or a bad channel address
> are all rejected; the receipt carries no signing/RPC authority (publish=13).
> **Gate:** ladder INTACT (+ publish rung 13, + wasm publish build), drift PASS, **both**
> smokes green (consumer + producer), clippy clean. **Next:** wire `chain-provider` to
> ABI-encode + broadcast the `UnsignedMintV1` (and a `content-market` index that scans the
> mint event), turning "prepared" into a real on-chain asset.

> **🎬 Day 60 — the producer half runs ACROSS REAL PROCESSES (Phase C, LANDED).**
> Day 59 proved the producer→authority→decrypt crypto spine on a fresh CEK in one test
> process; Day 60 takes it cross-binary so a human can SEE a video sealed *now* decrypt
> *now*. Three additive, feature-gated pieces (defaults byte-identical): (1) **Producer
> wire op** — `encrypt-provider` (feature `escrow`) mints an ML-DSA producer key at `init`
> and **publishes** `producer_verifying_key_b64`; a new `seal_inline` op mints a CEK
> in-boundary, CENC-encrypts inline plaintext into a single-sample fMP4 segment (the same
> box shape as the round-trip goldens), escrows the CEK to the authority's recipient key,
> zeroizes, and returns only `{kid_hex, content_id_hex, segment_b64, wrapped_cek_b64}` —
> never a raw CEK or the plaintext (encrypt escrow still 19 tests; the op is exercised by
> the smoke). (2) **Authority recover→re-seal on the wire** — `key-provider`'s
> `release_from_escrow_ref` takes the escrow blob + producer vk + KID + scheme (instead of
> a raw `cek_b64`), recovers the CEK via `recover_escrowed_cek`, then re-seals it to the
> decrypt session through the SAME shared sealing path as `release_ref`; a tampered/foreign
> escrow blob or a forged producer fails closed (key-authority-ref 26→27). (3)
> **`ddrm-producer-smoke.sh` + orchestrator** — drives the three REAL binaries
> `encrypt → key[recover+re-seal] → decrypt`, asserting the session opens on the
> freshly-sealed segment and that neither the escrow blob, any raw CEK, nor the plaintext
> is echoed on any wire. **Gate:** full ladder INTACT (key-authority-ref 27, +wasm escrow
> build), drift PASS, **both** smokes green (consumer + producer), no new warnings.
> **Still upstream-only after this:** real `plaintext_ref`→IPFS in the producer op and the
> dKMS-direct backend (needs Anders); next product rung is `publish-provider` (mint
> contentId=KID + tokenURI — the step that puts content on-chain toward the market).

> **🔐 Day 59 — the CEK-escrow ENGINE: producer→authority→decrypt on a FRESH CEK (Phase C, LANDED).**
> Day 58 pinned the escrow *seam* fail-closed; Day 59 fills it with real PQ-hybrid crypto
> and proves the whole producer→consumer key path without the committed golden. Three
> pieces, all additive/feature-gated, defaults byte-identical: (1) **Shared escrow AAD** —
> `ddrm-envelope::transcript::escrow_aad(scheme ‖ kid(bytes16) ‖ recipient_pub)`, one
> encoder both halves bind (same anti-drift discipline as the decrypt transcript;
> envelope lib 12→14). (2) **Authority recipient key** — the reference `key-provider`
> now mints a PQ-hybrid KEM keypair at `init` and **publishes** `seal_recipient_pub_b64`
> (distinct from its ML-DSA verifying key); `ReferenceAuthority::recover_escrowed_cek`
> opens a CEK escrowed to it, failing closed on a KID-swap or a forged producer
> (key-authority-ref 25→26). (3) **Producer escrow engine** — `encrypt-provider` (feature
> `escrow`) `seal_cek_to_authority` seals a freshly-minted CEK to that recipient via
> `ddrm-envelope`, raw CEK never in the blob (encrypt escrow=19). A single test walks the
> FULL spine: producer mints CEK → escrows to authority → authority recovers → **re-seals
> to a decrypt session** → decrypt opens the SAME CEK — the producer half meeting the
> already-built consumer half, fresh, no golden, no raw CEK across any boundary.
> **Gate:** full ladder INTACT (envelope 14, key-authority-ref 26, encrypt default 17 /
> escrow 19, +wasm escrow build), drift PASS, consumer smoke green, no new warnings.
> **Deliberately deferred to Day 60:** the *cross-binary* `ddrm-producer-smoke.sh` — it
> needs new wire ops (encrypt emits the escrow blob; key-provider recovers + re-seals),
> so it's its own clean commit rather than crammed here. The crypto path it will drive is
> already proven end to end this day.

> **🏭 Day 58 — producer half kickoff: identity join + fail-closed CEK escrow (Phase C, LANDED).**
> First Phase-C rung, and it's contract-first (no engine guesswork). Two things landed
> in `encrypt-provider`, both pinned by tests, default still fail-closed. (1) **Identity
> join, audit-grounded:** re-reading PC2 (`src/api/storage.ts`) confirmed the chain keys
> ownership on `hasAccessByContentId(address holder, bytes16 contentId)` — the content
> identity is the **KID** (16 bytes), NOT the IPFS CID (that's `payload_cid`, a separate
> field). `kid_to_content_id_bytes16` now proves the in-boundary-minted KID converts
> losslessly to that on-chain `bytes16 contentId`, and that the `SealedObjectV1` a
> producer emits carries exactly the KID the consumer chain (`chain content_id → rights
> binding → decrypt object_cid → transcript`) keys on — one identity end to end, so
> producer and consumer cannot drift. This folds the "bytes16 KID" carry-forward into the
> producer half. (2) **CEK escrow seam, fail-closed:** the producer must seal the CEK to a
> **key authority** before it can emit a SealedObject (invariant #1's hand-off half,
> mirroring PC2's host-mints / Lit-Action-wraps split but capability-scoped). With no
> authority recipient configured the escrow — and therefore `seal` — fails closed
> (`escrow_cek → NotConfigured`; status advertises `escrow: not_configured`); the producer
> refuses to mint a key it cannot safely hand off. The in-boundary keygen + CENC cipher
> were already proven (Days 19/31); this adds the contract around them. **Gate:** encrypt
> ladder 13→17, full ladder INTACT, drift PASS, consumer smoke green, no new warnings.
> **Next (Day 59):** the escrow ENGINE — key authority publishes a recipient key, the
> producer seals the CEK to it via `ddrm-envelope`, and a producer→consumer smoke runs
> `encrypt → SealedObjectV1 → key → decrypt` on a FRESH (non-golden) CEK.

> **🔗 Day 57 — the on-chain ownership answer is real & verifiable end to end (Phase B cont., LANDED).**
> Day 56 made `rights` consume a typed attestation; Day 57 makes that attestation
> trustworthy and wires the live wallet check. (1) **Characterized the chain-provider
> RPC boundary:** `has_access_by_content_id` now has golden tests that mock the EVM
> `eth_call` and prove it decodes the AuthorityGateway word into `has_access: true`
> (owned) **and** `has_access: false` (unowned), and **fails closed** (`upstream_invalid_bool`)
> on a malformed/non-boolean word — never silently coerced. (2) **Pinned the shape
> end to end:** a guard test proves `chain-provider`'s exact output keys deserialize
> 1:1 into `rights-provider`'s `ChainAccessAttestationV1` (rights `chain-rights`=18) —
> if chain-provider's output drifts, the guard fails (no shared-crate change needed, so
> the frozen contract surface + drift gate stay untouched). (3) **Opt-in live smoke:**
> with `DDRM_SMOKE_CHAIN_RPC` (+ contract/selector/subject/contentId) set, the consumer
> smoke builds and drives the **real `chain-provider`** against Base — your wallet vs the
> AuthorityGateway — and feeds the genuine answer into the rights decision; **offline
> (default) is unchanged**, deterministic mocked-owned, network-free. The smoke's content
> identity (`cid()`) now flows consistently through the chain query, the rights binding,
> and the decrypt transcript. **Gate:** smoke PASS (offline), full ladder INTACT, drift PASS,
> no new warnings. **What's still dev-shaped:** the runtime core that sequences
> `chain → rights → key → decrypt` is still the orchestrator; the producer half does not
> exist yet. **Next (Phase C):** the producer half — `encrypt → publish → IPFS → market`.

> **⛓️ Day 56 — real on-chain ownership gates the rights step (Phase B, LANDED).**
> The `rights` step is no longer a stub: behind a `chain-rights` dev profile,
> `rights-provider` consumes the typed answer of `chain-provider::has_access_by_content_id`
> (a `ChainAccessAttestationV1` injected by the runtime core — rights-provider holds NO
> chain-RPC capability, that authority stays in `chain-provider`), **binds it to the
> request** (content_id + right must match, else fail-closed), and renders a
> `RightsDecisionReceiptV1` (`allowed = has_access`). Owned → `allowed`; unowned →
> a real `denied` (key-provider then fails closed on it); a foreign/stale attestation
> or bad request → `invalid_request`. The clock is injected (`now_unix` + `ttl_secs`),
> never ambient. The op is isolated and additive: default build byte-identical
> (rights-provider=9), the new feature is the single new rung (`chain-rights`=17); a
> hidden `raw_chain_rpc` field is rejected (`deny_unknown_fields`). The **consumer smoke
> now drives the REAL rights decision** (mocked-owned attestation, no live RPC) and uses
> its emitted receipt to gate the key release — so it proves `rights(allowed) → key →
> decrypt` end to end. **Gate:** smoke PASS, full ladder INTACT, drift PASS, no new warnings.
> **What's still dev-shaped:** the on-chain answer is mocked in the smoke (a funded
> wallet + live Base RPC through `chain-provider` is the next rung); the runtime core
> that sequences `chain → rights → key` is still stood in for by the orchestrator.
> **Next (Phase B cont. / C):** drive `chain-provider` against live Base for a real
> token-ownership check; then begin the producer half (encrypt → publish → IPFS → market).

> **▶️ Day 55 — consumer-half orchestration smoke: the chain RUNS end to end (Phase A.4, LANDED).**
> The first point a human can drive the consumer half and SEE it work. A new
> `scripts/ddrm-consumer-smoke.sh` + standalone dev orchestrator
> (`scripts/dev/ddrm-consumer-smoke`, never shipped) builds the **real** capsule
> binaries and drives them over their stdin/stdout JSON protocol:
> `drm/open → rights → key (reference authority) → decrypt (OpenSessionV1)`.
> The previously-unproven cross-process **key→decrypt handoff** now executes for real:
> (1) the authority publishes its ML-DSA-65 verifying key at `key init`; (2) the
> decrypt boundary trusts it, then MINTS + PUBLISHES an in-sandbox session key
> (`decrypt init`, secret never leaves); (3) the authority seals the golden CEK to that
> published key, **transcript-bound via the shared `ddrm-envelope` encoder**
> (`key release_ref`); (4) the boundary unwraps in-VM and decrypts a real CENC segment
> (`decrypt open_session_v1`), returning ONLY a scoped session (`is_protected`,
> `sample_count`) — **no CEK, no plaintext crosses any process boundary**, asserted on
> both wires. A transcript-mismatched seal (flipped nonce) **fails closed**. To unblock
> the bootstrap ordering, the reference `key init` now **publishes its verifying key**
> (`key-authority-ref`=25), and `release_receipt_hash` was lifted into the shared
> `ddrm-envelope::transcript` so the authority and the boundary derive the IDENTICAL
> receipt binding (`ddrm-envelope` lib=12; `decrypt-provider` byte-identical — rail-bind=60,
> rail-material=65). **Gate:** smoke PASS, full ladder INTACT, drift PASS, no new warnings.
> **What this is NOT yet:** the orchestrator stands in for the runtime core (it holds no
> keys — it only sequences requests and computes the public transcript); the CEK is handed
> to the dev reference backend directly (production recovers it from a dKMS-wrapped envelope);
> `rights`/`drm` are driven as reachable steps, not yet real Base validation (Phase B).
> **Next (Phase B):** point `rights-provider` at `chain-provider::has_access_by_content_id`
> so the `rights` step is a real on-chain ownership check with the wallet.

> **🧬 Day 54 — shared decrypt-transcript `to_aad` (Phase A.4, LANDED).**
> The transcript binding is now a **single encoder** in `ddrm-envelope::transcript`
> (`DecryptTranscriptV1` + `to_aad`, domain-labelled, length-prefixed). This is the
> same anti-drift move the crypto dedup made, applied to the AAD: the **key authority**
> computes `to_aad()` and seals the CEK to it, and the **decrypt boundary** rebuilds the
> identical transcript from the authenticated request and unwraps against it — neither
> side owns a private copy of the field set/encoding, so a SEPARATE capsule (key-provider,
> a dKMS, a Lit-compat backend) can now produce material the decrypt boundary opens.
> `decrypt-provider` re-uses the shared struct under the historical `rail-bind` path
> (byte-identical: rail-bind=60, rail-material=65, all goldens replay). `key-provider`'s
> reference backend gains an orchestration proof (`key-authority-ref`=24): it builds the
> CANONICAL shared transcript, seals to its `to_aad()`, and the decrypt-side
> `hybrid_unwrap_bound` opens under the matching transcript and **fails closed** on any
> field change (replayed nonce). `ddrm-envelope` itself grows transcript coverage
> (lib=10): determinism, total field sensitivity, and a bound seal/unwrap round-trip.
> **Gate:** full ladder INTACT (decrypt counts unchanged; `ddrm-envelope`=10,
> `key-authority-ref`=24 pinned), wasm clean, drift PASS, no new warnings.
> **Next (Phase A.4 cont.):** a cross-binary dev-profile orchestration smoke that runs
> `drm/open → rights → key (reference) → decrypt (OpenSessionV1)` across the REAL capsule
> entrypoints — minting the session in decrypt, sealing in the reference authority to the
> shared transcript, and decrypting a segment — so a human can finally *see the consumer
> half run end to end* with no Lit, no dKMS, no chain.

> **♻️ Day 53 — dedup COMPLETE: `decrypt-provider` re-exports `ddrm-envelope` (Phase A.3b, LANDED).**
> The PQ-hybrid crypto now lives in **exactly one place**. `decrypt-provider::pq_envelope`
> deleted its in-tree copy (seal/unwrap/wire/verifiers/KDF, ~370 lines) and re-exports the
> shared crate under the historical `crate::pq_envelope::*` paths, so dispatch, the rail
> shim, the golden vectors and every test suite are **byte-for-byte unchanged**. The CENC
> glue (`decrypt_pq_sealed_segment*`, which calls `crate::decrypt_session_segment`) and the
> test-only `seal_support` stubs stay local; the seal engine itself is re-exported. To
> enable this the shared crate widened its surface (`pub signed_payload`, re-exported raw
> `Ciphertext`/`MlKem768`/`MlKemDk`/`MlKemEk`/`XStaticSecret`). The now-redundant
> `x25519-dalek` + `aes-gcm` deps were **pruned** from `decrypt-provider` (they live solely
> in `ddrm-envelope`); `ml-kem` + `sha2` remain only for the test-side golden helpers + stub
> signer. **Gate (pure refactor, zero behaviour change):** all **22** ladder combos keep
> their EXACT counts, the committed goldens replay byte-identically (vectors=42, rail-shim=45,
> harden=65 unchanged), wasm clean, drift PASS, no new warnings. The Day-52 equivalence
> guard (`envelope-conformance`=35) still passes — now confirming the dedup stays coherent.
> **Next (Phase A.4):** a shared decrypt-transcript `to_aad` (so the key authority and the
> decrypt boundary agree on the binding) + the dev orchestration smoke
> `drm/open → rights → key (reference) → decrypt`, proving the consumer half runs end to end
> without Lit/dKMS.

> **🔗 Day 52 — cross-capsule equivalence guard: `ddrm-envelope` ⇄ `decrypt-provider` (Phase A.3, LANDED).**
> The shared crate (what the key authority seals with) is now **provably wire- AND
> crypto-interoperable** with `decrypt-provider`'s own in-tree PQ-hybrid unwrap — in the
> real key→decrypt direction. A new guard (feature `envelope-conformance`, dev-dep on
> `ddrm-envelope`) has the provider mint+publish a decrypt-session key, the **shared
> crate seal** a CEK to it (transcript-bound, real ML-DSA-65), and this provider's OWN
> `PqSealedEnvelope::from_bytes` + `hybrid_unwrap_bound` recover the exact CEK; a
> mismatched transcript fails closed and no raw CEK appears on the wire. This pins the
> only thing the temporary duplication risks — **silent drift** — so the two
> implementations cannot diverge while the full dedup is pending. Additive and
> reversible: `pq-mldsa` stays exactly 34; the guard is the single new combo
> (`envelope-conformance`=35); the shipped capsule never links the shared crate (dev-dep).
> Ladder pins `envelope-conformance`=35; all other counts + wasm builds unchanged; drift
> guard PASS.
> **Why a guard and not the full rip-out yet:** the decrypt-provider PQ test suite is
> tightly bound to the *concrete* crypto types (it constructs `PqSealedEnvelope` literals,
> touches raw `ml-kem`/`x25519` types, calls the private `signed_payload`), so a faithful
> in-place migration is a broader API+visibility refactor than is wise to land in one
> increment on the crown-jewel capsule. The guard captures the value (no drift) now and
> **de-risks** that migration: it is the proof the rip-out must keep passing.
> **Next (Phase A.3b / A.4):** complete the dedup behind this guard (re-export the shared
> impl + widen `ddrm-envelope`'s surface: `pub signed_payload`, raw-type re-exports), then
> wire the dev orchestration smoke (`drm/open → rights → key (reference) → decrypt`).

> **🔑 Day 51 — reference key-authority seal engine + shared `ddrm-envelope` crate (Phase A.2, LANDED).**
> The first backend that produces real sealed material. New shared crate
> **`capsules/ddrm-envelope`** is the single source of truth for the PQ-hybrid
> seal/unwrap + wire format + ML-DSA-65 signer/verifier (extracted byte-identical from
> the proven `decrypt-provider::pq_envelope` island; the seal is **promoted to
> production** since the key authority needs it). `key-provider`'s `reference` backend
> (feature `key-authority-ref`) seals a recovered CEK to a decrypt session's published
> key via this crate and emits the exact suite-tagged `SealedDecryptMaterialV1` the
> decrypt boundary opens — exposed through a capsule-local `release_ref` op so the
> shared `KeyReleaseRequestV1` stays byte-identical (Parallel Change). **Cross-boundary
> proof:** a test seals with the reference authority and opens with the SAME
> `ddrm_envelope::hybrid_unwrap_bound` the decrypt boundary uses — the key→decrypt
> handoff is wire-compatible end to end, transcript-bound, with no raw CEK on the wire.
> 23 key-provider tests under the feature (18 default + 5 reference: round-trip,
> transcript binding, malformed-pubkey fail-closed, backend-required, validation-first);
> 7 in `ddrm-envelope`. Default build stays fail-closed; decrypt-provider untouched
> (its 10-combo ladder unchanged). Ladder pins `ddrm-envelope`=7 + `key-authority-ref`=23
> + both wasm builds. Mirrors PC2 `envelopeCEK` (`universal-decrypt-chipotle.js`).
> **Next (Phase A.3):** migrate `decrypt-provider` onto `ddrm-envelope` (pure refactor,
> gated by the committed goldens) to delete the duplication and yield the literal
> cross-capsule golden; then wire the orchestration (`drm/open → rights → key → decrypt`).

> **🔌 Day 50 — `key-provider` is a pluggable multi-backend authority (Phase A.1, LANDED).**
> Confirmed Anders' model in code: `key-provider` is the *authority boundary*, hosting
> interchangeable **key-delivery backends** — `reference` (native dev, PQ-hybrid suite),
> `dkms` (native production, PQ-hybrid), `lit` (PC2/Chipotle compat, classical suite) —
> all destined to emit the same suite-tagged `SealedDecryptMaterialV1` the decrypt
> sandbox already consumes. Backend selection is **operator/runtime config at `init`**
> (never an app input), so the shared `KeyReleaseRequestV1` stays byte-identical.
> `status` now advertises `supported_backends` (suite/kind/state) + `active_backend`;
> `release` runs **all existing validation first**, then routes to the active backend,
> each returning a precise backend-specific `not_configured` (the in-runtime `reference`
> seal engine is Phase A.2). Default (no backend) stays fail-closed. Pinned by 18
> characterization tests (was 9): routing, unknown/non-string backend rejection, and
> the property that **validation precedes backend routing** (a denied receipt never
> reaches a backend). Mirrors the PC2 Lit authority role (`chipotle-client.ts`
> `recoverCEKEnvelope`/`envelopeCEK`, `universal-decrypt-chipotle.js`).

> **🗺️ WHOLE-SYSTEM MAP (Day 49).** For the full PC2 journey (creator → publish →
> market → purchase → download → validate → key → decrypt → playback) mapped against
> the runtime, current/target architecture diagrams, the PC2→runtime pattern-migration
> table, and the phased road to a testable end-to-end, see
> **`SYSTEM_ARCHITECTURE_MAP.md`**. Net: the decrypt boundary is done and the
> infrastructure (IPFS/chain/wallet/content) exists; the missing middle is a **key
> authority** + **orchestration wiring** + **producer/market/viewer**. Fastest testable
> unblock = **Phase A** (runtime-native key authority feeding `OpenSessionV1`).

> **🧾 Day 48 — short-expiry enforcement + scoped audit (Anders' "short expiry, audit", LANDED).**
> `rail-audit`=62: new `OpenSessionAudited` op takes an injected capability clock
> (`now_unix`, never ambient), REJECTS a stale grant (`now_unix` past the request or
> release-receipt expiry) **before any unwrap** (fail-closed `expired`), and emits a
> scoped, tamper-evident **audit record bound to the transcript hash** on every
> decision (`opened`|`denied`) carrying **no CEK and no plaintext**. Proven: fresh
> grant opens + audits `opened`; expired grant fails closed + audits `denied`/
> `expired` with no session and no unwrap attempted. The shared bound-open logic was
> refactored into `prepare_bound_open` with `rail-bind`/`rail-mint` counts unchanged
> (no regression). Default + every golden unchanged; drift PASS. **With this the
> decrypt boundary implements all four of Anders' decrypt-side requirements** (push-in,
> transcript binding, in-sandbox key, expiry+audit). Remaining is upstream only: fold
> `sealed_decrypt_material` into the shared contract (needs push) + dKMS-direct sealing.

> **🔑 Day 47 — in-sandbox session-key mint + publish (Anders' Day-45 ask, LANDED).**
> Anders required the decrypt-provider to *"create a per-session one-time public key
> inside its sandbox."* Done (`rail-mint`=62): `init` mints the per-session hybrid
> KEM keypair (`pq_envelope::mint_session`, OsRng→WASI `random_get`, wasm-clean),
> keeps the secret in-VM, and publishes the pubkey + suite. The faithful flow is
> proven with **no injected secret**: sandbox mints + publishes → key authority
> seals the CEK to the published key (transcript-bound) → the minted secret opens it
> with no CEK/plaintext leak; a fresh key is minted per init. Minting is the only
> entropy the boundary needs; the unwrap path stays RNG-free (its own feature).
> Default build + every committed golden unchanged; drift PASS. The decrypt boundary
> now implements **all three** of Anders' decrypt-side requirements (push-in Option A,
> transcript binding, in-sandbox key). Remaining is upstream only: fold
> `sealed_decrypt_material` into the shared contract (needs push access) + dKMS-direct
> sealing (or audited key-provider re-seal). See `DDRM_DECRYPT_RAIL.md`.

> **🔒 Day 46 — sealed material binds the full transcript (Anders' Day-45 ask, LANDED).**
> Anders confirmed the architecture (hybrid, ElastOS-native, Option A push-in, chain
> `drm→rights→key/dKMS→decrypt`, in-sandbox session key, providers stay separate,
> PQ-hybrid root) and added one hard requirement: the sealed material must bind the
> **full decrypt transcript** with AEAD/AAD + signature + replay nonce. Done on the
> PQ-hybrid profile (`rail-bind`=60): a capsule-local `DecryptTranscriptV1` (principal,
> session, object CID + content hash, action, viewer interface, output kind, expiry,
> release-receipt hash, decrypt-session pubkey, suite, provider, nonce) is the
> AES-256-GCM **AAD** and is covered by the **ML-DSA-65 signature** (`hybrid_unwrap_bound`
> / `seal_bound`). `OpenSessionBound` rebuilds the transcript from the authenticated
> request + the boundary's own session pubkey (never the carrier) → a CEK sealed for
> one transcript **cannot be replayed** against another: a different `session_id`, a
> swapped nonce, and a tampered carrier all **fail closed**. `aad==b""` reproduces the
> legacy envelope byte-for-byte, so every committed golden + the `rail-shim-mldsa`/
> `harden` rungs are unchanged; default build still byte-identical + fail-closed.
> Remaining (upstream/needs Anders, not our boundary): fold `sealed_decrypt_material`
> into the shared contract, in-sandbox key mint+publish, dKMS-direct sealing. See
> `DDRM_DECRYPT_RAIL.md` §Transcript binding.

> **🔌 Day 45 — recommended rail WIRED (reference).** The recommended split
> (Option A at the decrypt boundary: the VM *receives* sealed material) is no
> longer just a tested island — it is wired into the provider dispatch behind the
> `rail-live` feature. A new `OpenSessionLive` op runs the proven
> `decrypt_from_carrier` in-boundary and returns a **scoped** response; a real
> ML-DSA-65-signed PQ-hybrid carrier decrypts through the **actual dispatch** with
> **no CEK/plaintext leak**, while a tampered carrier and an unprovisioned boundary
> both **fail closed** (`rail-live`: 57 passed, wasm-clean). Crucially the shared
> contract is **untouched** (VM-sealed material rides a capsule-local variant), so
> drift stays green and the default build is byte-identical + fully fail-closed.
> The exact additive `DecryptSessionRequestV1` delta for when Anders blesses Option
> A is written out in `DDRM_DECRYPT_RAIL.md` (§Reference rail LANDED). Net: the only
> remaining step to default-on live decrypt is Anders' thumbs-up on the contract
> field — the code path is already proven end-to-end.
**State:** the full Elacity dDRM provider chain is **fail-closed**, **compiles to
`wasm32-wasip1`**, **executes under WASI**, and has **verified inter-provider
contract handoffs**. Both chain ends are now pinned by tests: the **upstream rail
contract** (ECDH CEK-sealing envelope, `decrypt-provider/src/envelope.rs`) and the
**downstream consumer contract** (both players receive scoped output, never the
CEK). A full team-facing **security + threat model** is in
`DDRM_SECURITY_MODEL.md`. The only thing between here and live decrypt is one
architecture decision (the CEK transport rail) — see `DDRM_DECRYPT_RAIL.md`.

> **✅ 0.4.0 RELEASED — alignment verified (Day 44).** 0.4.0 shipped (tag `v0.4.0`
> = `cae83c3c3`). The contract-first bet paid off: `protected_content.rs` is
> **byte-identical** between this branch and the released `v0.4.0`, and
> `ddrm-drift-check.sh` **passes against the released base**. The crypto core was
> validated green ON `v0.4.0` (content-overlay in a throwaway worktree): drift PASS,
> `decrypt-provider` harden=65 + pq-mldsa-hybrid=37, `encrypt-provider`=13,
> `pc2-conformance` byte-compatible. Released v0.4.0 ships the providers as
> **fail-closed skeletons** (no CEK rail) — the rail decision is still the one
> blocker. Rebase conflict surface is now MEASURED (see `PUSH_PLAN.md`): clean for
> `decrypt-provider` (engine replaces skeleton) + `encrypt-provider` (new); genuine
> **3-way for `key-provider` + `drm-provider`** (we and Anders both evolved them —
> needs his intent). `encrypt-provider`'s sealed output already uses shared
> `SealedObjectV1` (Day 39); only its input `SealRequest` stays local.

## The chain

```
app/viewer --drm/open--> drm-provider --sequences--> rights -> key -> decrypt --scoped output--> app
                                          RightsReceipt -^   ReleaseReceipt -^ (wrapped CEK only)
```

## Parity table (proven bar)

| Provider | Role | Fail-closed | Host tests | wasm32-wasip1 | WASI smoke |
| --- | --- | --- | --- | --- | --- |
| `encrypt-provider` | seal/produce (invariant #1) | yes | 13 | builds | — |
| `drm-provider` | orchestrator (`drm/open`) + chain-seam | yes | 12 | builds | 4/4 |
| `rights-provider` | rights decision | yes | 9 | builds | 4/4 |
| `key-provider` | key release (rights-bound) | yes | 9 | builds | 4/4 |
| `decrypt-provider` | decrypt/render (cenc + envelope + consumer contract) | yes | 25 (+2 `rail-prep`) | builds | 4/4 |

The chain now has **both ends present**: `encrypt-provider` is the producer
(invariant #1) and `decrypt-provider` the consumer (invariant #2). **The encrypt
side's in-boundary keygen gap is CLOSED (Day 19):** the CEK+KID are now minted with
a CSPRNG inside the wasm boundary (`getrandom` → WASI `random_get`) and consumed by
a vendored CENC AES-128-CTR cipher (PC2 `cenc-encrypt` @ `a0a910158`), with the CEK
held in `Zeroizing` and an output type (`SealedSegment`) that has no CEK field. The
once-`#[ignore]`d `cek_and_kid_generated_inside_boundary` now passes. Only the full
`seal` (PQ-envelope CEK escrow + fMP4 packaging + ciphertext availability) remains,
behind a fail-closed `seal` — it shares the decrypt side's rail dependency. See
`DDRM_ENCRYPT_INVARIANT.md`.

## Security properties proven

- **Zero ambient authority surfaced.** Every provider's `status` advertises the
  raw authority it blocks (`raw_cek`, `chain_rpc`, `wallet_rpc`, `key_backend_sdk`,
  `kubo_api`, `elacity_sdk`, …) and wire-rejects hidden authority fields
  (`deny_unknown_fields`).
- **Fail-closed by default.** Every operation returns `not_configured` after full
  validation until its real backend exists. Invalid/mis-bound input returns
  `invalid_request`. Nothing opens by accident.
- **CEK containment.** The CEK only ever appears `wrapped` (key step) or
  contained + zeroized inside the cenc engine (decrypt step). The decrypt-step core
  seam is tested to leak neither the CEK nor plaintext to the caller.
- **Authorization binding.** `key-provider` verifies the upstream
  `RightsDecisionReceiptV1` (allowed + principal/session/object/right must match)
  before any release.
- **Contracts compose.** `drm-provider::chain_seam_tests` prove a
  `RightsDecisionReceiptV1` deserializes into the key request and a
  `ReleaseReceiptV1` into the decrypt request — shared-type drift fails loudly.
- **Upstream rail contract pinned (executable spec).** The CEK-sealing envelope
  (vendored from PC2 `ddrm-decrypt`: P-256 ECDH unwrap → AES-256-CBC) is captured
  as `decrypt-provider/src/envelope.rs` with characterization tests: v2/v3
  round-trip, fail-closed parsing, `Zeroizing` on recovered material, and a
  `sealed_envelope_does_not_contain_raw_cek` containment check. This is the
  concrete shape of the rail's "Option A" decrypt boundary.
- **Downstream consumer contract pinned (both players).** Tests in
  `decrypt-provider` prove the scoped, player-facing response carries **metadata
  only** for both viewer capsules — media (fMP4 segments via opaque handle) and
  non-media (render-only plaintext via opaque session id) — and that a real
  decrypted media segment never lets the CEK/IV/plaintext reach the player
  boundary (`media_segment_decrypt_keeps_cek_and_plaintext_off_the_player_boundary`).
- **Rail-landing composition prepped (Parallel Change, feature `rail-prep`).** The
  two previously-separate tested islands — the upstream envelope unwrap
  (`envelope::{parse, ecdh_unwrap, extract_cek}`) and the decrypt-step core
  (`decrypt_session_segment`) — are now joined by `decrypt_sealed_segment`, the
  single in-boundary operation the Hybrid rail will invoke once Anders confirms the
  CEK transport. It mirrors PC2 `ddrm-decrypt::session::unwrap_envelope` → cenc
  decrypt: the CEK materializes only after a correct ECDH unwrap, is held in
  `Zeroizing`, is consumed + zeroized by the cenc engine, and never reaches the
  scoped response. Pinned by characterization tests
  (`sealed_segment_decrypts_end_to_end_and_keeps_cek_off_the_boundary`,
  `sealed_segment_fails_closed_on_wrong_session_key`) and proven to build to
  `wasm32-wasip1`. **The flag is OFF by default — the live dispatch and the 25-test
  default suite are unchanged** — so the live wiring is a one-step swap into
  `open_session`/`render` once the rail + session-key provisioning land.

## How to run it yourself

```bash
# one-time prerequisites
rustup target add wasm32-wasip1
brew install wasmtime

# whole chain, one command:
scripts/ddrm-chain-smoke.sh

# per-provider host tests:
( cd capsules/drm-provider     && cargo test )
( cd capsules/rights-provider  && cargo test )
( cd capsules/key-provider     && cargo test )
( cd capsules/decrypt-provider && cargo test )
```

## PQ-hybrid-in-wasm viability (de-risked, Day 15)

The runtime profile requires PQ-hybrid crypto for the inter-stage CEK seal
(`x25519 + ml-kem-768` KEM, `ml-dsa-65` signature). Before committing the rail to
that profile, we proved the PQ halves actually build inside the wasm boundary:

| Crate | Algorithm | Resolved version | `wasm32-wasip1` |
| --- | --- | --- | --- |
| `ml-kem` (RustCrypto) | ML-KEM-768 (FIPS 203) | 0.2.3 | **builds clean** |
| `ml-dsa` (RustCrypto) | ML-DSA-65 (FIPS 204) | 0.0.4 | **builds clean** |

Proof: a throwaway crate depending on both, built with `cargo build --target
wasm32-wasip1` under the pinned `1.89.0` toolchain — green. Their transitive deps
(`sha3 0.10.9`, `keccak 0.1.6`, `kem`, `signature`, `zeroize`) are all wasm-clean.
The classical halves (`x25519-dalek`, `aes-gcm`) are already wasm-proven in tree.

**Go/no-go:** GO on PQ-in-wasm. One caveat to flag at rail-design time: `ml-dsa`
is still `0.0.x` (early, pre-1.0 API churn likely); `ml-kem` is more settled at
`0.2.x`. Recommend pinning exact versions and keeping the signature scheme behind
the envelope abstraction so a hybrid (ECDSA + ml-dsa) transition stays cheap.

### PQ-hybrid envelope de-risked end-to-end (Day 20)

Beyond "the crates compile", the **seal/unwrap shape now composes and recovers a
CEK** — `decrypt-provider/src/pq_envelope.rs`, the PQ analogue of the classical
`envelope.rs`, behind the `pq-envelope` feature (default OFF, Parallel Change):

- **Hybrid KEM:** `x25519` DH ‖ `ML-KEM-768`; the AES-256-GCM wrap key is derived
  (SHA-256 KDF, labelled + length-prefixed) from **both** shared secrets, so
  confidentiality holds if **either** primitive stays unbroken.
- **AEAD wrap:** authenticated — a wrong KEM secret or tampered blob fails closed
  (`UnsealFailed`), no plaintext on error.
- **Signature behind `CekSealVerifier`** so ml-dsa-65 (or hybrid ECDSA+ml-dsa)
  plugs in without touching the unwrap path (honours the caveat above).
- **CEK returned in `Zeroizing`**; the raw CEK never appears in the sealed bytes.
- **Unwrap needs no RNG and no outbound authority** — a pure in-VM transform, like
  the classical path.

Pinned by 4 characterization tests (`pq_hybrid_round_trip_recovers_cek`,
`wrong_session_secret_fails_closed`, `tampered_signature_fails_closed`,
`sealed_envelope_has_no_raw_cek`) and **proven to build to `wasm32-wasip1`** under
`1.89.0` with the feature on. Resolved versions: `ml-kem 0.2.3`, `x25519-dalek 2`,
`aes-gcm 0.10`, `sha2 0.10`. Run: `cargo test --features pq-envelope` (29 green:
25 default + 4 PQ). **The PQ rail is now a known-good drop-in for the classical
envelope the moment Anders confirms the transport + signature scheme.**

### Full PQ data path proven end-to-end, pre-rail (Day 21)

The three in-boundary engines — Day-18 rail-prep composition, Day-19 in-boundary
keygen, Day-20 PQ envelope — are now bound into **one executable cross-engine
proof**: `pq_envelope::decrypt_pq_sealed_segment` (feature `pq-rail-prep`, default
OFF, enables `pq-envelope`) chains `hybrid_unwrap → decrypt_session_segment`, i.e.
the PQ analogue of the Day-18 classical `decrypt_sealed_segment`. The PQ unwrap
slots exactly where the classical `ecdh_unwrap` does (mirroring PC2
`ddrm-decrypt::session::unwrap_envelope` → cenc), with the CEK in `Zeroizing`
throughout, consumed + zeroized by the cenc engine, and never reaching the scoped
response.

Pinned by a **cross-engine golden**: PQ-seal a CEK and CENC-encrypt a segment with
that *same* CEK, then prove the composed path recovers the plaintext while the CEK
stays off the boundary (`pq_sealed_segment_decrypts_end_to_end_and_keeps_cek_off_the_boundary`),
plus a wrong-session fail-closed case. Builds clean to `wasm32-wasip1`. Run:
`cargo test --features pq-rail-prep` (31 green: 29 + 2 cross-engine). **The entire
PQ dDRM data path — sealed CEK in → rendered bytes out, key contained — is now
proven before the rail lands; the remaining work is the transport shim, not the
crypto or the engines.**

### Engines pinned by portable golden vectors (Day 22)

Both decrypt data paths are now locked by **substrate-independent golden vectors**
(Feathers' characterization/golden-file pattern) committed under
`capsules/decrypt-provider/tests/vectors/` — fixed input bytes → expected output,
captured once and replayed through the engines with **no in-test sealing and no
RNG** (every consumer step — ECDH/x25519 DH, ML-KEM decapsulate, AES open, CENC
decrypt — is deterministic given the captured material):

- **`classical_cenc.json`** — P-256 ECDH envelope (v3) → CENC AES-128-CTR. This
  vector is **byte-compatible with PC2 `ddrm-decrypt`** (same envelope + cenc wire
  shapes), so it doubles as a cross-implementation conformance fixture that can be
  replayed against the reference implementation.
- **`pq_hybrid_cenc.json`** — x25519+ML-KEM-768 hybrid seal → CENC AES-128-CTR
  (`elastos-pq-hybrid-threshold-v0`). Runtime-specific (PC2 has no PQ), so the
  vector pins it across refactor/rebase/port. Replaying it also reconstructs the
  **typed `PqSealedEnvelope` from flat bytes** (ML-KEM dk + ciphertext
  (de)serialization) — exercising the exact wire-decode the live rail will need.

Each vector has a **replay** test (recover CEK → decrypt → assert plaintext) and a
**corrupted-input fail-closed** test. The schema lives in `src/vector_format.rs`.
Feature split keeps the surface clean: `vectors` (default OFF, enables
`pq-rail-prep`) compiles + runs the four replay tests against the committed
fixtures; `gen-vectors` regenerates the fixtures (`cargo test --features
gen-vectors emit_`). The four base suites are **unchanged** (default 25, `rail-prep`
27, `pq-envelope` 29, `pq-rail-prep` 31); `cargo test --features vectors` = **35
green** (31 + 4 golden). Builds clean to `wasm32-wasip1`. **The engines are now
refactor-/rebase-/port-safe and the classical path is conformance-checkable against
PC2 — independent of any in-test seal helper.**

### PC2 cross-impl conformance is now executable (Day 23)

The "byte-compatible with PC2 `ddrm-decrypt`" claim is no longer an assertion — it
**runs**. `scripts/pc2-conformance.sh` decrypts the committed `classical_cenc.json`
using PC2 `ddrm-decrypt`'s **real code** and asserts byte-for-byte parity end to
end:

1. **CEK transport** — PC2 `envelope::parse → ecdh_unwrap → extract_keys_blob`
   recovers the **same 16-byte CEK** from our sealed envelope.
2. **Media** — PC2 `mp4box::parse_segment → cenc::decrypt_samples` decrypts our
   segment to the **same plaintext**.

The harness compiles a small driver (`scripts/pc2-conformance/driver.rs`) against
the PC2 repo on demand via a temp crate, so **no absolute path or PC2 coupling ever
enters the ElastOS build graph**. It resolves PC2 via `PC2_REPO` (default
`/Users/sash/Documents/Cursor/pc2.net/pc2-node`) and **skips clean (exit 0)** when
PC2 is absent, so the default chain is never broken; it **fails (exit 1) only on a
genuine divergence**. Current result against the live PC2 checkout: **PASS** (CEK
and plaintext both match). **Two independent implementations now agree on the exact
bytes of the classical CEK rail — the strongest convergence evidence short of a
shared test crate, and a regression tripwire if either wire format drifts.**

### Conformance promoted to a standing gate + widened (Day 24)

The cross-impl check is now part of the standard pre-rebase/pre-PR gate and covers
more of the contract:

- **`scripts/ddrm-verify.sh`** — one button-press aggregator that runs (1) the
  contract drift check and (2) the PC2 cross-impl conformance. Exits non-zero if
  either gate fails; the conformance step **skips clean** when PC2 is absent, so
  the gate is safe to run anywhere. This is now the recommended first check before
  any rebase onto a moving 0.4.0.
- **Two envelope versions** are cross-checked: `classical_cenc.json` (**v3**,
  random IV) and `classical_cenc_v2.json` (**v2**, IV derived from the ephemeral
  pubkey) — both PC2-supported wire shapes. Each is replayed in-repo
  (`--features vectors` = **36 green**, +1 for the v2 replay).
- **Negative parity:** for every vector the harness also tampers the envelope and
  asserts **PC2 fails closed too** (`tamper: ... rejected ... fail-closed parity
  OK`) — proving both implementations reject the same corruption rather than
  silently leaking plaintext.

Current result against the live PC2 checkout: **PASS** for v3 + v2, positive and
negative. Base suites unchanged (25/27/29/31); chain 68; drift PASS. **The rail
contract is now guarded on both the happy path and the fail-closed path, across
both envelope versions, by code that runs the reference implementation.**

### Encrypt→decrypt round-trip golden — both invariants pinned on one artifact (Day 26)

The two ends were proven separately (invariant #1: `encrypt-provider` mints CEK+KID
in-boundary and CENC-encrypts; invariant #2: `decrypt-provider` unwraps + cenc-
decrypts). They are now proven to **compose** on a single artifact:

- `encrypt-provider` (feature `gen-vectors`) runs its **real in-boundary engine**
  (`mint_cek_and_kid` → `cenc::encrypt_samples` → mux) and writes
  `roundtrip_encrypt_to_decrypt.json` into `decrypt-provider/tests/vectors/`.
- `decrypt-provider` (feature `vectors`) replays it
  (`encrypt_to_decrypt_round_trip_golden`) and asserts it **recovers the producer's
  exact plaintext**, with the CEK leaking onto neither the producer's output type
  (`SealedSegment` has no CEK field — compile-time) nor the consumer's scoped
  response.

`cargo test --features vectors` (decrypt) = **37 green** (+1 round-trip); the base
ladder is unchanged (25/27/29/31) and `encrypt-provider` stays **13** (emit gated
off by default). Both build clean to `wasm32-wasip1`.

**Recorded gap (the rail, unchanged):** the CEK is captured into the fixture as a
stand-in for the still-blocked transport rail — in production it reaches decrypt
**sealed**, never in the clear. So this golden pins the **cipher + keygen
composition** (an asset sealed here decrypts there); the **seal/envelope transport**
is exactly what lands when Anders confirms the rail. The byte-identical cipher cores
(both `apply_keystream` AES-128-CTR with `pad_iv`) make that composition sound.

### Rail transport shim — the rail is now a flag flip, not a design (Day 27)

Everything *downstream* of the rail was proven (unwrap + cenc, both classical and
PQ). The missing piece was the **carrier→engine adapter**: the thin code that takes
the sealed-CEK material off the wire and hands it to the right engine. That adapter
now exists behind the `rail-shim` feature (`decrypt-provider/src/rail_shim.rs`,
default OFF, **NOT** wired into `OpenSession`/dispatch — a Parallel-Change island):

- `SealedDecryptCarrier { profile, sealed_cek, ciphertext_segment, init_segment }` —
  carries only sealed/public bytes (**never** a raw CEK), mirroring rail Option A
  (decrypt VM *receives* VM-sealed material) and PC2 `session::unwrap_envelope`
  (the VM holds the session key; the envelope arrives from outside).
- `decrypt_from_carrier(session, carrier, verifier)` dispatches on profile:
  `ClassicalP256` → `decrypt_sealed_segment` (`rail-prep`); `PqHybrid` →
  new **`PqSealedEnvelope::from_bytes`** wire-decode → `decrypt_pq_sealed_segment`
  (`pq-rail-prep`). The VM session secret is a separate argument — never a carrier
  field. CEK materializes only inside the engine, in `Zeroizing`, off the response.
- **7 characterization tests** (`cargo test --features rail-shim` = **41 green**):
  classical happy path is driven by the committed `classical_cenc.json` golden (so
  the shim and PC2-conformance share one fixture); PQ happy path uses the shared
  `seal_support` sealer; fail-closed is pinned for wrong session (both profiles),
  malformed carrier (both), profile/secret mismatch, and tampered PQ signature.

The base ladder is **unchanged** (25/27/29/31, `vectors` 37); `rail-shim` builds
clean to `wasm32-wasip1`; `ddrm-verify.sh` PASS. The day Anders answers, `OpenSession`
adds exactly one call — `rail_shim::decrypt_from_carrier(&vm_session_secret,
&carrier, &verifier)?` — then maps `(bytes, meta)` into the existing scoped response.
Q1 (dKMS-direct vs re-seal) does not touch the adapter; Q2 (signature scheme) plugs
in through the `CekSealVerifier`; profile is a per-deployment `SealProfile` pick.
Precise wire-up + question→knob mapping: `DDRM_DECRYPT_RAIL.md` §"Rail transport shim".

### Rail carrier pinned as a portable golden + checked against PC2's session API (Day 28)

The shim was proven in-process (Day 27); now its **carrier wire shape** is locked
the same way every engine is — a substrate-independent golden — and cross-checked
against PC2's *session model*, not just its crypto primitives:

- `tests/vectors/rail_carrier_classical.json` (schema `RailCarrierVector`) captures
  the rail Option-A carrier `{profile, sealed_cek, ciphertext_segment, init?,
  expected_plaintext}`. It is **derived from** `classical_cenc.json`, so its
  `sealed_cek` is byte-identical to the PC2-conformant fixture.
- `rail_shim::tests::rail_carrier_golden_replays_through_shim` replays it through
  **`decrypt_from_carrier`** (the exact entrypoint `OpenSession` will call) and
  recovers the plaintext; `…_tampered_fails_closed` pins fail-closed. So the
  carrier format now survives refactor/rebase/port, pinned at the shim boundary.
- **`scripts/pc2-conformance.sh` now checks two layers** against PC2's real code:
  the existing primitive parity (`envelope` + `cenc`) **and** the session/carrier
  path — a session holding the vector key runs PC2's public
  `session::unwrap_envelope` (L1 ECDH + L2 CEK store) → `media::decrypt_segment`
  (tenc IV-size + moof/traf/senc walk) and recovers the exact plaintext, for both
  the v3 and v2 envelopes; a tampered carrier fails closed inside `unwrap_envelope`
  too. This proves our Option-A carrier is wire-compatible with the **entrypoints
  PC2 production calls**, not merely its primitives.

`vectors` stays **37** and the base ladder is unchanged (25/27/29/31); `rail-shim`
= **43** (+2 carrier-golden); builds clean to `wasm32-wasip1`; `ddrm-verify.sh`
PASS (now including the two-layer session conformance). The carrier golden is the
artifact `OpenSession` will accept on the day Anders confirms the rail.

### PQ carrier golden — profile symmetry closed (Day 30)

Day 28 pinned the *classical* carrier as a portable golden + PC2 session conformance;
the **PQ-hybrid** profile now has the matching carrier golden:

- `tests/vectors/rail_carrier_pq.json` (schema `RailCarrierVector`, `profile: PqHybrid`):
  the `sealed_cek` is `PqSealedEnvelope::to_bytes()` (the carrier wire form the shim's
  `from_bytes` decodes); the VM session secret is carried as its **two parts** (x25519
  static secret + ML-KEM-768 decapsulation key) so replay reconstructs it with **no RNG**.
- `rail_shim::tests::rail_carrier_pq_golden_replays_through_shim` replays it through
  `decrypt_from_carrier`'s PQ branch (`from_bytes` → `decrypt_pq_sealed_segment`) and
  recovers the plaintext; `…_tampered_fails_closed` pins fail-closed.
- New `seal_support::session_secret_from_parts` reconstructs `SessionKemSecret`
  deterministically (mirrors the VM restoring its own session key).

**Deliberately no PC2 cross-impl layer for this profile.** The PQ-hybrid profile is
runtime-only (`elastos-pq-hybrid-threshold-v0`); PC2's `ddrm-decrypt` is classical
P-256 and has **no PQ session counterpart**, so there is no reference implementation to
check byte-parity against. (The classical carrier remains two-layer PC2-conformant.)

Base ladder unchanged (25/27/29/31, `vectors` 37); `rail-shim` = **45** (+2 PQ carrier);
builds clean to `wasm32-wasip1`; `ddrm-verify.sh` PASS. Both rail profiles now have a
carrier golden replayed through the exact `OpenSession` entrypoint.

### Media (cenc) golden widened toward real playback shapes (Day 31)

The media-contract goldens were all single-sample / single-subsample / default-IV.
Real fMP4 isn't, so the parts most likely to bite at wire-up are now pinned by
**executable PC2 parity**:

- `tests/vectors/classical_cenc_multisample.json` — a **3-sample** segment, each with
  its own per-sample IV and a fresh AES-128-CTR counter (`trun` per-sample sizes;
  `senc` no-subsample).
- `tests/vectors/classical_cenc_subsample.json` — a **subsample** sample (clear+encrypted
  ranges, `senc` flags `0x000002`); the CTR keystream is continuous across encrypted
  ranges only (clear bytes skipped).
- `tests/vectors/classical_cenc_initseg.json` — a **16-byte IV** segment whose size is
  driven by an `init` segment's `tenc.default_per_sample_iv_size` (moov→…→stsd→encv→sinf
  →schi→tenc), exercising the init-derived IV path.

Each replays through our decrypt engine (`vectors`, +3 → **40**) **and** through PC2's
real `mp4box::parse_segment` + `cenc::decrypt_samples` **and** PC2's session API
(`session::unwrap_envelope` → `media::decrypt_segment`, init threaded for the IV-size
case) in `scripts/pc2-conformance.sh`, asserting byte parity + tamper fail-closed. The
`ClassicalVector` schema gained optional `init_segment_b64` / `iv_size` (backward
compatible); the conformance driver now parses `senc` at the vector's IV size and passes
the init to `decrypt_segment`. Box layouts validated against PC2 `mp4box.rs`/`cenc.rs`
(byte-identical `parse_init_for_tenc`, incl. the encv 78-byte skip).

Base ladder otherwise unchanged (default 25; `rail-shim` 45); `wasm32-wasip1` clean;
`ddrm-verify.sh` PASS.

### Real ML-DSA-65 signature primitive — the last PQ placeholder closed (Day 32)

The PQ envelope's seal-signature was a **`StubSigner`/`StubVerifier`** (a SHA-256
placeholder behind the `CekSealVerifier` slot). The **real FIPS 204 ML-DSA-65**
primitive is now wired in, behind a new `pq-mldsa` feature (separate axis — the
default build + base ladder stay byte-stable):

- `pq_envelope::mldsa::MlDsa65Verifier` (production) implements `CekSealVerifier` over
  RustCrypto `ml-dsa` 0.1 (same family as the already-vetted `ml-kem`). The decrypt
  boundary only ever **verifies** — construction (`VerifyingKey::new_from_slice`) + verify
  need **no RNG**, so it compiles cleanly to **`wasm32-wasip1`** (the real constraint:
  ML-DSA verify inside the WASI sandbox). Fail-closed: a wrong-size key encoding yields
  no verifier; a malformed/non-matching signature verifies `false` (no panic, no
  which-half probe). `ml-dsa` is pulled with `default-features = false` (no pkcs8 /
  getrandom).
- **Proven (feature `pq-mldsa`, +5 tests → 34):** the real primitive plugs into the exact
  `hybrid_unwrap` path (genuine seal signature → CEK recovered; tampered sig → `BadSignature`);
  rejects a **wrong key**; rejects a **tampered body**; fails closed on **malformed**
  encodings.
- **Committed KAT** (`tests/vectors/mldsa65_kat.json`, schema `MlDsaKatVector`): a
  verifying key + signature over a fixed canonical transcript, generated deterministically
  via `SigningKey::from_seed`. Replayed under `pq-mldsa` (verify-accept + tamper-sig/body
  fail-closed). It pins the real primitive across refactor/rebase/port **and upstream-crate
  drift** — if `ml-dsa` ever changed its keygen/signature output, this would stop verifying.

**What this means for "quantum-proof":** the PQ rail is no longer stubbed anywhere — the
shipped signature primitive is real and WASI-verified. The remaining PQ gaps are now purely
*external*: Anders' Q2 transition policy (straight ML-DSA-65 vs hybrid ECDSA+ML-DSA during
PC2's migration) and landing the rail (the `rail-shim` flag-flip, which already accepts any
`CekSealVerifier` — `MlDsa65Verifier` drops straight in).

Base ladder byte-stable (default 25 / rail-prep 27 / pq-envelope 29 / pq-rail-prep 31 /
vectors 40 / rail-shim 45); new `pq-mldsa` = **34**; `wasm32-wasip1` clean (default +
`pq-mldsa`); `ddrm-verify.sh` PASS.

### Real ML-DSA-65 verified through the rail entrypoint — loop closed (Day 33)

Day 32 proved the real primitive in `hybrid_unwrap`; Day 33 drives it through the **exact
`decrypt_from_carrier` entrypoint** `OpenSession` flag-flips on, on a **committed
real-signed carrier golden** (feature `rail-shim-mldsa = rail-shim + pq-mldsa`):

- `tests/vectors/rail_carrier_pq_mldsa.json` — a PQ-hybrid carrier whose `sealed_cek`
  signature is a **genuine FIPS 204 ML-DSA-65 signature** (key authority key deterministic
  via `from_seed`, so the golden is reproducible). It carries the published verifying key
  (`mldsa_vk_b64`, new optional `RailCarrierVector` field — needed because the real verifier
  holds a key where the stub held none).
- Replayed through `decrypt_from_carrier`'s PQ branch verified by the production
  `MlDsa65Verifier(mldsa_vk_b64)` (**not** the stub): plaintext recovered; and fail-closed on
  (a) **tampered signature**, (b) a **different verifying key**, (c) a **tampered envelope body**.
  +4 tests → `rail-shim-mldsa` = **54**.

This is the strongest possible pre-rail proof: *the real PQ signature, verified through the
real rail entrypoint, on a portable committed artifact.* The day Anders answers Q2, the live
`OpenSession` passes a `MlDsa65Verifier` into the one `decrypt_from_carrier` call — nothing
else changes. `DDRM_DECRYPT_RAIL.md` Q2 updated: no longer a build gap, purely a policy choice.

Base ladder + `pq-mldsa` byte-stable (25/27/29/31/40/45; `pq-mldsa` 34); new
`rail-shim-mldsa` = 54; `wasm32-wasip1` clean; `ddrm-verify.sh` PASS.

### Both Q2 answers pre-proven — hybrid ECDSA+ML-DSA verifier (Day 41)

The straight-ML-DSA-65 answer to Anders' open Q2 was proven Day 32–33. Day 41
pre-proves the **other** answer so Q2 is purely a policy pick, never a build task: a
**hybrid** seal-signature verifier where a classical **ECDSA-P256** signature AND a PQ
**ML-DSA-65** signature must **both** verify — the migration-period profile (the key
authority can dual-sign while PC2 moves classical→PQ; a verifier trusting neither
algorithm alone still accepts).

- **`pq_envelope::hybrid::HybridVerifier`** (new feature `pq-mldsa-hybrid = pq-mldsa
  + p256/ecdsa`, off by default). Slots into the **same** `CekSealVerifier` the rail
  uses, driven through the exact `hybrid_unwrap` path the straight verifier uses — so
  `OpenSession` just constructs whichever verifier the policy selects.
- **Fail-closed, defense-in-depth (not OR-trust):** wire shape `u32 ecdsa_len ‖
  DER ‖ u32 mldsa_len ‖ mldsa`; **both halves required** (a valid ECDSA half with a
  wrong ML-DSA key still fails, and vice-versa), tampered signature → `BadSignature`,
  every proper prefix / trailing byte / garbage framing verifies `false` without
  panic, malformed key encoding yields no verifier. Verify-only + RNG-free →
  `wasm32-wasip1`-clean.
- **Proven (feature `pq-mldsa-hybrid`, +3 tests → 37):** `hybrid_real_signatures_drive_hybrid_unwrap`,
  `hybrid_requires_both_halves`, `hybrid_malformed_inputs_fail_closed`.

Base ladder byte-stable; new `pq-mldsa-hybrid` = **37**; `wasm32-wasip1` clean
(default + `pq-mldsa` + `pq-mldsa-hybrid` + `rail-shim-mldsa`); `ddrm-verify.sh` PASS.
`DDRM_DECRYPT_RAIL.md` Q2 updated: both answers now drop-in, the rail is pure wiring.

### Fail-closed under adversarial input — proven (Day 34)

The wire-decoders are the surfaces the rail exposes to **attacker-controlled carrier
bytes** (`envelope::parse`, `PqSealedEnvelope::from_bytes`, `decrypt_from_carrier`
dispatch). A new test-only `harden` feature (= `rail-shim-mldsa`; off by default, base
ladder byte-stable) adds an adversarial negative-space + containment sweep (+11 →
`harden` = **65**):

- **Truncation sweep:** *every* proper prefix of a valid envelope/carrier fails closed
  (classical + PQ).
- **Byte-flip sweep:** single-byte corruption at *every* position **never panics** (a
  panic in a wasm capsule is a DoS) — classical parse, PQ `from_bytes`, and the
  `decrypt_from_carrier` dispatch.
- **Oversized length prefixes:** over-large `u16`/`u32` prefixes (incl. `u32::MAX`,
  exercising the `checked_add` overflow guard) fail closed — no over-read.
- **Corruption-never-recovers:** a tampered-but-decodable PQ carrier still fails closed
  at unwrap (AES-256-GCM auth ‖ ML-DSA-65 signature) — never yields a CEK; error
  surfaces stay coarse (no which-field probe).
- **Containment:** profile/secret mismatch fails closed **both** directions; and neither
  the scoped metadata (happy path) nor the error string (tampered) contains the plaintext
  or the CEK across the carrier path.

This makes "**fail-closed and panic-free under adversarial input**" — a core capability-
security claim — executable, on the exact boundaries the rail will expose. Base ladder
byte-stable; `wasm32-wasip1` clean; `ddrm-verify.sh` PASS.

### Producer round-trip widened to real playback shapes (Day 37)

Day 26 pinned invariant #1 ↔ #2 on a single-sample artifact; the decrypt side already
proved multi-sample / subsample / non-default-IV shapes (Day 31). Day 37 closes that
asymmetry **from the producer end**: `encrypt-provider`'s real in-boundary engine
(`mint CEK+KID → cenc::encrypt_samples`) now emits two more round-trip goldens —
`roundtrip_multisample_encrypt_to_decrypt.json` (4 samples, per-sample IVs) and
`roundtrip_subsample_encrypt_to_decrypt.json` (16-byte clear leader + encrypted body) —
muxed with framing that mirrors PC2 `cenc-encrypt::mp4box::build_senc` /
`build_senc_with_subsamples`. `decrypt-provider` replays each back to the producer's
**exact** plaintext with the CEK off the scoped boundary (`vectors` 40 → **42**). The gate
(`ddrm-ladder-check.sh`) runs all three `*_round_trip_golden` tests **by name** (asserts 3
passed), so an encrypt-side change that breaks decrypt over any shape fails the gate.
`wasm32-wasip1` clean; `ddrm-verify.sh` PASS.

### Producer output proven consumable by PC2's real decrypt (Day 38)

The Day-37 round-trips proved *our* producer ↔ *our* consumer. Day 38 closes the
convergence-critical loop: the multi-sample + subsample segments
`encrypt-provider`'s real in-boundary engine emitted are now driven through **PC2
`ddrm-decrypt`'s real `mp4box::parse_segment` + `cenc::decrypt_samples`** in
`scripts/pc2-conformance.sh`, asserting byte-for-byte plaintext parity plus a
wrong-CEK key-bound check (PC2 must NOT recover the plaintext under a flipped CEK).
The driver dispatches on schema (classical envelope vectors keep their two-layer
envelope+session parity; producer round-trips, which carry no envelope, run the
segment-decrypt parity). PC2 decrypts our producer's output byte-for-byte —
**our producer ↔ PC2's real decrypt is now executable**, not just our internal
round-trip. `ddrm-verify.sh` PASS with PC2 present.

## Integrity audit — every claim maps to a gate (Day 40)

A "trust-but-verify-the-verifier" pass: every "proven"/count claim above was checked
against something the standing gate (`scripts/ddrm-verify.sh`) or a named test
actually enforces. Counts were re-validated by running the suites fresh, not from
memory.

| Claim | Enforced by (re-run Day 40) |
|---|---|
| 5 providers fail-closed, host-tested (13/12/9/9/25 = 68) | gate 3 ladder — per-provider suites, counts asserted |
| decrypt feature ladder (27/29/31/42/45/34/54/65) | gate 3 ladder — each rung run, count asserted (a dropped/feature-gated-out test fails the gate) |
| contract types intact on the current base | gate 1 drift — 13 consts / 10 structs / 1 fn / 10 fields |
| byte-compatible with PC2 (consumer *and* producer) | gate 2 conformance — classical envelope+session + producer round-trip segments through PC2's real code; skips clean w/o PC2 |
| builds to `wasm32-wasip1` (5 providers + PQ/rail features) | gate 3 ladder — 7 wasm builds |
| **executes under WASI, fail-closed end-to-end** | gate 4 WASI smoke — `ddrm-chain-smoke.sh` under wasmtime (added to the standing gate Day 40; skips clean w/o wasmtime) |
| encrypt↔decrypt seam over real shapes (single/multi/subsample) | gate 3 seam — all 3 `*_round_trip_golden` run by name (3 passed) |
| real ML-DSA-65 verified through the rail entrypoint | gate 3 — `rail-shim-mldsa` rung (54) + committed `rail_carrier_pq_mldsa.json` |
| hybrid ECDSA+ML-DSA verifier (both Q2 answers pre-proven) | gate 3 — `pq-mldsa-hybrid` rung (37) |
| fail-closed + panic-free under adversarial input | gate 3 — `harden` rung (65) |

**Orphan / dead-surface sweep:** all **13** committed golden vectors in
`decrypt-provider/tests/vectors/` are referenced by at least one test or the
conformance script (no orphan fixtures). Every decrypt-provider feature flag is a
ladder rung except `gen-vectors` (a fixture-regeneration tool, intentionally not a
test rung) — no documented-but-unwired flags. The only previously doc-only claim
("executes under WASI") is now gate-backed (gate 4). `ddrm-verify.sh`: **ALL GATES
PASS** (with PC2 + wasmtime present on this machine).

## The one open decision (for Anders / Irzhy)

How the CEK reaches the decrypt boundary. **Hybrid chosen** (decrypt step
*receives* sealed material; upstream rights→key is a provider chain). Irzhy
independently converged on the same gap and proposed **two boxes + secured channel
(ECDH + DSA)** over merging — adopted, upgraded to the runtime PQ-hybrid profile.
Three sharpened sub-questions remain for Anders:

1. Does the **dKMS seal directly** to the decrypt session key (key-provider as a
   pure broker that never holds a raw CEK), or is a key-provider **re-seal** ok?
2. Signature during transition: straight to **ml-dsa-65**, or a **hybrid**
   (ECDSA + ml-dsa) while PC2's classical path is migrated? *(BOTH answers are now
   built + WASI-verified and drop into the `CekSealVerifier` slot: straight ML-DSA-65
   behind `pq-mldsa`/`rail-shim-mldsa`, and the hybrid ECDSA-P256+ML-DSA-65
   `HybridVerifier` behind `pq-mldsa-hybrid` (Day 41). Purely a policy choice now, not
   a build gap — `OpenSession` constructs whichever verifier is selected.)*
3. Does the provider-invocation rail expose an in-capsule `carrier_invoke` client
   a microvm provider may use today, or is that still landing?

Full options, threat model, and the invariant→test table:
`DDRM_DECRYPT_RAIL.md` + `DDRM_SECURITY_MODEL.md`.

## Isolation tier

Providers ship as **`wasm` now** (proven cross-platform, runs on macOS today);
**microVM** remains the later max-isolation upgrade from the same Rust source. The
fail-closed contract is tier-independent. Rationale in `DDRM_DECRYPT_RAIL.md`.

## Base reconciliation (Day 17) — 0.4.0 force-push, zero type drift

Anders force-pushed `origin/0.4.0` (`42e4d7ffd` → `67b7560a7`), redoing commits as
warned, with more still to come. We did **not** rebase yet (0.4.0 is still moving),
but verified the impact:

- **`elastos-common/protected_content.rs` is byte-identical** between this branch
  and the redone `origin/0.4.0` (`git diff` = 0 lines). The redone base
  independently landed the exact types our providers were built against
  (`RightsDecisionReceiptV1`, `KeyReleaseRequestV1.rights_receipt`, typed
  `DecryptSessionRequestV1.release_receipt`, `ReleaseReceiptV1.session_id/action`).
  **The convergence held — zero type drift.**
- A drift guard, `scripts/ddrm-drift-check.sh`, asserts every schema constant,
  struct, and chain-binding field the chain depends on still exists on the current
  base. Run it before any rebase/PR; it fails loudly if a future 0.4.0 redo moves a
  type. **Currently: PASS.**
- All five providers' host tests pass against the current tree:
  `encrypt 13`, `drm 12`, `rights 9`, `key 9`, `decrypt 25` → **68 green, 0 ignored**
  (Day 19 closed the encrypt keygen gap: 6+1-ignored → 13). `decrypt` adds **+2**
  under `--features rail-prep` (Day-18 rail-landing composition).
- Rebase recipe + safety backup (`backup/decrypt-provider-cenc-preD17`):
  `PUSH_PLAN.md` § "Base moved".

**Day 36 reconcile-prep re-verification.** Re-measured against the force-pushed base:
- `origin/0.4.0` is **no longer an ancestor** of our branch (diverged; merge-base
  `589092b95`, +3 base commits). The rebase recipe now uses `git rebase --onto
  origin/0.4.0 "$(git merge-base …)"` so only our own commits replay, with a
  `ddrm-verify.sh` checkpoint after each branch and `git range-diff` to confirm
  nothing drops. `PUSH_PLAN.md` § "Rebase recipe" is now button-press + has the
  per-branch conflict surface (incl. the `encrypt-provider` self-containment and
  bincode-2x churn points).
- **Contract still byte-identical** (re-verified): `git diff
  origin/0.4.0..feat/decrypt-provider-cenc -- …/protected_content.rs` = 0 lines.
- **Drift guard widened to the full consumed surface** (was a Day-17 subset): now
  pins **13 consts / 10 structs / 1 free fn / 10 fields**, adding the genuinely-
  consumed-but-unpinned symbols — `validate_protected_content_key_envelope_algorithms`
  (called by drm + key), the `DEFAULT_PROTECTED_CONTENT_*` algorithm sets,
  `ViewerRequirementV1`, and the PQ-negotiation fields on `KeyEnvelopeAlgorithmsV1`
  (`cipher`/`kem`/`signature`/`share_scheme`). A rename of any now fails the guard
  loudly instead of surfacing as a compile error mid-rebase.
- **The encrypt↔decrypt seam is now gate-enforced:** `ddrm-ladder-check.sh` runs
  `encrypt_to_decrypt_round_trip_golden` **by name** and asserts 1 passed, so an
  encrypt-side change that breaks decrypt (or a silent cfg/rename drop of the
  cross-invariant golden) fails the gate.

## Commits (on `feat/decrypt-provider-cenc`, not yet pushed — GitHub suspension)

**17 of our commits** (the original 14 below, plus Day 15 status/PQ, Day 16
encrypt-provider, Day 17 drift guard). Note: `git rev-list --count
origin/0.4.0..HEAD` reports **19** against the force-pushed base because 2 orphaned
old-upstream commits are still in range — the rebase (`PUSH_PLAN.md`) drops them.
Newest last:

1. `docs(convergence)` — north-star playbook, product vision PRD, v0.4.0 plan
2. `feat(decrypt-provider)` — vendor PC2 cenc-decrypt engine as fail-closed backend
3. `docs(ddrm)` — record decrypt-rail decision (CEK/ciphertext transport)
4. `feat(decrypt-provider)` — tested decrypt-step core seam (Branch-by-Abstraction)
5. `docs(ddrm)` — isolation-tier recommendation (wasm now, microVM as hardening)
6. `docs(ddrm)` — confirm decrypt-provider compiles clean to wasm32-wasip1
7. `test(decrypt-provider)` — WASI-sandbox smoke harness proves fail-closed execution
8. `feat(key-provider)` — bind rights receipt + bring to wasm/WASI-proven bar
9. `test(rights-provider)` — WASI smoke completes rights→key→decrypt chain parity
10. `test(drm-provider)` — WASI smoke + cross-provider contract-seam tests
11. `feat(ddrm)` — unified chain smoke runner + review-ready status package
12. `feat(ddrm)` — vendor ECDH CEK-sealing envelope spec + PC2 player alignment
13. `test(ddrm)` — pin decrypt→player consumer contract for both viewer capsules
14. `docs(ddrm)` — security model doc + inter-stage CEK transport decision

Push order & PR mapping when GitHub returns: `PUSH_PLAN.md`.

Supporting docs: `DDRM_DECRYPT_RAIL.md`, `DDRM_SECURITY_MODEL.md`,
`PC2_PLAYER_ALIGNMENT.md`, `CONVERGENCE_PLAYBOOK.md`, `PRODUCT_VISION.md`,
`PUSH_PLAN.md`.
