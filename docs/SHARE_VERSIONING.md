# Share Versioning Spec (v0.3)

## Purpose

Define how ElastOS manages shared data capsules over time:

- Immutable content versions (CID-addressed capsules)
- Mutable channel pointers (latest, status, history)
- Viewer UX for navigating old/new versions
- Archive/revoke/delete-local behavior

This enables repeatable publishing while preserving content-addressed integrity.

## Goals

- Keep every published version immutable (`elastos://<cid>` never changes)
- Make "what is latest?" easy for humans and tooling
- Allow old versions to discover newer versions without mutating old content
- Support safe lifecycle states: active, archived, revoked
- Make local cleanup possible without pretending global deletion exists on IPFS

## Non-Goals (for v0.3)

- Global hard delete from all IPFS peers
- Multi-writer conflict resolution across unrelated publishers
- Complex semantic merge/diff UI

## Core Model

Use two layers:

1. Immutable version capsules (CID)
2. Mutable channel state (local catalog first, signed head later)

Each publish creates a new immutable capsule with metadata that links backward.
Forward links are derived from mutable channel state.

## Terms

- `share_id`: stable logical identifier for a share channel. Phase 1: plain channel name (e.g. `architecture`). Phase 2+: DID-scoped (e.g. `did:key:z6.../architecture`)
- `version`: sequential integer (1, 2, 3...) auto-incremented per channel
- `version_cid`: CID of one immutable shared capsule
- `head`: mutable record pointing to current latest CID
- `history`: append-only list of version entries for a share channel (local catalog in Phase 1)
- `status`: typed enum — one of `active`, `archived`, `revoked` (enforced via `ChannelStatus` in Rust)
- `content_digest`: deterministic SHA-256 hash of sorted `(relative_path, file_hash)` pairs — same content produces same digest across versions

## Data Artifacts

### 1) Immutable capsule metadata (`_share.json`)

Stored inside each shared capsule directory. Written before IPFS publish so it becomes part of the CID.

```json
{
  "schema": "elastos.share.meta/v1",
  "share_id": "architecture",
  "version": 3,
  "prev": "bafy...previous-cid",
  "created_at": 1740700800,
  "content_digest": "sha256:abc123...",
  "author_did": "did:key:z6Mk..."
}
```

Rules:

- `prev` is null for first version
- `created_at` is Unix timestamp (seconds since epoch)
- File is immutable once published (part of CID)
- `version` is a sequential integer, auto-incremented from local catalog
- `content_digest` is computed from content files only (excludes viewer, capsule.json, _share.json, _files.json)
- `author_did` is optional; populated from catalog's default DID or channel-level override

**Not included** (and why):
- `content_root`: the CID of this capsule is unknown before publish (circular). Stored post-publish in catalog only.
- `channel_ref`: unresolvable at Tier 0. Deferred to Phase 3 when head/history publication exists.

### 2) Local share catalog (`catalog.json`)

Stored at `~/.local/share/elastos/shares/catalog.json`.

```json
{
  "schema": "elastos.share.catalog/v1",
  "author_did": "did:key:z6Mk...",
  "channels": {
    "docs": {
      "latest_cid": "bafy...latest",
      "latest_version": 3,
      "updated_at": 1740700800,
      "status": "active",
      "revoke_reason": null,
      "author_did": null,
      "head_cid": "Qm...head3",
      "history": [
        { "cid": "bafy...v1", "version": 1, "created_at": 1740614400, "content_digest": "sha256:..." },
        { "cid": "bafy...v2", "version": 2, "created_at": 1740657600, "content_digest": "sha256:...", "provenance_cid": "Qm...prov2" },
        { "cid": "bafy...v3", "version": 3, "created_at": 1740700800, "content_digest": "sha256:...", "provenance_cid": "Qm...prov3" }
      ]
    }
  }
}
```

**Typed `ChannelStatus` enum** (Rust):
```rust
#[serde(rename_all = "lowercase")]
enum ChannelStatus { Active, Archived, Revoked }
```

All struct fields use `#[serde(default)]` for backward-compatible deserialization. Legacy channels (where `latest_version == 0` but `latest_cid` is non-empty) are automatically backfilled to `latest_version = 1` with a synthetic history entry on load.

Catalog writes use atomic tmp-then-rename to prevent corruption on crash or power loss (with Windows backup fallback).

### 3) Provenance attestation (`provenance.json`, Phase 2.5)

