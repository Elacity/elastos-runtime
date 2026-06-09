# ElastOS Runtime — Convergence Handover (read me first)

**Purpose.** This is the single entry point for a new engineer/agent picking up the
ElastOS Runtime ⇄ PC2 convergence work in a fresh context window. Read this top to
bottom once; it tells you exactly what we're doing, why, what's done, what to read,
and how to continue at the same quality bar — with no loss of insight.

> **🚀 NEW AGENT? Start with [`NEW_AGENT_BRIEF.md`](./NEW_AGENT_BRIEF.md).** It is the
> self-contained, zero-blind-spots onboarding: the mandate (*why this matters*), the whole
> system as visual maps, the full Day 45–66 ledger, where the PC2 reference lives and what
> to study, the runtime principles, the daily 10/10-prompt working format, and the exact
> bootstrap prompt to paste. Read it first, then use this file as the running day log.

**Last updated:** 2026-06-10 (end of Day 103–104).
**Active branch:** `feat/decrypt-provider-cenc` (tip Day-103–104 — **the threshold's identity is now CRYPTOGRAPHIC + AUDITABLE: the node-set is welded into the decrypt-transcript AAD itself (a swapped node-set fails the AEAD open AT THE BOUNDARY, not just at descriptor parse), every durable open record is STAMPED with the serving node-set identity, and ROTATION is fail-closed (a stale publish can never open against a rotated node-set).** Day 101–102 pinned the node-set at descriptor parse; this cycle makes the binding cryptographic + after-the-fact provable. Audited PC2 first: PC2's decrypt-side binding is `SHA-256(cek‖kid‖authority)` recomputed in the TEE vs the encrypt-time `dataToEncryptHash` (`universal-decrypt-chipotle.js:577`–`:589`) — a SINGLE authority address, no node-set; and PC2 has NO key-authority rotation concept at all ("rotation" there only ever means supernode provision-blob / Lit action CID rotation by manual redeploy, `chipotle-client.ts:125`/`:1043`/`:1064`); its audit trail can never say WHICH nodes served a decrypt (they're opaque inside Lit). The runtime is SUPERIOR on all three counts. **(transcript)** `DecryptTranscriptV1` gained an optional `node_set_id` field, appended to `to_aad()` ONLY when present — the single-node encoding stays BYTE-IDENTICAL (proven: the threshold AAD is a strict extension; ddrm-envelope 24→25). The runtime computes the open AAD with the descriptor-derived node-set (`transcript_aad(.., node_set_id)`, reusing the Day 101–102 pin-checked value via the new shared `derive_node_set_from_descriptor` helper); BOTH dkms nodes seal their shares to it (the AAD is opaque bytes to them — no node change); and the decrypt boundary independently derives the SAME id from its OWN pinned vks (`threshold_node_set_id(2, authority_vk, authority_vk2)`) in `open_session_threshold` → `prepare_bound_open(.., node_set_id)` — so a release whose node-set was swapped fails the AEAD open in the sandbox even when every per-share signature verifies. Plus defense-in-depth: a threshold-provisioned boundary now REFUSES a single-share material outright (never silently accepts a degraded release). rail-material 68→70 (+a genuine-nodes seal NOT bound to the node-set is denied; +a single-share material at a threshold boundary is refused). **(auditable record)** a new runtime-open `NodeSetStampingSink` persists the SAME CEK-free `open_event_record` shape and stamps `node_set_id_b64` into every durable record on the threshold rail (a public hash over public vks — the CEK-free invariant untouched; `None` single-node records stay byte-identical); the smoke reads the records back through a fresh `DurableEventStore` and asserts the stamp equals the producer pin (and that single-node records carry NO stamp). **(rotation)** verify gate 27: provision a REAL fresh node B′ (own store → genuinely distinct identity), publish a rotated descriptor naming it, and prove the OLD fixture's pin REFUSES it via the SAME `derive_node_set_from_descriptor` path `run()` enforces — a rotation is a NEW publish; a stale fixture fails closed; the rotated descriptor re-derives stably for a new publish. **(adversarial, live cross-binary)** gate 26: drive the LIVE key capsule with a well-formed release whose AAD names a FORGED node-set (both nodes re-seal honestly — the release SUCCEEDS), then prove the LIVE decrypt capsule refuses to open it (it rebuilds the AAD over the node-set IT trusts). Node B's daemon is now restored after the gate-23 kill so the live dual-recover gates downstream still run. Gate: ladder INTACT (ddrm-envelope=25, key-provider[key-authority-ref]=43, decrypt-provider rail-material=70), drift PASS, all dDRM smokes green (reference + dkms single-node + dkms 2-of-2 with gates 26–27 + the stamped-record read-back), clippy clean (no new warnings). **Escape hatch NOT needed:** transcript binding + auditable record + rotation gate + the live forged-node-set gate all landed. Earlier tip Day-101–102 — **the live 2-of-2 threshold is now RESILIENT + IDENTITY-BOUND: the production `DrmHost` rail provably FAILS CLOSED under a real node fault (either secret-holder down), NEVER degrades to a single node, and a silently SWAPPED node-set is DETECTED before the rail recovers anything.** Day 99–100 wired the threshold into the real open; this cycle proves it survives faults + pins WHO backs it. Audited PC2 first: PC2's run-path resilience STOPS at retrying the whole opaque Lit RPC (`chipotle-client.ts:575` — `RequestExpired` → "retry by re-running the Lit action"); a downed node lives INSIDE Lit's network so PC2 has NO per-node fault semantics and NO node-set identity it can pin (both are uninspectable). The runtime is SUPERIOR — it owns the two nodes, so it expresses both. **(node-set identity)** new pure `ddrm_envelope::threshold_node_set_id(t, vk_a, vk_b)` (domain-separated, length-prefixed SHA-256 over both nodes' vks + `t`; order-sensitive; 23→24) is the SINGLE SOURCE OF TRUTH for "which secret-holders back this rail." `publish_escrow` PINS it into the durable fixture (`node_set_id_b64`); `host.open()` RE-DERIVES it from the published descriptor's `threshold` block and FAILS CLOSED if they differ — a node silently re-pointed at a different secret-holder is caught BEFORE recovery (independent of the boundary's per-share seal check). **(node-fault fail-closed)** verify mode adds live gates 23–24 (threshold only): with the full 2-of-2 rail up, KILL node B's daemon → `host.open()` fails closed (no partial CEK, no single-node fallback, NO record persisted), then restore; KILL node A's daemon → same; node A is RESTARTED so the downstream socket probes still run. The daemon guards (`dkms_daemon`/`dkms_daemon_b`) are now `mut` so the gates kill+restart them. **(swap detection)** gate 25: a descriptor whose node B is swapped to a rogue ML-DSA identity re-derives to a DIFFERENT `node_set_id` than the pin — detected end-to-end (and the boundary independently rejects the rogue's seal under node B's pinned vk, Day 97–98 step 20, so the swap fails at BOTH layers). Gate: ladder INTACT (ddrm-envelope=24, key-provider[key-authority-ref]=43, decrypt-provider rail-material=68), drift PASS, all dDRM smokes green (reference + dkms single-node + dkms 2-of-2 with the new node-fault + swap gates), clippy clean (one PRE-EXISTING rng-borrow warning, not introduced here). **Escape hatch NOT needed:** full node-set-id binding + both live node-fault directions + the swapped-node detection all landed. Earlier tip Day-99–100 — **the 2-of-2 threshold now runs through the PRODUCTION `DrmHost` run-path, not just the verify-mode probe: the happy path itself provisions TWO secret-holding nodes, XOR-splits the CEK at publish, dual-recovers BOTH, and reconstructs the CEK ONLY inside the decrypt boundary — the CEK never exists whole before the boundary.** Day 97–98 landed the threshold crypto + a self-contained probe; this cycle wires it into the real open. Audited PC2 first: PC2's run-path (`recoverCEKEnvelope` → ONE Lit RPC, `chipotle-client.ts:1438`) NEVER orchestrates multiple nodes in its own code — `decryptAndCombine` is the LEGACY Datil threshold that lives entirely inside Lit's network (opaque, `chipotle-client.ts:1297`), and the current Chipotle path is a single-node TEE decrypt; PC2's runtime STOPS at one RPC. The runtime is SUPERIOR — it drives TWO owned nodes end to end through its own host. **(config)** `OpenConfig.authority.threshold` (bool) promotes the dkms open to 2-of-2; fail-closed if set with `backend != dkms` or non-boolean (+2 bin config tests, 8→10). We provision BOTH nodes from the same node binary, so it's a boolean knob (not a handed-in node-B descriptor path) — the descriptor's `threshold` block the key-provider consumes is what the runtime OWNS producing. **(publish)** `publish_escrow` provisions node A + node B (distinct stores/sockets/allow-lists), `split_cek_xor`s the CEK so node A escrows share-1 + node B escrows share-2 (neither sees the whole key), and publishes a `threshold` descriptor (`t:2`, both nodes); the fixture carries `wrapped_cek_share2_b64` + node B's `vk2_b64`. **(run-path)** the `DrmHost` starts BOTH daemons, binds `KeyOpenMaterial.wrapped_cek_share2_b64`, passes node B's vk to the `DecryptLauncher` (`authority_vk2_b64`), and `KeyHandle` supplies `wrapped_cek_share2_b64` in the `release` session — so `host.open()` drives the full dual-recover + in-VM XOR combine; a threshold↔descriptor desync fails closed. **(integration fix)** `merge_threshold_material` now welds node B's share into node A's NESTED `material.sealed_cek_share2_b64` (the shape the decrypt boundary consumes) — the Day 97–98 merge read a top-level field that the real node never emits, so it was never exercised end-to-end; the unit test was corrected to the real nested recover shape (key-provider[key-authority-ref] stays 43). **(adversarial)** verify mode adds gates 21–22 (threshold only): the live threshold rail REFUSES a release that supplies only ONE share (never degrades to a single node), and a 3-of-N descriptor FAILS CLOSED at key-provider init (the runtime never silently downgrades a stronger threshold). **(smoke)** new `ddrm-consumer-dkms-threshold-smoke.sh` (+ a `--threshold` flag on the consumer smoke) drives the whole 2-of-2 open cross-binary. Gate: ladder INTACT (ddrm-envelope=23, key-provider[key-authority-ref]=43, decrypt-provider rail-material=68), drift PASS, all dDRM smokes green (reference + dkms single-node + the NEW dkms 2-of-2), clippy clean. Earlier tip Day-97–98 — the threshold crypto is REAL: the CEK is XOR-split 2-of-2 across TWO secret-holding dKMS nodes so NO single node ever holds the whole content key, and the runtime reconstructs it ONLY inside the decrypt boundary. Day 95–96 left a fail-closed threshold STUB; this cycle makes it real end to end. Audited PC2 first: PC2's threshold is the OPAQUE Lit `decryptAndCombine` (`non-media-decrypt.js:76`) — the share set, the nodes, and the combine all live INSIDE Lit's proprietary network, uninspectable to PC2; the runtime is SUPERIOR here — an EXPLICIT, owned, inspectable 2-node split with the combine in our OWN sandbox. **(envelope)** `ddrm-envelope` gained pure `split_cek_xor(cek,mask)` (producer: `share1=mask`, `share2=cek⊕mask`, info-theoretically hides the CEK in either share alone) + `combine_cek_xor(s1,s2)→Zeroizing` (decrypt boundary: `cek=share1⊕share2`, fail-closed on length mismatch); 22→23. **(decrypt boundary)** `decrypt-provider` reconstructs IN-VM: `SealedDecryptMaterialV1` gained an optional `sealed_cek_share2_b64` and the boundary an optional second trusted node vk (`authority_vk2_b64`); when a second share is present, `rail_shim::decrypt_from_carrier_threshold` unwraps BOTH sealed shares (each under ITS node's vk, bound to the SAME transcript), XORs them in `Zeroizing`, then decrypts — the whole CEK exists ONLY in the sandbox, never in `key-provider`; single-share path unchanged; rail-material 65→68 (+happy 2-of-2, +unauthorized-second-share denied, +missing-second-vk fail-closed). **(key-provider)** the Day 95–96 stub is REPLACED: `build_dkms_client` resolves a PUBLIC-ONLY 2-of-2 `threshold` descriptor (`t==2`, two DISTINCT node entries) into TWO clients (3-of-N / identical-nodes / malformed all fail closed); `release` runs hello+recover against BOTH nodes over their OWN long-lived connections (known-caller, fresh `recover_seq`, possession proof per node), collects TWO re-sealed shares, and `merge_threshold_material` welds them into one two-share material WITHOUT XOR-combining (the CEK is never reconstructed here); the second share's escrow rides in the runtime-injected session context (`wrapped_cek_share2_b64`); 42→43 (+real 2-of-2 resolution & fail-closed, +merge helper). **(end-to-end)** `ddrm-runtime-open` verify mode adds a 2-of-2 probe (steps 18–20) that starts TWO real node daemons (distinct stores/sockets/allow-lists), escrows share-1→node A + share-2→node B, recovers a re-sealed share from EACH node over the full session/possession/freshness gates, then (as the decrypt boundary) unwraps both + reconstructs the EXACT CEK — and proves a single share is USELESS (not the CEK) and a FORGED second share fails closed under node B's vk. Gate: ladder INTACT (ddrm-envelope=23, key-provider[key-authority-ref]=43, decrypt-provider rail-material=68), drift PASS, all dDRM smokes green (incl. the dkms 2-of-2 probe), clippy clean (no new warnings). **Escape hatch used (per the 2-day prompt):** the production `DrmHost` run-path live dual-recover + its dedicated end-to-end smoke is the Day 99–100 finisher; this cycle landed the full producer split + two-daemon provisioning + `key-provider` dual-recover orchestration + the real in-VM reconstruction, all proven cross-binary. Earlier tip Day-95–96 — the dkms node now serves only a KNOWN, ALLOW-LISTED caller, every recover is FRESH (anti-replay), and a THRESHOLD descriptor fails closed. Day 93–94 gave a real transport + a non-replayable bearer session; Day 95–96 hardens WHO the node serves and makes each recover single-use. **(known-caller)** the node takes an OPERATOR-provisioned allow-list (`DKMS_AUTHORITY_ALLOWED_CALLERS`, resolved once at daemon start, never overridable by the connecting client); `hello` REFUSES a caller whose ephemeral identity key is not on the list (`caller_not_authorized`) BEFORE minting a token (the OWNER-BOUND analogue of `secureViewSession.ts:87`–`:100`); no allow-list → anonymous (dev/test). `key-provider` connects as its OWN stable identity derived from a runtime-provisioned `dkms_caller_seed_b64` — the rail AND the adversarial probe derive the SAME identity, which the runtime provisions into the node's allow-list (it is the runtime's own identity key, never the dKMS master or a CEK). **(anti-replay)** the possession proof now binds a per-recover `recover_seq`; the node tracks the highest consumed in the session and REFUSES any recover that does not strictly advance (commit-on-success) — a captured recover frame replayed verbatim is refused (the revocable-`nonce` analogue, `secureViewSession.ts:108`–`:112`). **(threshold seam)** `key-provider` RECOGNIZES a `threshold` descriptor (`t>1`/multi-node) and FAILS CLOSED rather than recovering from one node and pretending; a single-node (`t==1`/absent) descriptor still resolves — the real 2-of-N CEK-share split is the next cycle. Counts: ddrm-envelope=22 (recover proof binds `recover_seq`), dkms-authority 11→13 (allow-list on hello + replayed-seq refused), key-provider[key-authority-ref] 41→42 (stable caller identity + freshness counter + threshold fail-closed). `ddrm-runtime-open` provisions a per-run KNOWN caller into the daemon allow-list, hands the seed to both the rail + probe, and adds two adversarial gates against the REAL daemon (an UNKNOWN caller's hello refused; a REPLAYED recover frame refused after three strictly-advancing successful recovers); the reference path stays green. Drift untouched (the allow-list + freshness counter are capsule-local protocol). Gate: ladder INTACT (ddrm-envelope=22, dkms-authority=13, key-provider[key-authority-ref]=42), drift PASS, all dDRM smokes green (incl. dkms), clippy clean. Earlier tip Day-93–94 — the long-lived dkms node now has a REAL transport boundary and the bearer session is NON-REPLAYABLE across callers, closing the two seams Day 91–92 deferred. The node is no longer a stdin/stdout CHILD `key-provider` spawns: it BINDS + LISTENS on a Unix-domain socket and serves a length-prefixed FRAMED request/response (SAME JSON ops — a transport swap, not a protocol change), many SEQUENTIAL connections with ONE session per connection, and a torn/oversized/half-closed frame fails closed WITHOUT wedging the daemon (the connection is dropped, the listener serves on). The runtime CONNECTS to a node whose process it does NOT own (`ddrm-runtime-open` starts the node DAEMON listening before the rail + reaps it via a `DaemonGuard`). And the session is no longer a pure BEARER credential: the client mints an EPHEMERAL keypair per connection, sends its public half at `hello`, the node BINDS the session token to that pubkey (`challenge‖caller_pub‖expires_at`), and every `recover` REQUIRES a signature under the matching PRIVATE key the node verifies against the token-bound pubkey — a token captured + replayed by a DIFFERENT caller (no key) or signed by the WRONG key is refused. Audited PC2 first: the secure-view session is OWNER-BOUND — the stored `ownerAddress` must equal the authenticated wallet or `403 session_owner_mismatch`, and the Lit Action re-checks via `ecrecover(delegationSig) === del.ownerAddress` in the TEE (`secureViewSession.ts:87`–`:100`); the Boson proxy FRAMES every packet `[2-byte length][1-byte type][body]` + `MAX_PACKET_SIZE`/`PACKET_HEADER_SIZE` (`ProxyProtocol.ts:13`/`:251`/`:256`/`:371`). A NEW shared `ddrm-envelope` FRAME module (`frame::write_frame`/`read_frame`, `[4-byte BE len][payload]`, `MAX_FRAME_BYTES=1 MiB`, fail-closed on torn/oversized/zero) + caller-bound session token + recover possession-proof primitive back all three sides (single source of truth). Counts: ddrm-envelope 20→22, dkms-authority 9→11, key-provider[key-authority-ref]=41 (transport swapped + conn boxed; socket code `unix`-gated so wasm32-wasip1 stays clean). `ddrm-runtime-open` verify mode proves it cross-binary against the REAL daemon over the SOCKET — step 13: identity pinned + CALLER-BOUND token minted; step 14: NO/EXPIRED/FORGED/tampered token, NO possession proof, and a WRONG-KEY proof ALL refused; step 15: even WITH a live session+proof a DENIED/wrong-content receipt is refused; step 16: ONE socket connection+session → THREE successful recovers; step 17: a torn AND an oversized frame each fail closed without wedging the daemon, a clean session afterwards still succeeds — and the genuine open flows through the framed socket; the reference path stays green. Drift untouched (the frame module + possession proof are capsule-local protocol). Gate: ladder INTACT (ddrm-envelope=22, dkms-authority=11, key-provider[key-authority-ref]=41), drift PASS, all dDRM smokes green (incl. dkms), clippy clean. Earlier tip Day-91–92 — the dkms node is now a LONG-LIVED CONNECTION the client opens ONCE + the handshake mints a node-bound SESSION the node REQUIRES on every recover. Day 89–90 authenticated the channel but `key-provider` still SPAWNED a fresh node + re-handshook EVERY release, and the verified handshake gated nothing beyond that single call. Now the dkms client holds a long-lived `DkmsNodeConn` (the live child + the cached node-signed session token): it OPENS-ONCE (spawn + init + identity handshake + capture token), REUSES the connection + session across releases, and re-establishes fail-closed only when the session has expired (or with no clock) — the per-release spawn/shutdown is gone. The `dkms-authority` node's `hello` now also mints a node-SIGNED SESSION TOKEN (binding the client's challenge + `now+300s`, signed with the master-derived key) and `recover` REQUIRES one — verified under the node's OWN verifying key + checked unexpired against the caller's clock, fail-closed on a missing (hard parse error) / expired / forged / tampered token, BEFORE re-authorization and BEFORE any key material — so a captured/forged handshake can't drive recovery and a token minted for one challenge can't authorize a recover under a tampered challenge/binding. Audited PC2 first: the per-view session is ESTABLISHED ONCE (`begin-session`) + only RESURRECTED per request to gate recovery (`getSessionByToken`→`session_token_invalid` on unknown/expired `secureViewSession.ts:81`–`:85`, missing→`session_token_required` `:72`–`:79`, `getSessionView(token)` resurrects `:124`–`:128`, handlers must NOT re-load by token `:12`–`:14`); recovery refused without a live session. A NEW domain-separated `ddrm-envelope` session-token primitive (`sign_session_token`/`verify_session_token` over `DKMS_SESSION_DOMAIN ‖ challenge ‖ expires_at`, single source of truth, separated from the hello attestation + the CEK seals) backs both sides. Counts: ddrm-envelope 18→20, dkms-authority 6→9, key-provider[key-authority-ref] 40→41. `ddrm-runtime-open` verify mode proves it cross-binary against the REAL node — step 13: identity pinned/verified + a session token minted; step 14: recover with NO/EXPIRED/FORGED/tampered token refused (even with a valid escrow+receipt); step 15: even WITH a live session the node refuses a DENIED/wrong-content receipt; step 16: ONE live session → THREE SUCCESSFUL recovers (sealed material only, raw CEK never present) — and the genuine open now flows through the persistent connection; the reference path stays green. Drift untouched (the node CONSUMES the existing `RightsDecisionReceiptV1`; the session token is a capsule-local protocol message). Gate: ladder INTACT (ddrm-envelope=20, dkms-authority=9, key-provider[key-authority-ref]=41), drift PASS, all dDRM smokes green (incl. dkms), clippy clean. Earlier tip Day-89–90 — the delegation is now an AUTHENTICATED CHANNEL with a per-recover AUTHORIZATION the node re-checks in its own boundary. `key-provider`'s `dkms` client PINS the node's published verifying key and, BEFORE delegating any recovery, runs an IDENTITY HANDSHAKE — it sends a fresh challenge, the node returns a signature over it under its master-derived key, and the client requires the node to advertise EXACTLY the pinned vk + a valid attestation (fail-closed on a forged/mismatched node, the runtime-core analogue of pinning the Lit network identity). It then threads the rights receipt + the content/principal/session/right binding INTO `recover`, and the `dkms-authority` node RE-AUTHORIZES in its OWN boundary — refusing unless the receipt is `allowed`, a protected-content action, and binds the SAME content/principal/session/right the recover declares (a buggy/compromised caller forwarding a denied/foreign/incoherent receipt is caught) — mirroring PC2's Lit action re-running `hasAccessByContentId` in the TEE (`universal-decrypt-chipotle.js:560`–`:568`) and rebinding `sha256(cek‖kid‖authority)` to refuse a swapped authority (`:577`–`:590`). A NEW domain-separated `ddrm-envelope` attestation primitive (`attest_challenge`/`verify_attestation` over `DKMS_HELLO_DOMAIN ‖ challenge`, single source of truth) backs both sides; the node gained a `hello` op. Counts: ddrm-envelope 16→18, dkms-authority 4→6, key-provider[key-authority-ref] 39→40. `ddrm-runtime-open` verify mode proves it cross-binary against the REAL node — step 13: the attestation verifies under the descriptor vk while a flipped vk + a replayed challenge are rejected; step 14: the node refuses a recover for a DENIED receipt + for a receipt bound to OTHER content — and the happy path decrypts with the master never on the wire; the reference path stays green. Drift untouched (the node CONSUMES the existing `RightsDecisionReceiptV1`). Gate: ladder INTACT (ddrm-envelope=18, dkms-authority=6, key-provider[key-authority-ref]=40), drift PASS, all dDRM smokes green (incl. dkms), clippy clean. Earlier tip Day-87–88 — the `dkms` authority SPLITS into a SECRET-HOLDING NODE + a PUBLIC-ONLY runtime, and recovery is DELEGATED across the process boundary (the first real step from "provisioned-descriptor seam" toward "remote dKMS"). A NEW `dkms-authority` capsule (the node) OWNS the master key material (its own node-local durable store) and exposes ONLY a `recover` op: it recovers the producer-escrowed CEK INSIDE its boundary (fail-closed on a forged producer / KID-swap / scheme-mismatch / tamper), re-seals it to the decrypt session, and returns the `SealedDecryptMaterialV1` — NEVER the CEK, NEVER the master (node tests=4). `key-provider`'s `dkms` backend now holds a PUBLIC-ONLY descriptor (schema `elastos.dkms.authority/v2`: `verifying_key_b64` + `recipient_pub_b64` + `authority_endpoint`, NO secret; a master-seed-bearing descriptor is REJECTED fail-closed) and on `release` DELEGATES recovery to the node (spawn + JSON-RPC the granted endpoint) instead of deriving the secret locally — so the runtime holds NO recovery secret and a leaked descriptor recovers nothing (+1 test, 38→39). `ddrm-runtime-open` PROVISIONS the node at publish (the master is generated + persisted in the node's OWN store; the runtime reads back only the public identity + writes a PUBLIC-ONLY descriptor + endpoint) and ASSERTS the descriptor handed to the runtime carries NO master seed — proving the master NEVER crosses into the runtime; `authority.dkms_authority_bin` is required for `dkms` (+1 bin test, 7→8). The dkms smoke decrypts the segment with the master seed never entering the runtime; the reference path stays green. Mirrors PC2's Lit/dKMS node recovering the CEK INSIDE the TEE (`universal-decrypt-chipotle.js:572`/`:602`/`:610`, returns only the sealed envelope) and the client holding only the PUBLIC `pkpId`/`authority` + RPCing the node (`recoverCEKEnvelope`, `chipotle-client.ts:1438`), never the recovery secret. Gate: ladder INTACT (+dkms-authority=4, +key-provider[key-authority-ref]=39), drift PASS, all dDRM smokes green (incl. dkms), clippy clean. Earlier tip Day-85–86 — the `dkms` EXTERNAL authority ran the open END-TO-END through the live rail, and a backend SWAP was invisible to the open. `ddrm-runtime-open`'s `OpenConfig` gained a typed `authority.backend` (`reference | dkms`); the publish phase PROVISIONS the selected authority (for `dkms`: generate the key material on a durable store, then publish an IMMUTABLE descriptor = master + published-identity pins, the dKMS-node analogue) and the open RESOLVES that backend — the SAME binary, a ONE-FIELD change, a byte-identical flow (`KeyLauncher` carries only a backend-specific `init_config`). `key-provider` now REQUIRES the dkms descriptor's pins (`verifying_key_b64` + `recipient_pub_b64`) — a pinless descriptor fails closed — and the bin PROVES the descriptor was READ-ONLY across the open. New sibling smoke `ddrm-consumer-dkms-smoke.sh` drives dkms end-to-end; the reference path stays green. Mirrors PC2's `getSessionView(token)` backend dispatch (`BackendSessionService.ts:368`–`:377`, downstream agnostic) + treating the provisioned descriptor as immutable published data (cached once + only read, `chipotle-client.ts:935`/`:950`). +1 key-provider test (37→38), +2 bin tests. Gate: ladder INTACT (+key-provider[key-authority-ref]=38), drift PASS, all dDRM smokes green (incl. the new dkms variant), clippy clean. Earlier tip Day-83–84 — the open now BOOTS FROM CONFIG with NO smoke in the loop, and the `dkms` EXTERNAL authority resolves a STABLE identity from a HANDED-IN descriptor. Two seams: (a) a NEW default-on runtime-core entrypoint `scripts/dev/ddrm-runtime-open` (a `bin`, relocated from `ddrm-consumer-smoke`) reads a TYPED JSON CONFIG (`OpenConfig`: provider binaries, work dir, viewer, content id, `mode`; fail-closed on a missing/unreadable/malformed config), constructs the trusted `DrmHost` from `ProviderLauncher`s + a `DurableEventStore` via `DrmHost::launch`, and drives publish → open → durable CEK-free persist; `ddrm-consumer-smoke.sh` shrinks to WRITING a config + INVOKING that binary (`mode:"verify"` adds the two adversarial fail-closed gates); +5 config-parse tests in the bin; (b) `key-provider` promotes `dkms` from `not_configured` to a FAIL-CLOSED EXTERNAL-authority seam — `init.config.dkms_authority_descriptor` (a path) RESOLVES the authority's stable ML-DSA signer + KEM recipient from a HANDED-IN descriptor (the dKMS-provisioned key material, READ never minted), VERIFIES it against the descriptor's published `verifying_key_b64`/`recipient_pub_b64` pins (fail-closed on mismatch), and recovers/re-seals through the SAME `SealedDecryptMaterialV1` contract; no descriptor → "no dKMS node provisioned"; +2 tests (35→37). Mirrors PC2 booting `sessionService` ONCE from config (`BackendSessionService.ts:495`) and resolving the external authority key from config (`resolvePkpId(config)`, `chipotle-client.ts:963`–`:967`), not minting. Gate: ladder INTACT (+key-provider[key-authority-ref]=37), drift PASS, 4 smokes green, clippy clean. Earlier tip Day-81–82 — the key authority now has a STABLE, DURABLE-KEY-STORE identity so the producer ESCROWS the CEK at PUBLISH time to a recipient any later launch re-derives identically (collapsing the Day-79/80 "launch → publish → escrow → bind" dance). Three seams: (a) `ddrm-envelope` DETERMINISTIC key derivation — `mint_session_from_seed(seed)` (ML-KEM-768 `generate_deterministic(d,z)` + x25519 from-seed via domain-separated SHA-256 sub-seeds, NO RNG), `derive_seed(master,label)`, `random_seed()`; +2 tests (14→16); (b) `key-provider` reference authority DURABLE KEY STORE — `init.config.authority_key_store` loads-or-creates + atomically persists (`*.tmp`→`rename`, 0600) ONE 32-byte master seed and re-derives BOTH the signer + the KEM recipient from it, so the published recipient is STABLE across processes (fail-closed on a corrupt store; the dev default still mints fresh per init); +2 tests (33→35); (c) `ddrm-plan-runner` `DrmHost::launch(plan_source, launchers, events)` — the trusted-core composition helper that brings up its OWN rail + wires the sink in one call; +2 tests (43→45). The runtime-core analogue of PC2's stable `DEFAULT_AUTHORITY` (baked into every video's PSSH at encode time, `dashPackager.ts:44`) vs the per-open `WasmSessionView` session key, and PC2 escrowing the CEK to that stable authority at encode time (`encryptMediaCEK(cek,kid) → authority: DEFAULT_AUTHORITY`, `dashPackager.ts:131`–`:140`). `ddrm-consumer-smoke.sh` now runs a PUBLISH phase (escrow to the stable recipient → durable fixture) then an OPEN phase via `DrmHost::launch` that RELAUNCHES the authority from the SAME store, PROVES the recipient is byte-identical across the relaunch, READS the fixture (never re-escrows), and binds only the per-open session AAD. Drift untouched. Earlier tip Day-79–80 — the trusted host now LAUNCHES THE RAIL + PERSISTS THROUGH A PRODUCTION-SHAPED STORE: `ddrm-plan-runner` gained (1) a `ProviderLauncher` seam + `RuntimeCapabilityTable::from_launchers` so the HOST brings the rail up by LAUNCHING each provider (spawn → init → publish material) in dependency order, fail-closed tearing down a partial rail (analogue of PC2's `BackendSessionService.createSession` launching a view via `WasmSessionView.createNew()`, `chipotle-client.ts:603`–`:613`/`BackendSessionService.ts:307`), and (2) a production-shaped `DurableEventStore` (atomic `*.tmp`→`rename`, stable layout keyed by `content_id/event`, idempotent, fail-closed, `load(dir)` read-back across a fresh instance skipping corrupt — mirroring `FileSessionStore` `BackendSessionService.ts:107`/`:140`–`:196`). +5 tests → ddrm-plan-runner 38→43; the consumer smoke now hands the host LAUNCHERS (capsule binaries) not pre-provisioned capsules + reads the durable records back through a fresh `DurableEventStore::load`. Earlier tip Day-77–78 — the trusted host OWNS THE RAIL + PERSISTS the open: `ddrm_plan_runner::DrmHost` gained (1) host-owned transport TEARDOWN (`ProviderTransport::shutdown` + `RuntimeCapabilityTable::shutdown` + `DrmHost::shutdown(self)` tear down every runtime-owned transport, fail-closed) and (2) a PERSISTING event sink (`EventStore` seam + `PersistingEventSink` write each runtime-event step as a durable CEK-FREE record via `open_event_record` — open identity + steps + decision + artifact NAMES, never VALUES). Audited PC2 first: the per-view transport OWNS a releasable resource + tears it down on `dispose()` (`WasmSessionView.dispose()` → `requestDrop` `chipotle-client.ts:694`–`:698`, contract `:231`, opened `:603`/`:621`); PC2 persists the open as a lifetime-managed session (`mediaSessionManager.create` `sessionManager.ts:50`–`:80`, TTL/`cleanup`/`destroy` `:104`–`:123`, singleton `:126`, CEK server-side + out of the record `:5`–`:18`). Mirrored: the host owns spawn→use→teardown of each transport + persists a CEK-free record per runtime event. +4 tests → ddrm-plan-runner 34→38. `ddrm-consumer-smoke.sh` shrinks further — the transports now OWN their capsules and `host.shutdown()` tears down the WHOLE rail (no manual per-capsule shutdown), and the sink is `PersistingEventSink` over a `FileEventStore` writing durable CEK-free records to a temp dir, which the smoke reads back to prove the receipt + audit persisted without leaking the CEK/ciphertext/keys. Drift untouched. Earlier tip Day-75–76 — the runtime CORE now has a single TRUSTED HOST ENTRYPOINT that owns the WHOLE open: a new `ddrm_plan_runner::DrmHost` owns (1) a `PlanSource` (the seam to ask `drm-provider` for the plan), (2) the Day-74 `RuntimeCapabilityTable` of registered `ProviderTransport`s, and (3) a `RuntimeEventSink` for the plan's runtime-OWNED post-steps; `host.open(content_id, viewer)` FETCHES the plan, drives it through the registry (`open_drm_plan`'s parse→resolve→execute), then EMITS the plan's runtime-event steps (`release_receipt` + the open `audit`) in order. Audited PC2's server-owned composition first: the Express `/init` route is the ONE place that owns the whole open — `router.post('/init', authenticate, requireSecureViewSession, handler)` (`src/api/media.ts:133`); once the middleware resolves the capability into request state, the handler owns fetching+parsing the MPD (plan-equivalent), reading the resolved handle (`:481`) and driving recovery (`:482` `recoverMediaCEK`), CREATING the session that lives for the duration (`:489` `mediaSessionManager.create`), and logging the open (`:483`/`:518`), fail-closed in one place (`catch`→500 `:528`). Mirrored: `DrmHost` is the runtime-core analogue of that route — plan-fetch + drive-over-registry + runtime-event emission, owned in one entrypoint, fail-closed at every seam (a bad plan never resolves a capability; a missing transport fails closed; a declared runtime event the sink cannot emit fails the open). New `PlanStep.event` + `is_runtime_event()` lets the host emit the runtime-owned post-steps the executor only walks for ordering. +5 tests (host opens via plan source + registry + emits both events in order; tampered plan FROM THE SOURCE fails closed with no event; sink refusing the audit fails the open; unregistered required transport fails closed; runtime-event steps parse) → ddrm-plan-runner 29→34. `ddrm-consumer-smoke.sh` is now a THIN caller — it REGISTERS the three transports + wires a `SmokePlanSource` (the real `drm-provider`) and a `SmokeEventSink` into a `DrmHost` and calls `host.open(content_id, viewer)` (the SAME entrypoint the trusted core will call; the binaries are the host's registered transports + plan source); the runtime now emits the release_receipt + audit post-steps, and the tampered-edge gate flips the plan source into tamper mode and re-opens through the SAME host. Drift untouched. Earlier tip Day-74 — the runtime CORE now OWNS the capabilities the composition root resolves: a new `RuntimeCapabilityTable` (in `ddrm-plan-runner`) is a registry of runtime-owned `ProviderTransport`s — the runtime `register`s one transport per provider (at startup) and `open_drm_plan` → `resolve(provider)` OPENS a fresh handle over the registered transport, or `None` for an unregistered provider (→ fail closed). Audited PC2's transport ownership first: the runtime owns the factory as a process-lifetime singleton (`export const sessionService = new BackendSessionService(new FileSessionStore(...))`, `BackendSessionService.ts:495`, ctor `:266`); `getSessionView(token)` dispatches on `stored.backend` (`:371`) to construct the per-backend transport it owns the means to build (`:374`/`:377`), `null` for an unknown token/backend (`:370`). Mirrored: `ProviderTransport` (runtime-owned, registered once) vs `ProviderHandle` (fresh per-open, the analogue of a `BackendSessionView` minted per request); `register` rejects a duplicate provider, `resolve` opens over the registered transport or `None`. +4 tests → ddrm-plan-runner 25→29. `ddrm-consumer-smoke.sh` now REGISTERS three runtime-owned transports (`Rights`/`Key`/`Decrypt`Transport, each wrapping one real capsule) into the lib's `RuntimeCapabilityTable` (the SAME registry the core uses) and drives both the canonical open and tampered-edge re-run through `open_drm_plan` — no second code path. Drift untouched. Earlier tip Day-73 — the runtime CORE now has a single COMPOSITION ROOT for a dDRM open: `ddrm_plan_runner::open_drm_plan(plan, &mut capability_table)` parses the plan, RESOLVES each provider the plan requires from a runtime-supplied `CapabilityTable` at ONE point, builds the `RuntimeStepRunner`, and executes — the one entrypoint the trusted runtime calls. Audited PC2's composition root first: the secure-view middleware resolves the per-stage handle ONCE from a backend-keyed factory (`sessionService.getSessionView(token)` on `stored.backend`, `src/services/session/BackendSessionService.ts:368`) and attaches it to request state (`secureViewSession.ts:124`→`:129`); the handler reads it from that state and never re-resolves (`media.ts:481`→`:482`, helper takes `session` as a param `:1192`; doc forbids handlers re-loading by token `secureViewSession.ts:13`). Mirrored: a new `CapabilityTable` trait (the runtime-core analogue of the session factory); `RuntimeStepRunner::resolve_from` calls `table.resolve` once per required provider and fails closed if the table holds no capability for a required provider or returns a handle for the wrong provider; `open_drm_plan` ties parse→resolve→execute into the SINGLE entrypoint (parsing BEFORE touching the table, so a bad plan never reaches the runtime's capabilities). +4 tests (drives via the table resolving each required provider once in order; withheld required provider fails closed with zero invocations; misrouting table rejected; non-planned plan refused before any resolve) → ddrm-plan-runner 21→25. `ddrm-consumer-smoke.sh` no longer hand-builds the runner — it supplies a `SmokeCapabilityTable` and calls `open_drm_plan` for BOTH the canonical open and the tampered-edge re-run (same entrypoint, no second code path). Drift untouched. Earlier tip Day-72 — the runtime CORE now INJECTS per-provider capability handles into the Day-71 executor: a new `RuntimeStepRunner` (in `capsules/ddrm-plan-runner`) IMPLEMENTS the Day-71 `StepRunner` seam over a set of injected `ProviderHandle`s — one per provider the plan's `next_required_providers` names — routing each plan step to the handle registered for that step's `provider` while holding NO authority itself. Audited PC2's per-stage injected handle first: the secure-view middleware RESURRECTS a `BackendSessionView` once per request (`src/api/middleware/secureViewSession.ts:124`) and THREADS that handle into the downstream stage (`src/api/media.ts:1207` hands `session` into `recoverMediaCEK`→`recoverCEKEnvelope`; `/segment` reuses the SAME injected view, `:541`) — a stage never opens its own connection, it uses the handle it was given. Fail-closed construction (`RuntimeStepRunner::new`): every provider the plan names (normalized `key-provider`→`key`) MUST have an injected handle (no ambient default — the core cannot fabricate a missing capability) and a STRAY handle for a provider the plan does not name is REJECTED, so the `blocked_authority` set is structurally unreachable from the runner type. +7 characterization tests (drives the plan through injected handles in canonical order; refuses to build without a required handle; rejects a stray unnamed handle; rejects duplicate handles; never invokes a handle for an unnamed provider; parses+normalizes `next_required_providers`) → ddrm-plan-runner 14→21. `ddrm-consumer-smoke.sh`'s monolithic `SmokeRunner` is GONE — replaced by three per-provider handles (`RightsHandle`/`KeyHandle`/`DecryptHandle`, each wrapping ONE real capsule binary) injected into the SAME `RuntimeStepRunner` the trusted core will use with real providers (no second code path); both fail-closed gates ride along unchanged. Drift untouched (the runner consumes the plan, defines no shared contract). Earlier tip Day-71 — the runtime CORE now EXECUTES the open plan: a new fail-closed `capsules/ddrm-plan-runner` library walks the `DrmOpenPlanV1` `drm-provider` emits instead of the smoke hand-walking it inline. Audited PC2's open sequencer first — each stage gated on the prior: `requireSecureViewSession` resurrects the session view (`src/api/middleware/secureViewSession.ts:61`), then `recoverMediaCEK` → `recoverCEKEnvelope` whose access gate is `hasAccessByContentId(ownerAddress, kid)` (`src/api/media.ts:1163`) and which only THEN recovers + unwraps the CEK in-boundary (`:1196`, `:1216`). The executor `DrmOpenPlan::parse` validates schema/`planned`-status/the `rights_check<key_release<decrypt_session` canonical order/every binding edge names real steps + identities; `execute` seeds the virtual `drm_open` identities (`content_id`/`object_cid`/`viewer_interface`), walks the steps IN ORDER, threads each binding edge's artifact into the next step's plan-declared field, and FAILS CLOSED when a step needs an artifact not yet produced (out-of-order / silent prior failure) or runs without emitting the artifact the plan says it produces. It holds NO authority: the ONLY thing that touches a provider is the injected `StepRunner` (the runtime's capability seam) — the CEK/wallet/chain never appear in the crate. 14 characterization tests (canonical drive + order/threading/seeding + renamed-edge / dropped-artifact / backward-edge / out-of-order / wrong-schema / identity-split / no-authority fail-closed). `ddrm-consumer-smoke.sh` no longer hand-walks the chain: it fetches the REAL `drm open` plan, parses it through the core, and drives drm→rights→key→decrypt THROUGH the executor (the smoke is just the injected `SmokeRunner` transport), with a NEW cross-binary fail-closed gate — a TAMPERED binding edge is rejected by the real `key-provider` (`deny_unknown_fields` over a required `rights_receipt`). New ladder rung ddrm-plan-runner=14; drift untouched (the executor reads the plan, defines no shared contract). Earlier tip Day-70 — the canonical key release is REAL: `key-provider::release` (the op `drm-provider`'s `DrmOpenPlanV1` names for the key step, a `not_configured` stub for the reference backend until now) actually releases. Audited PC2's Lit authority first (`universal-decrypt-chipotle.js`: access-check `:560–568` → recover `Lit.Actions.Decrypt` `:570–575` → CEK↔KID↔authority bind `:577–590` → seal-to-session `envelopeCEK` `:602–608`). `release` validates the rights receipt then — for the reference backend — RECOVERS the producer-escrowed CEK from the rights-bound `key_envelope.wrapped_cek` (recomputing the shared `escrow_aad`, verifying the producer vk) and re-seals it to the runtime-injected decrypt session as `SealedDecryptMaterialV1`. Session material rides in a capsule-local `session` context (shared `KeyReleaseRequestV1` byte-identical, drift untouched). Fail-closed: no backend/no session → `not_configured`; denied/expired/kid-swap/scheme-mismatch/forged-producer → refused; CEK only `Zeroizing`, leaves only SEALED. key-provider 27→33 (key-authority-ref). `ddrm-consumer-smoke.sh` now escrows the golden CEK + drives the CANONICAL `release` (recover→reseal) instead of the raw-CEK `release_ref` shim — the consumer half runs drm→rights→key→decrypt with no raw CEK handed in. Earlier tip Day-69 — the production seal path is CLOSED: `encrypt-provider::seal` (non-inline op, fail-closed since Day 1) now runs the FULL pipeline on HANDED-IN asset bytes and emits a complete shared-contract `SealedObjectV1`. Audited PC2's producer input first (`dashPackager.ts`): the host reads each segment off disk (`readFileSync` `:504`) and HANDS the bytes to the CENC WASM (`executeCENCEncrypt(.., seg.data)` `:432`) — the encoder fetches nothing. Mirrored: `seal` gained `content_b64`/`recipient_pub_b64`/`availability_receipt_cid` (optional, `deny_unknown_fields` preserved); given bytes + recipient it runs the ONE shared `run_seal_pipeline` (mint→CENC→content-address→escrow; `seal_inline` now delegates to it too, PRINCIPLES #10) and assembles a `SealedObjectV1` with the real Day-68 `payload_cid`, `key_envelope.kid` == bytes16 contentId, `policy_hash = sha256(rights_policy_cid)`, and the PQ-hybrid suite the chain validates. NO fetch/IPFS/network authority. Fail-closed: no recipient/bytes → `not_configured`; missing receipt / empty viewer-interface / empty content → `invalid_request` (encrypt escrow 22→25). `ddrm-producer-smoke.sh` drives the REAL `seal`, deserializes into the SHARED `SealedObjectV1` and runs the SAME `validate_protected_content_key_envelope_algorithms` the `key-provider` runs — cross-binary proof the chain accepts it; no plaintext on the wire. Earlier tip Day-68 — the producer stopped FAKING storage: `encrypt-provider` content-addresses the sealed ciphertext IN-BOUNDARY (`payload_cid = CIDv1(raw, sha2-256)` of the segment, byte-for-byte what PC2's Helia `unixfs.addBytes` produces for single-chunk content; pure, no `kubo_api`/network; fail-closed above 1 MiB). `seal_inline` emits a real `payload_cid` (not the smoke's `bafybeig…` placeholder); golden pins three inputs to the EXACT CIDs PC2's real `ipfs-unixfs-importer` emits (encrypt-provider 17→20 / 19→22). `ddrm-producer-smoke.sh` independently recomputes the segment's CID via the canonical `cid` crate and demands a byte-for-byte match cross-binary. payload_cid (IPFS address) stays SEPARATE from the KID/contentId (chain key). Earlier tip Day-67 — the crown-jewel ORCHESTRATOR is real: `drm-provider::open` now emits a typed, executable **`DrmOpenPlanV1`** (status `planned`, never `opened`) — the single canonical `drm/open` sequence + its inter-step **binding edges** (rights ⇒ `RightsDecisionReceiptV1` → `key.rights_receipt`; key ⇒ `ReleaseReceiptV1` → `decrypt.release_receipt`; one content identity == KID under both `content_id`/`object_cid`) — holding ZERO authority (it PLANS, the runtime EXECUTES), capsule-local like `publish-provider::UnsignedMintV1` so the frozen contract + drift gate stay untouched (drm-provider=15). `ddrm-consumer-smoke.sh` now drives the REAL `drm open` and FOLLOWS the plan (order + binding edges + content identity) instead of a hardcoded sequence (PRINCIPLES #10). Mirrors PC2's `recoverCEKEnvelope` + Lit-action open ordering. Earlier tip Day-66 Phase C — the chain's OWN log reconstructs the same listing: `content-market::listing_from_event` decodes a PC2 `DigitalAssetRegistered` log (on-chain `bytes16 contentId` → SAME identity as the calldata path) or `AssetCreated` (no contentId → `metadata_status:"needs_kid"`, deferred to the `enrich_listing` kid-match) into a `ContentListingV1`; pure decode (log handed in by `chain-provider`, no RPC). `ddrm-market-smoke.sh` asserts the event path agrees with the calldata path cross-binary (content-market=29). Earlier tip Day-65 Phase C — the listing now carries HUMAN-FACING fields, fail-closed: `content-market::enrich_listing` fuses a resolved `metadata.json` (name/poster/mime/contentCID/asset-class) onto the calldata-derived identity but REJECTS any metadata whose `kid != content_id` (`identity_mismatch`) — metadata describes, never re-identifies; a hardening over PC2 which trusts `metadata.kid`. Still fetches nothing (the JSON is handed in by `ipfs-provider`); `ddrm-market-smoke.sh` drives `publish → chain → reconstruct → enrich` so a matching kid resolves and a tampered kid is rejected cross-binary (content-market=22). Earlier tip Day-64 Phase C — the published mint is now DISCOVERABLE: a new fail-closed `content-market` capsule reconstructs a typed `ContentListingV1` PURELY from the self-describing mint calldata (inverse of Day-62 `assemble_mint`) — `content_id == bytes16 KID`, tokenURI→metadataCID, opType, `(copies,price,payToken)` — holding no chain RPC/IPFS/keys and minting nothing (content-market=13). `ddrm-market-smoke.sh` drives the REAL `publish → chain → content-market` so a sealed asset's KID flows producer→chain→discovery as ONE identity. Runtime-superior vs PC2's 4-source `ContentIndexerService` — our mint is self-describing, so one pure decode yields a verifiable listing; human-facing enrichment is delegated. Earlier tip Day-63 Phase C — the producer→chain loop is CLOSED cross-binary: `publish-provider`'s `UnsignedMintV1` now emits STRUCTURED `op_raw`/`sell` (PC2-faithful payee arrays: creator ACCESS_TOKEN + ROYALTY_SHARE `amount=round(10*royalty)`, default royalty `100−ELACITY_ROYALTY_PERCENT`, BUY_AND_RESELL DISTRIBUTION_RIGHT + `resellerCut`) that drop STRAIGHT into `chain-provider::assemble_mint` with no shape translation; `scripts/ddrm-publish-smoke.sh` drives the REAL `publish (prepare) → chain (assemble_mint)` binaries so one identity flows KID → contentId → mint calldata, tokenURI + sell terms intact, no signing/RPC in the assembler (publish=16). Earlier tip Day-62 Phase C — the prepared mint is now real CALLDATA: `chain-provider` gained a pure `assemble_mint` op that ABI-encodes the PC2 `mint(string,uint16,bytes,bytes)` call (FREE `opRawData=abi.encode(bytes16)`, PAID payee/royalty tuple + `sellRawData=(copies,price,token)`) and returns the `{to,data,value}` the existing `prepare_transaction` → wallet sign → `broadcast_transaction` seam executes — no RPC/keys in the encoder, calldata decoded back to spec in 10 deterministic tests. Day 61 added the fail-closed `publish-provider` that ASSEMBLES the mint intent (binds `contentId == bytes16 KID`, derives `tokenURI = {metadataCid}/metadata.json`, emits the unsigned `UnsignedMintV1`; publish=13). Day 60 took the producer half cross-binary: `encrypt-provider` (feature `escrow`) `seal_inline` mints a CEK *now* + emits the SEALED escrow blob; `key-provider` (`release_from_escrow_ref`) recovers + re-seals it; `scripts/ddrm-producer-smoke.sh` drives `encrypt → key → decrypt` so a video sealed *now* decrypts *now*, no raw CEK/plaintext on any wire (key-authority-ref=27, encrypt escrow=19). Defaults fail-closed; consumer half still runs via `scripts/ddrm-consumer-smoke.sh`; ~67 commits). **0.4.0 released (tag `v0.4.0`); contract byte-identical, crypto core verified green on the released base; rebase surface measured in `PUSH_PLAN.md`. Anders confirmed the rail (Day 45) and the decrypt boundary now implements his ENTIRE decrypt-side spec, consolidated into the suite-tagged `SealedDecryptMaterialV1` drop-in: Option A push-in (`rail-live`), full-transcript binding (`rail-bind`), in-sandbox key mint+publish (`rail-mint`), short-expiry + scoped CEK-free audit (`rail-audit`), consolidated envelope (`rail-material`). Decrypt boundary is COMPLETE; remaining work is upstream only (contract merge needs push; dKMS sealing needs Anders).**
**Repo:** `/Users/sash/code/elastos-runtime` (this repo).
**PC2 reference repo (stable source of truth):** `/Users/sash/Documents/Cursor/pc2.net/pc2-node`.

