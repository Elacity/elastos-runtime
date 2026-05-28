# Phase 6 Day 2 — `components.json` schema edits (host-binary lane)

**Phase**: 6 (macOS native-binary surface)
**Day**: 2
**Date**: 2026-05-25
**Status**: Complete; Day-3 unblocked
**Predecessor**: [`PHASE_6_COMPONENTS_AUDIT.md`](./PHASE_6_COMPONENTS_AUDIT.md) (Day 1 audit)
**Successor**: Day 3 — Class-B `chat` capsule darwin-arm64 entry

---

## 1. Scope deviation from plan

**None substantive.** The Day-2 prompt prescribed Class-A, Class-D
(description-only), Class-E, and capsules-projection edits; this commit
ships exactly that and nothing more. Two minor framings updated as the
work landed:

- **Artifact storage.** The prompt's gate-4 wording said capture the
  dry-run output to `docs/vz-backend/artifacts/PHASE_6_DAY_2_carrier_setup_dry_run.txt`.
  The repo-level `.gitignore` ignores `artifacts/` (build-artifact
  convention). To keep the audit trail durable, the Mac-pre-flight
  evidence is embedded directly in this notes file (§ 4) instead of in
  a gitignored sibling.
- **"Mac pre-flight: PASS" wording.** The prompt suggested the smoke
  would now print `"[local-carrier-setup] Mac pre-flight: PASS"`. The
  smokes don't actually emit that string today — only the SKIP variant
  exists (when the helper fails); on the success path the smoke falls
  through silently to its work stage. The truthful Day-2 success
  signal is **the helper returning 0 when given each smoke's literal
  argument list**, verified in § 4 below. Modifying the smokes to
  print an explicit PASS line is out-of-scope (it would change the
  Linux byte-identical contract that Phase 5 spent eight days
  establishing).

---

## 2. Concrete changes

Two substrate files touched, plus this notes file + a plan banner.

### 2.1 `components.json`

26 entries in `external/`; 11 entries in `capsules/`. Day-2 edits:

| Class | Entry | Edit |
|---|---|---|
| **A** | `external.shell` | Added `platforms.darwin-arm64` (empty `cid`/`checksum`, `release_path` = `shell-darwin-arm64`) |
| **A** | `external.localhost-provider` | Added `platforms.darwin-arm64` |
| **A** | `external.did-provider` | Added `platforms.darwin-arm64` |
| **A** | `external.webspace-provider` | Added `platforms.darwin-arm64` |
| **A** | `external.ipfs-provider` | Added `platforms.darwin-arm64` |
| **A** | `external.tunnel-provider` | Added `platforms.darwin-arm64` |
| **A** | `external.site-provider` | Added `platforms.darwin-arm64` |
| **D** | `external.crosvm.description` | Appended Mac paragraph: *"Linux-only; Apple Vz substrate replaces crosvm on darwin (see docs/vz-backend/PLAN.md)."* No `platforms.darwin-arm64` added (per Decision B — clean install-loop skip suffices). |
| **E** | `external.kubo` | Added `platforms.darwin-arm64` with real upstream `url` + `sha512` |
| **E** | `external.cloudflared` | Added `platforms.darwin-arm64` with real upstream `url` + `sha256` + `extract_path: "cloudflared"` (darwin asset is a `.tgz`, unlike the linux bare-binary URL) |
| **E** | `external.llama-server` | Added `platforms.darwin-arm64` with real upstream `url` + `sha256` |
| **F** | `capsules.shell` … `capsules.agent` (10 entries) | Appended `"aarch64-darwin"` to `platforms` array. Excludes `chat-wasm` (stays `["any"]`). |

Diff stats: `+88 / −11` (the 11 deletions are all benign syntactic
changes — trailing-comma additions to existing `aarch64-linux` lines as
the array gained a new last element; plus one crosvm description line
replaced with a longer version that preserves the original verbatim and
appends the Mac paragraph). See § 4 gate 5b for the line-by-line proof.

### 2.2 `scripts/lib/components-json-verify.sh` (new)

Single-source-of-truth verifier for the `components.json` darwin-arm64
schema invariants Phase-6 locks in. Re-runnable from any Day-3+/Day-5
CI gate to detect drift. Checks:

1. JSON parses.
2. Class-A host binaries (7): linux-amd64 + linux-arm64 + darwin-arm64 all present.
3. Class-B (`chat`): baseline keys present; darwin-arm64 logged as Day-3 carry-forward (forward-compat).
4. Class-C (`vmlinux`): baseline keys present; darwin-arm64 logged as Day-4 carry-forward.
5. Class-D (`crosvm`): platforms == EXACTLY [linux-amd64, linux-arm64].
6. Class-E (3): linux-amd64 + linux-arm64 + darwin-arm64 all present with real `url` + `checksum`.
7. Capsules projection: every entry except `chat-wasm` includes `aarch64-darwin`.

