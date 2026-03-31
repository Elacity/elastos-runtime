# Namespaces

## Local And Network Spaces

The current rooted-space contract is:

- your local PC2 world is expressed through rooted `localhost://...` paths
- `elastos://` = decentralized identities, peer/provider surfaces, and signed shared content

File-backed localhost roots currently exposed by the runtime:

- `localhost://Users/...`
- `localhost://Public/...`
- `localhost://MyWebSite`
- `localhost://Local/...`
- `localhost://UsersAI/...`
- `localhost://AppCapsules/...`
- `localhost://ElastOS/...`
- `localhost://PC2Host/...`

Dynamic special root:

- `localhost://WebSpaces/...`
  - this is not ordinary storage; it is the dynamic WebSpace/AppCapsule resolver surface
  - the resolver owns `localhost://WebSpaces/<moniker>/...` first and returns typed handles instead of walking a normal filesystem path
  - the initial mounted `Elastos` handle already exposes typed children such as `content`, `peer`, `did`, and `ai`
  - today, `content/<cid>` resolves to a file endpoint, while `peer/<id>`, `did/<did>`, and `ai/<backend>` stop at one typed folder handle and deeper traversal fails closed until richer resolver semantics exist

Current namespace contract:

- `localhost://ElastOS/...` = runtime-owned local system state and services
- `elastos://...` = decentralized identities and provider-routed resources between nodes
- `localhost://WebSpaces/<moniker>/...` = local mounted resolver view of a broader dynamic named space

Useful current examples:

- `localhost://Users/self/Documents/report.md`
- `localhost://Public/manual.pdf`
- `localhost://MyWebSite`
- `localhost://WebSpaces/Elastos`
- `localhost://WebSpaces/Elastos/content/<cid>`
- `localhost://WebSpaces/Elastos/peer/<peer-id>`
- `localhost://ElastOS/SystemServices/Edge/SiteHeads/...`
- `elastos://<cid>` as the canonical identity returned by `elastos share`
- `elastos://peer/...` and `elastos://ai/...` as provider-routed surfaces

Useful current WebSpace commands:

- `elastos webspace list`
- `elastos webspace resolve Elastos`
- `elastos webspace list Elastos`
- `elastos webspace resolve Elastos/content/<cid>`

`elastos open elastos://<cid>` opens a share through the local bridge. `elastos share --public` holds an immediate public edge open while the command is running. Plain gateway URLs are convenience transport and may take time to propagate; the CID is the stable shared identity.

## Elastos Sites

The browser-facing local site root is:

- `localhost://MyWebSite`

`Public` remains the shared-files root. `MyWebSite` is the personal browser root.

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
- `localhost://Public/*` is a separate shared-files root
- `localhost://ElastOS/SystemServices/Publisher/...` owns release/install/artifact state for the public edge
- `elastos site ...` is the explicit site command surface

What remains for later:

- richer site-release provenance UX, release-channel policy, and a fuller mutable site-head/version model
- supernode / active-proxy gateway mode

For the broader architecture direction, see [ARCHITECTURE.md](ARCHITECTURE.md) and [OVERVIEW.md](OVERVIEW.md).
