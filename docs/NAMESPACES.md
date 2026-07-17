# Namespaces

## Local And Network Spaces

The current rooted-space contract is:

- your local Home world is expressed through rooted `localhost://...` paths
- `elastos://` = decentralized identities, peer/provider surfaces, and signed shared content

Target SmartWeb space model:

- `localhost://...` is the operator's own local computer space from their
  perspective.
- `dns://...` and registered domain-style spaces such as
  `joe.ela.city://...` are other principals' SmartWeb spaces. A remote
  principal may see your exported local space through a public name such as
  `joe.ela.city://...`; from your side, the same objects remain rooted in
  your local `localhost://...` authority.
- Remote spaces can be mounted and browsed as extensions of local drives, but
  read and write are never ambient. They require explicit capability keys for
  the target space, operation, principal, and session.
- Apps, agents, and users should not reason about IP addresses, HTTP gateways,
  raw Carrier links, cloud APIs, or host paths. Runtime and providers hide the
  underlying internet and expose only capability-scoped WebSpace handles.

File-backed localhost roots currently exposed by the runtime:

- `localhost://Users/...`
- `localhost://Public/...`
- `localhost://MyWebSite`
- `localhost://Local/...`
- `localhost://UsersAI/...`
- `localhost://AppCapsules/...`
- `localhost://ElastOS/...`

Dynamic special root:

- `localhost://WebSpaces/...`
  - this is not ordinary storage; it is the dynamic WebSpace/AppCapsule resolver surface
  - the resolver owns `localhost://WebSpaces/<moniker>/...` first and returns typed handles instead of walking a normal filesystem path
  - this is the local mounted view; any raw provider target such as `cloud://drive/...` or `elastos://content/...` stays behind the resolver/provider contract
  - the initial mounted `Elastos` handle already exposes typed children such as `content`, `peer`, `did`, and `ai`
  - today, `content/<cid>` resolves to a file endpoint, while `peer/<id>`, `did/<did>`, and `ai/<backend>` stop at one typed folder handle and deeper traversal fails closed until richer resolver semantics exist

Library's user-facing `Public` place is a projection under the active
principal root, for example `localhost://Users/<principal>/Public`. That
placement is separate from published content identity. A file has a local
`content_cid` when its bytes are addressable by the object provider, but it
only has public network reachability after `content-provider` creates a
`published_cid` and `elastos://<cid>` receipt. Published objects do not
automatically appear in `Public`, and placing an object in `Public` does not
silently publish it.

Current namespace contract:

- `localhost://ElastOS/...` = runtime-owned local system state and services
- `localhost://Users/<principal-root>/...` = passkey-principal-owned local user area; the first passkey is admin and later passkeys are guests. When a root has verified `elastos.principal.root-protection/v1` state, runtime/provider writers must store protected object envelopes rather than plaintext bytes.
- `elastos://...` = decentralized identities and provider-routed resources between nodes
- `localhost://WebSpaces/<moniker>/...` = local mounted resolver view of a broader dynamic named space
- `localhost://Users/<principal-root>/.AppData/ElastOS/Home/browser-state.json` = Home layout, window-session, and recent-target state for the active runtime principal

Mounted WebSpaces are not literal aliases for raw provider authority. A useful
external mount would look like:

- `localhost://WebSpaces/Cloud Drive/Project X/file.pdf` = Library/Home-visible mounted object handle
- `cloud://drive/files/<stable-file-id>` = provider-private target understood only by a future cloud-drive provider
- `elastos://content/<cid>` = provider-independent content identity after import, publish, or fork
- `joe.ela.city://Documents/report.md` = another principal's named SmartWeb
  space, accessible only when that principal grants an explicit capability
- `dns://team-space/Documents/report.md` = resolver-owned named space syntax
  for future user-defined WebSpaces

That split keeps the WebSpace-as-named-intent model intact: apps and users
speak mounted WebSpace intent; Runtime/provider contracts resolve it; raw
credentials, network APIs, Kubo/IPFS details, and Carrier transport stay below
the app-visible namespace.

Current `webspace-provider` supports both readonly resolver mounts and mutable
mounts/forks. Readonly mounts expose mounted/indexed handles only. Mutable
mounts can materialize local provider-owned files and folders under
`localhost://WebSpaces/<moniker>/...` with persisted object/head/access-policy
metadata. This is still not ordinary principal-root filesystem storage: remote
resolver traversal, cloud-provider sync, Carrier invocation, and multi-peer
availability remain provider responsibilities below the mounted WebSpace view.

