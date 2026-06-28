# W1b — Kernel TAP Egress Firewall (design)

**Status:** design (in-cloud). The enforcement code lands in the **KVM lane** (a host
with `/dev/kvm` + `CAP_NET_ADMIN`). This doc is the build contract so the chunk is
ready to execute the moment that lane is open.

## The one sentence

Turn the W0/W1 reach **model** (which today only *describes* how far a capsule can
reach) into enforced **teeth**: a per-capsule kernel firewall on its TAP interface
that physically drops any egress outside the capsule's declared `EgressReach`,
fail-closed, with every blocked attempt written to the signed flight recorder.

## Where we are (what's already true)

- **The vocabulary exists** (`elastos-common/src/reach.rs`): `EgressReach::{None,
  Allowlisted, Open}`, `EgressAllowlist::permits(host, scheme)` (fail-closed), and
  `EgressReach::resolve(has_net_capability, allowlist)`. This is *computed and
  projected* onto the catalog (W0/W1a) — **advisory only**.
- **The egress ops are now honest** (G3b drain ch3/ch4): `net-provider`
  (connect/stream/http) and `exit-provider` (open_stream/close_stream/http_fetch)
  declare `execute`, matching what the verb map enforces. So "this capsule can do
  network egress" is now a truthful, enforced capability statement — the input the
  firewall keys on.
- **TAP networking already has a launch hook** (`supervisor.rs:1113`):
  ```rust
  let needs_tap = manifest.permissions.guest_network;
  if needs_tap { vm_config = vm_config.with_network(NetworkConfig::new(&vm_id)); }
  ```
  A microVM only gets a TAP (an L2 path to the host network) when it explicitly
  declares `guest_network`. **This is the exact choke point W1b instruments.**

The gap: nothing installs kernel rules on that TAP. A capsule with `guest_network`
today can reach *anywhere* the host can — the `EgressAllowlist` is never consulted
at the packet level.

## The architecture

```
 capsule microVM ──(TAP: vm-<name>)── host ──> internet
                          │
                   W1b firewall (nftables, per-TAP chain)
                   derived from the capsule's EgressReach:
                     None        → drop ALL egress (default policy)
                     Allowlisted → allow ONLY resolved allowlist dests; drop rest
                     Open        → allow (but tagged "wide" in the projection)
```

1. **Resolve the reach at launch.** Right after `with_network(...)`, compute the
   capsule's `EgressReach` via `EgressReach::resolve(has_net_capability,
   allowlist)` from the manifest's net capability + its `EgressAllowlist`.
2. **Install a per-TAP nftables chain** keyed on the TAP device `vm-<name>`:
   - `None` → chain default `drop` (no allow rules). The capsule has a NIC but no
     reachable destination — fail-closed by construction.
   - `Allowlisted` → one `accept` rule per resolved destination (see DNS note),
     chain default `drop`.
   - `Open` → `accept` (the projection still renders this "hot", per the
     two-channel object — Open egress is the loud, scrutinise-me state).
   - Always allow the host-side carrier/DNS-proxy address; always `drop` RFC1918 /
     link-local unless explicitly allowlisted (matches the providers' own
     `reject_private_host` checks).
3. **Tear down on reap.** The chain is removed when the VM is reaped
   (`supervisor` reap loop / `has_exited`), keyed by `vm-<name>` — same lifecycle
   as the TAP itself. No orphaned rules (mirrors the BUG-2/3 leak discipline).
4. **Audit every drop.** A blocked egress is a denied, signed event on the flight
   recorder (`EgressDenied { capsule, dest, scheme }`). This is the load-bearing
   product moment: the recorder doesn't just log what the agent *did*, it logs
   what it *tried and was stopped from doing*.

## The DNS problem (named honestly)

`EgressAllowlist` is **host-scoped** (`permits("api.example.com", "https")`); a
kernel firewall filters by **IP**. Hosts resolve to changing IPs, so a static
host→IP snapshot at launch is wrong. The design:

- **Primary:** run a tiny host-side **DNS proxy / SNI-aware filter** the capsule is
  forced to use (the only allowed UDP/53 + the allowed egress path is via it). It
  resolves only allowlisted hosts and pins the returned IPs into the nft set
  dynamically; everything else is NXDOMAIN + dropped. This keeps host-scoping
  honest without trusting the guest.
- **Interim (simpler, shippable first):** resolve the allowlist hosts at launch
  into an nft IP set, refresh on TTL. Less precise (IP churn, CDNs) but real
  enforcement; documented as the v0 with the proxy as v1.

The provider plane (`net`/`exit`) already does host-level allow/deny in userspace;
W1b is the **kernel backstop** so a compromised guest that bypasses the provider
SDK still cannot egress past the leash.

## Requirements (why this is the KVM lane, not in-cloud)

- **`/dev/kvm`** — to boot the microVM whose TAP we filter (absent in this
  environment; `cargo` builds fine, VMs cannot run).
- **`CAP_NET_ADMIN`** (root or a granted capability) — to create the TAP, create a
  netns if used, and install nftables rules. The current container has neither.
- **`nftables`** (or `iptables-nft`) on the host.

These are exactly the privileges a sovereign-computer host has and a shared CI
container does not — so W1b is built + tested on the local/Cursor lane.

## Test plan (local lane, real VM)

1. **Allowlisted reaches allowed, blocked elsewhere** — a capsule with an
   `EgressAllowlist` of `{api.allowed.test}` can `https` GET that host and is
   dropped (audited `EgressDenied`) for any other host.
2. **None reaches nothing** — a `guest_network` capsule with no net capability has
   a TAP but every egress is dropped.
3. **Open reaches (and is tagged hot)** — wide egress works and the catalog
   projection renders `EgressReach::Open` as the scrutinise-me state.
4. **Teardown leaves no rules** — after reap, `nft list ruleset` shows no
   `vm-<name>` chain (leak-free, mirrors BUG-2/3).
5. **Compromised-guest backstop** — a guest that calls the host network directly
   (bypassing the `net`/`exit` provider SDK) is still dropped by the kernel chain.

## Why it matters (the convergence)

W1b is where three threads become one enforced thing: the **reach model** (W0/W1)
stops being a description and becomes a wall; the **G3b-honest egress capability**
becomes the firewall's keying input; and the **flight recorder** gains its most
valuable record — *attempted-but-contained* egress. That is the literal, packet-level
proof behind the pitch "we can prove the agent stayed in its lane" (EU AI Act
containment). It is the single highest-leverage remaining piece of the macro goal.

## Lane

In-cloud: this design + the proven reach model + the now-honest egress capability.
KVM lane: the nftables installer, the launch-path threading at `supervisor.rs:1115`,
the DNS proxy, the `EgressDenied` audit event, and the 5 tests above — tracked as
W1b in `ROADMAP.md`.
