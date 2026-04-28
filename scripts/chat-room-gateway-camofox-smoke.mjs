#!/usr/bin/env node

const CAMOFOX_BASE = process.env.CAMOFOX_BASE || "http://127.0.0.1:9377";
const ELASTOS_BASE_URL = (process.env.ELASTOS_BASE_URL || "http://127.0.0.1:8090").replace(/\/+$/, "");
const CHAT_ROOM_URL = process.env.CHAT_ROOM_URL || `${ELASTOS_BASE_URL}/apps/chat-room/`;
const USER_ID = process.env.CAMOFOX_USER_ID || `chat-room-smoke-${Date.now()}`;

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
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

async function cleanupSession() {
  await fetch(`${CAMOFOX_BASE}/sessions/${USER_ID}`, { method: "DELETE" }).catch(() => {});
}

async function waitForSelector(tabId, selector, timeoutMs = 15_000) {
  return request(`/tabs/${tabId}/wait`, {
    method: "POST",
    body: JSON.stringify({ userId: USER_ID, selector, timeoutMs }),
  });
}

async function createTab() {
  for (let attempt = 0; attempt < 4; attempt += 1) {
    await cleanupSession();
    await delay(500);
    let tabId = null;
    try {
      const created = await request("/tabs", {
        method: "POST",
        body: JSON.stringify({
          userId: USER_ID,
          sessionKey: `chat-room-smoke-${Date.now()}-${attempt}`,
          url: CHAT_ROOM_URL,
        }),
      });
      tabId = created.tabId;
      await waitForSelector(tabId, "#chat-card", 30_000);
      const ready = await waitFor(async () => {
        return evaluate(tabId, `(() => ({
          hasChatCard: !!document.querySelector("#chat-card"),
          hasAccessForm: !!document.querySelector("#browser-access-form"),
          readyState: document.readyState,
        }))()`).then((state) => state.hasChatCard && state.hasAccessForm);
      }, 30_000, 300);
      assert(ready, "chat-room gateway route did not render the access form");
      await delay(700);
      return tabId;
    } catch (error) {
      if (tabId) {
        await closeTab(tabId);
      }
      if (attempt === 3) {
        throw error;
      }
      await delay(1000);
    }
  }
  throw new Error("chat-room gateway smoke could not create a stable tab");
}

async function closeTab(tabId) {
  await fetch(`${CAMOFOX_BASE}/tabs/${tabId}?userId=${encodeURIComponent(USER_ID)}`, {
    method: "DELETE",
  }).catch(() => {});
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

async function refresh(tabId) {
  return request(`/tabs/${tabId}/refresh`, {
    method: "POST",
    body: JSON.stringify({ userId: USER_ID }),
  });
}

async function evaluate(tabId, expression) {
  const result = await request(`/tabs/${tabId}/evaluate`, {
    method: "POST",
    body: JSON.stringify({ userId: USER_ID, expression }),
  });
  return result.result;
}

async function setInputValue(tabId, selector, value) {
  return evaluate(
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

async function roomState(tabId) {
  return evaluate(tabId, `(() => ({
    badge: document.querySelector("#status-badge")?.textContent?.trim() || "",
    detail: document.querySelector("#status-detail")?.textContent?.trim() || "",
    error: document.querySelector("#error-text")?.textContent?.trim() || "",
    displayNameInvalid: document.querySelector("#display-name")?.matches(":invalid") || false,
    statusRowVisible: (() => {
      const node = document.querySelector("#browser-access-status-row");
      if (!node || node.hidden) return false;
      return getComputedStyle(node).display !== "none";
    })(),
    presenceVisible: (() => {
      const node = document.querySelector(".presence-card");
      if (!node || node.hidden) return false;
      return getComputedStyle(node).display !== "none";
    })(),
    browserAccessVisible: (() => {
      const node = document.querySelector("#browser-access-stage");
      if (!node || node.hidden) return false;
      return getComputedStyle(node).display !== "none";
    })(),
    chatVisible: !document.querySelector("#chat-card")?.hidden,
    resetVisible: !document.querySelector("#reset-button")?.hidden,
    outerCardBackground: getComputedStyle(document.querySelector("#chat-card")).backgroundColor,
    outerCardShadow: getComputedStyle(document.querySelector("#chat-card")).boxShadow,
  }))()`);
}

async function runCase(name, fn) {
  let tabId = null;
  try {
    tabId = await createTab();
    await fn(tabId);
    console.log(`PASS ${name}`);
  } catch (error) {
    const state = tabId ? await roomState(tabId).catch(() => null) : null;
    console.error(`FAIL ${name}`);
    console.error(error.message);
    if (error.details) {
      console.error(JSON.stringify(error.details, null, 2));
    }
    if (state) {
      console.error(JSON.stringify(state, null, 2));
    }
    throw error;
  } finally {
    if (tabId) {
      await closeTab(tabId);
    }
    await cleanupSession();
  }
}

async function main() {
  await cleanupSession();
  try {
    await runCase("starts-unpaired-in-room-layout", async (tabId) => {
      const state = await roomState(tabId);
      assert(state.badge === "Join", "chat-room did not start in Join state", state);
      assert(state.browserAccessVisible, "browser-access request flow should be visible on fresh start", state);
      assert(!state.statusRowVisible, "idle gateway join state should not show a status strip", state);
      assert(!state.presenceVisible, "idle gateway join state should not show the room sidebar", state);
      assert(state.chatVisible, "chat-room should stay in the room layout before browser access is approved", state);
      assert(!state.resetVisible, "reset should be hidden on a fresh chat-room start", state);
      assert(
        state.outerCardBackground === "rgba(0, 0, 0, 0)" || state.outerCardBackground === "transparent",
        "gateway join state still rendered an outer shell card behind the room card",
        state,
      );
      assert(
        state.outerCardShadow === "none",
        "gateway join state still rendered an outer shell card shadow",
        state,
      );
    });

    await runCase("blank-submit-shows-visible-error", async (tabId) => {
      await click(tabId, "#browser-access-submit").catch(() => {});
      await delay(700);
      const state = await roomState(tabId);
      assert(state.displayNameInvalid, "display name should be invalid on blank submit", state);
      assert(
        state.error === "Enter your name.",
        "blank submit did not show a visible browser-access error",
        state,
      );
    });

    await runCase("filled-submit-shows-approval-requested", async (tabId) => {
      await setInputValue(tabId, "#display-name", "QA Browser");
      await click(tabId, "#browser-access-submit");
      await delay(1200);
      const state = await roomState(tabId);
      assert(state.badge === "Waiting", "filled submit did not show waiting state", state);
      assert(
        state.detail === "Waiting for approval.",
        "approval detail did not match the reduced waiting copy",
        state,
      );
      assert(state.resetVisible, "reset should stay available after requesting browser access", state);
    });

    console.log(`PASS full-smoke url=${CHAT_ROOM_URL} user=${USER_ID}`);
  } finally {
    await cleanupSession();
  }
}

main().catch((error) => {
  console.error(error.stack || error.message || String(error));
  process.exitCode = 1;
});
