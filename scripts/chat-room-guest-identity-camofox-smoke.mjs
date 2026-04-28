#!/usr/bin/env node

const CAMOFOX_BASE = process.env.CAMOFOX_BASE || "http://127.0.0.1:9377";
const ELASTOS_BASE_URL = (process.env.ELASTOS_BASE_URL || "http://127.0.0.1:8090").replace(/\/+$/, "");
const HOME_URL = process.env.HOME_URL || `${ELASTOS_BASE_URL}/apps/home/`;
const CHAT_ROOM_URL = process.env.CHAT_ROOM_URL || `${ELASTOS_BASE_URL}/apps/chat-room/`;
const HOST_ORIGIN = new URL(HOME_URL).origin;
const HOME_USER_ID = process.env.CAMOFOX_HOME_USER_ID || `chat-room-home-${Date.now()}`;
const GUEST_USER_ID = process.env.CAMOFOX_GUEST_USER_ID || `chat-room-guest-${Date.now()}`;

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function assert(condition, message, details = null) {
  if (!condition) {
    const error = new Error(message);
    error.details = details;
    throw error;
  }
}

async function request(path, options = {}) {
  const { timeoutMs = 30_000, ...fetchOptions } = options;
  const controller = new AbortController();
  const timeoutId = setTimeout(() => controller.abort(), timeoutMs);
  let response;
  try {
    response = await fetch(`${CAMOFOX_BASE}${path}`, {
      ...fetchOptions,
      signal: controller.signal,
      headers: {
        "content-type": "application/json",
        ...(fetchOptions.headers || {}),
      },
    });
  } catch (error) {
    if (error.name === "AbortError") {
      throw new Error(`${fetchOptions.method || "GET"} ${path} -> timeout after ${timeoutMs}ms`);
    }
    throw error;
  } finally {
    clearTimeout(timeoutId);
  }
  const text = await response.text();
  let data = {};
  try {
    data = text ? JSON.parse(text) : {};
  } catch {
    data = { raw: text };
  }
  if (!response.ok) {
    const error = new Error(`${fetchOptions.method || "GET"} ${path} -> ${response.status}`);
    error.response = data;
    throw error;
  }
  return data;
}

async function waitFor(check, timeoutMs = 15_000, intervalMs = 250) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await check()) {
      return true;
    }
    await delay(intervalMs);
  }
  return false;
}

async function waitForValue(readValue, timeoutMs = 15_000, intervalMs = 250) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const value = await readValue();
    if (value) {
      return value;
    }
    await delay(intervalMs);
  }
  return null;
}

async function cleanupSession(userId) {
  await fetch(`${CAMOFOX_BASE}/sessions/${userId}`, { method: "DELETE" }).catch(() => {});
}

async function createTab(userId, url, sessionKey) {
  const created = await request("/tabs", {
    method: "POST",
    body: JSON.stringify({ userId, sessionKey, url }),
  });
  return created.tabId;
}

async function createStableShellTab() {
  for (let attempt = 0; attempt < 4; attempt += 1) {
    await cleanupSession(HOME_USER_ID);
    await delay(750);
    const tabId = await createTab(
      HOME_USER_ID,
      HOME_URL,
      `chat-room-guest-shell-${Date.now()}-${attempt}`,
    );
    try {
      await waitForSelector(HOME_USER_ID, tabId, 'body[data-home-status="ready"]', 30_000);
      await delay(800);
      return tabId;
    } catch (error) {
      await closeTab(HOME_USER_ID, tabId);
      if (attempt === 3) {
        throw error;
      }
    }
  }
  throw new Error("guest identity smoke could not create a stable shell tab");
}

async function closeTab(userId, tabId) {
  await fetch(`${CAMOFOX_BASE}/tabs/${tabId}?userId=${encodeURIComponent(userId)}`, {
    method: "DELETE",
  }).catch(() => {});
}

async function waitForSelector(userId, tabId, selector, timeoutMs = 15_000) {
  return request(`/tabs/${tabId}/wait`, {
    method: "POST",
    body: JSON.stringify({ userId, selector, timeoutMs }),
  });
}

async function click(userId, tabId, selector) {
  const clicked = await evaluate(
    userId,
    tabId,
    `(() => {
      const node = document.querySelector(${JSON.stringify(selector)});
      if (!node) return false;
      node.click();
      return true;
    })()`,
  );
  assert(clicked, `selector was not available: ${selector}`);
}

async function evaluate(userId, tabId, expression) {
  const result = await request(`/tabs/${tabId}/evaluate`, {
    method: "POST",
    body: JSON.stringify({ userId, expression }),
  });
  return result.result;
}

async function setInputValue(userId, tabId, selector, value) {
  return evaluate(
    userId,
    tabId,
    `(() => {
      const input = document.querySelector(${JSON.stringify(selector)});
      if (!input) return false;
      input.value = ${JSON.stringify(value)};
      input.dispatchEvent(new Event('input', { bubbles: true }));
      input.dispatchEvent(new Event('change', { bubbles: true }));
      return true;
    })()`,
  );
}

