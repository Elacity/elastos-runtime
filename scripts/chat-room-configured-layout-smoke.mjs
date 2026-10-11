#!/usr/bin/env node

import { createServer } from "node:http";
import { createRequire } from "node:module";
import { mkdir, mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../", import.meta.url)));
const browserRoot = join(repoRoot, "capsules/chat-room/browser");
const homeClipboardClient = join(repoRoot, "capsules/home/browser/home-clipboard-client.js");
const homeClipboardProtocol = join(repoRoot, "capsules/home/browser/home-clipboard-protocol.js");
const homeLayoutRoot = join(repoRoot, "capsules/home-gui/browser");
const homeNavigationClient = join(repoRoot, "capsules/home/browser/home-navigation-client.js");
const brave = process.env.BRAVE_BIN || "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";
const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const playwrightModule = process.env.ELASTOS_PLAYWRIGHT_MODULE
  ? await import(pathToFileURL(process.env.ELASTOS_PLAYWRIGHT_MODULE).href)
  : require("playwright");
const { chromium } = playwrightModule.chromium ? playwrightModule : playwrightModule.default;

function assert(condition, message, details = undefined) {
  if (!condition) {
    throw new Error(`${message}${details ? `\n${JSON.stringify(details, null, 2)}` : ""}`);
  }
}

function json(response, value, status = 200, headers = {}) {
  const body = Buffer.from(JSON.stringify(value));
  response.writeHead(status, {
    "access-control-allow-origin": "null",
    "cache-control": "no-store",
    "content-length": body.length,
    "content-type": "application/json",
    ...headers,
  });
  response.end(body);
}

function configuredPoll() {
  return {
    room_slug: "chat-room",
    display_name: "Configured User",
    expires_at: 4_000_000_000,
    latest_seq: 0,
    participants: [{
      display_name: "Configured User",
      device_label: "ElastOS shell",
      last_seen_at: 1,
      member_did: "did:key:z6configured",
      role: null,
      local_session_count: 1,
      is_current_session: true,
    }],
    objects: [],
    transport: {
      configured: true,
      available: true,
      connected_peer_count: 0,
      topic: "test-network/test-conversation",
      status: "Collaboration is configured; remote peer presence is not observed here.",
    },
  };
}

function directConversation() {
  return {
    conversation_id: "direct:sha256:fixture-conversation",
    display_name: "Fixture Friend",
    removed: false,
  };
}

function historyMessage(seq, text, createdAt) {
  return {
    seq, sender: "Fixture Friend", sender_profile_verified: true,
    from_current_session: false, kind: "text", body: text,
    created_at: Math.floor(Date.parse(createdAt) / 1000),
  };
}

function isDirectSwitchScenario(scenario) {
  return scenario === "direct-switch" || scenario.startsWith("direct-switch-hold-");
}

function holdBoundaryForScenario(scenario) {
  return {
    "direct-switch-hold-initial-conversations": "initial-conversations",
    "direct-switch-hold-bootstrap-messages": "bootstrap-messages",
    "direct-switch-hold-poll-conversations": "poll-conversations",
    "direct-switch-hold-poll-messages": "poll-messages",
  }[scenario] || null;
}

function createHold(label) {
  let release;
  const promise = new Promise((resolve) => {
    release = resolve;
  });
  return {
    label,
    promise,
    reached: false,
    released: false,
    release() {
      if (!this.released) {
        this.released = true;
        release();
      }
    },
  };
}

async function serveFile(response, pathname) {
  const relative = pathname === "/apps/chat-room/" ? "index.html" : pathname.slice("/apps/chat-room/".length);
  const path = join(browserRoot, relative);
  assert(path.startsWith(`${browserRoot}/`) || path === join(browserRoot, "index.html"), "invalid asset path");
  const body = await readFile(path);
  const contentType = {
    ".css": "text/css",
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript",
    ".wasm": "application/wasm",
  }[extname(path)] || "application/octet-stream";
  response.writeHead(200, {
    "access-control-allow-origin": "null",
    "content-length": body.length,
    "content-type": contentType,
  });
  response.end(body);
}

function startServer(scenario) {
  const initialDirectRecovery = scenario.startsWith("direct-initial-401");
  const sharedRecovery = scenario === "shared-poll-401";
  const contactAcceptance = scenario.startsWith("contact-acceptance");
  const historyScenario = scenario === "history-status-order-calendar";
  const unreadScenario = scenario === "direct-unread" || scenario === "visible-read";
  const retryScenario = scenario.startsWith("direct-retry");
  const holdBoundary = holdBoundaryForScenario(scenario);
  const holds = {
    "initial-conversations": holdBoundary === "initial-conversations"
      ? createHold("initial-conversations")
      : null,
    "bootstrap-messages": holdBoundary === "bootstrap-messages"
      ? createHold("bootstrap-messages")
      : null,
    "poll-conversations": holdBoundary === "poll-conversations"
      ? createHold("poll-conversations")
      : null,
    "poll-messages": holdBoundary === "poll-messages"
      ? createHold("poll-messages")
      : null,
    "retry": scenario === "direct-retry-held" ? createHold("retry") : null,
    "contact-request": scenario === "contact-acceptance-held-request"
      ? createHold("contact-request")
      : null,
  };
  const trace = {
    cycles: [],
    current: null,
    directConversations: 0,
    directListUnavailable: scenario === "direct-list-unavailable",
    directMessages: 0,
    freshDirectRequests: 0,
    heldBoundary: holdBoundary,
    heldReleases: [],
    leaves: 0,
    pollErrors: 0,
    rejectSharedPoll: false,
    peerAccepted: false,
    contactRequested: false,
    contactRequests: 0,
    contactResponses: 0,
    authPosts: 0,
    historyStatus: "searching",
    historyPhase: 0,
    pollCursors: [],
    directUnread: false,
    requests: [],
    reads: [],
    communityUnread: false,
    retryMode: "pending",
    retryPosts: [],
    sentAgain: null,
  };
  function pollView() {
    const poll = configuredPoll();
    if (historyScenario) {
      const details = {
        searching: "Checking recent Community history from online participants.",
        available: "Recent Community history is available from an online participant.",
        unavailable: "Recent Community history is unavailable. Another participant must be online to catch up.",
      };
      poll.transport.history = { status: trace.historyStatus, detail: details[trace.historyStatus] };
      poll.objects = trace.historyPhase < 2 ? [
        historyMessage(1, "today arrived first", "2026-03-09T00:05:00-04:00"),
        historyMessage(3, "yesterday same time second", "2026-03-08T12:00:00-04:00"),
        historyMessage(2, "yesterday recovered later", "2026-03-08T12:00:00-04:00"),
      ] : [
        historyMessage(6, "fall today", "2026-11-01T12:00:00-05:00"),
        historyMessage(5, "fall yesterday", "2026-10-31T12:00:00-04:00"),
      ];
      if (trace.historyPhase === 1) {
        poll.objects.push(historyMessage(4, "older catch-up", "2026-03-07T12:00:00-05:00"));
      }
      poll.latest_seq = trace.historyPhase === 2 ? 6 : trace.historyPhase === 1 ? 4 : 3;
    }
    if (contactAcceptance) {
      poll.participants.push({
        display_name: "Fixture Friend", profile_verified: true, device_label: "",
        last_seen_at: 1, local_session_count: 0, is_current_session: false,
        card: {
          participant_ref: "participant:fixture-friend",
          relationship: trace.peerAccepted ? "contact" : trace.contactRequested ? "requested" : "none",
          conversation_id: trace.peerAccepted ? directConversation().conversation_id : null,
          can_add_contact: !trace.peerAccepted && !trace.contactRequested, active_now: true,
        },
      });
    }
    return poll;
  }
  let directConversationResponses = 0;
  let directMessageResponses = 0;
  const server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url, "http://localhost");
      if (url.pathname.startsWith("/api/apps/chat-room")) {
        trace.requests.push(`${request.method} ${url.pathname}`);
      }
      if (request.method === "POST" && url.pathname.startsWith("/api/auth/")) trace.authPosts += 1;
      if (request.method === "OPTIONS") {
        response.writeHead(204, {
          "access-control-allow-headers": "content-type,x-elastos-home-token",
          "access-control-allow-methods": "GET,POST,OPTIONS",
          "access-control-allow-origin": "null",
        }).end();
        return;
      }
      if (["/shell-capsule-layout.js", "/shell-form-factor.js"].includes(url.pathname)) {
        const body = await readFile(join(homeLayoutRoot, url.pathname.slice(1)));
        response.writeHead(200, { "content-type":"application/javascript", "content-length":body.length });
        response.end(body); return;
      }
      if (url.pathname === "/fixture") {
        const homeOrigin = `http://127.0.0.1:${server.address().port}`;
        const chatSrc = `/apps/chat-room/?home_origin=${encodeURIComponent(homeOrigin)}${isDirectSwitchScenario(scenario) || initialDirectRecovery ? "&conversation_id=direct%3Asha256%3Afixture-conversation" : ""}#home_token=test-token`;
        const reconnectHost = initialDirectRecovery || sharedRecovery ? `<script>
          window.fixtureReconnects = 0;
          window.addEventListener("message", (event) => {
            const data = event.data;
            if (event.source !== document.querySelector("iframe").contentWindow || event.origin !== "null"
              || data?.type !== "home:launch-target" || data.target !== "chat-room"
              || data.homeToken !== "test-token" || typeof data.requestId !== "string") return;
            window.fixtureReconnects += 1;
            event.source.postMessage({ type: "home:shell-response", requestId: data.requestId,
              result: { target: "chat-room", attach_kind: "iframe", launch_status: "launched",
                route: "/apps/chat-room/?home_origin=" + encodeURIComponent(location.origin) + "#home_token=fresh-token" }, status: 0 }, "*");
          });
        </script>` : "";
        const body = Buffer.from(`<!doctype html><style>html,body{height:100%;margin:0}iframe{border:0;height:100%;width:100%}</style><iframe title="Chat" sandbox="allow-forms allow-modals allow-pointer-lock allow-scripts" src="${chatSrc}"></iframe>${reconnectHost}<script type="module">
          import { bindCapsuleLayout } from "/shell-capsule-layout.js";
          window.fixtureUnbindLayout = bindCapsuleLayout();
        </script>`);
        response.writeHead(200, {
          "content-length": body.length,
          "content-type": "text/html; charset=utf-8",
        });
        response.end(body);
        return;
      }
      if (url.pathname === "/apps/home/home-navigation-client.js") {
        const body = await readFile(homeNavigationClient);
        response.writeHead(200, {
          "access-control-allow-origin": "null",
          "content-length": body.length,
          "content-type": "text/javascript",
        });
        response.end(body);
        return;
      }
      if (url.pathname === "/apps/home/home-clipboard-client.js") {
        const body = await readFile(homeClipboardClient);
        response.writeHead(200, {
          "access-control-allow-origin": "null",
          "content-length": body.length,
          "content-type": "text/javascript",
        });
        response.end(body);
        return;
      }
      if (url.pathname === "/apps/home/home-clipboard-protocol.js") {
        const body = await readFile(homeClipboardProtocol);
        response.writeHead(200, {
          "access-control-allow-origin": "null",
          "content-length": body.length,
          "content-type": "text/javascript",
        });
        response.end(body);
        return;
      }
      if (url.pathname === "/api/apps/chat-room/summary") {
        if (scenario === "summary-failure") {
          return json(response, { error: "unavailable" }, 500);
        }
        await new Promise((resolveDelay) => setTimeout(resolveDelay, 250));
        return json(response, {
          room_slug: "chat-room",
          pending_count: 0,
          active_session_count: 0,
          browser_access_allowed: false,
          browser_access_block_reason: "Configured collaboration Chat is available only through its signed Home projection.",
          transport: {
            configured: true,
            available: true,
            connected_peer_count: 0,
            topic: "test-network/test-conversation",
            status: "Collaboration is configured; remote peer presence is not observed here.",
          },
        });
      }
      if (url.pathname === "/api/apps/chat-room/session/start") {
        trace.current.starts += 1;
        if (scenario === "session-failure") {
          return json(response, { error: "unauthorized" }, 401);
        }
        trace.current.ready = true;
        return json(
          response,
          {
            status: "connected",
            display_name: "Configured User",
            expires_at: 4_000_000_000,
            poll: pollView(),
          },
          200,
          { "set-cookie": "room-session=fixture-session; Max-Age=300; Path=/; HttpOnly; SameSite=Lax" },
        );
      }
      if (url.pathname === "/api/apps/chat-room/poll") {
        let body = "";
        for await (const chunk of request) body += chunk;
        const input = JSON.parse(body);
        if (historyScenario) trace.pollCursors.push(input.since);
        trace.reads.push({ scope:"shared", markRead:input.mark_read });
        if (input.mark_read === true) trace.communityUnread = false;
        trace.current.polls += 1;
        trace.current.pollBeforeReady ||= !trace.current.ready;
        trace.current.pollHeaders.push({
          authorization: request.headers.authorization || null,
          cookie: request.headers.cookie || null,
          homeToken: request.headers["x-elastos-home-token"] || null,
          origin: request.headers.origin || null,
        });
        if (sharedRecovery && trace.rejectSharedPoll) {
          trace.pollErrors += 1;
          return json(response, { error: "invalid or expired session" }, 401);
        }
        return json(response, pollView());
      }
      if (url.pathname === "/api/apps/chat-room/contacts/request" && contactAcceptance) {
        assert(request.method === "POST", "contact request method differs");
        let body = "";
        for await (const chunk of request) body += chunk;
        assert(JSON.parse(body).participant_ref === "participant:fixture-friend", "foreign participant requested");
        assert(request.headers["x-elastos-home-token"] === "test-token", "contact request lost Home authority");
        trace.contactRequests += 1;
        if (holds["contact-request"]) {
          holds["contact-request"].reached = true;
          await holds["contact-request"].promise;
        }
        trace.contactRequested = true;
        trace.contactResponses += 1;
        return json(response, { status: "requested" });
      }
      if (url.pathname === "/api/apps/chat-room/send") {
        trace.current.sends += 1;
        return json(response, { error: "unexpected send" }, 500);
      }
      if (url.pathname === "/api/apps/chat-room/direct/conversations") {
        trace.directConversations += 1;
        if (trace.directListUnavailable) {
          return json(response, { error: "direct messaging is unavailable on this Home", code: "direct_service_unavailable" }, 503);
        }
        if (initialDirectRecovery) {
          if (request.headers["x-elastos-home-token"] !== "fresh-token") {
            return json(response, { error: "Home session expired" }, 401);
          }
          trace.freshDirectRequests += 1;
        }
        if (scenario === "single-conversation" || (contactAcceptance && !trace.peerAccepted)) {
          return json(response, { conversations: [] });
        }
        directConversationResponses += 1;
        const hold = directConversationResponses === 1
          ? holds["initial-conversations"]
          : directConversationResponses === 3
            ? holds["poll-conversations"]
            : null;
        if (hold) {
          hold.reached = true;
          await hold.promise;
          trace.heldReleases.push(hold.label);
        }
        return json(response, { conversations: [{ ...directConversation(), unread: unreadScenario && trace.directUnread }],
          ...(url.searchParams.get("community_status") === "true" ? { community_unread:trace.communityUnread } : {}) });
      }
      if (url.pathname === "/api/apps/chat-room/direct/conversations/direct%3Asha256%3Afixture-conversation/messages") {
        trace.directMessages += 1;
        trace.reads.push({ scope:"direct", markRead:url.searchParams.get("mark_read") === "true" });
        if (unreadScenario && url.searchParams.get("mark_read") === "true") trace.directUnread = false;
        if (initialDirectRecovery) {
          if (request.headers["x-elastos-home-token"] !== "fresh-token") {
            return json(response, { error: "Home session expired" }, 401);
          }
          trace.freshDirectRequests += 1;
        }
        directMessageResponses += 1;
        const hold = directMessageResponses === 1
          ? holds["bootstrap-messages"]
          : directMessageResponses === 2
            ? holds["poll-messages"]
            : null;
        if (hold) {
          hold.reached = true;
          await hold.promise;
          trace.heldReleases.push(hold.label);
        }
        if (retryScenario) {
          const outgoing = { message_id:"message:retry-original", request_id:"chat-message:original",
            direction:"outgoing", text:"Original pending text", created_at:1_725_000_000,
            delivery_state:trace.retryMode === "settled" ? "receipt_settled" : "pending" };
          return json(response, { conversation_id:directConversation().conversation_id,
            messages:[outgoing, ...(trace.sentAgain ? [trace.sentAgain] : [])] });
        }
        return json(response, {
          conversation_id: directConversation().conversation_id,
          messages: [{
            message_id: "message:fixture-direct",
            direction: "incoming",
            text: "hello from direct",
            created_at: 1_725_000_000,
            delivery_state: "received",
          }],
        });
      }
      if (url.pathname === "/api/apps/chat-room/session/leave") {
        trace.leaves += 1;
        return json(response, { status: "disconnected" });
      }
      if (url.pathname === "/api/apps/chat-room/direct/messages/send" && retryScenario) {
        let raw = ""; for await (const chunk of request) raw += chunk;
        const input = JSON.parse(raw); trace.retryPosts.push(input);
        assert(input.conversation_id === directConversation().conversation_id && input.text === "Original pending text",
          "Retry replaced the stored message intent", input);
        if (input.retry_existing) {
          assert(input.request_id === "chat-message:original", "Retry minted a replacement request ID", input);
          if (holds.retry) { holds.retry.reached = true; await holds.retry.promise; }
          if (scenario === "direct-retry-terminal") return json(response, { code:"retry_expired", error:"expired" }, 410);
          if (scenario === "direct-retry-unavailable") return json(response, { code:"retry_unavailable", error:"unavailable" }, 410);
          trace.retryMode = "settled";
          return json(response, { status:"receipt_settled" }, 200);
        }
        assert(input.request_id !== "chat-message:original", "Send again reused expired signed intent", input);
        trace.sentAgain = { message_id:"message:retry-new", request_id:input.request_id,
          direction:"outgoing", text:input.text, created_at:1_725_000_001, delivery_state:"pending" };
        return json(response, { status:"pending" }, 202);
      }
      if (url.pathname.startsWith("/apps/chat-room/")) {
        if (url.pathname === "/apps/chat-room/") {
          trace.current = {
            pollBeforeReady: false,
            pollHeaders: [],
            polls: 0,
            ready: false,
            sends: 0,
            starts: 0,
          };
          trace.cycles.push(trace.current);
        }
        await serveFile(response, url.pathname);
        return;
      }
      response.writeHead(404).end();
    } catch (error) {
      response.writeHead(500, { "content-type": "text/plain" }).end(String(error));
    }
  });
  return new Promise((resolveServer, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => resolveServer({
      server,
      trace,
      holds,
    }));
  });
}

