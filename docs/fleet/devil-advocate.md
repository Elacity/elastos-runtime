# Devil's Advocate Analysis: Acceptance Fleet Plan

## Critical Challenges to the Plan

### 1. **Mac is an Unacceptable Bottleneck**
**Challenge**: 140-160 minutes per commit on the critical path (Mac alone) defeats the goal of "faster feedback." Current CI is 35-45 min for installed journeys. This fleet makes it WORSE, not better.

**Response**:
- Current CI runs Mac journeys once per PR (before merge). Fleet runs once per develop commit (~1-2/hour during active dev, 5-10/day typical).
- Absolute time doesn't matter as much as not blocking PRs. Developers get instant feedback that "your PR will be tested after merge."
- Parallel execution on other devices (Jetson, Linux) provides incremental confidence while Mac finishes.
- **Mitigation**: If Mac time becomes critical, add second Mac for ~$1000 or optimize Browser VM to ~30 min.

**Verdict**: Acceptable for Phase 1-4. Revisit in Phase 5 if develop queue backs up.

### 2. **Update Journey Script Doesn't Exist for Fleet**
**Challenge**: `update-hop-compare.py` is a CI fixture harness, not a fleet-runnable script. It requires:
- Prepared fixture homes (publisher, controller, CLI, operator)
- Signed envelopes pre-staged
- Complex coordination not exposed as a simple "run this journey" command

The fleet design handwaves "run `scripts/update-hop-compare.py`" without showing how.

**Response**:
- `.github/workflows/ci.yml:1029` and `:1343` prove Mac/Linux run this successfully in CI today
- CI job uses `scripts/ci-release-policy-test.py` which orchestrates fixtures
- **Gap Identified**: Fleet needs wrapper script `scripts/fleet-system-update-journey.sh` that:
  - Installs from `install.sh` (signed package)
  - Runs Home, opens Browser
  - Publishes update offers (local publisher fixture)
  - Validates System UI flow
  - Runs Undo
  - This is NEW WORK not accounted for in the plan

**Verdict**: Plan is incomplete. Add Phase 1.5: "Create fleet-runnable update journey wrapper" before Phase 3.

### 3. **Re-trust Has No Proof Script**
**Challenge**: The plan says "rerun `install.sh`" but provides no script that validates re-trust behavior. No grounding reference exists.

**Response**:
- `docs/INSTALL.md:365-377` describes re-trust process
- `elastos/crates/elastos-server/src/sources.rs:828-1041` has unit tests for re-trust logic
- `elastos/crates/elastos-server/src/update.rs:5312-5384` has integration tests
- **Gap Identified**: No end-to-end acceptance script. Fleet needs:
  - Initial `install.sh` with DID_A
  - Home uses source signed by DID_A
  - Rerun `install.sh` with DID_B
  - Validate source now trusts DID_B, refuses DID_A
  - This is NEW WORK

**Verdict**: Plan is incomplete. Re-trust needs acceptance script before Phase 3.

### 4. **Seed Community Journey Undefined**
**Challenge**: Plan punts seed Community to Phase 6 without defining what the journey IS. No script exists. OMP requested it, plan ignores the gap.

**Response**:
- `scripts/release-signer.py:861-933` validates Community bootstrap peer structure
- `scripts/release-publish.sh:181-183` accepts `RELEASE_COMMUNITY_DIR` for signed profile
- `docs/COLLABORATION_NETWORK_PROFILE.md:90` describes seed Community node
- **What's Missing**: No journey script that:
  - Sets up seed Runtime with Community network admission
  - Posts message to Community
  - Validates cross-network relay through seed node
  - Proves Community posts reach members without Discovery on
  - This is NEW WORK

**Verdict**: Plan incomplete. Seed Community needs journey design and script before Phase 6.

### 5. **Release-Publish Integration is Circular Logic**
**Challenge**: Plan says "fleet output feeds `release-publish.sh prepare`" but `prepare` validates CI evidence from `.github/workflows/ci.yml` runs. If journeys move to fleet, where's the CI evidence?

**Current release-publish.sh logic**:
- Line 73-95: Checks for completed successful develop CI run (push or merge_group)
- Line 87-93: Validates all required CI jobs passed
- Update pin check (line 99-113) expects CI ran update journey

**If fleet replaces CI journeys**, what does `release-publish.sh` validate?

