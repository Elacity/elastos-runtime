# Rights Provider

`rights-provider` is the protected-content policy boundary. It answers typed
questions for the DRM open path:

`capsule -> runtime capability -> elastos://rights/* -> rights-provider -> policy backend`

Capsules do not receive contract SDK objects, chain RPC, wallet RPC,
key-backend SDKs, raw CEKs, or provider credentials. The provider validates the
question and fails closed until a reviewed dDRM/chain policy backend is
configured.

## Operations

- `status`: list supported rights questions and blocked raw authority.
- `has_access_by_content_id`: ask whether a principal/session has a specific
  right for a content ID.
- `is_subscription_active`: ask whether a principal/session has an active plan.
- `can_stream`: ask whether protected content can be streamed.
- `can_download`: ask whether protected content can be downloaded.

Supported rights are shared with the protected-content schema:

```text
view, stream, download, execute
```

## Capability Schema

| Scope | Resource |
|-------|----------|
| Status | `elastos://rights/meta/status` |
| Access | `elastos://rights/access/has_access_by_content_id` |
| Subscription | `elastos://rights/subscription/is_subscription_active` |
| Stream | `elastos://rights/content/can_stream` |
| Download | `elastos://rights/content/can_download` |

## Current Status

The current provider is intentionally fail-closed. It gives Runtime, DRM, and
future key/decrypt providers one stable contract without pretending production
dDRM reads are ready.

The next slice is to configure a reviewed policy backend that can call approved
typed `chain-provider` rights reads such as:

```text
hasAccessByContentId(string contentId, address subject, string right) -> bool
```

Do not add generic contract calls, raw RPC passthrough, wallet objects, or
frontend-only license checks to this provider.

## Verification

```bash
cargo test --manifest-path capsules/rights-provider/Cargo.toml
cargo clippy --manifest-path capsules/rights-provider/Cargo.toml -- -D warnings
cargo test -p elastos-server --manifest-path elastos/Cargo.toml provider_resource
bash scripts/check-wci-alignment.sh
```