async function waitFor(check, timeoutMs = 15_000) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const value = await check();
      if (value) return value;
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 100));
  }
  throw lastError || new Error("timed out waiting for configured Chat UI");
}

async function chatFrame(page) {
  const handle = await page.waitForSelector('iframe[title="Chat"]');
  const frame = await handle.contentFrame();
  assert(frame, "opaque Chat frame is missing");
  await frame.waitForLoadState("domcontentloaded");
  return frame;
}

async function waitForConfiguredChatWithoutLegacyFlash(frame, label) {
  const deadline = Date.now() + 15_000;
  let firstViolation = null;
  while (Date.now() < deadline) {
    const state = await frame.evaluate(() => {
      const visible = (selector) => {
        const node = document.querySelector(selector);
        if (!node || node.hidden || node.getClientRects().length === 0) return false;
        const style = getComputedStyle(node);
        return style.display !== "none" && style.visibility !== "hidden";
      };
      const legacySelectors = [
        "#attach-button",
        "#browser-access-section",
        "#browser-access-stage",
        "#conversation-invite-create",
        "#room-access-section",
        "#room-access-toggle",
      ];
      return {
        ready: !!document.querySelector("#chat-card") && !!document.querySelector("#participant-count"),
        active: document.body?.dataset.roomSessionActive === "true",
        participantCount: document.querySelector("#participant-count")?.textContent,
        visibleLegacy: legacySelectors.filter(visible),
      };
    });
    if (!state.ready) {
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 20));
      continue;
    }
    if (!firstViolation && state.visibleLegacy.length > 0) {
      firstViolation = state;
    }
    if (!state.active && state.participantCount !== "Opening conversation") {
      firstViolation ||= state;
    }
    if (state.active) {
      assert(!firstViolation, `${label} exposed legacy controls before configured Chat opened`, firstViolation);
      return;
    }
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 20));
  }
  throw new Error(`${label} timed out before configured Chat opened`);
}

