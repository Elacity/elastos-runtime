    import { createHomeNavigationClient } from "/apps/home/home-navigation-client.js";
    import {
      createHomeClipboardClient,
    } from "/apps/home/home-clipboard-client.js?v=home-20260726a";
    import init from "./chat_room_ui.js?v=chat-room-ui-20261006a";

    const chatLaunchParams = new URLSearchParams(window.location.search);
    const chatHomeToken = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";
    if (chatHomeToken) {
      const homeNavigation = createHomeNavigationClient({ homeToken: chatHomeToken,
        homeOrigin: chatLaunchParams.get("home_origin") || "" });
      globalThis.elastosChatNavigation = (query) => homeNavigation.setQuery(query);
      const homeClipboard = createHomeClipboardClient({
        targetId: "chat-room",
        homeOrigin: chatLaunchParams.get("home_origin") || "",
        homeToken: chatHomeToken,
      });
      homeClipboard.start();
      globalThis.elastosChatCopyInvite = (text) => homeClipboard.writeText(text, {
        purpose: "conversation.invite",
      });
      init({
        module_or_path: new URL("./chat_room_ui_bg.wasm?v=chat-room-ui-20261006a", import.meta.url),
      });
    }