The script is **forward-compatible by design** — Day-3 and Day-4 will
extend the green check to Classes B and C; today the verifier emits
non-fatal "carry-forward" notes for them. This is so the same script
remains the gating CI invariant across the entire Phase 6.

---

## 3. The 3 upstream checksums (audit-trail evidence)

Fetched 2026-05-25 against the live release assets. Stored verbatim in
`external.{kubo,cloudflared,llama-server}.platforms.darwin-arm64`:

| Entry | Asset URL | Algorithm | Checksum |
|---|---|---|---|
| `kubo` | `https://dist.ipfs.tech/kubo/v0.40.1/kubo_v0.40.1_darwin-arm64.tar.gz` | sha512 | `13b6d5dc04e661bfde6b8ba469bcf5b19d9d0062fe8ed50c7aadd8a078f500d0bceee4be7c9bfa476b48c4eb84c246ba083605ed1ed24d16b98e6cd0f09140bb` |
| `cloudflared` | `https://github.com/cloudflare/cloudflared/releases/download/2026.2.0/cloudflared-darwin-arm64.tgz` | sha256 | `ba99c6f87320236b9f842c3ba4b9526f687560125b7b43a581201579543ca4ff` |
| `llama-server` | `https://github.com/ggml-org/llama.cpp/releases/download/b8192/llama-b8192-bin-macos-arm64.tar.gz` | sha256 | `de94484e7f5a50b74123b042aabcaec70111a0a31284f9cd0078efdefb193037` |

**Asset sizes (bytes), for traceability:**

- kubo: 39,739,653
- cloudflared: 18,309,012
- llama-server: 30,714,490

**Tarball layouts (for `extract_path` selection):**

- kubo: top-level `kubo/ipfs` (matches linux-arm64 row exactly)
- cloudflared: top-level `cloudflared` binary (differs from linux row,
  which is a bare-binary download — hence `extract_path: "cloudflared"`
  is darwin-only)
- llama-server: top-level `llama-b8192/llama-server` (matches the
  linux-amd64 row exactly)

`llama-server.linux-arm64` retains its `strategy: "source-build"` row;
darwin-arm64 ingests the upstream binary instead. This is the only
Class-E asymmetry vs the linux side.

---

## 4. Quality gates (6 of 6 green)

### Gate 1 — `components.json` parses

```
$ python3 -c "import json; data=json.loads(open('components.json').read()); print('external:', len(data['external']), 'capsules:', len(data['capsules']))"
external: 26 capsules: 11
```

Both inventory counts match the Day-1 audit. ✅

### Gate 2 — `cross-platform-test.sh` all 47 assertions green

```
$ bash scripts/lib/cross-platform-test.sh
…
cross-platform.sh: 47 passed, 0 failed
```

✅ (Phase 5 baseline of 47 preserved — no regression.)

### Gate 3 — `components-json-verify.sh` green

```
$ bash scripts/lib/components-json-verify.sh
[components-json-verify] forward-compat notes (non-fatal):
  [Class B] external.chat.platforms.darwin-arm64 — Day 3 expected (carry-forward)
  [Class C] external.vmlinux.platforms.darwin-arm64 — Day 4 expected (carry-forward)
[components-json-verify] OK
  Class A (host binaries):    7/7 green
  Class B (microVM bundles):  baseline green (1 entries, darwin Day 3)
  Class C (kernel):           baseline green (1 entry, darwin Day 4)
  Class D (linux-only):       1/1 green (darwin absent as required)
  Class E (3rd-party):        3/3 green (real url + checksum)
  Capsules projection:        10 entries include 'aarch64-darwin'
```

✅

### Gate 4 — Mac dry-run + direct pre-flight verification

Host: `Darwin arm64`.

**Part A — DRY_RUN=1 lane (CI fast-lane shape):**

```
$ ELASTOS_VZ_SMOKE_DRY_RUN=1 bash scripts/local-carrier-setup-smoke.sh
[local-carrier-setup] test root: /var/folders/…/elastos-local-carrier-setup.XXXXXX
[local-carrier-setup] ELASTOS_VZ_SMOKE_DRY_RUN=1 explicitly set; entering dry-run lane
[local-carrier-setup] dry-run mode: parse OK, helper sourced OK; exiting before cargo build
[local-carrier-setup] dry-run: Vz host capability check passed (macOS 12+)
$ echo $?
0
```