async function reopenOpaqueChat(page) {
  await page.evaluate(() => {
    const previous = document.querySelector('iframe[title="Chat"]');
    if (!previous) throw new Error("opaque Chat frame is missing");
    const frame = document.createElement("iframe");
    frame.title = "Chat";
    frame.setAttribute("sandbox", "allow-forms allow-modals allow-pointer-lock allow-scripts");
    frame.src = `/apps/chat-room/?home_origin=${encodeURIComponent(location.origin)}#home_token=test-token`;
    previous.replaceWith(frame);
  });
}

async function clickSharedChoice(frame, programmatic = false) {
  if (programmatic) {
    await frame.evaluate(() => {
      const button = document.querySelector('[data-conversation-choice="shared"]');
      if (!(button instanceof HTMLElement)) {
        throw new Error("shared choice is missing");
      }
      button.click();
    });
    return;
  }
  await frame.locator('[data-conversation-choice="shared"]').click();
}

async function directSwitchState(frame) {
  return frame.evaluate(() => ({
    active: document.body?.dataset?.roomSessionActive === "true",
    chatMode: document.body?.dataset?.chatMode || "",
    selected: document.querySelector("[data-conversation-choice].active")
      ?.dataset?.conversationChoice || "",
    participantCount: document.querySelector("#participant-count")?.textContent || "",
    errorText: document.querySelector("#error-text")?.textContent || "",
    conversationTitle: document.querySelector("#conversation-title")?.textContent || "",
    conversationDetail: document.querySelector("#conversation-detail")?.textContent || "",
    attachHidden: (() => {
      const node = document.querySelector("#attach-button");
      if (!node || node.hidden || node.getClientRects().length === 0) return true;
      const style = getComputedStyle(node);
      return style.display === "none" || style.visibility === "hidden";
    })(),
    sharedChoices: [...document.querySelectorAll("[data-conversation-choice]")]
      .map((node) => node.dataset.conversationChoice || ""),
  }));
}

