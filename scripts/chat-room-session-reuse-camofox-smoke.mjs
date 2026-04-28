#!/usr/bin/env node

const CAMOFOX_BASE = process.env.CAMOFOX_BASE || "http://127.0.0.1:9377";
const ELASTOS_BASE_URL = (process.env.ELASTOS_BASE_URL || "http://127.0.0.1:8090").replace(/\/+$/, "");
const HOME_URL = process.env.HOME_URL || `${ELASTOS_BASE_URL}/apps/home/`;
const CHAT_ROOM_URL = process.env.CHAT_ROOM_URL || `${ELASTOS_BASE_URL}/apps/chat-room/`;
const USER_ID = process.env.CAMOFOX_USER_ID || `chat-room-session-reuse-${Date.now()}`;
const SESSION_KEY = `chat-room-session-reuse-${Date.now()}`;

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
  const response = await fetch(`${CAMOFOX_BASE}${path}`, {
    ...options,
    headers: {
      "content-type": "application/json",
      ...(options.headers || {}),
    },
  });
  const text = await response.text();
  let data = {};
  try {
    data = text ? JSON.parse(text) : {};
  } catch {
    data = { raw: text };
  }
  if (!response.ok) {
    const error = new Error(`${options.method || "GET"} ${path} -> ${response.status}`);
    error.response = data;
    throw error;
  }
  return data;
}

async function cleanupSession() {
  await fetch(`${CAMOFOX_BASE}/sessions/${USER_ID}`, { method: "DELETE" }).catch(() => {});
}

async function createTab(url) {
  const created = await request("/tabs", {
    method: "POST",
    body: JSON.stringify({
      userId: USER_ID,
      sessionKey: SESSION_KEY,
      url,
    }),
  });
  return created.tabId;
}

async function closeTab(tabId) {
  await fetch(`${CAMOFOX_BASE}/tabs/${tabId}?userId=${encodeURIComponent(USER_ID)}`, {
    method: "DELETE",
  }).catch(() => {});
}

async function waitForSelector(tabId, selector, timeoutMs = 15_000) {
  return request(`/tabs/${tabId}/wait`, {
    method: "POST",
    body: JSON.stringify({ userId: USER_ID, selector, timeoutMs }),
  });
}

async function click(tabId, selector) {
  await waitForSelector(tabId, selector, 5_000).catch(() => {});
  try {
    return await request(`/tabs/${tabId}/click`, {
      method: "POST",
      body: JSON.stringify({ userId: USER_ID, selector }),
    });
  } catch (error) {
    const message = String(error.message || "");
    if (!message.includes("timeout") && !message.includes("-> 500")) {
      throw error;
    }
    await delay(500);
    await waitForSelector(tabId, selector, 5_000).catch(() => {});
    return request(`/tabs/${tabId}/click`, {
      method: "POST",
      body: JSON.stringify({ userId: USER_ID, selector }),
    });
  }
}