For documents, the intended identity split is:

- `localhost://ElastOS/Documents/<doc-did>` = canonical mutable document object
- `localhost://Users/<principal-root>/Documents/<file>.md` = passkey-principal-owned working-copy storage for markdown bytes
- `elastos://<cid>` = immutable published/shared revision; current implementation opens and publishes through the higher-level content availability plane described in [CONTENT_AVAILABILITY.md](CONTENT_AVAILABILITY.md), backed locally by `ipfs-provider`

For Home appearance, the current identity split is:

- `capsules/home-gui/browser/wallpaper.webp` = signed capsule-bundled default wallpaper
- `localhost://Users/<principal-root>/.AppData/ElastOS/Home/Appearance/background-image.{png,jpg,webp,gif}` = passkey-principal-owned wallpaper override
- `localhost://Users/<principal-root>/.AppData/ElastOS/Home/Appearance/background-overlay.json` = passkey-principal-owned overlay enabled/opacity preference; overlay is off by default

Appearance is not shared runtime state. System may edit it, but the runtime stores it under the active principal root and uses the protected principal-root object envelope when that root has verified protection. The DID-aligned next step is a signed profile/settings object anchored to the user's DID that can sync through Carrier/provider policy and then materialize into this principal-owned local projection on each trusted device.

Useful current examples:

- `localhost://ElastOS/Documents/<doc-did>`
- `localhost://Users/<principal-root>/.AppData/ElastOS/Home/Appearance/background-overlay.json`
- `localhost://Users/<principal-root>/Documents/report.md`
- `localhost://Public/manual.pdf`
- `localhost://MyWebSite`
- `localhost://WebSpaces/Elastos`
- `localhost://WebSpaces/Elastos/content/<cid>`
- `localhost://WebSpaces/Elastos/peer/<peer-id>`
- `localhost://ElastOS/SystemServices/Edge/SiteHeads/...`
- `elastos://<cid>` as the canonical content identity returned by `elastos share`
- `elastos://peer/...` and `elastos://ai/...` as provider-routed surfaces

Useful current WebSpace commands:

- `elastos webspace list`
- `elastos webspace resolve Elastos`
- `elastos webspace list Elastos`
- `elastos webspace resolve Elastos/content/<cid>`
- `elastos webspace health|refresh|cache|sync|fork ...`

`elastos open elastos://<cid>` opens a share through the local bridge. `elastos share --public` holds an immediate public edge open while the command is running. Plain gateway URLs are convenience transport and may take time to propagate; the CID is the stable shared content identity.

## Elastos Sites

The browser-facing local site root is:

- `localhost://MyWebSite`

`Public` remains the shared-files placement root. `MyWebSite` is the personal browser root.

This is now staged and served explicitly through:

- `elastos site stage <dir>`
- `elastos site path`
- `elastos site publish [--release <name>]`
- `elastos site releases`
- `elastos site channels`
- `elastos site activate [--release <name> | --channel <name>]`
- `elastos site history`
- `elastos site rollback [release-or-bundle-cid]`
- `elastos site promote <channel> <release>`
- `elastos site bind-domain <domain> [target]`
- `elastos site serve --mode local`
- `elastos site serve --mode ephemeral`
- `elastos open localhost://MyWebSite`

For CID-backed site publish and activation on a fresh installed layout, add the explicit extras first:

```bash
elastos setup --with kubo --with ipfs-provider
```

Public exposure sits above that root as an explicit operator choice:

- local gateway — static IP or stable domain you control
- ephemeral gateway — temporary public edge such as `cloudflared`
- supernode / active proxy — a higher-availability hosted front door for the same local or replicated site

What is implemented now:

- `localhost://MyWebSite` is a real local root under the runtime data dir
- `localhost://Public/*` is a separate shared-files root for global/local public-placement compatibility
- `localhost://ElastOS/SystemServices/Publisher/...` owns release/install/artifact state for the public edge
- `elastos site ...` is the explicit site command surface

What remains for later:

- richer site-release provenance UX, release-channel policy, and a fuller mutable site-head/version model
- supernode / active-proxy gateway mode

For the broader architecture direction, see [ARCHITECTURE.md](ARCHITECTURE.md) and [OVERVIEW.md](OVERVIEW.md).
