# Namespaces

## Current schemes

Runtime accepts two resource schemes:

- `localhost://...` names rooted local objects and mounted views.
- `elastos://...` names decentralized identities, immutable content, and
  provider-routed services.

The accepted local roots are defined in
[`localhost.rs`](../elastos/crates/elastos-common/src/localhost.rs). Mounted
WebSpace behavior is implemented by
[`webspace-provider`](../capsules/webspace-provider/src/main.rs).

## Current local roots

Runtime currently exposes these file-backed localhost roots:

- `localhost://Users/...`
- `localhost://Public/...`
- `localhost://MyWebSite`
- `localhost://Local/...`
- `localhost://UsersAI/...`
- `localhost://AppCapsules/...`
- `localhost://ElastOS/...`

Runtime also exposes the dynamic `localhost://WebSpaces/...` root. Its resolver
owns each moniker and returns typed handles instead of ordinary files. The
built-in `Elastos` mount has `content`, `peer`, `did`, and `ai` handles.
Each resolves one typed identifier: `content/<cid>`, `peer/<peer-id>`,
`did/<did>`, or `ai/<backend>`. Traversal beyond that endpoint fails closed.

Library's user-facing `Public` place is a projection under the active
principal root, for example `localhost://Users/<principal-root>/Public`. That
placement is separate from published content identity. A file has a local
`content_cid` when its bytes are addressable by the object provider, but it
only has public network reachability after `content-provider` creates a
`published_cid` and `elastos://<cid>` receipt. Published objects do not
automatically appear in `Public`, and placing an object in `Public` does not
silently publish it.

Mounted WebSpaces do not confer provider authority:

Resolver metadata and mount descriptions are not grants. Runtime maps each
operation to explicit capability keys and rechecks the caller before provider
dispatch.

| Name | Meaning |
| --- | --- |
| `localhost://WebSpaces/Cloud Drive/Project X/file.pdf` | Mounted object handle visible to Library and Home. |
| `cloud://drive/files/<stable-file-id>` | Illustrative provider-private target, not a current app-visible Runtime resource. |
| `elastos://<cid>` | Immutable content identity after publication. |

Apps use the mounted handle. Runtime and the provider own credentials, network
APIs, backend targets, and transport.

The current `webspace-provider` supports read-only resolver mounts, mutable
mounts, and forks. Read-only mounts expose indexed handles. Mutable mounts and
forks can create provider-owned files and folders under
`localhost://WebSpaces/<moniker>/...`, with persisted metadata for objects,
heads, and access policy. These files remain provider state rather than
principal-root storage. The provider remains responsible for remote traversal,
cloud sync, Carrier invocation, and availability across peers.

Documents use separate names for each form:

| Name | Form |
| --- | --- |
| `localhost://ElastOS/Documents/<doc-did>` | Canonical mutable document object. |
| `localhost://Users/<principal-root>/Documents/<file>.md` | Markdown working copy owned by the passkey principal. |
| `elastos://<cid>` | Immutable published revision. Publish and fetch operations use the contract in [CONTENT_AVAILABILITY.md](CONTENT_AVAILABILITY.md). |

[SITES.md](SITES.md) defines behavior and status for the
`localhost://MyWebSite` root. [COMMAND_MATRIX.md](COMMAND_MATRIX.md) lists its
current commands.
