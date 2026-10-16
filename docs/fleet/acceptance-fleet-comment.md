# Acceptance Fleet Plan (Issue #194)

**Context**: Move installed journeys from GitHub CI (35-75 min per PR, flaky virtual environments) to post-merge acceptance on dedicated hardware fleet, judged externally.

## Journey Coverage by Platform

### Mac ARM64 (`fleet-mac-1`)
1. **Installed journeys** (`scripts/ci-installed-journeys.sh home-repeat`):
   - 3 fresh runs for timing spread (Mac-only per CI job requirements)
   - Model package admission, Carrier Get, runtime reply, process cleanup
   - Expected: 45-50 min (serial due to VZ Browser exclusivity)

2. **Browser VM acceptance** (`scripts/browser-mac-vm-acceptance-handoff.sh`):
   - VZ Browser VM with WebRTC media proof
   - Authenticated profile persistence (ela.city edit-profile flow)
   - VM control restart, profile reset safety
   - Expected: 15-20 min

3. **System update journey** (`scripts/update-hop-compare.py cli_home_system_journey`):
   - Install from oldest-supported release via `install.sh`
   - System updates to current release through UI
   - System refuses tampered offer
   - Account preference written and survives
   - Browser stays open for Undo
   - Expected: ~75 min per CI logs

4. **Undo (rollback)** (continued from System update):
   - Plain `update --yes` refused (older than installed)
   - Explicit `update --rollback-to <head_cid> --yes` succeeds
   - User data preserved, Runtime downgraded to pinned release
   - Home restarts on previous version
   - Expected: included in System update timing

5. **Re-trust** (after update journey):
   - Rerun `install.sh` with new maintainer DID
   - Verifies new signature accepted
   - Old key no longer trusted
   - Expected: 5-10 min

**Mac Total**: ~140-160 min per commit (serial execution)

### Jetson (`fleet-jetson-1`)
1. **Installed journeys ARM64** (`scripts/ci-installed-journeys.sh home`):
   - Native ARM64 model timing
   - Process cleanup, disk reserve
   - Expected: 40-50 min

2. **Browser crosvm proof** (if KVM available):
   - Linux crosvm Browser VM
   - Native ARM64 media proof
   - Expected: 15-20 min (or skip if KVM unavailable)

**Jetson Total**: ~55-70 min

### Linux x86_64 #1 (`fleet-linux-amd64-1`)
1. **Carrier Get journey** (`scripts/ci-installed-journeys.sh home --carrier-get`):
   - Consumer Home with no local package blocks
   - Holder Home provides Kubo peer with package
   - Validates Carrier-only delivery
   - Expected: 50-60 min

2. **Model repair journey** (`scripts/ci-installed-journeys.sh model-repair`):
   - Stop closes engine
   - Get selects different model, Assistant replies
   - Interrupted Get survives Home restart
   - Retry preserves verified prefix, reads remaining bytes
   - Expected: 60-70 min

3. **Finite memory journeys** (`scripts/ci-installed-journeys.sh` with `--finite-memory`):
   - 1 GiB: Admits signed model, refuses Assistant Send before engine spawn
   - 4 GiB: Proves Busy, Stop, idle closure, engine crash recovery
   - Expected: 30-40 min each

4. **Two-runtime acceptance (A side)** (`scripts/home-two-runtime-acceptance.mjs`):
   - Coordinates with Linux #2 (B side)
   - Bilateral discovery, contact exchange, direct/shared messaging
   - Expected: 30-40 min

**Linux #1 Total**: ~200-250 min (can parallelize some with resource isolation)

### Linux x86_64 #2 (`fleet-linux-amd64-2`)
1. **Two-runtime acceptance (B side)** (`scripts/home-two-runtime-acceptance.mjs`):
   - Coordinates with Linux #1
   - Network-separated from #1 for Carrier multi-hop proof
   - Expected: 30-40 min

