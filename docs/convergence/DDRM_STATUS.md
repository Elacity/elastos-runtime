# dDRM chain — status & review package

**Branch:** `feat/decrypt-provider-cenc` (based on `origin/0.4.0`, **~97 commits**, tip Day-101–102 — **the live 2-of-2 threshold is now RESILIENT + IDENTITY-BOUND: the production `DrmHost` rail provably FAILS CLOSED under a real node fault (either secret-holder down) — no partial CEK, no single-node fallback, no record persisted — and a silently SWAPPED node-set is DETECTED before the rail recovers anything.** Audited PC2 first — PC2's run-path resilience STOPS at retrying the whole opaque Lit RPC (`chipotle-client.ts:575`: `RequestExpired` → "retry by re-running the Lit action"); a downed node lives INSIDE Lit's network, so PC2 has NO per-node fault semantics and NO inspectable node-set identity it can pin. The runtime is SUPERIOR — it owns the two nodes and expresses both. New pure `ddrm_envelope::threshold_node_set_id(t, vk_a, vk_b)` (domain-separated, length-prefixed SHA-256 over both vks + `t`; order-sensitive; 23→24) is the single source of truth for "which secret-holders back this rail": `publish_escrow` PINS it into the durable fixture (`node_set_id_b64`), and `host.open()` RE-DERIVES it from the published descriptor's `threshold` block + FAILS CLOSED on a mismatch (a node re-pointed at a different secret-holder is caught BEFORE recovery). Verify-mode threshold gates 23–24: with the full 2-of-2 rail up, KILLING node B's daemon → `host.open()` fails closed (no partial CEK, no single-node fallback, no record persisted); KILLING node A's → same; node A is restarted so the downstream socket probes still run (the daemon guards are now `mut` so the gates kill+restart them). Gate 25: a descriptor whose node B is swapped to a rogue identity re-derives to a DIFFERENT node-set-id than the pin — detected end-to-end (and the boundary independently rejects the rogue's seal under node B's pinned vk, Day 97–98 step 20, so the swap fails at BOTH layers). Gate: ladder INTACT (ddrm-envelope=24, key-provider[key-authority-ref]=43, decrypt-provider rail-material=68), drift PASS, all dDRM smokes green (reference + dkms single-node + dkms 2-of-2 with the new node-fault + swap gates), clippy clean. Earlier tip Day-99–100 — **the 2-of-2 threshold now runs through the PRODUCTION `DrmHost` run-path (not just the verify-mode probe): the happy open itself provisions TWO secret-holding nodes, XOR-splits the CEK at publish, dual-recovers BOTH, and reconstructs the CEK ONLY inside the decrypt boundary — never whole before the boundary.** Audited PC2 first — PC2's run-path (`recoverCEKEnvelope` → ONE Lit RPC) NEVER orchestrates multiple nodes in its own code (`decryptAndCombine` is the legacy Datil threshold inside Lit's opaque network, `chipotle-client.ts:1297`; the current Chipotle path is single-node TEE decrypt); PC2's runtime STOPS at one RPC — the runtime is SUPERIOR, driving TWO owned nodes end to end. `OpenConfig.authority.threshold` (bool, dkms-only, fail-closed otherwise; +2 bin tests 8→10) promotes the open to 2-of-2; `publish_escrow` provisions node A + node B (distinct stores/sockets/allow-lists), `split_cek_xor`s the CEK (share-1→A, share-2→B; neither sees the whole key), and publishes a `threshold` descriptor; the `DrmHost` starts BOTH daemons, binds share-2 + node B's vk (`authority_vk2_b64`), and `KeyHandle` supplies `wrapped_cek_share2_b64` so `host.open()` drives the full dual-recover + in-VM combine. Integration fix: `merge_threshold_material` now welds node B's share into node A's NESTED `material.sealed_cek_share2_b64` (the shape the boundary consumes) — the Day 97–98 merge read a top-level field the real node never emits, so it was never exercised end-to-end (key-provider[key-authority-ref] stays 43). Verify-mode gates 21–22 (threshold-only): the live rail refuses a one-share release (never degrades to a single node) + a 3-of-N descriptor fails closed at init. New `ddrm-consumer-dkms-threshold-smoke.sh` (+ `--threshold` flag) drives the whole 2-of-2 open cross-binary. Gate: ladder INTACT (ddrm-envelope=23, key-provider[key-authority-ref]=43, decrypt-provider rail-material=68), drift PASS, all dDRM smokes green (reference + dkms single-node + the NEW dkms 2-of-2), clippy clean. Earlier tip Day-97–98 — the threshold crypto is REAL: the CEK is XOR-split 2-of-2 across TWO secret-holding dKMS nodes so NO single node ever holds the whole content key; the runtime reconstructs it ONLY inside the decrypt boundary. Audited PC2 first — PC2's threshold is the OPAQUE Lit `decryptAndCombine` (`non-media-decrypt.js:76`): the share set + nodes + combine live INSIDE Lit's proprietary network, uninspectable; the runtime is SUPERIOR — an EXPLICIT, owned, inspectable 2-node split with the combine in our OWN sandbox. `ddrm-envelope` gained pure `split_cek_xor`/`combine_cek_xor` (22→23). `decrypt-provider` reconstructs IN-VM via `rail_shim::decrypt_from_carrier_threshold` — `SealedDecryptMaterialV1` gained optional `sealed_cek_share2_b64`, the boundary an optional `authority_vk2`; both sealed shares are unwrapped (each under ITS node's vk, same transcript) and XOR-combined in `Zeroizing` before decrypt — the whole CEK exists ONLY in the sandbox, never in `key-provider`; single-share path unchanged (rail-material 65→68: +happy 2-of-2, +unauthorized-second-share denied, +missing-vk2 fail-closed). `key-provider` REPLACED the Day 95–96 stub: `build_dkms_client` resolves a 2-of-2 `threshold` descriptor into TWO public clients (3-of-N/identical/malformed fail closed); `release` dual-recovers BOTH nodes (per-node connection, known-caller, fresh `recover_seq`, possession proof), and `merge_threshold_material` welds two re-sealed shares into one material WITHOUT XOR-combining (42→43). `ddrm-runtime-open` verify mode adds a 2-of-2 probe (steps 18–20): TWO real node daemons (distinct stores/sockets/allow-lists), share-1→node A + share-2→node B, recover from EACH, reconstruct the EXACT CEK in-boundary — proving a single share is USELESS and a FORGED second share fails closed under node B's vk. Gate: ladder INTACT (ddrm-envelope=23, key-provider[key-authority-ref]=43, decrypt-provider rail-material=68), drift PASS, all dDRM smokes green (incl. the dkms 2-of-2 probe), clippy clean. Escape hatch (per the 2-day prompt): the production `DrmHost` run-path live dual-recover + its dedicated end-to-end smoke is the Day 99–100 finisher; this cycle landed the full producer split + two-daemon provisioning + `key-provider` dual-recover orchestration + the real in-VM reconstruction, proven cross-binary. Earlier tip Day-95–96 — the dkms node now serves only a KNOWN, ALLOW-LISTED caller and every recover is FRESH (anti-replay), and the runtime recognizes a THRESHOLD descriptor fail-closed. Day 93–94 gave the node a real transport + a non-replayable bearer session; Day 95–96 hardens WHO it serves and makes each recover single-use. **(known-caller)** the node now takes an OPERATOR-provisioned allow-list of caller verifying keys (`DKMS_AUTHORITY_ALLOWED_CALLERS`, set by the provisioner who launches the daemon, never overridable by the connecting client); `hello` REFUSES a caller whose ephemeral identity key is not on the list (`caller_not_authorized`) BEFORE minting any token — the runtime-core analogue of PC2's session being OWNER-BOUND to a registered wallet (`secureViewSession.ts:87`–`:100`); with no allow-list the node stays anonymous (dev/test). The runtime (`key-provider`) connects as its OWN stable caller identity derived from a per-run seed the runtime provisions into the node's allow-list (the rail AND the adversarial probe both derive that one identity; the identity key is the runtime's own — never the dKMS master or a CEK). **(anti-replay)** the possession proof now binds a per-recover FRESHNESS counter (`recover_seq`), and the node tracks the highest `recover_seq` it has consumed in the session, REFUSING any recover that does not STRICTLY advance — so a captured recover frame replayed verbatim is refused even by the legitimate caller (the analogue of PC2's revocable per-delegation `nonce`, `secureViewSession.ts:108`–`:112`); the counter commits only on a successful recover. **(threshold seam)** `key-provider` now RECOGNIZES a `threshold` descriptor (`t>1` or more than one node) and FAILS CLOSED (`threshold dKMS is not yet implemented`) rather than silently recovering from one node and pretending it was a threshold — the real 2-of-N CEK-share split across multiple secret-holding nodes is the next cycle; a single-node (`t==1`/absent) descriptor still resolves. Gate: ladder INTACT (ddrm-envelope=22, dkms-authority=11→13, key-provider[key-authority-ref]=41→42), drift PASS, all dDRM smokes green (incl. the dkms variant — allow-list enforced + unknown-caller refused + replayed recover refused), clippy clean. Earlier tip Day-93–94 — the long-lived dkms node now has a REAL TRANSPORT BOUNDARY and the bearer session is NON-REPLAYABLE across callers, closing the two seams Day 91–92 deferred. The node is no longer a stdin/stdout CHILD the `key-provider` spawns: it BINDS + LISTENS on a Unix-domain socket and serves a length-prefixed FRAMED request/response (the SAME JSON ops — a transport swap, not a protocol change), many SEQUENTIAL connections with ONE handshake session per connection, and a torn / oversized / half-closed frame fails closed WITHOUT wedging the daemon (the connection is dropped, the listener serves on). The runtime now CONNECTS to a node whose process it does NOT own (`ddrm-runtime-open` starts the node DAEMON listening before the rail comes up + reaps it after). And the session is no longer a pure BEARER credential: the client mints an EPHEMERAL keypair per connection, sends its public half at `hello`, the node BINDS the session token to that pubkey (signs `challenge‖caller_pub‖expires_at`), and every `recover` REQUIRES a signature under the matching PRIVATE key that the node verifies against the token-bound pubkey — so a token captured + replayed by a DIFFERENT caller (no private key) or signed by the WRONG key is refused (the runtime-core analogue of PC2's session being OWNER-BOUND — the bearer token alone is insufficient, the owner is re-checked in the TEE via `ecrecover(delegationSig)`, `secureViewSession.ts:87`–`:100`). A NEW shared FRAME module in `ddrm-envelope` — `frame::{write_frame, read_frame}`, `[4-byte BE length][payload]`, `MAX_FRAME_BYTES = 1 MiB`, fail-closed on torn/oversized/zero-length (the runtime-core analogue of PC2's Boson proxy framing `[2-byte length][1-byte type][body]` + `MAX_PACKET_SIZE`, `ProxyProtocol.ts:13`/`:251`/`:256`/`:371`) — plus a caller-bound session token (`sign_session_token`/`verify_session_token` now bind `caller_pub`) and a recover possession-proof primitive (`sign_recover_proof`/`verify_recover_proof` over the session challenge + the recover's content binding), all single-source-of-truth so node + client cannot drift (ddrm-envelope 20→22). The node serves the framed socket + binds the token to the caller pubkey at hello + verifies the possession proof on every recover (dkms-authority 9→11: framed full-session round-trip + torn-frame drop, possession gate refusing no/wrong-key proof). The client CONNECTS over the framed socket instead of spawning, mints the ephemeral keypair, and signs every recover under it (key-provider[key-authority-ref]=41, the transport swapped + the conn boxed; the socket path is `unix`-gated so the wasm32-wasip1 ladder build stays clean). `ddrm-runtime-open` proves it cross-binary against the REAL daemon (verify steps 13–17: identity pinned over the socket + a CALLER-BOUND token minted; recover with NO/EXPIRED/FORGED/tampered token, NO possession proof, and a WRONG-KEY proof ALL refused; re-auth still refuses denied/wrong-content even WITH a live session+proof; ONE socket connection+session → THREE successful recovers; a torn AND an oversized frame each fail closed without wedging the daemon, a clean session afterwards still succeeds). Gate: ladder INTACT (ddrm-envelope=22, dkms-authority=11, key-provider[key-authority-ref]=41), drift PASS, all dDRM smokes green (incl. the dkms variant), clippy clean. Earlier tip Day-91–92 — the dkms delegation is now a LONG-LIVED NODE CONNECTION + a node-bound SESSION the node REQUIRES on every recover: the `key-provider` dkms client opens the node ONCE, proves its pinned identity ONCE, and REUSES the connection + a node-signed session across many releases (re-establishing fail-closed only when the session expires) instead of spawning + re-handshaking a fresh node PER release; the node's `hello` now mints a node-SIGNED SESSION TOKEN (binding the client's challenge + a bounded expiry) and `recover` REQUIRES a live, node-verified token — fail-closed on a missing / expired / forged / tampered token — verified in the node's OWN boundary BEFORE re-authorization and BEFORE any key material, so a captured/forged handshake can't drive recovery and a token minted for one challenge can't authorize a recover under a tampered challenge/binding (the runtime-core analogue of PC2's per-view session ESTABLISHED ONCE + RESURRECTED per request to gate recovery, refused without a live session, `secureViewSession.ts:81`–`:128`). A NEW domain-separated session-token primitive in `ddrm-envelope` — `sign_session_token`/`verify_session_token` over `DKMS_SESSION_DOMAIN ‖ challenge ‖ expires_at` (`elastos.dkms.authority/session/v1`, separated from the hello attestation + the CEK seals), the single source of truth so node + client cannot drift (ddrm-envelope 18→20) — backs the node's token mint + verify (dkms-authority 6→9: session issued at hello + required/verified/expiry-checked at recover, no-token refused at the protocol, one session authorizes many recovers) and the client's open-once/verify-once/reuse decision (key-provider 40→41, +`dkms_session_live` reuse gate). `ddrm-runtime-open` proves it cross-binary against the REAL node (verify steps 13–16: identity pinned + session token minted; recover with NO/EXPIRED/FORGED/tampered token refused; re-auth still refuses denied/wrong-content even WITH a live session; ONE session → THREE successful recovers, raw CEK never present), with the master never re-crossing the boundary. Gate: ladder INTACT (ddrm-envelope=20, dkms-authority=9, key-provider[key-authority-ref]=41), drift PASS, all dDRM smokes green (incl. the dkms variant), clippy clean. Earlier tip Day-89–90 — the delegation is now an AUTHENTICATED CHANNEL with a per-recover AUTHORIZATION: the `dkms` client PINS the node's published verifying key and VERIFIES a fresh hello ATTESTATION before it delegates anything (a forged/mismatched node is refused at the handshake), and the node RE-CHECKS the rights authorization in its OWN boundary (refusing a denied or content/principal-mismatched receipt) before touching key material — closing the gap between "spawned child it trusts implicitly" and "remote dKMS over an authenticated channel." A NEW domain-separated attestation primitive in `ddrm-envelope` — `attest_challenge`/`verify_attestation` over `DKMS_HELLO_DOMAIN ‖ challenge`, the single source of truth so node + client cannot drift (ddrm-envelope 16→18) — backs a `hello` op on the node (signs the client's fresh challenge with its master-derived signing key, proving possession of the key BEHIND the published vk; dkms-authority 4→6, +recover re-auth) and the client's pre-recover verify (`key-provider` 39→40, +handshake-verify fail-closed). `ddrm-runtime-open` proves it cross-binary against the REAL node (verify mode steps 13+14: a flipped vk + a replayed challenge rejected at the handshake; a DENIED / wrong-content receipt refused by the node), with the master never on the wire. The runtime-core analogue of PC2 PINNING the Lit network identity AND the Lit action re-running `hasAccessByContentId` in the TEE rather than trusting the caller (`universal-decrypt-chipotle.js:560`–`:568` / `:577`–`:590`). Gate: ladder INTACT (ddrm-envelope=18, dkms-authority=6, key-provider[key-authority-ref]=40), drift PASS, all dDRM smokes green (incl. the dkms variant), clippy clean. Earlier tip Day-87–88 — the `dkms` authority is now SPLIT into a SECRET-HOLDING NODE and a PUBLIC-ONLY runtime, and recovery is DELEGATED across the process boundary — the first real step from "provisioned-descriptor seam" to "remote dKMS." A new `dkms-authority` capsule (the node) OWNS the master key material (its own node-local durable store) and exposes ONLY a `recover` op: given the rights-validated escrow blob + the decrypt session's published key + transcript, it recovers the CEK INSIDE its own boundary and returns the `SealedDecryptMaterialV1` re-sealed to the session — NEVER the CEK, NEVER the master (node tests=4). `key-provider`'s `dkms` backend now holds a PUBLIC-ONLY descriptor (`verifying_key_b64` + `recipient_pub_b64` + `authority_endpoint`, schema v2; a master-seed-bearing descriptor is REJECTED fail-closed) and on `release` DELEGATES recovery to the node over the capsule protocol (spawn + JSON-RPC the granted endpoint) instead of deriving the secret locally — so the runtime holds NO recovery secret and a leaked descriptor recovers nothing (+1 test, 38→39). `ddrm-runtime-open` threads it end-to-end: the publish phase PROVISIONS the node (the master stays in the node's own store; the runtime gets only the public descriptor + delegates), and the bin ASSERTS the descriptor handed to the runtime is PUBLIC-ONLY (no master seed; vk + recipient + endpoint present) — proving the master NEVER crosses into the runtime (+1 bin test, 7→8). The dkms smoke decrypts the segment with the master seed never entering the runtime; the reference path stays byte-identical green. The runtime-core analogue of PC2's Lit/dKMS node recovering the CEK INSIDE the TEE (`Lit.Actions.Decrypt` → `envelopeCEK` seal-to-session → `setResponse` returns only the envelope, `universal-decrypt-chipotle.js:572`/`:602`/`:610`) and the client holding only the PUBLIC `pkpId`/`authority` and RPCing the network for recovery (`recoverCEKEnvelope`, `chipotle-client.ts:1438`), never the recovery secret. Gate: ladder INTACT (+dkms-authority=4, +key-provider[key-authority-ref]=39), drift PASS, all dDRM smokes green (incl. the dkms variant), clippy clean. Earlier tip Day-85–86 — the `dkms` EXTERNAL authority ran the full open END-TO-END through the live rail, and a backend SWAP was invisible to the open. `ddrm-runtime-open`'s `OpenConfig` gained a typed `authority.backend` (`reference | dkms`); the publish phase PROVISIONS the selected authority (for `dkms`, it generates the key material on a durable store then publishes an IMMUTABLE descriptor — master + published-identity pins — the dKMS-node analogue), and the open RESOLVES that backend — so the SAME binary runs the open against `reference` OR `dkms` with a ONE-FIELD config change and a byte-identical flow (`KeyLauncher` carries only a backend-specific `init_config`; everything downstream is backend-agnostic). `key-provider` now REQUIRES the dkms descriptor's published-identity pins (`verifying_key_b64` + `recipient_pub_b64`) — a pinless descriptor fails closed (a real external authority always publishes its identity) — and the bin PROVES the descriptor was READ-ONLY across the open (immutable published data, the key-provider only ever reads it). A new sibling smoke `ddrm-consumer-dkms-smoke.sh` drives the dkms path end-to-end (publish→resolve→recover→decrypt + the immutability proof); the reference path stays green. The runtime-core analogue of PC2's `getSessionView(token)` dispatching on `stored.backend` (`BackendSessionService.ts:368`–`:377`) while the downstream open is backend-agnostic, and PC2 treating the provisioned authority descriptor as immutable published data (cached once + only read, `chipotle-client.ts:935`/`:950`). +1 key-provider test (37→38), +2 bin config-parse tests. Gate: ladder INTACT (+key-provider[key-authority-ref]=38), drift PASS, all dDRM smokes green (incl. the new dkms variant), clippy clean. Earlier tip Day-83–84 — the runtime open now BOOTS FROM CONFIG with NO smoke in the loop, and the `dkms` EXTERNAL-authority backend resolves a STABLE identity from a HANDED-IN descriptor. Two seams: (a) a new default-on runtime-core entrypoint `scripts/dev/ddrm-runtime-open` (a `bin`) constructs the trusted `DrmHost` from `ProviderLauncher`s + a `DurableEventStore` via `DrmHost::launch` and drives the open reading a TYPED JSON CONFIG (`OpenConfig`: provider binaries, work dir, viewer, content id, `mode`) — `ddrm-consumer-smoke.sh` shrinks to WRITING a config + INVOKING that binary (no inline host assembly; `mode:"verify"` additionally drives the two adversarial fail-closed gates), +5 config-parse fail-closed tests in the bin; (b) `key-provider` promotes `dkms` from `not_configured` to a FAIL-CLOSED EXTERNAL-authority seam — `init.config.dkms_authority_descriptor` (a path) RESOLVES the authority's stable ML-DSA signer + KEM recipient from a HANDED-IN descriptor (the dKMS-provisioned key material, never minted/persisted), VERIFIES the resolved identity against the descriptor's published `verifying_key_b64`/`recipient_pub_b64` pins (fail-closed on mismatch), and recovers/re-seals through the SAME `SealedDecryptMaterialV1` contract — so the durable-key-store stability pattern carries to a NON-reference authority; selected-but-unprovisioned (no descriptor) keeps the "no dKMS node provisioned" fail-closed surface; +2 tests (35→37). The runtime-core analogue of PC2 booting `sessionService` ONCE from config (`new BackendSessionService(new FileSessionStore(SESSION_STORE_DIR))`, `BackendSessionService.ts:495`) rather than per request, and resolving the EXTERNAL authority's key from config (`resolvePkpId(config)` → `config.pkpId`/auto-provisioned/`DEFAULT_PKP_ID`, `chipotle-client.ts:963`–`:967`) rather than minting it. Gate: ladder INTACT (+key-provider[key-authority-ref]=37), drift PASS, 4 smokes green, clippy clean. Earlier tip Day-81–82 — the key authority now has a STABLE, PUBLISHED-ONCE identity backed by a DURABLE KEY STORE, so the producer ESCROWS the CEK at PUBLISH time to a recipient that any later authority launch re-derives identically — collapsing the Day-79/80 "launch → publish → escrow → bind" dance into "escrow at publish; launch resolves the same recipient." Three seams: (a) `ddrm-envelope` gained DETERMINISTIC, from-seed key derivation — `mint_session_from_seed(seed)` (ML-KEM-768 `generate_deterministic(d,z)` + x25519 from-seed, domain-separated sub-seeds, NO RNG, byte-identical every call), `derive_seed(master,label)` (`SHA-256(label‖master)`, domain-separated), `random_seed()` (one-time master from `OsRng`) — +2 tests (14→16); (b) `key-provider` reference authority gained a production-shaped DURABLE KEY STORE — `init.config.authority_key_store` (a path) makes it load-or-create + atomically persist (`*.tmp`→`rename`, mode 0600) ONE 32-byte master seed, then re-derive BOTH the ML-DSA signer and the KEM recipient from it, so the published recipient is STABLE across processes; fail-closed on a corrupt store (never a silent re-mint, which would strand every CEK escrowed to the prior recipient); the dev default (no store) still mints a fresh recipient per init — +2 tests (33→35); (c) `ddrm-plan-runner` gained `DrmHost::launch(plan_source, launchers, events)` — the trusted-core composition helper that brings up its OWN rail (`from_launchers`) and wires the sink in ONE call, so a caller hands the host launchers + a sink rather than assembling the table — +2 tests (43→45). The runtime-core analogue of PC2's stable `DEFAULT_AUTHORITY` (the AuthorityGateway address baked into every video's PSSH at encode time, kept in lock-step across `storage.ts`/`chipotle-client.ts`/`dashPackager.ts:44`) vs the per-open `WasmSessionView` session key, and PC2 escrowing the CEK to that stable authority at encode time (`encryptMediaCEK(cek,kid) → authority: DEFAULT_AUTHORITY`, `dashPackager.ts:131`–`:140`). `ddrm-consumer-smoke.sh` now runs a PUBLISH phase (the producer role) that brings the durable-key-store authority up ONCE, escrows the CEK to its stable recipient, and writes a durable publish fixture; the open phase builds the host via `DrmHost::launch`, RELAUNCHES the authority from the SAME store, PROVES the recipient is identical across the relaunch, READS the fixture (never re-escrows), and binds only the per-open session transcript AAD. Drift untouched. Earlier tip Day-79–80 — the trusted host now LAUNCHES THE RAIL + PERSISTS THROUGH A PRODUCTION-SHAPED STORE, closing the two "still dev-shaped" gaps: `ddrm_plan_runner` gained (1) a `ProviderLauncher` seam — `RuntimeCapabilityTable::from_launchers(launchers)` BRINGS THE RAIL UP by LAUNCHING each provider (spawn → init → the provider PUBLISHES its material) in caller-supplied dependency order, registering each resulting transport, and FAIL-CLOSED tearing down a partially-launched rail if any launch fails — so the HOST, not the smoke, brings up rights/key/decrypt (the runtime-core analogue of PC2's `BackendSessionService.createSession` launching a backend view via `WasmSessionView.createNew()`, which mints + PUBLISHES the session key inside the runtime, `src/api/chipotle-client.ts:603`–`:613` / `src/services/session/BackendSessionService.ts:307`); and (2) a production-shaped DURABLE `DurableEventStore` (impl `EventStore`) — ATOMIC writes (`*.tmp` then `rename`, no torn record), a stable on-disk layout keyed by `content_id/event`, IDEMPOTENT re-persist, FAIL-CLOSED on any I/O error, and a `DurableEventStore::load(dir)` read-back that returns every persisted record across a FRESH instance/process (skipping corrupt files) — mirroring PC2's `FileSessionStore` (one file per id, mode 0600 `persist`, `loadAll` restoring across a restart + skipping corrupt, `BackendSessionService.ts:107`/`:140`–`:196`). +5 characterization tests (host launches the whole rail then drives + tears it down; `from_launchers` fails closed + tears down the partial rail; the durable store persists + reads back across a fresh instance, atomic with no temp left + idempotent overwrite; load skips a corrupt record; the host persists durably through the real store, read back by a fresh reader) → ddrm-plan-runner 38→43. `ddrm-consumer-smoke.sh` shrinks again: it hands the host three `ProviderLauncher`s (each owning a capsule BINARY) instead of pre-provisioned capsules — `from_launchers` spawns + inits all three (the key authority + decrypt boundary publish their material into a shared rail), the runtime binds the cross-provider open material (producer escrow to the published recipient + transcript AAD over the published session key), and the event sink is the lib's `PersistingEventSink` over the `DurableEventStore`; the smoke proves durability by reading the records back through a FRESH `DurableEventStore::load` (a brand-new reader, as if a separate process) and asserting no CEK/secret leaked. Drift untouched. Earlier tip Day-77–78 — the trusted host OWNS THE RAIL + PERSISTS the open: `ddrm_plan_runner::DrmHost` gained (1) host-owned transport TEARDOWN — `ProviderTransport::shutdown` + `RuntimeCapabilityTable::shutdown` + `DrmHost::shutdown(self)` tear down every runtime-owned transport (each releasing the connection it owns), so the runtime that OWNS the transports owns their teardown, fail-closed (a transport that cannot release surfaces) — and (2) a PERSISTING event sink — a new `EventStore` seam + `PersistingEventSink` write each runtime-event step (`release_receipt` + the open `audit`) as a durable, CEK-FREE record (`open_event_record`: open identity + steps + decision + artifact NAMES, NEVER artifact VALUES / key material) instead of a throwaway in-memory note. Audited PC2's transport lifecycle + persistence first: the per-view transport OWNS a releasable resource and tears it down on `dispose()` — `WasmSessionView.dispose()` calls `requestDrop(this._requestHandle)` then nulls it (`src/api/chipotle-client.ts:694`–`:698`), `dispose()` is part of the `ISessionView` contract (`:231`), opened by `createNew`/`fromStoredSession` (`:603`/`:621`); and PC2 PERSISTS the open as a lifetime-managed session — `mediaSessionManager.create` mints a session with a lifecycle (`src/services/media/sessionManager.ts:50`–`:80`), TTL-expires + `cleanup`/`destroy` tear it down (`:104`–`:123`), a process singleton (`:126`), holding the CEK server-side and OUT of the record it returns (`:5`–`:6`, `:18`). Mirrored: the host owns spawn→use→teardown of each transport, and persists a CEK-free open record per runtime event (the analogue of `mediaSessionManager.create` + the audit log, minus the key material). +4 characterization tests (host shutdown tears down every owned transport; shutdown fails closed when a transport cannot release; the persisting sink writes CEK-free records for every runtime event; the open fails closed when the store cannot persist) → ddrm-plan-runner 34→38. `ddrm-consumer-smoke.sh` shrinks further: the three transports now OWN their capsules (each `shutdown`s the capsule it owns) and `host.shutdown()` tears down the whole rail (no manual per-capsule shutdown), and the event sink is the lib's `PersistingEventSink` over a `FileEventStore` writing durable CEK-free records to a temp dir, which the smoke reads back to prove the receipt + audit persisted without leaking the CEK/ciphertext/keys. Drift untouched. Earlier tip Day-75–76 — the runtime CORE now has a single TRUSTED HOST ENTRYPOINT that owns the WHOLE open: a new `ddrm_plan_runner::DrmHost` owns (1) a `PlanSource` (the runtime-owned seam to ask `drm-provider` "what is the canonical sequence to open this?"), (2) the `RuntimeCapabilityTable` of registered `ProviderTransport`s, and (3) a `RuntimeEventSink` for the plan's runtime-OWNED post-steps; `host.open(content_id, viewer)` FETCHES the plan from the source, drives it through the registry (`open_drm_plan`'s parse → resolve each required provider → execute), then EMITS the plan's runtime-event steps (`release_receipt` + the open `audit`) in order — the single owned entrypoint the trusted runtime calls. Audited PC2's server-owned composition first: the Express `/init` route is the ONE place that owns the whole open — `router.post('/init', authenticate, requireSecureViewSession, handler)` (`src/api/media.ts:133`) runs AFTER the middleware resolves the capability into request state, then the handler owns fetching + parsing the MPD (the "what to open" / plan-equivalent), reading the resolved handle from state and driving recovery (`:481` `const sessionView = req.secureViewSession!.view` → `:482` `recoverMediaCEK`), CREATING the playback session that lives for the duration (`:489` `mediaSessionManager.create`), and logging the open throughout (`:483`, `:518`) — all fail-closed in one place (the `catch` → 500, `:528`). Mirrored: `DrmHost` is the runtime-core analogue of that route — plan-fetch (the MPD fetch) + drive-over-registry (recovery over the resolved capability) + runtime-event emission (session create + audit log), owned in one entrypoint, fail-closed at every seam (a bad plan never resolves a capability; a missing transport fails closed; a declared runtime event that cannot be emitted fails the open). New `PlanStep.event` + `is_runtime_event()` (no provider, carries an `event`) lets the host emit the runtime-owned post-steps the executor only walks for ordering. +5 characterization tests (host opens via plan source + registry + emits both runtime events in order; fails closed on a tampered plan FROM THE SOURCE with no event emitted; fails closed when the event sink refuses the audit — receipt emitted, audit refused; fails closed when a required transport is unregistered; runtime-event steps parse) → ddrm-plan-runner 29→34. `ddrm-consumer-smoke.sh` is now a THIN caller of the host: it provisions the capabilities, REGISTERS the three transports + wires a `SmokePlanSource` (the real `drm-provider`) and a `SmokeEventSink` into a `DrmHost`, and calls `host.open(content_id, viewer)` — the SAME host entrypoint the trusted core will call (the capsule binaries are the host's registered transports + plan source); the tampered-edge gate now flips the plan source into tamper mode and re-opens through the SAME host. Drift untouched. Earlier tip Day-74 — the runtime CORE now OWNS the capabilities the composition root resolves: a new `RuntimeCapabilityTable` (in `ddrm-plan-runner`) is a registry of runtime-owned `ProviderTransport`s — the runtime `register`s one transport per provider it can drive (at startup), and `open_drm_plan` → `resolve(provider)` OPENS a fresh handle over the registered transport, or returns `None` for a provider the runtime never registered (→ the open fails closed). Audited PC2's transport ownership first: the runtime owns the capability factory as a process-lifetime singleton — `export const sessionService = new BackendSessionService(new FileSessionStore(...))` constructed ONCE with a runtime-injected store (`src/services/session/BackendSessionService.ts:495`, ctor `:266`) — and the factory `getSessionView(token)` dispatches on `stored.backend` to CONSTRUCT the per-backend transport it owns the means to build (`WasmSessionView.fromStoredSession` `:374` / `BackendSessionView.fromStoredSession` `:377`), returning `null` for an unknown token/backend (`:370`, fail-closed); a request supplies only a token. Mirrored: `ProviderTransport` (the runtime-owned, long-lived capability to drive one provider, `register`ed once) vs `ProviderHandle` (the fresh per-open handle the transport `open`s — the analogue of a `BackendSessionView` minted per request); `RuntimeCapabilityTable::register` rejects a duplicate (a provider has ONE owner) and `resolve` opens over the registered transport or `None`. +4 characterization tests (registered transports drive the plan; an unregistered required provider → `None` → open fails closed with zero step invocations; a duplicate registration is rejected; a fresh handle is opened per open over the same registered transports) → ddrm-plan-runner 25→29. `ddrm-consumer-smoke.sh` no longer hand-rolls a bespoke table — it REGISTERS three runtime-owned transports (`RightsTransport`/`KeyTransport`/`DecryptTransport`, each wrapping one real capsule binary) into the lib's `RuntimeCapabilityTable` (the SAME registry type the trusted core uses) and drives both the canonical open and the tampered-edge re-run through `open_drm_plan` — no second code path. Drift untouched. Earlier tip Day-73 — the runtime CORE now has a single COMPOSITION ROOT for a dDRM open: `ddrm_plan_runner::open_drm_plan(plan, &mut capability_table)` parses the plan, RESOLVES each provider the plan requires from a runtime-supplied `CapabilityTable` at ONE point, builds the `RuntimeStepRunner`, and executes — the one entrypoint the trusted runtime calls. Audited PC2's composition root first: the secure-view middleware resolves the per-stage handle ONCE from a backend-keyed factory (`sessionService.getSessionView(token)` dispatching on `stored.backend`, `src/services/session/BackendSessionService.ts:368`) and attaches it to request state (`src/api/middleware/secureViewSession.ts:124`→`:129` `req.secureViewSession = { stored, view }`); the handler then invokes it FROM that state and never re-resolves (`src/api/media.ts:481` `const sessionView = req.secureViewSession!.view` → `:482` `recoverMediaCEK(litParams, sessionView)`, the helper taking `session` as a parameter `:1192`; the middleware doc even mandates "handlers must NOT re-load by token", `secureViewSession.ts:13`). Mirrored: a new `CapabilityTable` trait (the runtime-core analogue of the backend-keyed session factory) resolves the `ProviderHandle` for a provider role; `RuntimeStepRunner::resolve_from` is the composition-root constructor that calls `table.resolve` once per required provider and fails closed if the table holds no capability for a required provider OR hands back a handle for the wrong provider; `open_drm_plan` ties parse→resolve→execute into the SINGLE entrypoint. +4 characterization tests (the core entrypoint drives the plan via the table resolving each required provider exactly once in order; fails closed when the table withholds a required provider — never executing a step; rejects a misrouting table; refuses a non-`planned` plan BEFORE touching the table) → ddrm-plan-runner 21→25. `ddrm-consumer-smoke.sh` no longer hand-builds the runner — it supplies a `SmokeCapabilityTable` (the runtime analogue of PC2's session factory, handing out capsule-backed handles) and calls `open_drm_plan` for BOTH the canonical open and the tampered-edge re-run, the SAME entrypoint the trusted core will call (no second code path). Drift untouched. Earlier tip Day-72 — the runtime CORE now INJECTS per-provider capability handles into the Day-71 executor. New `RuntimeStepRunner` (in `capsules/ddrm-plan-runner`) IMPLEMENTS the `StepRunner` seam over a set of injected `ProviderHandle`s — one per provider the plan's `next_required_providers` names — routing each plan step to the handle registered for that step's provider while holding NO authority of its own. Fail-closed construction: it REFUSES to build unless every required provider has an injected handle (no ambient default — the core cannot fabricate a missing capability) and REJECTS a stray handle for a provider the plan does not name (a capability the plan never authorized can never enter the runner), so the plan's `blocked_authority` set is structurally unreachable from the runner type. Audited PC2's per-stage injected handle first: the middleware resurrects the `BackendSessionView` (`src/api/middleware/secureViewSession.ts:124`) and threads it into the downstream stage (`src/api/media.ts:1207` hands it into `recoverMediaCEK`→`recoverCEKEnvelope`) — a stage uses the handle it was GIVEN, it never opens its own connection. The consumer smoke's monolithic `SmokeRunner` is replaced by three per-provider handles (`RightsHandle`/`KeyHandle`/`DecryptHandle`, each wrapping ONE real capsule binary) injected into the SAME `RuntimeStepRunner` the trusted core will use with real providers — no second code path; the two cross-binary fail-closed gates (transcript-mismatched seal; TAMPERED binding edge) ride along unchanged. +7 characterization tests (runtime runner drives the plan through injected handles; refuses to build without a required handle; rejects a stray unnamed handle; rejects duplicate handles; never invokes a handle for an unnamed provider; parses + normalizes `next_required_providers`) → ddrm-plan-runner 14→21. Drift untouched (the runner consumes the plan, defines no shared contract). Earlier tip Day-71 — the runtime CORE now EXECUTES the open plan. New fail-closed library `capsules/ddrm-plan-runner` walks the `DrmOpenPlanV1` `drm-provider` emits, instead of the consumer smoke hand-walking it inline. Audited PC2's gated open sequencer first (each stage gated on the prior): `requireSecureViewSession` resurrects the session view (`src/api/middleware/secureViewSession.ts:61`), then `recoverMediaCEK` → `recoverCEKEnvelope` whose access gate is `hasAccessByContentId(ownerAddress, kid)` (`src/api/media.ts:1163`) and which only THEN recovers + unwraps the CEK in-boundary (`:1196`, `:1216`). `DrmOpenPlan::parse` validates schema / `planned` status / the `rights_check<key_release<decrypt_session` canonical order / every binding edge naming real steps + identities (incl. `content_id==object_cid`); `execute` seeds the virtual `drm_open` identities (`content_id`/`object_cid`/`viewer_interface`), walks the steps IN ORDER, threads each binding edge's produced artifact into the next step's plan-declared `into_field`, and FAILS CLOSED when a step needs an artifact not yet produced (out-of-order / a prior step silently failed) or runs without emitting the artifact the plan says it produces. It holds NO authority — the ONLY thing that touches a provider is the injected `StepRunner` (the runtime's capability seam); the CEK/wallet/chain never appear in the crate. 14 characterization tests (canonical drive + order/threading/identity-seeding + renamed-edge / dropped-artifact / backward-edge / out-of-order / wrong-schema / identity-split / no-authority all fail closed). `ddrm-consumer-smoke.sh` no longer hand-walks the chain: it fetches the REAL `drm open` plan, parses it through the core, and drives drm→rights→key→decrypt THROUGH `DrmOpenPlan::execute` (the smoke is just the injected `SmokeRunner` transport), adding a NEW cross-binary fail-closed gate — a TAMPERED binding edge is rejected by the real `key-provider` (`deny_unknown_fields` over the required `rights_receipt`). New ladder rung ddrm-plan-runner=14 (host-side core, not a wasm capsule); drift untouched (the executor reads the plan, defines no shared contract). Earlier tip Day-70 — the canonical key release is REAL: `key-provider::release` (the op `drm-provider`'s `DrmOpenPlanV1` actually names for the key step, a `not_configured` stub for the reference backend since Phase A.1) now performs the full reference-backend seal. Audited PC2's Lit authority first (`data/lit-actions/universal-decrypt-chipotle.js`): access-check `hasAccessByContentId` (`:560–568`) → recover the CEK `Lit.Actions.Decrypt` (`:570–575`) → recompute `sha256(cek‖kid‖authority)` and refuse on mismatch (`:577–590`, the CEK↔KID↔authority bind) → seal-to-session `envelopeCEK` (`:602–608`); the client returns ONLY the sealed envelope, never the CEK (`chipotle-client.ts::recoverCEKEnvelope` `:1438–1538`). Mirrored exactly: `release` validates the rights receipt (already), then — for the reference backend — RECOVERS the producer-escrowed CEK from the rights-bound `key_envelope.wrapped_cek` (the wrapped CEK rides INSIDE the validated request, not side-band), recomputing the SHARED `escrow_aad(scheme, kid16, recipient_pub)` and verifying the producer's vk, and re-seals it to the runtime-injected decrypt session as the suite-tagged `SealedDecryptMaterialV1` the decrypt sandbox opens. The session material (decrypt session key + producer vk + transcript) is injected by the runtime in a `session` context on the op envelope (capsule-local — the shared `KeyReleaseRequestV1` stays byte-identical, drift untouched). Fail-closed: no backend → `not_configured`; no session context → `not_configured`; denied/mismatched receipt, expired request (when a clock is supplied), KID-swap, scheme mismatch, or forged producer → recover/validation refuses. The CEK exists only in `Zeroizing` inside the boundary and leaves only SEALED. key-provider key-authority-ref 27→33 (+grant→release round-trip, +denied/expired/kid-swap/forged-producer/missing-session fail-closed). `ddrm-consumer-smoke.sh` now ESCROWS the golden CEK to the authority's published recipient and drives the CANONICAL `release` (recover→reseal) instead of the raw-CEK `release_ref` shim — the consumer half runs end to end (drm→rights→key→decrypt) with no raw CEK handed in anywhere, through the op the plan names. Earlier tip Day-69 — the production seal path is CLOSED: `encrypt-provider::seal` (the non-inline op, fail-closed since Day 1) now runs the FULL pipeline on HANDED-IN asset bytes and emits a complete, shared-contract `SealedObjectV1`. Audited PC2's producer input first (`src/services/media/dashPackager.ts`): the host reads each segment off disk (`readFileSync` `:504`, `:571–572`) and HANDS the bytes to the CENC WASM (`executeCENCEncrypt(.., seg.data)` `:432–434`) — the encoder fetches nothing. Mirrored exactly: `seal` gained `content_b64` (+ `recipient_pub_b64`, `availability_receipt_cid`), `deny_unknown_fields` preserved, and when bytes + recipient are handed in it runs mint→CENC→content-address→escrow through the ONE shared `run_seal_pipeline` (`seal_inline` now delegates to it too — PRINCIPLES #10), assembling a `SealedObjectV1` whose `payload_cid` is the real Day-68 CID, `key_envelope.kid` IS the bytes16 contentId, `policy_hash = sha256(rights_policy_cid)`, and PQ-hybrid algorithm suite is accepted by the shared chain validator. NO fetch/IPFS/network authority — the producer seals the bytes it's handed. Fail-closed: no recipient or no bytes → `not_configured`; missing availability receipt / empty viewer interface / empty content → `invalid_request` (encrypt-provider escrow 22→25). `ddrm-producer-smoke.sh` drives the REAL production `seal`, deserializes the output into the shared `elastos_common::protected_content::SealedObjectV1` and runs the SAME `validate_protected_content_key_envelope_algorithms` the downstream `key-provider` runs — cross-binary proof the chain accepts the producer's object; no plaintext on the wire (the production output carries no segment at all). Earlier tip Day-68 — the producer stopped FAKING storage: `encrypt-provider` now content-addresses the sealed ciphertext IN-BOUNDARY — `payload_cid = CIDv1(raw, sha2-256)` of the segment, byte-for-byte what PC2's producer gets from Helia `unixfs.addBytes` (`@helia/unixfs` `add.ts`: `cidVersion:1, rawLeaves:true`, 1 MiB `fixedSize` chunker → single-chunk content's root IS its raw leaf). Pure function of the bytes, NO `kubo_api`/network (a CID is not a pin); fail-closed above one chunk (multi-block dag-pb refused, never guessed). `seal_inline` now emits a real `payload_cid` instead of the smoke's `bafybeig…` placeholder; the golden pins three inputs to the EXACT strings PC2's real `ipfs-unixfs-importer` produces (the ecosystem oracle — incl. the canonical raw-`abc` CID), so a codec drift fails loudly (encrypt-provider 17→20 default, 19→22 escrow). `ddrm-producer-smoke.sh` independently recomputes the segment's CID via the canonical `cid` crate (a DIFFERENT encoding path) and demands a byte-for-byte match cross-binary — the producer's content address now resolves to the bytes it sealed, verifiable by a human, no "trust me". payload_cid (IPFS address) stays a SEPARATE identity from KID/contentId (the chain key). Earlier tip Day-67 — the crown-jewel ORCHESTRATOR is real: `drm-provider::open` now emits a typed, executable **`DrmOpenPlanV1`** (status `planned`, never `opened`) — the single canonical `drm/open` sequence (content → rights → key → decrypt → render → receipt → audit) carrying its explicit inter-step **binding edges** (rights ⇒ `RightsDecisionReceiptV1` → `key.rights_receipt`; key ⇒ `ReleaseReceiptV1` → `decrypt.release_receipt`; one content identity == KID under both `content_id`/`object_cid`, honouring `key-provider`'s `content_id==object_cid` invariant) — while holding ZERO authority (no CEK/keys/RPC; it PLANS, the runtime EXECUTES), capsule-local like `publish-provider::UnsignedMintV1` so the frozen contract surface + drift gate stay untouched (drm-provider=15). `ddrm-consumer-smoke.sh` now drives the REAL `drm open`, asserts the `planned` plan + canonical order + binding edges, and FOLLOWS it — threading each receipt into the field the PLAN declares and taking the content identity from the plan — instead of a hardcoded sequence (PRINCIPLES #10, one canonical path owned by the capsule that owns it). Mirrors PC2's `recoverCEKEnvelope` + Lit-action open ordering (access-check → key-release → seal, fail-closed at every step). Earlier tip Day-66 Phase C — the chain's OWN emitted log now reconstructs the same listing: `content-market` gained `listing_from_event`, which decodes a PC2 `DigitalAssetRegistered` log (carries `bytes16 contentId` on-chain → SAME identity as the calldata path) or an `AssetCreated` log (no on-chain contentId → `metadata_status:"needs_kid"`, identity deferred to the `enrich_listing` kid-match, never guessed) into a `ContentListingV1`. Pure decode — the log bytes are handed in by `chain-provider` (named, no RPC). `ddrm-market-smoke.sh` now also builds a `DigitalAssetRegistered` log carrying our contentId and asserts the event path agrees with the calldata path on `content_id`/`token_uri`/`op_type` (content-market=29). Calldata and chain event agree on one identity. Earlier tip Day-65 Phase C — the listing now carries HUMAN-FACING fields, fail-closed: `content-market` gained `enrich_listing`, which fuses a resolved `metadata.json` (name/description/poster/`media.uri`→contentCID/`contentType`→mime/`classifyAssetType`) onto the calldata-derived identity — but re-derives the contentId from the calldata and REJECTS any metadata whose `kid != content_id` (`identity_mismatch`), so metadata can DESCRIBE but never RE-IDENTIFY a listing (content-market=22). It still fetches nothing — the `metadata.json` bytes are handed in by `ipfs-provider` (named, not invoked). `ddrm-market-smoke.sh` now drives `publish → chain → content-market(reconstruct) → content-market(enrich)` so a matching-kid metadata resolves to a full card and a tampered kid is rejected cross-binary. Earlier tip Day-64 Phase C — the published mint is now DISCOVERABLE: a new fail-closed `content-market` capsule reconstructs a typed `ContentListingV1` PURELY from the self-describing mint calldata (the inverse of Day-62's `assemble_mint`) — `content_id == bytes16 KID`, tokenURI→metadataCID (PC2 `extractCid`), opType, and `(copies,price,payToken)` sell terms — holding NO chain RPC / NO IPFS / NO keys and minting nothing, so a foreign/malformed call fails closed (content-market=13). `ddrm-market-smoke.sh` drives the REAL `publish → chain → content-market` binaries so a sealed asset's KID flows producer→chain→discovery as ONE identity: the listing's `content_id` IS the producer's KID. Runtime-superior vs PC2's `ContentIndexerService` (4 sources: event + tokenURI eth_call + metadata.kid + AuthorityGateway price) — our mint calldata is self-describing, so one pure decode yields a verifiable listing; human-facing fields (title/poster/mime) are delegated to ipfs+chain enrichers, not assumed. Earlier tip Day-63 Phase C — the producer→chain loop is CLOSED cross-binary: `publish-provider` now emits an `UnsignedMintV1` whose STRUCTURED `op_raw`/`sell` drop STRAIGHT into `chain-provider::assemble_mint` (PC2-faithful payee arrays — creator ACCESS_TOKEN + ROYALTY_SHARE `amount=round(10*royalty)`, default `100−ELACITY_ROYALTY_PERCENT(5)`, BUY_AND_RESELL DISTRIBUTION_RIGHT + `resellerCut`), proven by `ddrm-publish-smoke.sh` driving the REAL `publish (prepare) → chain (assemble_mint)` binaries so one identity flows KID → contentId → mint calldata with tokenURI + sell terms intact and no signing/RPC in the assembler; publish=16. Earlier tip Day-62 Phase C — the prepared mint is now real CALLDATA: `chain-provider` gained a pure `assemble_mint` op that ABI-encodes the PC2 `mint(string,uint16,bytes,bytes)` call (FREE `opRawData=abi.encode(bytes16 contentId)`, PAID opRawData payee/royalty arrays + `sellRawData=(copies,price,payToken)`) and returns the `{to,data,value}` an external signer signs and the existing `broadcast_transaction` (`eth_sendRawTransaction`) sends — no RPC/keys in the encoder, calldata decoded back to spec in 10 tests. Day 61 added the fail-closed `publish-provider` that ASSEMBLES the mint intent (binds `contentId == bytes16 KID`, derives `tokenURI = {metadataCid}/metadata.json`, emits the unsigned `UnsignedMintV1`; publish=13). Day 60 took the producer half cross-binary: `encrypt-provider` (feature `escrow`) `seal_inline` mints a CEK *now* + emits the SEALED escrow blob; `key-provider` (`release_from_escrow_ref`) recovers + re-seals it; `ddrm-producer-smoke.sh` drives `encrypt → key → decrypt` so a video sealed *now* decrypts *now*, no raw CEK/plaintext on any wire, no golden). **0.4.0 released — crypto core verified green on the released `v0.4.0`; rebase surface measured (see `PUSH_PLAN.md`). Anders confirmed the rail (Day 45); the decrypt boundary now implements his ENTIRE decrypt-side spec — Option A push-in (`rail-live`), full-transcript binding (`rail-bind`), in-sandbox key mint+publish (`rail-mint`), short-expiry + scoped CEK-free audit (`rail-audit`) — consolidated into the suite-tagged `SealedDecryptMaterialV1` drop-in (`rail-material`). Remaining work is upstream only (contract merge needs push; dKMS sealing needs Anders).**

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

> **🛡️ Day 101–102 — the live 2-of-2 threshold is now RESILIENT + IDENTITY-BOUND: the production rail FAILS CLOSED under a real node fault, NEVER degrades to a single node, and a silently SWAPPED node-set is DETECTED before recovery (LANDED).**
> Day 99–100 wired the threshold into the real open. This cycle proves it SURVIVES FAULTS and PINS who backs it — closing the gap between "two nodes on the happy path" and "two nodes you can trust under attack/failure."
> Audited PC2 first: PC2's run-path resilience is a SINGLE opaque RPC with a RETRY — `recoverCEKEnvelope` calls one Lit endpoint, and on `RequestExpired` "the caller should retry by re-running the Lit action" (`chipotle-client.ts:575`). A downed node, a swapped node, a partial recovery — all live INSIDE Lit's distributed network, so PC2 has NO per-node fault semantics it can express and NO node-set membership it can inspect or pin (the network identity is the only thing it pins, as a whole). **The runtime is SUPERIOR:** it OWNS the two nodes, so it can kill one and prove the rail fails closed, and it can pin EXACTLY which two secret-holders back a rail and detect a swap.
> Mirrored across the seams: **(1) node-set identity (single source of truth)** — a NEW pure `ddrm_envelope::threshold_node_set_id(t, vk_a, vk_b)` = `SHA-256(DOMAIN ‖ [t] ‖ len‖vk_a ‖ len‖vk_b)` (length-prefixed so no concatenation collision; order-sensitive; +1 test, 23→24: deterministic, swapping EITHER node OR `t` changes the id, the length prefix prevents a boundary-shift collision). `publish_escrow` computes it over BOTH provisioned nodes' vks and PINS it into the durable publish fixture (`node_set_id_b64`); `host.open()` RE-DERIVES it from the PUBLISHED descriptor's `threshold` block and FAILS CLOSED if they differ — so a descriptor whose node-set was silently swapped (one node re-pointed at a DIFFERENT secret-holder than the producer escrowed to) is DETECTED before the rail recovers anything, independent of the boundary's per-share seal check. **(2) live node-fault fail-closed (verify mode, threshold-only) gates 23–24** — with the full 2-of-2 rail up: KILL node B's daemon mid-session → `host.open()` FAILS CLOSED (the dual-recover errors at node B; no partial CEK, no single-node fallback) and persists NO runtime-event record, then node B is restored; KILL node A's daemon → SAME (the recover errors at node A, recovered first), and node A is RESTARTED so the post-shutdown socket probes (steps 13–17) still connect. The daemon guards (`dkms_daemon`/`dkms_daemon_b`) are now `mut` so the gates kill + restart them; teardown is explicit at run-end. **(3) swap detection gate 25** — a descriptor whose node B is swapped to a rogue ML-DSA identity re-derives to a DIFFERENT `node_set_id` than the producer-pinned value → detected end-to-end. Combined with the decrypt boundary independently rejecting the rogue node's seal under node B's PINNED vk (Day 97–98 step 20), a swapped node fails closed at BOTH the descriptor AND the boundary. Drift untouched (the node-set-id is a runtime-owned durable artifact + a pure envelope primitive; no shared contract changed). Gate: ladder INTACT (ddrm-envelope=24, key-provider[key-authority-ref]=43, decrypt-provider rail-material=68), drift PASS, all dDRM smokes green (reference + dkms single-node + the dkms 2-of-2 with the new node-fault + swap gates), clippy clean (one PRE-EXISTING rng-borrow warning in `ddrm-envelope`, not introduced here).

> **🔱 Day 99–100 — the 2-of-2 threshold now runs through the PRODUCTION `DrmHost` run-path (not just the verify-mode probe): the happy open provisions TWO nodes, dual-recovers BOTH, and reconstructs the CEK ONLY inside the decrypt boundary (LANDED).**
> Day 97–98 landed the threshold crypto + a SELF-CONTAINED probe (its own two daemons), but the production happy path still provisioned ONE node, escrowed the WHOLE CEK, and never supplied share-2 — the runtime (`DrmHost`) did not know it was talking to a threshold authority. This cycle wires it into the real open so a single compromised node yields NOTHING on the live rail, not just in a probe.
> Audited PC2 first: PC2's run-path delegates recovery with ONE RPC — `recoverCEKEnvelope(litParams, sessionView)` (`chipotle-client.ts:1438`) calls a single Lit network endpoint; PC2's OWN code never collects shares from multiple nodes. `decryptAndCombine` is the LEGACY Datil threshold whose share-set + node membership + combine live ENTIRELY inside Lit's proprietary network (`chipotle-client.ts:1297`, "NOT compatible with Datil's decryptAndCombine"), and the current Chipotle path is a single-node PKP-AES TEE decrypt (`Lit.Actions.Decrypt`). **PC2's runtime STOPS at one opaque RPC; the runtime is SUPERIOR** — it provisions, dual-recovers, and reconstructs across TWO OWNED, inspectable nodes inside its own host + boundary.
> Mirrored across the seams: **(1)** `OpenConfig.authority.threshold` (boolean) promotes the dkms open to 2-of-2 — fail-closed if set with `backend != dkms` (the in-runtime reference authority holds the master itself, so there is nothing to split across) or a non-boolean value (+2 bin config tests, 8→10). We provision BOTH nodes from the SAME node binary, so this is a boolean knob rather than a handed-in node-B descriptor path: the descriptor's `threshold` block the `key-provider` consumes is what the runtime OWNS producing. **(2)** `publish_escrow` provisions node A AND node B (distinct stores/sockets/allow-lists), `split_cek_xor`s the content CEK so node A escrows share-1 and node B escrows share-2 — NEITHER node ever sees the whole key — and publishes a `threshold` descriptor (`t:2`, both nodes' public identities); the durable fixture then also carries `wrapped_cek_share2_b64` + node B's `vk2_b64`. **(3)** the `DrmHost` run-path starts BOTH daemons before the rail, binds `KeyOpenMaterial.wrapped_cek_share2_b64` from the fixture, passes node B's vk to the `DecryptLauncher` (`authority_vk2_b64` at decrypt `init`), and `KeyHandle` supplies `wrapped_cek_share2_b64` in the `release` session context — so `host.open()` itself drives the full dual-recover (both nodes, each over its own session/possession/freshness gate) → merge two re-sealed shares (WITHOUT combining) → unwrap BOTH in-VM + XOR → decrypt; a threshold↔descriptor desync (config says threshold but the descriptor has no `threshold` block, or vice-versa) fails closed before launch. **(4) integration fix:** `merge_threshold_material` now welds node B's re-sealed share into node A's NESTED `material.sealed_cek_share2_b64` (the shape the decrypt boundary actually consumes). The Day 97–98 merge read a TOP-LEVEL `sealed_cek_b64` the real node never emits (the node nests it under `material`), so the merge was never exercised end-to-end (the probe combined directly, bypassing `key-provider`); this cycle's full run-path surfaced + fixed it, and the unit test was corrected to the real nested recover shape (key-provider[key-authority-ref] stays 43). **(5) adversarial (verify mode, threshold-only) gates 21–22:** the LIVE threshold rail REFUSES a `release` supplying only ONE share (it must never silently degrade to a single-node recover), and a 3-of-N `threshold` descriptor FAILS CLOSED at `key-provider` init (the runtime never downgrades a stronger threshold to what it can do). Plus config-level fail-closed: `threshold` on the `reference` backend is rejected at parse. **Cross-binary proof:** a NEW `ddrm-consumer-dkms-threshold-smoke.sh` (a thin `--threshold` sibling) drives the WHOLE 2-of-2 open against two real daemons end to end; the reference + single-node dkms smokes stay green. Drift untouched (the second share + second vk are capsule-local; no shared contract changed). Gate: ladder INTACT (ddrm-envelope=23, key-provider[key-authority-ref]=43, decrypt-provider rail-material=68), drift PASS, all dDRM smokes green (reference + dkms single-node + the NEW dkms 2-of-2), clippy clean.

> **🔱 Day 97–98 — the threshold is REAL: the CEK is XOR-split 2-of-2 across TWO secret-holding dKMS nodes; no single node ever holds the whole key, and the runtime reconstructs it ONLY inside the decrypt boundary (LANDED).**
> Day 95–96 left a fail-closed threshold STUB (`key-provider` refused a `threshold` descriptor). This cycle makes 2-of-2 real end to end so a compromised single node yields NOTHING.
> Audited PC2 first: PC2's threshold is the OPAQUE Lit `decryptAndCombine` (`non-media-decrypt.js:76`) — the Lit network does threshold BLS decryption across its own distributed nodes and combines INSIDE the Lit TEE; the share set, the node membership, and the combine are entirely inside Lit's proprietary network and uninspectable to PC2, which treats it as a black box. **The runtime is SUPERIOR here:** an EXPLICIT, owned, inspectable 2-node split (we provision the nodes, the XOR split, and the recipients) with the combine in our OWN sandbox — no black box.
> Mirrored across the seams: **(1)** `ddrm-envelope` gained the pure split/combine primitives — `split_cek_xor(cek, mask)` (producer: `share1=mask`, `share2=cek⊕mask`; a uniform mask hides the CEK information-theoretically in either share alone) and `combine_cek_xor(s1,s2) → Zeroizing` (decrypt boundary: `cek=share1⊕share2`, fail-closed on a length mismatch so a wrong/forged share can never yield a truncated key), +1 test (22→23: split round-trips, each share alone is NOT the CEK, single-share self-XOR is zeros, length mismatch fails closed). **(2)** `decrypt-provider` reconstructs IN-VM: `SealedDecryptMaterialV1` gained an OPTIONAL `sealed_cek_share2_b64` and the boundary an OPTIONAL second trusted node vk (`authority_vk2_b64`); when a second share is present, `rail_shim::decrypt_from_carrier_threshold` unwraps BOTH sealed shares (each under ITS node's verifying key, bound to the SAME decrypt transcript), XOR-combines them in `Zeroizing`, and only THEN decrypts — so the whole CEK materializes ONLY in the sandbox, never in `key-provider`; the single-share path is byte-unchanged (rail-material 65→68: +happy 2-of-2 reconstructs+opens, +an unauthorized second share (not node B's seal) denied, +a threshold material at a single-node boundary fails closed). **(3)** `key-provider` REPLACED the Day 95–96 fail-closed stub with real resolution + orchestration: `build_dkms_client` resolves a PUBLIC-ONLY 2-of-2 `threshold` descriptor (`t==2`, two DISTINCT node entries) into TWO clients (3-of-N / identical-nodes / malformed all fail closed); `release` runs hello+recover against BOTH nodes over their OWN long-lived connections (known-caller, fresh `recover_seq`, possession proof per node), collects TWO re-sealed shares, and `merge_threshold_material` welds them into one two-share material WITHOUT XOR-combining (the CEK is NEVER reconstructed here) — the second share's escrow rides in the runtime-injected session context (`wrapped_cek_share2_b64`); +1 test (42→43: real 2-of-2 resolution + 3-of-N/identical/single-node fail-closed; the merge helper welds + fails closed on missing/identical shares or a content mismatch). **Cross-binary proof:** `ddrm-runtime-open` verify mode adds a 2-of-2 probe (steps 18–20) that starts TWO real node daemons (distinct stores/sockets/allow-lists), escrows share-1→node A + share-2→node B, recovers a re-sealed share from EACH node over the full session/possession/freshness gates, then (as the decrypt boundary) unwraps both + reconstructs the EXACT CEK — and proves a single node's share is USELESS (it is NOT the CEK) and a FORGED second share fails closed under node B's vk; the reference + single-node dkms paths stay green. Drift untouched (the second share + second vk are capsule-local). **Escape hatch (per the 2-day prompt):** the production `DrmHost` run-path live dual-recover wiring + its dedicated end-to-end smoke is the Day 99–100 finisher; this cycle landed the full producer split + two-daemon provisioning + `key-provider` dual-recover orchestration + the real in-VM reconstruction, all proven cross-binary. Gate: ladder INTACT (ddrm-envelope=23, key-provider[key-authority-ref]=43, decrypt-provider rail-material=68), drift PASS, all dDRM smokes green (incl. the dkms 2-of-2 probe), clippy clean.

> **🪪 Day 95–96 — the dkms node serves only a KNOWN, ALLOW-LISTED caller, every recover is FRESH (anti-replay), and a THRESHOLD descriptor fails closed (LANDED).**
> Day 93–94 made the bearer session non-replayable across callers (possession proof), but the caller was still ANONYMOUS (any well-formed ephemeral key was served) and a captured recover frame could be replayed verbatim within a session. A production dKMS serves only callers it KNOWS, makes each recover single-use, and never silently weakens a threshold request.
> Audited PC2 first: **(owner-bound to a registered identity)** the secure-view session's `ownerAddress` must equal the authenticated wallet — a session is not anonymous, it is tied to a known owner re-checked in the TEE via `ecrecover(delegationSig)` (`secureViewSession.ts:87`–`:100`); **(revocable freshness nonce)** the wallet-signed canonical delegation carries a `nonce` the node reads back and refuses if revoked, so a credential can be invalidated per-use (`secureViewSession.ts:108`–`:112`).
> Mirrored across the seams: **(1)** `ddrm-envelope`'s recover possession-proof primitive now binds a per-recover FRESHNESS counter — `sign_recover_proof`/`verify_recover_proof` take `recover_seq` (length-prefixed into the signed preimage), so the seq is authenticated and a MITM cannot bump a stale frame's counter without invalidating the proof (tests updated to assert a swapped seq fails; 22 tests, count unchanged). **(2)** The `dkms-authority` node gained a KNOWN-caller allow-list + an anti-replay counter: an OPERATOR-provisioned `DKMS_AUTHORITY_ALLOWED_CALLERS` (comma-separated b64 verifying keys, resolved ONCE at daemon startup, never overridable by the connecting client) makes `hello` refuse an unknown caller (`caller_not_authorized`) before minting a token; and `recover` now tracks the highest `recover_seq` consumed in the session and refuses any recover that does not strictly advance (committing only on success) — fail-closed on a stale/replayed counter (+2 tests, 11→13: replayed/stale `recover_seq` refused; allow-list enforced on hello while a no-allow-list node stays anonymous). **(3)** `key-provider`'s dkms client now derives a STABLE caller identity from a runtime-provisioned `dkms_caller_seed_b64` (so the node's allow-list recognizes it; absent → ephemeral/anonymous as before), stamps + signs a STRICTLY-INCREASING `recover_seq` into every recover, and RECOGNIZES a `threshold` descriptor (`t>1`/multi-node) failing closed rather than recovering from one node (+1 test, 41→42: a threshold descriptor is refused at init while a single-node `t==1` descriptor resolves). **Cross-binary proof:** `ddrm-runtime-open` now provisions a per-run KNOWN caller identity into the daemon's allow-list and hands the same seed to BOTH the rail's key-provider AND the adversarial probe; the dkms smoke proves the rail opens as the allow-listed caller, and the probe adds two adversarial gates against the REAL daemon — an UNKNOWN caller's `hello` is refused (allow-list enforced), and a REPLAYED recover frame (stale `recover_seq`) is refused after three strictly-advancing successful recovers; the reference path stays green. Drift untouched: no shared contract changed (the allow-list + freshness counter are capsule-local protocol). Next: REAL 2-of-N threshold — split the CEK across multiple secret-holding nodes so no single node holds the whole key (key-provider orchestrates, the decrypt boundary reconstructs); then a real network transport beyond the local socket; a `lit` compat backend. Gate: ladder INTACT (ddrm-envelope=22, dkms-authority=13, key-provider[key-authority-ref]=42), drift PASS, all dDRM smokes green (incl. the dkms variant), clippy clean.

