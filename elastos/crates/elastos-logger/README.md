# elastos-logger

Purpose-built, dependency-light logging for the ElastOS workspace. Not `log`/`tracing`.

## Levels

`Trace < Info < Warn < Error < Critical` (`Ord`); a record is emitted iff
`record.level >= threshold`. Parsing is case-insensitive with aliases:
`debug`→Trace, `warning`→Warn, `err`→Error, `crit`/`fatal`→Critical.

**TRACE must be secret-free.** Never log raw keys, seeds, tokens, grants, or DIDs —
log `fp(value)` (`fp:` + first 8 hex of SHA-256), which correlates a workflow across
lines without persisting the secret.

## Usage

```rust
use elastos_logger::{log_info, log_warn};

const LOG_COMPONENT: &str = "gateway.auth";

log_warn!(component: LOG_COMPONENT, "token rejected {}", elastos_logger::fp(&token));
log_info!("plain call logs under the process-default component");
```

The process global installs once (`init`); before that, records fall back to stderr at
Info so early logging is never lost and never panics. Threshold resolution precedence
is CLI > env chain > default via `resolve_level` — the `elastos` binary wires
`--log-level` > `ELASTOS_LOG` > Info.

## Components

Component names are flat dotted buckets stamped per call site
(`gateway.*`, `cmd.*`, `vm.*`, `host.*`; libraries use their crate name:
`runtime`, `storage`, `tls`, `ns`, `identity`, `guest`, `carrier`, `collab`).
The module path travels separately as `target` and is appended to rendered lines
only at TRACE.

## Sinks

`LogSink` is the writer seam: impls must be cheap, non-panicking, drop-on-fail.
Provided: `StderrSink`, `StdoutSink`, `FileSink`, `VecSink` (tests), and
`JsonRingSink` (below). A new output is one more `impl LogSink`; `Logger` never changes.

## AI insight sink (`JsonRingSink`)

Observe-only structured stream for debugging assistance/tooling. Enable on the
`elastos` binary by setting `ELASTOS_LOG_JSON_DIR=/path/to/dir`. Each component gets
`<component>.jsonl` in that directory, one JSON object per line:

```json
{"ts":"2026-08-25T09:14:02Z","level":"WARN","component":"gateway.auth","target":"elastos_server::api::auth_gateway","msg":"token rejected fp:a1b2c3d4"}
```

Format contract for consumers: the five fields above, stable names, one object per
line. Messages are the exact rendered text the stderr sink sees (already `fp()`'d —
the sink adds no data of its own). Files rotate to `<component>.jsonl.1` at 5MB,
keeping at most two generations (~10MB) per component. Hostile component names are
sanitized to `[A-Za-z0-9._-]` and cannot escape the directory.