2. **Public journey smoke** (`scripts/public-user-journey-smoke.sh`):
   - Root site, system/home Camofox, chat room activity
   - Session reuse, guest identity, gateway smoke
   - Expected: 15-20 min

3. **Engine-absent Home** (`scripts/ci-installed-journeys.sh --home-only`):
   - Fresh fixture with no engine bundle
   - Dispatch unavailable (source engine required)
   - Home UI works, model dispatch refused
   - Expected: 20-25 min

**Linux #2 Total**: ~65-85 min

### Linux ARM64 VM (`fleet-linux-arm64-1`) [Fallback]
- Same as Jetson journeys if Jetson unreachable
- Cloud VM or local hardware
- Not run in parallel with Jetson

### Network Holder (`fleet-holder-1`)
- Runs holder Home for Carrier Get tests
- Started by fleet controller before dependent journeys
- Provides Kubo peer, consumer has no local blocks
- Lifecycle: start → ticket → wait → cleanup

---

## Integration with Release Workflow

### Build Acquisition
**From GitHub CI** (recommended):
1. PR CI continues fast smoke tests
2. On merge to develop, GitHub Actions builds release artifacts
3. Fleet controller downloads from `gh run download <run-id>`
4. Verifies checksums against CI receipts (`release-<platform>-<sha>.json`)
5. Distributes to devices via rsync or shared storage

**Artifacts needed per platform**:
- `elastos` binary (Runtime)
- `model-provider` binary
- `components.json` manifest
- Capsule trees
- Engine bundle (llama-server)
- `install.sh` (stamped with maintainer DID, publisher gateway)

### External Judgment
Each device produces:
- JSON receipts: `installed-journeys.json`, `home-journey.json`, `update-observation.json`
- Screenshots: Browser signup, System update UI, error states
- Timing data: Model provider stages, acknowledgements, durations
- Process cleanup receipts: No owned processes remain
- Disk observation: ≥2 GiB reserve maintained

**Pass criteria** (from `scripts/ci-installed-journeys.py` and `update-hop-compare.py`):
- All `"results": {<leg>: "passed"}` entries green
- Process cleanup: `{"status": "passed"}`
- Disk reserve: `available_bytes >= 2GiB`
- Update pin: Matches `scripts/update-hop-previous-release.json`
- Undo: Rollback succeeded, plain update refused
- Two-runtime: All bilateral legs passed (contact exchange, messaging, identity scan)

### Feeding release-publish.sh
Fleet must satisfy `scripts/release-publish.sh prepare` gates:

1. **Source on develop** (checked via `gh api repos/$REPO/compare`):
   - Commit SHA on develop branch
   - CI run completed successfully

2. **Required CI jobs green** (checked against branch protection):
   - All jobs in `.github/workflows/ci.yml` required checks passed
   - No missing or failed jobs

3. **Update journey pin valid** (`checked_update_pin`):
   - `scripts/update-hop-previous-release.json` at commit matches public release head
   - CI started from release users run now

4. **Platform artifacts** (`checked_artifact`, `provenance artifact`):
   - Each platform's package has receipts binding source commit, tree, digest
   - Artifacts uploaded to GitHub Actions (run ID)

5. **Seed Community** (if `RELEASE_COMMUNITY_DIR` set):
   - Custodian's signed Community profile output
   - Bootstrap peer ticket for seed Runtime

**Fleet controller reports**:
- Aggregate pass/fail per device
- Upload consolidated receipts to GitHub Actions artifacts
- Create GitHub Check Run: `acceptance-fleet` (success/failure)
- Link to detailed results (hosted on fleet or GitHub)

---

## Rollout Phases

### Phase 1: Mac Proof (Week 1)
- Set up fleet controller (Python script on Mac or VM)
- Mac runs installed journeys + Browser VM
- Reports to GitHub via Check Run API
- **Acceptance**: One develop commit tested, check visible in GitHub

### Phase 2: Add Linux Basics (Week 1-2)
- Linux #1: Installed journeys, Carrier Get with holder
- Linux #2: Public smoke, engine-absent
- Parallel execution where isolation permits
- **Acceptance**: Mac + 2x Linux all green on one commit

