# Principles Conformance & Improvement Register

Last updated: 2026-06-14 UTC

This is an audit surface, not a roadmap. It records where the code does and does not
hold the [PRINCIPLES.md](../PRINCIPLES.md) contract, ranked by how much each gap lets
authority become ambient, transport masquerade as identity, a hidden path bypass a
gate, or the trusted core swell. For open work in priority order see
[../TASKS.md](../TASKS.md); for current factual state see [../state.md](../state.md).

How to read each item: every claim cites `file:line` or a gate result, and is tagged
**confirmed** (read and reproduced), **needs-design** (real, but no safe mechanical
fix), or **needs-verification** (a claim to check, not yet a defect). Items are marked
*drift* (undocumented) or *known* (already tracked in TASKS/state).

## Headline verdict

The dangerous machinery holds. Managed keys stay inside provider capsules, capabilities
fail closed, and signed content is verified before trust — the audit kept finding
*enforcement* where it expected drift. The decrypt boundary binds
principal+session+CID+content-hash+action+expiry into AEAD AAD and checks an ML-DSA-65
signature before key release (`capsules/decrypt-provider/src/main.rs:597-664`); dKMS
rejects substituted channel keys as MITM (`capsules/key-provider/src/main.rs:744-748`);
launch grants reject raw `principal_id`/`home_token` and require a signed,
non-delegatable, expiry-bound token matched to the mounted frame
(`elastos/crates/elastos-server/src/api/gateway_home_token.rs:298-352`). The
control-preservation intent of PRINCIPLES.md is largely real code.

The drift is concentrated in two places: the **trusted core is swelling with app and
service logic** (Principle 5), and a few **fail-closed residuals** remain. Neither is a
key-leak or an auth-bypass; the one true security defect found (a WebAuthn DoS) was
fixed in the 2026-06-14 pass below.

## Conformance scorecard

| Dimension (Principles) | State | Sharpest remaining gap |
|---|---|---|
| Carrier / transport-as-truth (2,4,9) | Clean. Every app/viewer/UI capsule uses relative `/api/...` through providers; no raw HTTP/socket/IPFS in app code | Browser capsule builds a `tcp://`/`tls://` *display string* (`capsules/browser/browser/browser-runtime-api.js:16`) — cosmetic, and a blessed edge proof |
| No ambient authority (3,7,16) | Strongest area. Launch/pairing/wallet-bridge are capability-bound; no bearer tokens leak into public summaries; the agent path is *more* explicit than the human one | Doc-only: device-DID tokens skip session-liveness at `gateway_home_token.rs:339-343` (not client-forgeable; needs a clarifying comment) |
| Canonical path / fail-closed (10,11,12) | Good, with residuals | `namespace.rs:538-548` synthesizes an owner from `SHA256(session.id)` "until proper key management" |
| Small trusted core / distinct nouns (5,13,14) | **Biggest vision gap** | App/service logic inside `elastos-server`; `SystemWebspace*` conflates space and capsule nouns |
| Security | No critical, no key-leak, no auth-bypass | `export_managed_secret` returns a raw key by design — the gateway gate must be airtight (needs-verification) |
| Code quality | Committed `elastos/` code is clippy-clean and `TODO`-free (the gate enforces it) | Stringly-typed `Result<_,String>` dominates; capsule clippy is ungated |

## Improvement areas, prioritized

### A. Trusted-core erosion — Principle 5 (needs-design, drift)

The single largest gap between vision and code. `elastos-server` (the trusted base that
should do only isolation, signatures, principal/session binding, capability validation,
routing, audit) holds full app and service implementations whose capsules already exist:

- `elastos/crates/elastos-server/src/content.rs` — **13,062 lines**, mixing 8+ service
  concerns: fetch, availability receipts, federated abuse-control exchange, quota ledger,
  storage-market admission, external repair fleet, operator dashboard. Belongs in
  `capsules/availability-provider`, `capsules/content-market`, `capsules/ipfs-provider`.
- `elastos/crates/elastos-server/src/room_service.rs` — **5,441 lines** of chat-room
  service (members, invites, key-epochs, attachments, presence) while
  `capsules/chat-room` and `capsules/chat-room-ui` exist.
