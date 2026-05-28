# Phase 10 Day 14 — GitHub Actions Mac release lane

**Status:** complete; lane is wired but has not yet run against a real release event.
**Branch:** `sash/local-test`
**Date:** 2026-05-26
**Predecessor:** `PHASE_10_DAY_9_NOTES.md` (demo-bug fixes)
**Successor:** `PHASE_10_DAY_15_NOTES.md` (Phase 10 sign-off — to come)

---

## What landed

Three files, one purpose: enable an operator to publish a GitHub
release and have a signed, smoke-checked, Apple-Silicon tarball
attached to it automatically (modulo the manual notarisation step,
which stays on the operator's machine for security reasons).

### `scripts/release/release-mac.sh` (412 LOC, executable)

Operator-runnable release script. Takes a release tag as its single
positional argument; produces `elastos-<tag>-aarch64-apple-darwin.tar.gz`
plus a `.sha256` companion under `elastos/target/release-mac/<tag>/`.

Pipeline:

1. **Pre-flight** — assert macOS + arm64, assert clean tree + HEAD is at
   the named tag (skipped under `--dry-run`).
2. **Build** — `cargo build --release --bin elastos -p elastos-server` and
   `cargo build --release -p elastos-vz`. Workspace is `elastos/`; binary
   lands at `elastos/target/release/elastos`.
3. **Stage** — copy the binary to `target/release-mac/<tag>/stage/elastos`.
4. **Sign** — invoke `scripts/dev/sign-elastos-vz/sign.sh` unchanged
   (positional binary-path arg already supported). Ad-hoc signature
   (`--sign -`) with the four entitlements from `vz.entitlements.plist`.
5. **Verify** — `codesign -d --entitlements -` and grep for each of the
   four required keys. Missing any → exit 3.
6. **Smoke check** — run the staged binary and assert (a) `--version`
   exits 0 and prints "elastos", (b) `vm-debug --help` exits 0 and
   mentions "boot" (proves the macOS-only Vz subcommand linked).
7. **Tarball + SHA256** — deterministic top-level dir
   `elastos-<tag>-aarch64-apple-darwin/` containing the binary, a `VERSION`
   file, and a `README.txt` for the downloader. `gzip -n` strips embedded
   mtime for bit-reproducibility.
8. **Print notarise commands** — exact `xcrun notarytool submit … --wait`
   + repackage + `xcrun stapler staple` sequence the operator runs on
   their own Mac.

Idempotent on re-runs (`rm -rf` of the stage dir, `--force` re-sign).
Six exit codes for clean CI diagnosis (1 pre-flight, 2 build, 3 sign,
4 smoke, 5 tarball, 0 success).

### `.github/workflows/release-mac.yml` (167 LOC)

Workflow fires on `release: types: [published]` — the operator publishes a
draft release in the GitHub UI, this workflow builds + signs + uploads
the artifact within ~5-7 minutes of cache-warm build time.

Pinned to **`runs-on: macos-14`** (Apple Silicon) rather than
`macos-latest`. The target triple is `aarch64-apple-darwin`; the runner
host architecture must match for the codesigned binary to be loadable
by the kernel on the same architecture. Pinning to `macos-14`
prevents a surprise GitHub runner-image migration from changing the
signing host out from under us.

Three jobs the workflow does (one job, eight steps):

- Checkout the tag with `fetch-depth: 0` so `git rev-parse refs/tags/<tag>`
  inside the script can resolve.
- Install Rust + cache `~/.cargo` and `./target` keyed on `Cargo.lock`.
- Run `release-mac.sh <tag>` (no `--dry-run` — the workflow always
  enforces the clean-tree + tag-existence checks).
- Upload tarball + `.sha256` as a workflow artifact (retained 90 days)
  AND attach to the release (via `softprops/action-gh-release@v2`) but
  only on `release.published` events; on `workflow_dispatch` re-runs
  we skip the release-attach to avoid overwriting an operator's
  hand-stapled artifact.
- Print a reminder of the manual notarise step at the end.

Has a `workflow_dispatch` input `tag` so the operator can manually
re-run a build for an existing release without unpublishing.
`concurrency` groups runs by tag with `cancel-in-progress: false` —
release builds finish, they don't get cancelled mid-flight.

`permissions: contents: write` is the minimum scope needed by
`softprops/action-gh-release@v2` to attach assets.

### Why notarisation is NOT automated

Apple's notary credentials authorise binaries that the entire Mac
ecosystem trusts. Storing them in GitHub Actions secrets — even
encrypted at rest — moves that trust off the operator's machine onto
GitHub's infrastructure. For Phase 10 (alpha) the operator runs
`xcrun notarytool submit` + `xcrun stapler staple` on their own
machine using credentials configured once via
`xcrun notarytool store-credentials`. Phase 11+ may revisit this
using App Store Connect API key secrets if the operator opts in.

The workflow prints the exact commands the operator runs (file paths
substituted in) so there's no copy-paste-error path.

---

## Local dry-run verification

Ran end-to-end on the agent's machine against `HEAD` of `sash/local-test`
with the stub tag `v0.0.0-test`:

```text
$ bash scripts/release/release-mac.sh v0.0.0-test --dry-run
── release-mac.sh v0.0.0-test (DRY RUN) ──
  repo:     /Users/sash/code/elastos-runtime
  branch:   sash/local-test
  head:     e52b059
  skipping clean-tree + tag-existence checks (--dry-run)

── Build (cargo --release) ──
  binary: …/elastos/target/release/elastos (55,910,384 bytes)

── Sign with Vz/JIT entitlements ──
  …/release-mac/v0.0.0-test/stage/elastos: replacing existing signature

── Verify entitlements ──
  all 4 required entitlements present:
    ✓ com.apple.security.virtualization
    ✓ com.apple.security.cs.allow-jit
    ✓ com.apple.security.cs.allow-unsigned-executable-memory
    ✓ com.apple.security.cs.disable-executable-page-protection

── Smoke check ──
  ✓ --version: elastos 0.2.0-dev
  ✓ vm-debug --help mentions the boot subcommand

── Tarball + SHA256 ──
  tarball: …/release-mac/v0.0.0-test/elastos-v0.0.0-test-aarch64-apple-darwin.tar.gz (21,031,890 bytes)
  sha256:  985d4f56ac0e3e5a5f3810e7d4f5ae0041ddb818f8561e9bf8a59a18afd27c96

── Done ──
```

Build time: ~60 s (warm cache; cold cache is ~3-4 min).
Compressed-to-source ratio: 21 MB / 53 MB = 38 %.

### Round-trip extraction test

The tarball was extracted to `/tmp/` and the binary re-run from there
to prove no path coupling and that the entitlements survive the
tar/gzip cycle:

```text
$ tar -xzf elastos-v0.0.0-test-aarch64-apple-darwin.tar.gz -C /tmp
$ /tmp/elastos-v0.0.0-test-aarch64-apple-darwin/elastos --version
elastos 0.2.0-dev
$ codesign -d --entitlements - /tmp/elastos-v0.0.0-test-aarch64-apple-darwin/elastos
Executable=/private/tmp/elastos-v0.0.0-test-aarch64-apple-darwin/elastos
[Dict]
  [Key] com.apple.security.cs.allow-jit                            [Bool] true
  [Key] com.apple.security.cs.allow-unsigned-executable-memory     [Bool] true
  [Key] com.apple.security.cs.disable-executable-page-protection   [Bool] true
  [Key] com.apple.security.virtualization                          [Bool] true
```

All four entitlements intact after the tarball round-trip.

---

## Linter verification

```text
$ shellcheck scripts/release/release-mac.sh
(exit 0; no findings)

$ actionlint -verbose .github/workflows/release-mac.yml
verbose: Linting .github/workflows/release-mac.yml
verbose: Found 0 parse errors in 2 ms
verbose: Found total 0 errors in 25 ms
(exit 0)
```

Both clean.

---

## What was NOT tested locally (deferred to first real release)

The following paths can only be exercised by a real
`release: types: [published]` event firing the workflow on
GitHub-hosted infrastructure. Each is flagged with the test plan for
when the first real release happens:

1. **`actions/upload-artifact@v4` upload** — local dry-run only
   produces the file on disk; the artifact-upload path is GitHub-API.
   **Test plan:** download the workflow artifact from the Actions UI
   after the first release; verify SHA256 matches what `release-mac.sh`
   printed in the build log.

2. **`softprops/action-gh-release@v2` attach** — only fires on
   `github.event_name == 'release'`. **Test plan:** after publishing
   the first real release, confirm both assets appear on the
   release page; download one and `shasum -a 256` it against the
   workflow log output.

3. **`macos-14` runner availability** — GitHub's Apple Silicon
   runner pool is smaller than the Intel pool; if it's saturated the
   workflow queues but doesn't fail. **Test plan:** monitor first-run
   wait time; if > 10 minutes recurring, look into self-hosted
   alternative (the Phase 5 self-hosted runner spec
   `SELF_HOSTED_RUNNER_SPEC.md` is the starting point).

4. **`xcrun notarytool submit`** — the operator's manual step.
   Not tested by this branch's work at all. **Test plan:** on the
   first real release, run the printed `notarytool submit` command,
   verify Apple returns `Accepted`, then staple + repackage + re-upload.
   Document any friction in `PHASE_11_NOTARISATION_NOTES.md`.

5. **`fetch-depth: 0` checkout cost** — full-history clone is
   bigger than the default shallow. On this repo (~3000 commits)
   the cost is negligible (< 30 s); will need re-measurement if
   the repo grows. **Test plan:** read the "Set up job" step
   timing in the first workflow run.

6. **Bit-reproducibility across runs** — `gzip -n` strips embedded
   mtime, but `tar`'s ustar format records file mtimes from the
   filesystem. The staged binary's mtime is set by `cp -p` to match
   the cargo-built binary, which itself has a fresh mtime per build.
   **Practical impact:** rebuilding the same commit twice will
   produce two tarballs with the same SHA256 only if the build runs
   in the same calendar second. This is a future polish item, not a
   correctness issue.

---

## Out of scope (intentionally not addressed in Day 14)

- **Intel-Mac release lane** — Apple Silicon first per the Phase 6
  scope decision; Intel adds a runner cost + a separate triple
  without operator demand.
- **Linux release lane** — the existing crosvm path handles this;
  out of the Mac-substrate phase scope.
- **Homebrew formula** — Phase 11 work; needs a stable, notarised
  release to point the formula at first.
- **Auto-update mechanism** — Phase 11 work; needs a release feed
  to subscribe to first.
- **An actual notarised release** — this is engineering, not a
  release event. The first real release happens when the operator
  decides to cut one; this branch makes that one-button.

---

## File-level verification commands the reviewer can re-run

```bash
# Lint
shellcheck scripts/release/release-mac.sh
actionlint .github/workflows/release-mac.yml

# Re-run the dry-run end-to-end
bash scripts/release/release-mac.sh v0.0.0-test --dry-run

# Inspect the produced tarball
tar -tzf elastos/target/release-mac/v0.0.0-test/elastos-v0.0.0-test-aarch64-apple-darwin.tar.gz

# Extract + re-run from elsewhere to confirm no path coupling
tar -xzf elastos/target/release-mac/v0.0.0-test/elastos-v0.0.0-test-aarch64-apple-darwin.tar.gz -C /tmp
/tmp/elastos-v0.0.0-test-aarch64-apple-darwin/elastos --version
codesign -d --entitlements - /tmp/elastos-v0.0.0-test-aarch64-apple-darwin/elastos
```

---

## Phase 10 status (post-Day 14)

- ✅ Day 1 — CVE audit + ownership classification + handoff
- ✅ Day 2-3 — Mac threat model
- ✅ Day 4-8 — Carrier-bridge `cargo-fuzz` harness
- ✅ Day 9-10 — SIGINT/SIGTERM graceful shutdown + test-binary auto-resign
- ✅ Day 11-13 — External-review packet
- ✅ **Day 14 — GitHub Actions Mac release lane** (this document)
- ⏸ Day 15 — Phase 10 sign-off (`PHASE_10_SIGNOFF.md`)