async function evaluate(tabId, expression) {
  const result = await request(`/tabs/${tabId}/evaluate`, {
    method: "POST",
    body: JSON.stringify({ userId: USER_ID, expression }),
  });
  return result.result;
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

async function main() {
  await cleanupSession();
  let shellTab = null;
  let directTab = null;
  try {
    shellTab = await createTab(HOME_URL);
    await waitForSelector(shellTab, 'body[data-home-status="ready"]', 30_000);
    await waitForSelector(shellTab, '#desktop-shortcuts .desktop-shortcut[data-target="chat-room"]', 30_000);

    await click(shellTab, "#launcher-toggle");
    await delay(700);
    await click(shellTab, '.launcher-card[data-target="chat-room"]');
    await waitFor(async () => {
      const state = await evaluate(shellTab, `(() => {
        const frame = document.querySelector('.window[data-target="chat-room"] .window-frame');
        const doc = frame?.contentWindow?.document;
        return !!doc && doc.querySelector('#chat-card') && !doc.querySelector('#chat-card').hidden && !doc.querySelector('#message-input')?.disabled;
      })()`);
      return Boolean(state);
    }, 20_000, 500);

    directTab = await createTab(CHAT_ROOM_URL);
    await waitForSelector(directTab, "#chat-card, #browser-access-stage", 30_000);
    await delay(1500);

    const state = await evaluate(directTab, `(() => ({
      browserAccessStagePresent: !!document.querySelector('#browser-access-stage'),
      browserAccessStageVisible: (() => {
        const node = document.querySelector('#browser-access-stage');
        if (!node || node.hidden) return false;
        return getComputedStyle(node).display !== 'none';
      })(),
      chatCardHidden: document.querySelector('#chat-card')?.hidden ?? null,
      inputDisabled: document.querySelector('#message-input')?.disabled ?? null,
      chatHeaderPresent: !!document.querySelector('.chat-header'),
      badge: document.querySelector('#status-badge')?.textContent?.trim() || "",
      detail: document.querySelector('#status-detail')?.textContent?.trim() || "",
      error: document.querySelector('#error-text')?.textContent?.trim() || "",
      localParticipant: (() => {
        const item = document.querySelector('#participant-list .participant-local');
        return item
          ? {
              name: item.querySelector('.participant-name')?.textContent?.trim() || "",
              detail: item.querySelector('.participant-detail')?.textContent?.trim() || "",
            }
          : null;
      })(),
    }))()`);

    const shellState = await evaluate(shellTab, `(() => {
      const frame = document.querySelector('.window[data-target="chat-room"] .window-frame');
      const doc = frame?.contentWindow?.document;
      return {
        taskbarCount: document.querySelector('#taskbar-targets .taskbar-item[data-target="chat-room"]')?.dataset?.openWindows || "",
        windowCount: document.querySelectorAll('.window[data-target="chat-room"]').length,
        roomDetail: doc?.querySelector('#status-detail')?.textContent?.trim() || "",
        roomBadge: doc?.querySelector('#status-badge')?.textContent?.trim() || "",
      };
    })()`);

    assert(state.browserAccessStagePresent, "direct chat-room route did not render the integrated browser-access stage", state);
    assert(state.browserAccessStageVisible === false, "same browser session should not require browser access again", state);
    assert(state.chatCardHidden === false, "same browser session did not reopen the room surface", state);
    assert(state.inputDisabled === false, "same browser session left the chat composer disabled", state);
    assert(state.chatHeaderPresent === false, "chat-room still rendered the old room header on direct browser reopen", state);
    assert(state.localParticipant, "same browser session did not mark the local runtime participant", state);
    assert(
      !state.localParticipant.detail.includes("guest browser"),
      "same browser session still described the local runtime participant as a guest browser",
      state,
    );
    assert(
      !state.localParticipant.detail.includes("ElastOS shell"),
      "same browser session still leaked shell transport/device chrome into the active room surface",
      state,
    );
    assert(state.detail === shellState.roomDetail, "same browser session still diverged from shell room status detail", { direct: state, shell: shellState });
    assert(state.badge === shellState.roomBadge, "same browser session still diverged from shell room status badge", { direct: state, shell: shellState });
    assert(shellState.taskbarCount === "1", "shell taskbar inflated the chat-room open count during same-browser reuse", shellState);
    assert(shellState.windowCount === 1, "shell opened more than one chat-room window during same-browser reuse", shellState);

    console.log(`PASS same-browser-session-reuses-chat-room-access url=${CHAT_ROOM_URL} user=${USER_ID}`);
  } finally {
    if (shellTab) {
      await closeTab(shellTab);
    }
    if (directTab) {
      await closeTab(directTab);
    }
    await cleanupSession();
  }
}

main().catch((error) => {
  console.error(error.stack || error.message || String(error));
  process.exitCode = 1;
});
