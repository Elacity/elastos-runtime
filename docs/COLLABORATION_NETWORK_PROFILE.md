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

## Operator provisioning

`elastos collaboration-config` creates fixture startup files and verifies exact
candidate files. The offline custodian creates production startup files through
the initial-profile operation below. Key creation, profile generation, and
verification are offline. The separate explicit local
bootstrap export attaches only to the selected running Runtime. The
configuration authority is the signer explicitly named in
`trusted_profile_signer_dids`. Runtime device, Carrier, Wallet, passkey, and
host identities keep their separate roles.

The production Community network's profile signer is the release maintainer
DID. Runtime checks each signing role separately: the signed release pins the
file's CID and its signer set, and profile signatures use their own domain,
`elastos.collaboration-network.profile.v1`. Production profiles are signed by
the approved offline release custodian outside CI.

The existing release-signing interface, `scripts/release-signer.py`, uses the
custodian's Ed25519 PEM key through OpenSSL. The `collaboration-config` file
interface accepts an owner-only raw 32-byte Ed25519 key for the disposable test
and fixture networks below. Production keys stay in their existing custody
workflow.

### Initial production profile

The custodian runs the installed, reviewed `release-signer.py` with
`--operation community-profile`. This operation supports revision 1 with no
previous profile hash. Later chain revisions need their own reviewed operation.
It signs the exact approved profile through the existing maintainer PEM and
publicly verifies the signature before writing a new read-only output set.
The release/head signing operation keeps its existing domain checks.

Prepare inert `community-profile-input.json` with these exact fields:
`schema` (`elastos.collaboration-network.signing-input/v1`), `source`
(`commit`, `tree`), `domain` (`elastos.collaboration-network.profile.v1`),
`profile` (the unsigned profile schema above), `default_conversation_grant`
(the canonical grant), and `trusted_profile_signer_dids` (the approved set).
The profile names the existing maintainer DID, the approved bootstrap receipts,
and the raw CID of that grant. The grant names the same network, the approved
conversation, sender service `chat`, and policy `profile_scoped_signer`.
The bootstrap Runtime and its placement are explicit operator inputs; an
artifact host has its own installation role.

The protected custodian policy retains the source, interpreter, tool, OpenSSL,
PEM path, and file/total quotas described in the
[release signer instructions](../scripts/README.md#signing-and-publication-ownership).
Set `operation` to `community-profile` and `manifest_sha256` to the exact input
file hash. Its `community_profile` object has these exact fields:

- `expected_network_id` and `conversation_id`: the approved identifiers.
- `revision`: `1`; `previous_profile_sha256`: `null`.
- `trusted_profile_signer_dids`: the same explicit signer set as the input.
- `profile_payload_sha256`: SHA-256 of the canonical unsigned profile bytes.
- `default_conversation_grant_sha256`: SHA-256 of the canonical grant bytes.

Canonical bytes use sorted object keys, compact UTF-8 JSON, and no trailing
newline. Keep this policy outside the candidate input directory. It approves
the unsigned inputs before the final Community file CID or release manifest
exists. Run the installed tool with a clean environment and confirm the
complete maintainer DID at its prompt:

```sh
env -i /path/to/pinned-python -I -S /path/to/installed-release-signer.py \
  --operation community-profile --manifest community-profile-input.json \
  --policy /path/to/approved-community-policy.json \
  --input-root /path/to/unsigned-community-input \
  --output-root /path/to/new-community-output
```

The output contains `collaboration-network-release-v1.json` and
`collaboration-network-release-pin-v1.json`. The latter supplies the
`collaboration_network` fragment for the components manifest, with the exact
startup-file CID, network ID and signer set. The tool checks ticket bounds,
canonical encoding and node binding. The reviewed, key-free Runtime verifier
checks complete transport semantics before release preparation admits the
output:

```text
elastos collaboration-config verify \
  --input /path/to/new-community-output/collaboration-network-release-v1.json
```

After that check, include the exact startup file and pin in unsigned release
preparation. The existing release operation binds those bytes into the signed
release. Profile signing creates inert files; publication and activation use
their own explicit operator approval.

For an isolated test fixture, create a disposable authority key as a separate
explicit action. Give the fixture an owner and cleanup condition, keep its trust
store separate from real Homes, and keep its private key out of source and logs:

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

Generation fixes the logical sender service to Chat (`chat`); Runtime startup
separately binds the operation capsule to `chat-room`. The command creates the
canonical `profile_scoped_signer` grant, binds its raw SHA-256 CID into the
signed revision-1 profile, and writes the canonical startup configuration with
create-new owner-only semantics. Verification is pure: it uses the same
startup/profile/grant validators as Runtime but does not accept an
accepted-head marker, create a Runtime device identity, join Carrier, or write
product state. Its receipt contains only public identifiers and hashes; it
never prints the authority key or connect ticket.

An absent profile means isolated mode. It never falls back to a public network.
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
