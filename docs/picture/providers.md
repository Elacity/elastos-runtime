# Providers

A provider implements one protocol. Other capsules reach it by URI and operation. Runtime checks the session and the capability, then invokes.

Local invoke stays in process. Off-box invoke goes through the Carrier provider plane. The capsule code is the same.

HTTP `POST /api/provider/...` still exists as a control leftover. It is not the product ABI. Do not teach capsules to POST there.

Reserved names include content, peer, inspect, wallet, drm, rights, key, decrypt, availability, collaboration-direct, collaboration-profile, and the usual localhost / did / chain / net / exit set.

Built-in, not a capsule package: content, inspect, and Carrier gossip (peer). There is no `chat` or `agent` capsule in the install set. There is `chat-room`.

dKMS is a provider concern that may only use Carrier. It does not know Home, HTTP, sockets, or WireGuard. WireGuard is not in this branch's law.
