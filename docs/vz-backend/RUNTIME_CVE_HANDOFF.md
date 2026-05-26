# Runtime CVE hygiene — handoff to broader team

> **Audience:** the broader ElastOS runtime team (whoever owns
> `elastos-compute`, `elastos-server`, `elastos-storage`, `elastos-common`
> and shared dep selection).
> **Source:** `cargo audit v0.22.1` run against branch `sash/local-test`
> (and verified to apply identically against `main`).
> **Suggested branch for this work:** `chore/runtime-cve-hygiene` off `main`.
> **Estimated effort:** ~3 weeks for one engineer, dominated by the
> wasmtime 17 → 45 migration.
> **Not a `sash/local-test` deliverable.** This branch ships the Mac
> substrate; the inherited CVEs in this doc are workspace-wide and belong
> on a separate branch. See `PHASE_10_DAY_1_NOTES.md` §"Why this changes
> Phase 10 scope" for the ownership rationale.

## Why you are reading this

While preparing the Mac substrate branch (`sash/local-test`) for sign-off,
we ran `cargo audit` against the workspace as part of Phase 10 (security
hardening). The audit surfaced **34 vulnerabilities and 12 warnings**. We
classified every one of them by checking whether the vulnerable crate
exists in `main`'s `Cargo.lock` at the same version.

**Result: 34/34 inherited from `main`. Zero introduced by the Mac substrate
branch.** Every vulnerable crate is on the exact same version on `main` as
on `sash/local-test`. The new Mac-only crates we brought in (`objc2`,
`objc2-virtualization`, `objc2-foundation`, `block2`, `dispatch2`) have
zero audit findings.

These vulnerabilities therefore affect both Linux and Mac builds equally
and should be fixed on a runtime-wide branch (off `main`), not on a
Mac-substrate branch. Fixing them on `sash/local-test` would (a) couple
unrelated concerns, (b) violate the `check-linux-untouched.sh` invariant
that protects the clean-substrate-swap framing, and (c) ship under a
misleading PR title.

## Severity histogram

| CVSS | Count |
|---|---|
| 9.0 (CRITICAL) | 2 |
| 8.7 (HIGH) | 1 |
| 7.5 (HIGH) | 3 |
| 7.4 (HIGH) | 1 |
| 6.9 (MEDIUM) | 3 |
| 6.8 (MEDIUM) | 1 |
| 6.1 (MEDIUM) | 2 |
| 5.9 (MEDIUM) | 3 |
| 5.6 (MEDIUM) | 1 |
| 5.1 (MEDIUM) | 2 |
| 4.1 (MEDIUM) | 1 |
| 3.3 (LOW) | 1 |
| 2.3 (LOW) | 3 |
| 1.8 (LOW) | 1 |
| warn (unsound / unmaintained / unscored) | 16 |

## The five remediation clusters

### Cluster A — `wasmtime` 17 → 45 (15 vulnerabilities, both 9.0 criticals)

| Crate | Version | Findings |
|---|---|---|
| `wasmtime` | 17.0.3 | 13 (incl. RUSTSEC-2026-0020 CVSS 9.0, RUSTSEC-2025-0118 CVSS 9.0) |
| `wasmtime-wasi` | 17.0.3 | 1 (CVSS 6.1) |
| `wasmtime-jit-debug` | 17.0.3 | 1 (warn:unsound) |

**Direct dependency:** `elastos-compute/Cargo.toml` pins `wasmtime = "17"`.

**Our usage:** all WASM capsules run on this. The 9.0 critical
"Guest-controlled resource exhaustion in WASI implementations" is
exploitable via any WASI host call a malicious capsule can make. The 9.0
critical "Unsound API access to a WebAssembly shared linear memory" is
exploitable from inside guest WASM if shared memory is enabled.

**Recommended remediation:**

- Bump `wasmtime` to 45 (or whatever is current at the time of the work).
  This is a 28-major-version jump; the wasmtime API has been substantially
  redesigned across this span (component model rework, store/instance API
  changes, WASI surface changes).