> **🧷 Day 93–94 — the long-lived dkms node gets a REAL transport boundary (framed Unix-domain socket) and the bearer session becomes NON-REPLAYABLE across callers (possession-proof), closing the two seams Day 91–92 deferred (LANDED).**
> Day 91–92 made the node a long-lived connection, but it was still a stdin/stdout CHILD `key-provider` spawned, and the session token was a pure BEARER credential — anyone who captured the `hello` response could replay it. A real remote dKMS is reached over a transport the runtime does not own the process of, and a session must be bound to a secret only the legitimate caller holds.
> Audited PC2 first: **(owner-bound)** the secure-view session is not a pure bearer token — the stored session's `ownerAddress` must equal the authenticated wallet or `403 session_owner_mismatch`, and the Lit Action repeats the SAME check inside the TEE via `ecrecover(delegationSig) === del.ownerAddress` (`secureViewSession.ts:87`–`:100`); **(framed transport)** the Boson proxy frames every packet `[2-byte length (BE, includes itself)][1-byte type][body]` with `PACKET_HEADER_SIZE = 3` + `MAX_PACKET_SIZE = 65535`, recovering exact message boundaries rather than trusting a raw stream (`ProxyProtocol.ts:13`/`:251`/`:256`/`:371`).
> Mirrored across the seams: **(1)** a NEW shared FRAME module in `ddrm-envelope` — `frame::write_frame`/`frame::read_frame`, `[4-byte BE length][payload]`, `MAX_FRAME_BYTES = 1 MiB`, fail-closed on a torn (EOF mid-header/payload), oversized, or zero-length frame, with a clean EOF at a frame boundary signalled distinctly — plus the session token now binding the caller's ephemeral pubkey (`sign_session_token`/`verify_session_token` over `challenge‖caller_pub‖expires_at`) and a recover possession-proof primitive (`sign_recover_proof`/`verify_recover_proof` over `DKMS_RECOVER_DOMAIN ‖ challenge ‖ content_id ‖ kid_hex ‖ session_pub`), all length-prefixed + domain-separated + single-source-of-truth (+2 tests, 20→22: frame round-trip + torn/oversized/zero refusal; possession round-trip + wrong-key/tamper/domain-separation rejection). **(2)** The `dkms-authority` node gained a SOCKET serve mode (`DKMS_AUTHORITY_LISTEN=<path>` → bind + listen + serve framed connections sequentially, one fresh node + session per connection; a torn/oversized/half-closed frame drops THAT connection only, the listener serves on) keeping the EXACT same JSON ops; `hello` now requires + binds the caller's ephemeral pubkey into the token, and `recover` REQUIRES a possession proof it verifies against the token-bound pubkey BEFORE re-authorization and any key material — fail-closed on a missing/forged/wrong-key proof (+2 tests, 9→11: framed full-session round-trip + torn-frame drop without panic; possession gate refusing garbage + wrong-key proofs). **(3)** `key-provider`'s dkms client now CONNECTS to the node's socket (`UnixStream::connect`) instead of spawning, mints an EPHEMERAL keypair per connection (sends the pubkey at hello), and SIGNS every recover under the matching private key — the long-lived `DkmsNodeConn` now wraps the framed socket + the ephemeral signer (boxed to keep `KeyProvider` off the stack); the socket transport is `unix`-gated so the wasm32-wasip1 ladder build stays clean and fails closed on non-unix (key-provider[key-authority-ref]=41). **Cross-binary proof:** `ddrm-runtime-open` now starts the node DAEMON listening on its socket BEFORE the rail comes up (a `DaemonGuard` reaps it after), publishes the SOCKET PATH as the descriptor `authority_endpoint`, and the rail's key-provider CONNECTS to it; the dkms smoke drives verify mode through FIVE socket gates against the REAL daemon (step 13: identity pinned over the socket + a CALLER-BOUND token minted; step 14: NO/EXPIRED/FORGED/tampered token, NO possession proof, and a WRONG-KEY proof are ALL refused; step 15: even WITH a live session+proof a DENIED / wrong-content receipt is refused; step 16: ONE socket connection+session → THREE successful recovers, sealed material only; step 17: a torn AND an oversized frame each fail closed without wedging the daemon, a clean session afterwards still succeeds), and the genuine open flows through the framed socket; the reference path stays green. Drift untouched: no shared contract changed (the frame module + possession proof are capsule-local protocol). Next: a client-held long-term identity so the ephemeral key is bound to a known caller (vs anonymous-but-possessing); threshold shares across multiple nodes; a `lit` compat backend. Gate: ladder INTACT (ddrm-envelope=22, dkms-authority=11, key-provider[key-authority-ref]=41), drift PASS, all dDRM smokes green (incl. the dkms variant), clippy clean.

