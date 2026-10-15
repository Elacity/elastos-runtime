# ElastOS Acceptance Fleet Design

**Status**: Design Proposal  
**Issue**: #194  
**Author**: Cloud Agent (OMP handoff, 15 Oct 2026)  
**Version**: 1.0

## Executive Summary

This document specifies how Mac, Jetson, Linux x86_64, Linux ARM64, two networks, and a phone will run the installed journeys on every develop build, judged externally from outside the device. Currently, installed journeys run inside GitHub CI on every PR (slow and flaky); this plan moves them to post-merge on develop with dedicated physical hardware.

**Key Decision**: Fleet-based post-merge acceptance on develop builds, with external judgment collected from each device and aggregated into pass/fail signals visible in GitHub.

## Goals and Non-Goals

### Goals
- Run complete installed journeys on every develop merge
- Cover all supported platforms: macOS ARM64, Linux x86_64, Linux ARM64, Jetson
- Verify behavior across two networks (simulating real-world Carrier connectivity)
- Include phone integration tests
- External, observable judgment (not self-reported from within test)
- Results visible in GitHub (status checks, artifacts, or dashboard)
- Faster feedback than current 35-45 minute CI journeys
- Reduce GitHub Actions compute costs

### Non-Goals
- Replace PR-time smoke tests entirely (some quick checks may stay in PR CI)
- Support arbitrary developer machines (fleet is operator-controlled)
- Run on every commit (only merged develop commits)
- Windows support (not in current platform matrix)

## Current State Analysis

### Existing Test Infrastructure

Based on code review of the current implementation:

**CI Installed Journeys** (`scripts/ci-installed-journeys.sh`, `scripts/ci-installed-journeys.py`):
- Runs on GitHub Actions (Linux x86_64, Linux ARM64, macOS ARM64)
- Takes 35-45 minutes on Mac, 30-40 minutes on Linux
- Tests: Home signup, model package admission, Carrier Get, runtime reply, process cleanup, disk reserve
- Creates isolated Home state per run
- Collects screenshots, JSON receipts, timing data
- Flaky due to GitHub Actions environment variability

**Two-Runtime Acceptance** (`scripts/home-two-runtime-acceptance*.mjs`):
- Tests bilateral discovery, contact exchange, messaging
- Requires two distinct Runtime instances
- Uses fresh fixture Homes with controlled state
- Validates identity projection, conversation lifecycle
- Currently runs in CI with virtual environments

**Public Journey Smoke** (`scripts/public-user-journey-smoke.sh`):
- Root site, system/home Camofox smoke, chat room activity
- Gateway smoke tests with real browser interaction
- Session reuse, guest identity, runtime activity validation

**Mac VM Acceptance** (`scripts/browser-mac-vm-acceptance*.sh`):
- macOS VZ-based Browser VM testing
- Virtual auth profiles, WebRTC media proof
- Requires Mac hardware for VZ framework
- Handoff process with manual UX validation

**Journey Register** (`docs/audits/ElastOS-Home-Journey-Audit.xlsx`):
- Historical test input and acceptance criteria
- Catalogs expected journeys and verification steps
- [Inference: likely contains test cases to migrate to fleet]

### Current Pain Points
1. **CI Duration**: 35-45 minutes blocks PR merges
2. **Flakiness**: Virtual environments, network timing, resource contention
3. **Cost**: Heavy GitHub Actions usage
4. **Coverage Gaps**: No real Jetson hardware, limited network topology testing
5. **Feedback Loop**: Developers wait for slow CI before knowing if merge is safe

## Architecture Design

### Fleet Topology