- `elastos/crates/elastos-server/src/library.rs` (**6,904**) and `documents.rs` — app CRUD
  for surfaces that have `capsules/library` and `capsules/documents`.
- `elastos/crates/elastos-runtime/src/provider/registry.rs:448-476` — `RESERVED_SUB_NAMES`
  hardcodes a closed allowlist of specific app/service names (`wallet`, `drm`, `library`,
  `media`, `browser-engine`…); every new provider edits the trusted core. Replace with
  manifest/capability-declared registration so the taxonomy lives outside the core.

Largest core files by size pressure (mixed-concern candidates to move out, in order):
`content.rs` 13,062 · `carrier.rs` 8,263 · `library.rs` 6,904 · `room_service.rs` 5,441 ·
`home_cmd.rs` 3,287 · `setup.rs` 2,834 · `chat_cmd.rs` 2,347.

Note vs. TASKS: `TASKS.md` tracks some of these as *oversized-file splits* (no-behavior
module moves). That treats the symptom (file size); the architectural fact is sharper —
this is the "core grows past what one can reason about" mechanism Principle 5 exists to
prevent. The fix is a capability-contract move, not a cosmetic split — scoped in
[adr/0001-extract-app-and-service-logic-from-trusted-core.md](adr/0001-extract-app-and-service-logic-from-trusted-core.md).

### B. Fail-closed residuals — Principles 10/11 (mixed)

- **needs-design, known:** `elastos/crates/elastos-server/src/api/handlers/namespace.rs:538-548`
  — `session_owner` derives the owner key from `SHA256(session.id)` when `session.owner`
  is unset ("until proper key management"); a synthesized identity gates a resolve/ownership
  path. Should require a verified principal and reject when absent. Tracked under TASKS.md
  principal-root work (lines 38–40).
- **confirmed, latent, drift:** `elastos/crates/elastos-storage/src/providers/ipfs_streaming.rs`
  — `download_unverified` (renamed 2026-06-14 from the misleading `download_verified`) still
  performs no CID/hash verification. Currently dead code; add block-level CAR verification or
  a final-hash check before any caller trusts it.

### C. Noun and copy debt — Principles 13/14 (needs-design + easy, drift)

- **needs-design:** `elastos/crates/elastos-server/src/api/gateway_home_system.rs:1029-1090`
  types `SystemWebspaceEntry`/`SystemWebspaceSummary` and the UI heading "Elastos webspace"
  actually list **capsules** (apps/providers) — conflating *space* (a resolution namespace)
  with *capsule* (a software role). Synthesized `elastos://capsules/<name>` URIs
  (`gateway_home_system.rs:1049`, `capsules/system/browser/system.js:536`) treat capsules as
  a space path. Rename to an apps/inventory model; reserve "webspace" for resolver monikers
  (Principle 8). This is the home of the remaining Principle-14 copy too:
  `system.js:544` (backend fallback `"capsule"`), `capsules/system/browser/index.html:139`
  (capsule/provider product copy), `capsules/library/browser/src/dialog.js:588,598,668,782`
  (provider/transport language in user dialogs).
- **done 2026-06-14:** the unambiguous display strings (`system.js` empty-state, "Open"
  button, "Unknown App"; library footer) — see the pass record below.

### D. Security posture (good; one item to verify)

No Critical, no High key-leak, no auth-bypass were found across the wallet/key/dKMS/DID/auth
paths read. Verified sound: content-bound key release, MITM-resistant channel pinning,
SIWE/WebAuthn field binding, AES-256-GCM managed-key storage at `0o600`.

- **fixed 2026-06-14:** WebAuthn registration panicked on attacker-controlled `authData`
  (DoS via unchecked slice) — `elastos/crates/elastos-identity/src/webauthn.rs`. Now
  bounds-checked with a regression test.
- **needs-verification:** `capsules/wallet-provider/src/account.rs:458-466`
  (`export_managed_secret`) returns a raw `private_key_hex` — the single intentional path
  where a managed secret leaves a provider capsule. The capsule only checks principal/account
  ownership, so the security claim rests entirely on the **gateway** capability gate for this
  op being explicit, narrow, and audited. Confirm that gate; consider re-encrypting to a
  recovery recipient instead of returning plaintext hex.
