# Playwright reference fill client

This adapter lets an unchanged, pinned Playwright client attach to one existing
Runtime page and replace or clear a field through an approved element reference.
Runtime owns the operator session, inspection grant, writer lease, quotas,
revocation and native effects. The adapter translates a small Playwright wire
profile through the [existing operator client](browser-operator-client.md).

The exact dependency, source revision, registry integrity and extracted package
hash are recorded in `PLAYWRIGHT_OPERATOR_CLIENT` in the adjacent module. Tests
execute the unchanged SDK and check every file against that package hash. This
is a reference-fill compatibility subset. Full Browser acceptance remains in
[B04](../../docs/BROWSER_ACCEPTANCE.md#b04).

## Invitation, approval and connection

The owner shares the existing Browser page invitation. The operator attaches its
own Runtime session, creates `createBrowserOperatorClient` with that invitation,
and calls `requestAdmission({ actions: ["fill"], reason })`. The owner approves
inspection and field replacement/clearing in Browser. `status()` must report an
active admission before the Playwright handshake can finish. An existing active
admission is also usable; it is not evidence of invitation or approval by itself.

The operator host creates `createPlaywrightOperatorAdapter({ client, accessKey })`
and owns its WebSocket listener. Use a separate random access key for this one
adapter; Runtime session credentials stay in the native operator client. Bind
local listeners to loopback, limit messages to 16384 bytes, disable compression,
and retain at most one accepted connection. A remote listener needs the host's
authenticated encrypted transport and connection limits.

For a `ws`-compatible server, forward each connection as follows. The host also
owns listener shutdown and rejects connections on unrelated routes.

```js
server.on("connection", (socket, request) => {
  let peer;
  try {
    peer = adapter.connect({
      headers: request.headers,
      send: text => socket.send(text),
      close: () => socket.close(),
    });
  } catch { socket.close(1008, "operator connection rejected"); return; }
  socket.on("message", (data, binary) => {
    void peer.receive(binary ? null : data.toString());
  });
  socket.on("close", () => { void peer.disconnect(); });
  socket.on("error", () => { void peer.disconnect(); });
});
```

After approval, the actual pinned SDK workflow is:

```js
const browser = await chromium.connect(endpoint, {
  headers: {
    authorization: `Bearer ${accessKey}`,
    "x-elastos-playwright-client": PLAYWRIGHT_OPERATOR_CLIENT.version,
  },
  timeout: 3000,
});
const [context] = browser.contexts();
const [page] = context.pages();
let snapshot = await adapter.inspect();
const field = snapshot.nodes.find(node => node.role === "textbox" && node.name === "Message");
if (!field) throw new Error("Expected invited field is absent");
await page.locator(field.selector).fill("replacement", { timeout: 1500 });
snapshot = await adapter.inspect();
// Use the reference from this new inspection, selected by the caller.
const current = snapshot.nodes.find(node => node.role === "textbox" && node.name === "Message");
if (!current) throw new Error("Expected invited field is absent");
await page.locator(current.selector).clear({ timeout: 1500 });
const observed = await adapter.inspect();
await browser.close();
const release = await adapter.closed;
// Check release.detached. If false, retain its code/request_id for reconciliation.
```

The caller chooses the intended node from approved inspection. This example's
name lookup is application code, not Playwright locator resolution. Inspection
values keep the Runtime snapshot's bounds and sensitive-field policy.

## Exact limits

The accepted wire commands are JavaScript `initialize` and strict main-frame
`fill` with an exact selector returned by `adapter.inspect()`. The SDK's real
`Locator.clear()` sends `fill("")`. References expire after 30 seconds from the
start of inspection; a new snapshot replaces the adapter's reference set.
Runtime checks document identity and authority again for each effect.

Fill retains Runtime's supported text inputs and textarea, 1024 UTF-8 byte limit,
and control-character restriction. It performs one native dispatch and returns
its completed receipt. It does not implement Playwright's general actionability
or automatic waiting. Explicit `force: true`, CSS/role selectors, selector chains,
click, navigation, evaluation, `inputValue`, screenshots, events, network access,
new pages/contexts and page/context close return capability or reference errors.
The required SDK support objects expose no command authority.

Cached SDK getters can still return local object state: one context and page,
an empty URL/name, and the adapter profile as `browser.version()`. These are
transport projections, not Engine version, document state or load evidence.
SDK-only waits and local state setters retain upstream behavior; the adapter
does not advertise their conformance.

Timeouts cap each operation at 10 seconds, including a zero SDK timeout. The
existing Runtime client's per-request deadline also applies. Concurrent work
fails with `adapter_busy`; the adapter keeps no pending command queue. Unknown
wire methods fail explicitly; malformed, oversized or replayed messages end the
operator connection. Client network exposure is rejected during authentication.

An uncertain write returns `operator_reconciliation_required` and its Runtime
request ID. Further writes need a new admission; inspection/reconciliation is
observation only. A failed write or lost response is never replayed by the adapter.

Connected `browser.close()` closes the SDK transport and triggers Runtime
operator detach. Upstream marks its local Page objects closed on disconnect;
Runtime preserves the owner's page. Await `adapter.closed` separately to verify
release. A pending native effect or failed detach remains explicitly unconfirmed.
The host must call `adapter.disconnect()` when shutting down its listener too.

## Source conformance test

Provide an unchanged extracted `playwright-core` npm package matching the module's
pin. Browser binaries and a provider process are unnecessary.

```sh
BROWSER_OPERATOR_PLAYWRIGHT_CORE=/path/to/playwright-core \
  node --test scripts/lib/browser-operator-client-playwright.test.mjs
```

Tests use the actual SDK connection, initializer validators, Browser, Page, Frame
and Locator implementations. A small Runtime API fixture records native contract
calls; ephemeral loopback WebSockets are closed after each case. This proves the
client wire translation and failure fences. Installed DOM effects, owner UX,
remote lifecycle, full Playwright behavior and the shared B04 workflow still need
their respective product evidence.
