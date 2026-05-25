# Phase 6 Day 1 — `components.json` darwin-arm64 Audit

**Phase**: 6 (macOS native-binary surface)
**Day**: 1
**Date**: 2026-05-25
**Status**: Complete (read-only audit; no substrate edits)
**Predecessor**: [`PHASE_6_PLAN.md`](./PHASE_6_PLAN.md) § Day 1
**Successor**: Day 2 — `components.json` Decision-A/B/C metadata edits

---

## 1. Mission

Produce the per-binary decision matrix Phase 6 Days 2–4 need before any
`components.json` edit. The audit is **read-only on substrate** — the only
file touched by Day 1 is this audit doc plus the Day-1 outcome banner in
[`PHASE_6_PLAN.md`](./PHASE_6_PLAN.md).

Every Day-2/3/4 `components.json` edit must trace back to a row in
[§ 4](#4-per-binary-decision-matrix) below; every architecture decision
must trace back to a closed row in [§ 5](#5-architecture-decisions).

---

## 2. Anchors

The audit is anchored to existing, agreed documentation (no new opinions):

| Anchor | Why it matters here |
|---|---|
| [`PHASE_0_SCOPE.md`](./PHASE_0_SCOPE.md) § C ("Does the shipped `vmlinux` boot on Vz unmodified?") | Established that **a content-addressed `darwin-arm64` `vmlinux` is the Phase-6 deliverable**; lists the three viable strategies (build same 6.1.59 source / pin Ubuntu LTS / embed-in-binary). Decision A in [§ 5](#5-architecture-decisions) picks one. |
| [`PHASE_0_SCOPE.md`](./PHASE_0_SCOPE.md) § Risk register row "Shipped `vmlinux` on `linux-arm64` uses host-kernel-copy strategy" | Confirms `linux-arm64.vmlinux` already uses `strategy: "local-copy"` from `/boot/Image` — i.e. the Linux ARM lane is **not** a content-addressed artifact either. Day-1 audit notes this as pre-existing schema work that constrains Decision A. |
| [`PHASE_5_DAY_1_NOTES.md`](./PHASE_5_DAY_1_NOTES.md) ("Pre-Work removed dishonest darwin entries") | Sets the truthfulness bar: any darwin-arm64 entry added in Phase 6 must point at a real artifact (real `cid` / `checksum` or a real upstream `url` + `checksum`). Stub entries are explicitly forbidden. |
| [`PHASE_5_DAY_2_NOTES.md`](./PHASE_5_DAY_2_NOTES.md) (hoist of `cross_platform_assert_native_binary_release_metadata`) | Locks the smoke-side contract: the helper reads `manifest.external[name].platforms` and accepts either the host platform key (`darwin-arm64` on Mac) **or** the wildcard `"*"`. Any decision must produce one of these two keys for every smoke-required binary. |
| [`PHASE_5_DAY_3_NOTES.md`](./PHASE_5_DAY_3_NOTES.md) (chat-wasm interop, `install.sh` bash-3.2 issue, `ELASTOS_BIN_OVERRIDE`) | Decides Phase 6's `install.sh` scope: the published gateway's `install.sh` is upstream and we don't fix it in Phase 6. Day 2/3 smokes use `ELASTOS_BIN_OVERRIDE` instead. |
| [`scripts/lib/cross-platform.sh`](../../scripts/lib/cross-platform.sh) L184–254 (`cross_platform_assert_native_binary_release_metadata`) | The single source of truth for the platform-key check the smokes use. |
| [`elastos/crates/elastos-server/src/setup.rs`](../../elastos/crates/elastos-server/src/setup.rs) L407–421 (`detect_platform()`), L815–851 (`resolve_platform_info` + `platform_aliases`) | Confirms: (a) `detect_platform()` returns `"darwin-arm64"` on Apple Silicon, (b) `aarch64-darwin` ⇄ `darwin-arm64` are bidirectional aliases, (c) a missing platform entry returns `None` and the installer prints `"[skip] {name} — not available for {platform}"` and continues. Makes Decision B safe by design. |

---

## 3. Smoke surface map

`grep cross_platform_assert_native_binary_release_metadata scripts/*.sh`
returns three smokes that gate on `components.json` darwin-arm64 entries
today. The set of asserted binary names is small:

| Smoke | Asserted binaries (passed to the helper) | Source |
|---|---|---|
| `local-carrier-setup-smoke.sh` | `shell`, `localhost-provider`, `did-provider`, `webspace-provider` | [`scripts/local-carrier-setup-smoke.sh`](../../scripts/local-carrier-setup-smoke.sh) L171–173 |
| `home-frontdoor-smoke.sh` | `shell`, `localhost-provider`, `did-provider`, `webspace-provider` | [`scripts/home-frontdoor-smoke.sh`](../../scripts/home-frontdoor-smoke.sh) L187–189 |
| `chat-wasm-native-interop-smoke.sh` | `shell`, `localhost-provider`, `did-provider`, `chat` | [`scripts/chat-wasm-native-interop-smoke.sh`](../../scripts/chat-wasm-native-interop-smoke.sh) L209–211 |

**Union of smoke-required binaries** (= the minimum set Day 2 must populate
to flip all three smokes from SKIP to RUN on Mac):

  `shell`, `localhost-provider`, `did-provider`, `webspace-provider`, `chat`

Everything else in `external` either (a) is wildcard `"*"` already (so
the helper accepts it on every host) or (b) isn't asserted by any
Phase-5 smoke and so its darwin-arm64 entry is **optional for Phase 6
smoke green** but **required for Phase 6 install green** (see Day 5 in
the plan).

---

## 4. Per-binary decision matrix

The `external` section of [`components.json`](../../components.json) has
**26 entries** (excluding the `capsules` projection). Every entry below
gets a row; every row terminates in one of four columns: **needs
darwin-arm64 cross-compile**, **needs darwin-arm64 entry pointing at a
shared / upstream artifact**, **omit darwin entry (host-substrate
covers it)**, or **already cross-platform via `"*"`**.

> Legend
> - 🔴 **Smoke-required** — at least one Phase-5 smoke asserts on this binary; gating Phase 6 Day 5/6 CI green.
> - 🟡 **Profile-required** — listed in a `profiles.*` entry; gating Phase 6 Day 5 install green.
> - ⚪️ **Already cross-platform** — wildcard `"*"`; no Phase 6 action.
> - ⚫️ **Linux-only by design** — install-time skip on Mac is the correct outcome.

### 4.1 Class A — Host Rust binaries we build (cross-compile-from-source on Mac)

These are first-party Rust binaries that run on the host (not inside a
microVM). The descriptions in `components.json` start with `"Host …
binary required for …"`. To run on macOS they must be built for the
`aarch64-apple-darwin` target; **the linux-arm64 ELF cannot be reused**
because the host OS ABI is different.

| Binary | Smoke gate? | Profile use | Day-2 action | Decision row |
|---|---|---|---|---|
| `shell` | 🔴 carrier-setup + home-frontdoor + chat-wasm-interop | `home`, `demo`, `chat`, `operator`, `agent-local-ai`, `public-gateway`, `full` | Add `darwin-arm64` entry; `cid`+`checksum` populated by Day-3 release build | D.1 |
| `localhost-provider` | 🔴 carrier-setup + home-frontdoor + chat-wasm-interop | `home`, `demo`, `chat`, `operator`, `agent-local-ai`, `public-gateway`, `full` | Add `darwin-arm64` entry; `cid`+`checksum` populated by Day-3 release build | D.1 |
| `did-provider` | 🔴 carrier-setup + home-frontdoor + chat-wasm-interop | `home`, `demo`, `chat`, `operator`, `agent-local-ai`, `public-gateway`, `full` | Add `darwin-arm64` entry; `cid`+`checksum` populated by Day-3 release build | D.1 |
| `webspace-provider` | 🔴 carrier-setup + home-frontdoor | `home`, `demo`, `agent-local-ai`, `public-gateway`, `full` | Add `darwin-arm64` entry; `cid`+`checksum` populated by Day-3 release build | D.1 |
| `ipfs-provider` | 🟡 profile gate only | `demo`, `agent-local-ai`, `public-gateway`, `full` | Add `darwin-arm64` entry; `cid`+`checksum` populated by Day-3 release build | D.1 |
| `tunnel-provider` | 🟡 profile gate only | `demo`, `public-gateway`, `full` | Add `darwin-arm64` entry; `cid`+`checksum` populated by Day-3 release build | D.1 |
| `site-provider` | 🟡 profile gate only | `demo`, `public-gateway`, `full` | Add `darwin-arm64` entry; `cid`+`checksum` populated by Day-3 release build | D.1 |

**Strategy (Decision D.1):** **cross-compile-from-source**, identical to
the existing linux-amd64/linux-arm64 release lanes. The Day-2 schema
edit adds the `platforms.darwin-arm64` key with empty `cid`/`checksum`
fields and a `release_path` of `{name}-darwin-arm64`; Day-3 plugs the
release pipeline to populate the fields. This matches the existing
empty-`cid`/`checksum` shape for `linux-amd64`/`linux-arm64` on these
same binaries (the release pipeline is the populator for every
platform, not just darwin).

### 4.2 Class B — MicroVM capsule bundles (share linux-arm64 rootfs)

The bundle is a tarball of a Linux rootfs that boots inside the
microVM substrate. The same `linux-arm64.tar.gz` rootfs that runs in
crosvm on a Linux-arm64 host **also runs in Vz on a darwin-arm64
host** — Vz's `VZLinuxBootLoader` consumes the same `Image` + rootfs
contract as crosvm aarch64. No second build is required.

| Binary | Smoke gate? | Profile use | Day-2 action | Decision row |
|---|---|---|---|---|
| `chat` | 🔴 chat-wasm-interop | `demo`, `chat`, `full` | Add `darwin-arm64` entry that **reuses** the `linux-arm64.tar.gz` release artifact via a `share_release` field (or duplicates the `cid`/`checksum`/`release_path` of `linux-arm64`); see Decision D.2 | D.2 |

**Strategy (Decision D.2):** **share-linux-arm64-bundle**. Two
implementation options (Day-2 decides one):

- D.2.a — **Duplicate the `linux-arm64` `cid`/`checksum`/`release_path`**
  into `darwin-arm64`. Simplest; the release pipeline already produces
  the linux-arm64 tarball.
- D.2.b — **Introduce a `share_release: "linux-arm64"` reference** that
  the resolver dereferences. More elegant; requires a one-line
  setup.rs change. **Out of Day-1 scope** — recorded as a Day-2
  sub-decision.

Day-1 records the **strategy**; Day-2 picks `a`/`b` and ships the edit.

### 4.3 Class C — Kernel artifact (Mac-targeted rebuild)

| Binary | Smoke gate? | Profile use | Day-2 action | Decision row |
|---|---|---|---|---|
| `vmlinux` | not gated by Phase-5 smokes (vmlinux load is exercised by Phase 6 Day 6 full smoke, not Day 5) | `minimal`, `chat`, `full` | Add `darwin-arm64` entry; **strategy = build same 6.1.59 source for arm64** (per Decision A in [§ 5](#5-architecture-decisions)) | A |

**Notes:**
- The existing `linux-arm64.vmlinux` entry uses
  `strategy: "local-copy", source: "/boot/Image"` (host-kernel-copy
  from the running Jetson). This is **not** reusable on Mac (macOS has
  no `/boot/Image`; the host kernel is XNU, not Linux).
- Decision A in [§ 5](#5-architecture-decisions) picks among the three
  Phase-0 options. The audit's recommendation is option 1 (build same
  6.1.59 source for arm64) for the content-addressed identity story.

### 4.4 Class D — Linux-only substrate (omit darwin entry)

| Binary | Smoke gate? | Profile use | Day-2 action | Decision row |
|---|---|---|---|---|
| `crosvm` | ⚫️ not gated by any smoke | `minimal`, `chat`, `full` | **No darwin entry.** Add a `description` note: `"Linux-only; Apple Vz substrate replaces crosvm on darwin (see docs/vz-backend/PLAN.md)"`. | B |

**Why "omit" is safe (load-bearing on Decision B):**
`elastos/crates/elastos-server/src/setup.rs` L815–820 + L200–203 show
that `resolve_platform_info` returns `None` when the host platform key
is absent and the install loop prints
`"[skip] {name} — not available for {platform}"` and continues.
**Mac install of the `chat` profile therefore skips `crosvm`
cleanly without erroring; the host Vz substrate provides the
microVM monitor instead.** No schema change to the profile is needed
for Phase 6.

### 4.5 Class E — Third-party native helpers (upstream macOS builds)

These have public upstream macOS-arm64 releases at the same URL
pattern as their `linux-arm64` entries. Day-2 ingests them with their
real upstream `url` + `checksum` — no rebuild required.

| Binary | Smoke gate? | Profile use | Day-2 action | Decision row |
|---|---|---|---|---|
| `kubo` | 🟡 profile gate only | `demo`, `full`, `agent-local-ai`, `public-gateway` | Add `darwin-arm64`: `url = https://dist.ipfs.tech/kubo/v0.40.1/kubo_v0.40.1_darwin-arm64.tar.gz`, `checksum` fetched by Day-2 (`curl --silent` + `shasum -a 512` on the live release) | C |
| `cloudflared` | 🟡 profile gate only | `demo`, `full`, `public-gateway` | Add `darwin-arm64`: `url = https://github.com/cloudflare/cloudflared/releases/download/2026.2.0/cloudflared-darwin-arm64`, `checksum` fetched by Day-2 | C |
| `llama-server` | 🟡 profile gate only | `full`, `agent-local-ai` | Add `darwin-arm64`: `url = https://github.com/ggml-org/llama.cpp/releases/download/b8192/llama-b8192-bin-macos-arm64.tar.gz` (verify exact asset name at Day-2 time), `checksum` fetched by Day-2 | C |

**Strategy (Decision C):** **ingest-upstream**. Day-2's first action is
a 3-`curl` pass that fetches the live `.sha256`/`.sha512` for each
asset and writes them into the schema. No host build pipeline cost.

### 4.6 Class F — Already cross-platform via `"*"` (no action)

The remaining 13 entries are wildcard `"*"` and pass the helper on
every host. Day-2 **does not touch them.**

| Wildcard `"*"` entries (no Phase 6 action) |
|---|
| `home-cli`, `home`, `system`, `chat-wasm`, `documents`, `library`, `inbox`, `gba-emulator`, `gba-ucity`, `chat-room`, `model-qwen3.5-0.8b`, `model-qwen3.5-4b`, `model-qwen3.5-9b` |

### 4.7 Capsules-section parity

The `capsules` section of [`components.json`](../../components.json) is a
separate projection from `external`, using **Rust target-triple-style
keys** (`x86_64-linux`, `aarch64-linux`, etc.) instead of release-key-style
keys (`linux-amd64`, `linux-arm64`). 10 capsule entries today list
`[x86_64-linux, aarch64-linux]` and 1 (`chat-wasm`) lists `["any"]`.

Day-2's edit set extends every capsules entry that maps to a Class-A or
Class-B `external` row above by appending `"aarch64-darwin"` to the
platforms array. The alias map in `setup.rs::platform_aliases`
(L840–851) already bridges `aarch64-darwin` ⇄ `darwin-arm64`, so this is
a one-token-per-entry change.

| Capsule | Current platforms | Day-2 platforms |
|---|---|---|
| `shell` | `[x86_64-linux, aarch64-linux]` | `[x86_64-linux, aarch64-linux, aarch64-darwin]` |
| `localhost-provider` | `[x86_64-linux, aarch64-linux]` | `[x86_64-linux, aarch64-linux, aarch64-darwin]` |
| `chat` | `[x86_64-linux, aarch64-linux]` | `[x86_64-linux, aarch64-linux, aarch64-darwin]` |
| `did-provider` | `[x86_64-linux, aarch64-linux]` | `[x86_64-linux, aarch64-linux, aarch64-darwin]` |
| `ipfs-provider` | `[x86_64-linux, aarch64-linux]` | `[x86_64-linux, aarch64-linux, aarch64-darwin]` |
| `tunnel-provider` | `[x86_64-linux, aarch64-linux]` | `[x86_64-linux, aarch64-linux, aarch64-darwin]` |
| `notepad` | `[x86_64-linux, aarch64-linux]` | `[x86_64-linux, aarch64-linux, aarch64-darwin]` |
| `ai-provider` | `[x86_64-linux, aarch64-linux]` | `[x86_64-linux, aarch64-linux, aarch64-darwin]` |
| `llama-provider` | `[x86_64-linux, aarch64-linux]` | `[x86_64-linux, aarch64-linux, aarch64-darwin]` |
| `agent` | `[x86_64-linux, aarch64-linux]` | `[x86_64-linux, aarch64-linux, aarch64-darwin]` |
| `chat-wasm` | `[any]` | `[any]` (no change) |

> **Note on `ai-provider` / `llama-provider` / `agent` / `notepad`:** these
> have no corresponding `external` entry. They are pure capsule-metadata
> rows. Day-2 still appends `aarch64-darwin` for forward consistency;
> the runtime treats unknown-but-listed targets as "no native binary
> required" via the existing capsule-loader contract.

---

## 5. Architecture decisions

Every decision below was raised by the Phase-6 plan, surfaced by the
inventory above, or carried forward from a Phase-0/Phase-5 risk row.
All four are **closed** for Day 1 (i.e. the audit picks the path Day 2
implements).

### Decision A — `vmlinux` darwin-arm64 sourcing

**Question:** What artifact does `external.vmlinux.platforms.darwin-arm64`
point at?

**Options** (from [`PHASE_0_SCOPE.md`](./PHASE_0_SCOPE.md) § C.3):

1. **Build same 6.1.59 source tree for arm64** — most aligned with the
   runtime's content-addressed identity model. Single guest-kernel
   identity across all hosts.
2. **Pin Ubuntu LTS arm64 checksum** (cloud-images.ubuntu.com signed
   kernel). Lower build burden; weaker provenance story; introduces a
   second guest-kernel identity into the ecosystem.
3. **Embed kernel inside the Vz binary at compile time.** Rejected in
   Phase 0 — breaks the components.json content-addressed contract.

**Decision:** **Option 1 — build same 6.1.59 source for arm64.**

**Rationale:**
- The current `linux-arm64.vmlinux` uses a host-copy strategy
  (`/boot/Image`), which is itself a placeholder for "we don't have an
  arm64 build yet". Phase 6 darwin-arm64 work is the natural moment to
  set up the arm64 build pipeline; the same pipeline output then
  replaces the host-copy strategy on `linux-arm64` (Phase 7 carry-forward).
- Identity uniqueness matters for the provenance + audit story the
  runtime already invests in for `linux-amd64` (real `cid` + real
  `checksum` shipped today).
- The build cost is bounded: the existing `linux-amd64.vmlinux` build
  recipe extends to arm64 via standard cross-compile (`ARCH=arm64
  CROSS_COMPILE=aarch64-linux-gnu-`). Estimated <1 working day of
  pipeline plumbing.

**Day-2 follow-through:**
- The Day-2 schema edit adds `external.vmlinux.platforms.darwin-arm64`
  with empty `cid`/`checksum` and a TODO note pointing at the build
  task (which is **out of Day-2 scope**; tracked as a Phase-6
  carry-forward in [`PHASE_6_ENTRY_CHECKLIST.md`](./PHASE_6_ENTRY_CHECKLIST.md)
  § Unblockers).
- The Day-6 full-smoke gate (microVM boot) treats this entry as the
  first thing the Vz lane fetches.

**Closed:** ✅ Day 1.

### Decision B — `crosvm` darwin-arm64 sentinel

**Question:** Does `crosvm.platforms` get an explicit darwin entry, or
do we omit it?

**Options:**

1. **Omit darwin entry.** `resolve_platform_info` returns `None`;
   the install loop prints `"[skip] crosvm — not available for
   darwin-arm64"` and continues. The host Vz substrate (built into
   macOS) covers the microVM monitor role.
2. **Add `"n/a-on-darwin"` sentinel + teach the helper to skip it.**
   Schema change; install-loop change; **no operator visibility
   benefit** (the skip message already names crosvm).

**Decision:** **Option 1 — omit darwin entry.**

**Rationale:**
- `setup.rs::resolve_platform_info` + `setup.rs` L200–203 already
  produce a clean "not available for darwin-arm64" install-time skip.
  No code change required.
- The skip is **operator-correct**: crosvm genuinely is not needed on
  Mac because Vz replaces it.
- Adding the `description` field a single Mac-specific paragraph
  (`"Linux-only; Apple Vz substrate replaces crosvm on darwin (see
  docs/vz-backend/PLAN.md)"`) is the minimum-cost way to capture this
  for operators reading the manifest directly.
- The `chat` profile lists `crosvm` as a component, but the install
  loop skip is graceful; `chat-profile` install on Mac surfaces a
  single `"[skip]"` line and proceeds. **No profile edit required.**

**Day-2 follow-through:**
- Day-2 schema edit appends the description paragraph; no
  `platforms.darwin-arm64` key is added.

**Closed:** ✅ Day 1.

### Decision C — Native-helper darwin sourcing strategy

**Question:** `kubo`, `cloudflared`, `llama-server` — upstream macOS-arm64
builds or local source-build?

**Options:**

1. **Ingest upstream macOS builds** for each. All three projects publish
   official darwin-arm64 release assets.
2. **Local source-build** (mirror the `llama-server.linux-arm64` row,
   which uses `strategy: "source-build"`). Higher pipeline cost; only
   needed if the upstream asset is missing.

**Decision:** **Option 1 — ingest upstream macOS builds.**

**Rationale:**
- All three publishers ship darwin-arm64 assets at predictable URLs
  (table in [§ 4.5](#45-class-e--third-party-native-helpers-upstream-macos-builds)).
- The upstream checksum story is already part of the existing
  linux-arm64 ingestion path (the `url` + `checksum` pair); zero schema
  change required.
- Source-build is **not** required for any of the three on darwin-arm64
  as of Day-1 audit. (Re-check at Day 2 release time; if `llama-server`
  no longer publishes a darwin-arm64 release we'd fall back to
  `strategy: "source-build"` with a `scripts/build-llama-server-mac.sh`
  mirror — explicit Phase-6 sub-task.)

**Day-2 follow-through:**
- Day-2 fetches the live `.sha256`/`.sha512` per asset (3 × `curl
  --silent`) and writes them into the new `platforms.darwin-arm64`
  blocks.

**Closed:** ✅ Day 1.

### Decision D — `install.sh` scope on Mac

**Question:** Phase-5 Day 3 surfaced that the published gateway's
`install.sh` uses GNU-bash-isms incompatible with macOS BSD bash 3.2
(`[[ -v VAR ]]`, `mapfile`, etc.). Does Phase 6 fix `install.sh` upstream
or scope Mac smokes to `ELASTOS_BIN_OVERRIDE`?

**Options:**

1. **Fix `install.sh` upstream** in Phase 6.
2. **Scope Mac smokes to `ELASTOS_BIN_OVERRIDE`** + carry the upstream
   `install.sh` fix into Phase 7.

**Decision:** **Option 2 — `ELASTOS_BIN_OVERRIDE` for Phase 6 Mac
smokes; upstream `install.sh` fix is Phase-7 carry-forward.**

**Rationale:**
- `install.sh` lives on the published gateway, **not in this repo**.
  Fixing it is an out-of-repo change; we shouldn't expand Phase-6 scope
  to touch a separately-versioned artifact.
- `ELASTOS_BIN_OVERRIDE` is already implemented in the Day-3 chat-wasm
  interop smoke (per
  [`PHASE_5_DAY_3_NOTES.md`](./PHASE_5_DAY_3_NOTES.md)); reusing it for
  Mac unblocks Day 5/6 smokes without any new code.
- Phase 7 audit captures the upstream fix work (single bash 3.2 portability
  pass on the gateway's install.sh; mirrors the Phase-5 portability work
  done on every smoke).

**Day-2/3 follow-through:**
- The 3 smokes already use `cross_platform_assert_native_binary_release_metadata`
  + `ELASTOS_BIN_OVERRIDE`; no new wiring required on the smoke side.
- Day-7 retrospective records the `install.sh` upstream fix as the
  single Phase-7 carry-forward item.

**Closed:** ✅ Day 1.

---

## 6. Out of Day-1 scope (audit, not implementation)

These items the audit explicitly **does not** decide; they are Day-2+
work and tracked in [`PHASE_6_PLAN.md`](./PHASE_6_PLAN.md):

- **The actual `components.json` edit.** Day 1 is read-only. Day 2
  applies the schema changes derived from [§ 4](#4-per-binary-decision-matrix) +
  [§ 5](#5-architecture-decisions).
- **D.2.a vs D.2.b** (`chat` bundle: duplicate `linux-arm64`
  `cid`/`checksum` vs introduce `share_release` field). Day-2
  sub-decision; both options shape the audit's "share-linux-arm64-bundle"
  strategy identically.
- **Building the arm64 `vmlinux` artifact.** Decision A picks the
  strategy; the build pipeline plumbing is Day-3/Day-4 work and is
  itself a Phase-6 sub-deliverable.
- **Release pipeline plumbing** for the 7 Class-A host binaries
  (cross-compile `aarch64-apple-darwin`, sign, notarize). Day-3 + Day-4
  work; entirely out of Day-1 audit scope.
- **Profile-level edits.** No profile needs editing in Phase 6
  (Decision B + the install loop's clean skip make profiles
  Mac-compatible as-is).
- **Schema version bump.** No schema-shape change is required for the
  Day-2 edits — every new key (`darwin-arm64` per `external` entry,
  `aarch64-darwin` per `capsules` entry) is data-only inside the
  existing `elastos.components/v1` schema.

---

## 7. Quality gates (Day 1)

All six gates green:

- [x] **Docs-only diff.** Only files touched: this audit doc +
      [`PHASE_6_PLAN.md`](./PHASE_6_PLAN.md) § Day 1 outcome banner.
      Verified with `git diff --name-only`.
- [x] **Linux substrate untouched.** No edits to `components.json`,
      `setup.rs`, `cross-platform.sh`, or any smoke script. Verified by
      empty diff outside `docs/`.
- [x] **All four decisions closed.** A, B, C, D — each terminates in a
      "Decision: …" + "Closed: ✅ Day 1" line.
- [x] **All cross-references resolve.** Every `[link](path)` above
      points at an existing file (verified by tail-line `ls` checks
      during draft).
- [x] **Inventory completeness.** All 26 `external` entries and all 11
      `capsules` entries appear in exactly one row of the matrix in
      [§ 4](#4-per-binary-decision-matrix) (no double-counts, no
      omissions).
- [x] **Smoke surface map is byte-truthful.** The 4/4/4-binary lists in
      [§ 3](#3-smoke-surface-map) match the literal arguments passed to
      `cross_platform_assert_native_binary_release_metadata` at the
      cited line numbers (verified with `grep -A2`).

---

## 8. Day-2 entry signal

Day 2 may start when **all four decisions in [§ 5](#5-architecture-decisions)
are marked `Closed: ✅ Day 1`** and the Day-1 outcome banner in
[`PHASE_6_PLAN.md`](./PHASE_6_PLAN.md) § Day 1 reads
`"Audit complete; Day-2 unblocked."` Both gates are met at the time of
this commit.

---

**End of PHASE_6_COMPONENTS_AUDIT.md.**
