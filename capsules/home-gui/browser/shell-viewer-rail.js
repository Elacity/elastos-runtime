/* Viewer rail: a right-hand slide-over that hosts a runtime capsule for an
   artifact the agent produced or referenced (document, video, web page, table).

   This is the wallet rail generalized to a dynamic target. Same discipline:
   the capsule launches through the authority-carrying launchHomeTarget path
   (host allowlist decides what may open — UI != authority), then mounts in a
   sandboxed iframe (no allow-same-origin → opaque origin, structurally
   isolated). The shell holds no document bytes and no grant state; it only
   opens the door. Grants stay in the runtime / Inbox.

   Single slot: opening a second artifact retires the first frame and remounts.
   The frame is kept warm across open/close of the SAME target (hide, not
   unload) so scroll position survives; switching targets tears down.

   Authority boundary: we never write into the capsule — we pass the object to
   view via the launch query (e.g. library's ?uri= / ?objectUri= deep link),
   which the host already routes for desktop cross-app opens. */

import { escapeHtml, shellState, targetById, getHomeGuiLaunchToken } from "./shell-core.js?v=home-20260814a";
import {
  registerShellPopover,
  closeOtherShellPopovers,
} from "./shell-popovers.js?v=home-20260814a";
import {
  launchHomeTarget,
  iframeSandboxForLaunch,
  iframeAllowForLaunch,
  openTarget,
} from "./shell-windows.js?v=home-20260814a";
import { playUiSound } from "./shell-sounds.js?v=home-20260814a";

let rail = null;
let frame = null;
let mediaHost = null;
let mediaUrl = null;
let titleNode = null;
let iconNode = null;
let closeButton = null;
let windowButton = null;
let errorBlock = null;
let errorDetail = null;
let retryButton = null;
let invoker = null;
let launching = false;
let frameReady = false;
let outsideDismissBound = false;
let registered = false;
/* The artifact currently on view: { target, title, query }. Null when empty. */
let current = null;

const TYPE_GLYPH = {
  document:
    '<svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M3.5 1.75h6l3 3v9.5h-9z"/><path d="M9.5 1.75v3h3"/></svg>',
  video:
    '<svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true" fill="currentColor"><path d="M5 3.5v9l7-4.5z"/></svg>',
  web: '<svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.4"><circle cx="8" cy="8" r="6.25"/><path d="M1.75 8h12.5M8 1.75c-3.5 3.5-3.5 9 0 12.5M8 1.75c3.5 3.5 3.5 9 0 12.5"/></svg>',
  data: '<svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.4"><rect x="2" y="2.5" width="12" height="11" rx="1.5"/><path d="M2 6h12M6.5 6v7.5"/></svg>',
  code: '<svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M5.75 4.5 2.25 8l3.5 3.5M10.25 4.5 13.75 8l-3.5 3.5"/></svg>',
  game: '<svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="1.75" y="4.5" width="12.5" height="8" rx="2.5"/><path d="M5 7.25v2.5M3.75 8.5h2.5M10.5 7.5h.01M12 9h.01"/></svg>',
};

export function bindViewerRail() {
  if (rail) {
    return;
  }
  rail = document.querySelector("#viewer-rail");
  frame = document.querySelector("#viewer-rail-frame");
  mediaHost = document.querySelector("#viewer-rail-media");
  titleNode = document.querySelector("#viewer-rail-title");
  iconNode = document.querySelector("#viewer-rail-icon");
  closeButton = document.querySelector("#viewer-rail-close");
  windowButton = document.querySelector("#viewer-rail-open-window");
  errorBlock = document.querySelector("#viewer-rail-error");
  errorDetail = document.querySelector("#viewer-rail-error-detail");
  retryButton = document.querySelector("#viewer-rail-retry");
  if (!rail || !frame) {
    return;
  }
  if (!registered) {
    registerShellPopover("viewer-rail", () => hideViewerRail({ restoreFocus: false }));
    registered = true;
  }
  closeButton?.addEventListener("click", () => hideViewerRail());
  windowButton?.addEventListener("click", () => {
    /* Promote to a full Desktop window: tear down the rail frame first so two
       home_tokens never race, then open the same target as a window.
       Only meaningful for capsule-backed artifacts (media view has no window). */
    if (!current || !current.target) {
      return;
    }
    const target = current.target;
    retireViewerFrame();
    hideViewerRail({ animate: false });
    openTarget(target);
  });
  retryButton?.addEventListener("click", () => {
    if (current) {
      void mountViewer(current);
    }
  });
  rail.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      hideViewerRail();
    }
  });
}

