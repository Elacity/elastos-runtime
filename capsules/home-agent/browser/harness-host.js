/* Host seam for the Home Agent capsule.

   The harness was written inside Home GUI and imported a handful of shell
   symbols. It now runs in its own capsule frame, so this module provides those
   symbols at the capsule boundary — nothing in the harness itself changes.

   Ownership: Home GUI keeps the Shelf morph, the room's place and the breathe.
   This capsule owns the composer, the conversation and the workspace. Anything
   that needs Home is asked for by message; anything that needs Runtime goes
   through the launch token like every other capsule. */

const homeToken = (() => {
  const params = new URLSearchParams(window.location.hash.replace(/^#/, ""));
  return params.get("home_token") || "";
})();

/* ---- shell-core ------------------------------------------------------------ */

export function getHomeGuiLaunchToken() {
  return homeToken;
}

export async function fetchJson(url, init) {
  const response = await fetch(url, {
    ...init,
    headers: {
      "content-type": "application/json",
      ...(homeToken ? { "x-elastos-home-token": homeToken } : {}),
      ...(init && init.headers ? init.headers : {}),
    },
  });
  if (!response.ok) {
    const detail = await response.text().catch(() => "");
    const error = new Error(
      `request failed: ${response.status} ${response.statusText}${detail ? ` ${detail}` : ""}`,
    );
    error.status = response.status;
    throw error;
  }
  if (response.status === 204) {
    return null;
  }
  return response.json();
}

/* The harness reads Home's summary only to list Desktop objects for the
   attach menu. A capsule does not see Home's desktop; until a typed Library
   operation exists the list is empty and the menu's own empty state shows. */
export const shellState = { currentSummary: null, restoringSession: false };

export function desktopObjects() {
  return [];
}

/* ---- shell-stages ----------------------------------------------------------
   Home owns Spaces. Inside this frame the Agent Space is always the active
   stage; leaving it means asking Home to close the Shelf. */

const AGENT_STAGE = "agent";
const DESKTOP_STAGE = "desktop";

export function agentStageId() {
  return AGENT_STAGE;
}

export function desktopStageId() {
  return DESKTOP_STAGE;
}

export function getActiveStageId() {
  return AGENT_STAGE;
}

export function isAgentSpace(stageId) {
  return stageId === AGENT_STAGE;
}

export function setActiveStage(stageId) {
  if (stageId === DESKTOP_STAGE) {
    postToHome({ type: "home-agent:close" });
  }
}

export function syncSpacePager() {}

/* ---- shell-windows: workspace persistence seam ----------------------------
   The workspace is a Runtime object: /api/apps/home-agent/workspace, bound to
   this capsule's launch token, principal-root protected, revisioned. The
   capsule owns the document's shape; the Runtime owns where it lives, who may
   read it and how large it may be. */

const WORKSPACE_URL = "/api/apps/home-agent/workspace";
const WORKSPACE_SCHEMA = "elastos.home-agent.workspace/v1";
const PERSIST_DEBOUNCE_MS = 400;

let snapshotFn = null;
let persistTimer = 0;
let persistInFlight = null;
let persistDirty = false;
let workspaceRevision = null;
let lastSnapshot = null;

export function bindAgentWorkspaceSnapshot(getSnapshot) {
  snapshotFn = typeof getSnapshot === "function" ? getSnapshot : null;
}

/** GET the saved workspace; null when the Runtime has none or is unreachable. */
export async function loadAgentWorkspace() {
  const saved = await fetchJson(WORKSPACE_URL, { method: "GET" });
  if (!saved || saved.schema !== WORKSPACE_SCHEMA) {
    return null;
  }
  workspaceRevision = Number.isInteger(saved.revision) ? saved.revision : 0;
  const document = saved.document && typeof saved.document === "object" ? saved.document : null;
  lastSnapshot = document && Object.keys(document).length ? document : null;
  return lastSnapshot;
}

export function scheduleAgentWorkspacePersist() {
  persistDirty = true;
  window.clearTimeout(persistTimer);
  persistTimer = window.setTimeout(() => {
    persistTimer = 0;
    void persistAgentWorkspaceNow();
  }, PERSIST_DEBOUNCE_MS);
}

async function persistAgentWorkspaceNow() {
  if (persistInFlight) {
    return persistInFlight;
  }
  if (workspaceRevision === null) {
    /* Never write before the saved workspace was read: a blind PUT would
       race the load and the Runtime would refuse it (revision) anyway. */
    return null;
  }
  let snap;
  try {
    snap = snapshotFn?.();
  } catch {
    return null; /* snapshot mid-mutation; the next change reschedules */
  }
  if (!snap || typeof snap !== "object") {
    return null;
  }
  persistDirty = false;
  persistInFlight = (async () => {
    try {
      const saved = await fetchJson(WORKSPACE_URL, {
        method: "PUT",
        body: JSON.stringify({ schema: WORKSPACE_SCHEMA, if_revision: workspaceRevision, document: snap }),
      });
      if (saved && Number.isInteger(saved.revision)) {
        workspaceRevision = saved.revision;
        lastSnapshot = snap;
      }
    } catch (error) {
      if (error?.status === 409) {
        /* Another frame of this capsule wrote first: take its revision, then
           write ours on top on the next change. */
        try {
          const current = await fetchJson(WORKSPACE_URL, { method: "GET" });
          if (current && Number.isInteger(current.revision)) {
            workspaceRevision = current.revision;
          }
        } catch {
          /* stays at the old revision; the next PUT reports again */
        }
        persistDirty = true;
      }
    } finally {
      persistInFlight = null;
      if (persistDirty) {
        scheduleAgentWorkspacePersist();
      }
    }
  })();
  return persistInFlight;
}

/* ---- messaging ------------------------------------------------------------- */

/* The Home GUI frame is opaque-sandboxed, so the only honest target is "*";
   the parent reference pins the recipient and Home checks event.source. */
export function postToHome(message) {
  if (window.parent && window.parent !== window) {
    window.parent.postMessage(message, "*");
  }
}
