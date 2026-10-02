    (function () {
      const shell = document.querySelector(".settings");
      if (!shell) {
        return;
      }
      shell.addEventListener("click", (event) => {
        const item = event.target.closest(".settings-sidebar-item");
        if (!item || !shell.contains(item)) {
          return;
        }
        const tab = (item.getAttribute("data-settings") || "").trim();
        if (!tab) {
          return;
        }
        for (const node of document.querySelectorAll(".settings-sidebar-item")) {
          node.classList.toggle("active", node.getAttribute("data-settings") === tab);
        }
        for (const node of document.querySelectorAll(".settings-content")) {
          node.classList.toggle("active", node.getAttribute("data-settings") === tab);
        }
        const container = document.querySelector(".settings-content-container");
        if (container) {
          container.scrollTop = 0;
        }
      });
    })();
