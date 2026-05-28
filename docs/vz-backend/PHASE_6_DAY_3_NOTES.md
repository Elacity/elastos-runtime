# Phase 6 Day 3 — Class-B `chat` capsule darwin-arm64 (share-linux-arm64-bundle)

**Phase**: 6 (macOS native-binary surface)
**Day**: 3
**Date**: 2026-05-25
**Status**: Complete; Day-4 unblocked
**Predecessor**: [`PHASE_6_DAY_2_NOTES.md`](./PHASE_6_DAY_2_NOTES.md)
**Successor**: Day 4 — Class-C `vmlinux` darwin-arm64 build + signing/notarization

---

## 1. Scope deviation from plan

**None.** Day 3 is squarely on the audit's Class-B slice (the prompt's
"single remaining smoke-gating darwin-arm64 entry").

---

## 2. Decision D.2.a vs D.2.b — closed: **D.2.a (share-bundle metadata duplication)**

The Day-1 audit deferred this sub-decision to Day 3 (see
[`PHASE_6_COMPONENTS_AUDIT.md`](./PHASE_6_COMPONENTS_AUDIT.md) § 4.2).

### Options reviewed
- **D.2.a — Duplicate `linux-arm64.{cid,checksum,size,release_path,extract_path}`
  into a new `darwin-arm64` block.** Zero substrate change. The runtime
  fetches the linux-arm64 tarball under both keys; the Mac install
  treats it as a darwin-arm64 artifact. Pure metadata edit.
- **D.2.b — Add a new schema key `share_release: "linux-arm64"` to
  `darwin-arm64`.** The `setup.rs::resolve_platform_info` resolver
  follows the indirection. One-line Rust change in `setup.rs` plus
  a new unit test pinning the indirection semantics. More elegant;
  bigger blast radius (touches the only Rust file Phase 6 would
  otherwise leave untouched).

### Decision: **D.2.a.**

**Rationale.**
- Day 3 stays a pure metadata day, consistent with the
  Day-1/Day-2 framing — Phase 6 minimises substrate churn outside the
  elastos-vz crate.
- The "duplication" cost is bounded to **5 string fields** (`cid`,
  `checksum`, `size`, `release_path`, `extract_path`), enforced
  byte-identical by the verifier's new D.2.a invariant ([§ 3.2](#32-scriptslibcomponents-json-verifysh-promotion)).
- D.2.b is a strict refinement, not a strict win — same runtime
  behaviour, less manifest duplication, but introduces a new schema
  key the rest of the resolver doesn't use today. We get a
  read-friendlier manifest **eventually**; we get a lower-risk Phase 6
  **today**.

### Phase-7 carry-forward
**D.2.b parked as a Phase-7 schema-elegance task.** Single line in the
Phase-7 entry checklist when that document is written; not a Phase-6
blocker.

---

## 3. Concrete changes

### 3.1 `components.json` — `external.chat` (1 entry)

Appended `platforms.darwin-arm64` to `external.chat`, byte-identical
to `linux-arm64`:

| Field | linux-arm64 | darwin-arm64 (new) |
|---|---|---|
| `cid` | `""` | `""` |
| `checksum` | `""` | `""` |
| `size` | `0` | `0` |
| `release_path` | `chat-linux-arm64.tar.gz` | `chat-linux-arm64.tar.gz` |
| `extract_path` | `chat` | `chat` |
| `install_path` | `capsules/chat` | `capsules/chat` |

Plus a description extension annotating the Mac sharing strategy:

> *"…On darwin-arm64 the bundle is the same linux-arm64 rootfs (Vz
> boots Linux guests; the rootfs bytes are identical) — see
> docs/vz-backend/PHASE_6_COMPONENTS_AUDIT.md § 4.2 (Decision D.2.a,
> share-linux-arm64-bundle)."*

The empty `cid`/`checksum` will be populated by the release pipeline
in the same step that populates `linux-arm64.{cid,checksum}` — both
keys point at the same tarball, so both fields get the same value at
release time. The verifier's new invariant catches any divergence.

Diff stats: **+9 / −1** (1 deletion is the original chat description
replaced with the longer one).

### 3.2 `scripts/lib/components-json-verify.sh` promotion

Two changes:

1. **`CLASS_B_MICROVM_BUNDLES` promoted from forward-compat to required.**
   Previously the verifier emitted a non-fatal note when `chat.darwin-arm64`
   was absent. Now the same absence is a hard error. Forward-compat
   notes remain only for Class C (`vmlinux`, Day 4).
2. **New D.2.a share-bundle invariant assertion.** Iterates
   `SHARE_BUNDLE_INVARIANT_FIELDS = [cid, checksum, size, release_path,
   extract_path]` and asserts each field on `chat.darwin-arm64` equals
   the corresponding field on `chat.linux-arm64`. Catches accidental
   copy-paste drift in either direction.

**Negative-test proof (the invariant catches drift):**