export function viewerRailOpen() {
  return Boolean(rail) && !rail.hidden;
}

/* The capsule frame currently mounted in the rail (null for direct media
   views, which have no capsule). Lets home-gui deliver a payload (e.g. a
   document/code attachment) into the rail-hosted capsule, mirroring
   walletRailFrame()/inboxRailFrame(). */
export function viewerRailFrame() {
  return frame;
}

/* The capsule target currently on view in the rail ("" for media views). */
export function viewerRailTarget() {
  return current && current.target ? current.target : "";
}

function targetAvailable(target) {
  return Boolean(targetById(shellState.currentSummary, target));
}

/** Open the rail on an artifact.
   spec = { target, title, kind, query }                → capsule view (iframe)
         or { mediaUrl, mediaType, title, kind }         → direct media view
   (video / image). Media specs carry no capsule target. */
export function showViewerRail(spec) {
  if (!rail || !spec || typeof spec !== "object") {
    return;
  }
  const isMedia = typeof spec.mediaUrl === "string" && !!spec.mediaUrl;
  if (!isMedia && (typeof spec.target !== "string" || !spec.target)) {
    return;
  }
  if (!isMedia && !targetAvailable(spec.target)) {
    playUiSound("error");
    return;
  }
  closeOtherShellPopovers("viewer-rail");
  invoker =
    document.activeElement && document.activeElement !== document.body
      ? document.activeElement
      : null;

  const next = isMedia
    ? {
        target: "",
        title: String(spec.title || "Clip"),
        kind: String(spec.kind || "media"),
        query: {},
        mediaUrl: spec.mediaUrl,
        mediaType: String(spec.mediaType || "video"),
      }
    : {
        target: spec.target,
        title: String(spec.title || spec.target),
        kind: String(spec.kind || "document"),
        query: spec.query && typeof spec.query === "object" ? spec.query : {},
        /* Optional payload to post into the capsule once its frame is ready
           (e.g. a document/code attachment's bytes). Kept on the artifact so a
           warm remount can re-deliver. */
        deliver:
          spec.deliver && typeof spec.deliver === "object" ? spec.deliver : null,
      };
  const sameTarget = current && !isMedia && current.target && current.target === next.target;

  /* Reveal the rail. Only re-run the enter animation when it was actually
     closed — if it's already open (artifact→artifact switch), just swap the
     content with no slide, so it doesn't glitch. */
  const wasHidden = rail.hidden;
  rail.hidden = false;
  rail.inert = false;
  rail.setAttribute("aria-hidden", "false");
  rail.classList.remove("viewer-rail-leaving");
  if (wasHidden) {
    rail.style.animation = "none";
    void rail.offsetWidth;
    rail.style.animation = "";
  }
  syncHead(next);
  bindOutsideDismiss();
  rail.focus({ preventScroll: true });

  current = next;
  /* Same capsule target + same object → keep the warm frame (scroll survives). */
  if (sameTarget && frame.dataset.route && frame.dataset.objectKey === objectKey(next)) {
    return;
  }
  retireViewerFrame();
  void mountViewer(next);
}