### Phase 3: System Update (Week 2-3)
- Mac System update journey (75 min)
- Undo/rollback validation
- Re-trust after update
- **Acceptance**: Full update→Undo→re-trust cycle proven

### Phase 4: Full Coverage (Week 3-4)
- Add Jetson (or ARM64 VM fallback)
- Two-runtime across network boundary
- Model repair, finite memory journeys
- **Acceptance**: All platforms, all journeys passing

### Phase 5: Release Integration (Week 4-5)
- Fleet output feeds `release-publish.sh prepare`
- Validate against actual release preparation
- **Acceptance**: Signed release published from fleet-tested commit

### Phase 6: Seed Community Bootstrap (Future)
- Seed Runtime with Community network admission
- Community posts across networks proof
- **Acceptance**: TBD based on Community rollout

---

## Failure Handling

1. **Device Offline**:
   - Timeout after 5 min no response
   - Fallback: ARM64 VM if Jetson offline
   - Report as neutral check with comment

2. **Journey Failure**:
   - Retry once automatically
   - Second failure: Mark commit check failed
   - Collect full artifacts (logs, screenshots, receipts)
   - Block develop (or warn, configurable)

3. **Network Partition**:
   - Two-network journeys detect lack of connectivity
   - Fail explicitly rather than timeout
   - Operator alerted

4. **Fleet Controller Down**:
   - Manual intervention required
   - Alternative: Fallback to GitHub Actions (degraded mode)

---

## Weakest Points

1. **Mac VZ serialization bottleneck**: 140-160 min per commit on Mac alone (Browser VM requires exclusive access). Mitigation: Accept serial execution or add second Mac (~$1000).

2. **System update journey time**: 75 min for update+Undo is slow feedback. Mitigation: Run in parallel with other devices, accept that Mac is critical path.

3. **Seed Community scope undefined**: Community bootstrap peer admission journey not yet implemented (no script found). Mitigation: Defer to Phase 6, clarify requirements with OMP.

4. **Re-trust validation incomplete**: No existing script for re-trust journey verification. Mitigation: Extend `install.sh` acceptance with key rotation proof (add to `install-bootstrap-test.py`).

5. **Two-network isolation proof**: Assumes reliable VLAN/physical separation. Mitigation: Pre-test validation (ping fails), post-test Carrier peer logs inspection.

---

## Grounding References

**Installed journeys**:
- `scripts/ci-installed-journeys.sh` (orchestration)
- `scripts/ci-installed-journeys.py` (journey runner, receipts, validation)
- `.github/workflows/ci.yml:915-947` (Linux), `:1169-1187` (Mac)

**System update**:
- `scripts/update-hop-compare.py:3156-3233` (`cli_home_system_journey`)
- `scripts/update-hop-compare.py:3074-3120` (Undo/rollback)
- `scripts/update-hop-previous-release.json` (pinned release)
- `.github/workflows/ci.yml:1029`, `:1343` (update journey timing)

**Browser VM**:
- `scripts/browser-mac-vm-acceptance-handoff.sh`
- `scripts/browser-mac-vm-acceptance-audit.mjs`

**Two-runtime**:
- `scripts/home-two-runtime-acceptance-core.mjs`
- `scripts/home-two-runtime-acceptance.mjs`

**Public journeys**:
- `scripts/public-user-journey-smoke.sh`

**Release integration**:
- `scripts/release-publish.sh:97-113` (update pin check)
- `scripts/release-publish.sh:73-95` (CI and artifact validation)
- `scripts/release-provenance.py` (artifact binding)

**Community**:
- `scripts/release-signer.py:861-933` (bootstrap peer validation)
- `docs/COLLABORATION_NETWORK_PROFILE.md:90` (seed Community node)
- `scripts/release-publish.sh:30`, `:181-183` (`RELEASE_COMMUNITY_DIR`)
