# Security

## Reporting

If you find a security vulnerability, please report it privately via [GitHub Security Advisories](https://github.com/Elacity/elastos-runtime/security/advisories/new). Do not open a public issue.

Do not describe an unfixed issue in a public issue, pull request, commit message, or branch name.

## Current isolation and target boundary

First-party apps run as web projections in the browser's opaque sandboxed
frames. Runtime checks each app's signed launch token and actor before it performs an effect.
Home can currently obtain every app's capability, so a compromised Home can
reach those apps' authority. The target limits Home to delegation and gives each
app a separate, revocable capability. The WASM Component authoring path runs in
Wasmtime with memory and fuel limits and Runtime Bus hostcalls. Components run
inside the Runtime process: separate linear memories, but the same operating
system process as Runtime keys. Installed Homes admit only release-path
capsules matched by signed CID receipts and capsules signed by the release
publisher, so third-party executable capsules need key-free, confined
worker processes before they are admitted
([World Computer plan](https://github.com/Elacity/elastos-runtime/issues/285)).

Providers run as native operating-system processes with the Runtime user's
rights. Only the model provider is partly confined. The trusted shell helper
also runs as a native host process. The web Terminal is disabled by default;
host developer mode and closed guest registration are required to enable it.
An enabled Terminal runs commands with the host user's rights.

The seed operator can read hosted data, wallet keys and recovery material.
Passkeys control sign-in; stored data and keys remain accessible to the Runtime
account and root while Home is locked. Protection against hosted operators and
root, and against other software or OS users while a self-hosted Home is locked,
is the target of [hosted protection](https://github.com/Elacity/elastos-runtime/issues/209)
and [locked Home protection](https://github.com/Elacity/elastos-runtime/issues/210).
An unlocked self-hosted Home trusts its owner and their host software. Recovery
from a stolen device or profile key requires a new identity.

The [isolation plan](https://github.com/Elacity/elastos-runtime/issues/173)
records the remaining gates. Source checks describe this source tree. Accepted
installed proof binds the exact Runtime, components and app assets to the
journeys tested on that device.

## Stored keys and recovery

Keys are stored next to the data they protect. Full data-home backups include
all keys. Protect those backups and your Recovery Kit as secrets. Encryption
with keys stored on the same host leaves those secrets accessible to its
operator. Use hosted accounts only for public demos; keep wallets, private data
and recovery material on your own device until hosted protection passes its
acceptance gate. Independent review and installed proof remain security gates.

## Open Findings

The following security-relevant findings remain open in the current runtime and are documented here for transparency.

### Capability state and key rotation are not restart safe

**Severity:** Medium
**Files:** `elastos/crates/elastos-runtime/src/capability/manager.rs`, `elastos/crates/elastos-server/src/security_cmd.rs`
**Status:** Open

Capability signing-key creation and `elastos emergency rotate` log persistence
errors but continue with in-memory state. The emergency command can therefore
report success even when the new key was not written. Rotation also advances a
fresh in-process capability store rather than a durable epoch record. Until the
key and revocation state are committed atomically, operators must verify the
persisted key and restart result instead of treating command completion as a
durable rotation receipt.

### Runtime owns Carrier admission

**Severity:** Medium
**Files:** `elastos/crates/elastos-server/src/carrier.rs`, `elastos/crates/elastos-server/src/carrier_service.rs`
**Status:** Open (host-plane envelope)

Runtime identifies each Carrier peer by its endpoint key. A key alone gives no
authority. A Home serves a peer's `content/fetch` only for a CID pinned on it
now: pinned content is public by link, and unpinned or cached blocks are never
served to peers. Unpublish stops new transfers before garbage collection. A
Home answers a peer's read in one message and checks the pin again just before
sending it, so a read under way when the pin goes is refused. Bytes already
sent can't be recalled.
Only a node that serves publisher content (a seed) answers the raw
`content_fetch` byte operation or serves without the pin check. In this
release a seed accepts no new copies from other Homes; copies it already holds
stay. A Home accepts a new copy from a peer only if the owner gave that peer
the `content.replica` permission; copies it already holds stay. The owner
trusts such a peer like an own device: it can ask the Home to pin content,
import complete content and read storage status. An import pins only the CID
it was asked for and never removes an existing pin. No peer can publish,
remove or repair content on a Home or a seed. Peers cannot use the
availability provider. A Home signs an availability announcement only from its
own pin state. Peers cannot push or read gossip messages directly. Only topics
that a Home joins keep messages, to a maximum of 100 topics. A granted peer's
`ensure` (once the pin succeeds) and `import_object` announce the CID and join
its topic, so a granted peer shares that limit with the Home's own topics. A
granted peer's `import_exact`, `admission` and `status`, and any peer's
`fetch`, join no topic.

Host-process provider capsules use a raw operation and path envelope without a
capsule capability token. Runtime owns admission for this trusted provider
class. Manifests and audit output must show this boundary. The envelope does
not grant capsule authority.

### I/O bridge line reads are not bounded while reading

**Files:** `elastos/crates/elastos-runtime/src/handler/io_bridge.rs`
**Status:** Open

The I/O bridge rejects complete lines above 1 MiB, but its line readers
allocate before that check. Size checks after reading do not bound memory use
or an incomplete frame's lifetime. Add bounds while reading, with oversized
and slow-frame tests.

### Whole-object Carrier fetch replies are held in memory

**Files:** `elastos/crates/elastos-server/src/carrier.rs`
**Status:** Open ([#341](https://github.com/Elacity/elastos-runtime/issues/341))

A whole-object content fetch reply arrives as one line. It is refused while
reading once it passes one 200 MiB file as base64 (about 268 MiB), the largest
file Runtime downloads whole. Only two such fetches run at once, and one per
peer.

### Contact requests by DID

**Severity:** Low
**Files:** `elastos/crates/elastos-server/src/collaboration_discovery_runtime.rs`, `elastos/crates/elastos-server/src/collaboration_contact_store.rs`
**Status:** Open ([#86](https://github.com/Elacity/elastos-runtime/issues/86))

Anyone who has a Profile's DID can send it one contact request through the
seed's Community node, whether or not that Profile is visible in Discovery.
The node learns which Profile asked which and holds the requester's signed
Profile; it learns nothing more about the recipient until the recipient
answers in Inbox. Only the device that signed a request, answer or mailbox
read may submit it, and the recipient's Home checks every request and answer
itself. Profiles and devices are free to create, so the per-sender and
per-device limits bound one identity only: many new identities can fill one
person's 16 request slots on the node until those requests expire after
24 hours, and that person's other requests wait meanwhile. Leave Community
stops all requests.

## Resolved Findings

These findings are resolved in source but remain listed as security history because they shaped the runtime contract.

### I/O bridge parse-size check

**Severity:** Low (reduced from Medium)
**Files:** `elastos/crates/elastos-runtime/src/handler/io_bridge.rs`
**Status:** Fixed (2026-03-28)

The I/O bridge rejects complete request lines above 1 MiB before parsing. The
old `carrier_bridge.rs` has been removed. This resolved parse-size check does
not close the read-time framing gap above.

### Carrier and operator peer reads

**Files:** `elastos/crates/elastos-server/src/carrier.rs`, `elastos/crates/elastos-server/src/operator_control.rs`
**Status:** Fixed

Lines from peers are read with a size cap and a deadline, and longer lines are
refused before they are buffered. Incoming Carrier requests allow one provider
request or one gossip message plus envelope, within 10 seconds. Operator
requests, read before their signature is checked, allow 64 KiB within 10
seconds. Replies are capped by what the request can return: a full gossip
topic for a pull, one importable object for a provider call, update metadata
for an operator answer. Provider replies share a 256 MiB in-flight budget.
Whole-object content fetch replies allow one 200 MiB file as base64; the open
item above tracks streaming them. Relays close after five minutes without bytes
in either direction, and stream acknowledgements have deadlines.

## Architecture

Runtime validates signed capability tokens against the caller, action,
resource, epoch, revocation and token constraints. It checks principal and
session authority separately. Carrier authenticates transport endpoints;
Runtime verifies product identity and signed message authority before exposing
typed app projections. These checks complement the current browser boundary
and do not establish provider process confinement or a separate Home authority.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full trust model.
See the [threat model](docs/THREAT-MODEL.md) and [known limitations](docs/KNOWN-LIMITATIONS.md).
