# Phase 3A — `object-provider Acquire` (buy → pin-to-Library) — ✅ BUILT (additive)

> The buy→pin seam: on a confirmed buy, pin the bought **encrypted** IPFS asset into the buyer's local
> Library and register it as a `LibraryObject`, so the existing player opens it.
>
> **STATUS: built (commit `feat: object-provider Acquire op`), compile + unit-tested.** Implemented with
> the **lower-risk ADDITIVE design** (§ below was the original spec): `library_acquire` = fetch keylessly
> via `content/*` → `content/ensure` pin → `write_library_file_bytes` under the buyer root — with **NO**
> change to `record_is_published` / the publish path (the acquired asset is a normal `published=false`
> Library file; availability comes from the ensure receipt, best-effort). Dispatched in both
> registry-bearing sites; rejected on the keyless path (P11); `capsule.json` allow-lists `acquire`.
> Holds no keys, never decrypts (P4/P15/P16). **Entitlement is gated UPSTREAM by the marketplace/buy api
> caller** (the cleaner layering — the object-provider pins what it is told, like `Publish`). Live
> `fetch`/`ensure` + end-to-end open = Cursor. The original turnkey spec (record + gate-change variant)
> is kept below for reference; the additive build supersedes it.

## 1. The op (enum variant) — `library.rs` after `Publish` (~:295)
`ObjectProviderRequest` is `#[serde(tag="op", rename_all="snake_case", deny_unknown_fields)]` (so `op:"acquire"`; unknown keys fail closed). Add:
```rust
Acquire {
    principal_id: String,
    content_cid: String,
    #[serde(default)] uri: Option<String>,     // optional destination override under the buyer root
    #[serde(default)] metadata: Option<Value>, // {name?, mime?} from the buy step
},
```

## 2. `library_acquire(...)` — clone `library_publish` (`library.rs:1751`)
`async fn library_acquire(data_dir, registry: Arc<ProviderRegistry>, principal_id, content_cid, uri, metadata) -> anyhow::Result<Value>`:
1. **Derive destination under the buyer root only** (P16): `root = crate::auth::principal_localhost_root(principal_id)` (`auth.rs:1175`); `name` = `metadata.name` else last segment of `uri` else `"{content_cid}.bin"`; `dest = uri.unwrap_or(format!("{root}/Acquired/{name}"))`. Resolve via `library_target(data_dir, principal_id, &dest)` (`library.rs:2999`) — resolves ONLY under this principal's root, so a buyer can never pin into another's space.
2. **CAS guard:** `check_revision(data_dir, principal_id, &target.uri, None)?` (new object).
3. **Pin via `content/*` ensure** (P4 — never raw ipfs; the `pin_call` below): `registry.send_raw("content", {op:"ensure", cid, object_did: target.uri, publisher_did: principal_id})`; fail closed if `status=="error"` or `availability.status != "local_pinned"` (write NOTHING).
4. **Fetch the (encrypted) bytes keylessly** (`content.rs:2734`): `registry.send_raw("content", {op:"fetch", cid})` → b64-decode `data.data`. Bytes stay opaque ciphertext (P15). *Order: for a cold node, `fetch` (pulls+caches) may need to precede `ensure` (pins); re-check `local_pinned`.*
5. **Materialize under the buyer root:** `let object = write_library_file_bytes(data_dir, principal_id, &target.uri, mime.as_deref(), None, &bytes)?` (`library.rs:3634` → `crate::auth::write_principal_root_object`, encrypt-at-rest if the root is protected).
6. **Record + the gate change (see §3).**
7. `append_library_event(data_dir, principal_id, "acquire", &target.uri, json!({content_cid, availability, object}))?` (`library.rs:6888`).
8. Re-derive `library_object(data_dir, principal_id, &target.uri)?` (`library.rs:3311`) and return §4.

