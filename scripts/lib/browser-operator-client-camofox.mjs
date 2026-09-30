import { timingSafeEqual } from "node:crypto";
import { BrowserOperatorError } from "./browser-operator-client.mjs";

export const CAMOFOX_CLIENT = Object.freeze({ package: "@askjo/camofox-browser-mcp", version: "1.14.0",
  revision: "e5a36f5cd0332fde6597de474329a308a53a0716" });

// A fetch-compatible REST adapter; the host owns any HTTP listener and credentials.
// One instance binds one authenticated client to one invited Runtime page.
export function createCamofoxOperatorAdapter({ client, accessKey, userId, tabId = "runtime-page",
  snapshotProfile = "strict", now = () => performance.now() }) {
  if (!["strict", "inspection-only-v1"].includes(snapshotProfile) || typeof accessKey !== "string" || accessKey.length < 16 ||
    accessKey.length > 128 || !/^[a-zA-Z0-9_-]{1,64}$/.test(tabId) || typeof userId !== "string" || userId.length > 256) {
    throw new BrowserOperatorError("adapter_configuration_invalid", 400);
  }
  let snapshot = null, serial = 0, detached = false, busy = false;
  const refs = new Map();
  const unsupported = feature => { throw new BrowserOperatorError("capability_unsupported", 501, { feature }); };
  const json = (body, status = 200) => Response.json(body, { status });
  function authenticated(request) {
    const actual = Buffer.from(request.headers.get("authorization") || "");
    const expected = Buffer.from(`Bearer ${accessKey}`);
    return !request.headers.has("cookie") && !request.headers.has("x-elastos-home-token") &&
      actual.length === expected.length && timingSafeEqual(actual, expected);
  }
  async function handle(request) {
    try {
      if (!authenticated(request)) throw new BrowserOperatorError("adapter_authority_required", 401);
      if (busy) throw new BrowserOperatorError("adapter_busy", 409);
      busy = true;
      try {
        const url = new URL(request.url), path = url.pathname;
        let body = {};
        if (request.method === "POST") {
          const reader = request.body?.getReader(); let size = 0; const parts = [];
          try {
            if (!reader) throw new BrowserOperatorError("adapter_request_invalid", 400);
            for (;;) {
              const { done, value } = await reader.read(); if (done) break;
              size += value.byteLength;
              if (size > 8192) throw new BrowserOperatorError("adapter_request_too_large", 413);
              parts.push(value);
            }
          } finally { await reader?.cancel().catch(() => {}); reader?.releaseLock(); }
          try { body = JSON.parse(Buffer.concat(parts).toString("utf8")); }
          catch { throw new BrowserOperatorError("adapter_request_invalid", 400); }
        }
        if (!body || Array.isArray(body) || typeof body !== "object") throw new BrowserOperatorError("adapter_request_invalid", 400);
        const user = request.method === "POST" ? body.userId : url.searchParams.get("userId");
        if (user !== userId) throw new BrowserOperatorError("adapter_session_mismatch", 403);
        if (detached) throw new BrowserOperatorError("operator_detached", 409);
        if (path === "/tabs" && request.method === "GET") {
          const admission = await client.status({ signal: request.signal });
          return json({ tabs: [{ tabId, runtimePageId: client.pageId }], runtime: { client: CAMOFOX_CLIENT,
            admission_status: admission.status, snapshot_profile: snapshotProfile, capabilities: ["inspect", "ref_click", "ref_fill", "detach"],
            fill: { input_types: ["text", "search", "tel", "url", "password"], textarea: true,
              max_utf8_bytes: 1024, control_characters: false, contenteditable: false },
            unsupported: ["keyboard_type", "screenshot", "close", "create", "navigate", "evaluate"] } });
        }
        if (!path.startsWith(`/tabs/${tabId}/`) && path !== `/tabs/${tabId}`) unsupported("route");
        const action = path.slice(`/tabs/${tabId}`.length);
        if (action === "/snapshot" && request.method === "GET") {
          if (url.searchParams.get("includeScreenshot") === "true" && snapshotProfile === "strict") unsupported("screenshot");
          const offsetText = url.searchParams.get("offset") || "0";
          if (!/^(0|[1-9][0-9]*)$/.test(offsetText)) throw new BrowserOperatorError("adapter_offset_invalid", 400);
          const offset = Number(offsetText);
          if (offset === 0) {
            snapshot = null; refs.clear();
            const started = now(); const value = await client.inspect({ signal: request.signal });
            const lines = value.nodes.map(node => {
              if (serial >= 1_000_000_000) throw new BrowserOperatorError("adapter_reference_limit");
              const ref = `e${++serial}`;
              refs.set(ref, { ref: node.ref, document_generation: value.document_generation });
              return `- ${JSON.stringify(node.role)} ${JSON.stringify(node.name)} [${ref}] value=${JSON.stringify(node.value)}`;
            });
            snapshot = { text: lines.join("\n"), expires: started + 30000, nextOffset: 0 };
          } else {
            const admission = await client.status({ signal: request.signal });
            if (admission.status !== "active") throw new BrowserOperatorError("operator_admission_inactive", 403);
          }
          if (!snapshot || now() >= snapshot.expires || offset !== snapshot.nextOffset) throw new BrowserOperatorError("stale_inspection");
          const text = snapshot.text.slice(offset, offset + 12000), next = offset + text.length;
          const more = next < snapshot.text.length;
          snapshot.nextOffset = more ? next : null;
          return json({ snapshot: text, refsCount: refs.size, totalChars: snapshot.text.length, truncated: more,
            hasMore: more, nextOffset: more ? next : null, runtime: { snapshot_profile: snapshotProfile,
              omitted_requested: url.searchParams.get("includeScreenshot") === "true" ? ["screenshot"] : [],
              page_id: client.pageId, client: CAMOFOX_CLIENT } });
        }
        if (action === "/click" && request.method === "POST") {
          if (Object.keys(body).some(k => !["ref", "userId"].includes(k))) unsupported("click_options");
          const binding = refs.get(body.ref);
          if (!binding || !snapshot || now() >= snapshot.expires) throw new BrowserOperatorError("stale_inspection");
          await client.input({ ...binding, action: "click" }, { signal: request.signal });
          return json({ ok: true });
        }
        if (action === "/type" && request.method === "POST") {
          if (Object.keys(body).some(k => !["ref", "text", "userId", "mode", "pressEnter", "submit"].includes(k)) ||
            (body.mode !== undefined && body.mode !== "fill") ||
            (body.pressEnter !== undefined && body.pressEnter !== false) || (body.submit !== undefined && body.submit !== false))
            unsupported("fill_options");
          const binding = refs.get(body.ref);
          if (!binding || !snapshot || now() >= snapshot.expires) throw new BrowserOperatorError("stale_inspection");
          await client.input({ ...binding, action: "fill", text: body.text }, { signal: request.signal });
          return json({ ok: true });
        }
        if (action === "" && request.method === "DELETE") unsupported("page_close");
        if (action === "/detach" && request.method === "POST") {
          if (Object.keys(body).some(k => k !== "userId")) unsupported("detach_options");
          const result = await client.detach({ signal: request.signal });
          detached = true; snapshot = null; refs.clear(); return json(result);
        }
        unsupported("route");
      } finally { busy = false; }
    } catch (error) {
      if (!(error instanceof BrowserOperatorError)) return json({ error: "adapter_failed", code: "adapter_failed" }, 503);
      return json({ error: error.code, code: error.code, ...error.details }, error.status);
    }
  }
  return Object.freeze({ handle });
}