export function hideViewerRail({ restoreFocus = true, animate = true } = {}) {
  if (!rail || rail.hidden) {
    return;
  }
  const finish = () => {
    rail.hidden = true;
    rail.inert = true;
    rail.setAttribute("aria-hidden", "true");
    rail.classList.remove("viewer-rail-leaving");
    if (restoreFocus && invoker && typeof invoker.focus === "function") {
      try {
        invoker.focus({ preventScroll: true });
      } catch (_error) {
        /* invoker may be gone */
      }
    }
    invoker = null;
  };
  if (!animate) {
    finish();
    return;
  }
  rail.classList.add("viewer-rail-leaving");
  const onEnd = (event) => {
    if (event.target === rail && event.animationName === "viewer-rail-leave") {
      rail.removeEventListener("animationend", onEnd);
      finish();
    }
  };
  rail.addEventListener("animationend", onEnd);
  /* Safety: if reduced-motion kills the animation, finish anyway. */
  window.setTimeout(() => {
    rail.removeEventListener("animationend", onEnd);
    if (!rail.hidden) {
      finish();
    }
  }, 320);
}

function syncHead(spec) {
  if (titleNode) {
    titleNode.textContent = spec.title;
  }
  if (iconNode) {
    iconNode.innerHTML = TYPE_GLYPH[spec.kind] || TYPE_GLYPH.document;
  }
  rail.setAttribute("aria-label", spec.title);
}

function objectKey(spec) {
  return `${spec.target}:${JSON.stringify(spec.query)}`;
}

function retireViewerFrame() {
  if (frame) {
    frameReady = false;
    frame.removeAttribute("src");
    delete frame.dataset.route;
    delete frame.dataset.objectKey;
    frame.classList.remove("is-ready");
    frame.hidden = true;
  }
  if (mediaHost) {
    mediaHost.replaceChildren();
    mediaHost.hidden = true;
  }
  if (mediaUrl) {
    try {
      URL.revokeObjectURL(mediaUrl);
    } catch {
      /* best effort */
    }
    mediaUrl = null;
  }
}

/* Dispatch to the right mount: direct media view vs sandboxed capsule frame. */
async function mountViewer(spec) {
  if (spec.mediaUrl) {
    await mountViewerMedia(spec);
    return;
  }
  await mountViewerFrame(spec);
}

/* Show / hide the rail's error block. */
function showViewerError(show, detail = "") {
  if (!errorBlock) {
    return;
  }
  errorBlock.hidden = !show;
  if (show && errorDetail) {
    errorDetail.textContent = String(detail || "");
  }
}

/* Direct media view (video clip / image). Fetches the bytes and renders them in
   a native element inside the rail — no capsule iframe. */
async function mountViewerMedia(spec) {
  if (!mediaHost) {
    return;
  }
  showViewerError(false);
  try {
    const token = getHomeGuiLaunchToken();
    const response = await fetch(spec.mediaUrl, {
      headers: token ? { "x-elastos-home-token" : token } : {},
    });
    if (!response.ok) {
      throw new Error(`fetch failed (${response.status})`);
    }
    const buf = await response.arrayBuffer();
    const mime = spec.mediaType === "image" ? "image/png" : "video/mp4";
    mediaUrl = URL.createObjectURL(new Blob([buf], { type: mime }));
    const el =
      spec.mediaType === "image"
        ? Object.assign(document.createElement("img"), { src: mediaUrl, alt: spec.title || "" })
        : Object.assign(document.createElement("video"), {
            src: mediaUrl,
            controls: true,
            autoplay: true,
            playsInline: true,
            loop: true,
          });
    /* Lock the element's aspect-ratio to the clip's real dimensions once known,
       so the rectangle is full-width × natural-height (never a thin collapsed
       strip, never stretched to the full panel). CSS defaults to 16/9 until then. */
    const applyAspect = () => {
      const w = el.videoWidth || el.naturalWidth;
      const h = el.videoHeight || el.naturalHeight;
      if (w > 0 && h > 0) {
        el.style.aspectRatio = `${w} / ${h}`;
      }
    };
    el.addEventListener(spec.mediaType === "image" ? "load" : "loadedmetadata", applyAspect, {
      once: true,
    });
    mediaHost.replaceChildren(el);
    mediaHost.hidden = false;
  } catch {
    showViewerError(true, "This clip could not be loaded.");
  }
}