```
$ python3 -c "import json; d=json.load(open('components.json')); \
  d['external']['chat']['platforms']['darwin-arm64']['release_path']='chat-mac-only.tar.gz'; \
  json.dump(d, open('components.json','w'), indent=2)"
$ bash scripts/lib/components-json-verify.sh
[components-json-verify] FAILED:
  [Class B] D.2.a share-bundle invariant violated:
    external.chat.platforms.darwin-arm64.release_path = 'chat-mac-only.tar.gz',
    linux-arm64.release_path = 'chat-linux-arm64.tar.gz';
    these must be byte-identical for share-linux-arm64-bundle
```

The verifier flagged the mutation; restoring the original restored
green. The release pipeline can now mutate `linux-arm64.{cid,checksum}`
without fear of forgetting to mirror the change — this verifier will
catch it the next time CI runs.

Diff stats: **+30 / −9** (the doc-comment block grew, the Class-B
assertion expanded, and the output summary line updated).

---

## 4. Quality gates (6 of 6 green)

### Gate 1 — `components.json` parses

```
$ python3 -c "import json; d=json.loads(open('components.json').read()); print(f'external={len(d[\"external\"])} capsules={len(d[\"capsules\"])}')"
external=26 capsules=11
```

✅ Inventory counts unchanged from Day 2 — Day 3 only mutated one
entry's inner platforms map (no new top-level keys).

### Gate 2 — `cross-platform-test.sh` — 47/47

```
cross-platform.sh: 47 passed, 0 failed
```

✅ Phase 5 baseline preserved.

### Gate 3 — `components-json-verify.sh` — Class B promoted

```
[components-json-verify] forward-compat notes (non-fatal):
  [Class C] external.vmlinux.platforms.darwin-arm64 — Day 4 expected (carry-forward)
[components-json-verify] OK
  Class A (host binaries):    7/7 green
  Class B (microVM bundles):  1/1 green (D.2.a share-bundle invariant enforced)
  Class C (kernel):           baseline green (1 entry, darwin Day 4)
  Class D (linux-only):       1/1 green (darwin absent as required)
  Class E (3rd-party):        3/3 green (real url + checksum)
  Capsules projection:        10 entries include 'aarch64-darwin'
```

✅ Day-2's `Class B carry-forward` note is **gone** — promoted to
green required. Only Class C remains as forward-compat (Day 4).

### Gate 4 — Mac pre-flight — **3 of 3 smokes PASS** (headline outcome)

Host: `Darwin arm64`.

```
[Gate 4] Mac pre-flight 3/3:
  PASS: local-carrier-setup
  PASS: home-frontdoor
  PASS: chat-wasm-interop   ⬅ Day 3 headline flip
```

✅ All three Phase-5 smokes can now proceed past pre-flight on
Mac. Day 2 flipped 2/3; Day 3 closes the remaining one (`chat-wasm-interop`)
by landing the `chat` capsule's darwin-arm64 entry.

### Gate 5 — Linux behaviour preserved

```
[Gate 5] Linux preserved:
  PASS [linux-amd64] all 5 smoke binaries resolve
  PASS [linux-arm64] all 5 smoke binaries resolve
```

`5 smoke binaries` = the union of all 3 smokes' asserted-binary lists
(`shell, localhost-provider, did-provider, webspace-provider, chat`).
✅ The Day-3 diff is purely additive — both Linux keys still resolve
for every smoke-required binary.

### Gate 6 — Diff scope

```
$ git status --short
 M components.json
 M scripts/lib/components-json-verify.sh
```

Plus this notes file (new) + the Day-3 banner edit in
`PHASE_6_PLAN.md`. ✅ Four files touched, as planned.

---

## 5. Carry-forward to Day 4

- **Class C — `vmlinux` darwin-arm64.** Decision A (Phase 0) chose
  *"build same 6.1.59 source for arm64"*. Day 4 sets up the arm64
  cross-compile recipe (`ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu-`)
  + a `scripts/build-vmlinux-arm64.sh` analogous to the existing
  `build-llama-server.sh` source-build pattern. The verifier's
  Class-C forward-compat note flips to green required at that point.
- **Day-4 signing/notarization scope.** The audit's mission says
  Phase 6 ships *"a signed + notarized Mac binary"*. Day 4 is the
  earliest day this can land — `vmlinux` + the `elastos-server`
  binary build go through Apple Developer-ID signing + notarization
  on the same Day-4 commit. This is the **biggest single-day deliverable**
  of Phase 6; the original plan budgeted Day 4 at 6–8h precisely for
  this.
- **`crosvm` darwin marker.** No further work — Decision B is closed
  (Day 1) and the Mac install-loop skip is verified to behave as
  documented.
- **D.2.b schema-indirection refinement.** Phase 7 carry-forward; no
  Phase-6 follow-up needed.

---

## 6. Day-4 entry signal

Day 4 may start when:

- [x] All 6 Day-3 quality gates green (§ 4).
- [x] D.2.a closed with rationale + Phase-7 carry-forward recorded.
- [x] Class B promoted in the verifier; only Class C remains as
      forward-compat.
- [x] 3/3 Mac smoke pre-flights demonstrably PASS on a dev Mac
      (recorded in § 4 gate 4).

All four signals met at the time of this commit.

---

**End of PHASE_6_DAY_3_NOTES.md.**