**Response**:
- **Option A**: CI continues to run fast smoke tests, fleet runs deep acceptance. `release-publish.sh` validates CI smokes + fleet acceptance artifacts.
- **Option B**: Fleet uploads its receipts as GitHub Actions artifacts, `release-publish.sh` learns to accept "fleet run" as evidence equivalent to CI run.
- **Option C**: Keep minimal CI journeys for release validation, fleet is parallel confidence (doesn't gate releases).

**Gap Identified**: Plan doesn't choose an option. Release workflow must be redesigned.

**Verdict**: Plan incomplete. Must define CI/fleet/release integration before Phase 5.

### 6. **Two-Network Isolation is Unproven**
**Challenge**: Plan assumes "VLAN or physical isolation" but doesn't specify HOW. If misconfigured:
- Carrier tests pass by taking direct LAN path instead of P2P relay
- False confidence in Carrier functionality

**Current validation**:
- `scripts/ci-installed-journeys.py:924-932`: `consumer_package_root_block` check ensures consumer Kubo has no local blocks
- This proves bytes came over network, but not that they came over Carrier vs. direct connection

**Response**:
- **Hardware setup required**: Two physical switches with no inter-VLAN routing, or two separate physical networks
- **Pre-test validation**: `ping` between Linux #1 and #2 must fail
- **Post-test validation**: Carrier peer logs show only expected connections (no direct IP connections)
- **Gap Identified**: Fleet setup documentation must include network topology diagram and validation procedure

**Verdict**: Plan lacks operational detail. Add network setup guide before Phase 2.

### 7. **Parallel Execution Safety Unclear**
**Challenge**: Plan says "can parallelize some with resource isolation" for Linux #1 (200-250 min serial). What's safe to parallelize?

**Conflicts**:
- Carrier Get + Model Repair: Both need exclusive Kubo (port conflicts)
- Finite memory 1GiB + 4GiB: Different memory limits, but can isolate via cgroups
- Two-runtime A side: Needs network coordination with #2

**Response**:
- **Safe parallels**: Finite 1GiB + Finite 4GiB (different Home dirs, cgroup isolation)
- **Sequential required**: Carrier Get → Model Repair (Kubo port reuse)
- **Coordinator-dependent**: Two-runtime waits for #2 readiness signal

**Gap Identified**: Journey dependency graph and safe parallel sets not specified.

**Verdict**: Plan incomplete. Add dependency graph before Phase 4.

### 8. **Failure Retry Logic is Naive**
**Challenge**: Plan says "retry once automatically." What if the retry also fails? What if the first failure was a real bug, and retry passes due to Heisenbug?

**Problems**:
- Automatic retry can mask real bugs (timing-dependent issues)
- No distinction between infrastructure failure (network timeout) vs. product bug (assertion failed)
- Retry on same device doesn't help if device has bad state

**Response**:
- **Better policy**:
  - Infrastructure errors (SSH timeout, disk full, device offline): Retry on same or different device
  - Product errors (assertion failed, process didn't clean up): Report immediately, no retry
  - Classify errors in journey scripts with exit codes: 1=product bug, 2=infrastructure, 3=flaky
- **Gap Identified**: Journey scripts need error classification

**Verdict**: Plan's retry logic too simplistic. Revise before Phase 4.

---

## Unaddressed Questions from Original Feedback

**OMP requested**: "cover the update, Undo, re-trust and seed-Community journeys, install the signed package via install.sh, feed the result to release-publish.sh"

**Plan coverage**:
- ✅ Update journey: Covered (System update from oldest-supported)
- ✅ Undo: Covered (rollback with `--rollback-to`)
- ⚠️ Re-trust: Mentioned but no script exists (NEW WORK)
- ⚠️ Seed Community: Deferred to Phase 6, no journey defined (NEW WORK)
- ✅ Install via install.sh: Update journey uses this
- ⚠️ Feed to release-publish.sh: Integration unclear (needs design)

**Verdict**: Plan addresses 2.5 of 6 requirements. Needs more work.

---

## Alternative: "Start Smaller"
**Devil's Advocate Recommendation**:
Don't build the fleet. Instead:
1. Fix CI flakiness (better timeouts, isolated Homes)
2. Optimize slow journeys (Mac ~45 min → ~25 min)
3. Run heavy journeys post-merge only (not per PR)
4. Keep using GitHub Actions with improved efficiency

**Why this might be better**:
- No new infrastructure to maintain
- No fleet controller SPOF
- Reuses existing CI investment
- Faster to implement (weeks vs. months)

**Why original plan is still better**:
- Hardware isolation eliminates class of flakiness
- External judgment (not self-reported)
- Reduces GitHub Actions costs long-term
- Real-device testing (Jetson) impossible in CI

---

## Final Verdict

**Plan is 60% complete**. Needs before proceeding:

1. **Phase 0: New Work Identification** (1 week)
   - Write `scripts/fleet-system-update-journey.sh` (update+Undo wrapper)
   - Write `scripts/fleet-retrust-acceptance.sh` (re-trust validation)
   - Design seed Community journey (define acceptance criteria)
   - Specify CI/fleet/release integration model

2. **Phase 0.5: Network Setup** (1 week)
   - Document two-network topology
   - Build/configure switches or VLANs
   - Validation procedure (pre-test ping, post-test logs)

3. **Phase 1-6**: As originally planned, but only AFTER Phase 0 complete

**Estimated total time**: 6-8 weeks (was 5-6 weeks in original plan)

**Blocking risks**:
- Update journey wrapper more complex than expected (could be 2-3 weeks alone)
- Release workflow integration requires changes to `release-publish.sh` (needs OMP approval)
- Seed Community journey scope unknown (could be large)

**Recommendation**: Get OMP approval on Phase 0 scope before starting Phase 1.