async function browserJson(userId, tabId, path, options = {}) {
  const method = options.method || "GET";
  const body = typeof options.body === "string" ? options.body : "";
  const script = `fetch(${JSON.stringify(path)}, {
    method: ${JSON.stringify(method)},
    headers: { "content-type": "application/json" },
    ${body ? `body: ${JSON.stringify(body)},` : ""}
  }).then(async (response) => {
    const text = await response.text();
    let payload = {};
    try { payload = text ? JSON.parse(text) : {}; } catch (_) { payload = { raw: text }; }
    if (!response.ok) {
      throw new Error(${JSON.stringify(method)} + " " + ${JSON.stringify(path)} + " -> " + response.status + " " + text);
    }
    return payload;
  })`;
  return evaluate(userId, tabId, script);
}

async function launchShellTarget(tabId, target) {
  return browserJson(HOME_USER_ID, tabId, "/api/apps/home/launch", {
    method: "POST",
    body: JSON.stringify({ target }),
  });
}

async function inboxAction(tabId, actionId) {
  const launched = await launchShellTarget(tabId, "inbox");
  const token = new URL(launched.route, HOST_ORIGIN).searchParams.get("home_token");
  assert(token, "Inbox launch did not return a home token", launched);
  return evaluate(
    HOME_USER_ID,
    tabId,
    `fetch("/api/apps/inbox/actions", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-elastos-home-token": ${JSON.stringify(token)},
      },
      body: ${JSON.stringify(JSON.stringify({ action_id: actionId }))},
    }).then(async (response) => {
      const text = await response.text();
      if (!response.ok) throw new Error("Inbox action failed: " + response.status + " " + text);
      return text ? JSON.parse(text) : {};
    })`,
  );
}

async function cleanupRoomState(tabId) {
  const summary = await browserJson(HOME_USER_ID, tabId, "/api/apps/chat-room/summary");
  if (Array.isArray(summary.pending_requests)) {
    for (const request of summary.pending_requests) {
      await inboxAction(tabId, `room-deny-request:${request.request_id}`);
    }
  }
}

async function shellFrameState(tabId) {
  return evaluate(
    HOME_USER_ID,
    tabId,
    `(() => {
      const frame = document.querySelector('.window[data-target="chat-room"] .window-frame');
      const doc = frame?.contentWindow?.document;
      if (!doc) {
        return { ok: false };
      }
      return {
        ok: true,
        taskbarCount: document.querySelector('#taskbar-targets .taskbar-item[data-target="chat-room"]')?.dataset?.openWindows || "",
        windowCount: document.querySelectorAll('.window[data-target="chat-room"]').length,
        inputDisabled: doc.querySelector('#message-input')?.disabled ?? true,
        messages: [...doc.querySelectorAll('#message-list .message')].map((node) => ({
          sender: node.querySelector('.message-meta span')?.textContent?.trim() || "",
          body: node.querySelector('.message-body')?.textContent?.trim() || "",
        })),
      };
    })()`,
  );
}

async function sendShellMessage(tabId, body) {
  await evaluate(
    HOME_USER_ID,
    tabId,
    `(() => {
      const frame = document.querySelector('.window[data-target="chat-room"] .window-frame');
      const doc = frame?.contentWindow?.document;
      const input = doc?.querySelector('#message-input');
      const form = doc?.querySelector('#composer-form');
      if (!input || !form) return false;
      input.value = ${JSON.stringify(body)};
      input.dispatchEvent(new Event('input', { bubbles: true }));
      form.requestSubmit();
      return true;
    })()`,
  );
}

async function guestState(tabId) {
  return evaluate(
    GUEST_USER_ID,
    tabId,
    `(() => ({
      badge: document.querySelector('#status-badge')?.textContent?.trim() || "",
      detail: document.querySelector('#status-detail')?.textContent?.trim() || "",
      browserAccessVisible: (() => {
        const node = document.querySelector('#browser-access-stage');
        if (!node || node.hidden) return false;
        return getComputedStyle(node).display !== 'none';
      })(),
      inputDisabled: document.querySelector('#message-input')?.disabled ?? true,
      localParticipant: (() => {
        const item = document.querySelector('#participant-list .participant-local');
        return item
          ? {
              name: item.querySelector('.participant-name')?.textContent?.trim() || "",
              detail: item.querySelector('.participant-detail')?.textContent?.trim() || "",
            }
          : null;
      })(),
      messages: [...document.querySelectorAll('#message-list .message')].map((node) => ({
        sender: node.querySelector('.message-meta span')?.textContent?.trim() || "",
        body: node.querySelector('.message-body')?.textContent?.trim() || "",
      })),
    }))()`,
  );
}