- **hygiene only (not exploitable):** non-constant-time challenge compare at
  `webauthn.rs:378,540` (a server-issued nonce, not a guessed secret).

### E. Code-quality and gate debt (needs-design)

- **Stringly-typed errors dominate.** `elastos-common` exports `ElastosError`/`Result`
  (`elastos/crates/elastos-common/src/error.rs`) yet `Result<_, String>` is the default in
  product code (`capsules/webspace-provider` 43 sites, `content.rs` 29, `ddrm-plan-runner`
  26). Migrate hot trust paths first.
- **Capsule clippy is ungated.** `just verify` runs `cargo clippy --workspace -D warnings`
  only inside `elastos/`; capsules are separate crates, so latent lints accumulate invisibly
  (e.g. `capsules/chat/src/session.rs:267` `send_gossip` 8 args;
  `capsules/chat/src/main_stdio.rs:581` a single-arm `match`). Recommend extending the gate
  to clippy-check capsules, then clearing the backlog.
- **`alignment-check` produced wrong results without ripgrep — fixed 2026-06-14 by requiring
  `rg`.** `scripts/check-wci-alignment.sh` relies on `rg --glob '!...'` exclusions that are
  load-bearing: the forbidden-pattern checks exempt provider/connector/test capsules
  (`!capsules/*-provider/**`, `!capsules/wallet-metamask/*`, `!**/tests/**`, `!target/**`, …).
  The former plain-`grep` fallback translated only 5 of those globs and silently dropped the
  rest, so on a host without `rg` it (a) scanned compiled binaries under `capsules/*/target/`
  (a "fallback display modes" hit inside `browser-engine-adapter` `.rmeta`) and (b) matched the
  very capsules the checks are meant to exempt (`wallet-provider`, `wallet-metamask`),
  reporting false failures. A grep fallback cannot reproduce ripgrep's gitignore-aware
  multi-glob semantics, so the fallback was removed: the script now fails loudly with
  `exit 2` and an install hint when `rg` is absent, rather than lying. **Operators must have
  ripgrep installed** for `just verify` / `just alignment-check`.
- **Provider exemptions are now role-based (fixed 2026-06-14).** The "app capsule must not touch
  wallet/chain authority" checks exempted providers by the `-provider` name suffix, so role-
  providers named differently (`content-market`, `browser-engine-adapter`,
  `operator-drive-adapter`) were false-flagged. The script now derives exemptions from each
  capsule's declared `"role": "provider"` in `capsule.json`, so the exemption tracks role, not
  naming. **Still open:** the forbidden-pattern checks match string literals inside *comments*,
  so `capsules/creator/creator.js:18,472` is flagged for comment lines that actually *document*
  the correct provider-mediated, runtime-never-signs flow (`// …eth_sendTransaction…`,
  `// chain-provider broadcast_transaction`). These are false positives in an app capsule;
  fixing them means teaching the checks to skip comment-only matches (a deliberate change to
  gate strictness), not editing the comments to dodge the linter.
- **Comment-only matches are now skipped (fixed 2026-06-14); 2 residual false positives remain
  as a detection-policy call.** The `rg`-based authority checks and the Python host-topology
  check now ignore comment-only lines (`//`, `/* … */`, `* …`, `<!-- … -->`; `#` and bare `*`
  are not treated as comments, since they are Rust attributes / derefs). That cleared the
  `creator.js` hits. Two false positives remain, both from the host-topology check's bare
  provider/brand-**name** substring patterns matching legitimate code that merely *names* a
  capsule: `capsules/elacity-player/player.js:30,89` (its own `VIEWER_ID`/log prefix vs the
  `"elacity"` pattern) and `capsules/marketplace/marketplace.js:298` (a
  `["object-provider","content-provider"].includes(name)` classification list vs the
  `"object-provider"` pattern). The precise topology-leak signals are the provider *routes*
  (`/api/provider/<x>`) and concrete RPC/SDK tokens (`window.ethereum`, `rpc_url`, `eth_call`,
  `bitcoind`, `http://127.0.0.1`); the bare *name* substrings are noisy. **Resolved 2026-06-14:**
  the bare provider/adapter capsule-name substrings (15 of them) and bare `elacity` were removed
  from the host-topology `forbidden_source_patterns`; the check now relies on the `elastos://`
  namespaces, `/api/provider/*` routes, and concrete RPC/SDK/loopback tokens, which are the real
  raw-access vectors. With this, `just alignment-check` passes clean (exit 0). The policy
  tradeoff is recorded in the script: an app capsule that merely *names* a provider is no longer
  flagged, but any capsule that actually *reaches* a backend (via namespace, route, or token)
  still is.
