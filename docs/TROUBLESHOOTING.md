# Troubleshooting

Known symptoms, their causes and the fix. Each section starts with the log line
or screen state a person sees first. Add a section when a problem took real
effort to diagnose.

## Collaboration network and Carrier

Background: [collaboration network profile](COLLABORATION_NETWORK_PROFILE.md)
and [Carrier](CARRIER.md).

### `Carrier provider connect failed ticket_index=0 error=connect timed out`

A Home logs this when it cannot reach the Community bootstrap Runtime at the
address recorded in its network file. The release availability contract keeps
connected Homes exchanging Community messages and catch-up. A Home that starts
or restarts cannot join Community. Discovery and new contact requests stop.
New Direct messages show **Sending**, retry for up to 24 hours, then show
**Expired**. Each Home keeps the messages it already received.

See [Community availability](COLLABORATION_NETWORK_PROFILE.md#availability)
for the node's role and the behavior while it is unreachable.

1. Read the bootstrap address from the network file. `profile_chain_base64`
   holds the signed profile; its `bootstrap_peers` carry a connect ticket that
   lists `"Ip":"<address>:<port>"`.
2. Compare it with the bootstrap Runtime's log line
   `carrier: isolated <did> (port <port>)` and with
   `lsof -nP -iUDP | grep elastos`.
3. Match them:
   - The port differs: run
     `elastos config set carrier_bind_addr 0.0.0.0:<port from the file>` on the
     bootstrap Runtime.
   - It listens on `127.0.0.1` while the file names a LAN or public address:
     bind `0.0.0.0:<port>` instead. `127.0.0.1` answers only on the same
     machine.
   - Two Runtimes on one machine compete for the default port `4433`: give the
     other Runtime its own `carrier_bind_addr`, and start the bootstrap
     Runtime first.
   - The machine's address itself changed: generate the next signed revision
     with `elastos collaboration-config generate-revision` from a fresh
     bootstrap receipt, and give every Home the new file (a release for the
     public network).
4. Restart the bootstrap Runtime, then the other Homes. The timeout lines stop
   once the Home connects.

Prevention: pin the bootstrap Runtime's `carrier_bind_addr` before exporting
its bootstrap receipt. See the tip in the
[collaboration network profile](COLLABORATION_NETWORK_PROFILE.md).

### `Configured Carrier listener could not start … Failed to bind requested Carrier address`

Another process holds that address, often a second Runtime on the same machine
pinned to the same port. Give each Runtime its own port with
`elastos config set carrier_bind_addr`. Settings live in each Home's own
`config.toml`, so set a second Home's port with that Home's `HOME` or
`XDG_DATA_HOME`.

### One Home shows Community Live but exchanges no Chat messages

Other Homes' Community messages never appear on this Home, and its own never
reach them, although Chat shows Community as **Live**. The Home is the network's
bootstrap: its Carrier node ID is the `node_id` in the head profile's
`bootstrap_peers`. Runtime keeps a bootstrap to Discovery only, so participants
refuse Chat to and from it (the refusal logs at debug: `Community bootstrap
endpoints carry discovery metadata only`). This happens when a test network
was created from a person's own Home. Progress on letting participants serve
as bootstrap: [#324](https://github.com/Elacity/elastos-runtime/issues/324).

To confirm, compare the Home's `carrier: isolated <did> (port <port>)` log
line, whose did:key encodes the node ID, with the `node_id` in the network
file's last `profile_chain_base64` entry.

To make every person's Home a participant, give the network a separate
bootstrap Runtime without people, as the release does:

1. Install another Runtime with its own data root and Carrier port
   (`elastos config set carrier_bind_addr 0.0.0.0:<port>`), on the same
   network file, and start it.
2. Export its receipt with `elastos collaboration-config
   export-local-bootstrap-receipt`, then sign the next revision with the
   network's authority key (`generate-revision`) and `verify` it. See
   [operator provisioning](COLLABORATION_NETWORK_PROFILE.md#operator-provisioning).
3. Stop every Home. A release delivers the revision to installed Homes through
   update. Source-home setup only installs a network file where none exists,
   so on source Homes keep a copy of the current file and replace it,
   owner-only, with the new revision.
4. Start the new bootstrap Runtime first, then the other Homes. Each Home's
   accepted-head marker under `collaboration/config` then shows the new
   revision; a Home never returns to an earlier one.

### Community stays unreachable after the Community node comes back

A Home that kept running while the bootstrap Runtime was down, with no other
Home connected, can show **Community unreachable** for up to about five
minutes after the bootstrap is back. Reopening Chat does not help; the Home
recovers by itself.

This is a known iroh-gossip 0.101.0 issue. Upstream
[n0-computer/iroh-gossip#161](https://github.com/n0-computer/iroh-gossip/pull/161)
(merged after 0.101.0, not yet released) reworks this peer lifecycle; check
this case again when Runtime moves to a release that includes it.
After a failed dial, iroh-gossip can keep the peer pending with queued
messages, and it dials again only when that queue is empty. Runtime's
bootstrap recovery asks to rejoin every second, so the queue never empties
and the peer is never dialed again. With `iroh_gossip=debug`, the log shows
one `dial failed: timed out peer=<bootstrap>` and then a `Join` sent to that
peer every second with no further `start to dial`.

Runtime's Community link watchdog clears it. A joined Home with no Community
peer for 60 seconds leaves and rejoins the Community topic; a minute later it
replaces its Carrier gossip actor, and it keeps repairing while no peer
returns, waiting twice as long each time up to five minutes. A Home replaces
its gossip actor at most six times per start (each replacement leaves a small
address lookup behind in iroh); after that it only rejoins, and restarting
Home allows replacements again. A quiet Community with a peer listed is left
alone. The Home's log names each step (`Community link looks dead`,
`rebuilding Carrier gossip`, `rebuilds are capped`, `Community link
recovered`). A bootstrap Runtime never runs these steps.

### `discovery relay outbox delivery failed … discovery advertisement is not active`

This debug line means a queued contact request or decision names a Discovery
advertisement that the relay no longer holds, for example after the bootstrap
Runtime restarted. The outbox keeps retrying on later passes and stops
resending a request once its advertisement expires. It does not make Discovery
unavailable.
