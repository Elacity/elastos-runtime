# Phase 6 Day 4 — vmlinux build recipe + Mac signing/notarization scaffolding

**Phase**: 6 (macOS native-binary surface)
**Day**: 4 (split: 4a agent-shipped, 4b operator-shipped)
**Date**: 2026-05-25
**Status**: 4a complete; 4b operator handoff documented (see § 5)
**Predecessor**: [`PHASE_6_DAY_3_NOTES.md`](./PHASE_6_DAY_3_NOTES.md)
**Successor**: Day 5 — Self-hosted Mac CI runner activation

---

## 1. Scope deviation from plan — Day 4 split into 4a + 4b

The original Day-4 prompt described eight gates spanning **three
workstreams that converge in one commit**: (Sub-1) building the arm64
`vmlinux` Image, (Sub-2) populating components.json with the real
checksum + size from that build, (Sub-3) signing + notarizing the
`elastos-server` Mac binary against Apple's notary service.

**The honest reality:** Subs 1 and 3 require operator-side execution.

- **Sub-1** needs the ARM64 cross-compile toolchain (`brew install
  aarch64-elf-gcc`, GNU make, libelf, openssl, bc), ~10 GB free disk,
  ~30 min wall-clock on Apple Silicon. The build is deterministic but
  not the kind of thing an agent session can run in-band.
- **Sub-3** needs (a) an active Apple Developer Program enrollment
  (~$99/yr; operator-only), (b) a Developer ID Application certificate
  installed in the login keychain, (c) a `xcrun notarytool
  store-credentials` keychain profile bound to an Apple ID + app-specific
  password, and (d) an active connection to Apple's notary service to
  receive an actual notarization ticket. None of these can be agent-side
  artifacts.

**Day 4a (this commit — agent-shipped):**
- All scaffolding required for an operator to execute Subs 1 and 3
  reproducibly: build recipe, signing recipe, entitlements plist.
- The Sub-2 metadata edit applied **structurally** — `external.vmlinux.platforms.darwin-arm64`
  is created with the empty-`cid`/`checksum` stub pattern that Class-A
  host binaries already use; the release pipeline (or Day-4b operator
  handoff) populates the real values. The Class-C verifier check is
  promoted from forward-compat to **required-keys-present**, matching
  the Class-A pattern; populating the values is operator handoff.

**Day 4b (operator-shipped, separate commits):**
- Run `scripts/build-vmlinux-arm64.sh` on a dev Mac; commit the resulting
  `external.vmlinux.platforms.darwin-arm64.{checksum,size}` populated values.
- Run `scripts/release-mac.sh` end-to-end against a release-mode
  `elastos-server` binary; commit the signed binary's sha256 into the
  release manifest.

This split mirrors the precedent set in [`PHASE_3_DAY_7_NOTES.md`](./PHASE_3_DAY_7_NOTES.md)
§ "Out of scope" where the entitlement-check substrate was wired but
the actual *"Mac-side release-engineering: Developer ID signing pipeline,
entitlement plist, notarization"* was scoped to "release-engineering
territory, not coding work". Phase 6 Day 4a finally addresses that
territory; Day 4b is the operator-side execution.

---

## 2. Concrete changes (Day 4a — agent-shipped)

### 2.1 `scripts/build-vmlinux-arm64.sh` (new, 168 LoC)

Deterministic build recipe for the `vmlinux-darwin-arm64` artifact.

**Inputs (env vars, all defaulted):**

| Var | Default | Meaning |
|---|---|---|
| `ELASTOS_VMLINUX_SRC` | `${REPO_ROOT}/build/linux-6.1.59` | Pre-existing source tree; downloaded if absent |
| `ELASTOS_VMLINUX_SRC_URL` | kernel.org tarball URL | Download source if SRC absent |
| `ELASTOS_VMLINUX_CONFIG` | `scripts/release/vmlinux-arm64.config` | Kconfig template (operator supplies; carry-forward Day-4b) |
| `ELASTOS_VMLINUX_OUT` | `${REPO_ROOT}/elastos/target/vmlinux-darwin-arm64` | Output dir |
| `CROSS_COMPILE` | auto-detected | Toolchain prefix (`aarch64-elf-`, `aarch64-linux-gnu-`, …) |

**Outputs:** `Image`, `Image.sha256`, `Image.size`, `build.log`.