- Approach: stand up a temporary `elastos-compute-wasmtime45` shadow crate
  alongside the existing one, port WASI host functions and store/instance
  usage, validate against the existing `home`/`system` WASM capsules, then
  cut `elastos-compute` over and remove the shadow.
- Estimated effort: 3-5 dev-days.
- Expected close: all 15 advisories in this cluster, including both 9.0s.

### Cluster B — TLS chain refresh (10 vulnerabilities)

| Crate | Version | Findings |
|---|---|---|
| `aws-lc-sys` | 0.37.0 | 5 (incl. RUSTSEC-2026-0046 CVSS 7.5, RUSTSEC-2026-0047 CVSS 7.4) |
| `rustls-webpki` | 0.103.9 | 4 (incl. RUSTSEC-2026-0104 CVSS 8.7) |
| `rustls-pemfile` | 2.2.0 | 1 (warn:unmaintained) |

**Direct dependency:** via `reqwest 0.12` (HTTP client) and `iroh 0.96` (P2P
network). Used for Carrier network traffic and IPFS gateway pulls.

**Our usage:** every outbound TLS connection the runtime makes — IPFS pulls
during `elastos setup`, Carrier handshakes, registry fetches.

**Recommended remediation:**

- Bump `reqwest` to a release that pulls newer `rustls` (≥ 0.23 latest) and
  newer `aws-lc-sys` (≥ 0.40 likely).
- Verify `iroh` is on a release that pulls compatible TLS chain.
- Replace `rustls-pemfile` (unmaintained) with `rustls-pki-types` direct
  usage or another maintained PEM parser.
- Estimated effort: 1-2 dev-days.
- Expected close: 10 advisories.

### Cluster C — `cargo update` cascade fix (would close ~14 vulnerabilities)

**Background:** when we tried `cargo update` (passive patch-level updates)
during Day 1 triage, it pulled `pkcs8 0.11.0-rc.11 → 0.11.0` (RC to
stable). The stable release changed the `Error::KeyMalformed` variant
signature, breaking `ed25519-dalek`'s current call site. Build failed; we
reverted the lockfile.

**Recommended remediation:**

- Either bump `ed25519-dalek` to a release that supports stable `pkcs8`,
  or add a workspace-level `[patch]` to pin `pkcs8` to the RC line until
  `ed25519-dalek` catches up.
- Verify build is green after either approach.
- Then run `cargo update` to pick up the ~14 vulnerabilities that close
  automatically with patch-level updates.
- Estimated effort: 0.5-1 dev-day.
- Expected close: ~14 advisories (overlapping with Clusters B / D — re-run
  audit after the cascade fix to see the true delta).

### Cluster D — Targeted bumps (5-7 vulnerabilities)

| Crate | Version | Finding |
|---|---|---|
| `bytes` | 1.11.0 | RUSTSEC-2026-0007 CVSS 7.5 (integer overflow in `BytesMut::reserve`) |
| `tar` | 0.4.44 | RUSTSEC-2026-0067 CVSS 5.1 (`unpack_in` chmod-via-symlink) + warn |
| `time` | 0.3.46 | RUSTSEC-2026-0009 CVSS 5.1 (stack exhaustion) |
| `hickory-proto` | 0.25.2 | RUSTSEC-2026-0119 CVSS 2.3 (O(n²) compression) + warn |
| `quinn-proto` | 0.11.13 | RUSTSEC-2026-0037 warn (DoS) |
| `lru` | 0.12.5 | RUSTSEC-2026-0002 warn:unsound |
| `rand` | 0.8.5 / 0.9.2 | RUSTSEC-2026-0097 warn:unsound |

**Recommended remediation:**

- Audit `Cargo.toml` files for each — bump directly if we control the pin,
  use `[patch]` if transitive.
- Estimated effort: 1 dev-day.
- Expected close: 5-7 advisories.