- **`content.rs` URL-redaction duplication** (the `scheme`/`host`/`port`/`path_configured`
  object entries repeated across 11 `json!` literals) was intentionally **not** dedup'd this
  pass: the entries are interleaved with different sibling keys, so a behavior-preserving
  extraction is not mechanical. Fold it into the `content.rs` split (item A), not a standalone
  edit.

## Pass record — 2026-06-14 (orchestrator /orchestrate)

Shipped (each gated; nothing committed; in-progress branch untouched):

- WebAuthn DoS bounds-check + regression test — `webauthn.rs` (16/16 tests pass).
- Presence signing fails loud + observable poll errors — `capsules/chat/src/session.rs`,
  `main.rs`, `main_stdio.rs` (22/22 chat tests pass).
- Persistence write errors logged instead of swallowed — `capsules/chat/src/{main,main_stdio}.rs`.
- `download_verified` → `download_unverified` with a truthful doc — `ipfs_streaming.rs`.
- Principle-14 display copy — `capsules/system/browser/system.js` (×3),
  `capsules/library/browser/index.html`.
- Removed 7 redundant `let _ = …await?` discards — `capsules/chat-room-ui/src/lib.rs`.

### Investigated and found NOT a defect — do not re-churn

`capsules/wallet-provider/src/account.rs:127` — `find_map(decrypt.ok()).unwrap_or_else(random)`
*looks* like it silently mints a new wallet when a managed key fails to decrypt. It does not
harm the user: an undecryptable account is **preserved and visibly marked**
`managed_key_unavailable` (asserted at `src/tests/accounts.rs:136`), **signing fails closed**
with a "recover or recreate" error (`src/tests/accounts.rs:88-90`), and a *labeled replacement*
is created so the user can keep working. Forcing a hard error here breaks the deliberate,
tested replacement flow (`create_managed_account_replaces_unavailable_idempotent_account`). The
old address stays visible and is recoverable via the Recovery Kit. Recorded here so a future
audit pass does not "fix" intended behavior.

`elastos/crates/elastos-server/src/api/viewer_open.rs:421` — the forensic watermark anchor
(`grant_watermark_digest16`) is computed from the client-supplied `delegation_sig_hex` *before* the
gateway itself verifies it, which *looks* like it trusts an unverified signature (audit finding
"H1"). Traced to ground: it is **safe-by-construction, not exploitable.** The digest can only reach
(a) an **egressed decrypted frame** or (b) attribution of a **real leaked copy** after the dKMS
node's own `verify_access_grant` (EIP-191 owner recovery, `capsules/ddrm-envelope/src/access.rs`
`recover_eip191`/line ~531) **and** a live on-chain `hasAccessByContentId` check both pass — a
forged signature fails closed there (`DelSigInvalid`), so no CEK is recovered, no decrypt happens,
and the watermark embed (which runs only AFTER a successful 2-of-3 quorum recover) never executes.
The only residue of a forged signature is an intent-time custody "opened" row that then corresponds
to a **failed** open with no media served — a detectable anomaly, not attribution of a leaked frame
to an innocent wallet. The load-bearing invariant (a signature that does not recover to the owner is
rejected) is pinned by the `access.rs` test `delegation_sig_from_wrong_wallet_fails_closed`. Recorded
here so a future pass does not "fix" a non-exploitable hygiene item as if it were a security gap. A
defense-in-depth tightening (defer the custody write until after authorization, or a server-key MAC
over `owner ‖ content_id ‖ open_time`) is tracked in THREAT_MODEL §4 — an upgrade, not a blocker.

