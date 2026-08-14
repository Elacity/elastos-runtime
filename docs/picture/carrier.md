# Carrier

Carrier is the off-box pipe. Runtime chooses it when the other end is not this node. Same node uses an authenticated loopback with the same envelope and receipt.

A capsule never calls Carrier. A capsule asks for a typed resource. Runtime resolves the endpoint and picks the pipe.

Carrier proves the transport peer. It does not prove who wrote the message. Chat and dKMS do not become Carrier features.

Tickets, PeerDids, and routes stay inside Runtime. Apps do not see them.

Today the network plane is iroh (QUIC). That is an implementation. The contract is the typed invoke, not iroh.

Code still says "Carrier" for host-process stdio and virtio consoles. That is plumbing. It is not the P2P plane.

Do not teach the old chat/agent gossip diagram. Product Chat is a typed room. Capsules do not `peer/gossip_send`.