async function runScenario(scenario) {
  const { server, trace, holds } = await startServer(scenario);
  const profile = await mkdtemp(join(tmpdir(), "elastos-chat-layout-"));
  const port = server.address().port;
  const url = `http://127.0.0.1:${port}/fixture`;
  const context = await chromium.launchPersistentContext(profile, {
    executablePath: brave,
    headless: true,
    viewport: { width: 1280, height: 900 },
    ...(scenario === "history-status-order-calendar" ? { timezoneId: "America/New_York" } : {}),
  });
  const page = context.pages()[0] || await context.newPage();

  try {
    if (scenario === "history-status-order-calendar") {
      await page.clock.setFixedTime("2026-03-09T04:30:00Z");
    }
    await page.goto(url, { waitUntil: "domcontentloaded" });
    let frame = await chatFrame(page);

    if (scenario === "direct-list-unavailable") {
      await waitForConfiguredChatWithoutLegacyFlash(frame, "unavailable Direct list");
      const error = frame.locator("#error-text");
      const detail = "Direct conversations are temporarily unavailable.";
      await error.waitFor({ state: "visible" });
      assert(await error.innerText() === detail, "initial Direct list failure is silent");
      const draft = "Keep this Community draft through Direct list recovery";
      await frame.locator("#message-input").fill(draft);
      await waitFor(() => trace.directConversations >= 3);
      assert(await error.isVisible() && await error.innerText() === detail,
        "successful Community refresh cleared a current Direct list failure");
      assert(await frame.locator("#message-input").inputValue() === draft, "failed list read changed the draft");
      trace.directListUnavailable = false;
      await error.waitFor({ state: "hidden" });
      await frame.locator('[data-conversation-choice="direct:sha256:fixture-conversation"]').waitFor({ state: "visible" });
      assert(await frame.locator("#message-input").inputValue() === draft, "recovered list read changed the draft");
      trace.directListUnavailable = true;
      await error.waitFor({ state: "visible" });
      assert(await error.innerText() === detail, "later Direct list failure is silent");
      trace.directListUnavailable = false;
      await error.waitFor({ state: "hidden" });
      assert(await frame.locator("#message-input").inputValue() === draft, "second recovery changed the draft");
      return;
    }

    if (scenario === "shell-layout-visibility") {
      await frame.evaluate(() => {
        window.fixtureLayouts = [];
        window.addEventListener("message", event => {
          if (event.source === parent && event.data?.type === "elastos:shell-layout") window.fixtureLayouts.push(event.data.layout);
        });
        parent.postMessage({ type:"elastos:shell-layout", request:true }, new URL(location.href).searchParams.get("home_origin"));
      });
      const visibility = async expected => frame.waitForFunction(expected => window.fixtureLayouts.at(-1)?.visible === expected, expected);
      await visibility(true);
      for (const style of ["visibility:hidden", "display:none", "opacity:0"]) {
        await page.locator('iframe[title="Chat"]').evaluate((node, style) => { node.style.cssText = style; }, style);
        await visibility(false);
        await page.locator('iframe[title="Chat"]').evaluate(node => { node.style.cssText = ""; });
        await visibility(true);
      }
      await page.locator('iframe[title="Chat"]').evaluate(node => { node.dataset.spaceVisible = "false"; });
      await visibility(false);
      await page.locator('iframe[title="Chat"]').evaluate(node => { node.dataset.spaceVisible = "true"; });
      await visibility(true);
      await page.evaluate(() => {
        Object.defineProperty(document, "hidden", { configurable:true, value:true });
        document.dispatchEvent(new Event("visibilitychange"));
      });
      await visibility(false);
      await page.evaluate(() => { delete document.hidden; document.dispatchEvent(new Event("visibilitychange")); });
      await visibility(true);
      assert(trace.authPosts === 0, "presentation snapshot performed authority work");
      return;
    }

    if (scenario === "history-status-order-calendar") {
      await waitForConfiguredChatWithoutLegacyFlash(frame, "history bootstrap");
      const draft = "Keep this draft while recent history is checked";
      await frame.locator("#message-input").fill(draft);
      const historyStatus = async (status) => {
        await frame.waitForFunction(expected =>
          document.querySelector("#conversation-detail")?.dataset.historyStatus === expected, status);
        assert(await frame.locator("#conversation-detail").isVisible(), "history status is hidden in shell mode");
        assert(await frame.locator("#message-input").isEnabled() && await frame.locator("#send-button").isEnabled()
          && !(await frame.locator("#reconnect-button").isVisible()), "history status changed live session controls");
        assert(await frame.locator("#message-input").inputValue() === draft, "history status replaced the draft");
      };
      const messageOrder = async (expected) => frame.waitForFunction(order =>
        [...document.querySelectorAll("#message-list [data-seq]")].map(node => Number(node.dataset.seq)).join(",")
          === order.join(","), expected);
      const dayFor = async (seq) => frame.evaluate(id => {
        let node = document.querySelector(`#message-list [data-seq="${id}"]`)?.previousElementSibling;
        while (node && !node.classList.contains("day-separator")) node = node.previousElementSibling;
        return node?.textContent.trim();
      }, seq);
      await historyStatus("searching");
      await messageOrder([2, 3, 1]);
      assert(await dayFor(2) === "Yesterday" && await dayFor(1) === "Today",
        "spring DST labels use elapsed hours instead of local calendar days");
      trace.historyStatus = "available";
      trace.historyPhase = 1;
      await historyStatus("available");
      await messageOrder([4, 2, 3, 1]);
      await waitFor(() => trace.pollCursors.includes(4));
      trace.historyStatus = "unavailable";
      await historyStatus("unavailable");
      assert((await frame.locator("#conversation-detail").innerText()).includes("Another participant must be online"),
        "unavailable history does not explain how to catch up");
      await page.setViewportSize({ width: 375, height: 900 });
      await frame.waitForFunction(() => {
        const detail = document.querySelector("#conversation-detail");
        return innerWidth === 375 && detail && getComputedStyle(detail).whiteSpace === "normal"
          && detail.scrollWidth <= detail.clientWidth + 1 && detail.scrollHeight <= detail.clientHeight + 1;
      });
      await historyStatus("unavailable");
      await page.setViewportSize({ width: 1280, height: 900 });

      await frame.locator('[data-conversation-choice="direct:sha256:fixture-conversation"]').click();
      await frame.waitForFunction(() => document.body.dataset.chatMode === "direct");
      assert(await frame.locator("#conversation-detail").innerText() === "Direct message"
        && await frame.locator("#conversation-detail").getAttribute("data-history-status") === null,
      "Community history status leaked into a Direct conversation");
      await clickSharedChoice(frame);
      await historyStatus("unavailable");
      await page.clock.setFixedTime("2026-11-02T04:30:00Z");
      trace.historyPhase = 2;
      trace.historyStatus = "available";
      await historyStatus("available");
      await messageOrder([4, 2, 3, 1, 5, 6]);
      assert(await dayFor(5) === "Yesterday" && await dayFor(6) === "Today",
        "fall DST labels use elapsed hours instead of local calendar days");
      await waitFor(() => trace.pollCursors.includes(6));
      assert(trace.cycles[0].starts === 1 && trace.authPosts === 0 && trace.cycles[0].sends === 0,
        "history display performed session or send work", trace);
      return;
    }

    if (scenario.startsWith("direct-retry")) {
      await waitForConfiguredChatWithoutLegacyFlash(frame, "retry bootstrap");
      await frame.locator('[data-conversation-choice="direct:sha256:fixture-conversation"]').click();
      const original = frame.locator('[data-direct-message-id="message:retry-original"]');
      await original.locator('[data-direct-action="retry"]').waitFor({ state:"visible" });
      assert((await original.innerText()).includes("Sending"), "pending delivery copy is inaccurate");
      const draft = "Keep my current draft while retrying a different message";
      await frame.locator("#message-input").fill(draft);
      await original.locator('[data-direct-action="retry"]').click();
      await waitFor(() => trace.retryPosts.length === 1);
      if (holds.retry) {
        await frame.waitForFunction(() => document.querySelector('[data-direct-message-id="message:retry-original"]')?.textContent.includes("Sending"));
        assert(await frame.locator("#send-button").isDisabled()
          && await original.locator('[data-direct-action="retry"]').isDisabled(), "in-flight send controls permit duplicate intent");
        await clickSharedChoice(frame);
        await frame.locator("#message-input").fill("Different Community draft");
        holds.retry.release();
        await waitFor(() => trace.retryMode === "settled");
        await new Promise(resolveDelay => setTimeout(resolveDelay, 1200));
        assert(await frame.locator("#message-input").inputValue() === "Different Community draft"
          && (await frame.locator("#error-text").innerText()) === "", "stale Retry changed the new selection");
        await frame.locator('[data-conversation-choice="direct:sha256:fixture-conversation"]').click();
      }
      if (["direct-retry-terminal", "direct-retry-unavailable"].includes(scenario)) {
        await original.locator('[data-direct-action="send-again"]').waitFor({ state:"visible" });
        const text = await original.innerText();
        assert(text.includes("Delivery is unconfirmed") && text.includes(scenario === "direct-retry-terminal" ? "24-hour" : "unavailable"),
          "terminal refusal lacks honest recovery explanation", text);
        assert(await frame.locator("#message-input").inputValue() === draft, "terminal refusal changed the composer");
        await original.locator('[data-direct-action="send-again"]').click();
        await frame.locator('[data-direct-message-id="message:retry-new"]').waitFor({ state:"visible" });
        assert(trace.retryPosts.length === 2 && trace.retryPosts[1].retry_existing === false
          && trace.retryPosts[1].request_id !== trace.retryPosts[0].request_id,
          "explicit Send again did not create one fresh intent", trace.retryPosts);
        assert(await original.count() === 1 && await frame.locator("#message-input").inputValue() === draft,
          "Send again replaced the original record or current draft");
      } else {
        await frame.waitForFunction(() => document.querySelector('[data-direct-message-id="message:retry-original"]')?.textContent.includes("Sent"));
        assert(await original.locator('[data-direct-action]').count() === 0, "settled receipt retains Retry");
        assert(await frame.locator("#message-input").inputValue() === draft, "Retry cleared the current draft");
        assert(trace.retryPosts.length === 1 && trace.retryPosts[0].retry_existing === true, "Retry sent another fresh intent");
      }
      return;
    }

    if (scenario === "visible-read") {
      await waitForConfiguredChatWithoutLegacyFlash(frame, "visibility bootstrap");
      const shellFrame = page.locator('iframe[title="Chat"]');
      const sharedDot = frame.locator('[data-conversation-choice="shared"] .conversation-unread-dot');
      const hide = async (style) => {
        await shellFrame.evaluate((node, style) => { node.style.cssText = style; }, style);
        const count = trace.reads.length;
        await waitFor(() => trace.reads.length >= count + 3);
        assert(trace.reads.slice(-2).every(read => read.markRead === false), "hidden Chat marked a conversation read", trace.reads.slice(-3));
      };
      const restore = async () => {
        await shellFrame.evaluate(node => { node.style.cssText = ""; node.dataset.spaceVisible = "true"; });
        await waitFor(() => trace.reads.at(-1)?.markRead === true);
      };
      for (const style of ["visibility:hidden", "display:none", "opacity:0"]) {
        await hide(style);
        trace.communityUnread = true;
        await sharedDot.waitFor({ state:"attached" });
        assert(trace.communityUnread, "background Community poll erased unread truth");
        await restore();
        await waitFor(() => !trace.communityUnread);
        await sharedDot.waitFor({ state:"detached" });
      }
      await shellFrame.evaluate(node => { node.dataset.spaceVisible = "false"; });
      await hide("");
      trace.communityUnread = true;
      await sharedDot.waitFor({ state:"attached" });
      // Sibling and wrong-origin messages cannot override Home's hidden decision.
      await page.evaluate(() => {
        const frame = document.querySelector('iframe[title="Chat"]');
        const sibling = document.createElement("iframe"); document.body.appendChild(sibling);
        sibling.contentWindow.eval(`parent.document.querySelector('iframe[title="Chat"]').contentWindow.postMessage({type:'elastos:shell-layout',layout:{visible:true}}, '*')`);
        sibling.remove();
        window.fixtureUnbindLayout();
      });
      await frame.evaluate(() => {
        window.dispatchEvent(new MessageEvent("message", { source:parent, origin:"https://foreign.example",
          data:{ type:"elastos:shell-layout", layout:{visible:true} } }));
      });
      const count = trace.reads.length;
      await waitFor(() => trace.reads.length >= count + 2);
      assert(trace.reads.slice(-2).every(read => !read.markRead), "forged sibling changed read visibility");
      await page.evaluate(async () => {
        const { bindCapsuleLayout } = await import("/shell-capsule-layout.js");
        window.fixtureUnbindLayout = bindCapsuleLayout();
      });
      await restore();
      await page.evaluate(() => {
        window.fixtureParentHidden = true;
        Object.defineProperty(document, "hidden", { configurable:true, get:() => window.fixtureParentHidden });
        document.dispatchEvent(new Event("visibilitychange"));
      });
      const hiddenCount = trace.reads.length;
      await waitFor(() => trace.reads.length >= hiddenCount + 2);
      assert(trace.reads.slice(-2).every(read => !read.markRead), "parent visibilitychange kept read acknowledgements enabled");
      await page.evaluate(() => { delete document.hidden; document.dispatchEvent(new Event("visibilitychange")); });
      await restore();
      trace.directUnread = true;
      await frame.locator('[data-conversation-choice="direct:sha256:fixture-conversation"]').click();
      await waitFor(() => !trace.directUnread);
      await hide("visibility:hidden");
      trace.directUnread = true;
      const countDirect = trace.directMessages;
      await waitFor(() => trace.directMessages >= countDirect + 2);
      assert(trace.directUnread, "hidden selected Direct erased unread truth");
      await restore();
      await waitFor(() => !trace.directUnread);
      assert(trace.authPosts === 0 && trace.cycles[0].starts === 1, "visibility update performed authority work");
      return;
    }

    if (scenario === "direct-unread") {
      await waitForConfiguredChatWithoutLegacyFlash(frame, "unread bootstrap");
      const draft = "Keep this Community draft while reading a Direct message";
      await frame.locator("#message-input").fill(draft);
      const choice = frame.locator('[data-conversation-choice="direct:sha256:fixture-conversation"]');
      assert(await choice.locator(".conversation-unread-dot").count() === 0, "read conversation already has an unread dot");
      trace.directUnread = true;
      await frame.waitForFunction(() => {
        const choice = document.querySelector('[data-conversation-choice="direct:sha256:fixture-conversation"]');
        return choice?.classList.contains("unread") && !!choice.querySelector(".conversation-unread-dot")
          && choice.querySelector(".conversation-choice-detail")?.textContent === "New message";
      });
      assert(trace.directMessages === 0, "unread refresh opened the conversation before the person chose it");
      await choice.click();
      await frame.waitForFunction(() => document.body.dataset.chatMode === "direct"
        && document.querySelector("#message-list")?.textContent.includes("hello from direct"));
      assert(!trace.directUnread, "opening Direct did not use the existing conversation read path");
      await clickSharedChoice(frame);
      await frame.waitForFunction(() => {
        const choice = document.querySelector('[data-conversation-choice="direct:sha256:fixture-conversation"]');
        return !!choice && !choice.classList.contains("unread") && !choice.querySelector(".conversation-unread-dot");
      });
      assert(await frame.locator("#message-input").inputValue() === draft, "opening unread Direct replaced the Shared draft");
      trace.directUnread = true;
      await choice.locator(".conversation-unread-dot").waitFor({ state: "visible" });
      assert((await choice.innerText()).includes("New message"), "a later message did not restore the unread state");
      return;
    }

    if (scenario.startsWith("contact-acceptance")) {
      await waitForConfiguredChatWithoutLegacyFlash(frame, "contact acceptance bootstrap");
      const draft = "Keep this draft while a contact is accepted";
      await frame.locator("#message-input").fill(draft);
      assert(await frame.locator('[data-conversation-choice="direct:sha256:fixture-conversation"]').count() === 0,
        "unaccepted contact appeared in the Direct rail");
      await frame.locator("#participant-toggle").click();
      await frame.locator('#participant-list [data-participant-ref="participant:fixture-friend"]').click();
      await frame.waitForFunction(() => document.querySelector("#participant-card-action")?.textContent === "Add contact");
      await frame.locator("#participant-card-action").click();
      await waitFor(() => trace.contactRequests === 1);
      if (holds["contact-request"]) {
        const polls = trace.cycles[0].polls;
        await waitFor(() => trace.cycles[0].polls >= polls + 2);
        assert(await frame.locator("#participant-card-action").isDisabled(),
          "an unchanged poll re-enabled the in-flight contact request");
      } else {
        await frame.waitForFunction(() => document.querySelector("#participant-card-action")?.hidden === true
          && document.querySelector("#participant-card-state")?.textContent.includes("Waiting"));
      }
      await frame.locator("#participant-card-close").focus();
      trace.peerAccepted = true;
      await frame.waitForFunction(() => {
        const card = document.querySelector("#participant-card");
        const action = document.querySelector("#participant-card-action");
        return card?.hidden === false && card.dataset.participantRef === "participant:fixture-friend"
          && document.querySelector("#participant-card-state")?.textContent === "Contact · Active now"
          && action?.hidden === false && !action.disabled && action.dataset.cardAction === "message"
          && action.textContent === "Message"
          && !!document.querySelector('[data-conversation-choice="direct:sha256:fixture-conversation"]');
      });
      assert(await frame.evaluate(() => document.activeElement?.id) === "participant-card-close",
        "acceptance refresh moved focus or reopened the card");
      assert(trace.directMessages === 0 && trace.cycles[0].sends === 0,
        "card/rail refresh depended on a new message", trace);
      assert(await frame.locator("#message-input").inputValue() === draft, "acceptance refresh replaced the draft");
      if (holds["contact-request"]) {
        await frame.evaluate(() => {
          const card = document.querySelector("#participant-card");
          window.fixtureCardRegressed = false;
          window.fixtureCardObserver = new MutationObserver(() => {
            const action = document.querySelector("#participant-card-action");
            window.fixtureCardRegressed ||= card.hidden
              || document.querySelector("#participant-card-state")?.textContent !== "Contact · Active now"
              || action?.hidden !== false || action.disabled || action.dataset.cardAction !== "message";
          });
          window.fixtureCardObserver.observe(card, { subtree: true, childList: true, characterData: true, attributes: true });
        });
        const responseFinished = page.waitForResponse(response => response.request().method() === "POST"
          && new URL(response.url()).pathname === "/api/apps/chat-room/contacts/request")
          .then(response => response.finished());
        const polls = trace.cycles[0].polls;
        holds["contact-request"].release();
        await responseFinished;
        await waitFor(() => trace.contactResponses === 1 && trace.cycles[0].polls > polls);
        const regressed = await frame.evaluate(() => {
          window.fixtureCardObserver.disconnect();
          return window.fixtureCardRegressed;
        });
        assert(!regressed, "a held request response briefly reverted accepted contact state");
        assert(await frame.locator("#participant-card-state").innerText() === "Contact · Active now"
          && await frame.locator("#participant-card-action").isVisible()
          && await frame.locator("#participant-card-action").innerText() === "Message",
        "a held request response reverted accepted contact state");
        assert(await frame.evaluate(() => document.activeElement?.id) === "participant-card-close",
          "held request completion moved the card focus");
      }
      assert(trace.contactRequests === 1, "acceptance refresh repeated the contact request", trace);
      await frame.locator("#participant-card-action").click();
      await frame.waitForFunction(() => document.body.dataset.chatMode === "direct"
        && document.querySelector("[data-conversation-choice].active")?.dataset.conversationChoice
          === "direct:sha256:fixture-conversation");
      return;
    }

    if (scenario.startsWith("direct-initial-401")) {
      const originalFrame = frame;
      const timeOrigin = await frame.evaluate(() => performance.timeOrigin);
      await frame.locator("#reconnect-button").waitFor({ state: "visible" });
      await new Promise(resolveDelay => setTimeout(resolveDelay, 2200));
      assert(trace.directConversations === 1 && trace.directMessages === 0 && trace.cycles[0].starts === 0,
        "initial Direct401 performed automatic recovery work", trace);
      assert(await page.evaluate(() => window.fixtureReconnects) === 0, "initial Direct401 reopened itself");
      const selectedByUser = scenario === "direct-initial-401-user-selection";
      if (selectedByUser) await clickSharedChoice(frame);
      await frame.locator("#error-text").waitFor({ state: "visible" });
      assert((await frame.locator("#error-text").innerText()).trim(),
        "Reconnect lost its visible recovery explanation");
      await frame.locator("#reconnect-button").click();
      await frame.waitForFunction((shared) => document.body.dataset.roomSessionActive === "true"
        && document.querySelector("#reconnect-button")?.hidden === true
        && document.querySelector("[data-conversation-choice].active")?.dataset.conversationChoice
          === (shared ? "shared" : "direct:sha256:fixture-conversation"), selectedByUser);
      frame = await chatFrame(page);
      assert(frame === originalFrame && await frame.evaluate(() => performance.timeOrigin) === timeOrigin,
        "initial Direct401 recovery replaced its draft-owning document");
      assert(trace.cycles.length === 1 && trace.cycles[0].starts === 1,
        "initial Direct401 recovery did not use one explicit session start", trace);
      assert(await page.evaluate(() => window.fixtureReconnects) === 1, "initial Direct401 did not use one Home launch");
      if (!selectedByUser) {
        await frame.waitForFunction(() => document.querySelector("#message-list")?.textContent.includes("hello from direct"));
        assert(trace.freshDirectRequests >= 2, "requested Direct was not verified with fresh Home authority", trace);
      }
      return;
    }

    if (scenario === "session-failure" || scenario === "summary-failure") {
      const expected = scenario === "session-failure"
        ? "Chat session bootstrap was not authorized. Reopen Chat from Home."
        : "Chat session bootstrap failed. Reopen Chat from Home.";
      await frame.waitForFunction(
        (message) => document.querySelector("#error-text")?.textContent === message,
        expected,
      );
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 2_200));
      const cycle = trace.cycles[0];
      assert(
        cycle.polls === 0 && cycle.sends === 0,
        "bootstrap failure started poll/send work",
        { scenario, cycle },
      );
      assert(
        scenario === "summary-failure" ? cycle.starts === 0 : cycle.starts === 1,
        "bootstrap failure retried or skipped the canonical start boundary",
        { scenario, cycle },
      );
      assert(
        await frame.evaluate(() => document.body.dataset.roomSessionActive) === "false",
        "bootstrap failure activated Chat",
      );
      return;
    }

    if (isDirectSwitchScenario(scenario)) {
      const heldBoundary = holdBoundaryForScenario(scenario);
      const preBootstrapHold = heldBoundary === "initial-conversations"
        || heldBoundary === "bootstrap-messages";
      if (heldBoundary === "initial-conversations" || heldBoundary === "bootstrap-messages") {
        await frame.waitForSelector('[data-conversation-choice="shared"]', {
          state: "attached",
          timeout: 15_000,
        });
      } else {
        await frame.waitForFunction(() => {
          const selected = document.querySelector("[data-conversation-choice].active")
            ?.dataset?.conversationChoice;
          return document.body?.dataset?.chatMode === "direct"
            && selected === "direct:sha256:fixture-conversation"
            && document.querySelector("#message-list")?.textContent?.includes("hello from direct");
        }, null, { timeout: 15_000 });
      }

      if (heldBoundary) {
        await waitFor(() => holds[heldBoundary]?.reached);
      } else {
        assert(
          trace.directConversations >= 1 && trace.directMessages >= 1,
          "direct bootstrap did not load the configured direct conversation",
          trace,
        );
      }

      const preClickTrace = structuredClone(trace);
      await clickSharedChoice(frame, preBootstrapHold);
      if (heldBoundary) {
        holds[heldBoundary].release();
      }
      let switched;
      let lastState = null;
      try {
        switched = await waitFor(async () => {
          const state = await directSwitchState(frame);
          lastState = state;
          return state.active
            && state.chatMode === "shared"
            && state.selected === "shared"
            && state.attachHidden
            ? state
            : false;
        });
      } catch (error) {
        throw new Error(`direct-switch state did not converge\n${JSON.stringify({
          heldBoundary,
          lastState,
          preClickTrace,
          trace,
        }, null, 2)}`);
      }
      if (!heldBoundary) {
        await waitFor(() => trace.cycles[0]?.polls === 1);
        const sharedCycle = trace.cycles[0];
        assert(sharedCycle.starts === 1, "shared selection did not create exactly one shared session", sharedCycle);
        assert(sharedCycle.polls === 1 && !sharedCycle.pollBeforeReady, "shared selection polled before bootstrap", sharedCycle);
      }
      assert(
        switched.chatMode === "shared"
          && switched.selected === "shared"
          && switched.conversationTitle === "Community"
          && switched.conversationDetail === "Shared room"
          && switched.errorText === ""
          && switched.attachHidden,
        "direct-to-shared switch did not leave configured Chat in shared mode",
        { heldBoundary, ...switched, preClickTrace, trace },
      );
      return;
    }

    await waitForConfiguredChatWithoutLegacyFlash(frame, "initial opaque Chat load");
    await waitFor(() => trace.cycles[0]?.polls === 1);
    const firstCycle = trace.cycles[0];
    assert(firstCycle.starts === 1, "initial load did not create exactly one session", firstCycle);
    assert(firstCycle.polls === 1 && !firstCycle.pollBeforeReady, "initial poll ordering regressed", firstCycle);
    assert(firstCycle.pollHeaders.every((headers) =>
      headers.homeToken === "test-token"
        && headers.authorization === null
        && headers.cookie === null
        && headers.origin === "null"
    ), "opaque Chat poll depended on browser credentials", firstCycle);
    const authoritySurface = await frame.evaluate(() => {
        let storageAvailable = true;
        try { void localStorage.length; void sessionStorage.length; } catch { storageAvailable = false; }
        return {
          origin: self.origin,
          storageAvailable,
          leaked: document.documentElement.outerHTML.includes("session_token")
            || document.documentElement.textContent.includes("session_token")
            || location.href.includes("session_token"),
        };
      });
    assert(authoritySurface.origin === "null", "Chat fixture is not opaque", authoritySurface);
    assert(!authoritySurface.storageAvailable && !authoritySurface.leaked, "room credential reached a client truth surface", authoritySurface);

    if (scenario !== "single-conversation") {
      await frame.waitForSelector('[data-conversation-choice="direct:sha256:fixture-conversation"]');
    }

    for (const width of scenario === "single-conversation" ? [640] : [375, 640, 1280]) {
      await page.setViewportSize({ width, height: 900 });
      try {
        await frame.waitForFunction(
          ({ expectedWidth, singleConversation }) => {
            const sidebar = document.querySelector(".chat-sidebar");
            const sidebarHidden = !sidebar
              || sidebar.hidden
              || getComputedStyle(sidebar).display === "none";
            const frameWidth = window.innerWidth;
            const compact = window.matchMedia("(max-width: 760px)").matches;
            if (frameWidth !== expectedWidth || compact !== (expectedWidth <= 760)) {
              return false;
            }
            if (singleConversation) {
              return document.body.dataset.roomCompactRail === "hidden" && sidebarHidden;
            }
            const sidebarWidth = sidebar?.getBoundingClientRect().width || 0;
            const expectedSidebarWidth = expectedWidth <= 760 ? 72 : 220;
            return Math.abs(sidebarWidth - expectedSidebarWidth) <= 1;
          },
          { expectedWidth: width, singleConversation: scenario === "single-conversation" },
        );
      } catch {
        const failureState = await frame.evaluate((expectedLoopWidth) => {
          const sidebar = document.querySelector(".chat-sidebar");
          return {
            expectedLoopWidth,
            frameWidth: window.innerWidth,
            compact: window.matchMedia("(max-width: 760px)").matches,
            sidebarWidth: sidebar?.getBoundingClientRect().width || 0,
            sidebarHidden: !sidebar
              || sidebar.hidden
              || getComputedStyle(sidebar).display === "none",
            compactRail: document.body.dataset.roomCompactRail || "",
          };
        }, width);
        const topWidth = await page.evaluate(() => window.innerWidth);
        throw new Error(`responsive layout did not settle\n${JSON.stringify({ topWidth, ...failureState }, null, 2)}`);
      }
      const topWidth = await page.evaluate(() => window.innerWidth);
      const state = await frame.evaluate((expectedLoopWidth) => {
            const hidden = (selector) => {
              const node = document.querySelector(selector);
              return !node || node.hidden || getComputedStyle(node).display === "none" || getComputedStyle(node).visibility === "hidden";
            };
            const input = document.querySelector("#message-input");
            const send = document.querySelector("#send-button");
            const shell = document.querySelector("#chat-card");
            const sidebar = document.querySelector(".chat-sidebar");
            const thread = document.querySelector(".chat-thread");
            const selector = document.querySelector("#conversation-selector");
            const sidebarRect = sidebar?.getBoundingClientRect();
            const threadRect = thread?.getBoundingClientRect();
            return {
              expectedLoopWidth,
              topWidth: 0,
              width: innerWidth,
              compact: window.matchMedia("(max-width: 760px)").matches,
              active: document.body.dataset.roomSessionActive,
              attachHidden: hidden("#attach-button"),
              browserStageHidden: hidden("#browser-access-stage"),
              browserRequestsHidden: hidden("#browser-access-section"),
              roomSettingsHidden: hidden("#room-access-toggle") && hidden("#room-access-section"),
              textVisible: !hidden("#composer-form") && !!input && !input.disabled && !!send && !send.disabled,
              messageInputTag: input?.tagName || "",
              shellDisplay: shell ? getComputedStyle(shell).display : "",
              sidebarWidth: sidebarRect?.width || 0,
              sidebarHidden: !sidebar || sidebar.hidden || getComputedStyle(sidebar).display === "none",
              sidebarBeforeThread: !!sidebarRect && !!threadRect && sidebarRect.right <= threadRect.left + 1,
              selectorDirection: selector ? getComputedStyle(selector).flexDirection : "",
              compactRail: document.body.dataset.roomCompactRail || "",
              choices: [...document.querySelectorAll("[data-conversation-choice]")].map((node) => ({
                id: node.dataset.conversationChoice || "",
                active: node.classList.contains("active"),
                name: node.querySelector(".conversation-choice-name")?.textContent || "",
                detail: node.querySelector(".conversation-choice-detail")?.textContent || "",
              })),
              conversationTitle: document.querySelector("#conversation-title")?.textContent || "",
              conversationDetail: document.querySelector("#conversation-detail")?.textContent || "",
              emojiCount: document.querySelectorAll("#emoji-popover .emoji-chip").length,
              overflow: Math.max(document.documentElement.scrollWidth, document.body.scrollWidth) - document.documentElement.clientWidth,
            };
          }, width);
      state.topWidth = topWidth;
      assert(state.active === "true", "configured Chat session did not open", state);
      assert(state.attachHidden, "configured Chat exposed Attach", state);
      assert(state.browserStageHidden && state.browserRequestsHidden, "configured Chat exposed browser join controls", state);
      assert(state.roomSettingsHidden, "configured Chat exposed legacy room settings", state);
      assert(state.textVisible, "configured Chat text composer is unavailable", state);
      assert(state.messageInputTag === "TEXTAREA", "published Chat composer was not retained", state);
      assert(state.shellDisplay === "grid" && state.sidebarBeforeThread, "Chat is not a split conversation shell", state);
      assert(state.selectorDirection === "column", "conversation choices are not a vertical list", state);
      if (scenario === "single-conversation") {
        assert(
          state.choices.length === 1
            && state.choices[0]?.id === "shared"
            && state.choices[0]?.name === "Community"
            && state.choices[0]?.detail === "Shared room"
            && state.choices[0]?.active,
          "single-conversation Chat projected an unexpected conversation set",
          state,
        );
        assert(
          state.compactRail === "hidden" && state.sidebarHidden,
          "single-conversation compact Chat kept the switcher rail visible",
          state,
        );
      } else {
        assert(
          state.choices.length === 2
            && state.choices[0]?.id === "shared"
            && state.choices[0]?.name === "Community"
            && state.choices[0]?.detail === "Shared room"
            && state.choices[0]?.active
            && state.choices[1]?.id === "direct:sha256:fixture-conversation"
            && state.choices[1]?.name === "Fixture Friend"
            && state.choices[1]?.detail === "Direct message",
          "conversation list does not project the current Runtime conversations",
          state,
        );
        assert(
          Math.abs(state.sidebarWidth - (width <= 760 ? 72 : 220)) <= 1,
          "conversation sidebar width is not responsive",
          state,
        );
      }
      assert(
        state.conversationTitle === "Community" && state.conversationDetail === "Shared room",
        "active conversation header does not match the selected conversation",
        state,
      );
      assert(state.emojiCount === 12, "published emoji menu is incomplete", state);
      assert(state.overflow <= 1, "configured Chat has horizontal overflow", state);
      if (process.env.CHAT_LAYOUT_SCREENSHOT_DIR) {
        await mkdir(process.env.CHAT_LAYOUT_SCREENSHOT_DIR, { recursive: true });
        await page.screenshot({
          path: join(process.env.CHAT_LAYOUT_SCREENSHOT_DIR, `chat-${width}.png`),
        });
      }
    }

    const multilineState = await frame.evaluate(() => {
      const input = document.querySelector("#message-input");
      const field = document.querySelector(".composer-field");
      if (!(input instanceof HTMLTextAreaElement) || !(field instanceof HTMLElement)) {
        return null;
      }
      Object.defineProperty(input, "scrollHeight", {
        configurable: true,
        get() {
          return this.value.includes("\n") ? 88 : 34;
        },
      });
      input.value = "line one\nline two\nline three";
      input.dispatchEvent(new Event("input", { bubbles: true }));
      const tallHeight = input.style.height;
      const tallMultiline = field.dataset.multiline || "";
      input.value = "line one";
      input.dispatchEvent(new Event("input", { bubbles: true }));
      return {
        tallHeight,
        tallMultiline,
        shortHeight: input.style.height,
        shortMultiline: field.dataset.multiline || "",
      };
    });
    assert(multilineState, "configured Chat composer state is unavailable");
    assert(
      multilineState.tallHeight === "88px" && multilineState.tallMultiline === "true",
      "configured Chat did not present a multiline composer",
      multilineState,
    );
    assert(
      multilineState.shortHeight === "34px" && multilineState.shortMultiline === "false",
      "configured Chat did not reset multiline composer presentation",
      multilineState,
    );

    await frame.locator("#emoji-toggle").click();
    assert(
      await frame.locator("#emoji-popover").isVisible(),
      "emoji popover did not open from the compact composer",
    );
    await frame.locator("body").press("Escape");
    assert(
      !(await frame.locator("#emoji-popover").isVisible()),
      "emoji popover did not close on Escape",
    );

    await frame.locator("#participant-toggle").click();
    await frame.waitForFunction(() => document.querySelector("#chat-card")?.dataset.rosterOpen === "true");
    assert(await frame.locator("#presence-card").isVisible(), "conversation details drawer did not open");
    await frame.locator("#participant-close").click();
    await frame.waitForFunction(() => document.querySelector("#chat-card")?.dataset.rosterOpen === "false");

    await page.reload({ waitUntil: "domcontentloaded" });
    await waitFor(() => trace.cycles.length === 2);
    frame = await chatFrame(page);
    await waitForConfiguredChatWithoutLegacyFlash(frame, "opaque Chat refresh");
    await waitFor(() => trace.cycles[1]?.polls === 1);
    const refreshCycle = trace.cycles[1];
    assert(refreshCycle.starts === 1, "refresh did not bootstrap exactly one session", refreshCycle);
    assert(refreshCycle.polls === 1 && !refreshCycle.pollBeforeReady, "refresh polled before bootstrap", refreshCycle);

    await reopenOpaqueChat(page);
    await waitFor(() => trace.cycles.length === 3);
    frame = await chatFrame(page);
    await waitForConfiguredChatWithoutLegacyFlash(frame, "opaque Chat close/reopen");
    await waitFor(() => trace.cycles[2]?.polls === 1);
    const reopenCycle = trace.cycles[2];
    assert(reopenCycle.starts === 1, "reopen did not bootstrap exactly one session", reopenCycle);
    assert(reopenCycle.polls === 1 && !reopenCycle.pollBeforeReady, "reopen polled before bootstrap", reopenCycle);
  } finally {
    for (const hold of Object.values(holds)) hold?.release();
    await context.close();
    server.close();
    await rm(profile, { recursive: true, force: true });
  }
}

async function main() {
  const scenarios = process.env.CHAT_LAYOUT_SCENARIOS?.split(",").map((value) => value.trim()).filter(Boolean) || [
    "success",
    "single-conversation",
    "session-failure",
    "summary-failure",
    "direct-list-unavailable",
    "direct-switch",
    "direct-switch-hold-initial-conversations",
    "direct-switch-hold-bootstrap-messages",
    "direct-switch-hold-poll-conversations",
    "direct-switch-hold-poll-messages",
    "direct-initial-401",
    "direct-initial-401-user-selection",
    "shared-poll-401",
    "contact-acceptance",
    "contact-acceptance-held-request",
    "history-status-order-calendar",
    "direct-unread",
    "shell-layout-visibility",
    "visible-read",
    "direct-retry",
    "direct-retry-held",
    "direct-retry-terminal",
    "direct-retry-unavailable",
  ];
  for (const scenario of scenarios) {
    await runScenario(scenario);
  }
  console.log("PASS configured Chat bootstrap, refresh, failure, layout, and stale direct-switch boundaries");
}

main().catch((error) => {
  console.error(error.stack || error.message);
  process.exit(1);
});