> **🔗 Day 91–92 — the dkms node becomes a LONG-LIVED CONNECTION the client opens ONCE, and the handshake mints a node-bound SESSION the node REQUIRES on every recover (LANDED).**
> Day 89–90 authenticated the channel, but `key-provider` still SPAWNED a fresh node + re-ran `init`+`hello`+`recover`+`shutdown` EVERY release, and the verified handshake gated nothing beyond that single call. A real remote dKMS is a persistent process the client opens a connection to, proves identity to ONCE, and then drives many authorized recovers over — without re-deriving the master each time and without letting a captured handshake be replayed.
> Audited PC2 first: the per-view secure session is ESTABLISHED ONCE (`begin-session`) and only RESURRECTED per request to gate recovery, never re-minted — `requireSecureViewSession` extracts the opaque bearer token, `getSessionByToken(token)` returns null for an unknown/expired token → `401 session_token_invalid` (`secureViewSession.ts:81`–`:85`), a missing token → `401 session_token_required` (`:72`–`:79`), and the live view is resurrected via `getSessionView(token)` (`:124`–`:128`) and handed downstream directly — handlers must NOT re-load by token (`:12`–`:14`); recovery is refused without a live session.
> Mirrored across three seams: **(1)** a NEW domain-separated session-token primitive in `ddrm-envelope` — `sign_session_token(signer, challenge, expires_at)` / `verify_session_token(verifier, challenge, expires_at, sig)` over `DKMS_SESSION_DOMAIN ‖ challenge ‖ expires_at(LE)` (`elastos.dkms.authority/session/v1`, domain-separated from the hello attestation + the CEK seals so a token can never be replayed as either) — defined ONCE so node + client cannot drift (+2 tests, 18→20: round-trip + reject tampered-expiry/tampered-challenge/forged/malformed, and domain-separation from hello). **(2)** The `dkms-authority` node's `hello` now also mints a node-SIGNED SESSION TOKEN (binds the client's challenge to `now + 300s`, signed with the master-derived key), and `recover` REQUIRES one: it verifies the token under the node's OWN verifying key and checks it is unexpired against the caller's clock — fail-closed on missing (a hard parse error), expired, forged, or tampered (challenge/expiry) — BEFORE re-authorization and BEFORE any key material; a missing-token recover does not even deserialize (+3 tests, 6→9: hello mints a verifiable token; recover refused on no/expired/forged/tampered token; ONE session authorizes MANY recovers). **(3)** `key-provider`'s dkms client now holds a LONG-LIVED `DkmsNodeConn` (the live child + the cached session token) in interior-mutable state — on `release` it OPENS-ONCE (spawn + init + identity handshake + capture the session token), then REUSES the connection + session across releases, re-establishing fail-closed only when the cached session has expired (or with no clock); the per-release spawn/shutdown is gone — +1 test (40→41: the `dkms_session_live` reuse gate is live only with a clock AND before expiry, fail-closed otherwise). **Cross-binary proof:** the dkms smoke drives `ddrm-runtime-open` verify mode through FOUR gates against the REAL node (step 13: identity pinned/verified + the node minted a session token; step 14: recover with NO / EXPIRED / FORGED token, and a token whose bound challenge was tampered, are ALL refused even with a valid escrow+receipt; step 15: even WITH a live session the node still refuses a DENIED / wrong-content receipt; step 16: ONE live session → THREE SUCCESSFUL recovers, sealed material only, raw CEK never present), and the genuine open now flows through the persistent connection; the reference path stays green. Drift untouched: no shared contract changed (the node CONSUMES the existing `RightsDecisionReceiptV1`; the session token is a capsule-local protocol message). Next: a real socket/RPC transport for the long-lived node (vs the spawned child) + a client-held secret so the bearer token is non-replayable across callers; threshold shares across multiple nodes; a `lit` compat backend. Gate: ladder INTACT (ddrm-envelope=20, dkms-authority=9, key-provider[key-authority-ref]=41), drift PASS, all dDRM smokes green (incl. the dkms variant), clippy clean.

