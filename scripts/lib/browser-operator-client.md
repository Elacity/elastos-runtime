# Existing-page Camofox operator

Runtime owns approval, page identity, inspection grants and the input writer.
The adapter maps one configured Camofox client to one existing Runtime page and
admission. It uses the operator's separate Runtime session. The owner keeps its
Home token inside Runtime's approval path.

This source milestone covers an invited page, semantic inspection, reference
click, reference fill and operator detach. The complete B04 requirements in
`docs/BROWSER_ACCEPTANCE.md` remain pending target conformance.

## Client provenance

The tested client is `@askjo/camofox-browser-mcp` from `jo-inc/camofox-browser`.
[Fixture provenance](./browser-operator-client-fixtures/camofox/provenance.json)
records exact package versions, immutable source revision, primary API sources,
license and file hashes. The MIT-licensed `tool-contracts.mjs` and `cookies.mjs`
bytes remain unchanged.

Tests execute upstream `runTool`, `fetchSpec` and `adaptResponse`. An in-memory
transport invokes the adapter's Request/Response handler and Runtime API fixture.
Fill tests continue into the source Engine dispatcher and its fixed field
functions with simulated DOM and CDP transport. Separate tests exercise actual
CDP request tracking and the native writer gate with temporary Unix sockets.
The MCP stdio host, SDK process and installed browser need target proof.

## Invitation and explicit approval

Browser Settings provides **Agent access**. The owner creates a page invitation;
this visible step performs a bounded owner inspection to obtain the exact document
generation. The returned metadata contains Runtime origin, page ID, generation
and a 30-second expiry. Credentials and page content remain with the owner.

The operator constructs `createBrowserOperatorClient` with a separately trusted
Runtime origin, its own session token, page ID and that invitation. It calls
`requestAdmission({ actions: ["click", "fill"], reason: "Complete this field" })`.
Runtime creates the request ID. The owner reviews the session, reason, document,
30-second duration, three-action quota, inspection meaning and requested actions.
The UI explicitly says that inspection reads page text, labels and form values,
and that fill replaces or clears the selected field.

Owner approval grants separate signed read and write capabilities. Old requests
default to `inspect: false`. The operator's `/inspect` route checks its session,
page, admission and read capability before dispatch and again before returning
content. The read grant permits 16 requests within the admission deadline.
`inspect()` collects at most eight pages, 512 nodes and 128 KiB of node data.

The invitation is metadata, not an admission. The existing admission-ID
constructor remains a lower-level attachment option. A new document requires a
new invitation and approval. The current combined read/write admission acquires
the existing writer lease; reader-only admission is still a separate B04 gap.

## Typed workflow

Camofox's default `type` API calls `locator.fill`, which replaces the full value
and accepts an empty string to clear it. Runtime's original reference `type`
action inserts text into an already focused field. The new reference `fill`
action gives the adapter an explicit replacement contract while preserving the
original insertion behavior.

The Engine resolves the exact snapshot-bound field in an isolated world, checks
that it is visible, connected, enabled and editable, then focuses and selects its
full value. Its private input path inserts the replacement, or sends and releases
Delete for an empty value. The Engine verifies the final value before reporting
completion. The native writer hold starts before focus or selection. Late replies
and lost replies retain the existing effect accounting and prevent blind replay.
The client receives a request ID for reconciliation after an uncertain result.

This fill subset accepts text, search, telephone, URL and password inputs plus
textareas, with up to 1024 UTF-8 bytes and no control characters. Email, number,
date and other specialized inputs, multiline values and contenteditable fields
remain outside this subset. Selectors, keyboard mode, submission and `pressEnter`
return explicit capability errors. The operator supplies references and text;
Runtime owns the internal field functions and provider transport.

`createCamofoxOperatorAdapter` binds one access key and routing label to that
client. The host owns its listener or transport. A routing label selects the
configured session and grants no authority. Implemented upstream calls are list,
restricted snapshot, reference click and default reference fill. Document-bound
refs remain unique across snapshots.

The upstream snapshot always requests a screenshot. The default `strict` profile
therefore returns `capability_unsupported`. The explicit `inspection-only-v1`
profile returns text and reports the requested screenshot as omitted. Upstream
`adaptResponse` retains this marker. This is partial Camofox conformance, with
Playwright and Camoufox integration still pending.

The adapter extension `POST /tabs/:tabId/detach` authenticates the operator,
revokes its admission and reuses Runtime's writer-release path. Its result says
`page_closed: false`. The owner retains page-close authority; upstream close,
create, navigate, evaluate and screenshot calls report unsupported capabilities.

## Source checks and next installed milestone

```sh
node --test scripts/lib/browser-operator-client.test.mjs scripts/browser-operator-input.test.mjs scripts/browser-operator-cdp.test.mjs
(cd elastos && CARGO_BUILD_JOBS=2 nice -n 10 cargo test -p elastos-common browser_protocol::operator -- --nocapture)
(cd elastos && CARGO_BUILD_JOBS=2 nice -n 10 cargo test -p elastos-server browser_operator -- --nocapture)
```

Gateway regressions cover a separate Runtime session creating its own admission,
explicit inspection approval, read/write separation, replacement and clearing
payloads, receipt replay, foreign sessions, and detach during a delayed read.
Source tests cannot establish actual DOM or framework-host behavior.

The next installed milestone runs the pinned actual client against the integrated
Runtime and Engine: create an owner invitation, attach the operator's own session,
approve inspection and fill, inspect a prefilled field, replace it, inspect the
replacement, clear it, inspect the empty value, then revoke access and verify that
further reads and effects stop while the owner's page remains available. Bind the
client revision and source/artifact hashes to that evidence. Full B04 also keeps
its screenshot, reader-only, lifecycle, frames, files, navigation, waits/events,
remote/headless, other-framework, human and device gates.
