# S1 Isolation Design

Issue #173 S1: root-free runtime, confined providers, verified capsule assets, localhost-only default. Three components, two open questions.

---

## #171: Hash-Verified Capsule Assets

**Current:** `components.json` carries `browser_assets` with sha256 per web capsule (lines 105–121, produced by `scripts/stamp-source-home-capsule-metadata.py`).

**Gap:** `api/browser_capsules.rs` serves assets but doesn't verify bytes against hash.

**Design:** Before serving any `browser_assets` path, hash the bytes and compare to `components.json`. Refuse on mismatch. Serve only listed paths (no directory traversal).

**Test:** Edit one Home asset file after components.json is stamped. Launch Home. Asset serve must fail.

---

## #166: Provider Confinement and Keys

**Current:** `server_infra.rs` hands device key to did-provider (line 749), localhost-provider (792), object-provider (892), wallet-provider (1305). Model-provider has partial macOS seatbelt (`provider/bridge.rs:333–393`, `sandbox-exec` at 399); unconfined spawn at 848–861. Other providers run unconfined.

**Gap:** No Linux confinement. No per-provider storage key derivation. Coords file readable by all providers. No stdout cap or stderr rotation.

**Design:**

**macOS:** Deny-default seatbelt for every provider. Model-provider pattern (bridge.rs:333–393): deny network-outbound, deny file-read*/write* except explicit allows. Per provider: allow read/write on `$DATA_DIR/providers/<name>/`, allow read-metadata on ancestors, deny `runtime-coords.json`.

**Linux:** `no_new_privs` + `seccomp` + Landlock (kernel 5.13+). Fail closed if Landlock unavailable. Per provider: Landlock read/write rules on `$DATA_DIR/providers/<name>/`, seccomp denies `ptrace`/`personality`/etc.

**Derived keys:** Each provider gets derived storage key from Runtime-held root. Existing data migration: read under old key, write under derived key, mark migrated.

**Coords exclusion:** Seatbelt/Landlock deny `runtime-coords.json` read for all providers.

**Stdio:** Cap stdout/stderr at 1MB per provider, rotate on overflow.

**Test:** Confined provider binary attempts `open("../runtime-coords.json")`. Must fail with `EACCES` or equivalent, logged by Runtime.

---

## #183: Runtime as Limited Service

**Current:** `main.rs:2053–2060` already binds loopback for product path. Coords 0600 at `runtime_control.rs:583`. `api/gateway_local_control.rs:40` localhost-only.

**Gaps exposing to network:**
- Kubo swarm 0.0.0.0:4001 with mDNS/UPnP, open API (`capsules/ipfs-provider/src/main.rs:705–723` sets only `Addresses.Gateway`)
- Carrier `DEFAULT_CARRIER_BIND_ADDR` 0.0.0.0:4433 (`carrier.rs:1733`)
- `gateway --public` (`gateway_cmd.rs:119`)
- TURN, VM token, local CA

**Design (per 11 Oct plan):**

**Service:** systemd user unit (Linux) and launchd user agent (macOS). `NoNewPrivileges=true`. Binary in data folder. No dedicated account (runs as owner).

**Kubo:** Bind only 127.0.0.1:4001. Disable mDNS and UPnP by default. Gateway API on localhost only.

**Carrier:** Keep 0.0.0.0:4433 listening and mDNS on by default (#309 owns off switch, out of S1 scope).

**Gateway:** `--public` already requires explicit flag. No change needed.

**Test:** Launch Runtime without `--public` or Kubo. `ss -tulpn` shows only 127.0.0.1 binds (except Carrier 4433 per 11 Oct decision).

---

## Open Questions for Security Reviewer

**1. Linux Landlock/seccomp profile shape per provider**

#166 Done means names no_new_privs + seccomp + Landlock on Linux. Landlock (kernel 5.13+) gives path-based read/write rules. Seccomp denies syscalls (`ptrace`, `personality`, `kcmp`, etc.).

**Question:** Which seccomp syscalls to deny? Model: Docker default seccomp profile, or systemd `SystemCallFilter=@privileged @resources`?

**Related:** Landlock unavailable (kernel <5.13) means fail-closed. Acceptable to refuse provider launch on old kernel?

**2. Derived-key migration**

Each provider gets derived storage key from Runtime-held root (`HKDF(root, "provider:<name>", 32)`). Existing data encrypted under old key must migrate.

**Question:** Migration timing: on first provider start (read old, write new), or offline migration tool before upgrade?

**Concern:** First-start migration means provider holds both keys briefly. Offline tool requires downtime.

**Preference:** First-start with one-time "migrated" flag in provider state?
