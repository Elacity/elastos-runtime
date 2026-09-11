/* Pure workspace transition. Storage keeps complete records; context/UI limits
   belong to their consumers. Provider run and request IDs remain unchanged. */

export const cloneWorkspace = value => value === undefined ? undefined : JSON.parse(JSON.stringify(value));
const object = value => value !== null && typeof value === "object" && !Array.isArray(value);
const list = value => Array.isArray(value) ? value : [];
const own = (value, key) => Object.prototype.hasOwnProperty.call(value, key);

function stable(value) {
  if (value === undefined) return "undefined";
  if (Array.isArray(value)) return `[${value.map(stable).join(",")}]`;
  if (object(value)) return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${stable(value[key])}`).join(",")}}`;
  return JSON.stringify(value);
}

const equal = (a, b) => stable(a) === stable(b);
const scopedId = (source, kind, id, occurrence = 0) => `${source}:${encodeURIComponent(JSON.stringify([kind, id, occurrence]))}`;

function importedTurn(turn, source) {
  if (!object(turn)) return cloneWorkspace(turn);
  const active = !turn.completedAt && ["pending", "queued", "submitted", "starting", "running", "streaming", "cancel_pending", "settlement_unknown"].includes(turn.state);
  return {
    ...cloneWorkspace(turn),
    actorCapsule: source === "assistant" ? "assistant" : "home-agent",
    ...(active ? { state: "settlement_unknown" } : {}),
  };
}

function indexIds(records, source, kind) {
  const first = new Map();
  const counts = new Map();
  const ids = records.map((record, index) => {
    const original = record?.id ?? index;
    const key = stable(original);
    const occurrence = counts.get(key) || 0;
    counts.set(key, occurrence + 1);
    const id = scopedId(source, kind, original, occurrence);
    if (!first.has(original)) first.set(original, id);
    return id;
  });
  return { ids, resolve: id => first.get(id) ?? scopedId(source, kind, id) };
}

function importSource(source, raw) {
  const doc = source === "homeAgent" ? raw.document : raw;
  if (!object(doc)) throw new TypeError(`Invalid ${source} workspace document`);
  const sessions = list(doc.sessions);
  const projects = list(doc.projects);
  const sessionIds = indexIds(sessions, source, "session");
  const projectIds = indexIds(projects, source, "project");
  const imported = sessions.map((record, index) => {
    const session = cloneWorkspace(record);
    const messages = list(session.messages);
    const messageIds = indexIds(messages, `${source}/session/${index}`, "message");
    return {
      ...session,
      id: sessionIds.ids[index],
      title: session.title ?? "Imported conversation",
      group: session.group ?? "Earlier",
      mode: session.mode ?? "chat",
      ...(session.projectId != null ? { projectId: projectIds.resolve(session.projectId) } : {}),
      ...(session.forkedFrom != null ? { forkedFrom: sessionIds.resolve(session.forkedFrom) } : {}),
      ...(session.branchId != null ? { branchId: scopedId(source, "branch", session.branchId) } : {}),
      legacyOrigin: { source, id: cloneWorkspace(session.id) },
      ...(own(session, "lastTurn") ? { lastTurn: importedTurn(session.lastTurn, source) } : {}),
      messages: messages.map((original, messageIndex) => {
        const message = cloneWorkspace(original);
        return {
          ...message,
          id: messageIds.ids[messageIndex],
          role: message.role === "assistant" ? "agent" : message.role,
          text: message.text ?? message.content ?? message.summary ?? "",
          ...(message.parentId != null ? { parentId: messageIds.resolve(message.parentId) } : {}),
          ...(message.branchId != null ? { branchId: scopedId(source, "branch", message.branchId) } : {}),
          legacyOrigin: { source, id: cloneWorkspace(message.id), role: message.role },
          ...(own(message, "turn") ? { turn: importedTurn(message.turn, source) } : {}),
        };
      }),
    };
  });
  const drafts = [];
  if (typeof doc.draft === "string" && doc.draft.length) drafts.push(["draft", { text: doc.draft, parts: [] }]);
  if (object(doc.composerDraft) && (doc.composerDraft.text || list(doc.composerDraft.parts).length)) {
    drafts.push(["composerDraft", cloneWorkspace(doc.composerDraft)]);
  }
  if (typeof doc.studioDraft === "string" && doc.studioDraft.length) drafts.push(["studioDraft", { text: doc.studioDraft, parts: [] }]);
  for (const [field, draft] of drafts) {
    imported.push({
      id: scopedId(source, "draft", field),
      title: `${source === "assistant" ? "Assistant" : source === "homeAgent" ? "Home Agent" : "Home"} saved ${field === "studioDraft" ? "Studio " : ""}draft`,
      group: "Earlier", mode: field === "studioDraft" ? "studio" : "chat", pinned: true, archived: false,
      composerDraft: cloneWorkspace(draft),
      legacyOrigin: { source, field },
      // This is a visible draft, never a submitted user message or model run.
      messages: [],
    });
  }
  return {
    document: cloneWorkspace(doc),
    sessions: imported,
    projects: projects.map((project, index) => ({ ...cloneWorkspace(project), id: projectIds.ids[index], legacyOrigin: { source, id: cloneWorkspace(project.id) } })),
    activeSessionId: doc.activeSessionId != null ? sessionIds.resolve(doc.activeSessionId) : imported[0]?.id,
    selection: source === "assistant"
      ? { liveOfferId: doc.selected_offer_id ?? "", selectedModelCid: doc.selected_model_cid ?? null }
      : { liveOfferId: doc.liveOfferId ?? "", selectedModelCid: doc.selectedModelCid ?? null },
  };
}