```
┌─────────────────────────────────────────────────────────────┐
│  GitHub Develop Branch                                       │
│  ├─ Merge Event Webhook                                      │
│  └─ Commit SHA: abc123                                       │
└────────────────┬────────────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────────────────┐
│  Fleet Controller (Single VM or Mac Mini)                    │
│  ├─ Receives webhook from GitHub                             │
│  ├─ Dispatches jobs to device queue                          │
│  ├─ Collects results from devices                            │
│  ├─ Aggregates pass/fail                                     │
│  └─ Reports back to GitHub (commit status / check run)       │
└────────┬──────┬─────┬─────┬──────┬───────────────────────────┘
         │      │     │     │      │
    ┌────┴┐  ┌──┴──┐ │  ┌──┴───┐ │
    │     │  │     │ │  │      │ │
    ▼     ▼  ▼     ▼ ▼  ▼      ▼ ▼
  ┌───┐ ┌────┐  ┌────┐  ┌──────┐  ┌────────┐
  │Mac│ │Jet-│  │x64 │  │ARM64 │  │Network │
  │   │ │son │  │Lin │  │Linux │  │Holder  │
  └───┘ └────┘  └────┘  └──────┘  └────────┘
                                   (Phone TBD)
```

### Device Roles

| Device | Hostname/ID | Journeys | Concurrency | Notes |
|--------|-------------|----------|-------------|-------|
| **Mac ARM64** | `fleet-mac-1` | Browser VM, Mac source-home, installed journeys, model timing series (3 runs) | 1 | Requires exclusive lease for VZ Browser VM |
| **Jetson** | `fleet-jetson-1` | Linux ARM64 installed journeys, Browser crosvm proof, microVM chat | 1 | Real hardware for ARM acceptance |
| **Linux x86_64** | `fleet-linux-amd64-1` | Installed journeys, Carrier holder, two-runtime acceptance (A side) | 2-3 | Can run multiple isolated Homes |
| **Linux x86_64 #2** | `fleet-linux-amd64-2` | Two-runtime acceptance (B side), public journey smoke | 2-3 | Network isolation from #1 |
| **Linux ARM64 VM** | `fleet-linux-arm64-1` | ARM64 installed journeys (if Jetson unavailable) | 1 | Cloud VM or local hardware |
| **Network Holder** | `fleet-holder-1` | Separate Carrier peer for Carrier Get tests | N/A | Isolated Kubo, no package locally |

**Phone Integration** [Inference: Requires clarification on phone role]
- If phone is for Carrier network testing: use as additional peer
- If phone is for mobile app acceptance: requires mobile build and deployment mechanism
- Proposed: Start without phone, add in Phase 2 after core fleet proven

### Journey Distribution

#### Per-Device Journey Matrix

**Mac (`fleet-mac-1`)**:
- `scripts/ci-installed-journeys.sh home-repeat` (3 fresh runs for timing spread)
- `scripts/browser-mac-vm-acceptance-handoff.sh` with authenticated profile
- `scripts/public-user-journey-smoke.sh` (Mac-specific System/Camofox)
- Source-home setup and model package admission
- Expected duration: 45-60 minutes (serial due to VZ exclusivity)

**Jetson (`fleet-jetson-1`)**:
- `scripts/ci-installed-journeys.sh home` (ARM64 native)
- Browser crosvm proof (when KVM available)
- Model timing with ARM64 engine
- Expected duration: 40-50 minutes

**Linux x86_64 #1 (`fleet-linux-amd64-1`)**:
- `scripts/ci-installed-journeys.sh home` with Carrier Get
- `scripts/home-two-runtime-acceptance.mjs` (A side)
- Model repair journey (Stop, Get different model, interrupted Get/restart/Retry)
- Finite memory journeys (1 GiB refusal, 4 GiB lifecycle)
- Expected duration: 50-70 minutes (can parallelize Home isolation permitting)

