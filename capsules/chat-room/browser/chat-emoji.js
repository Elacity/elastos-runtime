    (function () {
      const toggle = document.getElementById("emoji-toggle");
      const popover = document.getElementById("emoji-popover");
      if (!toggle || !popover) return;

      function setOpen(open) {
        popover.hidden = !open;
        toggle.setAttribute("aria-expanded", open ? "true" : "false");
      }

      toggle.addEventListener("click", (event) => {
        event.preventDefault();
        setOpen(popover.hidden);
      });
      popover.addEventListener("click", (event) => {
        if (event.target.closest(".emoji-chip")) setOpen(false);
      });
      document.addEventListener("keydown", (event) => {
        if (event.key === "Escape" && !popover.hidden) {
          setOpen(false);
          toggle.focus();
        }
      });
      document.addEventListener("pointerdown", (event) => {
        if (popover.hidden || toggle.contains(event.target) || popover.contains(event.target)) return;
        setOpen(false);
      });
    })();