---

## 0. The 30-second picture

We are re-platforming the Elacity web product (**PC2 / pc2.net**) onto a
**capability-secure Rust runtime** (this repo, ElastOS). The crown jewel is
**dDRM** — decentralized DRM — a fail-closed provider chain that lets an app *see*
protected content while never letting it *hold* the keys.

Over Days 1–17 we brought the **entire dDRM provider chain to a proven,
fail-closed, wasm-built, contract-tested bar**, pinned **both** security invariants
(encrypt + decrypt), proved the **post-quantum crypto compiles in wasm**, and made
the work **rebase-safe** against Anders' in-flight 0.4.0. Days 18–24 then advanced
every *unblocked* edge of the rail right up to the transport decision: closed the
encrypt in-boundary-keygen gap, de-risked the PQ-hybrid envelope, **proved the full
PQ dDRM data path end-to-end pre-rail**, locked the engines with **portable golden
vectors**, and made the "byte-compatible with PC2" claim **executable** via a
standing cross-impl conformance gate (`ddrm-verify.sh`). Days 25–28 closed the gap to
the rail itself: an **encrypt→decrypt round-trip golden** (both invariants on one
artifact), the **rail transport shim behind a flag** (`rail-shim`) so the rail is now
a *flag-flip not a design*, and the shim's **carrier wire shape pinned as a portable
golden** that is also driven through **PC2's real session API** (`unwrap_envelope` →
`media::decrypt_segment`). Days 29–34 then made the **crypto core feature-complete,
PQ-proven, and adversarially hardened**: widened the cenc goldens to **real playback
shapes** (multi-sample / subsample / non-default-IV via init-`tenc`, all PC2-conformant);
**replaced the PQ signature stub with the real FIPS 204 `ml-dsa-65` primitive** (RustCrypto
`ml-dsa`, verify-only, `wasm32-wasip1`-clean) and proved it **through the exact
`decrypt_from_carrier` rail entrypoint** on a committed real-signed carrier golden; and
added an **adversarial negative-space + containment sweep** (`harden`) proving the
untrusted-input decoders fail closed and never panic. The verification gate
(`ddrm-verify.sh`) is now **authoritative over the whole ladder** (asserted test counts +
wasm builds). Everything is isolated on local branches because **GitHub push access is
suspended** (see §6).