### Cluster E — Unmaintained-crate replacements (6 warnings)

| Crate | Version | Replacement candidate |
|---|---|---|
| `atomic-polyfill` | 1.0.3 | Use Rust's stable `core::sync::atomic` directly |
| `bincode` | 1.3.3 | Bump to `bincode 2.x` (now maintained again) |
| `core2` | 0.4.0 (yanked) | Find consumer and pin away from `core2` |
| `fxhash` | 0.2.1 | `rustc-hash` |
| `mach` | 0.3.2 | `mach2` (drop-in) |
| `paste` | 1.0.15 | `pastey` (active fork) |
| `rustls-pemfile` | 2.2.0 | covered in Cluster B |

**Recommended remediation:** for each, identify the consumer (some may be
transitive — `cargo tree -i <crate>` works) and decide replace vs
accepted-risk-pending-replacement. The `mach` → `mach2` swap is usually
trivial.

- Estimated effort: 0.5-1 dev-day.
- Expected close: 6 warnings.

## Suggested branch plan

```
git checkout main
git checkout -b chore/runtime-cve-hygiene

# Day 1 — Cluster C (cargo-update cascade fix). Sets up clean baseline.
# Day 2-3 — Cluster B (TLS chain refresh via reqwest bump).
# Day 4-8 — Cluster A (wasmtime 17 → 45 migration). Biggest item.
# Day 9 — Cluster D (targeted bumps).
# Day 10 — Cluster E (unmaintained-crate replacements).
# Day 11 — Re-audit, verify zero HIGH/CRITICAL, document accepted-risk for
#          residual not-applicable advisories, write closeout.
```

End state: `cargo audit` exits zero (or with only accepted-risk warnings
documented in `docs/security/cve-accepted-risk.md`).

## Verification — how we determined ownership

For anyone wanting to reproduce the classification:

```bash
# Get main's lockfile as ground truth
git show main:elastos/Cargo.lock > /tmp/main-Cargo.lock

# For each vulnerable crate, compare presence + version across both
for crate in wasmtime aws-lc-sys rustls-webpki bytes tar time hickory-proto \
             quinn-proto lru rand atomic-polyfill bincode mach fxhash paste \
             core2 cap-primitives wasmtime-wasi wasmtime-jit-debug rustls-pemfile; do
  MAIN_VER=$(awk -v c="$crate" '$0 == "name = \""c"\"" {getline; print $3}' \
               /tmp/main-Cargo.lock | tr -d '"' | sort -u)
  BRANCH_VER=$(awk -v c="$crate" '$0 == "name = \""c"\"" {getline; print $3}' \
                 elastos/Cargo.lock | tr -d '"' | sort -u)
  echo "$crate  main=$MAIN_VER  branch=$BRANCH_VER"
done
```

All 20 unique vulnerable crates returned identical versions on both sides.

## Why this benefits the broader project

- **Both Linux and Mac builds become CVE-clean.** Linux is the primary
  production target today; fixing these on `main` lands them in every
  Linux release immediately.
- **The Mac substrate branch stays small and reviewable.** Without this
  handoff, `sash/local-test` would balloon by another 20+ days of CVE
  work that has nothing to do with Mac. Reviewers would lose the signal
  on what's actually a Mac change.
- **The `check-linux-untouched.sh` audit trail stays valid.** That script
  is the cheapest way for a reviewer to gain confidence that the Linux
  substrate is byte-identical across the merge.

## Pointer for the reviewer of this branch (`sash/local-test`)

If you are reviewing `sash/local-test` and asking "should I block this
merge until the workspace CVEs are addressed?" — the honest answer is
**no**. The CVEs exist on `main` today. Blocking this branch doesn't
make `main` safer; it just delays the Mac substrate shipping. Approve
this branch on its own merits (Mac substrate parity), and treat the
runtime CVE hygiene as parallel work tracked under
`chore/runtime-cve-hygiene` against `main`.
