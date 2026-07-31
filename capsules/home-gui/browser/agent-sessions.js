/* Agent session nav, search, CRUD, project create.
   Bound from agent-harness.js. Tip: home-20260728ag
   Host session.agent persist only — UI ≠ authority (Principle 16). */

import {
  listProjects,
  createProject,
} from "./mock-agent-provider.js?v=home-20260728ag";
import { persistAgentWorkspaceSoon } from "./agent-workspace.js?v=home-20260728ag";
import { closeHarnessPage } from "./agent-configure.js?v=home-20260728ag";
import {
  renderActiveSession,
  renderFollowUpQueue,
  stopMockStream,
  setTitle,
  titleFromPrompt,
} from "./agent-stream.js?v=home-20260728ag";
import {
  syncAgentSendButton,
  composerInput as shelfComposerInput,
} from "./agent-shelf.js?v=home-20260728ag";

/** @type {null | object} */
let ctx = null;
/** @type {null | Record<string, Function>} */
let host = null;

export function bindAgentSessions(nextCtx, nextHost = {}) {
  ctx = nextCtx;
  host = nextHost;
}

export function relativeTime(ts) {
  if (!ts) {
    return "";
  }
  const delta = Date.now() - ts;
  if (delta < 60_000) {
    return "now";
  }
  if (delta < 3_600_000) {
    return `${Math.max(1, Math.round(delta / 60_000))}m`;
  }
  if (delta < 86_400_000) {
    return `${Math.max(1, Math.round(delta / 3_600_000))}h`;
  }
  return `${Math.max(1, Math.round(delta / 86_400_000))}d`;
}

export function touchSession(session) {
  if (session) {
    session.updatedAt = Date.now();
    session.mode = ctx.sessionMode;
  }
  persistAgentWorkspaceSoon();
}

