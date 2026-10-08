    import { createHomeNavigationClient } from "/apps/home/home-navigation-client.js";
    import {
      createHomeClipboardClient,
    } from "/apps/home/home-clipboard-client.js?v=home-20260726a";
    import init from "./chat_room_ui.js?v=chat-room-ui-20261006a";

    const chatLaunchParams = new URLSearchParams(window.location.search);
    let chatHomeToken = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";
    if (chatHomeToken) {
      const homeNavigation = createHomeNavigationClient({ homeToken: chatHomeToken,
        homeOrigin: chatLaunchParams.get("home_origin") || "" });
      let currentQuery = chatLaunchParams.has("conversation_id")
        ? { conversation_id: chatLaunchParams.get("conversation_id") } : {};
      globalThis.elastosChatNavigation = (query) => {
        currentQuery = { ...query };
        homeNavigation.setQuery(query);
      };
      const clipboardOptions = () => ({
        targetId: "chat-room",
        homeOrigin: chatLaunchParams.get("home_origin") || "",
        homeToken: chatHomeToken,
      });
      let homeClipboard = createHomeClipboardClient(clipboardOptions());
      homeClipboard.start();
      globalThis.elastosChatCopyInvite = (text) => homeClipboard.writeText(text, {
        purpose: "conversation.invite",
      });
      globalThis.elastosChatHomeToken = () => chatHomeToken;
      let pendingReconnect = null;
      globalThis.elastosChatReconnect = () => {
        if (pendingReconnect) return pendingReconnect;
        const homeOrigin = chatLaunchParams.get("home_origin") || "";
        if (!homeOrigin || homeOrigin === "null" || homeOrigin === "*" || window.top === window) {
          return Promise.reject(new Error("Open Chat from Home to reconnect."));
        }
        const requestId = window.crypto.randomUUID();
        pendingReconnect = new Promise((resolve, reject) => {
          const finish = (error, token) => {
            window.clearTimeout(timer);
            window.removeEventListener("message", receive);
            pendingReconnect = null;
            if (error) reject(error); else resolve(token);
          };
          const receive = (event) => {
            const data = event.data;
            if (event.source !== window.top || event.origin !== homeOrigin
              || data?.type !== "home:shell-response" || data.requestId !== requestId) return;
            if (data.error) {
              finish(new Error(data.status === 401 || data.status === 403
                ? "Sign in to Home, then reconnect Chat. Your drafts are kept."
                : "Home could not reopen Chat. Try again."));
              return;
            }
            const launched = data.result;
            let route;
            try { route = new URL(launched?.route, homeOrigin); } catch { finish(new Error("Home returned an invalid Chat launch.")); return; }
            const token = new URLSearchParams(route.hash.replace(/^#/, "")).get("home_token") || "";
            if (launched?.target !== "chat-room" || launched.attach_kind !== "iframe"
              || (launched.launch_status && launched.launch_status !== "launched")
              || route.origin !== homeOrigin || route.pathname !== "/apps/chat-room/"
              || route.username || route.password
              || !token || token === chatHomeToken || token !== token.trim()) {
              finish(new Error("Home returned an invalid Chat launch.")); return;
            }
            homeClipboard.teardown();
            chatHomeToken = token;
            homeNavigation.setHomeToken(token);
            homeClipboard = createHomeClipboardClient(clipboardOptions());
            homeClipboard.start();
            window.dispatchEvent(new Event("elastos-chat-authority-renewed"));
            finish(null, token);
          };
          const timer = window.setTimeout(() => finish(new Error("Home did not answer. Try reconnecting again.")), 30000);
          window.addEventListener("message", receive);
          try {
            window.top.postMessage({ type: "home:launch-target", requestId, target: "chat-room",
              query: currentQuery, homeToken: chatHomeToken }, homeOrigin);
          } catch (error) { finish(error); }
        });
        return pendingReconnect;
      };
      init({
        module_or_path: new URL("./chat_room_ui_bg.wasm?v=chat-room-ui-20261006a", import.meta.url),
      });
    }
