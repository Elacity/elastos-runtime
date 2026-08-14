# Known issues

Facts with a scope. Not a backlog dump. Add the surface, who saw it, and where it was tried.

## Browser capsule (Mac)

Works. Unstable on Mac. Slow to load. Tried on Mac only (Anders). Other hosts unknown.

Repo `docs/BROWSER_CAPSULE.md` already says the product browser is not complete. Hosted Selkies is a proof path, not the ABI. Source-home is VM-only (`chromium_microvm`, display `webrtc_remote_display`). On Mac that is `browser-vz-engine-supervisor`. If VM artifacts or the VZ supervisor are missing, Browser must fail closed. No host-browser fallback.

The Browser capsule still selects `webrtc_remote_display` in product code. Law says a capsule does not pick a pipe. That gap is open.

## Browser engine on seed (no KVM)

The seed node has no KVM (Anders). A local crosvm browser engine cannot run there.

Repo documents the same shape for a gateway host: without `/dev/kvm`, point `ELASTOS_BROWSER_VM_CONTROL_SOCKET` at a Runtime-facing control socket backed by an operator VM provider. It does not say "seed." Do not pretend local crosvm exists.

Runtime selects that remote provider over Carrier. The Browser capsule must not name the host.

## Exit

On the current Browser path, public DNS/TCP dial-out happens in the Exit helper `elastos/tools/browser-local-exit`, not in the Browser capsule, the engine adapter, or the stream bridge. `exit-provider` is the contract (`elastos://exit/*`, including `remote_carrier_exits`). Do not collapse helper and capsule into one word.

If seed uses a hosted browser engine, Exit on both sides has to be solid: local fail-closed policy, and a remote-carrier exit that can actually carry the stream. Not fully tested. Track it with the browser-engine work. Do not treat Exit as Carrier. Do not treat Exit as Home.

## Still true, already on the August board

A buyer must not pay for an id that open does not recognize.
A trade must not approve an older mint if the newest mint data is missing.
`POST /api/provider` is still live (Library, wallet, System). It is not the capsule contract.
Product Chat is chat-room. `docs/CARRIER.md` still draws retired chat/agent gossip.
Shipped UI Apps are still web projections, not Components.
Authenticated same-node loopback is law, not present on `review/collaboration-candidate` @ `1e035af`. It lives on the unpublished line under `d1800ce` (`7457783`).
