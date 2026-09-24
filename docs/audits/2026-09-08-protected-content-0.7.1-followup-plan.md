# Protected-Content 0.7.1 Follow-up (issues #42, #48, #49) Implementation Plan

> Working copy for executors: `.superpowers/plans/2026-09-08-protected-content-0.7.1-followup.md` (git-ignored). This tracked copy is the reviewable record; keep both in sync when the plan changes.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One PR on top of `feat/protected-content-atomic-cutover` (stack #51 → #60 → #59 → this PR) that closes GitHub issues #42 (UI/UX: open-through-decryption, Creator app, any file type), #49 (five secondary cleanups from the #36 review) and #48 (crypto review package), with no regression on the installed mint → buy → play path.

**Architecture:** Three independent tracks land in risk order. (1) Wiring fixes with pinned tests: shell allowlist, phantom viewer ids, Library JS tests into CI, ledger file discipline, `ProviderBridge` state enum, unbound-vs-pending, approval idempotency, `buy` action granularity. (2) Product surface: a `creator` app capsule that uploads through the existing Library transport and publishes with `protection.mode = runtime_custody`; a non-media protection path that reuses the already-tested, zero-caller EPC1 chunked AES-256-GCM container in `elastos-protected-content-custody/src/payload.rs` through new chunked provider ops, a mint-journal content-identity enum with a versioned codec, and a new `elacity-reader` viewer capsule that receives decrypted bytes and renders image / PDF / text+code / 3D / EPUB / CBZ client-side; audio follows the media path through a media-provider audio transcode branch and a two-pair mime/codec allowlist. (3) #48 as documentation plus a JSON golden-vector generator and replay test. Chain, rights, `elastos-protected-content-contracts`, and `custody-provider` are not touched.

**Tech Stack:** Rust workspace under `elastos/` (Rust 1.91, clippy `-D warnings`), capsule Rust workspaces under `capsules/*` (`just test-capsules`), browser capsules in plain ES modules (no bundler; `node --test` for unit tests), ffmpeg/ffprobe via `capsules/media-provider`, `tracing` for all new diagnostics.

**Spec:** GitHub issues Elacity/elastos-runtime #42, #48, #49; PR #51 checklist body; decisions memo `.superpowers/memory/protected-content-0.7.1-followup-decisions.md`; base plan `.superpowers/plans/2026-09-02-elacity-2298-installed-e2e-proof.md` (format precedent) and the atomic-cutover plan referenced from `.superpowers/memory/README.md`.

## Resolved decisions (user, 2026-09-08)

- **D1 non-media sealing:** reuse `payload.rs` (EPC1, `aes-256-gcm-chunked/v1`, 1 MiB plaintext chunks). Zero new cryptographic primitives; the only crypto-module change is exposing its existing chunk loop as a streaming sealer/decrypter so sandboxed providers can process one chunk per frame. Byte-equivalence with the one-shot functions is pinned by test.
- **D2 viewer egress:** decrypted bytes to the viewer capsule, client renders. Server render-IR (dkms pixel-lock posture) is a recorded follow-up, not in scope.
- **D3 Creator:** port the dkms `capsules/creator` UX (upload → protect → list steps) with its own `<input type=file>`, reusing the Library upload transport verbatim; grant `CREATOR_CAPSULE_ID` on the four upload handlers plus `roots`, `stat`, `publish`.
- **D4 file types:** any non-video, non-audio mime may be protected; the reader renders image, PDF, text/code, 3D (glTF/GLB/OBJ/STL), EPUB, CBZ and shows an honest "no renderer" state otherwise.
- **D5 audio:** `audio/*` follows the MEDIA path; media-provider transcodes to an fMP4 AAC DASH rendition (`audio/mp4` / `mp4a.40.2`) alongside the pinned `video/mp4` / `avc1.640028`.
- **D6 #48:** docs (scope-and-corrections, threat model, build card) plus a JSON golden-vector generator and replay test. No proptest/fuzz.
- **D7 #49 item 4:** the "not wired" sentence is already gone; the fix is the Marketplace manifest buy affordance plus two ESP doc scoping sentences.
- **Order:** #42 gap 3 → #49 (items 2, 5, 3, 4, 1) → Creator → non-media path + reader → audio → #48 → docs. One PR.
- **Naming (this plan, flag if you disagree):** new viewer capsule id `elacity-reader`; Creator capsule id `creator`; contracts module `object.rs`; journal magic bump `epc-mj05` → `epc-mj06`.

## Global Constraints

- Branch off `feat/protected-content-atomic-cutover` at `25ab205e`; never rebase the three PRs below it; no `git push` without explicit per-instance user approval; no AI attribution lines in commits or PR body (user's standing rule).
- Every task ends green on: `git diff --check`, `node scripts/home-entropy-check.mjs`, `(cd elastos && cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings)`, and the narrow tests named in the task. Run `just verify` at Tasks 6, 15 and 18.
- `tracing` for all new diagnostics: workflow-retracing detail at `debug`/`trace`, operator-facing state changes at `info`, failures at `warn`/`error`. No `eprintln!`.
- Provider frames stay under `MAX_PROVIDER_FRAME_BYTES_V1` (16 MiB, `wire.rs:13`): part/chunk blobs are at most 2 MiB canonical bytes (`MAX_PROTECT_MEDIA_PART_BYTES_V1`, `MAX_VIEWER_MEDIA_PART_BYTES_V1`), which JSON int arrays inflate 3-4×. Object chunks are `PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1` (1 MiB) plaintext.
- `ContentAccessIdV1` stays random (`generate_runtime_content_access_id`, `protected_content_runtime.rs:1009-1018`): chain, rights, `elastos-protected-content-contracts`, `custody-provider`, `chain-provider` need zero changes for non-media objects. Do not touch them except the one `chain-provider` grep confirmation in Task 4.
- The media-provider config schema stays exactly `MEDIA_FIELDS` (`scripts/protected-content-installed-static-audit.py:70-91` asserts `set(config) == MEDIA_FIELDS`); no new config field, `output_profile` stays `browser_fmp4_h264_v1`.
- Capsule public copy must not match `/\b(runtime mirror|projection|schema|derived facts?|capability surface|provider boundary|hostcall|launch token)\b/i` (`scripts/check-capsule-templates.mjs:13`).
- Vendored browser libraries follow the xterm precedent: `browser/vendor/<lib>/README.md` with package, version, license, tarball, npm integrity, SHA-256 table, and a runnable verification command; imports carry `?v=<lib>-<version>`; an assert block is added to `scripts/home-entropy-check.mjs`.
- Every capsule stays opaque-origin (no `allow-same-origin`); no capsule may render untrusted HTML in a nested frame (`frame-src 'none'` stays in every CSP). EPUB chapters are rendered by sanitizing XHTML into DOM nodes, never by `srcdoc`/`iframe`.
- Fixtures `ContentAvailabilityTestProvider`, `ProcessChainEvidenceProvider`, `MockContentProvider`, `LoopbackCustodyCarrierInvoker` are test-only; never reuse in product code.
- Evidence per `AGENTS.md:200-240` for every installed-artifact change: edited source path, built + installed artifact SHA-256, restart performed, live proof command and result; branch heads go to `state.md`, never standing docs.

## File Structure

**Wiring fixes (Tasks 1–6)**
- `capsules/home/browser/home-shell-host.js:80` + `capsules/home/browser/index.html` — Library may open `elacity-player` / `elacity-reader`; `creator` may open `library`.
- `scripts/home-entropy-check.mjs:2297-2301` — updated pins.
- `elastos/crates/elastos-server/src/library.rs:6486-6535` — drop phantom viewer ids.
- `justfile` (`verify` recipe) + `.github/workflows/ci.yml:54` — `node --test capsules/library/browser/src/`.
- `elastos/crates/elastos-server/src/protected_content_runtime.rs:6725-6735, 7796-7815` — ledger open/write discipline; `elastos/crates/elastos-protected-content-runtime/src/mint_journal.rs:3237-3261` — export `ExclusiveFileLock`.
- `elastos/crates/elastos-runtime/src/provider/bridge.rs:182-522` — `ChildState` enum.
- `elastos/crates/elastos-server/src/api/gateway_provider_proxy.rs:2540-2565, 3487-3500, 1646` — unbound signal, approval idempotency, buy allowlist; `protected_content_runtime.rs:142, 5170-5178` — new message, `confirmed_approval`.
- `elastos/crates/elastos-server/src/provider_resource.rs:255-270` — `Action::Buy`; `capsules/object-provider/capsule.json:13` — `"buy"` action; `capsules/marketplace/capsule.json` — buy affordance; `docs/ESP_V0.md:178-181,196`, `docs/SHELL_ESP_BOUNDARY_MAP.md:83`.

**Creator (Task 7)**
- Create `capsules/creator/{capsule.json, browser/index.html, browser/creator.js, browser/style.css, browser/icons/icon.svg, browser/icons/icon-{32,64,128,256}.png, browser/creator.test.mjs}`.
- Modify the 19-site registration checklist (listed in Task 7).

**Non-media path (Tasks 8–14)**
- Create `elastos/crates/elastos-protected-content-provider-contracts/src/object.rs` — `ChunkedPayloadObjectIdentityV1`, protect/decrypt object request+response shapes; modify `protect.rs`, `decrypt.rs`, `lib.rs` (new op variants and re-exports).
- Modify `elastos/crates/elastos-protected-content-custody/src/payload.rs` + `lib.rs` — `PayloadSealerV1`, `PayloadChunkDecrypterV1`, `framed_chunk_ranges_v1`.
- Modify `capsules/protected-content-protect-provider/src/lib.rs` (+ `tests/process.rs`) — object protection session.
- Modify `capsules/protected-content-decrypt-provider/src/lib.rs` (+ `tests/`) — object viewer session and chunk read.
- Modify `elastos/crates/elastos-protected-content-runtime/src/mint_journal.rs` — `RuntimeContentIdentityV1` enum, `epc-mj06` codec reading `epc-mj05`.
- Modify `elastos/crates/elastos-server/src/protected_content_runtime.rs` — object publish branch, object twins of the six media-only helpers, viewer response, session binding v3, second admitted viewer; `gateway_provider_proxy.rs:1644, 1707-1717` — viewer admission by verified `launch_context.executable_actor`.
- Modify `capsules/library/browser/src/{model.js, actions.js, app.js}` + tests — predicate widening and reader routing.

**Reader (Task 15)**
- Create `capsules/elacity-reader/{capsule.json, browser/index.html, browser/reader.js, browser/style.css, browser/zip.js, browser/epub.js, browser/cbz.js, browser/render.js, browser/*.test.mjs, browser/vendor/pdfjs/…, browser/vendor/three/…, browser/icons/…}`.

**Audio (Task 16)**
- Modify `capsules/media-provider/src/lib.rs:425-532, 1007-1038` + `tests/process.rs`; `protected_content_runtime.rs:293-294, 3377-3382, 3777-3786`; `capsules/library/browser/src/{model.js,actions.js}`; `capsules/elacity-player/browser/{player.js,index.html,style.css}`; `scripts/elacity-player-smoke.mjs`.

**#48 (Task 17)**
- Create `docs/PROTECTED_CONTENT_CRYPTO_REVIEW.md`, `elastos/crates/elastos-protected-content-custody/examples/golden_vectors.rs`, `docs/audits/2026-09-protected-content-golden-vectors-v1.json`, `elastos/crates/elastos-protected-content-custody/tests/golden_vectors.rs`.

**Docs (Task 18)**
- `docs/PROTECTED_CONTENT.md`, `docs/PROTECTED_CONTENT_CONTRACTS_V1.md`, `state.md`, `TASKS.md`, `scripts/README.md`.

---

### Task 0: Stacked branch

**Files:** none (git only)

- [ ] **Step 1:** `git -C /Users/maciz/www/ela.city/elastos-runtime status --short` is empty and `git rev-parse HEAD` is `25ab205e…` on `feat/protected-content-atomic-cutover`.
- [ ] **Step 2:** `git switch -c feat/protected-content-0.7.1-followup`
- [ ] **Step 3:** Baseline the narrow suites this plan touches so later failures are attributable:

```bash
node --test capsules/library/browser/src/          # expected: all pass today (they run nowhere in CI)
just test-crate elastos-server -- --test-threads=4 protected_content_runtime::tests 2>&1 | tail -3
just test-crate elastos-protected-content-custody 2>&1 | tail -3
```

---

### Task 1: #42 gap 3 — Library opens protected video through the shell, phantom viewers removed, Library JS tests in CI

Library already routes a runtime-custody video double-click to `openTarget("elacity-player", { mint_id })` (`capsules/library/browser/src/actions.js:76-80, 98-111`), but the shell host refuses the message because `library`'s open-target set (`home-shell-host.js:80`) lacks `elacity-player`. `viewer_ids_for_name` (`library.rs:6486`) also emits `video-viewer` / `image-viewer`, ids of capsules that do not exist, so `viewer_options_for_name` always filters them away; they mislead readers and tests.

**Files:**
- Modify: `capsules/home/browser/home-shell-host.js:80`
- Modify: `capsules/home/browser/index.html` (bump the `home-shell-host.js?v=` value) and `scripts/home-entropy-check.mjs` (`homeShellHostAssetVersion` constant and the pin at `:2297-2301`)
- Modify: `elastos/crates/elastos-server/src/library.rs:6486-6535`
- Modify: `justfile` (`verify` recipe, after `node --test scripts/home-two-runtime-acceptance.test.mjs`), `.github/workflows/ci.yml` (after line 54)
- Test: `scripts/home-entropy-check.mjs`, `elastos/crates/elastos-server/src/library.rs` test module, `capsules/library/browser/src/actions.test.mjs`

**Interfaces:**
- Produces: shell allowlist entry `library: new Set(["archive-manager", "documents", "elacity-player", "gba-emulator", "library"])` (Task 15 appends `"elacity-reader"`; Task 7 adds a `creator` key).

- [ ] **Step 1: Pin the new allowlist in the entropy check first (failing).** Edit `scripts/home-entropy-check.mjs:2297-2301`:

```js
assert(
  shellJs.includes('"gba-emulator": new Set(["library"])') &&
    shellJs.includes(
      'library: new Set(["archive-manager", "documents", "elacity-player", "gba-emulator", "library"])',
    ),
  "Home must allow GBA to open Library, Library to return compatible ROMs, and Library to open protected media in Elacity Player, all source-gated",
);
```

- [ ] **Step 2:** Run `node scripts/home-entropy-check.mjs`. Expected: FAIL on the message above.
- [ ] **Step 3:** Edit `capsules/home/browser/home-shell-host.js:80` to the new set (alphabetical). Bump the `?v=` token on `home-shell-host.js` in `capsules/home/browser/index.html` and the matching `homeShellHostAssetVersion` constant in the entropy script (grep the current value; use `20260908a`).
- [ ] **Step 4:** Run `node scripts/home-entropy-check.mjs`. Expected: PASS.
- [ ] **Step 5: Failing Rust test for phantom ids.** In the `library.rs` test module add:

```rust
#[test]
fn viewer_ids_for_name_only_names_installed_capsule_ids() {
    for name in ["clip.mp4", "photo.png", "photo.jpg", "art.gif", "song.mp3"] {
        assert!(viewer_ids_for_name(name).is_empty(), "{name} must not name a phantom viewer");
    }
    assert_eq!(viewer_ids_for_name("notes.md"), vec!["documents"]);
    assert_eq!(viewer_ids_for_name("paper.pdf"), vec!["documents"]);
    assert_eq!(viewer_ids_for_name("game.gba"), vec!["gba-emulator"]);
    assert_eq!(viewer_ids_for_name("bundle.zip"), vec!["archive-manager"]);
}
```

- [ ] **Step 6:** `just test-crate elastos-server -- library::tests::viewer_ids_for_name_only_names_installed_capsule_ids`. Expected: FAIL (`.mp4` → `["video-viewer"]`).
- [ ] **Step 7:** Remove the `.png/.jpg/.jpeg/.gif` and `.mp4` arms from `viewer_ids_for_name` and the `"image-viewer"` / `"video-viewer"` arms from `viewer_label`. Do not add an `elacity-player` arm: protected media is routed by `mint_id`, not extension.
- [ ] **Step 8:** Re-run Step 6. Expected: PASS. Then `just test-crate elastos-server -- library::` to confirm no other test pinned the phantom ids (fix any that did by asserting the empty vector).
- [ ] **Step 9: Wire the Library JS tests.** Append to the `verify` recipe in `justfile` right after the `home-two-runtime-acceptance` line:

```make
    node --test capsules/library/browser/src/
```

and to `.github/workflows/ci.yml` after line 54:

```yaml
      - name: library browser unit tests
        run: node --test capsules/library/browser/src/
```

- [ ] **Step 10:** `node --test capsules/library/browser/src/` passes locally. `just verify` reaches the new line.
- [ ] **Step 11: Commit.**

```bash
git add capsules/home/browser scripts/home-entropy-check.mjs elastos/crates/elastos-server/src/library.rs justfile .github/workflows/ci.yml
git commit -m "fix(home,library): let Library open Elacity Player, drop phantom viewer ids, run Library JS tests in CI"
```

---

### Task 2: #49 item 2 — purchases ledger file discipline (`O_NOFOLLOW`, `nlink == 1`, exclusive lock)

The mint journal opens with `O_NOFOLLOW|O_CLOEXEC` and serializes through `ExclusiveFileLock` (`mint_journal.rs:3237-3261, 3299, 3313`). The purchases ledger writes through `write_owner_only_bytes` (`protected_content_runtime.rs:7796`) without custom flags and reads with a bare `fs::read` (`:6730`), and two concurrent `persist_runtime_custody_purchase` calls race on the rename.

**Files:**
- Modify: `elastos/crates/elastos-protected-content-runtime/src/mint_journal.rs:3237` (`pub struct ExclusiveFileLock` + re-export in `lib.rs`)
- Modify: `elastos/crates/elastos-server/src/protected_content_runtime.rs:6725-6735, 7796-7815`
- Test: `elastos/crates/elastos-server/src/protected_content_runtime/tests.rs` (next to `:5416`)

**Interfaces:**
- Produces: `pub struct ExclusiveFileLock` with `pub fn acquire(path: &Path) -> std::io::Result<Self>` (whatever the existing constructor is named; keep its name, only widen visibility); `fn runtime_purchase_lock_path(data_dir: &Path, principal_id: &str) -> PathBuf` returning `<purchases dir>/.lock`.

- [ ] **Step 1: Failing tests.**

```rust
#[cfg(unix)]
#[test]
fn load_runtime_custody_purchase_refuses_symlinked_record() {
    let dir = tempfile::tempdir().unwrap();
    let (principal, mint_id, record) = sample_purchase_record(); // reuse the helper used by the test at tests.rs:5416
    persist_runtime_custody_purchase(dir.path(), &record).unwrap();
    let path = runtime_purchase_path(dir.path(), &principal, mint_id);
    let real = path.with_extension("real");
    std::fs::rename(&path, &real).unwrap();
    std::os::unix::fs::symlink(&real, &path).unwrap();
    assert!(load_runtime_custody_purchase(dir.path(), &principal, mint_id).is_err());
}

#[test]
fn persist_runtime_custody_purchase_serializes_concurrent_writers() {
    let dir = tempfile::tempdir().unwrap();
    let (principal, mint_id, record) = sample_purchase_record();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let dir = dir.path().to_path_buf();
            let record = record.clone();
            std::thread::spawn(move || persist_runtime_custody_purchase(&dir, &record).unwrap())
        })
        .collect();
    for handle in handles { handle.join().unwrap(); }
    assert!(load_runtime_custody_purchase(dir.path(), &principal, mint_id).unwrap().is_some());
    assert!(!runtime_purchase_lock_path(dir.path(), &principal).with_extension("tmp").exists());
}
```

- [ ] **Step 2:** `just test-crate elastos-server -- protected_content_runtime::tests::load_runtime_custody_purchase_refuses_symlinked_record protected_content_runtime::tests::persist_runtime_custody_purchase_serializes_concurrent_writers`. Expected: first FAILS (symlink followed), second may fail on `runtime_purchase_lock_path` not existing.
- [ ] **Step 3:** In `mint_journal.rs` make `ExclusiveFileLock` and its constructor `pub`, re-export from the crate `lib.rs`. In `protected_content_runtime.rs`:
  - add `fn runtime_purchase_lock_path(data_dir, principal_id) -> PathBuf` (sibling of `runtime_purchase_path`);
  - in `persist_runtime_custody_purchase`, acquire `ExclusiveFileLock` on that path before `write_owner_only_bytes`;
  - in `write_owner_only_bytes`, add `.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)` to the temp-file `OpenOptions`;
  - replace the `fs::read(path)` in `load_runtime_custody_purchase` with an `open_owner_only_runtime_record_file(path)` helper copied from the `open_runtime_media_source_file` idiom (`:3345-3358`: `O_NOFOLLOW`, `is_file()`, `nlink() == 1`) and read from the handle. Add `tracing::warn!(path = %path.display(), "purchase ledger record rejected")` on rejection.
- [ ] **Step 4:** Re-run Step 2. Expected: PASS. Run `just test-crate elastos-server -- protected_content_runtime::tests::` for the whole module.
- [ ] **Step 5: Commit.** `git commit -am "fix(protected-content): purchases ledger gets O_NOFOLLOW, nlink and exclusive-lock parity with the mint journal"`

---

### Task 3: #49 item 5 — `ProviderBridge` lifecycle state enum

`ProviderBridge` (`bridge.rs:182-192`) tracks `child: Mutex<Option<Child>>` plus `shutdown_completed: AtomicBool`; the two can disagree. Replace with one state.

**Files:**
- Modify: `elastos/crates/elastos-runtime/src/provider/bridge.rs:182-192, 207-222, 232-317, 320-334, 444-522`
- Test: same file, test module (only `server_infra.rs:2017` uses `from_io`; the ~35 external callers of `spawn`/`request`/`shutdown` do not change)

**Interfaces:**
- Produces (private): `enum ChildState { Attached, Spawned(Child), ShutDown }`; `child: Mutex<ChildState>`; `shutdown()` transitions any state to `ShutDown` exactly once.

- [ ] **Step 1: Failing test.**

```rust
#[tokio::test]
async fn attached_bridge_shutdown_delivers_protocol_shutdown_once_and_is_idempotent() {
    let (client_reader, server_writer) = tokio::io::duplex(4096);
    let (server_reader, client_writer) = tokio::io::duplex(4096);
    let bridge = ProviderBridge::from_io(tokio::io::BufReader::new(client_reader), client_writer);
    let server = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(server_reader).lines();
        let first = lines.next_line().await.unwrap().unwrap();
        let mut writer = server_writer;
        writer.write_all(b"{\"id\":1,\"ok\":true}\n").await.unwrap();
        first
    });
    bridge.shutdown().await.unwrap();
    bridge.shutdown().await.unwrap(); // idempotent
    assert!(server.await.unwrap().contains("\"shutdown\""));
    assert!(matches!(*bridge.child.lock().await, ChildState::ShutDown));
}
```

(Adjust the JSON reply to whatever `shutdown()` currently expects from the provider; read the existing shutdown request builder at `bridge.rs:444-522` first.)

- [ ] **Step 2:** `just test-crate elastos-runtime -- provider::bridge::tests::attached_bridge_shutdown_delivers_protocol_shutdown_once_and_is_idempotent`. Expected: FAIL to compile (`ChildState` missing).
- [ ] **Step 3:** Introduce `ChildState`, replace the field pair, make `spawn` store `Spawned(child)`, `from_io` store `Attached`, `terminate_child_for_init_failure` take from `Spawned`, and `shutdown()` do: `ShutDown => return Ok(())`; `Attached => deliver protocol shutdown, set ShutDown`; `Spawned(child) => deliver protocol shutdown, wait/force reap with `shutdown_timeout`, set ShutDown`. Remove `shutdown_completed`. Log the transition at `debug`.
- [ ] **Step 4:** Re-run Step 2 and `just test-crate elastos-runtime`. Expected: PASS. `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] **Step 5: Commit.** `git commit -am "refactor(runtime): ProviderBridge tracks Attached/Spawned/ShutDown instead of Option<Child> plus flag"`

---

### Task 4: #49 item 3 — unbound-vs-pending at the Runtime layer

`chain-provider` answers `unknown_protected_content_object` when the content access id is not bound (`capsules/chain-provider/src/main.rs:966-971, 1561-1567`). The proxy swallows any provider error into `Ok(None)` (`gateway_provider_proxy.rs:2559-2561`), so the purchase stays "pending exact Wallet or Chain settlement" forever.

**Files:**
- Modify: `elastos/crates/elastos-server/src/protected_content_runtime.rs:142` (new constant)
- Modify: `elastos/crates/elastos-server/src/api/gateway_provider_proxy.rs:2540-2565`
- Test: `elastos/crates/elastos-server/src/api/gateway_tests/library.rs` (next to the buy tests at `:7143`, `:7284`), `gateway_tests/support_providers.rs` (chain support provider gains an `unbound` mode)

**Interfaces:**
- Produces: `pub(crate) const RUNTIME_CUSTODY_PURCHASE_UNBOUND_MESSAGE: &str = "Runtime custody purchase target is not bound on chain";` `Pending` purchases are marked `Complete { terminal: Denied }`-equivalent? No: the ledger keeps `Pending` semantics untouched; the buy response is an error with this message and the effect is recorded failed, not pending.

- [ ] **Step 1: Failing gateway test.** Copy the pending-buy test at `library.rs:7284`, switch the chain support provider to return `Response::error("unknown_protected_content_object", …)` for `resolve_protected_content_purchase_access`, and assert the buy response body error message equals `RUNTIME_CUSTODY_PURCHASE_UNBOUND_MESSAGE` and that `load_runtime_custody_purchase` afterwards is `Pending { confirmed_buy: None, .. }` or `None` (never a stuck `confirmed_buy`).
- [ ] **Step 2:** `just test-crate elastos-server -- gateway_tests::library::buy_reports_unbound_content_access_id`. Expected: FAIL (message is the pending one).
- [ ] **Step 3:** In the proxy, replace `let Ok(response) = response else { return Ok(None); };` with a match: on `Err(err)` where the provider error code is `unknown_protected_content_object`, `tracing::warn!(%request_id, "protected-content purchase target unbound on chain")` and `anyhow::bail!(RUNTIME_CUSTODY_PURCHASE_UNBOUND_MESSAGE)`; on any other `Err`, keep `Ok(None)` but log at `debug`. Read how provider error codes surface from `provider_call` in that file first (the `provider_error` helper at `library.rs:6539-6558` always emits `library_error` for the Library surface; the proxy has its own code path).
- [ ] **Step 4:** Re-run Step 2 and the neighbouring buy tests. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -am "fix(protected-content): surface chain-unbound purchase targets instead of pending forever"`

---

### Task 5: #49 item 4 — Marketplace buy affordance and ESP doc posture

The gateway grants `buy` to `MARKETPLACE_CAPSULE_ID` (`gateway_provider_proxy.rs:1646`) and Marketplace calls it (`marketplace.js:1148-1166`), but `capsules/marketplace/capsule.json` declares no method on `elastos://object/*`, so the Capsule Inspector shows a grant with no affordance. `docs/ESP_V0.md:178-181,196` and `docs/SHELL_ESP_BOUNDARY_MAP.md:83` still describe buy as future.

**Files:**
- Modify: `capsules/marketplace/capsule.json` (interfaces), `docs/ESP_V0.md:178-181,196`, `docs/SHELL_ESP_BOUNDARY_MAP.md:83`
- Test: `scripts/home-entropy-check.mjs` (add an assert that the Marketplace manifest declares `"operation": "buy"` on `elastos://object/*`), `elastos/crates/elastos-server/src/api/gateway_capsule_catalog.rs:912` test if it enumerates Marketplace methods

- [ ] **Step 1:** Add to the entropy check next to the `protectedHomeSurface` block (`:4729`):

```js
const marketplaceManifest = JSON.parse(read("capsules/marketplace/capsule.json"));
assert(
  marketplaceManifest.interfaces.some((iface) =>
    iface.methods.some((m) => m.resource === "elastos://object/*" && m.operation === "buy" && m.approval === "user"),
  ),
  "Marketplace must declare the protected-content buy affordance it exercises",
);
```

- [ ] **Step 2:** Run the check. Expected: FAIL.
- [ ] **Step 3:** Append a second interface to `capsules/marketplace/capsule.json`:

```json
{
  "id": "elastos.marketplace.protected-content",
  "version": "0.1.0",
  "description": "Buy listed protected content and import it into Library",
  "methods": [
    { "id": "content.buy", "description": "Buy one copy of a listed protected item with the default wallet.", "risk": "write", "approval": "user", "audit": "full", "resource": "elastos://object/*", "operation": "buy" },
    { "id": "content.import", "description": "Import a bought protected item into Library.", "risk": "write", "approval": "user", "audit": "full", "resource": "elastos://object/*", "operation": "import_runtime_custody" },
    { "id": "content.list", "description": "List protected items available to buy.", "risk": "read", "approval": "runtime_policy", "audit": "summary", "resource": "elastos://object/*", "operation": "list_runtime_custody" }
  ]
}
```

- [ ] **Step 4:** Reword `docs/ESP_V0.md:178-181,196` and `docs/SHELL_ESP_BOUNDARY_MAP.md:83` so each says: Marketplace exercises `object://…/buy` and `import_runtime_custody` through the gateway provider proxy today; ESP capsule-side purchase orchestration remains out of scope. Run `node scripts/home-entropy-check.mjs` (standing-docs prose lint) and `scripts/check-wci-alignment.sh` if it lists Marketplace methods.
- [ ] **Step 5:** `just test-crate elastos-server -- gateway_capsule_catalog` passes (update the method count if a test pins it).
- [ ] **Step 6: Commit.** `git commit -am "docs(marketplace,esp): declare the live buy affordance and scope ESP purchase text"`

---

### Task 6: #49 item 1 — `buy` action granularity and approval-stage idempotency

`provider_resource.rs:265-266` folds `buy` into `Action::Write`, so any capsule holding `write` on `elastos://object/*` may spend. `RuntimeCustodyPurchaseProgress::Pending { confirmed_buy }` (`protected_content_runtime.rs:5170-5178`) records the buy stage but not the ERC-20 approval stage, and the proxy re-drives approval unconditionally (`gateway_provider_proxy.rs:3487-3500`) on every retry.

**Files:**
- Modify: `elastos/crates/elastos-server/src/provider_resource.rs:255-270` (+ the `Action` enum and every `match` on it in that file and `gateway_provider_proxy.rs`), `capsules/object-provider/capsule.json:13`
- Modify: `protected_content_runtime.rs:5170-5178`, `gateway_provider_proxy.rs:3487-3500`
- Test: `provider_resource.rs` tests, `gateway_tests/library.rs:7143` neighbourhood

**Interfaces:**
- Produces: `Action::Buy`; `"buy"` in the object-provider `actions` array; `Pending { confirmed_approval: Option<RuntimeCustodyConfirmedPurchaseStage>, confirmed_buy: Option<…> }` both `#[serde(default, skip_serializing_if = "Option::is_none")]`.

- [ ] **Step 1: Failing tests.**

```rust
// provider_resource.rs tests
#[test]
fn buy_requires_its_own_action() {
    assert_eq!(object_operation_action("buy"), Some(Action::Buy));
    assert!(manifest_grants_action(&manifest_with_actions(&["read", "write", "delete"]), Action::Buy).is_none());
    assert!(manifest_grants_action(&manifest_with_actions(&["read", "write", "delete", "buy"]), Action::Buy).is_some());
}
```

and in `gateway_tests/library.rs` a test that drives buy where the approval stage completes and the buy stage stays pending on the first call, then asserts the second call does not re-issue an approval request to the wallet support provider (count approval requests on the support provider) and the persisted record has `confirmed_approval.is_some()`.

- [ ] **Step 2:** Run both. Expected: FAIL.
- [ ] **Step 3:** Add `Buy` to `Action`; map `"buy" => Some(Action::Buy)`; teach the manifest capability check to require `"buy"` in `actions` for that variant; add `"buy"` to `capsules/object-provider/capsule.json` actions. Add `confirmed_approval` to `Pending`; in the proxy, skip `complete_runtime_custody_purchase_stage` for approval when `confirmed_approval.is_some()`, and persist it once the approval completion returns `Some`.
- [ ] **Step 4:** Re-run Step 2, then `just test-crate elastos-server` fully, then `just verify` (first checkpoint). Expected: all green.
- [ ] **Step 5: Commit.** `git commit -am "fix(protected-content): dedicated buy action and idempotent approval stage"`

---

### Task 7: #42 gap 2 — `creator` app capsule

Port the dkms `capsules/creator` UX (upload → protect → list steps, `setStep`, `humanSize`, `resolveMime`, `onFile`, `refreshMintEnabled`) onto today's protocol: upload through the Library transport (`capsules/library/browser/src/api.js:26-103`: PUT `/api/provider/object/upload?uri=` up to 512 KiB, else `start`/`chunk`/`finish`), then `POST /api/provider/object/publish` with `{ uri, if_revision, protection: { mode: "runtime_custody", copies, price } }` (`actions.js:344-386`, hex quantities from `decimalIntegerToHexQuantity`, `model.js:177-190`). Drop dkms wallet/channel selection, thumbnails and `/api/apps/creator/*` routes (they do not exist here; the default wallet account is resolved server-side).

**Files:**
- Create: `capsules/creator/capsule.json`, `capsules/creator/browser/{index.html, creator.js, style.css, creator.test.mjs}`, `capsules/creator/browser/icons/{icon.svg, icon-32.png, icon-64.png, icon-128.png, icon-256.png}` (original marks; generate PNGs from the SVG with the same tool used for `elacity-player`)
- Modify (registration checklist, 19 places — every one is asserted somewhere): `components.json` (external entry like `elacity-player` at `:1114-1124` plus all five profile lists at `:1389, 1441, 1508, 1559, 1612`); `elastos/crates/elastos-server/src/setup.rs:2689-2712` (profile assertion) and `:3050-3079`; `elastos/crates/elastos-server/src/publish.rs:42,79`; `scripts/publish-release.sh:79,113,869`; `scripts/setup-source-home.sh:613`; `scripts/local-carrier-setup-smoke.sh:134,186,295,497`; `elastos/crates/elastos-server/src/api/gateway_capsule_catalog.rs:912` test; `capsules/home/browser/home-shell-host.js:72-91` (`creator: new Set(["library"])`) + entropy pins; `scripts/check-wci-alignment.sh:1083`; `scripts/vendor-ui-tokens.sh:32-50` TARGETS (Creator uses the shared tokens, so it joins the list and runs `just vendor-ui`); `docs/PROTECTED_CONTENT.md`.
- Modify: `elastos/crates/elastos-server/src/api/gateway.rs:224` (`const CREATOR_CAPSULE_ID: &str = "creator";`), `gateway_provider_proxy.rs:390, 543, 640, 716` (upload handlers → `&[LIBRARY_CAPSULE_ID, CREATOR_CAPSULE_ID]`), and the `object` op arm: split `"roots" | "stat" | "publish"` out to `&[LIBRARY_CAPSULE_ID, CREATOR_CAPSULE_ID]`.
- Test: `gateway_tests/library.rs` (Creator token may upload+publish; Creator token is refused `list`, `write`, `trash`), `capsules/creator/browser/creator.test.mjs`, `scripts/home-entropy-check.mjs`

**Interfaces:**
- Consumes: Library transport function bodies from `api.js:26-103` (copy, do not import across capsules), `decimalIntegerToHexQuantity` (copy).
- Produces: capsule `creator` (role `app`), one interface `elastos.creator.protect` with methods `asset.upload` (operation `write`? No: upload handlers are their own routes; declare `roots`, `stat`, `publish` on `elastos://object/*`).

- [ ] **Step 1: Failing gateway tests.** Add `creator_token_can_upload_and_publish_runtime_custody` (mirror the Library protect-and-list happy path at `gateway_tests/library.rs` with a Creator launch token) and `creator_token_is_refused_library_only_ops` asserting 403 on `list`, `write`, `trash`.
- [ ] **Step 2:** Run; expected FAIL (403 on upload).
- [ ] **Step 3:** Add `CREATOR_CAPSULE_ID` and the five allowlist edits. Re-run; expected PASS.
- [ ] **Step 4: Capsule manifest.**

```json
{
  "schema": "elastos.capsule/v1",
  "name": "creator",
  "version": "0.1.0",
  "description": "Protect a file and list it for sale in one flow",
  "role": "app",
  "type": "wasm",
  "runtime_abi": "elastos.runtime-projection/v1",
  "bus_contract": "elastos.runtime-projection/v1",
  "execution": "web-projection",
  "projections": ["web", "facts", "affordances", "gates"],
  "author": "elastos",
  "entrypoint": "browser/index.html",
  "icon": "browser/icons",
  "interfaces": [
    {
      "id": "elastos.creator.protect",
      "version": "0.1.0",
      "description": "Upload a file into Library, protect it, and list copies at a price",
      "methods": [
        { "id": "library.roots", "description": "Find the Library folder to upload into.", "risk": "read", "approval": "runtime_policy", "audit": "summary", "resource": "elastos://object/*", "operation": "roots" },
        { "id": "library.stat", "description": "Confirm the uploaded file before protecting it.", "risk": "read", "approval": "runtime_policy", "audit": "summary", "resource": "elastos://object/*", "operation": "stat" },
        { "id": "content.protect_and_list", "description": "Protect the uploaded file and list copies at a price.", "risk": "write", "approval": "user", "audit": "full", "resource": "elastos://object/*", "operation": "publish" }
      ]
    }
  ],
  "requires": [{ "name": "object-provider", "kind": "capsule" }],
  "resources": { "memory_mb": 32, "gpu": false }
}
```

- [ ] **Step 5: `creator.js` (pure functions first, tested).** Export from the module and cover in `creator.test.mjs` with `node --test`:

```js
export function targetUriFor(rootUri, fileName) // `${rootUri}/Creator/${sanitizeName(fileName)}`; rejects "..", "/", control chars
export function classifyProtection(mime)          // "media" for video/* and audio/*, "object" otherwise (drives the copy shown: "plays in Elacity Player" vs "opens in Elacity Reader")
export function decimalIntegerToHexQuantity(value) // verbatim copy of Library's
export function uploadPlan(size)                   // { mode: "single" } when size <= 512 * 1024 else { mode: "chunked", chunkBytes: 768 * 1024 }
```

Then the DOM flow: file input → `onFile` (size, mime, kind) → copies/price fields → "Protect and list" button → steps Upload / Protect / List with `setStep` → success panel with mint id and an "Open in Library" button that posts the shell open message for `library` (allowed by the new `creator` allowlist key). Token comes from the URL hash like Marketplace (`marketplace.js` top); post `home:app-ready` to `window.top` on load. Handle `409` (`if_revision`) by re-`stat`ing once.

- [ ] **Step 6:** `node --test capsules/creator/browser/` passes. Add the file to the Library JS test line in `justfile`/CI as `node --test capsules/creator/browser/`.
- [ ] **Step 7: Registration.** Walk the 19-site checklist above; each site is asserted by `setup.rs:2689-2712`, `gateway_capsule_catalog.rs:912`, `scripts/check-wci-alignment.sh`, or the entropy check, so run `just verify` and fix until green. Add shell allowlist `creator: new Set(["library"])` and its entropy pin; add the Creator icon assert block mirroring `:3757-3761`.
- [ ] **Step 8: Installed proof.** `scripts/setup-source-home.sh` (or the incremental capsule install path used for `elacity-player`), restart, open Creator from Home, upload a small `.mp4`, protect with copies 1 price 1, confirm the item appears in Library as protected and opens in Elacity Player (Task 1). Record artifact SHA-256 and the command in the PR evidence section.
- [ ] **Step 9: Commit.** `git commit -am "feat(creator): Creator app uploads through Library and protects-and-lists in one flow"`

---

### Task 8: Contracts — object protection and viewer ops

Media-only op sets: `ProtectProviderRequestOpV1` (`protect.rs:35-41`) and `DecryptProviderRequestOpV1` (`decrypt.rs:54-60`). Add object variants and an object identity type. `CencFmp4MediaIdentityV1` stays untouched.

**Files:**
- Create: `elastos/crates/elastos-protected-content-provider-contracts/src/object.rs`
- Modify: `protect.rs` (ops + request/response arms), `decrypt.rs` (ops + arms), `lib.rs` (`mod object; pub use object::*`), `wire.rs` only if op names are enumerated there
- Test: `object.rs` test module + existing `authority_tests.rs` style round-trips

**Interfaces:**
- Produces:

```rust
pub const MAX_OBJECT_PLAINTEXT_CHUNK_BYTES_V1: usize = 1_048_576;             // == custody PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1
pub const MAX_OBJECT_FRAMED_CHUNK_BYTES_V1: usize = MAX_OBJECT_PLAINTEXT_CHUNK_BYTES_V1 + 16 + 8; // tag + framing slack; assert against payload.rs in Task 9
pub const MAX_OBJECT_FRAMED_HEADER_BYTES_V1: usize = 4 + 2 + 512;
pub const MAX_OBJECT_PLAINTEXT_BYTES_V1: u64 = 64 * 1024 * 1024;              // dkms MAX_OBJECT_BYTES parity
pub const MAX_OBJECT_CHUNKS_V1: u32 = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkedPayloadObjectIdentityV1 {
    encrypted_content: EncryptedContentIdentityV1, // sha256 + byte length of the whole framed file
    content_type: String,                          // visible ASCII, <= 255
    plaintext_bytes: u64,
    framed_header_bytes: u32,                      // byte length of magic+len+header at file offset 0
}
impl ChunkedPayloadObjectIdentityV1 {
    pub fn new(encrypted_content, content_type, plaintext_bytes, framed_header_bytes) -> Result<Self, ContractError>;
    pub fn chunk_count(&self) -> u32;             // ceil(plaintext_bytes / 1 MiB)
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ContractError>;
    pub fn from_canonical_bytes(&[u8]) -> Result<Self, ContractError>;
    // accessors for every field
}

pub enum ProtectProviderRequestOpV1 { …, OpenObjectProtectionSession, ProtectObjectChunk, FinalizeObjectProtectionSession }
pub enum DecryptProviderRequestOpV1 { …, ReadViewerObjectChunk }
```

Request/response payloads (JSON, `deny_unknown_fields`, blobs as `CanonicalBlob<N>` like the media parts):
- `OpenObjectProtectionSession { session_id, content_type, plaintext_bytes, nodes: Vec<ProtectionSessionNodeV1>, threshold }` → `{ framed_header }`
- `ProtectObjectChunk { session_id, chunk_index, plaintext }` → `{ framed_chunk }`
- `FinalizeObjectProtectionSession { session_id, encrypted_content: EncryptedContentIdentityV1 }` (the Runtime computes it from its staged file; the provider checks it equals its running hash) → `{ object_identity, content_key_commitment, custody_envelope }`
- `OpenViewerSession` gains `content: ViewerContentV1 { Media(media_identity_blob) | Object { object_identity_blob, framed_header } }` (keep the existing field name for media so existing test vectors keep decoding; add the object alternative as a sibling optional field with an "exactly one" validation).
- `ReadViewerObjectChunk { session_handle, chunk_index, framed_chunk }` → `{ plaintext }`

- [ ] **Step 1: Failing round-trip tests** for `ChunkedPayloadObjectIdentityV1` (canonical bytes stable, rejects `content_type` > 255 or non-ASCII, rejects `plaintext_bytes` > 64 MiB, `chunk_count` boundary at exactly 1 MiB and 1 MiB + 1) and for each new request/response (encode → decode equality, unknown field rejected, oversize blob rejected).
- [ ] **Step 2:** `just test-crate elastos-protected-content-provider-contracts -- object::`. Expected: FAIL to compile.
- [ ] **Step 3:** Implement `object.rs`, the op variants, the wire-name strings (`"open_object_protection_session"`, `"protect_object_chunk"`, `"finalize_object_protection_session"`, `"read_viewer_object_chunk"`), and the `OpenViewerSession` sibling field.
- [ ] **Step 4:** Re-run; PASS. Run the whole crate: `just test-crate elastos-protected-content-provider-contracts`.
- [ ] **Step 5: Commit.** `git commit -am "feat(contracts): chunked-payload object identity and object protect/viewer ops"`

---

### Task 9: Custody — streaming sealer and chunk decrypter over `payload.rs`

`payload.rs` seals or decrypts a whole stream in one call. Providers receive one chunk per frame, so expose the existing loop body as two small stateful types. No new primitive, same AAD (`domain ‖ header ‖ index`, `payload.rs:445-453`), same nonce derivation, same identity (`sha256(framed) + length`, `:339-340`).

**Files:**
- Modify: `elastos/crates/elastos-protected-content-custody/src/payload.rs`, `src/lib.rs` (re-exports)
- Test: `payload.rs` test module

**Interfaces:**
- Produces:

```rust
pub struct PayloadSealerV1 { /* header, key, base_nonce, next_index, running_sha256, framed_bytes */ }
impl PayloadSealerV1 {
    pub fn open(content_type: &str, plaintext_bytes: u64) -> Result<(Self, Vec<u8> /* framed header */), CustodyError>;
    pub fn seal_chunk(&mut self, chunk_index: u32, plaintext: &[u8]) -> Result<Vec<u8> /* framed chunk */, CustodyError>; // index must equal next_index; size must be 1 MiB except the last
    pub fn finish(self, committee: &ValidatedCustodyCommitteeV1) -> Result<SealedPayloadMetadataV1, CustodyError>; // errors unless all chunks were sealed; provisions the envelope exactly like seal_payload_to_staging_writer_v1
}
pub struct PayloadChunkDecrypterV1 { /* header, key */ }
impl PayloadChunkDecrypterV1 {
    pub fn new(framed_header: &[u8], content_key: &ContentEncryptionKeyV1) -> Result<Self, CustodyError>; // parses header, checks content_key_commitment
    pub fn header(&self) -> &AuthenticatedChunkPayloadHeaderV1;
    pub fn decrypt_chunk(&self, chunk_index: u32, framed_chunk: &[u8]) -> Result<Vec<u8>, CustodyError>;
}
pub fn framed_chunk_ranges_v1(framed_header_bytes: u32, plaintext_bytes: u64) -> impl Iterator<Item = std::ops::Range<u64>>; // byte ranges of each framed chunk inside the sealed file, derived from the framing constants
pub fn reconstruct_content_key_for_object_session(...)  // only if the existing reconstruct fn is not already public; otherwise reuse `reconstruct_content_key_from_authenticated_operation`
```

- [ ] **Step 1: Failing equivalence tests.**

```rust
#[test]
fn streaming_sealer_matches_one_shot_seal_byte_for_byte() {
    let (key, nonce) = fixed_test_key_and_nonce();
    let plaintext = deterministic_bytes(3 * 1_048_576 + 12345);
    let one_shot = seal_with_context_for_tests("application/pdf", &plaintext, &key, nonce); // cfg(test) hook over seal_payload_to_staging_writer_inner
    let (mut sealer, header) = PayloadSealerV1::open_with_context_for_tests("application/pdf", plaintext.len() as u64, &key, nonce);
    let mut framed = header;
    for (index, chunk) in plaintext.chunks(1_048_576).enumerate() {
        framed.extend(sealer.seal_chunk(index as u32, chunk).unwrap());
    }
    assert_eq!(framed, one_shot);
}

#[test]
fn chunk_decrypter_round_trips_and_rejects_reordered_chunks() { /* seal, decrypt each range from framed_chunk_ranges_v1, assert plaintext; swap two chunk indexes and assert Err */ }

#[test]
fn framed_chunk_ranges_cover_the_sealed_file_exactly() { /* ranges are contiguous from framed_header_bytes to total length */ }

#[test]
fn sealer_finish_requires_every_chunk() { /* open, seal 1 of 2, finish → Err */ }
```

- [ ] **Step 2:** `just test-crate elastos-protected-content-custody -- payload::`. Expected: FAIL to compile.
- [ ] **Step 3:** Refactor `seal_payload_to_staging_writer_inner` to drive `PayloadSealerV1` internally (so there is one code path), implement the decrypter by extracting the per-chunk body of `decrypt_payload_to_staging_writer_with_content_key_v1`, add the ranges helper. Keep the one-shot public functions unchanged in signature.
- [ ] **Step 4:** Re-run; all `payload::` tests (old and new) PASS. `just test-crate elastos-protected-content-custody`.
- [ ] **Step 5: Commit.** `git commit -am "feat(custody): streaming EPC1 sealer and chunk decrypter over the existing payload container"`

---

### Task 10: Protect provider — object protection session

`capsules/protected-content-protect-provider/src/lib.rs` handles media sessions (`ProtectionSessionEntry` `:99-118`, dispatch `:232-640`, finalize `:545-611` provisioning through `provision_custody_envelope_for_exact_nodes` `:584`, aggregate cap 16 MiB `:30`, 64 sessions `:28`).

**Files:**
- Modify: `capsules/protected-content-protect-provider/src/lib.rs`
- Test: `capsules/protected-content-protect-provider/tests/process.rs`

**Interfaces:**
- Consumes: Task 8 ops, Task 9 `PayloadSealerV1`.
- Produces: `enum ProtectionSessionKind { Media(MediaSessionState), Object(PayloadSealerV1) }` inside `ProtectionSessionEntry`; object sessions are not subject to `MAX_AGGREGATE_PROTECTED_MEDIA_BYTES_V1` (the sealer is streaming) but are capped by `MAX_OBJECT_PLAINTEXT_BYTES_V1` and `MAX_OBJECT_CHUNKS_V1` from the request.

- [ ] **Step 1: Failing process test** in `tests/process.rs`: open object session (content type `application/pdf`, 2.5 MiB), send three chunks, finalize with the Runtime-side identity computed over `header ‖ chunks`, assert the response `object_identity` equals it, `custody_envelope` decodes, and that the media op `ProtectMediaSegment` on an object session returns the provider's invalid-session error. Add a second test: wrong `chunk_index` order → error and session cancelled.
- [ ] **Step 2:** `(cd capsules/protected-content-protect-provider && cargo test --test process object_session)`. Expected: FAIL.
- [ ] **Step 3:** Implement the three arms; on finalize compare the request `encrypted_content` with the sealer's running identity before provisioning; log `info!(session_id, chunks, plaintext_bytes, "object protection session finalized")`.
- [ ] **Step 4:** Re-run; PASS. `(cd capsules/protected-content-protect-provider && cargo test)`.
- [ ] **Step 5: Commit.** `git commit -am "feat(protect-provider): chunked object protection sessions"`

---

### Task 11: Decrypt provider — object viewer session and chunk read

`capsules/protected-content-decrypt-provider/src/lib.rs` (`ViewerSessionEntry` `:119-127`, CEK reconstruction `:482-510`, `ReadViewerMediaPart` `:568-628`).

**Files:**
- Modify: `capsules/protected-content-decrypt-provider/src/lib.rs`
- Test: `capsules/protected-content-decrypt-provider/tests/` (support at `tests/support.rs`)

**Interfaces:**
- Consumes: Task 8 `OpenViewerSession` object alternative and `ReadViewerObjectChunk`; Task 9 `PayloadChunkDecrypterV1`.
- Produces: `enum ViewerSessionContent { Media { … existing … }, Object(PayloadChunkDecrypterV1) }`; `ReadViewerMediaPart` on an object session and `ReadViewerObjectChunk` on a media session both return the invalid-session error.

- [ ] **Step 1: Failing tests:** open an object viewer session with a valid authenticated release operation whose binding's `encrypted_content` equals the object identity's (reuse the media test's operation builder with the object identity), read chunks 0..n and assert plaintext, tamper one framed chunk byte → error, cross-kind op → error.
- [ ] **Step 2:** `(cd capsules/protected-content-decrypt-provider && cargo test object_viewer)`. Expected: FAIL.
- [ ] **Step 3:** Implement: reconstruct the CEK exactly as `:482-510` does, build `PayloadChunkDecrypterV1::new(framed_header, &key)` (this checks the header commitment), store it in the session.
- [ ] **Step 4:** Re-run; PASS. Whole capsule test.
- [ ] **Step 5: Commit.** `git commit -am "feat(decrypt-provider): object viewer sessions decrypt one framed chunk per frame"`

---

### Task 12: Mint journal — content identity enum and `epc-mj06` codec

`RuntimeMintDraft.media_identity: CencFmp4MediaIdentityV1` (`mint_journal.rs:983`) is encoded positionally with no discriminant (`encode_record` `:2418`, `decode_record` `:2542`), under `STORE_MAGIC = b"epc-mj05"` and `STORE_DIGEST_DOMAIN …/v5` (`:31-32`).

**Files:**
- Modify: `elastos/crates/elastos-protected-content-runtime/src/mint_journal.rs`, `src/mint.rs`, `src/open.rs`, `src/coordinator.rs` (rename `media_identity()` accessor uses to `content_identity()`; add `media_identity()` returning `Option<&CencFmp4MediaIdentityV1>` for the existing callers)
- Test: `mint_journal.rs` tests, `test_media.rs` (add `test_object.rs` fixture builder)

**Interfaces:**
- Produces:

```rust
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RuntimeContentIdentityV1 {
    Media(CencFmp4MediaIdentityV1),            // kind byte 0x01
    Object(ChunkedPayloadObjectIdentityV1),    // kind byte 0x02
}
impl RuntimeContentIdentityV1 {
    pub fn encrypted_content(&self) -> &EncryptedContentIdentityV1;
    pub fn content_type(&self) -> &str;        // mime_type for media, content_type for objects
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, RuntimeMintJournalError>;      // kind byte ‖ nested canonical bytes
    pub fn from_canonical_bytes(&[u8]) -> Result<Self, RuntimeMintJournalError>;
}
const STORE_MAGIC_V5: &[u8; 8] = b"epc-mj05"; const STORE_MAGIC: &[u8; 8] = b"epc-mj06";
const STORE_DIGEST_DOMAIN_V5 / STORE_DIGEST_DOMAIN (…/v6)
```

`compute_mint_id` (`:1146`) hashes the draft's canonical bytes; for `Media` it must remain byte-identical to today (no kind byte inside the mint-id preimage for media) so existing mint ids stay stable. Only the journal record framing gains the kind byte; `Object` drafts hash `0x02 ‖ object canonical bytes` under the same domain.

- [ ] **Step 1: Failing tests:** (a) a v5 record fixture (hex of a record encoded by the current code, generated once in the test with the pre-change encoder and pasted as a literal) decodes to `Media`; (b) encode→decode round-trip for an `Object` draft; (c) `compute_mint_id` of a media draft equals the literal produced by today's code (pin the current value first with a one-off test run before changing anything); (d) a v6 record with kind byte `0x03` is `Corrupt`.
- [ ] **Step 2:** `just test-crate elastos-protected-content-runtime -- mint_journal::`. Expected: FAIL to compile.
- [ ] **Step 3:** Implement enum, encoder writes v6, decoder accepts v5 (media only, positional) and v6 (kind byte), `RuntimeMintDraft::new` takes `RuntimeContentIdentityV1`. Fix callers in `mint.rs`, `open.rs`, `coordinator.rs` with the `media_identity()` option accessor.
- [ ] **Step 4:** Re-run; PASS; whole crate green.
- [ ] **Step 5: Commit.** `git commit -am "feat(mint-journal): content identity enum with versioned record codec (v5 read, v6 write)"`

---

### Task 13: Runtime — object publish branch, object twins, viewer response, admission v3

The publish path branches after `read_runtime_media_source` (`protected_content_runtime.rs:3699-3703`) into media preparation (`RuntimeLibraryMediaPreparation` `:3766/3781`) and `RuntimeCustodyLibraryPublishInput.clear_init_segment/clear_segments` (`:3789-3791`). Six helpers are media-only: `verify_protected_content_directory` `:2884`, `protected_content_files` `:2991`, `verify_protected_content_manifest_and_files` `:3034`, `write_protected_content_staging_directory` `:4504`, `require_media_part_index` `:5475`, `runtime_custody_viewer_public_response` `:7654`. Session binding v2 hard-codes `ELACITY_PLAYER_CAPSULE_ID` (`:7606`); the proxy admits only the player (`gateway_provider_proxy.rs:1644, 1707-1717`).

**Files:**
- Modify: `elastos/crates/elastos-server/src/protected_content_runtime.rs` (all sites above + `RuntimeCustodyLibraryPublishInput` gains `content: RuntimeCustodyLibraryPublishContent { Media { mime_type, codecs, clear_init_segment, clear_segments } | Object { content_type, plaintext_bytes, source_file: fs::File } }`)
- Modify: `elastos/crates/elastos-server/src/api/gateway.rs:224` (`const ELACITY_READER_CAPSULE_ID: &str = "elacity-reader";`), `gateway_provider_proxy.rs:1644, 1707-1717`
- Test: `protected_content_runtime/tests.rs`, `gateway_tests/library.rs` (+ `support_providers.rs` gains object arms mirroring the real providers via the process providers under `ELASTOS_TEST_{PROTECT,DECRYPT}_PROVIDER_BIN`)

**Interfaces:**
- Consumes: Tasks 8–12.
- Produces:
  - Protected content directory for objects: `object.epc1` (the framed file) + `manifest.json` with `{"schema":"elastos.protected-content.object-manifest/v1","content_type","plaintext_bytes","framed_header_bytes","encrypted_content":{sha256,bytes}}`; `verify_protected_content_directory` dispatches on manifest schema.
  - Viewer response for objects: `{"schema":"elastos.library.runtime-custody-viewer/v1","mint_id","viewer_session_handle","expires_at","content_kind":"object","content_type","plaintext_bytes","chunk_count","chunk_bytes":1048576}`; media responses add `"content_kind":"media"` and keep every existing field (`:7663-7670`).
  - `read_viewer` for objects takes `{"viewer_session_handle","chunk_index"}`; the Runtime slices `object.epc1` by `framed_chunk_ranges_v1`, sends `ReadViewerObjectChunk`, returns `{"plaintext": <base64>}` (match how media parts are returned today; if they are raw bytes on a separate route, do the same).
  - Session binding domain `elastos.protected-content.runtime-session-binding.v3` with the verified `required.launch_context.executable_actor` in place of the constant; the proxy admits `{ELACITY_PLAYER_CAPSULE_ID, ELACITY_READER_CAPSULE_ID}` at `:1644` and requires `selected_resource == executable_actor ∈ that set` at `:1707-1717`; `open_viewer` additionally checks kind ↔ viewer (media → player, object → reader) and returns the existing "not authorized for this viewer" 403 otherwise.

- [ ] **Step 1: Failing runtime tests** in `protected_content_runtime/tests.rs`: `publish_object_seals_with_epc1_and_writes_object_manifest` (a 2.5 MiB `application/pdf` source → directory has `object.epc1` whose sha256/len match the manifest and the journal draft is `Object`), `verify_protected_content_directory_rejects_tampered_object_file`, `object_viewer_response_reports_chunk_geometry`, `session_binding_v3_differs_per_executable_actor`.
- [ ] **Step 2: Failing gateway tests** in `gateway_tests/library.rs`: `reader_token_opens_object_and_reads_every_chunk`, `player_token_is_refused_for_object_mint`, `reader_token_is_refused_for_media_mint`, `library_token_is_refused_viewer_ops` (already exists for the player; extend).
- [ ] **Step 3:** Run both sets; expected FAIL.
- [ ] **Step 4:** Implement the branch: at `:3699` after reading the source, if the Library object's mime is `video/*` or `audio/*` → existing media path; otherwise → `prepare_runtime_object_publish` which streams the source file through the protect provider (`OpenObjectProtectionSession` → `ProtectObjectChunk` × n → `FinalizeObjectProtectionSession`), writing framed bytes to the staging directory, then computes `EncryptedContentIdentityV1::new(sha256(file), len)` and passes it to finalize. Object twins of the six helpers; journal draft with `RuntimeContentIdentityV1::Object`. Viewer session open for objects sends the object alternative with `framed_header` read from offset 0. Admission and binding v3. Log `info!` at publish finalize and viewer open with mint id and kind.
- [ ] **Step 5:** Re-run; PASS. `just test-crate elastos-server` whole crate (expect the media suites unchanged).
- [ ] **Step 6:** Installed proof after Task 15 (reader) is in; record here that the proof is deferred to Task 15 Step 9.
- [ ] **Step 7: Commit.** `git commit -am "feat(protected-content): non-media objects publish through EPC1 sessions and open in a second admitted viewer"`

---

### Task 14: Library — protect any file, route by kind

Predicates `isRuntimeCustodyProtectableVideo` (`model.js:162-176`, `startsWith("video/")`) and `isRuntimeCustodyProtectedVideo` (`actions.js:98-103`) gate the flow; `openProtectedVideo` (`:105-111`) targets the player only.

**Files:**
- Modify: `capsules/library/browser/src/model.js:162-176`, `src/actions.js:76-111, 347`, `src/app.js:1013`, `src/dialog.js:811-845` (copy: "Protect and list" wording no longer says video)
- Test: `src/model.test.mjs` (create if absent), `src/actions.test.mjs` (12 fixtures on `mime: "video/mp4"` gain `application/pdf`, `image/png`, `application/epub+zip`, `application/vnd.comicbook+zip` counterparts)

**Interfaces:**
- Produces:

```js
export function protectedContentKindFor(mime) // "media" | "object"; media = video/* or audio/*
export function isRuntimeCustodyProtectable(object)  // same guards as before minus the video check; still requires capabilities.includes("publish")
export function viewerForProtectedContent(object)   // "elacity-player" for media, "elacity-reader" for object
function isRuntimeCustodyProtected(object)           // metadata.protected_content.schema === RUNTIME_CUSTODY_METADATA_SCHEMA (no mime check)
function openProtectedContent(object)                 // openTarget(viewerForProtectedContent(object), { mint_id })
```

- [ ] **Step 1:** Failing tests for the four functions and for the routing (`openTarget` spy receives `elacity-reader` for a protected PDF, `elacity-player` for protected mp4 and mp3).
- [ ] **Step 2:** `node --test capsules/library/browser/src/`. Expected: FAIL.
- [ ] **Step 3:** Implement; rename call sites; update dialog copy to "Protect and list this file"; status strings "Protected content is unavailable." Keep `.mp3`/audio in the media kind (Task 16 makes the server accept it).
- [ ] **Step 4:** Re-run; PASS. `node scripts/library-product-behavior-smoke.mjs` and `node scripts/library-menu-smoke.mjs` (they pin some strings; update pins in the same commit).
- [ ] **Step 5: Commit.** `git commit -am "feat(library): protect any file and open protected items in the viewer for their kind"`

---

### Task 15: `elacity-reader` viewer capsule

Port `capsules/ddrm-viewer` from `origin/feat/dkms-esp-port` (`viewer.js`: `fetchManifest`, `fetchObjectBytes`, `kindFor`, `renderImage`, `renderText`, `renderModel3D`, `failClosed`, `assertNoKeyMaterial`; `MAX_OBJECT_BYTES = 64 MiB`) onto the `open_viewer`/`read_viewer`/`close_viewer` ops used by `capsules/elacity-player/browser/player.js:293-307`. dkms rendered PDF/EPUB/CBZ server-side (render-IR); here they render client-side per D2. No client-side zip or PDF code exists in the repo (research 2026-09-08), so this task adds a ~150-line central-directory zip reader over `DecompressionStream("deflate-raw")` and vendors pdf.js and three.js with xterm-style pins.

**Files:**
- Create: `capsules/elacity-reader/capsule.json`; `browser/index.html` (CSP copied from `elacity-player/browser/index.html:6` with `img-src 'self' data: blob:` and `worker-src 'self'` for pdf.js; `frame-src 'none'` stays); `browser/reader.js` (session + routing), `browser/zip.js` (`listEntries(bytes)`, `readEntry(bytes, entry) → Uint8Array`, stored + deflate only), `browser/cbz.js` (natural-sort image entries, page pager), `browser/epub.js` (parse `META-INF/container.xml` → OPF → spine; each XHTML chapter parsed with `DOMParser`, sanitized to an allowlist of block/inline tags, `img` sources resolved from the zip to `blob:` URLs, appended into a scrolling article), `browser/render.js` (`renderImage`, `renderPdf` via `pdfjsLib.getDocument({data})` page canvases, `renderText` with code detection from dkms `kindFor`, `renderModel` via three.js r160 + GLTFLoader/OBJLoader/STLLoader/OrbitControls from the dkms vendor set, `renderUnsupported(contentType)`); `browser/style.css`; `browser/{zip,epub,cbz,reader}.test.mjs`; `browser/vendor/pdfjs/{README.md, LICENSE, pdf.min.mjs, pdf.worker.min.mjs}`; `browser/vendor/three/{README.md, LICENSE, three.module.js, controls/OrbitControls.js, loaders/GLTFLoader.js, loaders/OBJLoader.js, loaders/STLLoader.js, utils/BufferGeometryUtils.js}`; `browser/icons/{icon.svg, icon-32/64/128/256.png}`.
- Modify: the same 19-site registration checklist as Task 7 for `elacity-reader` (viewer role), plus `home-shell-host.js:80` (`library` set gains `"elacity-reader"`) and its entropy pin, `scripts/home-entropy-check.mjs:4729` `protectedHomeSurface` gains `"elacity-reader"`, vendor pin assert blocks for pdfjs and three (mirror `:4108-4112`), and icon asserts (mirror `:3757-3761`). Do not add the reader to `vendor-ui-tokens.sh` TARGETS (content-only viewer, like the player).
- Test: `node --test capsules/elacity-reader/browser/`, a new reader smoke script `elacity-reader-smoke.mjs` under `scripts/`, modelled on `scripts/elacity-player-smoke.mjs` (jsdom-style fake fetch returning a chunked object; asserts the "no renderer" state for `application/x-unknown`, the pager for a 2-page CBZ fixture built in-test with `zip.js`'s stored mode, and the sanitizer stripping `<script>` from an EPUB chapter).

**Interfaces:**
- Consumes: Task 13 viewer response (`content_kind`, `content_type`, `plaintext_bytes`, `chunk_count`) and `read_viewer { viewer_session_handle, chunk_index }`.
- Produces: capsule `elacity-reader`, manifest:

```json
{
  "schema": "elastos.capsule/v1",
  "name": "elacity-reader",
  "version": "0.1.0",
  "description": "Protected document, image, book, comic and 3D viewer",
  "role": "viewer",
  "type": "wasm",
  "runtime_abi": "elastos.runtime-projection/v1",
  "bus_contract": "elastos.runtime-projection/v1",
  "execution": "web-projection",
  "projections": ["web", "facts", "affordances", "gates"],
  "author": "elastos",
  "entrypoint": "browser/index.html",
  "icon": "browser/icons",
  "interfaces": [{
    "id": "elastos.protected-content.reader",
    "version": "0.1.0",
    "description": "Open protected files you own and read them page by page",
    "methods": [
      { "id": "viewer.open",  "description": "Open a protected file session.",  "risk": "read", "approval": "runtime_policy", "audit": "event", "resource": "elastos://object/*", "operation": "open_viewer" },
      { "id": "viewer.read",  "description": "Read one part of a protected file.", "risk": "read", "approval": "runtime_policy", "audit": "event", "resource": "elastos://object/*", "operation": "read_viewer" },
      { "id": "viewer.close", "description": "Close a protected file session.",  "risk": "read", "approval": "runtime_policy", "audit": "event", "resource": "elastos://object/*", "operation": "close_viewer" }
    ]
  }],
  "input_schema": { "accepts": [
    { "kind": "protected_object", "mime": ["image/png","image/jpeg","image/gif","image/webp","image/svg+xml"], "mode": "view" },
    { "kind": "protected_object", "mime": ["application/pdf"], "mode": "paged" },
    { "kind": "protected_object", "mime": ["text/plain","text/markdown","application/json","text/x-*"], "mode": "text" },
    { "kind": "protected_object", "mime": ["model/gltf-binary","model/gltf+json","model/obj","model/stl"], "mode": "3d" },
    { "kind": "protected_object", "mime": ["application/epub+zip"], "mode": "book" },
    { "kind": "protected_object", "mime": ["application/vnd.comicbook+zip","application/x-cbz"], "mode": "paged" }
  ]},
  "requires": [{ "name": "object-provider", "kind": "capsule" }],
  "resources": { "memory_mb": 96, "gpu": false }
}
```

- [ ] **Step 1: zip.js TDD.** Tests: build a stored-mode zip in-test (write local headers + central directory by hand in the test helper), assert `listEntries` names and sizes; build a deflate entry with `CompressionStream("deflate-raw")`, assert `readEntry` round-trips; reject zip64 and encrypted flags with a clear error; reject an entry whose declared size exceeds 64 MiB.
- [ ] **Step 2:** `node --test capsules/elacity-reader/browser/zip.test.mjs`. Expected FAIL → implement → PASS.
- [ ] **Step 3: epub.js + cbz.js TDD.** EPUB: fixture zip with `mimetype`, `META-INF/container.xml`, `content.opf` (two spine items), two XHTML chapters (one containing `<script>` and an `<img>`), assert spine order, script stripped, `img` rewritten to `blob:`. CBZ: fixture with `page10.jpg, page2.jpg, page1.jpg` → natural order `[page1, page2, page10]`.
- [ ] **Step 4:** Run → FAIL → implement → PASS.
- [ ] **Step 5: reader.js.** Token from URL hash + `mint_id` query (same as player), `open_viewer` → if `content_kind !== "object"` → `failClosed("This file plays in Elacity Player.")`; read chunks `0..chunk_count` sequentially into one `Uint8Array(plaintext_bytes)` with a progress line; abort if `plaintext_bytes > 64 MiB`; dispatch on `content_type` to `render.js`; `close_viewer` on `pagehide`. Copy dkms `assertNoKeyMaterial` (refuse any response carrying `content_key`, `cek`, `iv`, `share` fields) as a defensive check.
- [ ] **Step 6: Vendoring.** Download pinned tarballs: `pdfjs-dist@4.x` (latest 4.x at execution time; record version, tarball URL, npm integrity, per-file SHA-256 in `vendor/pdfjs/README.md` and a `Verification command` block) and `three@0.160.0` (matches dkms r160; same README shape). Imports carry `?v=pdfjs-<version>` / `?v=three-0.160.0`. pdf.js worker: `pdfjsLib.GlobalWorkerOptions.workerSrc = "./vendor/pdfjs/pdf.worker.min.mjs?v=…"` (needs `worker-src 'self'` in the CSP).
- [ ] **Step 7: Registration + pins.** Walk the 19 sites; shell allowlist `library: new Set(["archive-manager", "documents", "elacity-player", "elacity-reader", "gba-emulator", "library"])` and update the Task 1 pin; `protectedHomeSurface` gains the reader; vendor pin asserts; icon asserts. `just verify` green (second checkpoint).
- [ ] **Step 8: Smoke.** the new reader smoke script added to the `verify-ci` recipe next to `elacity-player-smoke`.
- [ ] **Step 9: Installed proof** (covers Task 13 too): install providers and capsules, restart, use Creator to protect a small PDF, a PNG, a two-page CBZ and an EPUB (fixtures under `scratchpad`, not the repo), buy each from Marketplace with the second principal, open each from Library → reader renders; open the protected mp4 from Task 7 → player still plays. Record artifact SHA-256s, restart, and commands. Run `scripts/installed-provider-verify.sh protected-content-protect-provider` and `… protected-content-decrypt-provider`.
- [ ] **Step 10: Commit.** `git commit -am "feat(elacity-reader): protected image, PDF, text, 3D, EPUB and CBZ viewer with vendored pdf.js and three.js"`

---

### Task 16: Audio through the media path

Contracts already accept audio-only fMP4 (`media.rs:1580-1581, 1692-1693, 2316-2317`; `esds` anticipated at `:1706-1713`; only track-count rule `1..=8`). `capsules/media-provider` requires a video stream (`validate_probe_output`, `lib.rs:1007-1038`) and drops audio (`-an`, `:480`). The Runtime pins one mime/codec pair (`protected_content_runtime.rs:293-294`), checks equality (`:3377-3382`), and on crash-resume synthesizes the expected output from the constants and compares a receipt digest that binds mime+codecs (`:3777-3786`; `RuntimeMediaPreparationRecord` stores no mime/codecs). The player is already generic (`buildViewerMimeType`, `player.js:89-96`); MSE accepts an audio-only SourceBuffer on `<video>`.

**Files:**
- Modify: `capsules/media-provider/src/lib.rs:36-40, 425-429, 459-532, 1007-1038` (+ `ValidatedProbeOutput` gains `track: MediaTrackKind { Video, Audio }`)
- Test: `capsules/media-provider/tests/process.rs` (new audio argv test parallel to `:527-590`; fixture helper at `:385-395` already builds `soun`/`mp4a`), plus an `#[ignore]` real-ffmpeg test that runs only when `ffmpeg` and `ffprobe` are on PATH
- Modify: `protected_content_runtime.rs:293-294` → `const MEDIA_PROVIDER_OUTPUT_PAIRS_V1: [(&str, &str); 2] = [("video/mp4", "avc1.640028"), ("audio/mp4", "mp4a.40.2")];`, `:3377-3382` membership, `:3777-3786` try each pair against `record.output_receipt_digest()`
- Modify: `capsules/library/browser/src/model.js` (`protectedContentKindFor` already treats `audio/*` as media from Task 14; nothing else), `capsules/elacity-player/browser/{player.js:236,295,350,354,357,384; index.html:15,17; style.css}` ("video" → "media" copy; `.audio-only` class collapsing the frame to controls height when the mime starts with `audio/`), `scripts/elacity-player-smoke.mjs:509` and the 13 `video/mp4` sites (add an `audio/mp4; codecs="mp4a.40.2"` case)
- Test: `protected_content_runtime/tests.rs` (resume path with an audio receipt), `gateway_tests/library.rs` (protect-and-list an `.mp3` through the process media provider stub returning the audio pair)

**Interfaces:**
- Produces: media-provider output `{ schema, mime_type: "audio/mp4", codecs: "mp4a.40.2" }` for audio-only inputs; ffmpeg argv for audio: identical to video except the codec block becomes `-map 0:a:0 -vn -c:a aac -b:a 128k -ar 48000 -ac 2` and all `-movflags +frag_keyframe+empty_moov+default_base_moof+separate_moof -f dash -seg_duration 4 -streaming 0 -use_timeline 0 -use_template 1 -init_seg_name init.mp4 -media_seg_name segments/$Number%08d$.m4s` flags are preserved verbatim.

- [ ] **Step 1: Failing media-provider tests:** `prepare_audio_only_source_emits_aac_dash_rendition` (probe JSON with only a `codec_type: "audio"` stream; assert response pair and the exact argv list), `probe_with_neither_video_nor_audio_is_rejected`, and the ignored `real_ffmpeg_audio_only_dash_layout_validates` (generate a 5 s sine with `ffmpeg -f lavfi -i sine=frequency=440:duration=5`, run the provider's argv, assert `init.mp4` + `segments/*.m4s` pass `ValidatedClearFmp4MediaSessionLayoutV1::new` + `validate_segment` like `:601-606`).
- [ ] **Step 2:** `(cd capsules/media-provider && cargo test)` → FAIL → implement the branch → PASS. Then `(cd capsules/media-provider && cargo test -- --ignored real_ffmpeg_audio_only_dash_layout_validates)` on this machine; paste the result in the PR evidence. If the real muxer rejects the flag set, adjust the audio argv (never the video one) and re-pin the argv test.
- [ ] **Step 3: Failing runtime tests:** `media_preparation_accepts_audio_pair`, `media_preparation_rejects_unknown_pair`, `media_preparation_resume_matches_audio_receipt` (record with an audio receipt digest resumes as `Prepared` with `audio/mp4`).
- [ ] **Step 4:** Run → FAIL → implement allowlist + resume loop → PASS; `just test-crate elastos-server`.
- [ ] **Step 5: Player + smoke.** Copy edits and `.audio-only` class; `node scripts/elacity-player-smoke.mjs` green with the added audio case.
- [ ] **Step 6: Installed proof.** Install media-provider, restart, protect an `.mp3` through Creator, buy, open → player plays audio with controls. Record evidence. `scripts/installed-provider-verify.sh media-provider`.
- [ ] **Step 7: Commit.** `git commit -am "feat(media): audio files protect through an AAC fMP4 rendition and play in Elacity Player"`

---

### Task 17: #48 — crypto review package (docs + golden-vector generator)

Facts (research 2026-09-08): no golden vector files exist (10 hex literals in tests, e.g. `contracts/src/authority_tests.rs:722-794`, `custody/src/pq_hybrid.rs:301-320`); `hpke` is a dependency in name only (`hpke::rand_core`); the real suite is X-Wing draft-06 (ML-KEM-768 ‖ X25519) → HKDF-SHA256 with no salt (`pq_hybrid.rs:101-116`) → AES-256-GCM with random nonce; shares are `vsss-rs 6.0.1` `Gf256` Shamir 2-of-3 (`provision.rs:208-222`, `reconstruct.rs:252`); EIP-191 recovery with low-S check (`contracts/src/rights.rs:416-445`, hash in `elastos-auth/src/lib.rs:1058-1064`); "External cryptographic review remains open" at `state.md:332-334`, `docs/PROTECTED_CONTENT.md:150-153`, `docs/PROTECTED_CONTENT_CONTRACTS_V1.md:298-300`; the "threat model" at `PROTECTED_CONTENT_CONTRACTS_V1.md:319-333` is a test list.

**Files:**
- Create: `docs/PROTECTED_CONTENT_CRYPTO_REVIEW.md` with sections: (1) Scope and corrections (what #48 asked vs what the code does, including the `hpke` naming correction and the EPC1 object container now in use); (2) Suite card (each primitive, crate + version from `Cargo.lock`, parameters, where in code, what binds what: session binding v3 preimage fields, chunk AAD, receipt digest); (3) Threat model (assets, principals, trust boundaries: Runtime ↔ sandboxed providers ↔ custody nodes ↔ viewer capsule; what a compromised viewer capsule, a compromised single custody node, a replayed release operation, a tampered staged file each can and cannot do, with the test that pins each claim); (4) Build card (toolchain, `cargo tree` for the crypto crates, reproducible build commands); (5) Golden vectors (how to regenerate, how the replay test uses them); (6) Open items for the external reviewer.
- Create: `elastos/crates/elastos-protected-content-custody/examples/golden_vectors.rs` — writes JSON `{ "schema": "elastos.protected-content.golden-vectors/v1", "generated_by": "<crate> <version>", "vectors": { "hkdf_sha256_no_salt": [{ikm, info, okm}], "aes_256_gcm_chunk": [{key, header_hex, chunk_index, plaintext, framed_chunk}], "shamir_gf256_2_of_3": [{secret, shares[3], reconstructed_from: [[0,1],[0,2],[1,2]]}], "xwing_decaps": [{dk, ek, ct, ss}], "eip191_recover": [{message, signature, address}] } }` using the crate's own public functions (no re-implementation; for X-Wing record `dk/ct/ss` from one encapsulation so replay only needs decapsulation; for Shamir use the crate's split with a seeded `ChaCha20Rng` and record the resulting shares).
- Create: `docs/audits/2026-09-protected-content-golden-vectors-v1.json` (generated once, committed).
- Create: `elastos/crates/elastos-protected-content-custody/tests/golden_vectors.rs` — replays every vector through the public API and asserts equality; fails if the JSON schema string changes.
- Modify: `state.md:332-334`, `docs/PROTECTED_CONTENT.md:150-153`, `docs/PROTECTED_CONTENT_CONTRACTS_V1.md:298-300, 319-333` — point to the new doc; keep "external review remains open".

- [ ] **Step 1:** Write `tests/golden_vectors.rs` against the JSON path first (FAIL: file missing).
- [ ] **Step 2:** Write the example; `cargo run -p elastos-protected-content-custody --example golden_vectors > docs/audits/2026-09-protected-content-golden-vectors-v1.json`.
- [ ] **Step 3:** `just test-crate elastos-protected-content-custody -- --test golden_vectors` PASS.
- [ ] **Step 4:** Write the doc; run `node scripts/home-entropy-check.mjs` (`docs/audits/` is exempt from the prose lint, `docs/*.md` is not: no branch names or commit hashes in the standing doc).
- [ ] **Step 5: Commit.** `git commit -am "docs(protected-content): crypto review package with suite card, threat model and golden vectors"`

---

### Task 18: Docs alignment, full gate, PR

**Files:**
- Modify: `docs/PROTECTED_CONTENT.md` (Creator, reader, object path, audio, viewer admission by executable actor), `TASKS.md` (mark #42/#48/#49 items), `state.md` ("Last updated: 2026-09-XX UTC"; record installed proof evidence and artifact hashes), `scripts/README.md` (new smoke), `.superpowers/memory/protected-content-0.7.1-followup-decisions.md` (record the naming decisions and anything that changed during execution).

- [ ] **Step 1:** `just verify` and `just verify-ci` green (third checkpoint). `git diff --check` clean.
- [ ] **Step 2:** Re-run the full installed proof list (Task 7 Step 8, Task 15 Step 9, Task 16 Step 6) once more on the final artifacts and record hashes in `state.md`.
- [ ] **Step 3:** Commit docs: `git commit -am "docs(protected-content): align standing docs and state with the 0.7.1 follow-up"`.
- [ ] **Step 4:** Ask the user for push approval; then open the PR with base `feat/protected-content-atomic-cutover`, title "feat(protected-content): 0.7.1 UI/UX, cleanup and crypto-review follow-up (#42, #48, #49)", body listing each issue's items with the pinning test names and the evidence block; the user stacks it in the GitHub UI (stack #61).

---

## Self-review notes

- **Spec coverage:** #42 gap 3 → Task 1; gap 2 → Task 7; gap 1 → Tasks 8–15 (objects) + 16 (audio) + 14 (Library). #49 items 2, 5, 3, 4, 1 → Tasks 2–6 in the ruled order. #48 → Task 17. Docs → Task 18. Render-IR (server-rendered pages) is explicitly out of scope per D2 and recorded in the memo.
- **Type consistency:** `ChunkedPayloadObjectIdentityV1` (Task 8) is what Tasks 10–13 store and send; `RuntimeContentIdentityV1::{Media, Object}` (Task 12) is what Task 13 writes to the journal; `PayloadSealerV1` / `PayloadChunkDecrypterV1` / `framed_chunk_ranges_v1` (Task 9) are used by Tasks 10, 11, 13; `ELACITY_READER_CAPSULE_ID` / `CREATOR_CAPSULE_ID` (Tasks 13, 7) match the capsule `name` fields; `content_kind` values `"media"` / `"object"` are identical in Task 13 (server), Task 14 (Library via `protectedContentKindFor`) and Task 15 (reader).
- **Known risks called out:** the real-ffmpeg audio flag set is unverified in-repo (Task 16 Step 2 runs it live before the argv pin is final); the `v5 → v6` journal codec must keep media mint ids byte-stable (Task 12 Step 1c pins the literal before any change); `set(config) == MEDIA_FIELDS` forbids any media-provider config change; vendoring pdf.js adds ~1 MB of governed JS (precedent: katex 600 KB, three.js 1.27 MB on the dkms branch).
- **Optional filler if the executor is blocked waiting on review:** ELACITY-2294 slice 1 (`capsules/ipfs-provider/src/main.rs:674, 998-1019` `_files.json` sidecar and the `~712-725` containment check using the `validate_dest_path` recipe) is independent and small; it is not part of this PR's scope unless the user says so.