**Linux x86_64 #2 (`fleet-linux-amd64-2`)**:
- `scripts/home-two-runtime-acceptance.mjs` (B side, coordinates with #1)
- `scripts/public-user-journey-smoke.sh`
- Engine-absent Home journey
- Expected duration: 30-40 minutes

**Network Holder (`fleet-holder-1`)**:
- Runs holder Home for Carrier Get tests
- Started by fleet controller before dependent journeys
- Provides Kubo peer with pinned package, consumer has no local blocks
- Lifecycle: start → provide ticket → wait for consumer Get → cleanup

**Linux ARM64 VM (`fleet-linux-arm64-1`)** [Inference: Backup for Jetson]:
- Same as Jetson journeys if Jetson unreachable
- Fallback only, not parallel

### Build Acquisition

**How Builds Get to Devices**:

1. **Trigger**: GitHub webhook on develop merge → Fleet Controller
2. **Source Acquisition**: Fleet controller clones/fetches exact commit SHA
3. **Build Strategy** (choose one per design review):

   **Option A: CI Pre-built Artifacts** (Recommended)
   - PR CI continues to build and test (fast smoke tests only)
   - On merge to develop, GitHub Actions builds release artifacts
   - Fleet controller downloads artifacts from GitHub Actions
   - Verifies checksums against CI receipts
   - Distributes to devices via secure copy or shared NFS/SMB mount
   - **Pros**: Reuses existing build infrastructure, no fleet build capacity needed
   - **Cons**: Still uses GitHub Actions for builds (but not journeys)

   **Option B: Fleet-Local Build**
   - Fleet controller or designated build machine compiles from source
   - Requires Rust toolchain, build dependencies on controller
   - Distributes built artifacts to test devices
   - **Pros**: No GitHub Actions compute for builds
   - **Cons**: Build machine capacity, toolchain maintenance, slower feedback

   **Option C: Hybrid**
   - Fast PR CI builds target-QA packages (existing, LTO disabled)
   - Fleet builds release-profile artifacts from develop SHA for acceptance
   - **Pros**: PR feedback stays fast, fleet tests exact shipping artifacts
   - **Cons**: More complex, two build profiles to maintain

   **Recommendation**: Start with **Option A**. CI builds on merge to develop are acceptable (only ~1-2/hour during active development). Fleet focuses on thorough acceptance testing. Transition to Option C if build costs become prohibitive.

4. **Installation**:
   - Each device has `$FLEET_DATA_ROOT/<commit-sha>/` directory structure
   - Controller deploys: `elastos` binary, `model-provider`, `components.json`, capsules, engine bundle
   - Devices verify installed checksums against manifest
   - Fresh `HOME` and `XDG_DATA_HOME` per test run (existing pattern from `ci-installed-journeys.py`)

5. **Model Inputs**:
   - Pinned model packages stored on controller or shared storage
   - Path: `$FLEET_SHARED_STORAGE/model-inputs/{qwen-inputs,signed-packages}/`
   - Symlinked or copied to device test environments
   - Matches `CI_MODEL_INPUTS` pattern from existing scripts

### Results Collection and Judgment

**External Judgment Mechanism**:

Each device produces:
- JSON receipts (e.g., `installed-journeys.json`, `home-journey.json`)
- Screenshots (`.png` files from Browser interactions)
- Timing data (model provider stages, acknowledgements)
- Process cleanup receipts
- Disk observation records

**Collection Flow**:
1. Device runs journey script (e.g., `ci-installed-journeys.sh summary`)
2. Script exits with 0 (pass) or non-zero (fail)
3. Device agent uploads artifacts to fleet controller:
   - SSH/rsync to `$FLEET_RESULTS/<commit-sha>/<device-id>/`
   - Or writes to shared NFS mount
4. Fleet controller waits for all devices to report (timeout: 90 minutes)
5. Controller aggregates results:
   - Parses JSON receipts for `"results": {...}` pass/fail per journey leg
   - Checks process cleanup, disk reserve, timing spread
   - Validates external contracts (e.g., Carrier Get blocks absent from consumer)

**Judgment Criteria** (from `ci-installed-journeys.py`):
- `home_screenshots`: passed
- `model_package_admission`: passed
- `carrier_get`: passed (if applicable)
- `installed_runtime_reply`: passed
- `model_timing_observer`: passed
- `process_cleanup`: passed (no owned processes remain)
- `disk_reserve`: passed (≥2 GiB free)
- `engine_absent_home`: passed (for engine-absent journey)
- `finite_memory_boundary`: passed (for memory-constrained tests)
- `model_repair`: passed (Stop, Get different, retry)

**Reporting Back to GitHub**:
- Fleet controller creates GitHub Check Run via API
  - Name: `acceptance-fleet-<device-id>`
  - Status: queued → in_progress → completed
  - Conclusion: success | failure | neutral
  - Output: summary table, links to artifacts
- Or: Commit Status API (simpler, less detailed)
  - State: pending → success | failure
  - Target URL: Link to fleet dashboard or artifact storage
- Artifacts uploaded to GitHub as workflow artifacts (if using GitHub Actions for coordination) or hosted on fleet web server

**Failure Handling**:

1. **Device Unreachable**: 
   - Timeout after 5 minutes of no response
   - Mark device as offline
   - Fallback: If Jetson offline, use ARM64 VM
   - Report as neutral check with comment: "fleet-jetson-1 offline, skipped ARM64 native test"

2. **Journey Failure**:
   - Collect full artifacts (logs, screenshots, receipts)
   - Mark commit check as failed
   - Annotate with specific failing leg (e.g., "carrier_get: failed")
   - Block develop if critical journey fails (configurable threshold)

3. **Flaky Test**:
   - Retry once on same device (automatic)
   - If second failure, mark as failed (do not mask real bugs)
   - Track retry rate per journey/device for health monitoring

4. **Fleet Controller Failure**:
   - Manual intervention required
   - Develop merges queue up
   - Alternative: Fallback to GitHub Actions journeys (degraded mode)

5. **Network Partition**:
   - Two-network journeys detect lack of connectivity
   - Fail explicitly rather than timeout
   - Operator alerted to fix network

### Cost and Hardware

**Existing Hardware Assumptions** [Inference: Based on AGENTS.md references]:
- 1x Mac (M-series for VZ) - already used for development
- 1x Jetson (Orin or Xavier) - already in inventory
- 2x Linux x86_64 machines (workstation or server-class)
- 1x Linux ARM64 (cloud VM acceptable, ~$50/month for 8-core)

**New Hardware Needed**:
- 1x Fleet Controller: Modest VM or Mac Mini (~$500 one-time or $30/month cloud)
- Network infrastructure: Separate VLANs or physical isolation for two-network tests (~$200 in switches/routers if not already present)
- Storage: Shared NFS or SMB for build artifacts and results (~1TB, $100 one-time or use existing NAS)

**Ongoing Costs**:
- Electricity for always-on devices: ~$50-100/month (estimate)
- Cloud VM for ARM64 fallback: $50/month (if not using physical hardware)
- GitHub Actions reduced usage: **Saves ~$200-500/month** (estimate based on eliminating 35-45 min journeys from every PR)

**Total Estimated Cost**:
- Initial: $500-1000 (if new controller needed)
- Monthly: $100-200 (offset by GitHub Actions savings)
- **Net Monthly**: $0 or slight savings after 2-3 months

### Rollout Order

**Phase 1: Foundation** (Week 1-2)
- [ ] Set up fleet controller VM/machine
- [ ] Install fleet coordination daemon (simple Python service)
- [ ] Configure GitHub webhook → controller
- [ ] Implement single-device proof: Mac runs `ci-installed-journeys.sh home`, reports to GitHub
- [ ] Validate artifact collection and pass/fail judgment
- [ ] **Acceptance**: One develop commit successfully tested on Mac, check status in GitHub

**Phase 2: Expand Coverage** (Week 2-3)
- [ ] Add Linux x86_64 #1: installed journeys, Carrier Get with holder
- [ ] Add Linux x86_64 #2: public journey smoke
- [ ] Coordinate holder startup for network tests
- [ ] Parallel execution for independent journeys
- [ ] **Acceptance**: Develop commit tested on Mac + 2x Linux, all results aggregated

**Phase 3: Full Fleet** (Week 3-4)
- [ ] Add Jetson: ARM64 native journeys
- [ ] Add ARM64 VM fallback
- [ ] Two-runtime acceptance across network boundary (#1 ↔ #2)
- [ ] Finite memory journeys on Linux #1
- [ ] Model repair journey on Linux #1
- [ ] **Acceptance**: All device types reporting, full journey matrix covered

**Phase 4: Production Hardening** (Week 4-5)
- [ ] Failure retry logic
- [ ] Device health monitoring
- [ ] Artifact retention policy (keep last 20 builds, or 30 days)
- [ ] Fleet dashboard (web UI showing device status, recent runs)
- [ ] Alert operator on repeated failures
- [ ] **Acceptance**: 10 consecutive develop commits tested with <5% false-negative rate

**Phase 5: Optimization** (Week 5-6)
- [ ] Parallel journey execution where safe
- [ ] Preemptive build caching
- [ ] Browser VM persistence across runs (if safe)
- [ ] Reduce total fleet time to <30 minutes per commit
- [ ] **Acceptance**: Develop commit feedback in <30 minutes from merge

**Phase 6: Phone Integration** (Future, after Phase 4 stable)
- [ ] Define phone role (Carrier peer? Mobile app? Both?)
- [ ] Mobile build pipeline
- [ ] Device enrollment and management
- [ ] **Acceptance**: TBD based on phone requirements

## Technical Implementation Details

### Fleet Controller Daemon

**Technology Choice**: Python (matches existing CI script language)

**Core Logic**:
```python
# Pseudocode structure
class FleetController:
    def on_webhook(commit_sha, branch):
        if branch != "refs/heads/develop":
            return
        
        job = Job(commit_sha)
        job.fetch_artifacts_from_ci()  # or build locally
        
        # Dispatch to devices (parallel where safe)
        with concurrent.futures.ThreadPoolExecutor() as executor:
            futures = {
                executor.submit(run_device_journey, "fleet-mac-1", job, "mac-journeys"): "mac",
                executor.submit(run_device_journey, "fleet-linux-amd64-1", job, "linux-journeys"): "linux-1",
                executor.submit(run_device_journey, "fleet-linux-amd64-2", job, "linux-smoke"): "linux-2",
                # Jetson waits for holder, so coordinate dependencies
            }
            
            results = {}
            for future in concurrent.futures.as_completed(futures):
                device = futures[future]
                results[device] = future.result()
        
        aggregate_and_report(commit_sha, results)
    
    def run_device_journey(device_id, job, journey_type):
        # SSH to device
        ssh(device_id, f"fleet-agent run {job.sha} {journey_type}")
        # Wait for completion or timeout
        # Collect artifacts
        # Return pass/fail + artifacts path
```

**Device Agent** (on each test machine):
```bash
#!/bin/bash
# fleet-agent.sh run <commit-sha> <journey-type>
set -euo pipefail

COMMIT_SHA="$1"
JOURNEY_TYPE="$2"
WORK_DIR="/opt/fleet/work/$COMMIT_SHA"
ARTIFACT_DIR="/opt/fleet/results/$COMMIT_SHA/$(hostname)"

mkdir -p "$WORK_DIR" "$ARTIFACT_DIR"

# Fetch artifacts from controller
rsync -a controller:/opt/fleet/artifacts/"$COMMIT_SHA"/ "$WORK_DIR"/

# Run journey
case "$JOURNEY_TYPE" in
    mac-journeys)
        CI_SOURCE_HOME="$WORK_DIR/home" \
        RUNNER_TEMP="$ARTIFACT_DIR" \
        scripts/ci-installed-journeys.sh prepare
        scripts/ci-installed-journeys.sh home-repeat
        scripts/ci-installed-journeys.sh summary
        ;;
    linux-journeys)
        # Similar pattern
        ;;
esac

# Upload results
rsync -a "$ARTIFACT_DIR"/ controller:/opt/fleet/results/"$COMMIT_SHA"/$(hostname)/
```

### GitHub Integration

**Webhook Setup**:
- Repository Settings → Webhooks → Add webhook
- Payload URL: `https://fleet-controller.example.com/webhook`
- Content type: `application/json`
- Events: `push` (filter to `refs/heads/develop` in controller)

**Check Run API** (recommended):
```python
def report_to_github(commit_sha, results):
    check_run = github.create_check_run(
        name="ElastOS Acceptance Fleet",
        head_sha=commit_sha,
        status="completed",
        conclusion="success" if all_passed(results) else "failure",
        output={
            "title": "Fleet Acceptance Results",
            "summary": f"{len(results)} devices tested",
            "text": format_results_table(results),
        },
        details_url=f"https://fleet-dashboard.example.com/runs/{commit_sha}",
    )
```

**Artifact Storage**:
- Option A: Upload to GitHub as workflow artifacts (requires Actions workflow stub)
- Option B: Host on fleet web server, link from check run
- **Recommendation**: Option B for simplicity, Option A if GitHub UI integration desired

### Security and Access Control

**Device Access**:
- Fleet controller has SSH keys for all devices
- Devices do not have controller access (results pushed by controller pull, or devices write to shared storage)
- Operator SSH access to all machines for maintenance

**Secrets**:
- GitHub webhook secret (validates webhook authenticity)
- GitHub API token (creates check runs, read-only repo access)
- No production keys on fleet devices
- Test fixtures use isolated keys (existing pattern)

**Network Isolation**:
- Fleet devices on dedicated VLAN or subnet
- Firewall rules permit only: controller ↔ devices, devices → GitHub/CDN (artifact downloads), two-network test isolation
- No inbound public access to devices

## Alternative Designs Considered

### Alternative 1: Keep GitHub Actions, Add Physical Device Triggers
- **Description**: Leave journeys in GitHub Actions CI, add Jetson as self-hosted runner
- **Pros**: No new fleet infrastructure
- **Cons**: Still pays GitHub Actions costs, Jetson exposed as runner (security), doesn't solve two-network isolation, flakiness remains

### Alternative 2: Batched Fleet Testing (Daily or Per-Release)
- **Description**: Run fleet tests once per day or only on release candidates
- **Pros**: Simpler coordination, less hardware wear
- **Cons**: Slower feedback, bugs reach develop before detection, defeats goal of post-merge acceptance

### Alternative 3: Distributed Volunteer Devices
- **Description**: Developers contribute devices, fleet controller dispatches to available pool
- **Pros**: Lower central cost, broader device coverage
- **Cons**: Unreliable availability, security concerns, harder to reproduce issues, coordinator complexity

**Chosen Design Justification**: Dedicated fleet (proposed design) balances cost, reliability, and coverage. Post-merge per-commit testing catches bugs before they propagate while reducing GitHub Actions costs.

## Weakest Points of This Design

### 1. Single Fleet Controller is a SPOF
**Risk**: If fleet controller crashes or becomes unreachable, no acceptance testing occurs until manual recovery.

**Mitigation**: 
- Keep controller simple (Python daemon, systemd supervision)
- Automated health checks (ping every 5 minutes, alert on failure)
- Runbook for manual restart
- Future: Add secondary controller with leader election (not in initial scope)

**Severity**: High impact, medium likelihood. Acceptable for Phase 1-3 with operator on-call.

### 2. No Device Failure During Journey Detection
**Risk**: If a device crashes mid-journey (OOM, kernel panic, power loss), the fleet controller waits until timeout (90 minutes) before declaring failure.

**Mitigation**:
- Heartbeat mechanism: Device agent pings controller every 60 seconds during journey
- Controller detects missing heartbeat → immediate failure, no timeout wait
- Requires implementation in Phase 4

**Severity**: Medium impact (wasted wait time, delayed feedback), low likelihood on stable hardware.

### 3. Two-Network Test Assumes Reliable Isolation
**Risk**: If network isolation breaks (e.g., routing leak, VLAN misconfiguration), Carrier tests may pass incorrectly by taking a direct path instead of P2P.

**Validation**:
- Existing `consumer_package_root_block` check ensures consumer Kubo has no local package blocks → must have come over Carrier
- Pre-test validation: Ping between isolated networks fails
- Post-test: Verify Carrier peer logs show only expected connections

**Severity**: Medium impact (false pass), low likelihood if network validated before each run.

### 4. Mac VZ Exclusivity Serializes Mac Tests
**Risk**: macOS VZ framework requires exclusive access (only one Browser VM at a time). This serializes Mac journeys, creating a bottleneck (45-60 minutes per commit).

**Options**:
- Accept serial execution as initial implementation (Mac is only one of five device types)
- Add second Mac to fleet (expensive: ~$1000 hardware, but enables parallel Mac journeys)
- Optimize Browser VM tests to be stateless and fast-startable (future work)

**Severity**: Medium impact (slower feedback on Mac-specific issues), high likelihood. Not critical if other devices test in parallel.

### 5. Artifact Storage Retention Not Specified
**Risk**: Unbounded artifact storage (screenshots, logs, receipts) grows indefinitely, filling disk.

**Mitigation**:
- Define retention policy: Keep last 20 develop commits OR last 30 days, whichever is longer
- Automated cleanup cron job on controller
- Implement in Phase 4
- Reserve 500GB for artifacts (sufficient for 50+ commits with full journeys)

**Severity**: Low impact (disk space), medium likelihood (will happen without policy).

### 6. Phone Role Unclear
**Risk**: "Phone" in requirements is not defined—is it a Carrier peer, a mobile app test target, or both? Design punts this to Phase 6 without clarity.

**Recommendation**:
- Clarify with OMP: Is phone for mobile app acceptance (requires mobile builds, deployment) or Carrier network testing (simpler, just another peer)?
- If mobile app: Requires separate mobile CI/CD pipeline, app distribution, device enrollment—major scope addition
- If Carrier peer: Add phone as another holder/peer, use Termux or dedicated phone with SSH access

**Severity**: High impact if mobile app (large scope), low impact if just Carrier peer.

## Open Questions for Review

1. **Build Strategy**: Confirm Option A (CI-built artifacts on develop merge) vs. Option C (hybrid) vs. Option B (fleet-local builds).
2. **Phone Role**: Is phone a Carrier peer test device, a mobile app acceptance target, or both?
3. **GitHub Integration**: Commit Status API (simple) or Check Runs API (detailed)? Should artifacts upload to GitHub or stay on fleet web server?
4. **Fallback to GitHub Actions**: If fleet is down, should there be an automatic fallback to CI journeys, or is manual intervention acceptable?
5. **Browser VM Persistence**: Can Browser VM profiles be reused across runs (with reset), or must each journey start from fresh VM image? (Impacts Mac performance.)
6. **Operator On-Call**: What is the SLA for fleet controller recovery? Same-day? Next-business-day?

## Success Metrics

**Phase 4 Acceptance Criteria**:
- ✅ 10 consecutive develop commits tested end-to-end
- ✅ <5% false-negative rate (flaky failures)
- ✅ 0% false-positive rate (real bugs reported as pass)
- ✅ Mean time to feedback: <45 minutes from merge to GitHub check completion
- ✅ Device uptime: >95% (each device available when needed)

**Phase 5 Goals**:
- ⭐ Mean time to feedback: <30 minutes
- ⭐ Device utilization: >70% (not idle waiting for jobs)
- ⭐ GitHub Actions cost reduction: >$200/month vs. baseline

## References

### Existing Scripts (Grounding)
- `scripts/ci-installed-journeys.sh` - Main CI journey orchestration
- `scripts/ci-installed-journeys.py` - Python journey runner with receipts
- `scripts/home-two-runtime-acceptance-core.mjs` - Two-Home bilateral tests
- `scripts/public-user-journey-smoke.sh` - Public-facing journey smoke tests
- `scripts/browser-mac-vm-acceptance-handoff.sh` - Mac Browser VM acceptance
- `scripts/browser-mac-vm-acceptance-audit.mjs` - Mac Browser audit

### Current CI Implementation
- `.github/workflows/ci.yml:915-947` - Installed journey CI jobs
- `.github/workflows/ci.yml:1169-1187` - Mac installed journey (home-repeat)

### Product Contracts
- `AGENTS.md` - Verification gates, journey acceptance criteria
- `docs/audits/ElastOS-Home-Journey-Audit.xlsx` - Historical journey register

### Related Issues
- #194 - CI and tests: warm caches, fail fast, keep only what catches real bugs (parent issue)
- #180 - Home-as-web-page tests (future replacement, not deleted yet)
- #293 - Exact-candidate hardware/media proof (Browser validation)

---

## Reviewer Notes

**For Claude Code (via OMP)**:
- Validate journey distribution across devices makes sense
- Check if Mac VZ serialization is acceptable
- Confirm build acquisition strategy aligns with CI reuse plans

**For Devil's Advocate Pass**:
- Challenge SPOF risks (controller, Mac exclusivity)
- Question whether two-network isolation is reliably testable
- Push on phone definition—is it actually in scope?
- Verify artifact retention won't cause operational issues

**Weakest Points** (as requested):
1. Single fleet controller SPOF
2. Mac VZ exclusivity creates serial bottleneck
3. Phone role undefined (scope risk)