The dry-run lane bypasses pre-flight by design (DRY_RUN=1 is the CI
fast-lane gate); exits cleanly. ✅

**Part B — direct pre-flight helper test (the DRY_RUN=0 shape):**

```
--- Smoke 1: local-carrier-setup-smoke.sh ---
Asserted binaries: shell localhost-provider did-provider webspace-provider
RESULT: PASS — Day-2 metadata satisfies pre-flight

--- Smoke 2: home-frontdoor-smoke.sh ---
Asserted binaries: shell localhost-provider did-provider webspace-provider
RESULT: PASS — Day-2 metadata satisfies pre-flight

--- Smoke 3: chat-wasm-native-interop-smoke.sh ---
Asserted binaries: shell localhost-provider did-provider chat
[cross-platform] components.json missing darwin-arm64 entries for: chat
RESULT: SKIP — Day-3 expected (chat-bundle Class-B work)
```

**2 of 3 smoke pre-flights flip SKIP → PASS as of Day 2.** Chat-wasm
flips on Day 3 (Class-B `chat` capsule). This matches the audit's
per-Class delivery schedule ([`PHASE_6_COMPONENTS_AUDIT.md`](./PHASE_6_COMPONENTS_AUDIT.md) § 4.2). ✅

### Gate 5 — Linux behaviour preserved

**5a — helper passes for both Linux platform keys:**

```
PLATFORM_KEY=linux-amd64:
  PASS: all 4 carrier-setup binaries have linux-amd64 (Linux behaviour preserved)
  PASS: all 4 chat-wasm-interop binaries have linux-amd64 (Linux behaviour preserved)

PLATFORM_KEY=linux-arm64:
  PASS: all 4 carrier-setup binaries have linux-arm64 (Linux behaviour preserved)
  PASS: all 4 chat-wasm-interop binaries have linux-arm64 (Linux behaviour preserved)
```

**5b — diff shows only ADDITIVE changes:**

```
$ git diff HEAD -- components.json | grep '^-' | grep -v '^---'
```

Output: 10 instances of `"aarch64-linux"` (trailing-comma transitions
in arrays gaining a new last element) + 1 crosvm description line
(replaced with a longer version whose prefix is byte-identical to the
original). **No linux-amd64 or linux-arm64 platform row deleted.** ✅

### Gate 6 — Diff scope check

```
$ git status --short
 M components.json
?? scripts/lib/components-json-verify.sh
```

Plus this notes file + the Day-2 banner edit in `PHASE_6_PLAN.md`
landing in the same commit. No other substrate touched. ✅

---

## 5. Carry-forward to Day 3

- **Class B — `chat` capsule darwin-arm64.** Decision D.2 in the audit
  picks "share-linux-arm64-bundle". Day 3's sub-decision (D.2.a duplicate
  cid/checksum vs D.2.b introduce `share_release` field) is the first
  thing Day 3 closes.
- **Class B — verify chat-wasm-interop smoke flips SKIP → PASS on Mac
  once the chat-bundle row lands.** The Day-3 success signal mirrors
  Day-2's 2-of-3.
- **Class C — `vmlinux` darwin-arm64.** Day-4 work (build pipeline for
  arm64 vmlinux per Decision A).
- **Schema-evolution decision (Day 3 vs Day 4):** if Decision D.2 picks
  D.2.b (`share_release` field), the schema gains a new key
  `external.<name>.platforms.<key>.share_release` whose value is
  another platform key. The `setup.rs::resolve_platform_info` resolver
  needs a one-line follow indirection. This is the only piece of Rust
  code Phase 6 touches outside the elastos-vz crate; Day 3 schedules it
  if/when D.2.b is chosen.
- **Phase-7 carry-forward (audit's Decision D):** the published
  gateway's `install.sh` bash-3.2 portability fix is still parked at
  Phase 7. Day 5/6 Mac smokes use `ELASTOS_BIN_OVERRIDE` per
  [`PHASE_5_DAY_3_NOTES.md`](./PHASE_5_DAY_3_NOTES.md).

---

## 6. Day-3 entry signal

Day 3 may start when:

- [x] All 6 Day-2 quality gates green (§ 4).
- [x] `scripts/lib/components-json-verify.sh` is the durable gate for
      Day-3 onwards (Class B promotion).
- [x] The two carry-forward notes in the verifier output point at
      Day 3 (chat) and Day 4 (vmlinux) — the schedule the audit locked
      in.

All three signals met at the time of this commit.

---

**End of PHASE_6_DAY_2_NOTES.md.**