> **🔐 Day 89–90 — the delegation becomes an AUTHENTICATED CHANNEL with a per-recover AUTHORIZATION the node re-checks in its own boundary (LANDED).**
> Day 87–88 split out the node + delegated recovery, but `key-provider` SPAWNED the node and trusted it implicitly,
> and the node recovered for whatever the caller sent. A real client (1) talks to a node it did not spawn, (2)
> verifies it is the AUTHENTIC authority, and (3) presents a per-recover authorization the node independently checks.
> Audited PC2 first: (a) the Lit action PINS the authority it talks to — after recovering the CEK it recomputes
> `sha256(cek‖kid‖authority)` in the TEE and DENIES `kid_authority_mismatch` on a swapped authority/KID
> (`universal-decrypt-chipotle.js:577`–`:590`); (b) the node independently RE-RUNS the access check in its own
> boundary — `hasAccessByContentId(addr, normalizedKid)` against the on-chain gateway, denying `access_denied`
> rather than trusting the caller (`:560`–`:568`). Mirrored across three seams: (1) a NEW domain-separated
> attestation primitive in `ddrm-envelope` — `attest_challenge(signer, challenge)` / `verify_attestation(verifier,
> challenge, sig)` over `DKMS_HELLO_DOMAIN ‖ challenge` (`elastos.dkms.authority/hello/v1`, separated from the
> CEK-seal signatures so a hello can never be replayed as a seal) — defined ONCE so node + client cannot drift
> (+2 tests, 16→18: round-trip + reject forged/impersonator/replayed/malformed, and domain-separation). (2) The
> `dkms-authority` node gained a `hello` op — it signs the client's fresh challenge with its master-derived signing
> key and returns the attestation + its published vk (fail-closed before `init`) — and now RE-AUTHORIZES every
> `recover` in its own boundary BEFORE touching key material: the request carries the `RightsDecisionReceiptV1` +
> the content/principal/session/right binding, and the node refuses unless the receipt is `allowed`, a
> protected-content action, and binds the SAME content/principal/session/right the recover declares (a buggy/compromised
> caller forwarding a denied/foreign/incoherent receipt is caught) — +2 tests (4→6: hello attests identity + requires
> init; recover fails closed on denied / wrong-content / wrong-principal / non-protected-right). (3) `key-provider`'s
> dkms client now runs the IDENTITY HANDSHAKE before delegating — it sends a fresh challenge, requires the node to
> advertise EXACTLY the descriptor-pinned vk AND a valid attestation over the challenge under it (fail-closed on a
> forged/mismatched node), then threads the rights receipt + binding into `recover` so the node re-checks — +1 test
> (39→40: handshake-verify accepts the genuine attestation, rejects a mismatched vk / impostor signature / replayed
> challenge / malformed sig). **Cross-binary proof:** the dkms smoke now drives `ddrm-runtime-open` verify mode through
> two NEW adversarial gates against the REAL node (step 13: the node's attestation verifies under the descriptor vk
> while a flipped vk + a replayed challenge are both rejected — an impersonated node refused at handshake; step 14:
> the node refuses a recover for a DENIED receipt and for a receipt bound to OTHER content), and the happy path still
> decrypts with the master never crossing the wire; the reference path stays green. Drift untouched: no shared
> contract changed (the node CONSUMES the existing `RightsDecisionReceiptV1`). Next: a long-lived socket/RPC transport
> (vs the spawned child) + threshold shares across multiple nodes; a `lit` compat backend. Gate: ladder INTACT
> (ddrm-envelope=18, dkms-authority=6, key-provider[key-authority-ref]=40), drift PASS, all dDRM smokes green (incl.
> the dkms variant), clippy clean.