The chain is **blocked on exactly one architectural decision from Anders** (the CEK
transport rail — `DDRM_DECRYPT_RAIL.md`). Everything that depended on it has been
pinned, de-risked, or proven pre-rail — *including the transport shim itself and the real
PQ signature primitive*, both built and fully tested. The remaining PQ items are now pure
**policy** (Anders' Q2: straight `ml-dsa-65` vs hybrid during PC2's migration), not build
gaps. **As of Day 45 the `OpenSession` wire-up itself is also done** — the recommended
rail (Option A) is wired into the provider dispatch behind `rail-live` as a fail-closed
reference: `OpenSessionLive` runs `rail_shim::decrypt_from_carrier(...)` with a real
`MlDsa65Verifier` and returns a scoped response (proven: real PQ carrier decrypts through
dispatch with no CEK/plaintext leak; tampered/unprovisioned fail closed; wasm-clean). The
shared contract is deliberately **untouched** (material rides a capsule-local variant), so
the only thing left to flip live decrypt on by default is **Anders' thumbs-up on the
additive `DecryptSessionRequestV1` field** (exact delta in `DDRM_DECRYPT_RAIL.md`).
Everything that *can* be done ahead of that answer, *is* done.

---

## 1. Mission & priority stack

From `CONVERGENCE_PLAYBOOK.md` (the north star — read it second):