async function mountViewerFrame(spec) {
  if (launching) {
    return;
  }
  launching = true;
  frameReady = false;
  if (errorBlock) {
    errorBlock.hidden = true;
  }
  frame.hidden = false;
  frame.classList.remove("is-ready");
  try {
    const launched = await launchHomeTarget(spec.target, spec.query);
    if (launched.attach_kind !== "iframe") {
      throw new Error(`unsupported attach kind: ${launched.attach_kind || "unknown"}`);
    }
    if (
      typeof launched.launch_status === "string" &&
      launched.launch_status.trim() !== "" &&
      launched.launch_status !== "launched"
    ) {
      throw new Error(
        typeof launched.launch_detail === "string" && launched.launch_detail.trim() !== ""
          ? launched.launch_detail.trim()
          : `launch status: ${launched.launch_status}`,
      );
    }
    frame.setAttribute("sandbox", iframeSandboxForLaunch(launched));
    frame.setAttribute("allow", iframeAllowForLaunch(launched));
    frame.title = escapeHtml(spec.title);
    frame.addEventListener("load", () => markFrameReady(), { once: true });
    const route = new URL(String(launched.route || ""), window.location.origin);
    /* Mark rail presentation so the capsule hides its own duplicate chrome. */
    route.searchParams.set("presentation", "rail");
    frame.src = route.href;
    frame.dataset.route = route.href;
    frame.dataset.objectKey = objectKey(spec);
  } catch (error) {
    frame.hidden = true;
    frameReady = false;
    playUiSound("error");
    if (errorBlock) {
      errorBlock.hidden = false;
      if (errorDetail) {
        errorDetail.textContent = String(error?.message || error);
      }
    }
  } finally {
    launching = false;
  }
}

function markFrameReady() {
  if (frameReady) {
    return;
  }
  frameReady = true;
  frame?.classList.add("is-ready");
  deliverPendingPayload();
}

/* Post the artifact's `deliver` payload into the freshly-mounted capsule. The
   capsule's message listener may attach a beat after iframe `load`, so retry a
   few times (mirrors openHomeGuiTargetWithPayload). Bytes travel in the
   message — no Library object, no host round-trip. */
function deliverPendingPayload() {
  const payload = current && current.deliver;
  if (!payload || !frame?.contentWindow) {
    return;
  }
  let attempts = 0;
  const send = () => {
    attempts += 1;
    try {
      /* The capsule frame is an opaque origin (sandboxed, no allow-same-origin),
         so we cannot know its origin — post with "*" exactly as home-gui's
         OPAQUE_FRAME_TARGET does for wallet/inbox rails. */
      frame.contentWindow.postMessage(payload, "*");
    } catch {
      /* frame mid-teardown */
    }
    if (attempts < 8 && current && current.deliver === payload) {
      window.setTimeout(send, 150);
    }
  };
  send();
}

/* Selectors for surfaces that OPEN the rail (Studio clip rows, chat artifact
   cards). A pointerdown on one of these is an artifact→artifact switch, not an
   "outside" dismiss — ignoring it here lets showViewerRail swap content smoothly
   instead of racing a close+reopen (the glitch). */
const RAIL_OPENER_SELECTOR =
  "[data-studio-library-item], .agent-artifact-main, .agent-artifact-card, [data-open-artifact]";

function bindOutsideDismiss() {
  if (outsideDismissBound) {
    return;
  }
  outsideDismissBound = true;
  document.addEventListener("pointerdown", (event) => {
    if (!viewerRailOpen() || rail.contains(event.target)) {
      return;
    }
    /* Clicking another artifact opener → let its click handler swap the rail's
       content; don't dismiss. */
    if (event.target.closest?.(RAIL_OPENER_SELECTOR)) {
      return;
    }
    hideViewerRail({ restoreFocus: false });
  });
}