ECDSA signature malleability (high-`s`) at the `recover_from_prehash` / `verify_prehash` sites
(`capsules/ddrm-envelope/src/access.rs` `recover_eip191`, `elastos/crates/elastos-auth/src/lib.rs`
`recover_evm_address`, `capsules/wallet-provider/src/approval.rs`, `…/crypto/evm/proof.rs` — audit
finding "M1") *looks* like it could let a second valid signature form bypass a replay/dedup gate. It
cannot: a full sweep confirms **no replay/dedup key in the tree is keyed on signature bytes.** The
grant replay guard keys on explicit server-generated nonces (`"{delegation_nonce}:{request_nonce}"`,
`access.rs` `ReplayGuard::check_and_record`), recovered identities are derived from the *public key*
(malleability-invariant), and the `signature_hash` fields in `approval.rs` / `proof.rs` are
audit/receipt values or equality checks, never dedup keys. So malleability has no reachable impact —
**safe-by-construction.** Low-`s` normalization is reasonable future hygiene, but it is **not** a
no-op to add blindly: normalizing `s` without also flipping the `recovery_id` parity changes the
recovered address and would reject every high-`s` (but valid) grant. It is therefore deliberately
NOT applied to the crown-jewel recover path until it is needed and done with parity handling + a
recovery round-trip test. Recorded so a future pass neither "fixes" M1 as a security gap nor adds a
naive normalization that breaks owner recovery.

The audit/custody trail (PRE_AUDIT #3 / "GAP-8") was flagged as *best-effort, not tamper-evident:
silent drops, no hash-chain, no signature, no fsync, and "the dDRM open path emits no audit event at
all."* Traced to ground against the current tree: **already resolved.** `AuditLog::emit` returns
`Result` and writes a `ChainedRecord` — monotonic `seq` + `prev_hash` + `record_hash` over
`domain ‖ seq ‖ prev_hash ‖ event_json`, ed25519-signed with a crypto-agility tag (ML-DSA-ready) —
which it `flush`es and `sync_all`s to disk BEFORE advancing the chain head, so a failed write retries
the same `seq` (no gap, no silent loss); `verify_chain` walks the whole chain for tamper-evidence
(`primitives/audit.rs:461-535`). The dDRM open emits a **fail-closed** `content_open`
(`viewer_open.rs:481`): the open is REFUSED with `503` if the custody record can't be durably
committed, and the emit sits on the common path BEFORE dispatch to the object/media/quorum handlers,
so every authorized open is recorded. All session-bound segment/byte serving (`viewer_media_*`,
`viewer_object_*`) is covered **transitively** — sessions are minted only by that handler, so the one
custody record covers the whole open session. The original "no open event, only a comment at
`viewer_open.rs:1021`" was a **misread**: `:1021` is media-layout *detection*, not an open path.
Two paths a coverage sweep flags are **not** custody gaps: `GET /api/provider/object/download/raw`
serves the principal's OWN library files (`library_download_object` → `read_library_file_bytes`, no
decrypt/CEK/quorum), correctly audited via `append_provider_effect_audit` (a DDRM `content_open`
would be the wrong abstraction, and there are no chain "rights" to revoke on one's own files); and the
demo routes (`open_demo_media` / `open_owned_object`) serve an operator-fixed sample
(`ELASTOS_DDRM_SAMPLE_VIDEO`), not attacker-selectable content. The one genuinely open item — external
anchoring of the chain head to defend a *live-compromised* runtime that still holds the signing key —
is documented as deliberate roadmap in the `audit.rs` threat-model header, not a present defect.
Recorded so a future pass does not re-implement an already-tamper-evident audit log. (Surface-area
note, separate from custody: the demo open routes are mounted unconditionally; gating them behind an
explicit enable, or only when a sample is configured, is optional defense-in-depth, not a fix.)

## How this register was produced

Six read-only audit agents, one per principle cluster (Carrier/transport, ambient authority,
canonical-path/fail-closed, trusted-core/nouns, security, code-quality), each required to read
its hits rather than grep-and-guess and to mark documented-known vs undocumented drift. Re-run
the same fan-out via `/orchestrate` to refresh this register; update the pass record and the
"investigated/not-a-defect" list as items are resolved or re-confirmed.