async function main() {
  let shellTab = null;
  let guestTab = null;
  try {
    await cleanupSession(HOME_USER_ID);
    await cleanupSession(GUEST_USER_ID);

    shellTab = await createStableShellTab();
    await cleanupRoomState(shellTab);

    await click(HOME_USER_ID, shellTab, "#launcher-toggle");
    await delay(700);
    await click(HOME_USER_ID, shellTab, '.launcher-card[data-target="chat-room"]');
    await waitFor(async () => {
      const state = await shellFrameState(shellTab);
      return state.ok && state.inputDisabled === false;
    }, 20_000, 500);

    const shellMessage = `shell-${Date.now()}`;
    await sendShellMessage(shellTab, shellMessage);
    await waitFor(async () => {
      const state = await shellFrameState(shellTab);
      return state.messages.some((entry) => entry.body === shellMessage && entry.sender === "You");
    }, 12_000, 500);

    guestTab = await createTab(
      GUEST_USER_ID,
      CHAT_ROOM_URL,
      `chat-room-guest-browser-${Date.now()}`,
    );
    await waitForSelector(GUEST_USER_ID, guestTab, "#browser-access-form", 30_000);
    await setInputValue(GUEST_USER_ID, guestTab, "#display-name", "Guest QA");
    await click(GUEST_USER_ID, guestTab, "#browser-access-submit");

    await waitFor(async () => {
      const state = await guestState(guestTab);
      return state.badge === "Waiting";
    }, 12_000, 500);

    const requestId = await waitForValue(async () => {
      const summary = await browserJson(HOME_USER_ID, shellTab, "/api/apps/chat-room/summary");
      const request = summary.pending_requests?.find((entry) => entry.display_name === "Guest QA");
      return request?.request_id || null;
    }, 12_000, 500);
    assert(requestId, "chat-room summary never surfaced the pending guest browser request");
    await inboxAction(shellTab, `room-approve-request:${requestId}`);

    await waitFor(async () => {
      const state = await guestState(guestTab);
      return state.browserAccessVisible === false && state.inputDisabled === false;
    }, 20_000, 500);

    await waitFor(async () => {
      const state = await guestState(guestTab);
      return state.messages.some((entry) => entry.body === shellMessage);
    }, 12_000, 500);

    const approvedGuest = await guestState(guestTab);
    const shellMessageEntry = approvedGuest.messages.find((entry) => entry.body === shellMessage);
    assert(shellMessageEntry, "guest browser never received the shell message", approvedGuest);
    assert(shellMessageEntry.sender !== "You", "guest browser misclassified the shell message as its own", approvedGuest);
    assert(
      approvedGuest.localParticipant?.name === "Guest QA",
      "guest browser did not keep the approved guest identity as the local participant",
      approvedGuest,
    );
    assert(
      !approvedGuest.localParticipant?.detail?.includes("guest browser"),
      "guest browser still renders noisy guest-browser self chrome",
      approvedGuest,
    );

    const guestMessage = `guest-${Date.now()}`;
    await setInputValue(GUEST_USER_ID, guestTab, "#message-input", guestMessage);
    await click(GUEST_USER_ID, guestTab, "#send-button");
    await waitFor(async () => {
      const state = await guestState(guestTab);
      return state.messages.some((entry) => entry.body === guestMessage && entry.sender === "You");
    }, 12_000, 500);
    await waitFor(async () => {
      const state = await shellFrameState(shellTab);
      return state.messages.some((entry) => entry.body === guestMessage && entry.sender !== "You");
    }, 12_000, 500);

    const shellAfterGuest = await shellFrameState(shellTab);
    const guestMessageEntry = shellAfterGuest.messages.find((entry) => entry.body === guestMessage);
    assert(guestMessageEntry, "shell route never received the guest message", shellAfterGuest);
    assert(guestMessageEntry.sender === "Guest QA", "shell route mislabeled the guest sender", shellAfterGuest);
    assert(shellAfterGuest.taskbarCount === "1", "shell taskbar inflated the chat-room window count", shellAfterGuest);
    assert(shellAfterGuest.windowCount === 1, "shell opened more than one chat-room window during guest approval", shellAfterGuest);

    console.log(`PASS guest-browser-keeps-separate-chat-identity home=${HOME_USER_ID} guest=${GUEST_USER_ID}`);
  } finally {
    if (shellTab) {
      await closeTab(HOME_USER_ID, shellTab);
    }
    if (guestTab) {
      await closeTab(GUEST_USER_ID, guestTab);
    }
    await cleanupSession(HOME_USER_ID);
    await cleanupSession(GUEST_USER_ID);
  }
}

main().catch((error) => {
  console.error(error.stack || error.message || String(error));
  if (error.details) {
    console.error(JSON.stringify(error.details, null, 2));
  }
  process.exitCode = 1;
});