export function exportActiveSessionMarkdown() {
  const session = ctx.sessions.find((s) => s.id === ctx.activeSessionId);
  if (!session) {
    return;
  }
  const lines = [`# ${session.title}`, "", `Mode: ${session.mode || ctx.sessionMode}`, ""];
  for (const msg of session.messages) {
    if (msg.role === "user") {
      lines.push(`## You`, "", msg.text, "");
    } else if (msg.role === "agent") {
      if (msg.thinking) {
        lines.push(`## Thinking`, "", msg.thinking, "");
      }
      lines.push(`## Agent`, "", msg.text, "");
    } else if (msg.role === "grant") {
      lines.push(`## Grant · ${msg.label || msg.toolId}`, "", `${msg.state}: ${msg.summary || ""}`, "");
    }
  }
  const blob = new Blob([lines.join("\n")], { type: "text/markdown;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = `${(session.title || "chat").replace(/[^\w\-]+/g, "_").slice(0, 48)}.md`;
  a.click();
  URL.revokeObjectURL(url);
}

export function sessionSearchOpen() {
  const root = document.querySelector("#agent-session-search");
  return Boolean(root) && !root.hidden;
}

export function renderSessionSearchResults(query = "") {
  const host = document.querySelector("#agent-session-search-results");
  if (!host) {
    return;
  }
  host.replaceChildren();
  const q = String(query || "").trim().toLowerCase();
  const matches = ctx.sessions.filter((session) => {
    if (!q) {
      return true;
    }
    if (session.title.toLowerCase().includes(q)) {
      return true;
    }
    return (session.messages || []).some((m) =>
      String(m.text || "").toLowerCase().includes(q),
    );
  });
  if (!matches.length) {
    const empty = document.createElement("p");
    empty.className = "agent-session-search-empty";
    empty.textContent = q ? "No chats match" : "No chats yet";
    host.append(empty);
    return;
  }
  for (const session of matches) {
    const row = document.createElement("button");
    row.type = "button";
    row.className = `agent-session-search-row${
      session.id === ctx.activeSessionId ? " is-ctx.active" : ""
    }`;
    row.dataset.sessionId = session.id;
    row.setAttribute("role", "option");
    row.innerHTML =
      `<span class="agent-session-search-row-mark" aria-hidden="true"></span>` +
      `<span class="agent-session-search-row-title"></span>` +
      `<span class="agent-session-search-row-when"></span>`;
    row.querySelector(".agent-session-search-row-title").textContent = session.title;
    row.querySelector(".agent-session-search-row-when").textContent =
      session.group || "";
    host.append(row);
  }
}

export function openSessionSearch() {
  if (!ctx.active) {
    return;
  }
  const root = document.querySelector("#agent-session-search");
  const input = document.querySelector("#agent-session-search-input");
  if (!root) {
    return;
  }
  root.hidden = false;
  root.inert = false;
  root.setAttribute("aria-hidden", "false");
  renderSessionSearchResults(input?.value || "");
  window.requestAnimationFrame(() => {
    input?.focus({ preventScroll: true });
    input?.select?.();
  });
}

export function closeSessionSearch() {
  const root = document.querySelector("#agent-session-search");
  const input = document.querySelector("#agent-session-search-input");
  if (!root || root.hidden) {
    return;
  }
  root.hidden = true;
  root.inert = true;
  root.setAttribute("aria-hidden", "true");
  if (input) {
    input.value = "";
  }
}

export function appendSessionRow(host, session, { nested = false } = {}) {
  const row = document.createElement("div");
  row.className = `agent-harness-session${session.id === ctx.activeSessionId ? " is-ctx.active" : ""}${
    nested ? " agent-projects-session" : ""
  }${session.pinned ? " is-pinned" : ""}`;
  row.dataset.sessionId = session.id;

  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "agent-harness-session-btn";
  if (session.pinned) {
    const pinMark = document.createElement("span");
    pinMark.className = "agent-harness-session-pin";
    pinMark.setAttribute("aria-hidden", "true");
    pinMark.title = "Pinned";
    btn.append(pinMark);
  }
  const title = document.createElement("span");
  title.className = "agent-harness-session-title";
  title.textContent = session.title;
  btn.append(title);
  if ((session.mode || "chat") === "build") {
    const meta = document.createElement("span");
    meta.className = "agent-harness-session-meta";
    meta.textContent = "Build";
    btn.append(meta);
  }
  btn.title = [session.title, session.pinned ? "Pinned" : "", relativeTime(session.updatedAt)]
    .filter(Boolean)
    .join(" · ");

  const kebab = document.createElement("button");
  kebab.type = "button";
  kebab.className = "agent-harness-session-menu";
  kebab.dataset.sessionMenu = session.id;
  kebab.setAttribute("aria-label", `Chat actions for ${session.title}`);
  kebab.setAttribute("aria-haspopup", "menu");
  kebab.setAttribute("aria-expanded", "false");
  kebab.title = "Pin, add to project, rename…";
  kebab.textContent = "···";

  row.append(btn, kebab);
  host.append(row);
}

export function renderProjectsNav() {
  const host = document.querySelector("[data-projects-list]");
  if (!host) {
    return;
  }
  host.replaceChildren();
  const projects = listProjects();
  for (const project of projects) {
    const wrap = document.createElement("div");
    wrap.className = "agent-projects-item";
    wrap.dataset.projectId = project.id;
    const head = document.createElement("div");
    head.className = "agent-projects-item-head";
    const toggle = document.createElement("button");
    toggle.type = "button";
    toggle.className = "agent-projects-toggle";
    toggle.dataset.projectToggle = project.id;
    toggle.setAttribute("aria-expanded", "true");
    toggle.textContent = project.title;
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "agent-projects-remove";
    remove.dataset.projectRemove = project.id;
    remove.setAttribute("aria-label", `Remove project ${project.title}`);
    remove.title = "Remove project — chats stay under Chats";
    remove.textContent = "×";
    head.append(toggle, remove);
    const nest = document.createElement("div");
    nest.className = "agent-projects-ctx.sessions";
    nest.dataset.projectSessions = project.id;
    const kids = ctx.sessions
      .filter((s) => s.projectId === project.id)
      .sort((a, b) => Number(Boolean(b.pinned)) - Number(Boolean(a.pinned)));
    if (!kids.length) {
      const none = document.createElement("p");
      none.className = "agent-projects-none";
      none.textContent = "No chats";
      nest.append(none);
    } else {
      for (const session of kids) {
        appendSessionRow(nest, session, { nested: true });
      }
    }
    wrap.append(head, nest);
    host.append(wrap);
  }
}

export function renderSessions() {
  const list = host.sessionListEl();
  if (!list) {
    return;
  }
  list.replaceChildren();
  renderProjectsNav();
  const ungrouped = ctx.sessions.filter((s) => !s.projectId);
  const pinned = ungrouped.filter((s) => s.pinned);
  const groups = [
    { id: "Pinned", items: pinned },
    { id: "Today", items: ungrouped.filter((s) => !s.pinned && s.group === "Today") },
    { id: "Earlier", items: ungrouped.filter((s) => !s.pinned && s.group === "Earlier") },
  ];
  if (ungrouped.length) {
    const chatsLabel = document.createElement("div");
    chatsLabel.className = "agent-harness-group-label";
    chatsLabel.textContent = "Chats";
    list.append(chatsLabel);
    for (const group of groups) {
      if (!group.items.length) {
        continue;
      }
      const label = document.createElement("div");
      label.className = "agent-harness-group-label agent-harness-group-label-sub";
      label.textContent = group.id;
      list.append(label);
      for (const session of group.items) {
        appendSessionRow(list, session);
      }
    }
  }
}

/* Settings / Usage / workbench → agent-configure.js */
export function projectCreateEl() {
  return document.querySelector("[data-project-create]");
}

export function closeProjectCreate() {
  const form = projectCreateEl();
  const input = document.querySelector("[data-project-create-input]");
  if (form) {
    form.hidden = true;
  }
  if (input) {
    input.value = "";
  }
}

export function openProjectCreate() {
  const form = projectCreateEl();
  const input = document.querySelector("[data-project-create-input]");
  if (!form || !input) {
    return;
  }
  form.hidden = false;
  input.focus();
  input.select?.();
}

export function submitProjectCreate() {
  const input = document.querySelector("[data-project-create-input]");
  const title = String(input?.value || "").trim();
  if (!title) {
    closeProjectCreate();
    return false;
  }
  const project = createProject(title);
  closeProjectCreate();
  if (!project) {
    return false;
  }
  /* After add: file the chat that opened “New project…”, else the ctx.active
     ungrouped chat (ChatGPT/Claude-like). */
  const pending = ctx.sessions.find((s) => s.id === pendingProjectAssignSessionId);
  pendingProjectAssignSessionId = null;
  if (pending) {
    pending.projectId = project.id;
  } else {
    const session = ctx.sessions.find((s) => s.id === ctx.activeSessionId);
    if (session && !session.projectId) {
      session.projectId = project.id;
    }
  }
  renderSessions();
  const item = document.querySelector(`.agent-projects-item[data-project-id="${project.id}"]`);
  const nest = item?.querySelector("[data-project-ctx.sessions]");
  const toggle = item?.querySelector("[data-project-toggle]");
  if (nest) {
    nest.hidden = false;
  }
  toggle?.setAttribute("aria-expanded", "true");
  item?.classList.add("is-just-created");
  item?.scrollIntoView?.({ block: "nearest" });
  window.setTimeout(() => item?.classList.remove("is-just-created"), 1200);
  persistAgentWorkspaceSoon();
  return true;
}

export function ensureSessionForPrompt(prompt) {
  if (ctx.activeSessionId) {
    const existing = ctx.sessions.find((s) => s.id === ctx.activeSessionId);
    if (existing && existing.messages.length === 0) {
      existing.title = titleFromPrompt(prompt);
      return existing;
    }
    if (existing && existing.title === "New chat") {
      existing.title = titleFromPrompt(prompt);
      return existing;
    }
  }
  const session = {
    id: `s-${Date.now()}`,
    title: titleFromPrompt(prompt),
    group: "Today",
    messages: [],
  };
  ctx.sessions = [session, ...sessions];
  ctx.activeSessionId = session.id;
  return session;
}

export function selectSession(sessionId) {
  if (!sessionId || !ctx.sessions.some((s) => s.id === sessionId)) {
    return;
  }
  stopMockStream({ keepPartial: true });
  closeHarnessPage();
  ctx.activeSessionId = sessionId;
  renderSessions();
  renderActiveSession();
  persistAgentWorkspaceSoon();
}

export function newChat() {
  stopMockStream({ keepPartial: false });
  closeHarnessPage();
  ctx.followUpQueue = [];
  renderFollowUpQueue();
  const session = {
    id: `s-${Date.now()}`,
    title: "New chat",
    group: "Today",
    messages: [],
  };
  ctx.sessions = [session, ...sessions];
  ctx.activeSessionId = session.id;
  renderSessions();
  renderActiveSession();
  shelfComposerInput()?.focus({ preventScroll: true });
  syncAgentSendButton();
  persistAgentWorkspaceSoon();
}

export function renameSession(sessionId) {
  const session = ctx.sessions.find((s) => s.id === sessionId);
  if (!session) {
    return;
  }
  const next = window.prompt("Rename chat", session.title);
  if (!next?.trim()) {
    return;
  }
  session.title = next.trim().slice(0, 64);
  if (session.id === ctx.activeSessionId) {
    setTitle(session.title);
  }
  renderSessions();
  persistAgentWorkspaceSoon();
}

export function deleteSession(sessionId) {
  ctx.sessions = ctx.sessions.filter((s) => s.id !== sessionId);
  if (ctx.activeSessionId === sessionId) {
    ctx.activeSessionId = ctx.sessions[0]?.id || null;
    renderActiveSession();
  }
  renderSessions();
  persistAgentWorkspaceSoon();
}

export function assignSessionProject(sessionId, projectId) {
  const session = ctx.sessions.find((s) => s.id === sessionId);
  if (!session) {
    return;
  }
  session.projectId = projectId || null;
  renderSessions();
  persistAgentWorkspaceSoon();
}

export function sessionActionsEl() {
  return document.getElementById("agent-session-actions");
}

export function sessionActionsOpen() {
  const menu = sessionActionsEl();
  return Boolean(menu && !menu.hidden);
}

export function closeSessionActions() {
  const menu = sessionActionsEl();
  if (menu) {
    menu.hidden = true;
    host.clearFloatingMenuStyle(menu);
  }
  for (const btn of document.querySelectorAll("[data-session-menu][aria-expanded='true']")) {
    btn.setAttribute("aria-expanded", "false");
  }
  sessionActionsId = null;
}

export function openSessionActions(sessionId, anchor) {
  const menu = sessionActionsEl();
  const list = menu?.querySelector("[data-session-actions-list]");
  const session = ctx.sessions.find((s) => s.id === sessionId);
  if (!menu || !list || !session || !anchor) {
    return;
  }
  host.closeApproveMenu();
  host.closeModelMenu();
  sessionActionsId = sessionId;
  list.replaceChildren();

  const addAction = (label, action, { danger = false, disabled = false } = {}) => {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = `agent-session-action${danger ? " is-danger" : ""}`;
    btn.role = "menuitem";
    btn.dataset.sessionAction = action;
    btn.textContent = label;
    btn.disabled = disabled;
    list.append(btn);
  };

  addAction(session.pinned ? "Unpin" : "Pin", "pin");

  const projects = listProjects();
  if (session.projectId) {
    addAction("Remove from project", "ungroup");
  }
  if (!projects.length) {
    addAction("Add to project…", "new-project");
  } else {
    const label = document.createElement("p");
    label.className = "agent-session-actions-label";
    label.textContent = session.projectId ? "Move to project" : "Add to project";
    list.append(label);
    for (const project of projects) {
      if (project.id === session.projectId) {
        continue;
      }
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "agent-session-action";
      btn.role = "menuitem";
      btn.dataset.sessionProject = project.id;
      btn.textContent = project.title;
      list.append(btn);
    }
    const create = document.createElement("button");
    create.type = "button";
    create.className = "agent-session-action is-quiet";
    create.role = "menuitem";
    create.dataset.sessionAction = "new-project";
    create.textContent = "New project…";
    list.append(create);
  }

  addAction("Rename", "rename");
  addAction("Export", "export");
  addAction("Delete", "delete", { danger: true });

  for (const btn of document.querySelectorAll("[data-session-menu]")) {
    btn.setAttribute("aria-expanded", btn.dataset.sessionMenu === sessionId ? "true" : "false");
  }
  menu.hidden = false;
  host.positionFloatingMenu(menu, anchor, { minWidth: 200, maxWidth: 260, preferRight: true });
  menu.focus?.({ preventScroll: true });
}

export function runSessionAction(action) {
  const id = sessionActionsId;
  const session = ctx.sessions.find((s) => s.id === id);
  closeSessionActions();
  if (!session || !action) {
    return;
  }
  if (action === "pin") {
    session.pinned = !session.pinned;
    renderSessions();
    persistAgentWorkspaceSoon();
    return;
  }
  if (action === "ungroup") {
    assignSessionProject(id, null);
    return;
  }
  if (action === "new-project") {
    openProjectCreate();
    /* After create, submitProjectCreate already assigns the ctx.active chat if ungrouped.
       If this chat isn't ctx.active, assign once the project exists via a one-shot flag. */
    pendingProjectAssignSessionId = id;
    return;
  }
  if (action === "rename") {
    renameSession(id);
    return;
  }
  if (action === "export") {
    ctx.activeSessionId = id;
    exportActiveSessionMarkdown();
    return;
  }
  if (action === "delete") {
    deleteSession(id);
  }
}