Published as a detached IPFS sidecar (separate CID). Cannot be embedded in the capsule (adding `output_cid` would change the CID — circular hash).

```json
{
  "schema": "elastos.share.provenance/v1",
  "subject_cid": "bafy...capsule-cid",
  "content_digest": "sha256:abc123...",
  "builder_did": "did:key:z6Mk...",
  "built_at": 1740700800,
  "tool_version": "elastos 0.1.0",
  "signature": "hex-encoded-ed25519-signature"
}
```

Signing uses domain-separated deterministic bytes: `SHA256(b"elastos.provenance.v1\0" || canonical_json_bytes)`. This prevents cross-protocol signature reuse. CID comparison is semantic (parsed via `cid::Cid`) to handle CIDv0/v1 representation differences.

The `provenance_cid` is stored in each `ShareEntry` in the local catalog for lookup during `elastos verify --cid`.

### 4) Signed channel head (`head.json`, Phase 3)

Published to IPFS as a mutable pointer (new head replaces old per channel). Represents the channel owner's signed assertion of current channel state.

```json
{
  "schema": "elastos.share.head/v1",
  "channel": "docs",
  "latest_cid": "bafy...latest",
  "latest_version": 3,
  "status": "active",
  "updated_at": 1740700800,
  "signer_did": "did:key:z6Mk...",
  "provenance_cid": "Qm...prov3",
  "prev_head_cid": "Qm...prev-head",
  "revoke_reason": null,
  "signature": "hex-encoded-ed25519-signature"
}
```

Signing uses domain-separated deterministic bytes: `SHA256(b"elastos.channel.head.v1\0" || canonical_json_bytes)`. Distinct domain separator prevents cross-protocol signature reuse with provenance attestations.

Fields:
- `prev_head_cid`: references the previous published head CID for this channel (head-chain continuity). None/missing means genesis head.
- `provenance_cid`, `prev_head_cid`, `revoke_reason`: omitted from JSON when None (serde `skip_serializing_if`).
- `status`: same `ChannelStatus` enum as catalog (`active`, `archived`, `revoked`).
- `revoke_reason`: populated only when status is `revoked`.

Verification (`verify_channel_head`): validates signature, schema, and embedded CID formats (`latest_cid`, `provenance_cid`, `prev_head_cid`). Trust (signer_did vs expected DID) is the caller's responsibility.

The `head_cid` is stored in `ShareChannel` in the local catalog for lookup during `elastos open` and `elastos shares head`.

### 5) Revocation tombstone (`tombstone.json`, Phase 4+)

```json
{
  "schema": "elastos.share.tombstone/v1",
  "share_id": "architecture",
  "status": "revoked",
  "reason": "sensitive content",
  "revoked_at": 1740700800,
  "revoked_by": "did:key:z6Mk...",
  "signature": "base64-ed25519-signature"
}
```

## Linking Strategy

Backward linking (immutable):

- Every new version sets `_share.json.prev` to prior CID

Forward linking (mutable):

- `elastos open` checks local catalog and prints "Newer version available" hint to stderr
- `elastos open` fetches signed channel head from IPFS when available — signed+trusted warnings supersede unsigned catalog warnings
- Old version can link to latest through head/history, without rewriting old capsule
- Phase 4: viewer resolves network-published signed heads and shows "Newer version available" banner

This gives bidirectional navigation while preserving immutability.

## Viewer Behavior Contract

Input:

- Current `version_cid`
- Optional channel reference (query parameter or `_share.json` metadata)

Flow:

1. Load `_share.json` from current capsule (silently ignore absence for backward compat)
2. Show metadata badge: `share_id` (bold), `version` (blue tag), `created_at`, optional truncated `author_did`
3. If `prev` exists, show link derived from current origin:
   - Path-based gateway (`/ipfs/<cid>/`): replace CID in path
   - Subdomain gateway (`<cid>.ipfs.dweb.link`): replace CID in hostname
   - Local/unknown: omit prev link (no hardcoded gateway URLs)
4. In multi-doc mode: badge appears in sidebar below title
5. In single-doc mode: badge appears above content

**Deferred** (Phase 4):
- "Newer version available" banner — requires viewer network fetch of signed heads
- Archive/revoke banners in viewer — `elastos open` prints CLI warnings (signed or unsigned) instead

## CLI Contract

Publishing:

```bash
elastos share <path>                       # auto-channel from path name
elastos share <path> --channel <name>      # explicit channel name
```

Opening shared content:

```bash
elastos open <uri>                         # serve locally (no auto-open)
elastos open <uri> --browser               # serve and open browser
elastos open <uri> --port 4000             # serve on specific port
# <uri> can be: elastos://<cid>, bare CID, https://ipfs.io/ipfs/<cid>/,
#               https://<cid>.ipfs.dweb.link/
```

`elastos open` behavior:
- Parses URI via `parse_share_uri` (supports elastos://, bare CID, path gateways, subdomain gateways)
- CID validation via `cid` crate (rejects invalid CIDs)
- Checks local catalog: prints revoked WARNING, archived Note, or "newer version available" hint
- Finds free ephemeral port by default (bind to `127.0.0.1:0`)
- No auto-browser by default (safe for headless/CI); `--browser` to open

Provenance (Phase 2.5):

```bash
elastos share <path>                       # auto-attests (creates provenance sidecar)
elastos share <path> --no-attest           # skip provenance attestation
elastos share <path> --no-head             # skip signed channel head publication
elastos attest <cid>                       # manual attestation for existing CID
elastos attest <cid> --key <keyfile>       # attest with specific key
elastos attest <cid> --content-digest <d>  # override digest (skip _share.json fetch)
elastos verify --cid <cid>                 # verify provenance (from local catalog)
elastos verify --cid <cid> --provenance <p> # verify with explicit provenance CID
```

Channel management:

```bash
elastos shares list                        # list channels with status, DID, [attested], [head]
elastos shares history <channel>           # show version history with provenance CIDs
elastos shares head <channel>              # fetch and verify signed channel head from IPFS
elastos shares delete-local <channel>      # remove channel from local catalog
elastos shares archive <channel>           # mark as archived + publish signed head
elastos shares unarchive <channel>         # restore to active + publish signed head
elastos shares revoke <channel> --reason "..." # mark as revoked + publish signed head
elastos shares set-did <did>               # set default author DID (must start with did:key:)
```

Behavior notes:

- `share` updates head/history in local catalog (best-effort; no file locking). Publishes signed channel head to IPFS (opt-out: `--no-head`). Provenance and head are independent (`--no-attest` disables provenance only, `--no-head` disables head only).
- `delete-local` removes local catalog entry only; published content remains on IPFS
- `archive` sets status to `archived`; guards against double-archive; publishes signed head
- `unarchive` restores to `active`; guards against unarchiving non-archived; publishes signed head
- `revoke` sets status to `revoked` with reason; publishes signed head with `revoke_reason`; published content remains on IPFS
- `set-did` validates `did:key:` prefix, stores as catalog-level default
- `head` fetches `head_cid` from catalog, downloads from IPFS, verifies signature, displays all fields

## Archive and Delete Semantics

`archive`:

- Keep all CIDs valid
- Status visible in `shares list`
- `elastos open` prints "Note: archived" to stderr

`revoke`:

- Keep all CIDs valid (immutability)
- `elastos open` prints WARNING to stderr
- Viewer does NOT show banners (local-only state until Phase 3 network-published heads)

`delete-local`:

- Remove local catalog entry
- Published content remains on IPFS (no unpin in Phase 2)
- Allow local GC

No "global delete":

- Once a CID is replicated, universal deletion is not guaranteed
- Sensitive shares should use encrypted payloads and key revocation strategy

## Trust and Verification

Phase 2 (implemented):

- `author_did` in `_share.json` (populated from catalog, not signed)
- `content_digest` for deterministic content identity
- Typed `ChannelStatus` enum prevents invalid state drift
- `set-did` command for catalog-level default DID

Phase 2.5 (implemented):

- Detached provenance attestations (`provenance.json` as sidecar CID)
- Ed25519-signed with domain-separated deterministic payload (`b"elastos.provenance.v1\0"` prefix)
- `did:key` encode/decode (Ed25519 multicodec prefix)
- Share signing key auto-generation at `~/.local/share/elastos/shares/signing.key` (0600 perms)
- `elastos share` auto-attests (opt-out: `--no-attest`)
- `elastos attest <cid>` for manual attestation
- `elastos verify --cid <cid>` for provenance verification (semantic CID comparison)
- `provenance_cid` tracked in catalog `ShareEntry`
- `[attested]` indicator in `shares list`

Phase 3 (implemented):

- Signed channel heads (`head.json` published to IPFS per channel)
- Domain-separated Ed25519 signing (`b"elastos.channel.head.v1\0"` prefix, distinct from provenance)
- `elastos share` auto-publishes signed head (opt-out: `--no-head`, independent of `--no-attest`)
- Lifecycle commands (`archive`, `unarchive`, `revoke`) publish signed heads with status transitions
- `elastos shares head <channel>` fetches and verifies signed head from IPFS
- `elastos open` prefers signed+trusted head over unsigned catalog warnings; trust check compares `signer_did` to expected DID
- `head_cid` tracked in `ShareChannel` (migration-safe with `#[serde(default)]`)
- `prev_head_cid` for head-chain continuity (genesis head has None)
- `[head]` indicator in `shares list`
- CID validation on embedded `latest_cid`, `provenance_cid`, `prev_head_cid` in `verify_channel_head`
- 13 new unit tests (signing bytes, create/verify roundtrip, tamper detection, wrong signer, revoked/archived status, schema rejection, domain separator, migration, optional fields, prev head chain, CID validation)

Phase 4+:

- IPNS publication for head discovery without catalog
- "Newer version available" banner in viewer
- Archive/revoke banners in viewer
- Sign immutable `_share.json` digest and include detached proof
- Multiple trusted signers policy per channel

## Storage Layout (Local Node)

Local share catalog:

- `~/.local/share/elastos/shares/catalog.json`
- Keyed by channel name
- Stores latest head, history entries, status, author_did, and local metadata

## Migration Plan

Phase 1 (implemented):

- `_share.json` generation in `elastos share`
- Local share catalog with `elastos shares list/history`
- Sequential integer versions, Unix timestamps
- Channel name derived from path or `--channel` flag

Phase 2 (implemented):

- Typed `ChannelStatus` enum (`active`, `archived`, `revoked`) with `#[serde(default)]` migration safety
- `content_digest` in `_share.json` and `ShareEntry` (deterministic sorted-file-tree SHA-256)
- `author_did` in `_share.json` and catalog (catalog-level default via `set-did`)
- `elastos open <uri>` command (URI parsing, ephemeral port, catalog status warnings)
- Lifecycle commands: `delete-local`, `archive`, `unarchive`, `revoke`
- Updated `shares list` with STATUS column and truncated DID
- md-viewer version badge (channel name, version tag, date, optional DID, prev link)
- Prev link derivation from current origin (path + subdomain gateways, no hardcoded URLs)
- Atomic catalog writes (tmp-then-rename, Windows backup fallback)
- Legacy channel backfill on load (pre-versioning entries)
- Migration tests for old catalog shapes and URI parser

Phase 2.5 (implemented):

- Detached provenance attestations (`provenance.json` as sidecar CID)
- Domain-separated Ed25519 signing (prevents cross-protocol signature reuse)
- `did:key` encode/decode with Ed25519 multicodec prefix
- Share signing key auto-management (`signing.key` with 0600 permissions)
- `elastos share` auto-attests; `--no-attest` to skip
- `elastos attest <cid>` for manual attestation of existing CIDs
- `elastos verify --cid <cid>` with catalog lookup or `--provenance` override
- `provenance_cid` in catalog `ShareEntry` (migration-safe with `#[serde(default)]`)
- `[attested]` indicator in `shares list`, truncated provenance CID in `shares history`
- IPFS single-file publish and fetch helpers
- 11 new unit tests (DID roundtrip, provenance sign/verify, tamper detection, key persistence, domain separator)

Phase 3 (implemented):

- Signed channel heads (`head.json`) published to IPFS per channel
- Domain-separated Ed25519 signing (distinct from provenance domain)
- `head_cid` in `ShareChannel` (migration-safe with `#[serde(default)]`)
- `prev_head_cid` for head-chain continuity (enables rollback detection)
- `elastos share` auto-publishes head; `--no-head` to skip (independent of `--no-attest`)
- Lifecycle commands (`archive`, `unarchive`, `revoke`) publish signed heads
- `elastos shares head <channel>` for fetch + verify
- `elastos open` signed head verification with trust check (signed-preferred, unsigned-fallback)
- `[head]` indicator in `shares list`
- Embedded CID validation in `verify_channel_head`
- 13 new unit tests

Phase 4:

- Replication profiles and retention policies
- Automated e2e tests for share lifecycle

## Open Questions

- Canonical mutable channel transport: local-only first, then IPNS, DID doc, or gateway index?
- How should multi-author channels handle concurrent head updates?
- Should revoked content be blocked by default or just warned?