/** Accept the server's exact `legacy` object. Existing import markers make
 * repeat imports a no-op, including when a legacy source changes later.
 * Root renders session.composerDraft as an editable draft on selecting that entry.
 * legacyImports retains each complete original envelope and all unknown fields. */
export function migrateLegacyWorkspaces(legacy = {}, canonicalDocument = {}) {
  const result = cloneWorkspace(canonicalDocument);
  if (!object(result)) throw new TypeError("Canonical workspace must be an object");
  result.v ??= 1;
  result.sessions = list(result.sessions);
  result.projects = list(result.projects);
  result.legacyImports = object(result.legacyImports) ? result.legacyImports : {};
  const existingIds = new Set([...result.sessions, ...result.projects].map(record => record.id));
  // The current Sash workspace supplies the active view; every other selection
  // remains in its source snapshot instead of silently replacing that choice.
  for (const source of ["homeAgent", "assistant", "homeSessionAgent"]) {
    const raw = legacy[source];
    if (raw == null || own(result.legacyImports, source)) continue;
    const imported = importSource(source, raw);
    for (const record of [...imported.sessions, ...imported.projects]) {
      if (existingIds.has(record.id)) throw new Error(`Imported ID already exists: ${record.id}`);
      existingIds.add(record.id);
    }
    if (Object.keys(result.legacyImports).length === 0 && !canonicalDocument.sessions?.length) {
      // Keep all top-level fields, including future metadata; install remapped
      // collection/selection fields below instead of passing through a serializer.
      for (const [key, value] of Object.entries(imported.document)) {
        if (!["sessions", "projects", "activeSessionId", "legacyImports"].includes(key) && !own(result, key)) {
          Object.defineProperty(result, key, { value: cloneWorkspace(value), enumerable: true, writable: true, configurable: true });
        }
      }
      result.activeSessionId ??= imported.activeSessionId;
      Object.assign(result, imported.selection);
    }
    result.sessions.push(...imported.sessions);
    result.projects.push(...imported.projects);
    result.legacyImports[source] = { snapshot: cloneWorkspace(raw), selection: imported.selection };
  }
  return result;
}

// A stable content label, not an integrity or authority primitive. Collision
// checks below compare complete records before reusing any generated ID.
function label(value) {
  let hash = 2166136261;
  for (const character of stable(value)) hash = Math.imul(hash ^ character.codePointAt(0), 16777619) >>> 0;
  return hash.toString(16).padStart(8, "0");
}

function mergeValue(base, local, remote, path, conflicts) {
  if (equal(local, remote) || equal(remote, base)) return cloneWorkspace(local);
  if (equal(local, base)) return cloneWorkspace(remote);
  if (object(local) && object(remote) && (object(base) || base === undefined)) {
    const merged = {};
    for (const key of new Set([...Object.keys(base || {}), ...Object.keys(local), ...Object.keys(remote)])) {
      const value = mergeValue(base?.[key], local[key], remote[key], [...path, key], conflicts);
      if (value !== undefined) Object.defineProperty(merged, key, { value, enumerable: true, writable: true, configurable: true });
    }
    return merged;
  }
  conflicts.push({ path, base: cloneWorkspace(base), local: cloneWorkspace(local), remote: cloneWorkspace(remote),
    localPresent: local !== undefined, remotePresent: remote !== undefined });
  return cloneWorkspace(local);
}

