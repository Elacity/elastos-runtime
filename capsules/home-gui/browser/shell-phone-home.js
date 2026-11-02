/* Phone Home screen. On a phone the resting screen is the app grid, as on
   every phone OS, not the desktop: no files on the wallpaper (Library has the
   Desktop and the Bin) and no Apps sheet to slide up. An app is in the Dock
   or on the grid, never both; the grid keeps the launcher's order (apps,
   then Library items) and does not reshuffle as apps run.

   The phone arrangement is the person's own, kept beside the desktop's in
   the Home layout: `homeDock` (up to PHONE_DOCK_SLOTS target ids) and
   `homePages` (pages of target ids), both written by the edit mode
   (shell-phone-home-edit.js). Until they arrange anything both are absent:
   the Dock is the first Shelf pins and the grid one run in launcher order.
   Arranging the phone never moves the desktop Shelf. Stored ids are a wish,
   not a contract: ones this Home no longer has drop out, new installs join
   the last page. docs/HOME_MOBILE.md. */

export const PHONE_DOCK_SLOTS = 4;

// The Assistant's own Dock tile shows whenever this Home has the Assistant
// (syncAssistantFaceAvailability), so it never takes a Dock slot or a grid
// cell.
export const PHONE_ASSISTANT_TARGET = "assistant";

export function normalizePhoneHomeIds(value) {
  if (!Array.isArray(value)) {
    return null;
  }
  return value.filter((id) => typeof id === "string" && id.length > 0);
}

export function normalizePhoneHomePages(value) {
  if (!Array.isArray(value)) {
    return null;
  }
  return value.filter(Array.isArray).map(normalizePhoneHomeIds);
}

// `pins` is the Shelf order, already limited to targets this Home has.
export function phoneDockTargetIds(targets, pins, storedDock) {
  const known = new Set(targets.map((app) => app.target));
  const dock = [];
  for (const id of normalizePhoneHomeIds(storedDock) || pins) {
    if (known.has(id) && id !== PHONE_ASSISTANT_TARGET && !dock.includes(id)) {
      dock.push(id);
    }
  }
  return dock.slice(0, PHONE_DOCK_SLOTS);
}

export function phoneHomeTargets(targets, dockIds) {
  const inDock = new Set([PHONE_ASSISTANT_TARGET, ...dockIds]);
  const onHome = targets.filter((app) => !inDock.has(app.target));
  const isObject = (app) => app.target_kind === "object";
  return [...onHome.filter((app) => !isObject(app)), ...onHome.filter(isObject)];
}

// Pages as stored, less what the grid no longer holds, plus what no page
// lists on the last one; a page longer than the screen holds runs on to the
// next (fitPhoneHomePages).
export function phoneHomePages(targets, dockIds, storedPages) {
  const onHome = phoneHomeTargets(targets, dockIds);
  const byId = new Map(onHome.map((app) => [app.target, app]));
  const placed = new Set();
  const pages = [];
  for (const page of normalizePhoneHomePages(storedPages) || []) {
    const apps = [];
    for (const id of page) {
      if (byId.has(id) && !placed.has(id)) {
        placed.add(id);
        apps.push(byId.get(id));
      }
    }
    if (apps.length > 0) {
      pages.push(apps);
    }
  }
  const unplaced = onHome.filter((app) => !placed.has(app.target));
  if (unplaced.length > 0) {
    if (pages.length > 0) {
      pages[pages.length - 1].push(...unplaced);
    } else {
      pages.push(unplaced);
    }
  }
  return pages.length > 0 ? pages : [[]];
}

// How many icons one page holds is the screen's business (4 or 6 columns,
// as many rows as fit), so a stored page may be longer than a page here.
export function fitPhoneHomePages(pages, perPage) {
  const size = Math.max(1, Math.floor(perPage) || 1);
  const fitted = [];
  for (const page of pages) {
    if (page.length === 0) {
      fitted.push([]);
      continue;
    }
    for (let start = 0; start < page.length; start += size) {
      fitted.push(page.slice(start, start + size));
    }
  }
  return fitted.length > 0 ? fitted : [[]];
}

/* One drop in edit mode, on pages of ids. `id` leaves wherever it was and
   lands at `index` on page `pageIndex`; a page index one past the end opens a
   new page there. A page pushed past `perPage` hands its last icon to the
   front of the next page, as a phone does, and pages left empty close. */
export function movePhoneHomeTarget(pages, id, pageIndex, index, perPage) {
  const size = Math.max(1, Math.floor(perPage) || 1);
  const next = pages.map((page) => page.filter((candidate) => candidate !== id));
  const to = Math.max(0, Math.min(pageIndex, next.length));
  if (to === next.length) {
    next.push([]);
  }
  const page = next[to];
  page.splice(Math.max(0, Math.min(index, page.length)), 0, id);
  for (let at = to; at < next.length; at += 1) {
    while (next[at].length > size) {
      if (at + 1 === next.length) {
        next.push([]);
      }
      next[at + 1].unshift(next[at].pop());
    }
  }
  return next.filter((candidate) => candidate.length > 0);
}

// An app leaving the grid (dragged into the Dock).
export function removePhoneHomeTarget(pages, id) {
  return pages
    .map((page) => page.filter((candidate) => candidate !== id))
    .filter((page) => page.length > 0);
}

// One drop onto the Dock at `index`, or null when a full Dock cannot take a
// newcomer (a drag within the Dock always fits).
export function movePhoneDockTarget(dock, id, index) {
  const next = dock.filter((candidate) => candidate !== id);
  if (next.length === dock.length && next.length >= PHONE_DOCK_SLOTS) {
    return null;
  }
  next.splice(Math.max(0, Math.min(index, next.length)), 0, id);
  return next;
}
