import { migrateLegacyWorkspaces, mergeWorkspaceDocuments } from "./workspace-transition.js";
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

const WORKSPACE_URL = "/api/apps/assistant/workspace-v2";
const WORKSPACE_SCHEMA = "elastos.assistant.workspace/v2";
const PERSIST_DEBOUNCE_MS = 400;

let snapshotFn = null;
let persistTimer = 0;
let persistInFlight = null;
let persistDirty = false;
let workspaceRevision = null;
let lastSnapshot = null;
let migrationRevision = null;
let applySnapshotFn = null;
let canApplySnapshot = () => true;
let mergeDeferred = false;
export function bindWorkspaceMergeGuard(fn) { canApplySnapshot = fn; }
let persistError = null;
let conflictNotice = null;
window.addEventListener("beforeunload", event => {
  if (persistDirty || persistInFlight || persistError) {
    event.preventDefault(); event.returnValue = "";
  }
});
export function bindWorkspaceApply(fn) { applySnapshotFn = fn; }
export function workspaceSaveError() { return persistError; }
function reportSave(error) {
  persistError = error;
  window.dispatchEvent(new CustomEvent("assistant:workspace-save", {detail: {error: error || conflictNotice}}));
}

export function bindAgentWorkspaceSnapshot(getSnapshot) {
  snapshotFn = typeof getSnapshot === "function" ? getSnapshot : null;
}

/** GET the saved workspace; null when the Runtime has none or is unreachable. */
export async function loadAgentWorkspace() {
  const saved = await fetchJson(WORKSPACE_URL, { method: "GET" });
  if (!saved || saved.schema !== WORKSPACE_SCHEMA || !Number.isInteger(saved.revision)) {
    throw new Error("Unsupported workspace response");
  }
  workspaceRevision = Number.isInteger(saved.revision) ? saved.revision : 0;
  migrationRevision = saved.migration_revision ?? null;
  const document = saved.legacy ? migrateLegacyWorkspaces(saved.legacy) : saved.document;
  lastSnapshot = document && Object.keys(document).length ? structuredClone(document) : null;
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

export async function persistAgentWorkspaceNow() {
  if (persistInFlight) {
    return persistInFlight;
  }
  if (workspaceRevision === null) {
    /* Never write before the saved workspace was read: a blind PUT would
       race the load and the Runtime would refuse it (revision) anyway. */
    return null;
  }
  if (mergeDeferred && !canApplySnapshot()) return null;
  let snap;
  try {
    snap = snapshotFn?.();
  } catch {
    return null; /* snapshot mid-mutation; the next change reschedules */
  }
  if (!snap || typeof snap !== "object") {
    return null;
  }
  snap = structuredClone(snap);
  persistDirty = false;
  persistInFlight = (async () => {
    try {
      const sentRevision = workspaceRevision;
      const saved = await fetchJson(WORKSPACE_URL, {
        method: "PUT",
        body: JSON.stringify({ schema: WORKSPACE_SCHEMA, if_revision: workspaceRevision, document: snap, ...(migrationRevision ? {migration_revision: migrationRevision} : {}) }),
      });
      if (!saved || saved.schema !== WORKSPACE_SCHEMA || saved.revision !== sentRevision + 1 || !saved.document || typeof saved.document !== "object") {
        throw new Error("Invalid workspace save acknowledgement");
      }
      if (saved) {
        workspaceRevision = saved.revision;
        lastSnapshot = snap;
        migrationRevision = null;
        reportSave(null);
        return true;
      }
    } catch (error) {
      reportSave("Your changes are still open here. Workspace save failed; retry before closing.");
      if (error?.status === 409) {
        if (!canApplySnapshot()) {
          mergeDeferred = true;
          reportSave("This window has unsaved changes. Finish the current run, then select this message to merge and save both windows.");
          return;
        }
        try {
          const current = await fetchJson(WORKSPACE_URL, { method: "GET" });
          if (!current || !Number.isInteger(current.revision)) return;
          const remote = current.legacy ? migrateLegacyWorkspaces(current.legacy) : current.document;
          // Include edits made while this request was in flight. Retain same-session
          // conflicts as visible copies instead of overwriting either writer.
          const local = snapshotFn?.() || snap;
          const merged = mergeWorkspaceDocuments(lastSnapshot || {}, local, remote || {});
          if (!applySnapshotFn) return;
          if (!canApplySnapshot()) {
            mergeDeferred = true;
            reportSave("Finish the current run, then select this message to merge and save both windows.");
            return;
          }
          if (applySnapshotFn(merged.document) === false) throw new Error("Workspace merge could not be applied");
          mergeDeferred = false;
          workspaceRevision = current.revision;
          migrationRevision = current.migration_revision ?? null;
          lastSnapshot = structuredClone(remote || {});
          persistDirty = true;
          if (merged.conflicts.length) conflictNotice = "Changes from both windows were kept. Review the conflict copies.";
          reportSave(null);
        } catch {
          // Keep local work and the original revision; an explicit retry can recover.
        }
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

/** Flush the latest state before dispatching a new externally owned run. */
export async function flushAgentWorkspace() {
  for (let attempt = 0; attempt < 3; attempt++) {
    if (workspaceRevision === null || (mergeDeferred && !canApplySnapshot())) return false;
    await persistAgentWorkspaceNow();
    if (persistError) return false;
    if (JSON.stringify(lastSnapshot) === JSON.stringify(snapshotFn?.())) return true;
  }
  return false;
}

/* ---- messaging ------------------------------------------------------------- */

export function openModelsFromAgent() {
  const value = new URL(window.location.href).searchParams.get("home_origin");
  let origin;
  try {
    const parsed = new URL(value);
    if (!["http:", "https:"].includes(parsed.protocol) || parsed.origin !== value) return false;
    origin = parsed.origin;
  } catch { return false; }
  if (!homeToken || window.top === window) return false;
  // The top Home registers this exact nested app source before accepting its intent.
  window.top.postMessage({ type: "home:app-ready", homeToken }, origin);
  window.top.postMessage({ type: "home:open-target", target: "system", query: { settings: "models" }, homeToken }, origin);
  return true;
}

/* The Home GUI frame is opaque-sandboxed, so the only honest target is "*";
   the parent reference pins the recipient and Home checks event.source. */
export function postToHome(message) {
  if (window.parent && window.parent !== window) {
    window.parent.postMessage(message, "*");
  }
}
