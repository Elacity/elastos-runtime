# ElastOS Sites

This document defines the local/public site model for `elastos-runtime`.

## Core Contract

The browser-facing local site root is:

```text
localhost://MyWebSite
```

Meaning:

- `localhost://` = your local sovereign machine world
- `MyWebSite` = your personal browser-facing site root
- `Public` remains the shared-files root for the world, not the browser root

This keeps site publishing inside the same `localhost`-first model Rong Chen is
describing instead of treating “website hosting” as a separate external product.

## Current Status

This is now a first-class local site surface in code.

What exists today:

- a real local site root
  - `localhost://MyWebSite`
- a first-class site command surface
  - `elastos site stage`
  - `elastos site path`
  - `elastos site publish [--release <name>]`
  - `elastos site releases`
  - `elastos site channels`
  - `elastos site activate [--release <name> | --channel <name>]`
  - `elastos site history`
  - `elastos site rollback [release-or-bundle-cid]`
  - `elastos site promote <channel> <release>`
  - `elastos site bind-domain`
  - `elastos site serve`
  - CID-backed `publish` / `activate` require the explicit `kubo` + `ipfs-provider` extras
- a first-party site serving component
  - `site-provider` owns the local HTTP edge for `localhost://MyWebSite`
- direct local opening
  - `elastos open localhost://MyWebSite`
- a single public HTTP application edge
  - `elastos gateway` now serves `/`, `/release.json`, `/release-head.json`, `/install.sh`, `/artifacts/...`, `/s/...`, and `/ipfs/...` from runtime-owned state
  - release/install objects live under `localhost://ElastOS/SystemServices/Publisher/...`
  - named site releases live under `localhost://ElastOS/SystemServices/Publisher/SiteReleases/...`
  - domain bindings live under `localhost://ElastOS/SystemServices/Edge/Bindings/...`
  - release promotion channels live under `localhost://ElastOS/SystemServices/Edge/ReleaseChannels/...`
  - signed site activation heads live under `localhost://ElastOS/SystemServices/Edge/SiteHeads/...`
  - activation snapshots live under `localhost://ElastOS/SystemServices/Edge/SiteHistory/...`
  - active site heads now carry an immutable site bundle CID
  - active site heads may also carry the friendly release and channel name that were activated
  - the gateway now exposes `/.well-known/elastos/site-head.json` for the current bound site root
- explicit gateway modes
  - local
  - ephemeral
- a live-host pattern
  - nginx is only TLS/front-door proxying to the gateway

What does **not** exist yet:

- richer site-release provenance UX, release-channel policy, and a fuller mutable site-head/version model above the current named-release/history/rollback/promote slice
- supernode / active-proxy publication mode
- a richer runtime-owned edge control plane above simple file-backed domain bindings and site-head activation files

So the current contract is:

- local site root
  - `localhost://MyWebSite`
- explicit public exposure
  - chosen per serve mode
- stable signed content identity
  - `elastos://<cid>` for share flows and now also for CID-backed site bundles

## Gateway Modes

Site/public exposure should be an explicit layer above the local site root.

### 1. Local

For a machine with a stable IP/domain or a normal reverse proxy.

Examples:

- `elastos site stage ./website/elastos`
- `elastos site serve --mode local --addr 0.0.0.0:8081`
- nginx / Caddy / reverse proxy in front
- public domain such as `runtime.ela.city`

This is the boring, durable operator path.

The public `https://elastos.elacitylabs.com/` root now follows this pattern:

- repo site source under `website/elastos`
- staged into `localhost://MyWebSite`
- optionally published with `elastos site publish --release weekend-demo`
- inspectable through `elastos site releases`
- promotable through `elastos site promote live weekend-demo`
- inspectable through `elastos site channels`
- optionally activated with `elastos site activate --channel live`
- inspectable through `elastos site history`
- revertible through `elastos site rollback [release-or-bundle-cid]`
- served publicly by `elastos gateway`
  - which now resolves `localhost://MyWebSite` plus `localhost://ElastOS/SystemServices/Publisher/...`
  - `elastos site bind-domain <domain> [target]` writes runtime-owned Edge binding state under `localhost://ElastOS/SystemServices/Edge/Bindings/...`
  - `elastos site promote <channel> <release>` writes runtime-owned Edge channel state under `localhost://ElastOS/SystemServices/Edge/ReleaseChannels/...`
  - `elastos site activate [--release <name> | --channel <name>] [target]` writes a signed active site head under `localhost://ElastOS/SystemServices/Edge/SiteHeads/...`
  - the active site head now points at an immutable site bundle CID
- reverse-proxied by nginx for TLS only
- local `elastos site serve --mode local --addr 127.0.0.1:8081` remains an operator preview path, not the canonical public edge

### 2. Ephemeral

For temporary public visibility without durable infrastructure.

Examples:

- `elastos site serve --mode ephemeral`
- tunnel-provider + `cloudflared`

This is good for demos, proof runs, and temporary sharing. It should never silently become the default durable publication contract.

### 3. Supernode / Active Proxy

For a higher-availability hosted/public front door that proxies or mirrors the same local/public site.

Examples:

- active proxy / supernode fronting a local site
- hosted reverse-proxy path coordinated with the broader PC2 / domain-registration work

This is the path that can later map cleanly onto:

- `runtime.ela.city`
- `carrier.ela.city`
- broader PC2-hosted site surfaces

## Design Rules

1. No silent fallback.
   Local site publication must not quietly escape to arbitrary web hosting.

2. The local root stays primary.
   Public exposure is transport, not identity.

3. `MyWebSite` and `Public` are not the same thing.
   `MyWebSite` is the browser-facing site root. `Public` is the shared-files root.

4. `elastos://` remains the stable shared identity.
   Gateway URLs are transport conveniences, not the canonical object. Active site heads now carry immutable site bundle CIDs, and named releases/channels are only human-friendly promotion layers above those immutable bundles.

5. Ordinary internet access remains explicit.
   Local/public site serving can use gateway modes, but only as deliberate operator choices.

6. The current runtime should not overclaim.
   Local and ephemeral site modes are shipped. They are now provider-backed, active site heads now sign immutable CIDs, named releases exist as Publisher aliases, release channels exist as Edge aliases, and basic history/rollback exists, but fuller provenance UX and full `WebSpaces` daemon resolution are still future work.

## Next Implementation Steps

- evolve the current CID-backed Edge site-head activation/history into a fuller mutable site-head/versioning model with stronger provenance UX, release policy, and friendlier promotion flows
- deepen the runtime-owned publisher root (`localhost://ElastOS/SystemServices/Publisher/...`) from filesystem-backed state into a first-class resolver-owned object model
- evolve `localhost://ElastOS/SystemServices/Edge/...` from file-backed bindings into a richer runtime-owned domain/edge control plane
- add supernode / active-proxy publication mode
- keep the live public website synced from the versioned repo source
- keep the live public domain root and release/install endpoints served by the gateway, not drift-prone static nginx aliases
- generalize `site-provider` into a broader `webspace-provider` / `WebSpaces` daemon later
