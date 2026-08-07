# Process Manager (`elastos-supervisor`) — POSTPONED

Date: 2026-07-14 · Postponed: 2026-07-15
Status: **postponed — do not implement.** Revisit only after the ESP/System implementation
lands. The original design (ports/adapters supervisor core, two-tier restart policy,
launch-token secret delivery, control protocol, `elastos monitor` TUI, 7 delivery slices) is
withdrawn; it is preserved in git history at commit `c937350` for reference.

## 1. Decision

Review feedback from the parallel ESP work (2026-07-15) established that the overlap with
ESP is **substantial, not merely potential**, and that shipping this spec would create a
second lifecycle authority. We postpone the feature until ESP/System is done, then rebuild
it in the shape §3 records — beneath the existing Runtime supervisor, not beside it.

## 2. Review findings (recorded so they are not re-litigated later)

**The overlap.** The repository already has a Runtime supervisor
(`elastos/crates/elastos-server/src/supervisor.rs` — capsule/VM/carrier-provider lifecycle,
liveness probes, reap ordering), Runtime service facts, Home CLI debug services,
Runtime-owned PTY lifecycle, and a canonical audit chain. A second `elastos-supervisor` with
its own socket, registry, protocol, and monitor would produce **two lifecycle truths**.

**Security contradictions in the withdrawn design:**

1. **The inherited-FD prohibition contradicted itself.** Invariant 1 banned secrets on
   env/argv/inherited fd, yet the §9 threat model conceded an inherited fd is what closes
   the same-UID token race. A **purpose-created pipe/socketpair FD is safer and simpler**
   than placing a capability token in argv or environment. The correct invariant is: no
   secret in env or argv, no *ambient* inherited fds — a purpose-created delivery fd handed
   only to the child IS the mechanism, not a violation.
2. **"The supervisor holds no authority" (old §11.6) was inaccurate.** A supervisor that
   holds and releases the caller seed possesses authority — it is a trust-path participant
   for credential delivery and must be modeled (and audited) as one.
3. **Socket mode `0600` does not verify peer identity.** Any control or secret-delivery
   socket needs real peer credentials: `SO_PEERCRED` on Linux, `getpeereid` on macOS.
   File mode alone was underspecified in both `LocalControlSurface` and the redeem endpoint.
4. **`capsule_watchdog` is not present on the ESP branch** — the default
   `OsProcessTarget` adapter depended on machinery the target substrate does not carry.
5. **Gateway-owned orchestration conflicts with the agreed gateway-as-edge boundary.** The
   design had `elastos gateway` building and owning the supervisor; the ESP direction keeps
   the gateway an edge, not an orchestrator.

## 3. The agreed future shape (build this after ESP/System)

A small **managed-native-service facility beneath the existing Runtime supervisor** — not a
parallel supervisor. Division of labor per the PO/CTO clarification (2026-07-15): **the
Runtime owns lifecycle and authority; System/ESP displays facts and requests audited actions**
— ESP is a projection + request surface, never the truth-owner.

- **Lifecycle facts** are owned by the Runtime and *displayed* through **ESP/System**.
- **Lifecycle events** flow through the **canonical audit chain**, not a bespoke log.
- **Mutations** (start/stop/restart) come through the **existing Runtime authority path** —
  ESP may *request* them (audited); it never executes them. No second control socket,
  registry, or verb protocol.
- The `htop`-style monitor becomes a **projection** of ESP/System-displayed facts — a
  read-side view, never another control authority.

Two ideas from the withdrawn design worth carrying over when this is rebuilt:

- **The tier invariant:** anything restartable holds no authority; anything holding
  authority (crypto/decrypt boundaries) is observe-only — a crash there *is* a failed open,
  and only the request path may re-run authorization.
- **Fail-closed credential delivery:** a child that cannot obtain its credential never
  serves — via a purpose-created pipe/socketpair FD (per finding 1), not env/argv/token.

## 4. Open security item that does NOT wait for ESP

**PO/CTO confirmed explicitly (2026-07-15):** "ESP/System should not put the bug fixes on
hold. The CEK commitment, content pin/provide, diagnostics, and caller-seed exposure should be
fixed independently now." This section is therefore a directive, not a recommendation.

The confirmed caller-seed leak that motivated §1 of the withdrawn design is still live and
is independent of the supervisor question:

> The dKMS **caller seed** (the credential the geo nodes allow-list; the identity that
> authorizes every recover) is exported as `ELASTOS_DDRM_QUORUM_CALLER_SEED_B64` by
> `run-creator-gateway.sh`, readable from the process environment (`ps -E`) and inherited
> by every child the gateway spawns.

Postponing the supervisor must not silently postpone this fix. The minimal standalone
remediation is exactly the review's recommendation: pass the seed over a **purpose-created
inherited pipe/socketpair FD** (or a `0600` file path read-then-zeroized), never the
environment. Small, script + one consumer change, no supervisor required. Track it with the
program's bug list (alongside P0 CEK-commitment and P1 IPFS pin/provide).

## 5. Program-ordering consequence

The dKMS improvement program ordering loses its step 1; the t-of-n / pool-custody bump
(`2026-07-14-dkms-tofn-pool-custody-design.md`) proceeds directly after the P0/P1 bug
fixes and does not depend on anything here.