function appendDistinct(records, record, identity) {
  identity = cloneWorkspace(identity);
  const prefix = `recovered:${label(identity)}`;
  let id = prefix;
  let suffix = 0;
  for (;;) {
    const existing = records.find(item => item.id === id);
    if (!existing) { records.push({ ...record, id }); return id; }
    if (equal(existing.recoveryIdentity, identity)) return id;
    id = `${prefix}:${++suffix}`;
  }
}

function mergeRecords(kind, base, local, remote, conflicts) {
  const maps = [base, local, remote].map(records => new Map(list(records).map(record => [record.id, record])));
  for (let i = 0; i < maps.length; i++) {
    if (maps[i].size !== list([base, local, remote][i]).length) throw new Error(`Duplicate ${kind} IDs`);
  }
  const [b, l, r] = maps;
  const records = [];
  const forks = [];
  for (const id of new Set([...l.keys(), ...r.keys(), ...b.keys()])) {
    const disputed = [];
    const merged = mergeValue(b.get(id), l.get(id), r.get(id), [kind, id], disputed);
    if (!disputed.length) {
      if (merged !== undefined) records.push(merged);
      continue;
    }
    conflicts.push(...disputed);
    // Keep the remote identity as saved, and the complete local version as a
    // visible recovery copy. A delete/edit conflict keeps the edited survivor.
    if (r.has(id)) records.push(cloneWorkspace(r.get(id)));
    if (l.has(id)) forks.push({ record: cloneWorkspace(l.get(id)), identity: { kind, id, local: l.get(id), remote: r.get(id) } });
  }
  for (const { record, identity } of forks) {
    const recovery = { ...record, title: `${record.title || "Untitled"} (recovered changes)`, recoveryIdentity: cloneWorkspace(identity) };
    if (kind === "sessions") {
      recovery.archived = false; recovery.projectId = null; recovery.forkedFrom = record.id;
      if (object(recovery.lastTurn)) recovery.lastTurn = { ...recovery.lastTurn, attachmentAllowed: false };
    }
    appendDistinct(records, recovery, identity);
  }
  return records;
}

/** Three-way save recovery. Disjoint fields merge. Session/project conflicts
 * retain complete variants; other conflicts also get visible recovery entries.
 * Caller must show the conflict notice and let users review recovered copies.
 * No provider operation is issued and no run/request identity is rewritten. */
export function mergeWorkspaceDocuments(base = {}, local = {}, remote = {}) {
  const conflicts = [];
  const withoutRecords = doc => Object.fromEntries(Object.entries(doc).filter(([key]) => !["sessions", "projects"].includes(key)));
  const document = mergeValue(withoutRecords(base), withoutRecords(local), withoutRecords(remote), [], conflicts);
  const fieldConflicts = conflicts.slice();
  document.sessions = mergeRecords("sessions", base.sessions, local.sessions, remote.sessions, conflicts);
  document.projects = mergeRecords("projects", base.projects, local.projects, remote.projects, conflicts);
  for (const conflict of fieldConflicts) {
    const identity = { kind: "workspace-field", ...conflict };
    appendDistinct(document.sessions, {
      title: `Recovered changes: ${conflict.path.join(" / ")}`, group: "Today", archived: false, pinned: true,
      mode: "chat", recoveryIdentity: cloneWorkspace(identity),
      ...(conflict.path[0] === "composerDraft" && object(remote.composerDraft)
        ? { composerDraft: cloneWorkspace(remote.composerDraft) } : {}),
      messages: [{ id: `recovery-note:${label(identity)}`, role: "system", text: JSON.stringify(conflict, null, 2) }],
    }, identity);
  }
  // Keep the local composer attached to its complete local conversation after
  // a conflict, rather than stamping that draft onto the remote variant.
  const activeRecovery = document.sessions.find(session => session.recoveryIdentity?.kind === "sessions" &&
    session.recoveryIdentity.id === local.activeSessionId);
  if (activeRecovery) {
    document.activeSessionId = activeRecovery.id;
  } else if (!document.sessions.some(session => session.id === document.activeSessionId)) {
    document.activeSessionId = document.sessions[0]?.id ?? null;
  }
  return { document, conflicts, requiresReview: conflicts.length > 0 };
}
