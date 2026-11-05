    import { createHomeNavigationClient } from "/apps/home/home-navigation-client.js";
    import {
      createHomeClipboardClient,
    } from "/apps/home/home-clipboard-client.js?v=home-20260726a";
    import init from "./chat_room_ui.js?v=chat-room-ui-20261008a";

    const chatLaunchParams = new URLSearchParams(window.location.search);
    let chatHomeToken = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";
    if (chatHomeToken) {
      const homeNavigation = createHomeNavigationClient({ homeToken: chatHomeToken,
        homeOrigin: chatLaunchParams.get("home_origin") || "", target: "chat-room",
        onHomeToken(token) {
          homeClipboard.setHomeToken(token);
          chatHomeToken = token;
          window.dispatchEvent(new Event("elastos-chat-authority-renewed"));
        } });
      globalThis.elastosChatNavigation = (query) => {
        homeNavigation.setQuery(query);
      };
      const clipboardOptions = () => ({
        targetId: "chat-room",
        homeOrigin: chatLaunchParams.get("home_origin") || "",
        homeToken: chatHomeToken,
      });
      const homeClipboard = createHomeClipboardClient(clipboardOptions());
      homeClipboard.start();
      globalThis.elastosChatCopyInvite = (text) => homeClipboard.writeText(text, {
        purpose: "conversation.invite",
      });
      globalThis.elastosChatCopyProfileId = (text) => homeClipboard.writeText(text, {
        purpose: "profile.id",
      });
      globalThis.elastosChatHomeToken = () => chatHomeToken;
      globalThis.elastosChatReconnect = () => homeNavigation.renewAuthority();
      init({
        module_or_path: new URL("./chat_room_ui_bg.wasm?v=chat-room-ui-20261008a", import.meta.url),
      });
    }
