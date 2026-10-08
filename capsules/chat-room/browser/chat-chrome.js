    (function () {
      const launchParams = new URLSearchParams(window.location.search);
      let homeToken = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";
      const homeParentOrigin = launchParams.get("home_origin") || "";
      const participantToggle = document.getElementById("participant-toggle");
      const roomAccessToggle = document.getElementById("room-access-toggle");
      const conversationSelector = document.getElementById("conversation-selector");
      const conversationJoinSection = document.getElementById("conversation-join-section");
      const composerForm = document.getElementById("composer-form");
      const composerField = composerForm?.querySelector(".composer-field") || null;
      const messageInput = document.getElementById("message-input");
      let homeReadySent = false;
      let lastMenuManifest = "";

      function canUseAccessSettings() {
        return !!roomAccessToggle && !roomAccessToggle.hidden && !roomAccessToggle.disabled;
      }

      function currentMenuManifest() {
        const viewItems = [
          { label: "People", cmd: "view-people" },
        ];
        if (canUseAccessSettings()) {
          viewItems.push({ label: "Access Settings", cmd: "view-access-settings" });
        }
        return {
          type: "home:menu-manifest",
          homeToken,
          menus: [
            {
              title: "File",
              items: [
                { label: "Close Window", cmd: "__close-window" },
              ],
            },
            {
              title: "View",
              items: viewItems,
            },
          ],
        };
      }

      function announceHomeChrome() {
        if (!homeToken || !homeParentOrigin || window.top === window) {
          return;
        }
        if (!homeReadySent) {
          window.top.postMessage({ type: "home:app-ready", homeToken }, homeParentOrigin);
          homeReadySent = true;
        }
        const manifest = currentMenuManifest();
        const nextMenuManifest = JSON.stringify(manifest);
        if (nextMenuManifest === lastMenuManifest) {
          return;
        }
        lastMenuManifest = nextMenuManifest;
        window.top.postMessage(manifest, homeParentOrigin);
      }

      function syncCompactConversationRail() {
        if (!conversationSelector || !conversationJoinSection) {
          return;
        }
        const choiceCount = conversationSelector.querySelectorAll("[data-conversation-choice]").length;
        const canHideCompactRail = choiceCount < 2 && conversationJoinSection.hidden;
        document.body.setAttribute("data-room-compact-rail", canHideCompactRail ? "hidden" : "visible");
      }

      function syncComposerPresentation() {
        if (!messageInput || !composerField) {
          return;
        }
        const lineHeight = Number.parseFloat(globalThis.getComputedStyle(messageInput).lineHeight) || 0;
        messageInput.style.height = "auto";
        const nextHeight = Math.min(messageInput.scrollHeight || 0, 132);
        if (nextHeight > 0) {
          messageInput.style.height = `${nextHeight}px`;
        }
        composerField.dataset.multiline = (
          messageInput.value.includes("\n") || nextHeight > lineHeight * 2
        ) ? "true" : "false";
      }

      function isTrustedHomeMenuEvent(event) {
        return event.origin === "null" && event.source === window.parent;
      }

      function clickExistingControl(id) {
        const node = document.getElementById(id);
        if (node instanceof HTMLElement) {
          node.click();
        }
      }

      announceHomeChrome();
      syncCompactConversationRail();
      syncComposerPresentation();

      window.addEventListener("elastos-chat-authority-renewed", () => {
        homeToken = globalThis.elastosChatHomeToken();
        homeReadySent = false;
        lastMenuManifest = "";
        announceHomeChrome();
      });

      if (messageInput) {
        messageInput.addEventListener("input", syncComposerPresentation);
      }

      // Until the shell names the form factor, the device screen stands in for
      // it: this window's own width is the app window, not the device. Same
      // 640 px phone class as the Home shell.
      function isPhone() {
        const named = document.documentElement.dataset.elFormFactor;
        if (named) {
          return named === "phone";
        }
        const screen = window.screen || {};
        const coarse = Boolean(window.matchMedia?.("(pointer: coarse)")?.matches
          || window.matchMedia?.("(hover: none)")?.matches);
        return screen.width <= 640 || (coarse && screen.height <= 640);
      }

      if (messageInput && composerForm) {
        messageInput.addEventListener("keydown", (event) => {
          // 229: Safari reports an IME's confirming Enter without isComposing.
          if (event.key !== "Enter" || event.shiftKey || event.altKey || event.ctrlKey || event.metaKey
            || event.isComposing || event.keyCode === 229) {
            return;
          }
          // A phone keyboard's Return adds a line; Send sends.
          if (isPhone()) {
            return;
          }
          event.preventDefault();
          const sendButton = document.getElementById("send-button");
          if (sendButton instanceof HTMLButtonElement && !sendButton.disabled) {
            composerForm.requestSubmit(sendButton);
          }
        });
      }

      if (roomAccessToggle) {
        new MutationObserver(() => {
          announceHomeChrome();
        }).observe(roomAccessToggle, {
          attributes: true,
          attributeFilter: ["hidden", "disabled"],
        });
      }

      if (conversationSelector) {
        new MutationObserver(syncCompactConversationRail).observe(conversationSelector, {
          childList: true,
          subtree: true,
        });
      }

      if (conversationJoinSection) {
        new MutationObserver(syncCompactConversationRail).observe(conversationJoinSection, {
          attributes: true,
          attributeFilter: ["hidden"],
        });
      }

      window.addEventListener("message", (event) => {
        if (!isTrustedHomeMenuEvent(event)) {
          return;
        }
        const message = event.data;
        if (message?.type !== "elastos:menu-command" || typeof message.cmd !== "string") {
          return;
        }
        if (message.cmd === "view-people") {
          clickExistingControl("participant-toggle");
          return;
        }
        if (message.cmd === "view-access-settings" && canUseAccessSettings()) {
          clickExistingControl("room-access-toggle");
        }
      });
    })();
