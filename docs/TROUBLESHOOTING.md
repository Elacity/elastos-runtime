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

### `discovery relay outbox delivery failed … discovery advertisement is not active`

This debug line means a queued contact request or decision names a Discovery
advertisement that the relay no longer holds, for example after the bootstrap
Runtime restarted. The outbox keeps retrying on later passes and stops
resending a request once its advertisement expires. It does not make Discovery
unavailable.
