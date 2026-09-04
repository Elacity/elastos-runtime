/* Home Agent — capsule entry.

   Boots the harness exactly as Home GUI did (bind Shelf, bind harness) and
   speaks the small contract with the Home GUI frame that hosts it:

     Home → capsule   home-agent:open              the Shelf morph began; raise the room
                      home-agent:shelf-handover    Home's pill reached composer geometry (on)
                                                   or the leave began: the composer fades on
                                                   this pill before Home takes it back (off)
                      home-agent:close             Home is closing the Space: lower the room.
                                                   Sent on every leave, also one this capsule
                                                   asked for — the room never lowers on its
                                                   own request, so both paths are one morph
     capsule → Home   home-agent:ready             module booted, frame can be shown
                      home-agent:shelf-metrics     composer pill size, so Home's pill matches
                      home-agent:close             user asks to leave from inside (Home, Esc)
                      home-agent:open-viewer       artifact to show in Documents
                      home-agent:open-browser      web link to open in Browser */

import { bindAgentShelf, raiseComposerFace, agentShelfFaceActive, composerInput } from "./agent-shelf.js";
import {
  bindAgentHarness,
  showAgentHarness,
  hideAgentHarness,
  agentHarnessActive,
  applyAgentWorkspaceSnapshot,
} from "./agent-harness.js";
import { handleShellEscape } from "./shell-popovers.js";
import { postToHome, loadAgentWorkspace } from "./harness-host.js";

const HOME_MESSAGE_TYPES = new Set(["home-agent:open", "home-agent:shelf-handover", "home-agent:close"]);

function fromHome(event) {
  return event.source === window.parent && event.source !== window;
}

function taskbarEl() {
  return document.querySelector(".taskbar");
}

function raiseRoom() {
  raiseComposerFace();
  if (!agentHarnessActive()) {
    showAgentHarness({ fromShelf: true, syncStage: false });
  } else {
    showAgentHarness({ syncStage: false });
  }
}

let faceReleaseListener = null;
function lowerRoom() {
  hideAgentHarness({ syncStage: false });
  const taskbar = taskbarEl();
  if (!taskbar) {
    return;
  }
  /* The pill keeps its composer geometry while it fades under Home's
     returning glass; the face class goes once that fade has ended, so the
     shape never changes on a pill the user can still see. A reopen during
     the fade brings the handover back and the class stays. */
  if (faceReleaseListener) {
    taskbar.removeEventListener("transitionend", faceReleaseListener);
  }
  faceReleaseListener = (event) => {
    if (event.target !== taskbar || event.propertyName !== "opacity") {
      return;
    }
    taskbar.removeEventListener("transitionend", faceReleaseListener);
    faceReleaseListener = null;
    if (!document.documentElement.dataset.shelfHandover) {
      taskbar.classList.remove("is-agent-face");
    }
  };
  taskbar.addEventListener("transitionend", faceReleaseListener);
}

function setHandover(on) {
  if (on) {
    document.documentElement.dataset.shelfHandover = "1";
    const input = composerInput();
    if (input && !input.disabled) {
      input.focus({ preventScroll: true });
    }
  } else {
    delete document.documentElement.dataset.shelfHandover;
  }
}

function reportShelfMetrics() {
  const taskbar = taskbarEl();
  if (!taskbar || !taskbar.classList.contains("is-agent-face")) {
    return;
  }
  const rect = taskbar.getBoundingClientRect();
  if (rect.width < 1 || rect.height < 1) {
    return;
  }
  postToHome({
    type: "home-agent:shelf-metrics",
    width: Math.round(rect.width),
    height: Math.round(rect.height),
    radius: getComputedStyle(taskbar).borderTopLeftRadius,
  });
}

window.addEventListener("message", (event) => {
  if (!fromHome(event)) {
    return;
  }
  const message = event.data;
  if (!message || typeof message !== "object" || !HOME_MESSAGE_TYPES.has(message.type)) {
    return;
  }
  if (message.type === "home-agent:open") {
    if (!agentShelfFaceActive() || !agentHarnessActive()) {
      raiseRoom();
    }
    reportShelfMetrics();
    return;
  }
  if (message.type === "home-agent:shelf-handover") {
    setHandover(message.on === true);
    return;
  }
  if (message.type === "home-agent:close") {
    setHandover(false);
    lowerRoom();
  }
});

/* Escape rides the same popover stack Home used; when nothing inside claims
   it the harness's own handler leaves the room, which reaches Home through
   the stage seam in harness-host. */
document.addEventListener(
  "keydown",
  (event) => {
    if (event.key !== "Escape") {
      return;
    }
    if (handleShellEscape()) {
      event.preventDefault();
      event.stopPropagation();
    }
  },
  true,
);

bindAgentShelf();
bindAgentHarness();

const taskbar = taskbarEl();
if (taskbar && typeof ResizeObserver === "function") {
  new ResizeObserver(() => reportShelfMetrics()).observe(taskbar);
}

/* The saved workspace is read before Home is told the frame is ready, so the
   room opens on the user's sessions, never on an empty one that would then be
   replaced. An unreachable Runtime still gets a ready room; it just starts empty
   and does not write until the workspace has been read. */
loadAgentWorkspace()
  .then((snapshot) => {
    applyAgentWorkspaceSnapshot(snapshot);
  })
  .catch(() => {})
  .finally(() => {
    postToHome({ type: "home-agent:ready" });
  });
