# ESP — ElastOS Shell Protocol (shared types)

`esp_v0.ts` is the shared TypeScript contract for **ESP v0**, the read-only
projection protocol between the ElastOS runtime and any shell that renders it.

- **Spec:** [`docs/ESP_V0.md`](../../docs/ESP_V0.md).
- **What this is:** types EXTRACTED from the runtime's shipped serde shapes — the
  contract, not an aspiration. Each type cites the Rust struct + file it mirrors.
- **Wire shapes:** enum values are the serde `rename_all = "snake_case"` forms;
  optional fields are `skip_serializing_if`/`default` on the Rust side; fact types
  carry an index signature because a shell **must ignore unknown fields**.

## Type-check

```sh
/opt/node22/bin/tsc --noEmit --strict esp_v0.ts
```

## Staying in sync

These types are hand-maintained against the Rust structs. The alignment gate
(`scripts/check-wci-alignment.sh`) pins that the routes + `schema` tags the spec
documents still exist in the code, so the doc/types cannot silently drift from
what the runtime serves. A future slice may codegen this file from the serde
definitions.
