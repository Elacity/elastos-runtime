# Collaboration network profile

The collaboration-network profile is a signed, versioned bootstrap document.
It selects one collaboration namespace and describes how a Runtime may find its
initial Carrier peers. It is configuration authority only: the profile signer
does not author user messages, grant capsule authority, establish Chat
membership, or act as a Carrier peer merely by signing a profile.

A bootstrap Runtime can keep a stable UDP listener with the existing Runtime
configuration command, for example
`elastos config set carrier_bind_addr 0.0.0.0:4433`. Choose a free address for each
Runtime, restart it, then export
its bootstrap receipt. Gateway and full server startup use that address on each
restart. Invalid configuration or an occupied explicit address stops startup;
an absent setting keeps the default listener and ephemeral-port fallback.
The setting changes the listener address only. Carrier network policy and
collaboration authority keep their own checks.

> **Tip — pin the bootstrap Runtime's Carrier address before exporting its
> receipt.** The bootstrap receipt, and so every Home's network file, records
> the bootstrap Runtime's exact IP address and UDP port. Without
> `carrier_bind_addr`, a Runtime takes `0.0.0.0:4433` when that port is free
> and a random port otherwise, so a restart can move it. Pin a port that every
> joining Home can reach, for example `0.0.0.0:4433`: `127.0.0.1` answers only
> on the same machine, while the receipt usually names the LAN or public
> address. When two Runtimes share one machine, give each its own port.
> See [Troubleshooting](TROUBLESHOOTING.md#collaboration-network-and-carrier).

The canonical JSON envelope uses lexicographically ordered object keys with no
insignificant whitespace and has exactly three fields: `payload`, `signature`,
and `signer_did`. The canonical bytes of its `payload` field are signed with the
domain `elastos.collaboration-network.profile.v1`. The payload schema is
`elastos.collaboration-network.profile/v1` and contains:

- a stable, canonical `network_id`;
- a revision beginning at 1 and increasing by exactly one;
- `previous_profile_sha256`, absent at revision 1 and equal to the SHA-256 of
  the preceding canonical envelope thereafter;
- the signer DID, which must equal the envelope signer;
- at most 16 unique Carrier bootstrap peers, each binding a canonical node ID
  to a canonical v1 connect ticket. Its topic is null, it contains one to eight
  complete endpoints, and every endpoint has that exact ID;
- an optional `default_conversation` descriptor containing one canonical raw
  SHA-256 CID. The signed profile authenticates the exact content-addressed
  grant bytes; the grant is not separately signed, and its CID is not secret.

The v1 default-conversation grant contains only its schema, the stable network
ID, a canonical conversation ID, a canonical sender service, and the
`profile_scoped_signer` admission policy. Each message payload has exactly the
shape `{ product, signed_profile }`. The Runtime binds the operation to that
whole envelope, then verifies that the Profile authorizes both the sending
Runtime endpoint and the signer for the exact service and payload type. The
product receives only the inner payload and verified Profile. There is no
legacy payload parser. This is an open network-room policy; it does not prove
contact, private membership, delivery, or trust beyond that bounded authority.

Validation is pure. The caller supplies both the expected network ID and the
complete trusted profile-signer DID set. Validation never derives trust from a
release publisher, `sources.json`, a Carrier identity, or a hostname. It does
not read or write files, provision identity, connect Carrier, fetch or apply the
optional grant, join Chat, or create any session or state.

Runtime startup selects collaboration only from the owner-only canonical JSON
file `collaboration-network-v1.json` directly under the Runtime data root. Its
schema is `elastos.collaboration-network.startup-config/v1` and its complete
field set is:

- `schema`;
- `expected_network_id`;
- `trusted_profile_signer_dids`, the complete trusted profile-signer DID set;
- `profile_chain_base64`, the complete ordered signed profile chain beginning
  at revision 1, with every canonical envelope encoded as canonical standard
  base64;
- optional `default_conversation_grant_base64`, containing the exact canonical
  grant bytes named by the signed profile.

The file contains no release publisher, hostname, IP address, ambient seed, or
fallback. Absence selects isolation. A present file is validated and accepted
before Carrier subscription or worker startup; invalid permissions, bounds,
encoding, trust, chain, or grant fail closed.

## Release default network

Every Home joins the shared Community room by default. The signed release
names that network, and setup installs it; a Home stays isolated only when its
person chooses isolation.

The release pins the network in `components.json` as `collaboration_network`:

- `head_cid`, the raw SHA-256 CIDv1 of the exact startup configuration bytes;
- `expected_network_id`;
- `trusted_profile_signer_dids`, the complete trusted signer set.

The release ships those bytes beside `components.json` as
`collaboration-network-release-v1.json`. The release signature covers
`components.json`, so the release publisher vouches for the pin, and the pin
fixes the exact configuration, network and signer set. Setup then checks the
configuration with the same validator Runtime startup uses. Runtime startup
itself still reads only `collaboration-network-v1.json`.

The release delivers that file the same way it delivers `model-catalog.json`.
Release staging and the custodian signer bind it to the pin, and release
publication admits it only when its bytes match the pin and pass the startup
validator. Installed setup fetches it by name from the trusted source over
Carrier. `elastos update` and Home's System update both fetch it by its pinned
CID with the Runtime and `components.json`, while Home still runs and before the
new Runtime replaces the old one. `elastos update` installs it as its last
update step; System update installs it right after activation, before Home
restarts. A refusal before or during that step restores the previous release
files. A release rolled back after the network was joined keeps the network,
because Runtime never drops an accepted network. A Home that stays isolated
fetches nothing. An offline update hop refuses a changed pin, like other support
changes.

Setup and update apply the release network to the data root:

- A new Home receives `collaboration-network-v1.json` from the release copy
  and prints one line saying it joins the Community room and how to stay
  isolated.
- An update that pins a newer revision of the same network and signer set
  replaces the file only when the new chain strictly extends the installed
  chain. The accepted-head marker then advances at the next start.
- A Home that already holds a configuration for another network keeps it.
- `elastos setup --isolated` (or `install.sh --isolated`) records the
  owner-only choice `collaboration-isolated-v1` before the first start. Setup
  and update then leave the release network uninstalled. A Home that already
  joined a network keeps it, because Runtime refuses to drop an accepted
  network.
- Source-home setup keeps its explicit `ELASTOS_COLLABORATION_STARTUP_MODE`
  (`configured` or `isolated`) and ignores the release pin.

A release without `collaboration_network` leaves every Home's collaboration
configuration unchanged. The operator provisioning below creates the network
and its configuration bytes that a release then pins.

## Operator provisioning

`elastos collaboration-config` is the only repository tool that creates this
startup file and is never called by the installer or Runtime. Key creation,
profile generation, and verification are offline. The separate explicit local
bootstrap export attaches only to the selected running Runtime. The
configuration authority is a dedicated raw 32-byte Ed25519 key at an explicit
operator path; it is not a Runtime device key, Carrier identity, release
publisher, Wallet/passkey identity, or host identity.

Create the key as a separate explicit action:

```text
elastos collaboration-config create-authority-key --key <owner-only-key-path>
```

Export one canonical owner-only bootstrap receipt from the intended running
local Runtime before returning to the offline flow:

```text
elastos collaboration-config export-local-bootstrap-receipt \
  --data-root <explicit-runtime-data-root> \
  --runtime-kind gateway \
  --output <owner-only-bootstrap-receipt>
```

Select `gateway` for an `elastos gateway` process or `operator` for an
`elastos serve` process. This explicit operator step reads only that exact
runtime kind's coordinates in the supplied data root,
attaches through the loopback operator boundary, and asks only the local
Carrier Provider for its ticket and node ID. It does not call a public
well-known endpoint, inspect Provider files, or create identity or product
state. Its output uses the existing bootstrap-peer contract:
`{"connect_ticket":"<canonical-ticket>","node_id":"<canonical-node-id>"}`.
The ticket is written only to the create-new owner-only file and is never
printed. The remaining key, generation, and verification steps are offline and
supply no hostname, address, or seed default. Generate revision 1 and then
verify the exact output:

```text
elastos collaboration-config generate-initial \
  --authority-key <owner-only-key-path> \
  --network-id <network-id> \
  --conversation-id <conversation-id> \
  --bootstrap-peer <owner-only-bootstrap-receipt> \
  --output <data-root>/collaboration-network-v1.json
elastos collaboration-config verify \
  --input <data-root>/collaboration-network-v1.json
```

When the bootstrap Runtime's address changes, export a fresh bootstrap
receipt and generate the next signed revision from the current file with the
same authority key. The network ID, trusted signer set and default-conversation
grant stay the same; only the bootstrap peers change:

```text
elastos collaboration-config generate-revision \
  --authority-key <owner-only-key-path> \
  --input <current collaboration-network-v1.json> \
  --bootstrap-peer <owner-only-bootstrap-receipt> \
  --output <next collaboration-network-v1.json>
```

Ship the new revision in the next release with its pin; update then advances
each joined Home along the same chain.

Generation fixes the logical sender service to Chat (`chat`); Runtime startup
separately binds the operation capsule to `chat-room`. The command creates the
canonical `profile_scoped_signer` grant, binds its raw SHA-256 CID into the
signed revision-1 profile, and writes the canonical startup configuration with
create-new owner-only semantics. Verification is pure: it uses the same
startup/profile/grant validators as Runtime but does not accept an
accepted-head marker, create a Runtime device identity, join Carrier, or write
product state. Its receipt contains only public identifiers and hashes; it
never prints the authority key or connect ticket.

At Runtime startup, an absent profile means isolated mode; Runtime never
substitutes a network of its own. Joining the default network is a setup and
update step governed by the release pin above.
A profile with another valid network ID is a separate namespace and is rejected
when a different network was requested. Updates fail closed on signature,
canonicalization, bounds, signer, network, revision, or previous-hash errors.

The Runtime-internal loader accepts configuration only as the complete ordered
canonical chain from revision 1 through the selected head. After configuration,
an owner-only accepted-head marker binds the network ID, revision, exact head
envelope hash, and canonical trusted-signer-set hash. While that namespace and
marker are retained, every ordinary restart must present the complete chain
containing the exact accepted head before advancing it; omission, rollback,
replacement, fork, network change, and trust-root change fail closed. A present
configuration namespace with a missing marker also fails closed. If the selected
head names a default-conversation grant, its exact canonical bytes are required
and verified against the signed CID. Loading this configuration does not create
a Runtime device key, product state, or Carrier session.

The accepted-head marker is a retained local rollback witness, not an external
or cryptographic rollback anchor. Deleting the entire collaboration state
namespace or data root is an operator reset indistinguishable from first run.
It does not protect against an actor who can rewrite or delete that whole root.