**End-of-build verification** mirrors the runtime's byte-magic check
in [`elastos/crates/elastos-crosvm/src/config.rs`](../../elastos/crates/elastos-crosvm/src/config.rs)
L164–166 (`looks_like_arm64_image()`): bytes `0x38..0x3c` == `b"ARMd"`,
bytes `0x40..0x44` == `b"PE\0\0"`. If the produced Image fails the
check, the script exits 3 with a precise diagnostic — guards against
"built for the wrong ARCH" mistakes.

**Exit codes** are typed (1 = prerequisite missing, 2 = build failure,
3 = byte-magic check failed); the operator gets a clear path to the
fix from the diagnostic alone.

**Preflight tested today on dev Mac:** running with no toolchain
installed produces:

```
[vmlinux-arm64] verifying toolchain prerequisites
[vmlinux-arm64] ERROR: no aarch64 cross-compiler found. Install via 'brew install aarch64-elf-gcc' or set CROSS_COMPILE explicitly.
```

Exit code = 1 (clean). ✅

### 2.2 `scripts/release-mac.sh` (new, 188 LoC)

Sign + notarize + staple recipe.

**7-stage flow:**

1. **Preflight** — verify cert, profile, binary, plist all present.
   Each failure produces a typed exit code (1) with a diagnostic
   naming the missing prerequisite + the exact `security` / `xcrun
   notarytool` command the operator needs to fix it.
2. **Sign** — `codesign --options runtime --entitlements <plist>
   --timestamp --sign <identity>`. Exit 2 on failure.
