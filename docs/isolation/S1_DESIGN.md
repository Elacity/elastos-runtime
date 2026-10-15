# Isolation Stage S1: Home Gets Only the Permissions It Needs

**Status:** Design Draft (Read-only)  
**Issue:** [#173](https://github.com/Elacity/elastos-runtime/issues/173)  
**Stage:** S1 (moved to 0.8.1 per 12 Oct decision)  
**Reviewers:** Irzhy (security), Claude Code via OMP  
**Date:** 15 October 2026

## Executive Summary

S1 establishes the foundation for privilege separation by removing root dependencies, confining providers, securing capsule assets, and limiting Runtime's network exposure. This design draft describes what S1 means concretely, the files it would change, and the tests that would prove it works.

## S1 Scope

S1 comprises four isolation tasks that together ensure Home and providers operate with minimal necessary permissions:

1. **#165: Root-free Browser networking** — Browser Engine runs without sudo, setuid, or file capabilities
2. **#171: Hash-verified capsule assets** — Runtime verifies capsule assets against manifest hashes before use
3. **#166: Provider confinement and keys** — Providers run confined per manifest; keys stay in Runtime or key providers
4. **#183: Runtime as a limited service** — Runtime exposes nothing to the network unless the owner explicitly enables it

## Target T5 Alignment

From Issue #173, T5 states:
> The Runtime runs without any root path at run time (no sudo, setuid or file capabilities); each provider runs confined to what its manifest declares; keys stay in the Runtime or in their key providers (the did provider for the device key, the wallet provider for wallet keys). The Runtime's local control listener and attach secret are reachable only by the Runtime account's own tools, not by providers.

S1 delivers the first three clauses of T5; S2 will complete it with Runtime-owned capabilities and sandboxed Home.

---

## Component 1: Root-Free Browser Networking (#165)

### Current State

Browser networking currently requires privileged operations on Linux:

**Evidence from code:**
- `elastos/crates/elastos-vz/src/provider.rs` lines 1–40: VZ provider uses macOS Virtualization.framework, which requires no sudo
- `scripts/browser-vm-linux-network.py` (referenced in `components.json` line 1540): Linux crosvm networking path
- Issue #173 comment by andersalm (2 Oct): "the Linux Browser engine needs passwordless sudo"

**Inference:** The Linux Browser engine currently uses TAP/bridge networking that requires root to create network interfaces. The macOS VZ path does not have this requirement because Virtualization.framework handles networking internally.

### Target State

Browser networking must work without sudo, setuid binaries, or file capabilities at runtime.

### Design Approach

**For Linux/crosvm:**
1. Replace TAP/bridge with user-mode networking (slirp or similar)
2. Or: Pre-configure network interfaces at install time (one-time sudo with user approval), then use them at runtime without privilege
3. Or: Use Runtime-mediated networking proxy that the Browser Engine connects to via unprivileged sockets

**For macOS/VZ:**
No change needed — Virtualization.framework already handles this.

### Files Changed

**New or modified:**
- `scripts/browser-vm-linux-network.py` — Remove or rewrite to use unprivileged networking
- `elastos/crates/elastos-crosvm/src/provider.rs` — Update crosvm configuration to use user-mode networking
- `elastos/crates/elastos-vz/src/network.rs` (if exists) or equivalent — Document that VZ path is already root-free
- `docs/BROWSER_SUPPORT.md` — Update to document Linux networking approach
- **Inference:** May need to add new `elastos/crates/elastos-server/src/browser_network_proxy.rs` if using Runtime-mediated approach

**Read from codebase:**
- `elastos/crates/elastos-vz/src/provider.rs` (lines 1–150): VZ provider initialization and VM configuration
- `components.json` (lines 1534–1543): browser-vm-linux-network.py component entry

### Tests That Would Prove It

**Negative tests (must refuse):**
1. Attempt to start Browser Engine with sudo — must fail with clear error
2. Check for setuid bits on all Browser-related binaries — must find none
3. Check for file capabilities on Browser binaries — must find none
4. Run Browser Engine as non-root user — must succeed

**Positive tests:**
1. Browser Engine starts successfully without elevated privileges
2. Browser can establish network connections through unprivileged path
3. Multiple concurrent Browser sessions work without network conflicts
4. Browser networking survives Runtime restart

**Test implementation locations:**
- `elastos/crates/elastos-vz/tests/` — Add `browser_networking_unprivileged.rs`
- `elastos/crates/elastos-crosvm/tests/` (if exists) — Add Linux-specific tests
- `scripts/ci-release-policy-test.py` — Add check that scans for setuid/capabilities

---

## Component 2: Hash-Verified Capsule Assets (#171)

### Current State

Capsule loading exists but hash verification coverage is incomplete:

**Evidence from code:**
- `elastos/crates/elastos-server/src/capsule_cmd.rs` lines 84–98: `load_capsule_manifest` reads and validates `capsule.json` structure
- `components.json` lines 100–122: Browser assets include `sha256` and `size` fields for entrypoint and assets
- `elastos/crates/elastos-server/src/capsule_record.rs` (glob result): Likely contains capsule state tracking

**Current verification:** Entrypoint hash is verified (line 95 in components.json shows `entrypoint_sha256`), but:
**Inference:** It's unclear whether all `browser_assets` are verified on load, or only the entrypoint.

### Target State

Runtime verifies every capsule asset (HTML, CSS, JS, WASM, images) against its manifest hash before serving it. Modified or unverified files fail closed.

### Design Approach

1. **At capsule install time:** Calculate and store SHA-256 hashes for all assets
2. **At capsule load time:** Verify entrypoint hash (already done)
3. **At asset serve time:** Before serving any asset from a capsule's browser projection, verify its SHA-256 against the manifest
4. **Cache verified assets:** Once verified, mark the file (inode + mtime) as verified to avoid repeated hashing

### Files Changed

**New or modified:**
- `elastos/crates/elastos-server/src/capsule_record.rs` — Add hash verification state cache
- `elastos/crates/elastos-server/src/api/capsule_asset_handler.rs` (inferred, or modify existing handler) — Add hash verification before serving assets
- `capsules/*/capsule.json` — Ensure all web projections declare complete `browser_assets` lists with SHA-256
- `scripts/capsule-manifest-builder.mjs` or similar (inferred) — Tool to generate/update asset hashes
- `docs/CAPSULE_AUTHORING.md` — Document hash verification requirement

**Read from codebase:**
- `elastos/crates/elastos-server/src/capsule_cmd.rs` lines 84–98: Manifest loading and validation
- `components.json` lines 95, 109–122: Asset hash declaration format

### Tests That Would Prove It

**Negative tests (must refuse):**
1. Modify a capsule asset file after install — Runtime must refuse to serve it
2. Create capsule with missing hash in manifest — install must fail
3. Create capsule with wrong hash in manifest — load must fail
4. Symlink attack: replace asset with symlink to different file — must fail

**Positive tests:**
1. Install capsule with correct hashes — succeeds
2. Launch web projection — all assets serve successfully
3. Verify cache: second access to same asset doesn't re-hash (performance test)
4. Update capsule with new assets and hashes — new version loads correctly

**Test implementation locations:**
- `elastos/crates/elastos-server/tests/capsule_asset_verification.rs` — New test file
- `elastos/crates/elastos-server/src/capsule_cmd.rs` — Add unit tests for hash verification logic
- CI: Add journey test that modifies a capsule asset and verifies refusal

---

## Component 3: Provider Confinement and Keys (#166)

### Current State

Providers run as native OS processes with Runtime user's full rights:

**Evidence from code:**
- `elastos/crates/elastos-runtime/src/provider/bridge.rs` lines 1–100: Provider bridge launches providers as child processes via `tokio::process::Command`
- Lines 40–88: Model provider on macOS has partial seatbelt confinement (`seatbelt_path` function)
- `elastos/crates/elastos-server/src/provider_host.rs` lines 1–100: Native provider hosting for standalone provider processes
- Issue #173: "Providers run as native operating-system processes with the Runtime user's rights. Only the model provider is partly confined."

**Key handling:**
- **Inference:** Currently, providers likely receive keys or have access to key material because they run with full user permissions
- `components.json` lines 260–269: DID provider "required for managed chat identity"
- `components.json` lines 456–467: Wallet provider "for proof challenges, linked-account metadata, and typed wallet authority"

### Target State

Per Issue #173 T5:
> Each provider runs confined to what its manifest declares; keys stay in the Runtime or in their key providers (the did provider for the device key, the wallet provider for wallet keys).

**Concrete requirements:**
1. Each provider runs in a sandbox that grants only declared capabilities
2. File system access limited to declared paths
3. Network access limited to declared destinations or prohibited
4. Keys never leave their owning provider (did-provider for device key, wallet-provider for wallet keys)
5. Other providers never receive raw keys — they request signed operations via Runtime

### Design Approach

**Platform-specific confinement:**

**macOS:**
- Extend existing seatbelt profile from model-provider (bridge.rs lines 48–88) to all providers
- Seatbelt profile declares:
  - Allowed file paths (read/write separately)
  - Network destinations (if any)
  - No access to keys directory except for did-provider and wallet-provider

**Linux:**
- Use `seccomp-bpf` to restrict syscalls
- Or: Use Landlock LSM (Linux 5.13+) for path-based restrictions
- Or: Use `bubblewrap` or similar sandboxing tool
- **Inference:** Need to research which mechanism is most maintainable and provides adequate confinement

**Provider manifest extensions:**
Add to `components.json` provider declarations:
```json
"confinement": {
  "filesystem": {
    "read": ["$DATA_DIR/providers/model-provider/journal"],
    "write": ["$DATA_DIR/providers/model-provider/journal"]
  },
  "network": {
    "allowed_destinations": ["example.com:443"]
  },
  "keys": {
    "owns": ["device_key"],  // only for did-provider
    "requests": []  // other providers list key operations they request
  }
}
```

**Key isolation:**
1. Device key stays in did-provider's exclusive storage
2. Wallet keys stay in wallet-provider's exclusive storage
3. Other providers never see raw keys — they call did-provider or wallet-provider via Runtime for sign operations
4. Runtime validates the operation before forwarding to key provider

### Files Changed

**New or modified:**
- `components.json` — Add `confinement` declarations for every provider
- `elastos/crates/elastos-runtime/src/provider/bridge.rs` — Enforce confinement profiles at provider launch
- `elastos/crates/elastos-runtime/src/provider/confinement/` (new module):
  - `mod.rs` — Confinement abstraction
  - `macos_seatbelt.rs` — macOS seatbelt profile generation and application
  - `linux_seccomp.rs` or `linux_landlock.rs` — Linux confinement mechanism
  - `manifest.rs` — Parse and validate confinement declarations
- `elastos/crates/elastos-runtime/src/provider/registry.rs` (inferred) — Validate provider requests against confinement policy
- `capsules/did-provider/` — Document that it owns device key
- `capsules/wallet-provider/` — Document that it owns wallet keys
- `docs/PROVIDER_CONFINEMENT.md` (new) — Design and operator guidance

**Read from codebase:**
- `elastos/crates/elastos-runtime/src/provider/bridge.rs` lines 40–88: Existing model-provider seatbelt confinement
- `elastos/crates/elastos-server/src/provider_host.rs` lines 1–100: Provider hosting infrastructure
- `components.json` lines 260–290: DID and chain provider declarations

### Tests That Would Prove It

**Negative tests (must refuse):**
1. Provider attempts to read file outside declared paths — blocked by OS, logged by Runtime
2. Provider attempts network connection to non-declared destination — blocked
3. Non-key-provider attempts to read key file — blocked by filesystem permissions
4. Provider attempts to use restricted syscall (Linux seccomp) — blocked
5. Provider attempts to escape confinement via symlink — blocked

**Positive tests:**
1. Provider operates normally within confinement
2. Did-provider successfully signs with device key
3. Wallet-provider successfully signs with wallet key
4. Other provider successfully requests sign operation via Runtime (no raw key access)
5. Confinement survives provider restart
6. Multiple confined providers run concurrently without interference

**Fail-closed requirement:**
- If confinement mechanism unavailable (old kernel, unsupported platform), provider launch must fail with clear error
- No fallback to unconfined operation

**Test implementation locations:**
- `elastos/crates/elastos-runtime/tests/provider_confinement.rs` — New test file
- `elastos/crates/elastos-runtime/src/provider/confinement/tests.rs` — Unit tests per platform
- Platform-specific CI jobs: Verify confinement on macOS and Linux

---

## Component 4: Runtime as Limited Service (#183)

### Current State

Runtime listens on configured addresses and serves HTTP:

**Evidence from code:**
- `elastos/crates/elastos-server/src/main.rs` lines 118–120: `Serve` command with `--addr` (default `0.0.0.0:3000`)
- Lines 439–441: `Gateway` command with `--addr` (default `127.0.0.1:8090`)
- Lines 250–251: `Home` command with `--gateway_addr` (default `127.0.0.1:8090`)

**Current exposure:**
- `0.0.0.0:3000` — Binds to all interfaces, accepts LAN/WAN connections
- `127.0.0.1:8090` — Localhost only by default
- **Inference:** Carrier also opens listening ports for peer-to-peer connections

### Target State

Per Issue #173 T6:
> The front door accepts only expected origins and hosts, nothing sensitive works without sign-in, and nothing listens beyond this device unless the owner turns it on.

**Concrete requirements:**
1. Default: Runtime listens only on 127.0.0.1 (localhost)
2. LAN/WAN listening requires explicit owner approval (System settings or command flag)
3. Carrier P2P: Only listens if collaboration is enabled
4. Public gateway: Only listens if owner explicitly starts it
5. Attach secret: Only accepted from localhost connections (enforced at connection level)

### Design Approach

**1. Secure defaults:**
- Change default `--addr` from `0.0.0.0:3000` to `127.0.0.1:3000`
- Document that `--addr 0.0.0.0:3000` requires `--allow-lan` flag or System settings confirmation

**2. Carrier network control:**
- Add System setting: "Allow incoming peer connections" (default: off)
- When off: Carrier can make outbound connections but doesn't listen
- When on: Carrier listens on configured address with authenticated connections only

**3. Public gateway control:**
- Gateway command requires `--public` flag or tunnel configuration
- Running `elastos gateway` without explicit public flag only binds 127.0.0.1

**4. Attach secret protection:**
- Verify incoming connection is from 127.0.0.1 before accepting attach secret
- Reject attach attempts from non-localhost origins

### Files Changed

**New or modified:**
- `elastos/crates/elastos-server/src/main.rs` lines 118–120: Change default addr to 127.0.0.1, add `--allow-lan` flag
- `elastos/crates/elastos-server/src/serve_cmd.rs` (inferred) — Enforce localhost-only default
- `elastos/crates/elastos-server/src/api/auth.rs` (inferred) — Verify attach secret only from localhost
- `capsules/system/` — Add UI for "Allow incoming connections" setting
- `docs/INSTALL.md` — Document network security model
- `docs/TROUBLESHOOTING.md` — Add section on "LAN access requires --allow-lan"
- **Inference:** May need to modify Carrier initialization to respect network policy

**Read from codebase:**
- `elastos/crates/elastos-server/src/main.rs` lines 118–121, 249–251, 439–442: Current address binding configurations

### Tests That Would Prove It

**Negative tests (must refuse):**
1. Try `elastos serve` without flags — should bind 127.0.0.1 only
2. Try attach secret from remote IP — must fail with "unauthorized"
3. Try to access Runtime API from LAN without `--allow-lan` — connection refused

**Positive tests:**
1. `elastos serve` binds 127.0.0.1 and accepts local connections
2. `elastos serve --addr 0.0.0.0:3000 --allow-lan` binds all interfaces (after confirmation)
3. Attach secret works from localhost
4. Carrier outbound connections work even when incoming disabled
5. Carrier incoming connections work when "Allow incoming" enabled

**Test implementation locations:**
- `elastos/crates/elastos-server/tests/network_binding.rs` — New test file
- `elastos/crates/elastos-server/src/api/auth/tests.rs` — Add attach-secret-from-remote test
- Journey test: Verify default installation only binds localhost

---

## Cross-Cutting Concerns

### Installation Requirements

**One-time sudo at install (acceptable):**
- Installing system service (if needed)
- Pre-configuring network interfaces (if used for #165)
- Installing signed binaries to system paths (if needed)

**Must ask user permission first** and document why sudo is needed.

**Never at runtime:**
No sudo, setuid, or capabilities during normal Runtime operation.

### Verification and Testing Strategy

**Per Issue #173 S0-S2 requirements:**
> Every S0-S2 change needs a regression test for the refused case.

Each component above includes negative tests (must refuse) that prove the isolation boundary holds.

**Recommended test order:**
1. Implement #171 (capsule hash verification) first — foundational, no platform dependencies
2. Implement #183 (localhost-only default) second — quick win, improves security immediately
3. Implement #166 (provider confinement) third — complex, platform-specific
4. Implement #165 (root-free Browser) fourth — depends on #166's confinement infrastructure

### Documentation Requirements

From Issue #173 T7:
> Docs, manifests, website and release notes state exactly what is isolated.

**New/updated docs:**
- `docs/isolation/S1_IMPLEMENTATION.md` — Detailed implementation notes (created after this design is approved)
- `docs/PROVIDER_CONFINEMENT.md` — Provider confinement model and manifest format
- `docs/ARCHITECTURE.md` — Update "Current isolation and target boundary" section
- `docs/INSTALL.md` — Document sudo requirements and network security defaults
- `docs/SECURITY.md` — Update threat model with S1 completions
- `CHANGELOG.md` — S1 isolation milestone entry

---

## Open Questions for Security Reviewer (Irzhy)

### Question 1: Linux Confinement Mechanism
**Context:** Multiple options exist for Linux provider confinement:
- seccomp-bpf (syscall filtering)
- Landlock LSM (path-based, requires Linux 5.13+)
- bubblewrap (namespace-based)
- AppArmor or SELinux profiles (requires distro support)

**Question:** Which mechanism should we use? Criteria:
- Adequate confinement for our threat model (Issue #173, in-scope threats)
- Maintainable (doesn't require learning new policy language)
- Wide platform support (must work on Ubuntu, Fedora, Arch, etc.)
- Fail-closed (must refuse to start if confinement unavailable)

**Recommendation needed:** Landlock seems cleanest for path-based restrictions, but it's relatively new. Should we support multiple mechanisms (probe and use best available) or pick one and fail on unsupported systems?

### Question 2: Key Provider IPC Channel
**Context:** Non-key-providers need to request sign operations without seeing raw keys.

**Current architecture (from code reading):**
- Providers communicate with Runtime via stdio JSON-RPC (`elastos/crates/elastos-runtime/src/provider/bridge.rs`)
- **Inference:** Key operations would go: App → Runtime → Provider → Runtime → Key-Provider → Runtime → Provider → Runtime → App

**Question:** Is this IPC chain acceptable, or do we need a more direct (but still mediated) channel between providers and key-providers? Concern: Latency for signature operations, especially in wallet flows.

**Related:** Should key-providers run in-process (like they might today) or must they be separate processes? In-process is faster but harder to confine.

### Question 3: Browser Networking Privilege Separation
**Context:** Linux Browser needs networking without root.

**Options identified:**
1. User-mode networking (slirp) — slow, but no privileges needed
2. Pre-configured TAP at install time — fast, but requires one-time sudo
3. Runtime-mediated proxy — moderate complexity, Runtime acts as network broker

**Question:** Which approach best balances security and usability? Option 2 (pre-configured TAP) requires trusting the install script and validating the TAP configuration, but it's fastest. Option 1 is simplest but may have performance issues for video streaming.

**Related:** If we use pre-configured TAP, how do we verify it hasn't been tampered with at Runtime start?

### Question 4: Fail-Closed vs. Graceful Degradation
**Context:** S1 confinement mechanisms may be unavailable on some platforms (old kernels, BSD, Windows future).

**Question:** When confinement is unavailable, should we:
- **Option A (fail-closed):** Refuse to start providers, fail with clear error message
- **Option B (warn and continue):** Log a warning, continue without confinement
- **Option C (platform-specific):** Fail-closed on Linux/macOS (where we can confine), warn on others

**Current stance from Issue #173:** "fail-closed per platform" suggests Option A.

**Clarification needed:** Does this mean we simply don't support old kernels? Or do we maintain a compatibility mode for development/testing only (with big warning)?

### Question 5: Carrier Listening Address
**Context:** Carrier needs to accept peer connections for collaboration features.

**From Issue #173:**
> nothing listens beyond this device unless the owner turns it on

**Question:** Should Carrier listening be:
- **Option A:** Disabled by default, enabled via System settings UI only
- **Option B:** Enabled by default on localhost, user can enable WAN via settings
- **Option C:** Always enabled but only accepts authenticated peer connections (current?)

**Related:** What's the UX for "turn on collaboration"? Does joining Community automatically enable Carrier listening, or is it a separate step?

### Question 6: Capsule Hash Verification Performance
**Context:** Verifying SHA-256 on every asset access could be expensive.

**Proposed mitigation:** Cache verified files by (inode, mtime) so we only hash once per file version.

**Question:** Is this cache safe against:
- Inode reuse attacks (file deleted, new file gets same inode)?
- mtime manipulation (attacker with filesystem access sets old mtime)?

**Alternative:** Cache in-memory only (loses cache on Runtime restart but safer)?

### Question 7: Scope of S1 vs. S2 Boundary
**Context:** S1 focuses on provider confinement and root elimination. S2 will add Runtime-owned capabilities and sandboxed Home GUI.

**Question:** Should S1 include any Runtime-level capability enforcement, or is that entirely S2?

**Specific case:** If a provider is confined by S1 but Home can still mint any app token (current state per Issue #173), is that acceptable for S1, or should we include basic capability scoping in S1?

**Current understanding:** S1 is provider confinement, S2 is Home confinement. But clarification would help scope the work.

### Question 8: Testing Approach for Confinement
**Question:** How do we test that confinement actually works?

**Options:**
1. **Adversarial test suite:** Write malicious provider that tries to escape, verify it fails
2. **Audit tooling:** Static analysis of provider binaries to ensure they can't escape
3. **Runtime monitoring:** Log all blocked operations during normal use, verify expected blocks occur
4. **Manual pentesting:** Security review attempts to break confinement

**Current plan:** Option 1 (adversarial test suite per "refused case" requirement) + Option 3 (runtime monitoring).

**Question:** Is this sufficient, or do we need Options 2 and 4 as well before S1 is accepted?

### Question 9: Backwards Compatibility
**Context:** S1 changes may break existing provider binaries or user workflows (e.g., LAN access).

**Question:** Do we maintain compatibility mode, or is this a breaking change?

**Specific cases:**
- Existing providers without confinement manifests — refuse to load, or run unconfined with warning?
- Users who currently use `--addr 0.0.0.0` — break their workflow, or auto-add `--allow-lan`?

**Current stance from Issue #173:** Authority changes require approval, suggesting we can break for security.

### Question 10: Platform Support Matrix
**Question:** Which platforms must S1 support?

**From components.json:** Linux (x86_64, aarch64), macOS (arm64)

**Per-component platform support:**
- #165 (Browser): macOS VZ already root-free; Linux crosvm needs work
- #171 (hashes): Platform-independent
- #166 (confinement): macOS has seatbelt; Linux needs new mechanism
- #183 (network): Platform-independent

**Question:** Is it acceptable for S1 to ship with Browser disabled on Linux until #165 is complete, or must we support all platforms simultaneously?

---

## Summary for OMP and Reviewers

**This design establishes:**
1. Concrete technical approach for each S1 component
2. Specific files that would change (citing actual code paths)
3. Testable acceptance criteria with positive and negative tests
4. Clear fail-closed boundaries (no fallback to insecure operation)
5. Open questions requiring security review before implementation

**What this design does NOT include (by instruction):**
- Actual code changes (read-only on code)
- New test files (only describes what tests would prove)
- Changes outside docs/ (this PR contains only this document)

**Next steps:**
1. Security review by Irzhy addresses open questions
2. OMP approves design and priorities within S1
3. Implementation begins in separate PRs per component
4. Each component merges with its own regression tests and proof

**Timeline inference:**
S1 moved to 0.8.1 per 12 Oct decision, so this design informs post-0.8.0 work.
