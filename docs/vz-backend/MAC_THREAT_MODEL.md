# Mac substrate threat model

> **Audience:** an engineer or external security reviewer who wants to
> understand, in one document, where every trust boundary in the macOS
> compute substrate lives, what enforces it, and what would constitute a
> compromise.
>
> **Scope:** the `elastos-vz` substrate, the Mac-specific bootstrap and
> signing pipeline, and the Carrier-bridge framing parser (the
> trust-boundary code we co-own with the Linux substrate). Cross-platform
> code paths (supervisor, gateway, capability manager, identity, IPFS pull)
> are covered only insofar as they enforce a Mac-relevant boundary.
>
> **Status:** Phase 10 Day 2-3 deliverable. Single source of truth for
> Mac-substrate threat-modelling on the `sash/local-test` branch.
>
> **Anchors:** code line references throughout point to branch HEAD as of
> the commit that ships this file.
>
> **Companion docs:** `RUNTIME_CVE_HANDOFF.md` (inherited CVE inventory),
> `PHASE_10_PLAN.md` (working day-by-day), `BRANCH_SUMMARY.md` (high-level
> branch overview).

## Trust posture, in one paragraph

ElastOS on macOS is a capsule-execution substrate. The host runs a
*supervisor process* (`elastos-server`) that launches capsules, each in its
own hardware-isolated virtual machine (microVM capsules) or in a wasmtime
JIT sandbox (WASM capsules). Capsules cannot communicate at the network
layer (NAT-only, no L2 between guests); they can only exchange information
by routing JSON-framed requests through a **Carrier bridge** the supervisor
mediates, where capability checks gate every operation. The supervisor
runs as the unprivileged operator's user account; it does not require
root and acquires elevated capability solely via the macOS code-signing
entitlement system. Trust ultimately rests on three foundations we do not
implement: Apple's hardware virtualization isolation (Vz +
`Hypervisor.framework`), Apple's code-signing enforcement (Gatekeeper +
hardened runtime), and the soundness of wasmtime's WASM sandbox.

## Stated principles

1. **No silent privilege downgrade.** If a capsule requests a privileged
   resource the runtime cannot provide (e.g., bridged networking without
   the `com.apple.vm.networking` entitlement), the runtime returns a typed
   error. It never substitutes a less-privileged version (e.g., NAT) and
   continue. See `elastos-vz/src/ffi/builder.rs:137-152`.
2. **Single inter-capsule channel.** All capsule-to-capsule communication
   flows through the Carrier bridge. The substrate does not configure
   secondary side-channels (no shared filesystems, no shared memory, no
   inter-VM network paths).
3. **Minimum entitlement surface.** The release entitlements plist
   (`scripts/release/elastos-server.entitlements.plist`) carries exactly
   what is needed to operate the substrate. Each entry is annotated with
   its justification in-line.
4. **Default-deny network.** Every microVM is NAT'd by default
   (`builder.rs:138`). Bridged networking requires both a capsule-side opt-in
   (`permissions.guest_network: true` in the manifest) *and* the
   `com.apple.vm.networking` entitlement on the host binary.

## The eight trust boundaries

### TB1 — Host operator ↔ supervisor process

| | |
|---|---|
| **Parties (untrusted ↔ trusted)** | Local user processes / shell sessions ↔ the `elastos-server` (supervisor) and `elastos-vz` (substrate) binaries running under the operator's account. |
| **What the attacker can attempt** | The operator (or any local process running as them) can: read on-disk state at `~/Library/Application Support/elastos/`; signal/kill the supervisor; modify the operator-owned manifest in `components.json`; connect to the runtime API on `127.0.0.1:<port>`; replace binaries on disk and restart. |
| **Enforcement mechanism** | macOS file-system permissions (operator owns their `~/Library/Application Support/` tree). The supervisor does not attempt to defend against an attacker who is already the operator — by design, the operator has full control over their own runtime. The relevant defence is **codesigning of the supervisor binary itself**, which prevents *other* processes from masquerading as it: `scripts/release-mac.sh` signs `elastos-server` with the project Developer ID and the hardened runtime, and the bootstrap auto-re-signs after every `cargo build` (`scripts/dev/mac-local-setup.sh`). |
| **What would constitute a break** | A binary not signed by the project Developer ID being accepted by `Hypervisor.framework` as if it carried our entitlements — i.e., a code-signature spoof. This would be an Apple-side breakage, not ours. |
| **Known weaknesses / accepted risk** | The supervisor offers no defence against a malicious operator. This is the *intended* posture: ElastOS is a tool the operator uses, not a sandbox protecting the host from the operator. Multi-user shared-host scenarios are explicitly out of scope for the Mac substrate as of Phase 9. |