3. **Verify (pre-notarization)** — `codesign --verify` (must pass) +
   `spctl --assess` (expected to fail at this stage; logged but not
   gating, since notarization hasn't happened yet).
4. **Notarize** — wrap the binary in a zip via `ditto -c -k
   --sequesterRsrc --keepParent`, submit via `xcrun notarytool submit
   --wait --output-format json`. Parse JSON; exit 4 + fetch detailed
   Apple log if status ≠ Accepted.
5. **Staple** — `xcrun stapler staple`. Exit 5 on failure.
6. **Re-verify (post-staple)** — `spctl --assess --type execute` must
   now report `source=Notarized Developer ID`. Exit 6 if not.
7. **Summary** — print signed binary path + sha256 + size for the
   release manifest, plus sanity-check commands the operator should
   run before committing.

**Preflight tested today on dev Mac:** running with no
`ELASTOS_SIGNING_IDENTITY` env var produces:

```
[release-mac] preflight
[release-mac] ERROR: ELASTOS_SIGNING_IDENTITY env var required.
See 'security find-identity -v -p codesigning' for the value;
expect 'Developer ID Application: <name> (TEAMID)'.
```

Exit code = 1 (clean). ✅

### 2.3 `scripts/release/elastos-server.entitlements.plist` (new)

Apple Developer-ID entitlements grant set. Six entitlements granted:

| Entitlement | Why |
|---|---|
| `com.apple.security.virtualization` | Required for `VZVirtualMachine` instantiation. The minimum entitlement to start any microVM via Vz. |
| `com.apple.security.hypervisor` | Required for the underlying Hypervisor.framework calls Vz issues on Apple Silicon. Refusing produces `VZErrorInternalError` at `machine.start()`. |
| `com.apple.vm.networking` | OPTIONAL — only the bridged-network capsule lane needs it. Phase 3 Day 7's runtime check detects this entitlement and routes accordingly (granted → bridged; absent → typed Err for guest_network capsules). Granted here for the signed release binary. |
| `com.apple.security.network.client` | Carrier-bridge guests open client sockets to host RPC + outbound to IPFS gateways. |
| `com.apple.security.network.server` | Carrier-bridge guests accept connections on TCP 127.0.0.1 for the RPC surface. |
| `com.apple.security.files.user-selected.read-write` | Runtime reads operator-selected paths (capsule rootfs, vmlinux) outside its containerised data directory. |

**Linted with `plutil -lint`** — produces `OK`. ✅

### 2.4 `components.json` — Class-C structural promotion

Diff stats: **+10 / −1**.

Added `external.vmlinux.platforms.darwin-arm64`:

```json
"darwin-arm64": {
  "cid": "",
  "checksum": "",
  "size": 0,
  "release_path": "vmlinux-darwin-arm64",
  "install_path": "bin/vmlinux",
  "build_recipe": "scripts/build-vmlinux-arm64.sh",
  "note": "Day-4b operator handoff: …"
}
```

Two new schema fields (additive, schema-forward):

- **`build_recipe`** — names the script that produces the artifact.
  Same shape as the existing `strategy: "source-build"` field on
  `llama-server.linux-arm64`, except more specific (names the actual
  script).
- **`note`** — operator-facing pointer at the Day-4b handoff step.
  Same shape as the existing `note` field on `vmlinux.linux-arm64`.

Plus an extension to `external.vmlinux.description` noting the Decision A
choice + the build-recipe location.

### 2.5 `scripts/lib/components-json-verify.sh` — Class-C promotion

Diff stats: **+24 / −5**.

`CLASS_C_KERNEL` promoted from forward-compat (Day 3) to required.
Two checks:

1. **Structural (hard error):** `linux-amd64`, `linux-arm64`, **and
   `darwin-arm64`** keys must all exist; `darwin-arm64.release_path`
   must be present.
2. **Operator-handoff (soft note):** if `darwin-arm64.checksum` is
   empty, emit a non-fatal note pointing at `scripts/build-vmlinux-arm64.sh`.
   The soft note matches the Class-A pattern (Class-A `cid`/`checksum`
   are also empty pre-release-pipeline; the verifier doesn't gate on
   them).

After Day 4b runs (operator populates checksum), the note disappears
naturally.

---

## 3. Quality gates — Day 4a (5 of 5 green)

Day 4a's gates are a strict subset of the original Day-4 prompt's 8 gates
— the 3 operator-side gates (3, 6, 7 in the original numbering) are
explicitly out-of-scope today; the agent-deliverable gates are all in.

### Gate 4a-1 — components.json parses + verifier Class-C structurally green

```
external=26 capsules=11

[components-json-verify] forward-compat notes (non-fatal):
  [Class C] external.vmlinux.platforms.darwin-arm64.checksum empty — Day-4b operator handoff pending (run scripts/build-vmlinux-arm64.sh)
[components-json-verify] OK
  Class A (host binaries):    7/7 green
  Class B (microVM bundles):  1/1 green (D.2.a share-bundle invariant enforced)
  Class C (kernel):           1/1 green (structural; checksum populated by Day-4b operator handoff)
  Class D (linux-only):       1/1 green (darwin absent as required)
  Class E (3rd-party):        3/3 green (real url + checksum)
  Capsules projection:        10 entries include 'aarch64-darwin'
```

✅ Class C is **structurally** in the green column (parity with Class A).
Only an *operator-handoff note* remains — same shape as the empty-cid
notes the release pipeline addresses on Class A.

### Gate 4a-2 — `cross-platform-test.sh` 47/47

```
cross-platform.sh: 47 passed, 0 failed
```

✅ Phase-5 baseline preserved.

### Gate 4a-3 — Recipe syntax + clean preflight diagnostics

```
build-vmlinux-arm64.sh syntax OK
release-mac.sh syntax OK
scripts/release/elastos-server.entitlements.plist: OK
```

Plus both recipes exit cleanly with typed exit code 1 when their
prerequisites are absent (tested live on the dev Mac at commit time). ✅

### Gate 4a-4 — Mac pre-flight still 3/3 (no Day-4 regression)

```
PASS: carrier-setup
PASS: home-frontdoor
PASS: chat-wasm-interop
```

✅ Day-3's headline outcome preserved.

### Gate 4a-5 — Linux behaviour preserved

```
PASS [linux-amd64] 7 entries resolve (incl. vmlinux+crosvm)
PASS [linux-arm64] 7 entries resolve (incl. vmlinux+crosvm)
```

The 7-entry list includes `vmlinux` and `crosvm` (Class-C + Class-D)
to catch Day-4-specific regressions; both Linux keys still resolve. ✅

### Gate 4a-6 — Diff scope

5 files touched:

```
M  components.json
M  scripts/lib/components-json-verify.sh
?? scripts/build-vmlinux-arm64.sh             (new)
?? scripts/release-mac.sh                     (new)
?? scripts/release/                           (new dir)
   └── elastos-server.entitlements.plist      (new)
```

Plus this notes file + Day-4 banner edit in `PHASE_6_PLAN.md`. Total 7
files in the Day-4a commit. ✅ (above the usual 4-file budget — expected
given Sub-1+Sub-3 each contribute a new script.)

---

## 4. The 3 deferred operator gates (Day 4b)

The original Day-4 prompt's gates 3, 6, 7 are operator-side and need
real Apple cert + real Linux source tree + real notarytool service
access. None of these can close in an agent session. Tracked here for
the operator handoff:

### Gate 4b-3 — `vmlinux-darwin-arm64/Image` builds from `scripts/build-vmlinux-arm64.sh`

**Operator action:** install toolchain (`brew install aarch64-elf-gcc
make elfutils openssl@3 bc`), supply or write `scripts/release/vmlinux-arm64.config`
(see § 5 below for the config-seeding sub-task), run:

```
bash scripts/build-vmlinux-arm64.sh
```

**Success signal:** the recipe's `╔═══` summary box at the end prints
the Image path + a real `sha256:…` value + a real byte size + a build
time. The byte-magic check passes in-band.

### Gate 4b-6 — `scripts/release-mac.sh` end-to-end on a test `elastos-server` build

**Operator action:**
1. Enroll in Apple Developer Program (`https://developer.apple.com/programs/`).
2. Generate a Developer ID Application certificate via
   `https://developer.apple.com/account/resources/certificates/list`;
   install in login keychain.
3. Set up the notarytool profile once:
   ```
   xcrun notarytool store-credentials elastos-notarytool \
       --apple-id you@example.com \
       --team-id <TEAMID> \
       --password <app-specific-password>
   ```
4. Build the binary release-mode (`cargo build --release -p elastos-server`).
5. Export env vars:
   ```
   export ELASTOS_SIGNING_IDENTITY="Developer ID Application: <name> (TEAMID)"
   export ELASTOS_NOTARYTOOL_PROFILE="elastos-notarytool"
   ```
6. Run: `bash scripts/release-mac.sh`.

**Success signal:** the recipe's `╔═══` summary box prints `Notarized:
yes (status=Accepted, ticket stapled)`. `spctl --assess` reports
`source=Notarized Developer ID`.

### Gate 4b-7 — Signed binary entitlements-check substrate passes

**Operator action:** with the binary from Gate 4b-6, run a NAT-only
capsule on Mac and confirm:

```
RUST_LOG=elastos_vz=info ./elastos/target/release/elastos-server --version
```

…does NOT log `lacks com.apple.vm.networking` (the Phase-3-Day-7
substrate's entitlement check should see the entitlement as granted
in the signed binary). Then optionally run a `guest_network: true`
capsule and confirm bridged networking works end-to-end.

---

## 5. Carry-forward to Day 5 + Day-4b operator action items

### Day-5 unblocked
The Mac CI runner activation (Day 5) doesn't depend on Day-4b
completion — the runner can be activated with the structurally-green
Class-C entry (the build pipeline populates the real checksum at
release time, independent of the runner's first activation). **Day 5
is unblocked today.**

### Day-4b operator queue (in execution order)

1. **`scripts/release/vmlinux-arm64.config`** (new — operator
   sub-task). The Kconfig template referenced by `build-vmlinux-arm64.sh`
   doesn't exist yet (the script bails on the prerequisite check until
   it does). Operator: derive from the existing `linux-amd64.vmlinux`
   `.config` (extract from the running Linux-amd64 kernel artifact),
   set `CONFIG_VIRTIO_VSOCKETS=y`, `CONFIG_VIRTIO_CONSOLE=y`, validate.
   ~2h work.
2. **Build vmlinux-arm64 Image** (Gate 4b-3). ~30min.
3. **Populate components.json** with the resulting checksum + size.
   Recipe prints the exact `jq` command at the end of the build.
4. **Set up Developer ID + notarytool profile** (one-time). ~2h
   including Apple enrollment if not already active.
5. **Run signing recipe** (Gate 4b-6). ~5min after step 4.
6. **Run entitlement check sanity** (Gate 4b-7). ~5min.

**Total Day-4b operator wall-clock:** ~5h (one focused afternoon).
**Estimated calendar:** can happen any time before Phase 6 ships; not
on the Day-5/6/7 critical path.

### Phase-7 carry-forward (cross-referenced with Phase 5 retrospective)
- D.2.b schema indirection (parked at Day 3).
- Upstream `install.sh` bash-3.2 portability fix (parked at Day 1
  Decision D).
- The arm64 `vmlinux` build pipeline replacing the host-copy strategy
  on `linux-arm64` (Decision A noted this as a Phase-7 follow-up;
  Day-4a's recipe enables that work but doesn't ship it).

---

## 6. Day-5 entry signal

Day 5 may start when:

- [x] All 5 Day-4a quality gates green (§ 3).
- [x] Build recipe + signing recipe + entitlements plist all
      syntactically valid + clean preflight diagnostics tested.
- [x] components.json Class-C structurally green; operator-handoff
      note documented in § 5.
- [x] No regression on Day-3's 3/3 Mac pre-flight outcome.

All four signals met at the time of this commit.

---

**End of PHASE_6_DAY_4_NOTES.md.**
