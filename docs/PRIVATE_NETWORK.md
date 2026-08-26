# Private network contract

ElastOS can provide a private network between a person's devices and trusted
peers without giving Apps a general-purpose VPN. The default product model is
a set of named services reached through Runtime authority and Carrier. IP
compatibility is an optional adapter for software that cannot use typed Runtime
resources.

This is target architecture. Current implementation and proof remain in
[`state.md`](../state.md). Open work remains in [`TASKS.md`](../TASKS.md).

## Decision

A private network is a signed, mutable `PrivateNetwork` object. It records
membership and service policy. It is not a Profile, principal, capsule,
capability, Carrier topic, or list of IP addresses.

Each network has a separate protected policy authority. It may be controlled by
one principal or a reviewed group policy, but it does not reuse the
ProfileAuthority signing key. A Profile can identify an owner or invitee
without becoming the network's policy or membership store.

The core path is:

```text
App or Agent Host
  -> typed Runtime resource
  -> source Runtime capability check
  -> named private service route
  -> Carrier when the service is off-box
  -> destination Runtime admission
  -> destination provider capability and service policy
  -> audited result
```

The source Runtime and destination Runtime make separate decisions. Membership
allows a device to participate in the network, but does not authorize every
service or operation.

## Object and authority boundaries

| Concept | Purpose | Authority it does not grant |
| --- | --- | --- |
| PrivateNetworkAuthority | Protected signing and policy authority for one private network | Profile signing, Runtime capabilities, or ambient device control |
| PrivateNetwork | Signed mutable membership and service-policy object | A Runtime session, capability, or unrestricted route |
| Profile DID | Person or contact identity used to name the network owner or invited member | Device enrollment or service access |
| Device DID | Endpoint identity for one admitted Runtime | Human identity or access to every service |
| Membership revision | Signed addition, removal, role change, or policy update | Authority after expiry or revocation |
| Named service | Stable service identity and typed operation contract | Raw access to the provider or host port |
| Capability grant | Runtime authority for an exact resource, action, and constraints | Membership in another network or authority on another node |
| Carrier route | Endpoint-authenticated off-box transport selected by Runtime | Application authorship or destination authorization |
| Exit grant | Explicit permission to use a selected node for external egress | LAN access or general membership authority |
| LAN Gateway grant | Explicit permission to reach a bounded legacy subnet or service | A default route or arbitrary local-network access |

PrivateNetwork membership stays separate from a public Profile. A Profile may
publish that a person accepts invitations, but it must not publish private
membership, device topology, service inventory, routes, or policy. Private
network state belongs under the controlling principal's protected rooted data.

## Membership lifecycle

Each network has a stable identifier and a signed revision chain. A revision
names the previous revision, network owner or policy authority, admitted Profile
and Device DIDs, member roles, service constraints, expiry, and revocation
state. Runtime verifies the complete revision path needed for the decision.

Enrollment follows this sequence:

1. The network owner creates an invitation scoped to a Profile, device, role,
   expiry, and allowed enrollment proof.
2. The joining Runtime proves control of its Device DID and the invited
   principal or Profile binding.
3. The network authority signs a new membership revision.
4. Each Runtime installs only the membership and service facts it needs.
5. Service access still requires a Runtime capability and destination policy
   check.

Removal or device loss produces a signed revocation revision. Runtimes must
reject new sessions from the revoked device and close or expire routes derived
from the old revision. Cached membership data cannot extend authority beyond
its signed expiry or revocation policy.

## Named services first

The normal user model is service-oriented:

- `Home on laptop`
- `Files on studio Jetson`
- `Browser Exit on home server`
- `Agent session on laptop`

These names resolve to typed Runtime resources with exact operations. Apps do
not receive peer tickets, Carrier topics, hostnames, ports, socket handles, or
subnet routes. Runtime may choose a same-node provider or an off-box Carrier
route without changing the App contract.

Service discovery returns bounded descriptors, not grants. A descriptor may
name the service, owning Runtime DID, contract version, health, and policy
hints. Runtime still verifies the active principal, session, capsule, grant,
membership revision, destination service policy, expiry, and revocation before
dispatch.

## Compatibility adapters

Some existing software requires an IP network. A privileged TUN adapter may
project a private address space for that software, but it remains a host adapter
behind Runtime policy. It must:

- map destinations to admitted Device DIDs and named service policy;
- block routes that have no explicit grant;
- prevent ordinary capsules from configuring the interface;
- keep DNS, route, and peer credentials out of capsule state;
- record connection and policy outcomes in Runtime audit; and
- close derived routes when the session, grant, or membership ends.

The TUN adapter is not the primary capsule ABI. Supporting it must not create a
second authority path beside typed Runtime resources.

## Exit and LAN Gateway roles

External internet egress and access to a legacy LAN are different roles.

An **Exit provider** accepts a bounded egress grant for selected schemes,
hosts, ports, quotas, and expiry. Browser traffic continues to use the
Browser, Net, and Exit contracts. Private-network membership alone cannot turn
a member into an exit.

A **LAN Gateway provider** exposes selected legacy services or subnets. Its
grant names the allowed destinations and operations. It blocks private targets
outside that scope and does not advertise a default route.

Both roles are optional provider functions. The destination Runtime owns their
policy, credentials, host networking, and accounting. Carrier carries the
off-box stream selected by Runtime but does not decide whether the route is
allowed.

## Human and agent parity

Humans and agents use the same service and transport path. A person may approve
a one-time connection in Home or Inbox. An agent may use an existing bounded
grant or request approval. Both requests resolve to the same Runtime capability
and destination policy checks.

An agent mandate may limit service names, operations, duration, traffic, or
spend. It cannot replace PrivateNetwork membership, create a device identity,
or bypass the destination Runtime.

## Failure and recovery

Private-network operations fail closed when membership, service discovery,
Carrier reachability, destination admission, or provider policy cannot be
verified. The result must distinguish at least:

- no current membership;
- revoked or expired device;
- service not found;
- source capability denied;
- destination policy denied;
- peer unreachable;
- route interrupted; and
- result unknown after dispatch.

Retry is safe only when the operation contract is idempotent or carries a
durable effect identifier that the destination can reconcile. Reconnecting a
UI or Carrier route must not repeat an effect whose settlement is already
known.

## Non-negotiable boundaries

- A Profile is not a network membership list.
- Membership is not a capability.
- A Device DID is not a person or agent identity.
- Discovery is not authorization.
- Carrier authentication is not application authorship.
- The source Runtime cannot grant authority on the destination Runtime.
- An IP adapter cannot become the default capsule network contract.
- An Exit or LAN Gateway grant cannot become ambient network authority.
- The same operation must follow the same Runtime enforcement path for a human
  or an agent.

## Related documents

- [Principles](../PRINCIPLES.md)
- [Architecture](ARCHITECTURE.md)
- [Carrier](CARRIER.md)
- [Human and agent architecture](AGENT_ARCHITECTURE.md)
- [Browser capsule](BROWSER_CAPSULE.md)
- [People and conversations](PEOPLE_CONVERSATIONS.md)
- [Current state](../state.md)