### TB2 — Supervisor ↔ Apple Virtualization.framework (entitlement boundary)

| | |
|---|---|
| **Parties (untrusted ↔ trusted)** | The supervisor (caller, untrusted by Apple) ↔ `Virtualization.framework` (Apple-controlled). |
| **What the attacker can attempt** | A malicious supervisor process could attempt to: instantiate a `VZVirtualMachine` without the `com.apple.security.virtualization` entitlement; configure a bridged network device without `com.apple.vm.networking`; access guest memory the framework should isolate; pass crafted `VZVirtualMachineConfiguration` values that bypass framework validation. |
| **Enforcement mechanism** | Apple gates every Vz API call on the running binary's signed entitlements. Without `com.apple.security.virtualization` + `com.apple.security.hypervisor`, `VZVirtualMachine::init` returns an error before any hardware resource is touched. The runtime-side check for the bridged-networking entitlement (`elastos-vz/src/ffi/entitlement.rs:106-161`) queries `SecTaskCreateFromSelf` + `SecTaskCopyValueForEntitlement` and fails closed before the supervisor even constructs the bridged attachment — preventing the opaque `VZErrorInternalError` Apple would otherwise return at `machine.start()` time. |
| **What would constitute a break** | A Vz call succeeding without the required entitlement (Apple-side codesigning regression), or a guest reading host memory the framework asserted was isolated (Vz / `Hypervisor.framework` CVE). |
| **Known weaknesses / accepted risk** | We grant `com.apple.security.cs.allow-jit` + `com.apple.security.cs.allow-unsigned-executable-memory` (required by wasmtime). These weaken the hardened-runtime codesign-enforcement window by allowing RWX pages and unsigned memory regions inside the supervisor's address space. Gated to the single binary that hosts wasmtime; reviewer-relevant tradeoff documented in `BRANCH_SUMMARY.md` Security section. |
| **Apple docs** | [Virtualization.framework entitlements](https://developer.apple.com/documentation/bundleresources/entitlements/com_apple_security_virtualization), [`com.apple.vm.networking`](https://developer.apple.com/documentation/bundleresources/entitlements/com_apple_vm_networking). |

### TB3 — Guest VM ↔ Carrier bridge (the trust-boundary parser we co-own)

| | |
|---|---|
| **Parties (untrusted ↔ trusted)** | Guest userspace inside a microVM (potentially fully compromised) ↔ the supervisor's Carrier-bridge framing parser running on the host. |
| **What the attacker can attempt** | A capsule whose guest userspace has been fully compromised by its own (untrusted) app can send arbitrary bytes over the virtio-console fd (`/dev/hvc1` inside the guest). The host's framing parser reads newline-delimited JSON envelopes; an attacker can attempt: oversized lines (memory exhaustion), malformed JSON (parser DoS), structurally valid but semantically out-of-contract envelopes (capability spoofing), envelopes claiming a different `capsule_id` than the bridge was spawned for (identity spoofing), repeated requests (rate / amplification). |
| **Enforcement mechanism** | (a) The bridge is spawned per-capsule with an immutable `BridgeContext.capsule_id` (`elastos-server/src/carrier_bridge.rs:38`); identity in incoming envelopes is not consulted for routing. (b) Every dispatched request is gated by `CapabilityManager` checks (`BridgeContext.capability_manager`); a guest cannot self-authorize a new capability — requests for unapproved capabilities block on `CAPABILITY_APPROVAL_POLL_MS` cycles up to `CAPABILITY_APPROVAL_MAX_POLLS` (`carrier_bridge.rs:28-29`). (c) The bridge loop uses `tokio::io::AsyncBufReadExt::read_line` which caps line length (oversize-line teardown documented at `carrier_bridge.rs:43-47`). |
| **What would constitute a break** | A guest issuing a request that lands at a provider with a capability it never had granted. A guest crashing or hanging the bridge thread for another capsule (the bridge is per-capsule, so this should be isolated). A guest causing the host parser to allocate unbounded memory or panic. |
| **Known weaknesses / accepted risk** | **THIS PARSER HAS NEVER BEEN FUZZ-TESTED.** This is the single highest-priority Phase 10 item for our branch (Day 4-8). Today the parser relies on `serde_json` for structural validation and an in-place line-length cap; whether the cap is exhaustive across every code path is exactly what fuzzing will tell us. |
| **Anchor for the reviewer** | `elastos-server/src/carrier_bridge.rs:1-120` for setup; the dispatch loop lives further down in the same file. The macOS-specific entry point is `spawn_carrier_bridge_on_stream` (`carrier_bridge.rs:117+`), called by the supervisor on the host-fd half of a `socketpair(AF_UNIX, SOCK_STREAM)` set up in `elastos-vz/src/ffi/console.rs::build_carrier_console_slot`. |

### TB4 — Capsule ↔ capsule (the "must not be possible directly" boundary)

| | |
|---|---|
| **Parties (untrusted ↔ trusted)** | Any capsule ↔ any other capsule. |
| **What the attacker can attempt** | A compromised capsule may attempt: direct network reach to another capsule's guest (L2/L3); shared-memory side-channels; shared-filesystem reads; impersonating another capsule on the Carrier bridge; eavesdropping on another capsule's bridge traffic. |
| **Enforcement mechanism** | (a) **No L2/L3 path exists between capsules.** Every microVM is configured with NAT-only networking by default (`elastos-vz/src/ffi/builder.rs:137-138`). NAT'd VMs cannot see each other on the host's bridge; outbound goes via Apple's NAT and inbound from another capsule is structurally impossible without bridged networking. (b) **No shared memory.** Each `VZVirtualMachine` instance is configured with its own memory region via `setMemorySize` (`builder.rs:167`); Vz does not expose a guest-shared-memory API in the way crosvm's `--shared-dir` does. (c) **No shared filesystem.** Each VM gets its own storage devices (`builder.rs:172-178`); the supervisor does not configure any cross-capsule mounts. (d) **Per-capsule Carrier bridge.** Each bridge is spawned with its own `BridgeContext` (`carrier_bridge.rs:32-53`); a capsule cannot read another's bridge channel because the channel is the host fd half of a per-VM `socketpair`. (e) **No capability transfer between capsules.** Capability grants are per-capsule, mediated by `CapabilityManager`. |
| **What would constitute a break** | Two capsules exchanging arbitrary bytes via any path other than supervisor-mediated provider calls would be a substrate failure. A capsule reading another capsule's memory would be an Apple Vz break. |
| **Known weaknesses / accepted risk** | (1) **Bridged-networking opt-in.** If a capsule's manifest declares `permissions.guest_network: true` *and* the supervisor binary carries `com.apple.vm.networking`, that capsule's VM is bridged onto the host LAN — at which point it can reach other LAN devices (including other bridged capsules). This is by design for capsules that need real network access (e.g., a Tor relay), but reviewers should know the substrate does not enforce "no inter-capsule L2 reach" when bridged mode is opted into. The Phase-9 default builds and the demoed managed-Home flow do not grant this entitlement. (2) **Shared physical CPU + memory bus.** Side-channel attacks (Spectre/Meltdown family, microarchitectural timing) between capsules sharing the same physical CPU are not mitigated by us beyond what Apple Silicon's microarchitecture already provides. |
| **Inherited-CVE weakening** | None at this boundary. |

### TB5 — Operator ↔ runtime API (local 127.0.0.1 gateway)

| | |
|---|---|
| **Parties (untrusted ↔ trusted)** | Any process on the host that can open a socket to `127.0.0.1:<gateway-port>` ↔ the supervisor's Axum HTTP server. |
| **What the attacker can attempt** | A local process — a different app, a malicious browser tab via DNS rebinding, a curl invocation by the operator from a poisoned shell — could: enumerate routes; call provider proxies; mint launch tokens for the Home shell; upload large request bodies (DoS); fetch artifacts from CIDs; impersonate the operator via session cookies; pivot via redirects. |
| **Enforcement mechanism** | (a) **Loopback-only bind.** The gateway binds explicitly on a loopback address; remote network reach is not configured. (b) **Per-request body limits.** `axum::extract::DefaultBodyLimit::max(...)` is applied on every body-accepting route; the single global ceiling is `MAX_GATEWAY_FILE_SIZE = 100 MB` (`elastos-server/src/api/gateway.rs:37`). (c) **Session-based routing.** Browser/home/room sessions are tracked by signed cookies (`HOME_SESSION_COOKIE`, `BROWSER_SESSION_COOKIE`, `ROOM_SESSION_COOKIE` — gateway.rs:39-41). (d) **Time-bounded launch tokens.** Home launch tokens carry a domain and TTL (`HOME_LAUNCH_TOKEN_DOMAIN`, `HOME_LAUNCH_TOKEN_TTL_SECS = 12h` — gateway.rs:43-44). (e) **Capability gating on provider proxies.** The `/api/provider/:scheme/:op` route is mediated by the same `CapabilityManager` that gates Carrier-bridge requests. |
| **What would constitute a break** | Remote network reach to the gateway port (would indicate a binding regression). A request bypassing `CapabilityManager` (would indicate a routing regression). DNS-rebinding pivoting through a browser without a Host-header check (worth verifying — see "Known weaknesses" below). |
| **Known weaknesses / accepted risk** | (1) **No Host-header allowlist verified yet.** A DNS-rebinding attacker who can resolve a hostname to `127.0.0.1` may be able to issue same-origin requests from a malicious browser tab. Need to verify whether `axum` is configured to reject Host headers other than `127.0.0.1` / `localhost`. **Action item for Day 11-13 reviewer.** (2) **Trust-on-first-use for session cookies.** The session-cookie scheme does not include device-binding; a stolen cookie file is replayable. Accepted risk for local-only use; flag for any future remote-gateway work. |
| **Anchor for the reviewer** | `elastos-server/src/api/gateway.rs:62-100` for the route table. |

### TB6 — Upstream component registry ↔ runtime (IPFS pull paths, CID verification)

| | |
|---|---|
| **Parties (untrusted ↔ trusted)** | The component registry / IPFS gateway / network path ↔ the runtime fetcher (`elastos setup` and ad-hoc pulls). |
| **What the attacker can attempt** | A malicious registry, MITM on the network path, or compromised IPFS gateway could serve: tampered capsule artifacts; substituted vmlinux or rootfs; CIDs that resolve to attacker content; oversized responses (DoS). |
| **Enforcement mechanism** | (a) **CID-addressed pulls.** Capsule artifacts are addressed by content hash (CID). After download, the fetcher computes the hash and compares; mismatch is a fail-closed error. (b) **`.elastos-cid` + `.elastos-artifact-sha256` on-disk markers.** Once an artifact is staged, the supervisor's `resolve_plan` re-reads the CID from the marker file (`PHASE_9_DAY_5_NOTES.md` §"Gap A fix"); a tamper with the on-disk content would mismatch the marker. (c) **`components.json` schema-validated.** The manifest of installed components is JSON-schema-validated on load; an attacker who replaces `components.json` cannot insert a structurally-invalid entry that the supervisor would silently accept. (d) **TLS for IPFS gateway connections.** The HTTP client (`reqwest`) is configured with `rustls` (verified at `elastos/Cargo.toml`); MITM requires a chain compromise. |
| **What would constitute a break** | A capsule artifact landing on disk whose contents do not match its CID (would indicate the fetcher's hash check was skipped or wrong). The supervisor consuming a `components.json` entry whose CID does not exist on disk under the corresponding marker. |
| **Known weaknesses / accepted risk** | (1) **Local-dev CIDs are 64 bits.** Dev-mode CIDs stamped by `mac-local-setup.sh` use only the first 16 hex chars of the SHA-256 (`stamp_local_capsule_cid()`). This is fine for cache addressing in a developer's own checkout but not collision-resistant against adversarial inputs. **Production releases use full IPFS CIDs (256 bits).** This is documented in `BRANCH_SUMMARY.md` Security section; a Day 11-13 reviewer should verify the supervisor refuses to consume a local-dev CID at runtime if the binary was signed for release. (2) **TLS chain has open CVEs** (RUSTSEC-2026-0044 / -0046 / -0047 in `aws-lc-sys`; RUSTSEC-2026-0104 in `rustls-webpki`). Inherited from `main`; flagged in `RUNTIME_CVE_HANDOFF.md` Cluster B; fix benefits both Linux and Mac. **This boundary is weakened by inherited CVEs until that cluster lands.** |
| **Inherited-CVE weakening** | YES — Cluster B in `RUNTIME_CVE_HANDOFF.md`. |

### TB7 — WASM guest ↔ wasmtime host (JIT sandbox)

| | |
|---|---|
| **Parties (untrusted ↔ trusted)** | WASM bytecode loaded into a WASM capsule (potentially attacker-controlled) ↔ the supervisor's wasmtime host. |
| **What the attacker can attempt** | A malicious WASM capsule could attempt: escape from the WASM linear memory; out-of-bounds reads through misaligned operations; corrupt or panic the wasmtime host; exhaust host resources via crafted control-flow; abuse WASI calls (file/network) the capsule was not granted; pivot via JIT-emitted machine code. |
| **Enforcement mechanism** | (a) wasmtime's sandbox itself — memory linearity, control-flow integrity, capability-based WASI. (b) Our `CapabilityManager` gates which WASI capabilities a capsule receives, the same way it gates microVM Carrier-bridge requests. (c) The JIT pages are confined to the supervisor process; an escape from wasmtime lands the attacker in the supervisor's address space, not the kernel. |
| **What would constitute a break** | A WASM module reading/writing host memory outside its linear memory. A WASM module crashing or hanging the supervisor process. A WASI call landing at a provider the capsule did not have a capability for. |
| **Known weaknesses / accepted risk** | **MULTIPLE OPEN HIGH/CRITICAL CVEs in wasmtime 17.0.3** (RUSTSEC-2026-0020 CVSS 9.0 "Guest-controlled resource exhaustion in WASI implementations" is directly applicable; RUSTSEC-2025-0118 CVSS 9.0 "Unsound API access to a WebAssembly shared linear memory" is applicable if shared memory is used; multiple Cranelift aarch64 / Winch advisories — RUSTSEC-2026-0096, -0095). Cranelift aarch64 advisories are directly applicable on Apple Silicon. Inherited from `main`; flagged in `RUNTIME_CVE_HANDOFF.md` Cluster A. **This boundary is materially weakened until the wasmtime 17→45 migration on `chore/runtime-cve-hygiene` lands.** A reviewer should mark public-alpha as blocked on Cluster A. |
| **Inherited-CVE weakening** | YES — Cluster A (the biggest cluster) in `RUNTIME_CVE_HANDOFF.md`. |
| **Anchor for the reviewer** | `elastos/crates/elastos-compute/Cargo.toml` (the `wasmtime = "17"` pin). |

### TB8 — macOS kernel ↔ Vz hypervisor (the boundary we depend on but do not implement)

| | |
|---|---|
| **Parties (untrusted ↔ trusted)** | Guest kernel + guest userspace running inside a `VZVirtualMachine` ↔ the macOS host kernel and the hardware. |
| **What the attacker can attempt** | A guest that has fully escaped the Linux kernel (i.e., kernel-mode code execution inside the VM) could attempt: vmexit handler bugs in Vz; hypervisor escapes via Apple Silicon hardware vulnerabilities; passthrough device exploits (we do not configure any). |
| **Enforcement mechanism** | (a) Hardware virtualization on Apple Silicon (`Hypervisor.framework` + virtualization extensions in the SoC). (b) Vz framework-level isolation of vCPU state, EPT/SLAT memory translation, and I/O virtualization. (c) We do not configure GPU passthrough, USB passthrough, or any direct-device-access feature; the only host-accessible interfaces inside the guest are the virtio-block storage, virtio-net NAT NIC, virtio-console for kernel logs, virtio-console for Carrier bridge, and virtio-rng. |
| **What would constitute a break** | A guest gaining code execution outside its VM — i.e., on the host or in another VM. This would be an Apple-side or hardware-side vulnerability. |
| **Known weaknesses / accepted risk** | This boundary is provided by Apple and the SoC. We **have not independently verified**: (1) that Vz zeroes VM memory on `VZVirtualMachine.stop()` (Apple is *believed* to; flagged as Day-11-13 reviewer item in `BRANCH_SUMMARY.md`); (2) that our `setSerialPorts` / `setConsoleDevices` / `setStorageDevices` / `setNetworkDevices` / `setSocketDevices` / `setEntropyDevices` / `setMemoryBalloonDevices` configuration (`builder.rs:178-207`) exposes no additional host-facing attack surface beyond the documented virtio devices. (3) That memory ballooning (`builder.rs:206-207`) does not introduce a side channel via memory-pressure signalling. |
| **Inherited-CVE weakening** | None at this boundary directly. Indirectly: if `Hypervisor.framework` itself ships a vulnerability fix, our binary picks it up automatically when macOS is updated (we don't ship Apple framework code). |

## Trust Boundary Summary Table

| # | Boundary | Untrusted side | Trusted side | Enforcement | Weakened by inherited CVEs? | Fuzz/review status |
|---|---|---|---|---|---|---|
| TB1 | Operator ↔ supervisor | Local processes (as operator) | `elastos-server` binary | macOS file perms + Developer-ID codesign | No | Not in scope for review (intended posture: operator owns runtime) |
| TB2 | Supervisor ↔ Apple Vz | Our `elastos-server` calls | `Virtualization.framework` | Apple entitlement enforcement + our pre-check fail-closed | No | Apple-owned; our pre-check has unit tests |
| TB3 | Guest VM ↔ Carrier bridge | Guest userspace bytes | Host framing parser + `CapabilityManager` | Per-capsule bridge, line-length cap, capability gating | No | **NOT YET FUZZ-TESTED — Phase 10 Day 4-8** |
| TB4 | Capsule ↔ capsule | Any capsule | Any other capsule | NAT-only default, no shared mem/fs, per-capsule bridge | No (unless bridged opt-in) | Architecturally enforced; needs Day-11-13 review of bridged opt-in path |
| TB5 | Operator ↔ runtime API | Local processes on host | Axum gateway on 127.0.0.1 | Loopback bind, body limits, sessions, capability gating | No | **Host-header allowlist needs verification — Day 11-13** |
| TB6 | Upstream registry ↔ runtime | IPFS gateway / network | Runtime fetcher | CID hashing, on-disk markers, TLS via `rustls` | **YES — Cluster B** | Fetcher logic needs Day-11-13 review for CID-mismatch handling |
| TB7 | WASM guest ↔ wasmtime | WASM bytecode | wasmtime host inside supervisor | wasmtime sandbox + `CapabilityManager` on WASI | **YES — Cluster A (largest)** | Public-alpha blocked on Cluster A landing |
| TB8 | macOS kernel ↔ Vz hypervisor | Guest kernel/userspace | Host macOS + Apple Silicon hardware | Apple framework + hardware virtualization | No (Apple ships fixes via OS updates) | Apple-owned; reviewer should sanity-check our `setXxxDevices` config |

## What this document does and does not claim

**Claims we are confident in (anchored in our code):**
- The default network posture for every microVM is NAT-only.
- Bridged networking is gated by both manifest opt-in and binary entitlement, with a typed fail-closed error path.
- There is exactly one inter-capsule communication channel — the Carrier bridge — and it is per-capsule.
- The release entitlements plist contains exactly the entitlements named in this document; no others.
- Capability gating mediates every provider call from both microVM and WASM capsules.

**Claims that depend on Apple framework behaviour (we do not independently verify):**
- Vz enforces guest memory isolation at the hardware level.
- Vz zeroes guest memory on `stop()`.
- Hardened-runtime codesigning prevents binary substitution.
- `Hypervisor.framework` is not exploitable from inside a guest.

**Boundaries that need Phase 10 work before public alpha:**
- TB3 — fuzz the Carrier-bridge framing parser (Day 4-8 on this branch).
- TB5 — verify Host-header allowlist on the gateway (Day 11-13 reviewer item).
- TB6 — wait for Cluster B on `chore/runtime-cve-hygiene` (TLS chain refresh).
- TB7 — wait for Cluster A on `chore/runtime-cve-hygiene` (wasmtime 17→45). **This is the substantive blocker for public alpha.**

**Boundaries that are accepted-risk for the Mac substrate phase:**
- TB1 — operator owns their own runtime; not defending against operator.
- TB8 — Apple-side; we sanity-check our config but cannot audit Vz internals.

## What the external security reviewer should focus on (Day 11-13)

1. **TB3 — Carrier-bridge framing parser.** Read `elastos-server/src/carrier_bridge.rs` end-to-end. Confirm that every read path is bounded, every JSON envelope is validated before dispatch, every dispatched request is capability-checked. The Phase 10 Day 4-8 fuzz harness should be running in parallel; reviewer reads our findings.
2. **TB5 — Gateway host-header / DNS-rebinding posture.** Verify the gateway rejects requests with Host headers that don't match `127.0.0.1` or `localhost`. If it doesn't, recommend an `axum` middleware.
3. **TB6 — CID-mismatch handling.** Verify the fetcher fails closed (does not consume the artifact, does not write the marker, surfaces a typed error) when an IPFS pull returns content whose hash doesn't match the requested CID.
4. **TB2 — Entitlement minimization.** Verify the entitlements in `scripts/release/elastos-server.entitlements.plist` are all genuinely required and that `com.apple.security.cs.allow-jit` is the only JIT-related entitlement granted (no `allow-dyld-environment-variables`, no `disable-library-validation`).
5. **General `elastos-vz` substrate review.** Read `provider.rs` (816 LOC) and `ffi/builder.rs` (543 LOC) end-to-end. The `unsafe` blocks are the highest-leverage attention surface; each has an in-code `// SAFETY:` annotation.

## Threats explicitly out of scope for this document

- Multi-user shared-host scenarios.
- Defending the operator from themselves.
- Side-channel attacks against the Apple Silicon SoC.
- Supply-chain attacks on Apple's own framework code.
- Attacks that require physical access.

## Next steps after this document

- Day 4-8: Stand up the Carrier-bridge fuzz harness (closes TB3's "not yet fuzzed" gap).
- Day 9-10: Demo-bug fixes (do not affect threat model; cosmetic + dev-experience).
- Day 11-13: External reviewer takes this document, the fuzz findings, and reviews the substrate code per the focused list above.
- Day 14: Sign+notarize+smoke CI lane.
- Day 15: Phase 10 sign-off.

## Quotable line for cross-team escalation

> *"The Mac substrate's eight trust boundaries are documented, anchored in
> code, and each is either architecturally enforced today or has a named
> Phase 10 item that closes it. The two boundaries materially weakened by
> inherited CVEs (TB6 TLS chain, TB7 wasmtime sandbox) are handed off to
> the broader runtime team via `RUNTIME_CVE_HANDOFF.md` Clusters A and B;
> public alpha is blocked on those clusters landing, not on Mac-substrate
> work."*