> **🛰️ Day 87–88 — the `dkms` authority SPLITS into a SECRET-HOLDING NODE + a PUBLIC-ONLY runtime; recovery is DELEGATED across the process boundary (the first real step toward remote dKMS, LANDED).**
> Day 85–86 ran `dkms` end-to-end, but the runtime was still HANDED the master seed (the descriptor carried it) — a
> true external authority NEVER gives the runtime its secret. Day 87–88 closes that. Audited PC2 first: (a) the
> Lit/dKMS authority recovers the CEK INSIDE the node and returns ONLY the sealed envelope — the action runs
> `Lit.Actions.Decrypt` in the TEE (`universal-decrypt-chipotle.js:572`), rebinds CEK↔KID↔authority (`:577`–`:590`),
> seals to the session via `envelopeCEK` (`:602`–`:608`), and `setResponse` returns only the sealed `data` (`:610`–`:613`),
> never the raw CEK or the PKP secret. (b) The client holds only the authority's PUBLIC identity and RPCs the
> network for recovery — `recoverCEKEnvelope` takes the public LIT params (ciphertext/kid/authority) + a session view
> and returns a sealed `Buffer` (`chipotle-client.ts:1438`–`:1453`); the recovery secret stays in the node. Mirrored:
> (1) a NEW `dkms-authority` capsule (the node) OWNS the master key material (its own node-local durable store,
> resolved from `authority_key_store` or the `DKMS_AUTHORITY_KEY_STORE` env the provisioner sets) and exposes ONLY a
> `recover` op — recover the producer-escrowed CEK in-boundary (fail-closed on a forged producer / KID-swap / scheme
> mismatch / tamper), re-seal it to the decrypt session, return the `SealedDecryptMaterialV1` verbatim; it NEVER
> returns the CEK or the master (node tests=4: recover round-trip + open-to-CEK, stable identity across relaunch,
> forged-producer + before-init fail-closed, no-store fail-closed). (2) `key-provider`'s `dkms` backend now holds a
> PUBLIC-ONLY descriptor (schema `elastos.dkms.authority/v2`: `verifying_key_b64` + `recipient_pub_b64` +
> `authority_endpoint`, NOTHING secret) — a descriptor carrying `authority_master_seed_b64` is REJECTED fail-closed,
> and on `release` it DELEGATES recovery to the node (spawn the granted endpoint, JSON-RPC `init`+`recover`, return
> the node's sealed material) instead of deriving the secret locally; the runtime holds NO `ReferenceAuthority`
> (no signer, no recipient secret) for `dkms` (+1 test, 38→39: public-only resolution, secret-bearing rejection,
> incomplete-descriptor + missing-descriptor fail-closed). (3) `ddrm-runtime-open` threads it: the publish phase
> PROVISIONS the node (the master is generated + persisted in the node's OWN store; the runtime reads back only the
> public identity + writes a PUBLIC-ONLY descriptor + endpoint), and the bin ASSERTS the descriptor handed to the
> runtime carries NO master seed (and IS a complete public descriptor) — proving the master NEVER crosses into the
> runtime; `authority.dkms_authority_bin` is required for `dkms` (+1 bin test, 7→8). **Cross-binary proof:** the dkms
> smoke (`ddrm-consumer-dkms-smoke.sh` / `ddrm-consumer-smoke.sh --backend dkms`) publishes → provisions the node →
> the open DELEGATES recovery to the node → decrypts the segment, with the master seed NEVER entering the runtime
> (step 12: the external identity was PUBLIC-ONLY + read-only across the open); the reference path stays green. Drift
> untouched: no shared contract changed. Next: a real REMOTE node transport (a long-lived socket/RPC instead of a
> spawned child) + threshold shares across multiple nodes; a `lit` compat backend. Gate: ladder INTACT
> (+dkms-authority=4, +key-provider[key-authority-ref]=39), drift PASS, all dDRM smokes green (incl. the dkms
> variant), clippy clean.

> **🔀 Day 85–86 — the `dkms` EXTERNAL authority runs the open END-TO-END, and a backend SWAP is invisible to the open (Phase A wiring, LANDED).**
> Day 83–84 made `dkms` a fail-closed external-descriptor seam but unit-tested it only — the live smoke still
> ran `reference`. Day 85–86 drives `dkms` end-to-end and proves the backend swap is a one-field change.
> Audited PC2 first: (a) PC2 selects the backend PER STORED SESSION without changing the open path —
> `getSessionView(token)` reads `stored.backend` and dispatches to `WasmSessionView.fromStoredSession` vs
> `BackendSessionView.fromStoredSession` (`BackendSessionService.ts:368`–`:377`); the downstream handler
> consumes the resulting `ISessionView` agnostically. (b) PC2 treats the external authority descriptor as
> IMMUTABLE published data — the provisioned config is written ONCE to a cache (`writeFileSync(PROVISION_CACHE_PATH,
> …, mode 0600)`, `chipotle-client.ts:935`) and thereafter only READ (`ensureProvisioned` returns the cached
> descriptor `:950`–`:951`; `resolvePkpId` only reads `:963`–`:967`), never rewritten. Mirrored: (1)
> `ddrm-runtime-open`'s `OpenConfig` gained a typed `authority` block (`backend: reference | dkms`; fail-closed
> on an unknown/non-object authority); `KeyLauncher` now carries only a backend-specific `init_config` and the
> publish → launch → open → recover/re-seal flow is BYTE-IDENTICAL across backends — switching is a ONE-FIELD
> change (PC2's `getSessionView` dispatch). (2) The publish phase PROVISIONS the selected authority: for `dkms`
> it generates the key material via the reference authority on a durable store, then publishes an IMMUTABLE
> external descriptor (the persisted master seed + the published-identity pins) — the dKMS-node provisioning
> analogue. (3) `key-provider` now REQUIRES the dkms descriptor's pins (`verifying_key_b64` AND
> `recipient_pub_b64`): a pinless descriptor fails closed (a real external authority always publishes its
> identity), +1 test (37→38). (4) The bin PROVES the descriptor was READ-ONLY across the whole open (snapshot
> before launch, byte-compare after shutdown) — the key-provider only ever reads it. **Cross-binary proof:** a
> new sibling smoke `ddrm-consumer-dkms-smoke.sh` runs the consumer half with `authority.backend = dkms` —
> publish provisions the descriptor, the open RESOLVES the stable identity from it, recovers the publish-time
> escrow, decrypts the segment, and asserts descriptor immutability (step 12); `ddrm-consumer-smoke.sh
> [--backend reference|dkms]` runs either path, and the reference path stays green. Drift untouched: no shared
> contract changed. Next: a true REMOTE dKMS (resolve PUBLIC-only keys + DELEGATE recovery to the external node,
> vs today's provisioned-descriptor seam holding the key material); a `lit` compat backend. Gate: ladder INTACT
> (+key-provider[key-authority-ref]=38), drift PASS, all dDRM smokes green (incl. the new dkms variant), clippy clean.

> **🧩 Day 83–84 — the open BOOTS FROM CONFIG with NO smoke in the loop (`ddrm-runtime-open` bin) + the `dkms` EXTERNAL authority resolves a STABLE identity from a handed-in descriptor (Phase A wiring, LANDED).**
> Day 81–82 made escrow-at-publish work against a stable authority, but the host bootstrap was still
> the LAST smoke-owned bit (the consumer smoke assembled the `DrmHost` inline), and the `dkms` tag was
> still `not_configured`. Day 83–84 closes both. Audited PC2 first: (a) PC2 boots its authority/session
> service as a PROCESS-LIFETIME SINGLETON FROM CONFIG, not per request — `export const sessionService =
> new BackendSessionService(new FileSessionStore(SESSION_STORE_DIR))` constructed ONCE with the store dir
> derived from config (`BackendSessionService.ts:491`–`:497`, ctor `:266`); handlers use the singleton,
> never re-construct it; (b) PC2 resolves an EXTERNAL authority's key from a DESCRIPTOR rather than minting
> it — `resolvePkpId(config)` returns `config.pkpId`, else the supernode-auto-provisioned `provision.pkpId`,
> else `DEFAULT_PKP_ID` (`chipotle-client.ts:963`–`:967`, `:77`, `:938`); the `authority` address is
> likewise an external identity descriptor bound into the CEK composite hash (`:1318`, `:1346`–`:1350`).
> Mirrored with two seams: (1) a NEW default-on runtime-core entrypoint `scripts/dev/ddrm-runtime-open`
> (a `bin`, relocated from `ddrm-consumer-smoke`) — it reads a TYPED JSON CONFIG (`OpenConfig`: provider
> binaries, work dir, viewer, content id, `mode`; a missing path / unreadable file / malformed JSON /
> missing required binary / unknown mode all FAIL CLOSED), constructs the trusted `DrmHost` from
> `ProviderLauncher`s + a `DurableEventStore` via `DrmHost::launch`, runs the publish-time escrow fixture,
> and drives the open; `mode:"open"` is the operator path (publish → launch → open → persist → durable
> CEK-free readback), `mode:"verify"` ALSO drives the two adversarial fail-closed gates; +5 config-parse
> tests in the bin. (2) `key-provider` promotes `dkms` from `not_configured` to a FAIL-CLOSED
> EXTERNAL-authority seam — `init.config.dkms_authority_descriptor` (a path) RESOLVES the authority's stable
> ML-DSA signer + KEM recipient from a HANDED-IN descriptor (the dKMS-provisioned key material, READ never
> minted/persisted), VERIFIES the resolved identity against the descriptor's published
> `verifying_key_b64`/`recipient_pub_b64` pins (fail-closed on mismatch), and recovers/re-seals through the
> SAME `SealedDecryptMaterialV1` contract as the reference authority — so the durable-key-store stability
> pattern carries to a NON-reference authority, with the reference store as the local fixture for it;
> no descriptor → selected-but-unconfigured (the "no dKMS node provisioned" surface); a corrupt / wrong-schema
> / malformed-seed / identity-mismatched descriptor → init fails closed; +2 tests (35→37). **Cross-binary
> proof:** `ddrm-consumer-smoke.sh` no longer assembles a host — it WRITES an `OpenConfig` JSON and INVOKES
> `ddrm-runtime-open` (`mode:"verify"`), which owns publish → `DrmHost::launch` → open → durable CEK-free
> persist + the two adversarial gates; the 4 dDRM smokes stay green. Drift untouched: no shared contract
> changed. Next: a real REMOTE dKMS authority (resolve PUBLIC-only keys + DELEGATE recovery to the external
> node, vs today's provisioned-descriptor seam); a backend selector in `OpenConfig` so the bin can drive the
> `dkms` authority end-to-end. Gate: ladder INTACT (+key-provider[key-authority-ref]=37), drift PASS, 4
> smokes green, clippy clean.

> **🔑 Day 81–82 — the key authority gets a STABLE, durable-key-store identity, so the producer ESCROWS at PUBLISH time to a recipient any later launch re-derives; + `DrmHost::launch` composition helper (Phase A wiring, LANDED).**
> Day 79–80 made the host LAUNCH its rail, but the reference authority still minted a FRESH KEM recipient at
> every `init` — so the CEK could only be escrowed AFTER the authority launched (the "launch → publish →
> escrow → bind" dance), and a relaunch stranded everything escrowed to the prior recipient. Day 81–82
> closes that. Audited PC2 first: (a) PC2's authority is a STABLE, long-lived identity — `DEFAULT_AUTHORITY`
> (`0x09dBe796…`), the AuthorityGateway address baked into every video's PSSH at encode time, kept in
> lock-step across `storage.ts`/`chipotle-client.ts`/`dashPackager.ts:44` — vs the PER-OPEN `WasmSessionView`
> session key minted per request; (b) PC2 escrows the CEK to that stable authority at ENCODE/PUBLISH time —
> `encryptMediaCEK(cek, kid) → authority: DEFAULT_AUTHORITY` (`dashPackager.ts:131`–`:140`), the sealed
> envelope baked alongside the content, not recomputed at play time. Mirrored with three seams: (1)
> `ddrm-envelope` DETERMINISTIC key derivation — `mint_session_from_seed(seed)` (ML-KEM-768
> `generate_deterministic(d,z)` + x25519 from-seed via domain-separated SHA-256 sub-seeds, NO RNG,
> byte-identical), `derive_seed(master,label)`, `random_seed()`; +2 tests (14→16). (2) `key-provider`
> reference authority DURABLE KEY STORE — `init.config.authority_key_store` (a path) loads-or-creates +
> atomically persists (`*.tmp`→`rename`, mode 0600) ONE 32-byte master seed, then re-derives BOTH the signer
> and the KEM recipient from it, so the published recipient is STABLE across processes; FAIL-CLOSED on a
> corrupt store (never a silent re-mint); the dev default (no store) still mints fresh per init; +2 tests
> (33→35). (3) `ddrm-plan-runner` `DrmHost::launch(plan_source, launchers, events)` — the trusted-core
> composition helper that brings up its OWN rail (`from_launchers`) + wires the sink in one call; +2 tests
> (43→45). **Cross-binary proof:** `ddrm-consumer-smoke.sh` now has a PUBLISH phase (the producer role) that
> brings the durable-key-store authority up ONCE, escrows the CEK to its stable recipient, and writes a
> durable publish fixture; the OPEN phase builds the host via `DrmHost::launch`, RELAUNCHES the authority
> from the SAME store, PROVES the recipient is byte-identical across the relaunch (else fail-closed), READS
> the publish fixture (it never re-escrows), and binds ONLY the per-open session transcript AAD over the
> decrypt boundary's freshly-minted session key. Drift untouched: no shared contract changed. Next: fold
> the launchers + durable store into a default-on runtime-core `bin` entrypoint a NON-smoke caller drives
> (the producer/escrow already moved to a publish-time fixture; the bootstrap is the last smoke-owned bit);
> a real external dKMS authority backend (the `dkms` tag is still `not_configured`). Gate: ladder INTACT
> (+ddrm-envelope=16, +key-provider[key-authority-ref]=35, +ddrm-plan-runner=45), drift PASS, 4 smokes
> green, clippy clean.

> **🚀 Day 79–80 — the trusted host LAUNCHES the rail + persists through a PRODUCTION-SHAPED durable store, closing the two "still dev-shaped" gaps (Phase A wiring, LANDED).**
> Day 77–78 gave the host owned teardown + a persisting sink, but two gaps stayed dev-shaped: the smoke
> still PRE-SPAWNED the providers (the host received pre-provisioned capsules, it didn't bring the rail
> up), and the event store was a throwaway temp-dir writer (no atomicity / read-back guarantees). Day
> 79–80 closes both. Audited PC2 first: (a) PC2's runtime LAUNCHES + auto-provisions each backend
> connection — `BackendSessionService.createSession` launches a backend view (`BackendSessionService.ts:307`),
> and for the WASM backend `WasmSessionView.createNew()` calls `rt.sessionCreate()` to MINT a session +
> keypair INSIDE the runtime and PUBLISH only the public key (`chipotle-client.ts:603`–`:613`), the secret
> never crossing FFI — the service threads that published material into the stored session. (b) PC2's
> durable persistence is `FileSessionStore` — one JSON file per record id (`:140`–`:143`), `set` = mem +
> `persist` (mode 0600, `:145`–`:151`), and `loadAll` on construction restores every record across a fresh
> process, skipping corrupt/expired (`:153`–`:196`). Mirrored with two seams on the runtime-core:
> (1) a `ProviderLauncher` trait (`launch(self) -> Box<dyn ProviderTransport>`) + `RuntimeCapabilityTable::from_launchers`
> — the HOST brings the rail up by LAUNCHING each provider (spawn → init → the provider publishes its
> material) in caller-supplied dependency order; a failed launch tears down the partial rail and surfaces
> fail-closed. (2) a `DurableEventStore` (impl `EventStore`) — atomic write (`*.tmp` then `rename`),
> stable layout keyed by `content_id/event`, idempotent re-persist, fail-closed on I/O error, and a
> `DurableEventStore::load(dir)` read-back that returns every record on a fresh instance (skipping corrupt).
> +5 characterization tests (host launches the whole rail then drives + tears it down; `from_launchers`
> fails closed + tears down the partial rail; durable store persists + reads back across a fresh instance,
> atomic + idempotent; load skips a corrupt record; the host persists durably through the real store) →
> ddrm-plan-runner 38→43. **Cross-binary proof:** `ddrm-consumer-smoke.sh` shrinks again — it hands the
> host three `ProviderLauncher`s (each owning a capsule BINARY, not a pre-spawned process); `from_launchers`
> spawns + inits all three (key authority + decrypt boundary publish their material into a shared rail),
> the runtime binds the cross-provider open material (escrow to the published recipient + transcript AAD
> over the published session key), and the sink is `PersistingEventSink` over the `DurableEventStore`. The
> smoke proves durability by reading the records back through a FRESH `DurableEventStore::load` (a brand-new
> reader, as if a separate process) and asserts no CEK/ciphertext/escrow/session-key leaked. Drift
> untouched: the host consumes the plan, defines no shared contract. Next: a PRODUCTION key-authority
> backend (the reference authority still mints keys at init); folding the durable store + launchers into a
> default-on runtime-core entrypoint a non-smoke caller drives. Gate: ladder INTACT (+ddrm-plan-runner=43),
> drift PASS, 4 smokes green, clippy clean.

> **🔌 Day 77–78 — the trusted host OWNS THE RAIL + PERSISTS the open: host-owned transport teardown + a durable CEK-free event sink, fail-closed (Phase A wiring, LANDED).**
> Day 75–76 gave the core a trusted host (`DrmHost`) that fetches the plan, drives the registry, and
> EMITS the runtime-event steps — but the transports were torn down by the smoke's inline cells, and the
> events went to a throwaway in-memory note. The host didn't yet OWN the rail's teardown or PERSIST the
> open. Day 77–78 closes both. Audited PC2 first: (a) the per-view transport OWNS a releasable resource
> and tears it down on `dispose()` — `WasmSessionView.dispose()` calls `requestDrop(this._requestHandle)`
> then nulls it (`chipotle-client.ts:694`–`:698`); `dispose()` is part of the `ISessionView` contract
> (`:231`); the view is opened by `createNew`/`fromStoredSession` (`:603`/`:621`). (b) PC2 PERSISTS the
> open as a lifetime-managed session — `mediaSessionManager.create` mints it with a lifecycle
> (`sessionManager.ts:50`–`:80`), TTL-expires + `cleanup`/`destroy` tear it down (`:104`–`:123`), a
> process singleton (`:126`) holding the CEK server-side and OUT of the returned record (`:5`–`:6`, `:18`).
> So PC2's runtime owns open→use→teardown of each transport AND persists a CEK-free record of the open.
> Mirrored with two seams on the runtime-core host: (1) `ProviderTransport::shutdown` (the analogue of
> `dispose()`) + `RuntimeCapabilityTable::shutdown` (tears down ALL transports, best-effort then surfaces
> the first error) + `DrmHost::shutdown(self)` (consumes the host, so its capabilities can't be used after
> teardown) — the runtime that OWNS the transports owns their teardown, fail-closed. (2) An `EventStore`
> seam (`persist(key, record)`) + a `PersistingEventSink` that builds a CEK-FREE record via
> `open_event_record` (event + open identity + `steps_run` + `decrypt_session_opened` + artifact NAMES,
> NEVER artifact VALUES) and writes one per runtime event; a store that cannot persist a declared event
> fails the open. +4 characterization tests (host shutdown tears down every owned transport; shutdown
> fails closed when a transport cannot release — best-effort then surfaces; the persisting sink writes
> CEK-free records for every runtime event, names not values; the open fails closed when the store cannot
> persist the audit, the receipt having persisted first) → ddrm-plan-runner 34→38. **Cross-binary proof:**
> `ddrm-consumer-smoke.sh` shrinks further — the three transports now OWN their capsules (each `shutdown`s
> the capsule it owns), so `host.shutdown()` tears down the WHOLE rail (no manual per-capsule shutdown in
> the smoke), and the event sink is the lib's `PersistingEventSink` over a `FileEventStore` writing
> durable records to a temp dir. The smoke reads the records back and asserts both the receipt + audit
> persisted AND that no record leaks the CEK, ciphertext, escrowed material, or session/producer keys
> (only artifact NAMES + open metadata). Drift untouched: the host consumes the plan, defines no shared
> contract. Next: a PRODUCTION persisting store (durable receipt/audit beyond a temp dir) + the host
> spawning/connecting to the rail it owns (the transports still wrap capsules the smoke provisioned for
> their published keys) so the open runs default-on inside the core; a production key authority backend.
> Gate: ladder INTACT (+ddrm-plan-runner=38), drift PASS, 4 smokes green, clippy clean.

> **🧭 Day 75–76 — the runtime CORE gets a single TRUSTED HOST: `DrmHost::open(content_id, viewer)` owns plan-fetch + drive-over-registry + runtime-event emission, fail-closed (Phase A wiring, LANDED).**
> Day 74 gave the core a runtime-OWNED capability registry (`RuntimeCapabilityTable`), but the only thing
> that COMPOSED the whole open — fetched the plan, registered transports, drove `open_drm_plan` — was the
> consumer smoke's inline `run()`. There was still no runtime-core entrypoint that owns the open the way a
> server owns a request. Day 75–76 builds it. Audited PC2's server-owned composition first: the Express
> `/init` route is the ONE place that owns the whole open — `router.post('/init', authenticate,
> requireSecureViewSession, handler)` (`src/api/media.ts:133`). Once the middleware resolves the capability
> into request state, the route handler owns fetching + parsing the MPD (the "what to open" /
> plan-equivalent), reading the resolved handle from state and driving recovery (`:481` `const sessionView =
> req.secureViewSession!.view` → `:482` `recoverMediaCEK(litParams, sessionView)`), CREATING the playback
> session that lives for the duration (`:489` `mediaSessionManager.create`), and logging the open throughout
> (`:483`, `:518`) — all fail-closed in one place (the `catch` → 500, `:528`). So PC2 has ONE owned
> entrypoint that fetches the plan-equivalent, drives recovery over the resolved capability, and performs the
> runtime-owned post-steps (session create + audit log). Mirrored with three owned collaborators on a new
> `DrmHost`: a `PlanSource` (`fetch(content_id, viewer) -> plan` — the runtime's seam to `drm-provider`, the
> analogue of the MPD fetch), the Day-74 `RuntimeCapabilityTable` (the registry the recovery drives over),
> and a `RuntimeEventSink` (`emit(event, &ExecutionReport)` — the analogue of `mediaSessionManager.create` +
> the open log). `host.open(content_id, viewer)` FETCHES the plan, drives it through the registry
> (`open_drm_plan`'s parse → resolve → execute), then EMITS the plan's runtime-event steps in order. New
> `PlanStep.event` + `is_runtime_event()` (a step with no provider that carries an `event` — `release_receipt`,
> `protected_content.open.audit`) lets the host emit the runtime-OWNED post-steps the executor only walks for
> ordering; no provider performs them. **Fail-closed at every seam:** a bad plan never resolves a capability
> (parse precedes resolve), a missing transport fails closed, and a declared runtime event the sink cannot
> emit fails the whole open. +5 characterization tests (host opens via plan source + registry + emits BOTH
> runtime events in order; a tampered plan FROM THE SOURCE fails closed with no event emitted; the sink
> refusing the audit fails the open — receipt emitted first, then audit refused; an unregistered required
> transport fails closed; runtime-event steps parse) → ddrm-plan-runner 29→34. **Cross-binary proof:**
> `ddrm-consumer-smoke.sh` is now a THIN caller — it provisions the capabilities, REGISTERS the three
> transports and wires a `SmokePlanSource` (the real `drm-provider`) + a `SmokeEventSink` into a `DrmHost`,
> and calls `host.open(content_id, viewer)` (the SAME host entrypoint the trusted core will call; the capsule
> binaries are the host's registered transports + plan source). The runtime now emits the `release_receipt` +
> `audit` post-steps the plan declares (step 7). The tampered-edge gate flips the plan source into TAMPER mode
> and re-opens through the SAME host — a corrupt plan FROM THE SOURCE fails closed at the real key-provider,
> emitting no event. Drift untouched: the host consumes the plan, defines no shared contract. Next: give the
> host REAL owned transports (spawn/connect to the provider rail the runtime owns) + a persisting event sink
> (durable receipt + audit), so the host runs from capabilities + a sink the core itself owns end to end.
> Gate: ladder INTACT (+ddrm-plan-runner=34), drift PASS, 4 smokes green, clippy clean.

> **🏭 Day 74 — the runtime CORE OWNS the capabilities: `RuntimeCapabilityTable` registers a `ProviderTransport` per provider, resolves a fresh handle over it, fail-closed (Phase A wiring, LANDED).**
> Day 73 gave the core a composition root (`open_drm_plan`) over a `CapabilityTable` trait, but the
> only concrete table was the consumer smoke's bespoke `SmokeCapabilityTable` — the capabilities were
> still ones a dev harness hand-built, not ones the core OWNS. Day 74 builds the runtime-owned registry.
> Audited PC2's transport ownership first: the runtime owns the capability factory as a process-lifetime
> singleton — `export const sessionService = new BackendSessionService(new FileSessionStore(...))`
> constructed ONCE at module load with a runtime-injected store (`BackendSessionService.ts:495`, ctor
> `:266`) — and `getSessionView(token)` dispatches on `stored.backend` (`:371`) to CONSTRUCT the
> per-backend transport it owns the means to build (`WasmSessionView.fromStoredSession` `:374` /
> `BackendSessionView.fromStoredSession` `:377`), returning `null` for an unknown token/backend (`:370`).
> So the runtime owns the transports the factory hands out; a request supplies only a token, and an
> unknown one fails closed. Mirrored with two distinct types: `ProviderTransport` — the runtime-owned,
> long-lived capability to drive ONE provider, `register`ed once into the table at startup (the analogue
> of the per-backend view constructors the `sessionService` singleton owns) — and `ProviderHandle` — the
> FRESH per-open handle the transport `open`s (the analogue of a `BackendSessionView` minted per request).
> `RuntimeCapabilityTable` is the registry: `register(transport)` rejects a duplicate provider (one owner,
> never a silent override) and `resolve(provider)` opens a fresh handle over the registered transport or
> returns `None` (→ `open_drm_plan` fails closed at `resolve_from`). +4 characterization tests (registered
> transports drive the plan; an unregistered required provider → `None` → the open fails closed with zero
> step invocations; a duplicate registration is rejected; a fresh handle is opened per open over the same
> registered transports — proving the runtime reuses the owned transport across opens like PC2 reuses the
> singleton across requests) → ddrm-plan-runner 25→29. **Cross-binary proof:** `ddrm-consumer-smoke.sh`
> no longer hand-rolls a bespoke table — it REGISTERS three runtime-owned transports
> (`RightsTransport`/`KeyTransport`/`DecryptTransport`, each wrapping one real capsule binary) into the
> lib's `RuntimeCapabilityTable` (the SAME registry type the trusted core uses) and drives both the
> canonical open AND the tampered-edge re-run through `open_drm_plan` — no second code path; both
> fail-closed gates ride along unchanged. Drift untouched: the registry consumes the plan, defines no
> shared contract. Next: construct the `RuntimeCapabilityTable` inside a trusted runtime-core caller whose
> transports drive the runtime's REAL provider→provider rail (the smoke proves the registry; the core
> owns the real transports). Gate: ladder INTACT (+ddrm-plan-runner=29), drift PASS, 4 smokes green,
> clippy clean.

> **🚪 Day 73 — the runtime CORE gets a single COMPOSITION ROOT: `open_drm_plan(plan, &mut CapabilityTable)` resolves each handle from a runtime table at one entrypoint, fail-closed (Phase A wiring, LANDED).**
> Day 72 gave the executor a `RuntimeStepRunner` over injected handles, but the only thing that
> COMPOSED a runner (resolved the handles, built it, ran it) was the consumer smoke's inline `run()`
> — there was no runtime-core entrypoint the trusted runtime could call to open a plan. Day 73 builds
> it. Audited PC2's composition root first: the secure-view middleware resolves the per-stage handle
> ONCE from a backend-keyed factory — `sessionService.getSessionView(token)` dispatching on
> `stored.backend` (`src/services/session/BackendSessionService.ts:368`) — and attaches it to request
> state (`secureViewSession.ts:124`→`:129` `req.secureViewSession = { stored, view }`); the route
> handler then reads the handle FROM that state and invokes it without re-resolving (`media.ts:481`
> `const sessionView = req.secureViewSession!.view` → `:482` `recoverMediaCEK(litParams, sessionView)`,
> the helper taking `session` as a parameter `:1192`), and the middleware doc explicitly forbids
> handlers re-loading by token (`secureViewSession.ts:13`). So PC2 has ONE composition root (the
> middleware) that resolves a capability from a backend-keyed table and hands it downstream; nothing
> re-resolves. Mirrored: a new `CapabilityTable` trait (`fn resolve(&mut self, provider) ->
> Option<Box<dyn ProviderHandle>>`) is the runtime-core analogue of the backend-keyed session factory;
> `RuntimeStepRunner::resolve_from(plan, table)` is the composition-root constructor — it calls
> `table.resolve` once per provider the plan's `next_required_providers` names, fails closed if the
> table holds no capability for a required provider, and rejects a table that hands back a handle for
> the wrong provider (then the Day-72 `new` re-checks required/stray/duplicate); and `open_drm_plan`
> ties parse→resolve→execute into the SINGLE entrypoint the trusted runtime calls. **Fail-closed
> ordering:** `open_drm_plan` parses the plan BEFORE touching the table, so a non-`planned`/foreign
> plan never reaches the runtime's capabilities; a withheld required provider fails closed without
> executing a single step. +4 characterization tests (drives the plan via the table resolving each
> required provider exactly once, in order; withheld required provider fails closed + zero invocations;
> misrouting table rejected; non-planned plan refused before any resolve) → ddrm-plan-runner 21→25.
> **Cross-binary proof:** `ddrm-consumer-smoke.sh` no longer hand-builds the runner — it supplies a
> `SmokeCapabilityTable` (holding the live capsule cells + provisioned session material, handing out
> fresh `RightsHandle`/`KeyHandle`/`DecryptHandle` on `resolve`) and calls `open_drm_plan` for BOTH the
> canonical open AND the tampered-edge re-run — the SAME entrypoint, no second code path; both
> fail-closed gates ride along unchanged. Drift untouched: the entrypoint consumes the plan, defines no
> shared contract. Next: stand `open_drm_plan` up inside a trusted runtime-core caller wired to the
> real provider→provider rail (the smoke proves the entrypoint; the core supplies the real table).
> Gate: ladder INTACT (+ddrm-plan-runner=25), drift PASS, 4 smokes green, clippy clean.

> **🔌 Day 72 — the runtime CORE injects per-provider capability handles into the executor: `RuntimeStepRunner` over injected `ProviderHandle`s, fail-closed (Phase A wiring, LANDED).**
> Day 71 made the core EXECUTE the plan, but the only `StepRunner` was the consumer smoke's
> monolithic `SmokeRunner` — one struct that held every capsule handle and `match`ed on the step
> name. There was no runtime-core seam that takes the runtime's capabilities and resolves each step
> through them. Day 72 builds it. Audited PC2's per-stage capability injection first: the secure-view
> middleware RESURRECTS a `BackendSessionView` once per request (`src/api/middleware/secureViewSession.ts:124`)
> and THREADS that handle into the downstream stage — `media.ts:1207` hands `session` into
> `recoverMediaCEK`→`recoverCEKEnvelope`, and the `/segment` route reuses the SAME injected view
> (`media.ts:541`); a stage never opens its own connection, it uses the handle it was given. Mirrored:
> a new `ProviderHandle` trait is the single injected capability (the runtime-core analogue of the
> session view), and `RuntimeStepRunner` IMPLEMENTS the Day-71 `StepRunner` over a `BTreeMap<provider,
> handle>` — it walks each plan step to the handle registered for that step's `provider`, and holds NO
> authority of its own (no CEK, no RPC; it only routes to handles). **Fail-closed construction**
> (`RuntimeStepRunner::new`): every provider the plan's `next_required_providers` names (normalized
> `key-provider`→`key`) MUST have an injected handle — no ambient default, the core cannot fabricate a
> missing capability — and a STRAY handle for a provider the plan does not name is REJECTED, so a
> capability the plan never authorized can never enter the runner and the `blocked_authority` set is
> structurally unreachable from the runner type. A provider-call step with no handle and not required
> (the `content` status/fetch steps this chain does not drive) is a runtime no-op; a required provider
> can never be missing because construction proved it. +7 characterization tests (runtime runner drives
> the plan through injected handles in canonical order; refuses to build without a required handle;
> rejects a stray unnamed handle; rejects duplicate handles; never invokes a handle for an unnamed
> provider; parses + normalizes `next_required_providers`) → ddrm-plan-runner 14→21. **Cross-binary
> proof:** `ddrm-consumer-smoke.sh`'s monolithic `SmokeRunner` is GONE — replaced by three per-provider
> handles (`RightsHandle`/`KeyHandle`/`DecryptHandle`, each wrapping ONE real capsule binary, the
> binaries becoming the injected handles) constructed into the SAME `RuntimeStepRunner` the trusted core
> will use with real providers (no second code path). The plan is fetched from the REAL `drm open`,
> parsed + driven through `DrmOpenPlan::execute(&mut runtime_runner)`, and both fail-closed gates ride
> along: a transcript-mismatched seal must not open, and a TAMPERED binding edge is rejected
> cross-binary by the real `key-provider`. Drift untouched: the runner consumes the plan, defines no
> shared contract. Next: stand the `RuntimeStepRunner` up inside the trusted core wired to the real
> provider→provider rail (the smoke proves the seam; the core supplies the real handles). Gate: ladder
> INTACT (+ddrm-plan-runner=21), drift PASS, 4 smokes green, clippy clean.

> **🧭 Day 71 — the runtime CORE executes the open plan: `ddrm-plan-runner` walks `DrmOpenPlanV1`, fail-closed (Phase A wiring, LANDED).**
> Day 67 made `drm-provider::open` emit a typed, executable `DrmOpenPlanV1` (the canonical
> sequence + binding edges), but the only thing that actually FOLLOWED that plan was the
> hand-written consumer smoke — it read the order + edges off the plan, yet the walk itself was
> inline literal code. Day 71 extracts that walk into the runtime core. Audited PC2's open
> sequencer first — each stage gated on the prior: `requireSecureViewSession` resurrects the
> backend session view (`src/api/middleware/secureViewSession.ts:61`), then `recoverMediaCEK` →
> `recoverCEKEnvelope` (`src/api/media.ts:1180`, `:1196`), whose access gate is the Lit action's
> `hasAccessByContentId(del.ownerAddress, kid)` (`:1163`) and which only THEN unwraps the CEK
> in-boundary (`:1216`) — a missing/failed prior stage short-circuits the whole open. New library
> `capsules/ddrm-plan-runner`: `DrmOpenPlan::parse` validates the plan (schema, `planned` status,
> the `rights_check<key_release<decrypt_session` canonical order, every binding edge naming real
> steps + the `content_id==object_cid` identity), and `execute` seeds the virtual `drm_open`
> identities, walks the steps IN ORDER, threads each binding edge's produced artifact into the next
> step's plan-declared `into_field`, and FAILS CLOSED on a step that needs an artifact not yet
> produced (out-of-order / a silently-failed prior step) or that runs without emitting the artifact
> the plan says it produces. **No authority:** the executor performs no I/O and holds no capability
> — the ONLY thing that can reach a provider is the injected `StepRunner` (exactly the
> `blocked_authority` set the plan advertises). 14 characterization tests pin it (valid plan drives
> the canonical sequence in order + threads the edges + seeds the identities; renamed-edge,
> dropped-artifact, backward-edge, out-of-order, wrong-schema, identity-split, and no-authority all
> fail closed). **Cross-binary proof:** `ddrm-consumer-smoke.sh` no longer hand-walks the chain — it
> fetches the REAL `drm open` plan, parses it through the core, and drives the real
> drm→rights→key→decrypt binaries THROUGH `DrmOpenPlan::execute` (the smoke supplies a `SmokeRunner`
> that injects the live capsule handles + session material per step), and a TAMPERED binding edge is
> rejected cross-binary by the real `key-provider` (`deny_unknown_fields` over a required
> `rights_receipt`). New ladder rung ddrm-plan-runner=14 (host-side core, NOT a wasm capsule — the
> runtime drives the wasm capsules, it is not one). Drift untouched: the executor consumes the plan,
> it defines no shared contract. Next: inject the runtime's REAL provider→provider rail in place of
> the smoke's spawned binaries so the open runs default-on inside the core (the `StepRunner` trait is
> that seam). Gate: ladder INTACT (+ddrm-plan-runner=14), drift PASS, 4 smokes green, clippy clean.

> **🔑 Day 70 — the canonical `key-provider::release` actually releases (reference backend): recover-from-escrow → re-seal to session (Phase A, LANDED).**
> Days 50–60 made `key-provider` a pluggable multi-backend authority, landed the reference
> seal engine + the shared `ddrm-envelope`, and added the dev `release_ref`/`release_from_escrow_ref`
> ops — but the CANONICAL `release(KeyReleaseRequestV1)`, the op `drm-provider`'s `DrmOpenPlanV1`
> (Day 67) names for the key step, was still a `not_configured` stub for the reference backend
> ("seal engine lands in Phase A.2"), and the consumer smoke handed the authority a RAW golden CEK
> via `release_ref`. Day 70 closes both. Audited PC2's Lit authority first
> (`data/lit-actions/universal-decrypt-chipotle.js`): the action runs access-check
> `hasAccessByContentId` (`:560–568`) → recover the CEK `Lit.Actions.Decrypt` (`:570–575`) → recompute
> `sha256(cek‖kid‖authority)` and `deny("kid_authority_mismatch")` on mismatch (`:577–590`) →
> seal-to-session `envelopeCEK` (`:602–608`), returning only the sealed envelope; the client
> sequencer signs the request and returns sealed-only (`chipotle-client.ts::recoverCEKEnvelope`
> `:1438–1538`). Mirrored: `release` validates the rights receipt (always, before any backend), then
> for the reference backend RECOVERS the producer-escrowed CEK from the rights-bound
> `key_envelope.wrapped_cek` — recomputing the SHARED `escrow_aad(scheme, kid16, recipient_pub)` and
> verifying the producer vk — and re-seals it to the runtime-injected decrypt session as the
> suite-tagged `SealedDecryptMaterialV1` the decrypt sandbox opens (reusing the proven
> `recover_escrowed_cek` + `seal_recovered_cek_into_material`). The wrapped CEK rides INSIDE the
> validated request (not a side-band param); the per-session material (decrypt session key + producer
> vk + transcript + optional clock) is injected by the runtime in a `session` context on the op
> envelope — capsule-local, so the shared `KeyReleaseRequestV1` stays byte-identical and the drift
> gate is untouched. **Fail-closed:** no backend or no session context → `not_configured`; denied or
> mismatched rights receipt, an expired request (when a clock is supplied), a KID-swap, a scheme
> mismatch, or a forged producer → recover/validation refuses; the CEK lives only in `Zeroizing`
> inside the boundary and leaves only SEALED (never echoed). key-provider key-authority-ref 27→33
> (+`canonical_release_recovers_escrow_and_seals_to_session`, +denied/expired/kid-swap/forged-producer/
> missing-session fail-closed); default stays 18. **Cross-binary proof:** `ddrm-consumer-smoke.sh` now
> ESCROWS the golden CEK to the authority's published recipient and drives the CANONICAL `release`
> (recover→reseal) instead of the raw-CEK `release_ref` shim, so the whole consumer half
> (drm→rights→key→decrypt) runs with NO raw CEK handed in anywhere — through the exact op the Day-67
> plan names — and a transcript-mismatched seal still fails closed. **Gate:** key-provider=18/33,
> ladder INTACT (+wasm), drift PASS, consumer/producer/publish/market smokes green, clippy clean.
> **Next:** the runtime core executing the Day-67 plan default-on (driving this canonical `release`
> itself), a live-Base read-only round trip, or the producer-side IPFS pin.
>
> **📦 Day 69 — `encrypt-provider::seal` runs the full production pipeline on handed-in bytes → complete `SealedObjectV1` (Phase C, LANDED).**
> The dev `seal_inline` (Day 60/68) proved the producer crypto + content-addressing, but the
> PRODUCTION `seal` op was still the Day-1 fail-closed skeleton (`seal → not_configured`) — so
> the chain-shaped sealed object was only ever hand-built in tests, never emitted by the boundary.
> Day 69 closes that gap. Audited PC2's producer INPUT path first
> (`src/services/media/dashPackager.ts`): the CEK is minted in the host (`generateCEK` `:122–126`),
> each fMP4 segment is read off disk (`readFileSync` `:504`, `:571–572`), and the BYTES are handed
> to the CENC WASM (`executeCENCEncrypt(wasmBinary, segCommand, seg.data, …)` `:432–434`) — the
> encoder NEVER fetches; the host resolves the reference to bytes and passes them in. Mirrored
> faithfully: `SealRequest` gained `content_b64` (the handed-in asset bytes), `recipient_pub_b64`
> (the authority's published escrow recipient), and `availability_receipt_cid` (the pin receipt
> from the storage step) — all optional, `deny_unknown_fields` preserved, so the existing
> fail-closed tests still hold. When bytes + recipient are present, `seal` runs the ONE canonical
> in-boundary pipeline — `run_seal_pipeline`: mint CEK+KID → CENC-encrypt the handed-in bytes →
> content-address (`payload_cid`, Day 68) → escrow the CEK SEALED to the authority — and assembles
> a complete `elastos_common::protected_content::SealedObjectV1`: real `payload_cid`,
> `key_envelope.kid` == bytes16 contentId (Day 58), `policy_hash = sha256(rights_policy_cid)`
> (binds the envelope to the handed-in policy without fetching the doc), and the PQ-hybrid algorithm
> suite the whole chain validates. `seal_inline` now DELEGATES to the same `run_seal_pipeline`
> (one pipeline, two front doors — PRINCIPLES #10). The capsule acquires NO fetch/IPFS/network
> authority — it seals the bytes it is handed, exactly like PC2's WASM. **Fail-closed:** no recipient
> or no bytes → `not_configured`; missing availability receipt, empty viewer interface, or empty
> content → `invalid_request` — `seal` never emits a partial object. encrypt-provider escrow 22→25
> (+`configured_seal_emits_complete_sealed_object`, +`each_seal_freshly_mints_and_addresses`,
> +`seal_fails_closed_on_missing_inputs`); default stays 20. **Cross-binary proof:**
> `ddrm-producer-smoke.sh` now drives the REAL production `seal` op, deserializes the response into
> the SHARED `SealedObjectV1` type (so its full shape + `deny_unknown_fields` hold) and runs the
> SAME `validate_protected_content_key_envelope_algorithms` the downstream `key-provider` runs —
> proving cross-binary that the producer emits an object the chain accepts; asserts `payload_cid`
> (`bafkrei…`) ≠ KID (distinct identities) and that no plaintext appears (the production output
> carries the sealed object only — no segment at all). **Gate:** encrypt-provider=20/25, ladder
> INTACT (+wasm), drift PASS, consumer/producer/publish/market smokes green, clippy clean on the
> touched code. **Next:** a real key-authority backend so the dev escrow shim drops out, a live-Base
> read-only round trip, or runtime-core execution of the Day-67 `DrmOpenPlanV1`.
>
> **🧮 Day 68 — `encrypt-provider` content-addresses the ciphertext: `payload_cid` is REAL (Phase C, LANDED).**
> The producer half had one last "trust me": the sealed object's `payload_cid` (the IPFS
> address of the ciphertext) was a hardcoded `bafybeig…` placeholder in the smoke, never
> derived from the bytes. Day 68 closes it. Audited PC2's producer storage path first
> (`src/storage/ipfs.ts`): content is stored via Helia `unixfs.addBytes` (`storeFile`
> `:644–678`, `fs.addBytes(data)` `:659`, returns `cid.toString()` `:673`), and the importer
> defaults (`node_modules/@helia/unixfs/src/commands/add.ts:15–24`) are `cidVersion: 1,
> rawLeaves: true`, `fixedSize({ chunkSize: 1_048_576 })` — so any content ≤ 1 MiB is a single
> chunk whose root CID IS the lone raw leaf's CID (codec `raw` 0x55, sha2-256), a PURE function
> of the bytes. `encrypt-provider` now derives exactly that in-boundary: `payload_cid_v1_raw`
> = multibase-base32(`0x01 0x55 0x12 0x20 ‖ sha256(segment)`), and `seal_inline` returns it.
> NO `kubo_api`, no network — a content address is not a pin (pinning stays a separate, later
> capability); fail-closed above one chunk (a multi-block balanced dag-pb tree we refuse to
> emit rather than guess). **Golden:** three inputs pinned to the EXACT strings PC2's real
> `ipfs-unixfs-importer` emits (generated via the ecosystem oracle during the audit; incl.
> `abc → bafkreif2pall7…`, the canonical raw-`abc` CID — an independent cross-check against
> the whole IPFS ecosystem), so a codec/multibase drift fails loudly. encrypt-provider 17→20
> (default) and 19→22 (escrow): +golden, +deterministic/collision-sensitive, +fail-closed-
> above-one-chunk. **Cross-binary proof:** `ddrm-producer-smoke.sh` now captures the running
> binary's `payload_cid`, INDEPENDENTLY recomputes the segment's CID via the canonical IPLD
> `cid` crate (a different encoding path, not a copy of our code), and demands a byte-for-byte
> match (and `bafkrei…` shape) — the producer's content address provably resolves to the bytes
> it sealed. `payload_cid` (IPFS address) remains a SEPARATE identity from the KID/`contentId`
> (the chain ownership key, Day 58) — the smoke proves both without conflating them. **Gate:**
> encrypt-provider=20/22, ladder INTACT (+wasm, incl. the new `sha2` dep building clean on
> wasm32-wasip1), drift PASS, consumer/producer/publish/market smokes green, clippy clean on
> the touched code. **Next:** real `plaintext_ref`→bytes resolution in `seal` (so the
> non-inline path content-addresses too), a key-authority backend so the dev escrow shim drops
> out, or a live-Base read-only round trip.
>
> **🎛️ Day 67 — `drm-provider::open` emits the executable `DrmOpenPlanV1`: the orchestrator is real (Phase A wiring, LANDED).**
> Days 45–66 built the decrypt boundary, the consumer half, and the producer→chain→
> discovery spine — but the keystone, the `drm/open` orchestrator, was still the Day-1
> fail-closed skeleton (`open → not_configured`) while the consumer smoke HARDCODED the
> rights→key→decrypt order itself: a PRINCIPLES #10 violation (two copies of the one
> canonical path). Day 67 moves that path into the capsule that owns it. Audited PC2 first
> (`data/lit-actions/universal-decrypt-chipotle.js` `main`): the open flow runs in fixed,
> fail-closed order — on-chain access-check `hasAccessByContentId` (`:545–568`) → key-release
> `Lit.Actions.Decrypt` (`:570–575`) → CEK↔KID↔authority binding (`:577–590`) → seal-to-session
> `envelopeCEK` (`:602–608`), each prerequisite gating the next via `deny(...)`; and the client
> sequencer `chipotle-client.ts::recoverCEKEnvelope` (`:1438–1538`) signs a per-asset request
> (`:1475–1478`), assembles the bound jsParams (`:1486–1510`), runs the action (`:1513`), and
> returns ONLY the sealed envelope (`:1531`) — never the CEK. New `drm-provider::open` mirrors
> this as a capsule-owned plan: it validates the request fail-closed (unsupported action,
> non-`SealedObjectV1`, weak cipher / missing hybrid-PQ KEM, hidden authority via
> `deny_unknown_fields`) then returns a typed **`DrmOpenPlanV1`** with status `planned` (never
> `opened`) — the 8-step canonical sequence, the **binding edges** (`rights_check ⇒
> RightsDecisionReceiptV1 → key_release.rights_receipt`; `key_release ⇒ ReleaseReceiptV1 →
> decrypt_session.release_receipt`; `drm_open ⇒ content_id → rights_check`; `⇒ object_cid` /
> `viewer_interface → decrypt_session`), the next-required providers, the runtime events, and
> the advertised `blocked_authority`. The content identity is the **KID** (== `bytes16
> contentId`, Day-58 join), emitted under BOTH `content_id` and `object_cid` because
> `key-provider` enforces `rights_receipt.content_id == object_cid` (`key-provider:740`) — one
> identity that cannot drift between the rights check and the decrypt session. The capsule holds
> NO `raw_cek`/`key_backend_sdk`/`wallet_rpc`/`chain_rpc`/`kubo_api`/`elacity_sdk`: it PLANS, the
> runtime EXECUTES (the "core injects capabilities" pattern, exactly like Day-61
> `publish-provider`). `DrmOpenPlanV1` is **capsule-local** (like `UnsignedMintV1`), so the
> shared `protected_content` surface + drift gate are untouched. **Proof:** drm-provider 12→15
> (+`planned`-plan/never-opened, canonical-sequence+events, one-identity-under-both-names,
> receipt-binding-edges, no-raw-authority; existing fail-closed cases unchanged) +
> `ddrm-consumer-smoke.sh` now drives the REAL `drm open`, asserts the `planned` plan + content
> identity + action + canonical order + binding edges, and FOLLOWS the plan (threads each
> receipt into the plan-declared field, content identity from the plan) instead of a hardcoded
> sequence. **Gate:** drm-provider=15, ladder INTACT (+wasm), drift PASS, consumer/producer/
> publish/market smokes green, clippy clean. **Next:** wire a real key-authority backend so the
> dev escrow shim drops out, a live-Base read-only round trip, or real `plaintext_ref`→IPFS in
> the producer op.
>
> **📡 Day 66 — `listing_from_event`: the chain's own log reconstructs the same listing (Phase C, LANDED).**
> Day 64 reconstructed a listing from the calldata WE assemble; Day 66 closes the gap to
> what the chain actually EMITS, so discovery works against real `eth_getLogs` output, not
> just our own intent. Audited PC2's `ContentIndexerService` event handling: the topic
> hashes (`ContentIndexerService.ts:59–63`), `DigitalAssetRegistered(address indexed
> channel, uint256 indexed tokenId, address creator, string tokenURI, uint16 opType,
> bytes16 contentId)` (lines 857–896, `data = abi.encode(creator, tokenURI, opType,
> contentId)`, channel in topics[1]), and `AssetCreated(address indexed _to, address indexed
> _channel, uint256 _tokenId, string _tokenUri, uint16 _opType, address indexed opContract)`
> (lines 922–967, `data = abi.encode(tokenId, tokenURI, opType)`, channel in topics[2], NO
> contentId on-chain). New `content-market::listing_from_event` decodes `{topics, data,
> address}`: `DigitalAssetRegistered` yields the FULL identity (its on-chain `bytes16
> contentId` == the same KID the calldata path produced, `metadata_status:"unresolved"`);
> `AssetCreated` carries no contentId, so rather than guess it emits
> `metadata_status:"needs_kid"` and defers identity to `enrich_listing`'s authoritative
> kid-match — fail-closed by construction, no phantom identities. Still pure: the log bytes
> are handed in by `chain-provider` (named in `enrich_requires`), no RPC. Fail-closed on an
> unrecognized topic, missing topics, truncated/overflowing data, a bad emitter address, or
> an unknown opType. **Proof:** +7 tests (DigitalAssetRegistered ≡ calldata identity,
> AssetCreated defers with `needs_kid`, 5 fail-closed cases) + `ddrm-market-smoke.sh`
> extended to build a `DigitalAssetRegistered` log carrying our contentId and assert the
> event path agrees with the calldata path cross-binary. **Gate:** content-market=29,
> ladder INTACT (+wasm), drift PASS, market/publish/producer/consumer smokes green, clippy
> clean. **Next:** a live-Base read-only round trip (real logs → reconstruct → enrich), or
> real `plaintext_ref`→IPFS in the producer op.
>
> **🏷️ Day 65 — `enrich_listing`: human-facing fields fused onto the calldata identity, fail-closed (Phase C, LANDED).**
> Day 64 made a mint discoverable as a verifiable *identity*; Day 65 gives that listing its
> *card* — title, description, poster, content CID, mime, asset class — without ever letting
> the descriptive layer hijack the identity. Audited PC2's `resolveItemMetadata`
> (`ContentIndexerService.ts:1102–1128`): `name`, `description`, `image` ‖ `media.previewURL`
> (poster fallback), `media.uri`→contentCID, `media.contentType`→mime, `classifyAssetType`
> (line 114), and `content_id = metadata.kid ‖ properties.kid` (line 1106). The cardinal
> change vs PC2: PC2 *trusts* `metadata.kid` to set the catalog `content_id`. We invert the
> trust — `content-market::enrich_listing` re-derives the contentId from the **calldata**
> (authoritative), then REQUIRES `metadata.kid == content_id` before attaching a single
> descriptive field; a mismatch returns `identity_mismatch`, a missing/malformed kid is
> rejected. So a tampered or swapped `metadata.json` can describe an asset but can NEVER
> re-point the listing at a different identity — a real fail-closed hardening over the
> upstream indexer. Still capability-clean: the `metadata.json` bytes are handed in by
> `ipfs-provider` (named in `enrich_requires`); the capsule fetches nothing and remains
> pure. **Proof:** +9 tests (clean fuse, `0x`/uppercase + `properties.kid` acceptance,
> `previewURL` poster fallback, kid-mismatch/missing/malformed rejection, foreign-calldata
> fail-closed, `classifyAssetType` fidelity) and `ddrm-market-smoke.sh` extended to drive
> `publish → chain → reconstruct → enrich` so a matching-kid metadata resolves to a full
> card and a tampered kid is rejected across real binaries. **Gate:** content-market=22,
> ladder INTACT (+wasm), drift PASS, market/publish/producer/consumer smokes green, clippy
> clean. **Next:** a live-Base event-scan path (real `AssetCreated`/`DigitalAssetRegistered`
> logs → reconstruct), or the live producer→consumer round trip.
>
> **🛒 Day 64 — `content-market`: the mint becomes DISCOVERABLE, fail-closed (Phase C, LANDED).**
> Days 61–63 put a sealed asset's identity on-chain as mint calldata; Day 64 makes it
> *findable* — the step that turns a publish into something the consumer chain
> (`has_access_by_content_id`) + the existing `rights → key → decrypt` half can act on.
> Audited PC2's `ContentIndexerService.ts` first: it scans chain logs
> (`AssetCreated`/`DigitalAssetRegistered`, topics at lines 59–63), `eth_call`s `tokenURI`
> (selector `0xc87b56dd`), fetches `metadata.json`, and — crucially — sets the catalog
> row's `content_id` from **`metadata.kid`** (lines 1106, 1117), with `extractCid`
> (line 1140) mapping `tokenURI → CID` and a separate AuthorityGateway `sellersOf`/
> `listings` query (selectors `0x997eab2d`/`0x6bd3a64b`) for price. So PC2 needs FOUR
> sources to build one card. New `content-market` capsule inverts our own Day-62 calldata
> instead: a PURE `reconstruct_listing` decodes `mint(string,uint16,bytes,bytes)` back into
> a typed `ContentListingV1` — `content_id` = the `bytes16` that leads `opRawData`
> (== the KID, no `metadata.json` round-trip needed), tokenURI, metadataCID via the same
> `extractCid` rule, opType, and `(copies,price,payToken)` from `sellRawData`. Because our
> mint is **self-describing**, one verifiable decode yields a complete listing — a genuine
> architectural win over the 4-source indexer. It holds **no** chain RPC, **no** IPFS,
> **no** keys and mints nothing; human-facing enrichment (title/poster/mime from
> `metadata.json`, live event scanning) is NAMED (`ipfs-provider` + `chain-provider`) but
> not performed — the "core injects capabilities" pattern. Fail-closed on a foreign
> selector, truncated/overflowing offsets, a non-`bytes16` opRawData, a FREE-with-sale-terms
> or PAID-without-terms mismatch, an unknown opType, or a bad channel. **Proof:** 13 tests
> (free/paid/resell round-trips, content_id↔KID round-trip, 7 fail-closed cases,
> `extractCid` fidelity) + `ddrm-market-smoke.sh` driving the REAL `publish → chain →
> content-market` so the listing's `content_id` equals the contentId publish bound equals
> `0x{KID}`. **Gate:** content-market=13, ladder INTACT (+wasm), drift PASS, market/publish/
> producer/consumer smokes green, clippy clean. **Next:** metadata.json enrichment via
> `ipfs-provider` (title/poster/mime), a live-Base event-scan path, or the live producer→
> consumer round trip.
>
> **🔗 Day 63 — producer→chain loop closed end to end (Phase C, LANDED).**
> Day 61 assembled the mint *intent*; Day 62 turned it into real calldata; Day 63 joins
> them across real binaries so a sealed asset's identity becomes mint calldata in one hop.
> Re-audited PC2's `encodeOpRawData` inputs first (`elacity-creator/app.js`): the payee
> arrays lead with the creator as the ACCESS_TOKEN holder (`amount = copies`), then a
> ROYALTY_SHARE per partner with `amount = round(10 * royalty)` (app.js:1608), the default
> royalty being `100 − ELACITY_ROYALTY_PERCENT` (=95, app.js:1596), and `metadataUri =
> ipfs://{metaCid}` (app.js:1601); BUY_AND_RESELL appends a DISTRIBUTION_RIGHT for the
> distributor (identifier "C", distinct from the creator) plus a `uint16 resellerCut`
> (default 900). `publish-provider`'s `PublishRequestV1` now carries `creator_address`,
> optional `royalties[]`, and `reseller_cut`, and its `UnsignedMintV1` emits the
> STRUCTURED `op_raw` (`metadata_uri, addresses, role_types, amounts[, reseller_cut]`) +
> `sell` (`copies, price_wei, pay_token`) in the EXACT shape `chain-provider::assemble_mint`
> consumes — no shape translation between the two. New `ddrm-publish-smoke.sh` + native
> orchestrator drive the REAL `publish-provider` then feed its `unsigned_mint` straight
> into the REAL `chain-provider assemble_mint`, asserting the calldata carries the SAME
> `contentId` publish bound + the tokenURI bytes, and that the assembler never signs. PAID
> and FREE both flow end to end. **Capability split holds:** publish touches no RPC/keys,
> chain owns ABI+RPC, wallet owns keys. **Gate:** publish=16 (3 new: assemble-ready sell
> terms, PC2 payee arrays, BUY_AND_RESELL reseller_cut+distribution), ladder INTACT, drift
> PASS, publish/producer/consumer smokes green, clippy clean. **Next:** the `content-market`
> index that scans the mint event into a marketplace listing, or a live-Base producer→
> consumer round trip.
>
> **🧱 Day 62 — `chain-provider assemble_mint`: the mint becomes real EVM calldata (Phase C, LANDED).**
> Day 61's `publish-provider` produced an *intent*; Day 62 turns it into byte-faithful
> Solidity calldata the chain can execute. Audited PC2's exact encoders first
> (`elacity-creator/app.js`): `encodeOpRawData` (app.js:1583 — `bytes16, string, address[],
> uint256[] roleTypes, uint256[] amounts[, uint16 resellerCut]`), `encodeSellRawData`
> (app.js:1633 — `uint256 copies, uint256 price, address payToken`), the FREE case
> `abi.encode(['bytes16'],[contentId])` (app.js:4941), and the outer
> `iface.encodeFunctionData('mint',[uri,opType,opRawData,sellRawData])` (app.js:4950) sent as
> `{to: channel, data, value: mediaCreationFee}`. New **pure** `chain-provider::assemble_mint`
> (no RPC, no keys) reproduces all of it: FREE → `opRawData = bytes16` + empty `sellRawData`;
> PAID → full payee/royalty tuple + sale terms, with the trailing `uint16 resellerCut`
> present iff BUY_AND_RESELL (op_type 2) and rejected for BUY_ONCE (op_type 1) — because the
> uint16 shifts the ABI layout. The selector is configured (same pattern as the `has_access`
> selector; keccak is not computed in-capsule). It returns `{to,data,value}` that feeds the
> EXISTING `prepare_transaction` → wallet-provider sign → `broadcast_transaction`
> (`eth_sendRawTransaction`) seam — so the capability split holds: `chain-provider` owns
> EVM/ABI+RPC, `wallet-provider` owns keys, `publish-provider` touches neither. **Proof:** 10
> tests DECODE the produced calldata back against the Solidity ABI spec (selector, offsets,
> `_uri` string, the `bytes16` contentId in `opRawData`, the `(copies,price,token)` sell
> tuple) — correctness vs the spec, not a self-pinned blob, and no ethers dependency.
> Fail-closed on a non-`bytes16` contentId, a bad selector/channel, free-with-sale-terms,
> paid-without-terms, or a mismatched reseller_cut. **Gate:** ladder INTACT (+ deterministic
> `mint*` rung = 10, filtered around chain-provider's one env-flaky lifecycle test), drift
> PASS, both smokes green, clippy clean. **Next:** wire `publish-provider`→`chain-provider`
> end to end (feed the `UnsignedMintV1` straight into `assemble_mint`, paid payee arrays
> included), or the `content-market` index that scans the mint event into a listing.

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
> `scripts/ddrm-consumer-smoke.sh` + the default-on runtime-core entrypoint
> (`scripts/dev/ddrm-runtime-open`, relocated from `ddrm-consumer-smoke` in Day 83–84, never shipped) builds the **real** capsule
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