1. **dDRM is the crown jewel.** Protected-content economy is the product's reason to
   exist. Everything else serves it.
2. **Capability security is non-negotiable.** Small trusted Rust core; everything
   else is an isolated capsule/provider with zero ambient authority; fail-closed.
3. **Contract-first convergence.** PC2 is the stable behavioural reference; we
   translate its *patterns* into the capability model, we do not copy its trust
   assumptions. Pin contracts with characterization tests before wiring engines.
4. **One boundary at a time. Isolated, reversible, reviewable.**

---

## 2. The mental model (how the system is shaped)

**ElastOS is a capability OS.** Isolation tiers, highest authority first:

| Tier | Tech | Isolation | Examples |
|---|---|---|---|
| Trusted core | Rust, native host | the runtime process | the Runtime itself |
| **Providers** | Rust, `type: microvm` | full VM (crosvm/Linux, Apple VZ/macOS) | decrypt, key, rights, drm, encrypt, wallet, ai… |
| Shells / system logic | Rust → `wasm32-wasip1`, `type: wasm` | wasmtime sandbox | Home, System |
| App / content / UI | Web (HTML/JS), `type: data` | runtime-mediated browser principal | Library, Marketplace |

**The dDRM chain** (the spine of the crown jewel):

```
app/viewer --drm/open--> drm-provider --> rights-provider --> key-provider --> decrypt-provider --scoped output--> player
                                          (RightsDecisionReceipt) (ReleaseReceipt + sealed CEK)        (NO CEK ever)
```

- Authority is passed between stages as **signed receipts**, never as keys.
- The **CEK** (content key) is the only true secret. It travels **sealed**, is
  unwrapped/used/zeroized **inside one boundary** (decrypt-provider), and **never**
  reaches the player.
- Two **viewer/player** kinds consume scoped output: **media** (video/audio → fMP4
  segments) and **non-media** (pdf/epub/cbz/images → rendered/plaintext). Both get
  an opaque handle, never the CEK. (Ross built these in PC2.)

**Irzhy's two security invariants (binding):**
- **#1 (encrypt):** CEK+KID generated **inside** a wasm boundary; only ciphertext +
  non-secret relatives output.
- **#2 (decrypt):** CEK **never** passed as plaintext to other components; recovery
  + decryption colocated in one boundary + zeroize at end.

---

## 3. What's built (current truth)

The four-stage chain **plus** the encrypt producer, all fail-closed and wasm-built:

| Provider | Role | Host tests | wasm | Notes |
|---|---|---|---|---|
| `capsules/encrypt-provider` | seal/produce (invariant #1) | 13 | builds | **in-boundary CEK+KID keygen closed** (Day 19); output reconciled to shared `SealedObjectV1` (Day 39) |
| `capsules/drm-provider` | orchestrator `drm/open` + chain-seam | 12 | builds | declares canonical open sequence |
| `capsules/rights-provider` | rights decision | 9 | builds | wire-rejects hidden authority |
| `capsules/key-provider` | key release (rights-bound) | 9 | builds | verifies upstream RightsDecisionReceipt |
| `capsules/decrypt-provider` | decrypt/render (invariant #2) | 25 | builds | cenc engine + envelope spec + consumer contract |

**68 host tests green; 0 ignored** (Day 19 closed the encrypt keygen gap: 6+1-ignored → 13).

The `decrypt-provider` also carries **feature-gated tested islands** (Parallel
Change — off by default, so the base surface above is unchanged). Cumulative test
counts per feature:

| `cargo test --features …` | count | what it adds |
|---|---|---|
| *(default)* | 25 | the shipped decrypt contract |
| `rail-prep` | 27 | classical `ecdh_unwrap → cenc` composition (Day 18) |
| `pq-envelope` | 29 | PQ-hybrid CEK-seal envelope island (Day 20) |
| `pq-rail-prep` | 31 | full PQ data path `hybrid_unwrap → cenc` (Day 21) |
| `vectors` | 42 | replay portable goldens: v3+v2, encrypt↔decrypt round-trips (single + **multi-sample + subsample**), multi-sample/subsample/init-IV cenc (Days 22, 24, 26, 31, 37) |
| `rail-shim` | 45 | carrier→engine adapter (`decrypt_from_carrier`) + carrier goldens, both profiles (Days 27–30) |
| `pq-mldsa` | 34 | real FIPS 204 ML-DSA-65 verifier in the `CekSealVerifier` slot + KAT (Day 32) |
| `pq-mldsa-hybrid` | 37 | hybrid ECDSA-P256 + ML-DSA-65 verifier (BOTH must verify) — the other Q2 answer (Day 41) |
| `rail-shim-mldsa` | 54 | the real ML-DSA-65 verified through `decrypt_from_carrier` on a committed carrier golden (Day 33) |
| `harden` | 65 | adversarial negative-space + containment sweep over the wire-decoders (Day 34) |
| `rail-live` | 57 | **recommended rail (Option A) WIRED into dispatch** — `OpenSessionLive` runs `decrypt_from_carrier` in-boundary, real PQ carrier decrypts through dispatch with no CEK/plaintext leak; tampered/unprovisioned fail closed (Day 45) |
| `rail-bind` | 60 | **sealed CEK binds the full decrypt transcript** (Anders Day-45 ask) — `DecryptTranscriptV1` as AES-256-GCM AAD + ML-DSA-65 signature; `OpenSessionBound` rebuilds it from the authenticated request; replay against a different session / swapped nonce / tampered carrier all fail closed (Day 46) |
| `rail-mint` | 62 | **in-sandbox session-key mint + publish** (Anders Day-45 ask) — `init` mints the per-session hybrid KEM keypair (OsRng→WASI `random_get`), holds the secret in-VM, publishes the pubkey + suite; faithful flow proven (authority seals to the published key → minted secret opens it), fresh key per init (Day 47) |
| `rail-audit` | 62 | **short-expiry enforcement + scoped audit** (Anders Day-45 ask) — `OpenSessionAudited` rejects a stale grant (`now_unix` past request/receipt expiry) BEFORE any unwrap (`expired`), and emits a CEK/plaintext-free audit record bound to the transcript hash on every decision (opened\|denied); clock is an injected capability (Day 48) |
| `rail-material` | 65 | **consolidated suite-tagged `SealedDecryptMaterialV1`** (drop-in contract shape) — canonical `OpenSessionV1` routes by `suite` (dKMS-native vs Lit-compat is a field, not a fork) into the audited bound path; compat suite rejected on the product path, unknown suite fails closed (Day 49) |
| `gen-vectors` | — | regenerate the committed vectors (writes `tests/vectors/`) |

The standing gate `scripts/ddrm-verify.sh` now asserts **all** of these counts +
the wasm builds (gate 3, `ddrm-ladder-check.sh`), so a dropped/feature-gated-out
test fails the gate rather than passing silently.

Proven properties (all test-backed — see `DDRM_SECURITY_MODEL.md` §9):
- Zero ambient authority surfaced; every provider advertises + wire-rejects raw
  authority (`deny_unknown_fields`).
- Fail-closed by default (`not_configured` until a real backend exists).
- **CEK containment + zeroization** at both ends.
- **Invariant #1 closed:** `encrypt-provider` mints CEK+KID with a CSPRNG **inside**
  the boundary and the seal engine emits no key material (Day 19).
- **Authorization binding** (rights receipt → key release).
- **Contracts compose** (cross-provider seam tests).
- **Upstream rail contract** captured as an executable spec
  (`decrypt-provider/src/envelope.rs`: P-256 ECDH unwrap → AES-256-CBC, vendored
  from PC2 `ddrm-decrypt`).
- **Downstream consumer contract** pinned for both players (metadata-only output).
- **PQ-hybrid is real, not stubbed, and wasm-clean** (Day 32): the signature is the
  **real FIPS 204 ML-DSA-65** (`ml-dsa 0.1`, RustCrypto — same family as `ml-kem 0.2.3`),
  verify-only + rng-free so it builds to `wasm32-wasip1`. `pq_envelope::mldsa::MlDsa65Verifier`
  fills the `CekSealVerifier` slot; pinned by a committed deterministic KAT
  (`mldsa65_kat.json`) + fail-closed tests. The PQ signature is no longer a build gap —
  only Anders' Q2 transition *policy* remains.
- **Full PQ dDRM data path proven pre-rail** (Day 21): `pq_envelope.rs`
  `decrypt_pq_sealed_segment` chains `x25519+ml-kem-768` hybrid unwrap → cenc
  decrypt, CEK in `Zeroizing` throughout, never on the boundary.
- **Engines pinned by portable golden vectors** (Days 22, 24): substrate-independent
  fixtures in `decrypt-provider/tests/vectors/` (classical v3 + v2, and PQ-hybrid)
  replayed with no in-test sealing and no RNG.
- **Cross-impl conformance is executable** (Days 23–24, 28, 31, 38): `scripts/pc2-conformance.sh`
  decrypts our committed vectors with PC2 `ddrm-decrypt`'s **real code** and asserts
  byte-for-byte parity (CEK + plaintext) plus fail-closed parity on tamper, for both
  envelope versions — at **two layers**: the crypto primitives (`envelope`+`cenc`)
  and PC2's **public session API** (`session::unwrap_envelope` → `media::decrypt_segment`,
  the carrier path), over single/multi-sample/subsample/init-IV shapes. **And the
  producer half** (Day 38): the segments `encrypt-provider`'s real engine emitted
  (multi-sample + subsample) are decrypted by PC2's `mp4box`+`cenc` to the producer's
  exact bytes (+ wrong-CEK key-bound check) — proving PC2 consumes *our producer's
  output*, not only our consumer. Skips clean when PC2 is absent.
- **Both invariants pinned on one artifact, over real playback shapes** (Days 26, 37):
  `encrypt-provider`'s real in-boundary engine emits round-trip goldens —
  `roundtrip_encrypt_to_decrypt.json` (single sample) plus
  `roundtrip_multisample_encrypt_to_decrypt.json` (4 samples, per-sample IVs) and
  `roundtrip_subsample_encrypt_to_decrypt.json` (16-byte clear leader + encrypted
  body) — which `decrypt-provider` replays back to the producer's exact plaintext
  (`vectors`), CEK contained. Producer mux mirrors PC2 `cenc-encrypt::mp4box`
  (`build_senc` / `build_senc_with_subsamples`); the gate exercises all three by name.
- **The rail is a flag-flip, not a design** (Days 27–28): `decrypt-provider/src/rail_shim.rs`
  (`rail-shim`, default OFF, **not** wired into dispatch) is the carrier→engine adapter
  for rail Option A — `decrypt_from_carrier(session, carrier, verifier)` routes a sealed
  CEK + segment to the proven classical/PQ engines. Its carrier wire shape is pinned by
  a portable golden (`rail_carrier_classical.json`) and validated against PC2's session
  model. Q1 (who seals) doesn't touch it; Q2 (signature) plugs in via `CekSealVerifier`.
  The day Anders answers, `OpenSession` adds **one line**. (`DDRM_DECRYPT_RAIL.md` §"Rail
  transport shim".)
- **Media goldens widened to real playback shapes** (Day 31): multi-sample, subsample
  (clear+encrypted ranges), and a 16-byte-IV-via-init-`tenc` vector — each replayed
  through our engine **and** PC2's real `cenc`/`media::decrypt_segment` (byte parity +
  tamper fail-closed). `ClassicalVector` gained optional `init_segment_b64`/`iv_size`.
- **Real ML-DSA-65 verified through the rail entrypoint** (Day 33): a committed carrier
  golden (`rail_carrier_pq_mldsa.json`) whose seal signature is a genuine ML-DSA-65 sig,
  replayed through `decrypt_from_carrier` verified by the production `MlDsa65Verifier`
  (`rail-shim-mldsa`) — plaintext recovered + fail-closed on tampered sig / wrong key /
  tampered body. *The real PQ signature, through the real rail entrypoint, on a portable artifact.*
- **Both Q2 signature answers pre-proven** (Day 41, `pq-mldsa-hybrid`): a hybrid
  ECDSA-P256 + ML-DSA-65 `HybridVerifier` (BOTH halves must verify) drives the same
  `hybrid_unwrap` path — happy path, both-halves-required, tampered, and malformed framing
  all proven; `wasm32-wasip1`-clean. Q2 is now a pure policy pick, not a build task.
- **Fail-closed + panic-free under adversarial input** (Day 34, `harden`): truncation,
  single-byte-flip, and oversized-length-prefix sweeps over `envelope::parse`,
  `PqSealedEnvelope::from_bytes`, and the `decrypt_from_carrier` dispatch — every malformed
  shape fails closed, never panics, never recovers a CEK; error/metadata surfaces leak no
  plaintext/CEK; profile/secret mismatch fails closed both directions.

---

## 4. Document map — what to read, in order

All in `docs/convergence/`. Read 1→3 to onboard; the rest are reference.

1. **`HANDOVER.md`** (this file) — start here.
2. **`CONVERGENCE_PLAYBOOK.md`** — north star: mission, priority stack, decision
   rules, capability model, convergence laws, migration patterns, the 10/10 bar.
3. **`DDRM_STATUS.md`** — current truth: parity table, proven properties, commit
   inventory, the open rail decision, base-reconciliation status. **Refresh this as
   you work.**
4. **`DDRM_SECURITY_MODEL.md`** — the trust model: actor/boundary map, encrypt +
   decrypt mermaid flows, threat model, PQ crypto profile, invariant→test table.
5. **`DDRM_DECRYPT_RAIL.md`** — the one open architecture decision (how the CEK
   reaches decrypt) with options, recommendation, and the sharpened questions for
   Anders. **This is the blocker.**
6. **`DDRM_ENCRYPT_INVARIANT.md`** — encrypt side (invariant #1): the PC2
   host-keygen gap, the target contract, the scoped landing.
7. **`PC2_PLAYER_ALIGNMENT.md`** — media vs non-media players mapped to tiers;
   Irzhy's invariants validated; the ECDH envelope as the concrete rail evidence.
8. **`PRODUCT_VISION.md`** — the PRD: what ElastOS is, personas, pillars, roadmap.
9. **`PUSH_PLAN.md`** — how to land the local branches as PRs when GitHub returns,
   **including the rebase recipe** for the force-pushed 0.4.0.
10. **`V040_COORDINATION.md`** — tactical week plan + division of labour with Anders.
11. **`MAC.md` / `RUN_HOME_LOCALLY.md`** (in `docs/`) — run the UI locally on macOS
    (`elastos gateway` not `serve`; use `localhost:8090` for WebAuthn).

**Full prior conversation transcripts** (every decision, verbatim) — search by
keyword (filename, error, "Day N") if you need the why behind a decision:
- Days 1–17: `…/agent-transcripts/6f8c08cd-415d-4f58-b41d-74e2724fb796/6f8c08cd-415d-4f58-b41d-74e2724fb796.jsonl`
- Days 18–34: `…/agent-transcripts/43110c1d-e79d-43d4-818b-4a2f0fb3233b/43110c1d-e79d-43d4-818b-4a2f0fb3233b.jsonl`

(both under `/Users/sash/.cursor/projects/Users-sash-code-elastos-runtime/`)

---

## 5. Key people & their concerns

- **Anders** — runtime lead. Owns the 7 binding Mac-VZ decisions and the 0.4.0
  mainline. **Actively redoing 0.4.0 today** (only ~20% on GitHub; force-pushed
  once already; more redones coming). Owns the **rail decision** (DDRM_DECRYPT_RAIL
  §"Questions for Anders"). Do **not** rely on the GitHub 0.4.0 being final.
- **Irzhy** — security. Author of the two invariants (§2). Proposed "two boxes +
  secured channel (ECDH + DSA)" for the key→decrypt hop — we adopted it, upgraded to
  PQ-hybrid. He had requested a clearer picture → that's `DDRM_SECURITY_MODEL.md`.
- **Ross** — built PC2's media + non-media players (the consumers of our decrypt
  output). Their contract is pinned in `PC2_PLAYER_ALIGNMENT.md`.

---

## 6. Critical constraints (do not forget these)

- **GitHub push is SUSPENDED** (user's account). All work lives on **local
  branches**; nothing is pushed. We can `git fetch` (read) but not push. Plan to
  push = `PUSH_PLAN.md`.
- **0.4.0 is in flux.** Anders force-pushed it and more redones are coming. **Do not
  rebase onto it yet.** When it settles: run `scripts/ddrm-verify.sh` (drift +
  cross-impl conformance, must PASS), then follow the rebase recipe in
  `PUSH_PLAN.md`. A safety backup of our tip is `backup/decrypt-provider-cenc-preD17`.
- **Contract converged — zero type drift.** `elastos-common/protected_content.rs`
  is byte-identical between our branch and the redone 0.4.0. Our providers were
  built against the exact types Anders independently landed. Keep it that way; the
  drift guard enforces it.
- **MSRV is pinned 1.89** (`rust-toolchain.toml`). The Carrier `iroh`/Hickory CVE
  closure needs 1.91 → it's a deferred operator decision (`CARRIER_IROH_UPGRADE.md`).
- **PC2 uses classical crypto (P-256 ECDH/ECDSA); the Runtime mandates PQ-hybrid**
  (`x25519+ml-kem-768`, `ml-dsa-65`). Keep PC2's envelope *structure*, upgrade the
  *crypto*.
- **`encrypt-provider` is reconciled to `elastos-common`** (Day 39): its sealed
  **output** is the shared `SealedObjectV1`/`KeyEnvelopeV1`; only the **input**
  `SealRequest` stays local (no shared seal-request type yet). The Day-16
  self-containment is retired (the contract is stable + drift-pinned).

---

## 7. Branch topology (local, unpushed)

dDRM + convergence work (all based on `origin/0.4.0`):

| Branch | What | Push order (PUSH_PLAN) |
|---|---|---|
| `feat/decrypt-provider-cenc` | **the main dDRM branch** (Days 1–17): 5 providers, engine, envelope spec, consumer contract, security model, drift guard, all docs | #5 (the big one) |
| `fix/crosvm-darwin-build` | gate Linux-only TAP networking so 0.4.0 builds on macOS | #1 |
| `fix/home-summary-resilience` | corrupt `browser-state.json` resets instead of failing login | #2 (stacked on #1) |
| `chore/bincode-2x` | bincode 1.3→2.x with wire-format compat tests | #3 |
| `chore/carrier-iroh-upgrade` | iroh/Hickory ADR + audit.toml rationale (docs only) | #4 |
| `backup/decrypt-provider-cenc-preD17` | safety snapshot before the Day-17 base analysis | — (do not push) |

Older/unrelated: `sash/local-test*` (Mac VZ core work, intentionally separate),
`chore/runtime-cve-hygiene*`, `sash/v040-integration`.

---

## 8. Open items / what's NOT done (and why)

1. **The decrypt rail** (BLOCKER, needs Anders). How the sealed CEK reaches
   decrypt-provider. We chose Hybrid (decrypt *receives* sealed material; upstream
   is a provider chain) + Irzhy's secured ECDH+DSA channel, PQ-hybrid. **3 sharpened
   questions for Anders** in `DDRM_DECRYPT_RAIL.md` / `DDRM_STATUS.md`. The full
   unwrap→cenc composition is proven for both the classical (`rail-prep`) and PQ
   (`pq-rail-prep`) profiles, **and the transport shim itself is now built + fully
   tested** behind `rail-shim` (Days 27–28: `decrypt_from_carrier`, carrier golden,
   PC2 session conformance). So the only work left behind the blocker is the
   **one-line `OpenSession` wire-up** (`rail_shim::decrypt_from_carrier(...)`) — Q1
   (dKMS-direct vs re-seal) doesn't touch the adapter, Q2 (signature) plugs in via
   the `CekSealVerifier`, profile is a per-deployment `SealProfile` pick.
2. ~~Encrypt in-boundary keygen engine (invariant #1 gap).~~ **CLOSED (Day 19).**
   `encrypt-provider` now mints CEK+KID with a CSPRNG inside the boundary and the
   seal engine emits no key material; `cek_and_kid_generated_inside_boundary` and
   `seal_engine_emits_no_key_material` pass. See `DDRM_ENCRYPT_INVARIANT.md`.
3. **PQ migration of the envelope** (de-risked + real primitive, not yet wired into
   default dispatch). `envelope.rs` is the classical PC2 spec; `pq_envelope.rs` proves
   the PQ-hybrid profile end to end behind `pq-envelope`/`pq-rail-prep`, and the signature
   is now the **real FIPS 204 ML-DSA-65** (`pq-mldsa`/`rail-shim-mldsa`, Days 32–33), not a
   stub. Wiring it into default dispatch lands with the rail; the remaining choice is
   Anders' Q2 *policy* (straight ML-DSA vs hybrid during PC2's migration), not a build gap.
4. **Carrier iroh/Hickory upgrade** — deferred (MSRV 1.91), operator decision.
5. **Rebase onto stabilised 0.4.0** — deferred until Anders stops force-pushing.
   Pre-rebase gate is now `scripts/ddrm-verify.sh` (drift + cross-impl conformance +
   ladder counts/wasm; `DDRM_VERIFY_FAST=1` skips the heavy ladder gate).

---

## 9. How to verify (commands)

```bash
# THE standing pre-rebase/PR gate: drift + PC2 conformance + ladder/wasm + WASI smoke.
# Gate 3 (ladder) asserts every test count + the wasm builds, so a dropped or
# feature-gated-out test FAILS the gate. Gate 2 (conformance) skips clean without PC2;
# gate 4 (WASI smoke) skips clean without wasmtime.
scripts/ddrm-verify.sh                          # expect: ALL GATES PASS
DDRM_VERIFY_FAST=1 scripts/ddrm-verify.sh       # skip the heavy gates 3+4 (1+2 only)

# (the gate's three parts, runnable on their own)
scripts/ddrm-drift-check.sh                     # contract drift — expect PASS
scripts/pc2-conformance.sh                      # cross-impl parity — PASS (or SKIP without PC2)
scripts/ddrm-ladder-check.sh                    # ladder counts + wasm builds — expect INTACT

# per-provider host tests (fast, authoritative): 13+12+9+9+25 = 68 green, 0 ignored
for p in encrypt drm rights key decrypt; do (cd capsules/$p-provider && cargo test); done

# decrypt-provider feature ladder (tested islands; counts in §3)
# default 25 / rail-prep 27 / pq-envelope 29 / pq-rail-prep 31 / vectors 42 /
# rail-shim 45 / pq-mldsa 34 / pq-mldsa-hybrid 37 / rail-shim-mldsa 54 / harden 65 / rail-live 57 / rail-bind 60 / rail-mint 62 / rail-audit 62 / rail-material 65
( cd capsules/decrypt-provider && \
  for f in rail-prep pq-envelope pq-rail-prep vectors rail-shim pq-mldsa pq-mldsa-hybrid rail-shim-mldsa harden rail-live rail-bind rail-mint rail-audit rail-material; do \
    cargo test --features $f; done )

# regenerate the committed golden vectors (only when intentionally changing them)
( cd capsules/decrypt-provider && cargo test --features gen-vectors emit_ )
# (the ML-DSA goldens need pq-mldsa too:)
( cd capsules/decrypt-provider && cargo test --features "gen-vectors,pq-mldsa" emit_ )

# whole chain under the WASI sandbox (needs: rustup target add wasm32-wasip1; brew install wasmtime)
scripts/ddrm-chain-smoke.sh                     # 4 chain providers PASS

# wasm build of a provider
( cd capsules/decrypt-provider && rustup run 1.89.0 cargo build --target wasm32-wasip1 --release )
```

---

## 10. The working method (how we operate — keep this bar)

**Convergence laws** (from the playbook):
- One boundary at a time; isolated commits; reversible.
- Contract-first: pin the interface with **characterization tests** before wiring an
  engine.
- Anti-Corruption Layer: translate PC2 patterns, don't import its trust model.
- CEK containment is sacred: it lives sealed, is used in one boundary, zeroized.
- Fail-closed: every unconfigured path returns `not_configured`, never opens.

**Validate against the source of truth.** When mapping a PC2 behaviour, read the PC2
repo (`/Users/sash/Documents/Cursor/pc2.net/pc2-node`), not memory. Key PC2 crates:
`crates/cenc-encrypt`, `crates/ddrm-decrypt`, `wasm-apps/ddrm-renderer`,
`src/services/media/dashPackager.ts`.

**Commit discipline.** Small, scoped, descriptive commits on the right isolated
branch. Never push (suspended). Never commit `build/` or `scripts/dev/`.

---

## 11. The "10/10 daily prompt" methodology (important)

The user runs this as a **day-by-day loop**. At the end of each day you:
1. Report what was done (crisp, evidence-backed: tests green, commit SHA, branch
   ahead-count).
2. **Present the next day's "10/10 prompt"** and ask the user to continue.

A **10/10 prompt** is engineered with this anatomy:
- **Role** — frame the agent as a senior specialist (e.g. "Convergence lead").
- **Objective** — the single highest-leverage, *unblocked* outcome for the day,
  justified by the priority stack and current blockers.
- **Tasks** — 2–4 concrete, ordered steps; validate against PC2 + the runtime
  principles; pin with characterization tests; keep isolated.
- **Definition of done** — measurable: tests green / proof recorded / one isolated
  commit / docs updated.
- Implicitly: best-practice framing (industry standards, named patterns —
  Strangler Fig, ACL, Branch-by-Abstraction, characterization tests), and always
  rebase-safe + fail-closed + contract-first.

The loop's discipline: **never do blocked work**; always advance the most valuable
thing that is *currently* unblocked; leave the chain provably green; document so the
next context can continue cold.

---

## 12. Day log (1–28, one line each)

- **D1** vendor PC2 cenc engine into decrypt-provider; fix typed `release_receipt`.
- **D2** gate Linux-only crosvm networking → 0.4.0 builds on macOS (`fix/crosvm-darwin-build`).
- **(bugfix)** passkey 500 = corrupt `browser-state.json`; resilient reset (`fix/home-summary-resilience`).
- **D3** decrypt-step core seam (Branch-by-Abstraction) + rail decision recorded.
- **D4–5** decrypt-provider wasm/WASI proofs + isolation-tier rationale.
- **D6** key-provider binds upstream rights receipt; wasm/WASI bar.
- **D7** rights-provider WASI smoke (chain parity).
- **D8** drm-provider WASI smoke + cross-provider contract-seam tests.
- **D9** unified `ddrm-chain-smoke.sh` + review-ready `DDRM_STATUS.md`; architecture visuals.
- **D10** bincode 1.3→2.x with wire-format golden tests (`chore/bincode-2x`).
- **D11** Carrier iroh/Hickory upgrade ADR — blocked on MSRV (`chore/carrier-iroh-upgrade`).
- **D12** vendor ECDH envelope spec + PC2 player alignment; `PUSH_PLAN.md`.
- **D13** pin decrypt→player consumer contract (both players).
- **D14** `DDRM_SECURITY_MODEL.md` (flows, threat model, invariant→test) + inter-stage CEK transport decision (Irzhy).
- **D15** refresh status; prove PQ-hybrid compiles in wasm (ml-kem/ml-dsa).
- **D16** `encrypt-provider` skeleton; pin invariant #1; capture in-boundary-keygen gap.
- **D17** 0.4.0 force-push reconciled (zero type drift); `ddrm-drift-check.sh`; deferred rebase.
- **D17.5** `HANDOVER.md` single-entry onboarding (`14cb2306d`).
- **D18** prep rail-landing: classical `ecdh_unwrap → cenc` composition behind `rail-prep` (`27cce2d5e`).
- **D19** close invariant #1: in-boundary CEK+KID keygen + seal engine; 68 green/0 ignored (`ec6fd6dcf`).
- **D20** de-risk PQ-hybrid CEK-seal envelope island behind `pq-envelope` (`38fa91a48`).
- **D21** prove full PQ dDRM data path end-to-end pre-rail behind `pq-rail-prep` (`ee5b084f9`).
- **D22** pin both engines with portable golden vectors (`vectors`/`gen-vectors`) (`7df180297`).
- **D23** make PC2 cross-impl conformance executable (`scripts/pc2-conformance.sh`) (`8bf242a20`).
- **D24** promote conformance to a standing gate (`ddrm-verify.sh`) + v2 vector + tamper parity (`8cb43b814`).
- **D25** refresh `HANDOVER.md` to current truth (Days 18–24) (`874c3f5b6`).
- **D26** encrypt→decrypt round-trip golden — both invariants on one artifact (`vectors`=37) (`48aef61c9`).
- **D27** rail transport shim behind `rail-shim` — `decrypt_from_carrier`, both profiles, un-wired (`f3d09e922`).
- **D28** pin the carrier as a portable golden + PC2 session-level conformance (`rail-shim`=43) (`363d75b09`).
- **D29** refresh `HANDOVER.md` to current truth (Days 25–28) (`80137f260`).
- **D30** PQ carrier golden through the shim — profile symmetry closed (`rail-shim`=45) (`e4e4d11c2`).
- **D31** widen cenc goldens to real shapes (multi-sample/subsample/init-IV) + PC2 parity (`vectors`=40) (`787bb3acd`).
- **D32** wire the real FIPS 204 ML-DSA-65 into the `CekSealVerifier` slot behind `pq-mldsa` (=34) (`d6899b9ed`).
- **D33** verify the real ML-DSA-65 through `decrypt_from_carrier` on a carrier golden (`rail-shim-mldsa`=54) (`aadb4f1fc`).
- **D34** adversarial negative-space + containment sweep behind `harden` (=65) (`b1f8b7dd5`).
- **D35** make the gate authoritative (`ddrm-ladder-check.sh`: counts + wasm) + handover refresh (`90899e70d`).
- **D36** reconcile-prep: widen drift guard to full consumed surface (fn + DEFAULT_* + PQ-algo fields), button-press rebase recipe, gate the encrypt↔decrypt seam by name (`d1035d98b`).
- **D37** widen the producer round-trip to real shapes: encrypt-provider emits multi-sample + subsample round-trip goldens, replayed byte-exact by decrypt (`vectors`=42); gate asserts all 3 seams by name (`c63c375db`).
- **D38** prove PC2 consumes the producer's output: drive the multi-sample + subsample producer segments through PC2's real `mp4box`+`cenc` (byte parity + wrong-CEK key-bound) in `pc2-conformance.sh` (`926b9adcb`).
- **D39** reconcile `encrypt-provider` to `elastos-common`: sealed output now the shared `SealedObjectV1`/`KeyEnvelopeV1` (typed), algorithm set checked by the shared validator; only input `SealRequest` stays local; Day-16 self-containment retired (`b3b5f0a9d`).
- **D40** integrity audit: every claim→gate mapped (table in `DDRM_STATUS.md`), no orphan vectors / dead flags, counts re-validated fresh; **WASI smoke wired into `ddrm-verify.sh` as gate 4/4** (skips clean w/o wasmtime) — the last doc-only claim is now gate-backed (`4f0cc653a`).
- **D41** pre-prove Anders' OTHER Q2 answer: a hybrid ECDSA-P256 + ML-DSA-65 `HybridVerifier` (feature `pq-mldsa-hybrid`=37, BOTH halves must verify, `wasm32-wasip1`-clean) through the same `hybrid_unwrap` path — Q2 is now a pure policy pick, both answers drop-in (`779c74ff6`, lock `a291becb7`).
- **D42** build-hygiene (sibling branch, off the dDRM critical path): verified `fix/crosvm-darwin-build` is **green on this macOS** — `elastos-crosvm` 18 tests pass + warning-free, `elastos-server` builds clean; recorded in `PUSH_PLAN.md` (#1 now build-verified, not just authored). dDRM gate untouched (still 4/4).
- **D43** build-verify push queue #3 + #2 on macOS: `chore/bincode-2x` **311 passed / 0 failed** incl. the capability-token byte-identity golden (`token_wire_format_is_bincode_1x_legacy`) — wire format provably unchanged; `fix/home-summary-resilience` builds clean + its `home_browser_state_*` tests pass (4 `home_launch`/`runtime_ensure` failures are **no-KVM env limits, identical on the crosvm branch → not a regression**, pass on Linux CI). Recorded in `PUSH_PLAN.md` with a Linux-test-gating follow-up. dDRM gate still 4/4.
- **D44** **0.4.0 RELEASED** (tag `v0.4.0`=`cae83c3c3`) — alignment audit: `protected_content.rs` **byte-identical** to the release; `ddrm-drift-check.sh` **passes against the released base**; crypto core validated green ON `v0.4.0` (overlay worktree: drift PASS, harden=65, pq-mldsa-hybrid=37, encrypt=13, pc2-conformance byte-compatible). Released providers are still fail-closed skeletons (no rail). Rebase surface MEASURED (`PUSH_PLAN.md`): decrypt/encrypt clean, **key+drm 3-way (needs Anders)**. Rail decision remains the one blocker.
- **D45** **recommended rail WIRED into dispatch** (Option A, decision taken with the team): new `OpenSessionLive` op runs the proven `rail_shim::decrypt_from_carrier` in-boundary with a real `MlDsa65Verifier` and returns a scoped response. Feature `rail-live`=57: a real ML-DSA-65-signed PQ-hybrid carrier decrypts through the **actual provider dispatch** with **no CEK/plaintext leak**; tampered carrier + unprovisioned boundary both fail closed; `wasm32-wasip1`-clean. Shared `DecryptSessionRequestV1` **untouched** (material rides a capsule-local variant) → drift still PASS, default build byte-identical + fail-closed. The exact additive contract delta for default-on is written in `DDRM_DECRYPT_RAIL.md` (§Reference rail LANDED). Ladder gate now pins `rail-live`=57 + its wasm build. Only remaining step to live decrypt: Anders' thumbs-up on the contract field.
- **D46** **Anders confirmed the rail** (hybrid, ElastOS-native, Option A push-in, chain `drm→rights→key/dKMS→decrypt`, in-sandbox session key, providers stay separate, PQ-hybrid root, P-256/Lit compat-only) and added one hard requirement: the sealed material must **bind the full decrypt transcript** (AEAD/AAD + signature + replay nonce). **LANDED** on the PQ profile (`rail-bind`=60): capsule-local `DecryptTranscriptV1` (principal, session, object CID+content hash, action, viewer interface, output kind, expiry, release-receipt hash, decrypt-session pubkey, suite, provider, nonce) is the AES-256-GCM **AAD** + covered by the **ML-DSA-65 signature** (`hybrid_unwrap_bound`/`seal_bound`, golden-safe: `aad==b""`==legacy). `OpenSessionBound` rebuilds the transcript from the **authenticated request** + the boundary's own session pubkey → a CEK bound to one transcript **cannot be replayed**: different `session_id` / swapped nonce / tampered carrier all fail closed. `rail-shim-mldsa`=54 + `harden`=65 unchanged → no golden disturbed; drift PASS; default byte-identical. Ladder pins `rail-bind`=60 + wasm. Remaining (upstream, needs Anders/dKMS): fold `sealed_decrypt_material` into the shared contract, in-sandbox key mint+publish, dKMS-direct sealing.
- **D47** **in-sandbox session-key mint + publish** (Anders Day-45 ask) — feature `rail-mint`=62: `init` now MINTS the per-session hybrid KEM keypair inside the boundary (`pq_envelope::mint_session`, OsRng→WASI `random_get`, `wasm32-wasip1`-clean), holds the secret in-VM, and PUBLISHES the pubkey + suite (`decrypt_session_public_key_b64`) for the key authority to seal to. Faithful flow proven with NO injected secret: sandbox mints+publishes → authority seals the CEK to the published key (transcript-bound) → the minted secret opens it, no CEK/plaintext leak; a fresh key is minted per init. Mint is the ONLY entropy the boundary needs; the unwrap path stays RNG-free (separate feature). Default + every golden unchanged; drift PASS; ladder pins `rail-mint`=62 + wasm. Remaining (upstream, needs Anders/dKMS): fold `sealed_decrypt_material` into the shared contract; dKMS-direct sealing (or audited key-provider re-seal).
- **D48** **short-expiry enforcement + scoped audit** (Anders Day-45 "short expiry, audit") — feature `rail-audit`=62: new `OpenSessionAudited` op takes an injected capability clock (`now_unix`, never ambient), REJECTS a stale grant (`now_unix` past `request.expires_at` or the release-receipt expiry) BEFORE any unwrap (fail-closed `expired`), and emits a scoped, tamper-evident **audit record bound to the transcript hash** on every decision (`opened`|`denied`) carrying NO CEK/plaintext. Proven: a fresh grant opens + audits `opened` (with scoped session); an expired grant fails closed + audits `denied`/`expired` with no session and no unwrap; audit is CEK/plaintext-free on both paths. Shared `open_session_bound` logic refactored into `prepare_bound_open` (rail-bind=60 + rail-mint=62 unchanged → no regression). Default + goldens unchanged; drift PASS; ladder pins `rail-audit`=62 + wasm. **The decrypt boundary now implements Anders' ENTIRE decrypt-side spec.** Remaining is upstream only: fold `sealed_decrypt_material` into the shared contract (needs push); dKMS-direct sealing (needs Anders).
- **D49** **consolidated `SealedDecryptMaterialV1`** (drop-in contract shape) — feature `rail-material`=65: the carrier is now a single backend-neutral, **suite-tagged** envelope (dKMS-native PQ-hybrid vs P-256/Lit compat is a FIELD, not a fork). Canonical op `OpenSessionV1` routes by `suite` into the audited/expiry-enforcing transcript-bound path; the compat suite is rejected on the product path and an unknown suite fails closed. `DDRM_DECRYPT_RAIL.md` §Consolidated envelope now carries the **verbatim additive `DecryptSessionRequestV1` delta** for Anders to lift. Default + goldens unchanged; drift PASS; ladder pins `rail-material`=65 + wasm. **The decrypt boundary is COMPLETE** — every clearly-ours task is done. Remaining is upstream only: (1) fold `SealedDecryptMaterialV1` into the shared `elastos-common` contract (needs push access); (2) the dKMS-direct sealing producer / audited key-provider re-seal (needs Anders).
- **(research, Day 49)** whole-system study: mapped the full PC2 journey (creator→publish→market→purchase→download→validate→key→decrypt→playback, Base + Lit/Chipotle) against the runtime; wrote **`SYSTEM_ARCHITECTURE_MAP.md`** (current/target diagrams, PC2→runtime pattern-migration table, check-against-PC2 index, phased road to a testable E2E). Net: decrypt boundary done + infra exists; missing middle = key authority + orchestration wiring + producer/market/viewer.
- **D51** **reference key-authority seal engine + shared `ddrm-envelope` crate** (Phase A.2) — new `capsules/ddrm-envelope` is the single source of truth for the PQ-hybrid seal/unwrap + wire format + ML-DSA-65 signer/verifier (extracted byte-identical from `decrypt-provider::pq_envelope`; seal promoted to production). `key-provider`'s `reference` backend (feature `key-authority-ref`) seals a recovered CEK to a decrypt session's published key via the crate and emits the exact `SealedDecryptMaterialV1` the decrypt boundary opens, through a capsule-local `release_ref` op (shared `KeyReleaseRequestV1` byte-identical, Parallel Change). **Cross-boundary proof:** a test seals with the reference authority + opens with the SAME `ddrm_envelope::hybrid_unwrap_bound` the decrypt boundary uses — wire-compatible, transcript-bound, no raw CEK on the wire. 23 key-provider tests under the feature (18 default + 5 reference) + 7 in `ddrm-envelope`; default fail-closed; decrypt-provider untouched (10-combo ladder unchanged); ladder pins `ddrm-envelope`=7 + `key-authority-ref`=23 + both wasm. **Next (Phase A.3):** migrate decrypt-provider onto `ddrm-envelope` (pure refactor, golden-gated) then wire `drm/open → rights → key → decrypt`.
- **D71** **runtime-core plan EXECUTOR — the `DrmOpenPlanV1` is now walked by the core, not hand-walked by the smoke.** New fail-closed library `capsules/ddrm-plan-runner`. Audited PC2's open sequencer first — each stage gated on the prior: `requireSecureViewSession` resurrects the session view (`src/api/middleware/secureViewSession.ts:61`), then `recoverMediaCEK` → `recoverCEKEnvelope` whose access gate is `hasAccessByContentId(ownerAddress, kid)` (`src/api/media.ts:1163`) and which only THEN recovers + unwraps the CEK in-boundary (`:1196`, `:1216`). `DrmOpenPlan::parse` validates schema / `planned` status / the `rights_check<key_release<decrypt_session` canonical order / every binding edge naming real steps + identities (incl. `content_id==object_cid`); `execute` seeds the virtual `drm_open` identities, walks the steps IN ORDER, threads each binding edge's produced artifact into the next step's plan-declared `into_field`, and FAILS CLOSED when a step needs an artifact not yet produced (out-of-order / a prior step silently failed) or runs without emitting the artifact the plan says it produces. It holds NO authority — the ONLY thing that touches a provider is the injected `StepRunner` (the runtime's capability seam); the CEK/wallet/chain never appear in the crate. 14 characterization tests (canonical drive + order/threading/identity-seeding + renamed-edge / dropped-artifact / backward-edge / out-of-order / wrong-schema / identity-split / no-authority all fail closed). `ddrm-consumer-smoke.sh` no longer hand-walks the chain: it fetches the REAL `drm open` plan, parses it through the core, and drives drm→rights→key→decrypt THROUGH `DrmOpenPlan::execute` (the smoke is just the injected `SmokeRunner` transport), adding a NEW cross-binary fail-closed gate — a TAMPERED binding edge is rejected by the real `key-provider` (`deny_unknown_fields` over the required `rights_receipt`). New ladder rung ddrm-plan-runner=14 (host-side core, not a wasm capsule). drift untouched (the executor reads the plan; it defines no shared contract). Gate: ladder INTACT, drift PASS, 4 smokes green, clippy clean.
- **D70** **the canonical `key-provider::release` actually releases (reference backend): recover-from-escrow → re-seal to session.** The op `drm-provider`'s `DrmOpenPlanV1` names for the key step was a `not_configured` stub for the reference backend, and the consumer smoke handed the authority a RAW golden CEK (`release_ref`). Both gone. Audited PC2's Lit authority (`universal-decrypt-chipotle.js`: access-check `:560–568` → recover `Lit.Actions.Decrypt` `:570–575` → `sha256(cek‖kid‖authority)` bind `:577–590` → seal-to-session `envelopeCEK` `:602–608`; `chipotle-client.ts::recoverCEKEnvelope` `:1438–1538` returns sealed-only). `release` validates the rights receipt (always, before any backend), then for the reference backend RECOVERS the producer-escrowed CEK from the rights-bound `key_envelope.wrapped_cek` (recomputing the shared `escrow_aad(scheme,kid16,recipient_pub)`, verifying the producer vk) and re-seals it to the runtime-injected decrypt session as `SealedDecryptMaterialV1` (reusing the proven `recover_escrowed_cek` + `seal_recovered_cek_into_material`). The wrapped CEK rides INSIDE the validated request; the per-session material (session key + producer vk + transcript + optional clock) is a capsule-local `session` context on the op envelope, so the shared `KeyReleaseRequestV1` stays byte-identical (drift untouched). Fail-closed: no backend/no session → `not_configured`; denied/expired/kid-swap/scheme-mismatch/forged-producer → refused; CEK only `Zeroizing`, leaves only SEALED. key-provider key-authority-ref 27→33; default 18. `ddrm-consumer-smoke.sh` now escrows the golden CEK to the authority's published recipient and drives the CANONICAL `release` (recover→reseal) — the consumer half (drm→rights→key→decrypt) runs with no raw CEK shim, and a transcript-mismatched seal still fails closed. Gate: key-provider=18/33, ladder INTACT, drift PASS, 4 smokes green, clippy clean.
- **D69** **`encrypt-provider::seal` runs the full production pipeline on handed-in bytes → complete `SealedObjectV1`.** The non-inline `seal` op (fail-closed Day-1 skeleton) now emits the chain-shaped sealed object — closing the gap between dev `seal_inline` and production `seal`. Audited PC2's producer INPUT path first (`src/services/media/dashPackager.ts`): CEK minted in the host (`generateCEK` `:122–126`), each segment read off disk (`readFileSync` `:504`, `:571–572`), bytes HANDED to the CENC WASM (`executeCENCEncrypt(.., seg.data)` `:432–434`) — the encoder fetches nothing. Mirrored: `SealRequest` gained `content_b64` (handed-in bytes), `recipient_pub_b64` (authority's escrow recipient), `availability_receipt_cid` (pin receipt) — all optional, `deny_unknown_fields` preserved (existing fail-closed tests hold). Given bytes + recipient, `seal` runs the ONE shared `run_seal_pipeline` (mint CEK+KID → CENC-encrypt → content-address Day-68 `payload_cid` → escrow CEK SEALED) and assembles an `elastos_common::protected_content::SealedObjectV1`: real `payload_cid`, `key_envelope.kid` == bytes16 contentId, `policy_hash = sha256(rights_policy_cid)`, PQ-hybrid suite the chain validates. `seal_inline` now DELEGATES to the same pipeline (PRINCIPLES #10). NO fetch/IPFS/network authority — it seals the bytes it's handed. Fail-closed: no recipient/bytes → `not_configured`; missing receipt / empty viewer-interface / empty content → `invalid_request`. encrypt escrow 22→25 (+configured-emits-complete, +each-seal-fresh, +fail-closed-matrix); default 20. `ddrm-producer-smoke.sh` drives the REAL `seal`, deserializes into the SHARED `SealedObjectV1` and runs the SAME `validate_protected_content_key_envelope_algorithms` `key-provider` runs — cross-binary proof the chain accepts it; asserts `payload_cid` (`bafkrei…`) ≠ KID and no plaintext on the wire (production output carries no segment). Gate: encrypt=20/25, ladder INTACT (+wasm), drift PASS, 4 smokes green, clippy clean.
- **D68** **`encrypt-provider` content-addresses the ciphertext — `payload_cid` is REAL.** The producer's last "trust me" (a hardcoded `bafybeig…` placeholder) is gone. Audited PC2's producer storage first (`src/storage/ipfs.ts`: `storeFile`/`fs.addBytes` `:644–678`; `@helia/unixfs/src/commands/add.ts:15–24` → `cidVersion:1, rawLeaves:true`, 1 MiB `fixedSize` chunker). `encrypt-provider` now derives `payload_cid = CIDv1(raw 0x55, sha2-256)` of the sealed segment in-boundary (`payload_cid_v1_raw` + hand-rolled base32 multibase) — a pure function of the bytes, NO `kubo_api`/network (a CID is not a pin), fail-closed above one chunk (multi-block dag-pb refused). `seal_inline` emits it. Golden pins three inputs to the EXACT strings PC2's real `ipfs-unixfs-importer` produces (incl. canonical raw-`abc`). encrypt-provider 17→20 / 19→22. `ddrm-producer-smoke.sh` independently recomputes the CID via the canonical IPLD `cid` crate (different encoding path) and demands a byte-for-byte cross-binary match. payload_cid (IPFS address) ≠ KID/contentId (chain key). Gate: encrypt=20/22, ladder INTACT (+wasm/sha2), drift PASS, 4 smokes green, clippy clean.
- **D67** **`drm-provider::open` emits the executable `DrmOpenPlanV1`** — the orchestrator stops being a Day-1 skeleton (`open → not_configured`). Audited PC2's open ordering first (`universal-decrypt-chipotle.js` `main`: access-check `:545–568` → key-release `:570–575` → bind `:577–590` → seal `:602–608`; `chipotle-client.ts::recoverCEKEnvelope` `:1438–1538` sign→assemble→run→return-sealed-only). `open` now validates fail-closed then returns a typed **`DrmOpenPlanV1`** (status `planned`, never `opened`): the 8-step canonical sequence + **binding edges** (rights ⇒ `RightsDecisionReceiptV1` → `key.rights_receipt`; key ⇒ `ReleaseReceiptV1` → `decrypt.release_receipt`; content identity == KID under BOTH `content_id`/`object_cid` per `key-provider:740`'s `content_id==object_cid` invariant) + next-providers + runtime events + `blocked_authority`. Holds ZERO authority (PLANS, runtime EXECUTES — Day-61 pattern); `DrmOpenPlanV1` capsule-local like `UnsignedMintV1` → shared contract + drift untouched. drm-provider 12→15; `ddrm-consumer-smoke.sh` now drives the REAL `drm open`, asserts `planned`+order+bindings, and FOLLOWS the plan (threads receipts into plan-declared fields, identity from the plan) instead of hardcoding (PRINCIPLES #10). Gate: drm-provider=15, ladder INTACT, drift PASS, all 4 smokes green, clippy clean.
- **D50** **`key-provider` → pluggable multi-backend authority** (Phase A.1; confirms Anders' "providers inside the key capsule") — `KeyAuthorityBackend`: `reference` (native-dev, PQ-hybrid suite), `dkms` (native-production), `lit` (PC2/Chipotle compat, classical suite), all destined to emit the same suite-tagged `SealedDecryptMaterialV1` the decrypt sandbox consumes. Backend is **operator/runtime config at `init`** (never an app input) → shared `KeyReleaseRequestV1` byte-identical. `status` advertises `supported_backends` (suite/kind/state) + `active_backend`; `release` runs **all existing validation first**, then routes per-backend to a precise `not_configured` (reference seal engine = Phase A.2); no backend = fail-closed. 18 characterization tests (was 9), incl. **validation-precedes-backend** (a denied receipt never reaches a backend) + unknown/non-string backend rejection. Default fail-closed + goldens unchanged; ladder pins key-provider=18 + wasm. Mirrors PC2 Lit authority role (`chipotle-client.ts`/`universal-decrypt-chipotle.js`).

---

## 13. Next

**The decrypt boundary is COMPLETE** (Days 45–49): Option A push-in (`rail-live`),
full-transcript binding (`rail-bind`), in-sandbox key mint+publish (`rail-mint`),
short-expiry + scoped CEK-free audit (`rail-audit`), and the consolidated suite-tagged
`SealedDecryptMaterialV1` drop-in (`rail-material`). Anders confirmed the architecture
on Day 45 and the boundary now implements his entire decrypt-side spec.

For the **whole-system** picture — the full PC2 creator→publish→market→purchase→
download→validate→key→decrypt→playback journey mapped against the runtime, with a
current/target architecture map and the phased road to a testable end-to-end — read
**`SYSTEM_ARCHITECTURE_MAP.md`** (Day 49 research). Summary of where the gaps are:

- ✅ **Done / exists:** the decrypt boundary; the trusted core; `ipfs-provider`,
  `chain-provider` (incl. typed `has_access_by_content_id`), `wallet-provider`,
  `content` publish/fetch.
- 🟩 **Orchestrator (Day 67):** `drm-provider::open` now emits the executable
  **`DrmOpenPlanV1`** (status `planned`) — the capsule-owned canonical `drm/open`
  sequence + its inter-step binding edges — and the consumer smoke FOLLOWS that plan
  cross-binary. The plan is declarative (zero authority); the runtime still EXECUTES it.
- 🟩 **Producer content-addressing (Day 68):** `encrypt-provider` now derives the real
  `payload_cid` (CIDv1 raw/sha256) of the sealed ciphertext in-boundary, IPFS-faithful and
  fail-closed; the producer smoke verifies it cross-binary against the canonical `cid` crate.
- 🟩 **Production seal (Day 69):** `encrypt-provider::seal` (non-inline op) now runs the full
  pipeline on HANDED-IN asset bytes (`content_b64`, mirroring PC2's host→WASM byte hand-off)
  and emits a complete shared `SealedObjectV1` — real `payload_cid`, bytes16-KID envelope,
  PQ-hybrid suite the chain validator accepts. NO fetch authority; fail-closed without bytes +
  recipient. Producer smoke drives it cross-binary against the shared type + validator.
- 🟩 **Canonical key release (Day 70):** `key-provider::release` (the op the plan names) now
  ACTUALLY releases for the reference backend — recovers the producer-escrowed CEK from the
  rights-bound `key_envelope` and re-seals it to the runtime-injected decrypt session as
  `SealedDecryptMaterialV1`, fail-closed on denied/expired/kid-swap/forged-producer. The
  consumer smoke drives the canonical op (escrow→recover→reseal), removing the raw-CEK shim.
- 🟩 **Runtime-core plan executor (Day 71):** `capsules/ddrm-plan-runner` walks the
  `DrmOpenPlanV1` — validating order + binding edges, threading each edge's artifact into the
  next step, and failing closed on a broken/out-of-order edge — holding NO authority (the only
  thing that touches a provider is the injected `StepRunner`). The consumer smoke now drives the
  REAL drm→rights→key→decrypt binaries THROUGH the core (the smoke is just the transport), and a
  TAMPERED binding edge is rejected cross-binary by the real key-provider. Mirrors PC2's gated
  open sequencer (`secureViewSession.ts:61` → `media.ts:1163`/`:1196`). ddrm-plan-runner=14.
- 🟩 **Runtime-core injected capability handles (Day 72):** a new `RuntimeStepRunner` (in
  `ddrm-plan-runner`) IMPLEMENTS the Day-71 `StepRunner` over a set of injected `ProviderHandle`s
  — one per provider the plan's `next_required_providers` names — routing each step to the handle
  for that step's `provider`, holding no authority itself. Fail-closed construction: refuses to
  build without a handle for every required provider (no ambient default) and rejects a STRAY
  handle for a provider the plan does not name (the `blocked_authority` set is unreachable from
  the runner type). The consumer smoke's monolithic `SmokeRunner` is replaced by three
  per-provider handles (`RightsHandle`/`KeyHandle`/`DecryptHandle`, each wrapping ONE real capsule
  binary) injected into the SAME runner the trusted core will use (no second code path). Mirrors
  PC2's per-stage injected `BackendSessionView` (`secureViewSession.ts:124` → `media.ts:1207`).
  ddrm-plan-runner 14→21.
- 🟩 **Runtime-core composition root (Day 73):** `ddrm_plan_runner::open_drm_plan(plan, &mut
  CapabilityTable)` is the single entrypoint the trusted runtime calls — it parses the plan,
  resolves each required provider's handle from a runtime-supplied `CapabilityTable` (the analogue
  of PC2's backend-keyed `getSessionView` factory) at ONE point via `RuntimeStepRunner::resolve_from`,
  builds the runner, and executes. Fail-closed: parses before touching the table (a bad plan never
  reaches the runtime's capabilities), fails closed when the table withholds a required provider
  (zero step invocations), and rejects a misrouting table. The consumer smoke supplies a
  `SmokeCapabilityTable` and calls `open_drm_plan` for both the canonical open and the tampered-edge
  re-run — same entrypoint, no second code path. Mirrors PC2's composition root
  (`BackendSessionService.ts:368` factory → `secureViewSession.ts:124`/`:129` → `media.ts:481`).
  ddrm-plan-runner 21→25.
- 🟩 **Runtime-owned capability registry (Day 74):** `RuntimeCapabilityTable` (in `ddrm-plan-runner`)
  is the concrete `CapabilityTable` the trusted core owns — a registry of runtime-owned
  `ProviderTransport`s. The runtime `register`s one transport per provider (at startup); `resolve`
  opens a FRESH `ProviderHandle` over the registered transport, or `None` for an unregistered
  provider (→ `open_drm_plan` fails closed). `ProviderTransport` (long-lived, owned, registered once)
  vs `ProviderHandle` (per-open) mirrors PC2's `sessionService` singleton owning per-backend view
  constructors (`BackendSessionService.ts:495`/`:368`) and minting a `BackendSessionView` per request.
  The consumer smoke registers three capsule-backed transports into the SAME registry. ddrm-plan-runner
  25→29.
- 🟩 **Runtime-core trusted host (Day 75–76):** `ddrm_plan_runner::DrmHost` is the single owned
  entrypoint that composes the WHOLE open — it owns a `PlanSource` (the seam to ask `drm-provider`
  for the plan), the Day-74 `RuntimeCapabilityTable`, and a `RuntimeEventSink`. `host.open(content_id,
  viewer)` fetches the plan, drives it through the registry (`open_drm_plan`'s parse→resolve→execute),
  then emits the plan's runtime-OWNED post-steps (`release_receipt` + the open `audit`) in order via the
  sink. New `PlanStep.event` + `is_runtime_event()` (no provider, carries an `event`) lets the host emit
  the steps the executor only walks for ordering. Fail-closed at every seam: a bad plan never resolves a
  capability, a missing transport fails closed, and a runtime event the sink cannot emit fails the open.
  Mirrors PC2's server-owned `/init` route (`media.ts:133` route → `:481`/`:482` recover → `:489`
  session create → `:528` catch): plan-fetch + drive-over-capability + session/audit, owned in one place.
  The consumer smoke is now a THIN caller — registers the three transports + a `SmokePlanSource`
  (real `drm-provider`) + a `SmokeEventSink` into a `DrmHost` and calls `host.open`; the tampered-edge
  gate flips the plan source into tamper mode and re-opens through the SAME host. ddrm-plan-runner 29→34.
- 🟩 **Host owns the rail + persists the open (Day 77–78):** `DrmHost` gained (1) host-owned transport
  TEARDOWN — `ProviderTransport::shutdown` (the analogue of PC2's `ISessionView.dispose()` →
  `requestDrop`, `chipotle-client.ts:694`–`:698`/`:231`) + `RuntimeCapabilityTable::shutdown` (tears down
  ALL transports, best-effort then surfaces the first error) + `DrmHost::shutdown(self)` (consumes the
  host) — the runtime that OWNS the transports owns their teardown, fail-closed; and (2) a PERSISTING
  event sink — an `EventStore` seam (`persist(key, record)`) + `PersistingEventSink` that builds a
  CEK-FREE record via `open_event_record` (event + open identity + `steps_run` + `decrypt_session_opened`
  + artifact NAMES, never artifact VALUES) and writes one per runtime event; a store that cannot persist
  a declared event fails the open. Mirrors PC2 persisting the open as a lifetime-managed session
  (`mediaSessionManager.create` `sessionManager.ts:50`–`:80`, TTL/`cleanup`/`destroy` `:104`–`:123`,
  CEK server-side + out of the record `:5`–`:18`). +4 tests → ddrm-plan-runner 34→38. The consumer smoke
  shrinks further: the transports OWN their capsules and `host.shutdown()` tears down the whole rail (no
  manual per-capsule shutdown), and the sink is `PersistingEventSink` over a `FileEventStore` writing
  durable CEK-free records the smoke reads back (asserting no CEK/ciphertext/key leak).
- 🟩 **Host launches the rail + a production-shaped durable store (Day 79–80):** `ddrm-plan-runner` gained
  (1) a `ProviderLauncher` seam (`launch(self) -> Box<dyn ProviderTransport>`) + `RuntimeCapabilityTable::from_launchers(launchers)`
  — the HOST brings the rail up by LAUNCHING each provider (spawn → init → the provider PUBLISHES its
  material) in caller-supplied dependency order, registering each transport, and fail-closed tearing down a
  partially-launched rail if any launch fails (the analogue of PC2's `BackendSessionService.createSession`
  launching a backend view via `WasmSessionView.createNew()`, which mints + publishes the session key
  inside the runtime, `chipotle-client.ts:603`–`:613`/`BackendSessionService.ts:307`); and (2) a
  production-shaped `DurableEventStore` (impl `EventStore`) — ATOMIC write (`*.tmp` then `rename`), stable
  layout keyed by `content_id/event`, idempotent re-persist, fail-closed on I/O error, and a
  `DurableEventStore::load(dir)` read-back returning every record across a FRESH instance (skipping corrupt)
  — mirroring PC2's `FileSessionStore` (one file per id, mode 0600, `loadAll` across a restart skipping
  corrupt, `BackendSessionService.ts:107`/`:140`–`:196`). +5 tests → ddrm-plan-runner 38→43. The consumer
  smoke shrinks again: it hands the host three `ProviderLauncher`s (each owning a capsule BINARY) instead of
  pre-provisioned capsules — `from_launchers` spawns + inits all three, the runtime binds the cross-provider
  open material, and the sink is `PersistingEventSink` over the `DurableEventStore`; durability is proven by
  reading the records back through a FRESH `DurableEventStore::load` (a brand-new reader) asserting no
  CEK/secret leak.
- 🟩 **Stable authority identity + escrow-at-publish (Day 81–82):** the reference key authority gained a
  STABLE, durable-key-store identity, so the producer escrows the CEK at PUBLISH time to a recipient any
  later launch re-derives identically — collapsing the Day-79/80 "launch → publish → escrow → bind" dance.
  Three seams: (a) `ddrm-envelope` DETERMINISTIC key derivation — `mint_session_from_seed(seed)` (ML-KEM-768
  `generate_deterministic(d,z)` + x25519 from-seed via domain-separated SHA-256 sub-seeds, NO RNG,
  byte-identical), `derive_seed(master,label)`, `random_seed()`; +2 tests (14→16); (b) `key-provider`
  reference authority DURABLE KEY STORE — `init.config.authority_key_store` (a path) loads-or-creates +
  atomically persists (`*.tmp`→`rename`, 0600) ONE 32-byte master seed and re-derives BOTH the ML-DSA signer
  and the KEM recipient from it, so the published recipient is STABLE across processes (FAIL-CLOSED on a
  corrupt store — never a silent re-mint, which would strand every CEK escrowed to the prior recipient; the
  dev default with no store still mints fresh per init); +2 tests (33→35); (c) `ddrm-plan-runner`
  `DrmHost::launch(plan_source, launchers, events)` — the trusted-core composition helper that brings up its
  OWN rail (`from_launchers`) + wires the sink in one call; +2 tests (43→45). Mirrors PC2's stable
  `DEFAULT_AUTHORITY` (baked into every video's PSSH at encode time, `dashPackager.ts:44`) vs the per-open
  `WasmSessionView` session key, and PC2 escrowing the CEK to that stable authority at encode time
  (`encryptMediaCEK(cek,kid) → authority: DEFAULT_AUTHORITY`, `dashPackager.ts:131`–`:140`). The consumer
  smoke now runs a PUBLISH phase (escrow → durable fixture) then an OPEN phase via `DrmHost::launch` that
  RELAUNCHES the authority from the SAME store, PROVES the recipient is byte-identical across the relaunch,
  READS the fixture (never re-escrows), and binds only the per-open session AAD.
- 🟩 **Config-driven runtime-open `bin` + `dkms` external-authority seam (Day 83–84):** the host bootstrap
  stopped being smoke-owned, and `dkms` stopped being a bare `not_configured`. (a) A NEW default-on
  runtime-core entrypoint `scripts/dev/ddrm-runtime-open` (a `bin`, relocated from `ddrm-consumer-smoke`)
  reads a TYPED JSON CONFIG (`OpenConfig`: provider binaries, work dir, viewer, content id, `mode`), builds
  the trusted `DrmHost` from `ProviderLauncher`s + a `DurableEventStore` via `DrmHost::launch`, runs the
  publish-time escrow fixture, and drives the open — `mode:"open"` is the operator path (publish → launch →
  open → persist → durable CEK-free readback), `mode:"verify"` adds the two adversarial fail-closed gates;
  config fail-closed on a missing path / unreadable file / malformed JSON / missing required binary / unknown
  mode; +5 config-parse tests. `ddrm-consumer-smoke.sh` now just WRITES a config + INVOKES the binary (no
  inline host assembly). (b) `key-provider` `dkms` is now a FAIL-CLOSED EXTERNAL-authority seam —
  `init.config.dkms_authority_descriptor` (a path) RESOLVES the authority's stable signer + KEM recipient
  from a HANDED-IN descriptor (the dKMS-provisioned key material, READ never minted/persisted), VERIFIES the
  resolved identity against the descriptor's published `verifying_key_b64`/`recipient_pub_b64` pins
  (fail-closed on mismatch), and recovers/re-seals through the SAME `SealedDecryptMaterialV1` contract — so
  the durable-key-store stability pattern carries to a NON-reference authority, with the reference store as
  its local fixture; no descriptor → selected-but-unconfigured; corrupt/wrong-schema/mismatched descriptor →
  init fails closed; +2 tests (35→37). Mirrors PC2 booting `sessionService` ONCE from config
  (`BackendSessionService.ts:495`) + resolving the external authority key from config (`resolvePkpId(config)`,
  `chipotle-client.ts:963`–`:967`), not minting.
- 🟩 **`dkms` runs the open end-to-end + a backend swap is invisible (Day 85–86):** the live consumer half
  now runs against the EXTERNAL `dkms` authority, and switching backends is a one-field config change. (a)
  `OpenConfig` gained a typed `authority.backend` (`reference | dkms`; fail-closed on an unknown/non-object
  authority, +2 bin tests); `KeyLauncher` carries only a backend-specific `init_config` and the publish →
  launch → open → recover/re-seal flow is BYTE-IDENTICAL across backends. (b) The publish phase PROVISIONS the
  selected authority — for `dkms` it generates the key material via the reference authority on a durable store,
  then publishes an IMMUTABLE descriptor (master seed + published-identity pins), the dKMS-node provisioning
  analogue. (c) `key-provider` now REQUIRES the dkms descriptor's pins (`verifying_key_b64` AND
  `recipient_pub_b64`); a pinless descriptor fails closed (a real external authority always publishes its
  identity), +1 test (37→38). (d) The bin PROVES the descriptor was READ-ONLY across the open (snapshot before
  launch, byte-compare after shutdown). A new sibling smoke `ddrm-consumer-dkms-smoke.sh` drives the dkms path
  end-to-end (and `ddrm-consumer-smoke.sh [--backend reference|dkms]` runs either). Mirrors PC2's
  `getSessionView(token)` backend dispatch (`BackendSessionService.ts:368`–`:377`, downstream agnostic) +
  treating the provisioned descriptor as immutable published data (`chipotle-client.ts:935`/`:950`).
- ⬜ **Missing middle:** a **REMOTE production EXTERNAL key authority** (`dkms` today is a
  PROVISIONED-DESCRIPTOR seam — the runtime is handed the authority's key material; a true remote dKMS would
  resolve PUBLIC-only keys + DELEGATE recovery to the external node / threshold-HSM, never holding the secret;
  `lit` compat is still `not_configured`); the **producer side** actual IPFS **pin** (the availability receipt
  is handed in today; pinning stays a separate, later capability) and a multi-block (>1 MiB) `payload_cid` for
  large assets; a **viewer**.

**Phase A is underway** (`SYSTEM_ARCHITECTURE_MAP.md §6`): Day 50 made `key-provider`
a pluggable multi-backend authority (A.1); Day 51 landed the **reference seal engine**
+ the shared **`ddrm-envelope`** crate (A.2) — the reference authority now seals a CEK
to a decrypt session's published key and the decrypt boundary's exact unwrap opens it
(cross-boundary proof). Day 52 landed the **cross-capsule equivalence guard** (A.3,
feature `envelope-conformance`): the shared crate's seal proven wire- AND
crypto-interoperable with `decrypt-provider`'s unwrap. Day 53 then **completed the dedup**
(A.3b): `decrypt-provider::pq_envelope` deleted its in-tree PQ crypto (~370 lines) and now
re-exports `ddrm-envelope` under the historical `crate::pq_envelope::*` paths — so the PQ
crypto lives in **exactly one place** and cannot drift. Pure refactor: all 22 ladder combos
kept their exact counts, goldens replayed byte-identically, wasm clean, drift PASS; the
shared crate widened its surface (`pub signed_payload`, raw-type re-exports) and the
redundant `x25519-dalek`/`aes-gcm` deps were pruned from `decrypt-provider`. Day 54 then
lifted the **decrypt-transcript `to_aad` into `ddrm-envelope::transcript`** (A.4, part 1):
the AAD field set + encoder now lives once, so the key authority computes the SAME binding
it seals to and the decrypt boundary rebuilds. Day 55 then **closed Phase A.4**: a new
`scripts/ddrm-consumer-smoke.sh` + standalone dev orchestrator
(`scripts/dev/ddrm-runtime-open`, relocated from `ddrm-consumer-smoke` in Day 83–84) builds the REAL capsule binaries and drives
`drm/open → rights → key (reference) → decrypt (OpenSessionV1)` end to end — the authority
publishes its vk, the boundary mints+publishes a session key, the authority seals the golden
CEK to it transcript-bound (shared encoder), the boundary unwraps in-VM and decrypts a real
CENC segment, returning only a scoped session (no CEK/plaintext on any wire); a mismatched
transcript fails closed. To enable it, the reference `key init` now publishes its verifying
key (`key-authority-ref`=25) and `release_receipt_hash` moved into the shared crate
(`ddrm-envelope` lib=12; decrypt byte-identical). **Phase A (consumer half) is now runnable.** Day 56 then started **Phase B**: behind a
`chain-rights` dev profile, `rights-provider` consumes the typed
`chain-provider::has_access_by_content_id` answer (injected by the runtime core — it holds
no chain-RPC capability), binds it to the request, and emits a `RightsDecisionReceiptV1`;
the consumer smoke now drives that real decision (mocked-owned attestation) and uses the
receipt to gate the key release, proving `rights(allowed) → key → decrypt`
(`rights-provider`=9 default unchanged, `chain-rights`=17). Next, in order:
- **Phase B (cont.)** — drive `chain-provider` against live Base (funded wallet holding an
  Elacity access token) so the ownership answer is real, not mocked.
- **Phase C (underway)** — the producer half. Days 58–60 landed the crypto half (KID ==
  on-chain `bytes16` contentId, the fail-closed CEK-escrow seam + real engine, and the
  **cross-binary producer smoke** `encrypt(seal_inline) → key(release_from_escrow_ref) →
  decrypt` — a CEK sealed *now* decrypts *now*, no raw CEK/plaintext on any wire,
  `scripts/ddrm-producer-smoke.sh`). Day 61 started the on-chain half: a fail-closed
  `publish-provider` capsule ASSEMBLES the mint (binds `contentId == bytes16 KID`, derives
  `tokenURI = {metadataCid}/metadata.json`, emits an unsigned `UnsignedMintV1` for
  `chain-provider`+`wallet-provider`; holds no RPC/keys; publish=16). Day 62 made the mint
  REAL CALLDATA: `chain-provider::assemble_mint` (pure, no RPC/keys) ABI-encodes the PC2
  `mint(string,uint16,bytes,bytes)` call and returns `{to,data,value}` for the existing
  `prepare_transaction`→wallet-sign→`broadcast_transaction` seam (calldata decoded back to
  spec, `mint*` rung=10). Day 63 CLOSED the producer→chain loop: `publish-provider`'s
  `UnsignedMintV1` now emits PC2-faithful STRUCTURED `op_raw`/`sell` (creator/royalty payee
  arrays, BUY_AND_RESELL distribution-right + resellerCut) that drop STRAIGHT into
  `assemble_mint`; `scripts/ddrm-publish-smoke.sh` drives the REAL `publish → chain`
  binaries so one identity flows KID → contentId → mint calldata, no signing/RPC in the
  assembler. Day 64 made it DISCOVERABLE: a fail-closed `content-market` capsule
  reconstructs a `ContentListingV1` PURELY from the self-describing mint calldata
  (`content_id == bytes16 KID`, tokenURI→metadataCID, opType, sell terms; no RPC/IPFS/keys,
  mints nothing); `scripts/ddrm-market-smoke.sh` drives the REAL `publish → chain →
  content-market` so the listing's `content_id` IS the producer's KID. Day 65 gave the
  listing its card: `content-market::enrich_listing` fuses a resolved `metadata.json`
  (name/poster/mime/contentCID/asset-class) onto the calldata identity but REJECTS any
  metadata whose `kid != content_id` (`identity_mismatch`) — metadata describes, never
  re-identifies (a hardening over PC2, which trusts `metadata.kid`); still fetches nothing.
  Day 66 added `listing_from_event`: a PC2 `DigitalAssetRegistered`/`AssetCreated` log →
  `ContentListingV1` (DAR carries on-chain `bytes16 contentId` ≡ the calldata identity; AC
  defers to `needs_kid`); pure decode, log handed in by `chain-provider` (content-market=29).
  Next: a live-Base read-only round trip (real logs → reconstruct → enrich), a live
  producer→consumer round trip, then real `plaintext_ref`→IPFS.
- **Phase A follow-ups** — a `wasm32-wasip1` variant of the smoke (today native).

Still **blocked on others** (parallel): fold `SealedDecryptMaterialV1` into the shared
`elastos-common` contract (needs push access); production dKMS (Anders/dKMS team).

Whatever you pick: keep it isolated on `feat/decrypt-provider-cenc`, pin it with
characterization tests, keep the gate green (`scripts/ddrm-verify.sh` + the ladder),
update `DDRM_STATUS.md`, and end the day by presenting the next 10/10 prompt.