## 3. The derived-view + the surgical gate change
`LibraryObject` is **never** constructed directly — it's derived from on-disk bytes + a `LibraryPublishRecord` sidecar. Write an **acquire-record** (distinct schema) so the object shows `availability="local_pinned"` + `content_cid`, but `published=false`:
```rust
let record = LibraryPublishRecord {
    schema: "elastos.library.acquire-record/v1".into(),  // marks acquired, not published
    object_uri: target.uri.clone(),
    cid: content_cid.into(),                              // surfaces as published_cid
    published_at: now_ts(), unpublished_at: None, shared_at: None,
    share_policy: None, share_grants: Vec::new(),
    content_security: default_publish_content_security(), // library.rs:5689
    receipt, availability,                                // from ensure (status:"local_pinned")
};
write_publish_record(data_dir, principal_id, &record)?;   // library.rs:6838
```
**Gate change** (`record_is_published`, `library.rs:5679`) — required so `local_pinned` coexists with `published=false`, and so acquired assets do NOT expose publish/unpublish/share/repair caps:
```rust
fn record_is_published(record: &LibraryPublishRecord) -> bool {
    if record.schema == "elastos.library.acquire-record/v1" { return false; } // <-- add
    record.unpublished_at.is_none()
        && record.availability.get("status").and_then(Value::as_str).map(|s| s != "local_unpinned").unwrap_or(true)
}
```
*Regression guard (unit test): a normal publish-record with `status="local_pinned"` STILL returns true; only the acquire-record returns false.*

## 4. Response (same envelope as `library_publish`, via `provider_ok`)
```rust
Ok(json!({ "object": object, "uri": object.uri, "content_cid": content_cid,
           "availability": record.availability, "receipt": record.receipt }))
```
The load-bearing return is the **Library uri** under the buyer root (e.g. `localhost://Users/<hex>/Acquired/<name>`) — the openable path the player resolves via `read_owned_object_for_viewer` (`library.rs:3595`), still ciphertext until the DRM/key providers run at open.

## 5. Dispatch (3 sites) + capsule manifest
- `ObjectProvider::send_raw` arm after `Publish` (`library.rs:360-381`): `let Some(registry)=self.registry.upgrade() else {…provider_error…}` then `library_acquire(...).await`.
- `handle_object_provider_runtime_request` arm after `Publish` (`library.rs:625-641`): registry is `Arc`; call directly.
- `handle_object_provider_raw_request` (`library.rs:449-453`): add `| ObjectProviderRequest::Acquire { .. }` to the rejected set — the registry-less stdio capsule **fails closed** ("requires Runtime content coordinator"). The buy flow must target the in-process `send_raw`/runtime path (which has the registry), not the standalone capsule.
- `capsules/object-provider/capsule.json` (`:17`): add `"acquire"` to the operations allow-list.

## 6. Pure-unit-testable here (Cursor or a later pass)
serde of `op:"acquire"` (+ `deny_unknown_fields` rejects junk); destination-URI derivation stays under the buyer root; the `record_is_published` gate change (acquire-record→false, publish-record→true); object derivation after a hand-written acquire-record + temp file (`availability=="local_pinned"`, `published==false`, `content_cid` set, no publish caps); keyless fail-closed reject in `handle_object_provider_raw_request`; `write_library_file_bytes` stores ciphertext verbatim (round-trips, no decrypt).

## 7. Live (Cursor — needs a running registry + IPFS)
The `ensure` pin + `fetch` (pull/cache the encrypted block); end-to-end Acquire → object opens in the existing player via `read_owned_object_for_viewer` with **no** key release; integrity (fetched bytes hash to `content_cid`); directory CIDs (DASH media — `library_publish` is file-only today; extend with `add_directory`/`download_directory`).

## 8. Hard prerequisite (security)
**No entitlement check exists on any object-provider path.** The marketplace/settlement plane MUST verify `principal_id` actually bought `content_cid` (the on-chain `hasAccessByContentId` / a confirmed `buyAccess`) **before** dispatching `Acquire`, or add that gate inside `library_acquire`. Pinning encrypted bytes grants no decryption (keys gated at open), so the blast radius is wasted storage, not disclosure — but the gate belongs upstream regardless (P11).
