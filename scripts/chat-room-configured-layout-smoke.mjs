#!/usr/bin/env node

import { createServer } from "node:http";
import { createRequire } from "node:module";
import { mkdir, mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../", import.meta.url)));
const browserRoot = join(repoRoot, "capsules/chat-room/browser");
const homeClipboardClient = join(repoRoot, "capsules/home/browser/home-clipboard-client.js");
const homeClipboardProtocol = join(repoRoot, "capsules/home/browser/home-clipboard-protocol.js");
const brave = process.env.BRAVE_BIN || "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";
const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");

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
  };
  const trace = {
    cycles: [],
    current: null,
    directConversations: 0,
    directMessages: 0,
    heldBoundary: holdBoundary,
    heldReleases: [],
    leaves: 0,
    pollErrors: 0,
    requests: [],
  };
  let directConversationResponses = 0;
  let directMessageResponses = 0;
  const server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url, "http://localhost");
      if (url.pathname.startsWith("/api/apps/chat-room")) {
        trace.requests.push(`${request.method} ${url.pathname}`);
      }
      if (request.method === "OPTIONS") {
        response.writeHead(204, {
          "access-control-allow-headers": "content-type,x-elastos-home-token",
          "access-control-allow-methods": "GET,POST,OPTIONS",
          "access-control-allow-origin": "null",
        }).end();
        return;
      }
      if (url.pathname === "/fixture") {
        const chatSrc = isDirectSwitchScenario(scenario)
          ? "/apps/chat-room/?conversation_id=direct%3Asha256%3Afixture-conversation#home_token=test-token"
          : "/apps/chat-room/#home_token=test-token";
        const body = Buffer.from(`<!doctype html><style>html,body{height:100%;margin:0}iframe{border:0;height:100%;width:100%}</style><iframe title="Chat" sandbox="allow-forms allow-modals allow-pointer-lock allow-scripts" src="${chatSrc}"></iframe>`);
        response.writeHead(200, {
          "content-length": body.length,
          "content-type": "text/html; charset=utf-8",
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
            poll: configuredPoll(),
          },
          200,
          { "set-cookie": "room-session=fixture-session; Max-Age=300; Path=/; HttpOnly; SameSite=Lax" },
        );
      }
      if (url.pathname === "/api/apps/chat-room/poll") {
        trace.current.polls += 1;
        trace.current.pollBeforeReady ||= !trace.current.ready;
        trace.current.pollHeaders.push({
          authorization: request.headers.authorization || null,
          cookie: request.headers.cookie || null,
          homeToken: request.headers["x-elastos-home-token"] || null,
          origin: request.headers.origin || null,
        });
        return json(response, configuredPoll());
      }
      if (url.pathname === "/api/apps/chat-room/send") {
        trace.current.sends += 1;
        return json(response, { error: "unexpected send" }, 500);
      }
      if (url.pathname === "/api/apps/chat-room/direct/conversations") {
        trace.directConversations += 1;
        if (scenario === "single-conversation") {
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
        return json(response, { conversations: [directConversation()] });
      }
      if (url.pathname === "/api/apps/chat-room/direct/conversations/direct%3Asha256%3Afixture-conversation/messages") {
        trace.directMessages += 1;
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
        "#conversation-join-section",
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
    frame.src = "/apps/chat-room/#home_token=test-token";
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
  });
  const page = context.pages()[0] || await context.newPage();

  try {
    await page.goto(url, { waitUntil: "domcontentloaded" });
    let frame = await chatFrame(page);

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
      assert(
        await frame.evaluate(() => document.querySelector("#conversation-join-section")?.hidden),
        "bootstrap failure exposed the legacy Join surface",
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
              joinHidden: hidden("#conversation-join-section"),
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
      assert(state.joinHidden, "configured Chat exposed invite/join controls", state);
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
    "direct-switch",
    "direct-switch-hold-initial-conversations",
    "direct-switch-hold-bootstrap-messages",
    "direct-switch-hold-poll-conversations",
    "direct-switch-hold-poll-messages",
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
