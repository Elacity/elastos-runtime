/* Monotonic cursor for runs_events.
   Number(null) and Number("") are 0; treating that as a valid provider cursor
   replays the full log and pegs the tab.
   If the provider ignores `cursor` and returns the whole log every poll, slice
   locally from `applied`. Re-applying the full array concatenates unbounded
   duplicates on the main thread (100% CPU, empty caret). */

const MAX_EVENTS_PER_POLL = 256;

export function selectUnseenRunEvents(applied, events, reported) {
  const list = Array.isArray(events) ? events : [];
  const from = Math.max(0, Number(applied) || 0);
  if (list.length === 0) {
    return list;
  }
  if (from === 0) {
    return list.length > MAX_EVENTS_PER_POLL
      ? list.slice(0, MAX_EVENTS_PER_POLL)
      : list;
  }
  const reportedN = Number(reported);
  const total =
    Number.isFinite(reportedN) && reportedN > 0 ? reportedN : list.length;
  const fullLog = list.length >= from && list.length >= total && total >= from;
  const unseen = fullLog ? list.slice(from) : list;
  return unseen.length > MAX_EVENTS_PER_POLL
    ? unseen.slice(0, MAX_EVENTS_PER_POLL)
    : unseen;
}

export function nextAppliedCursor(applied, unseenCount, reported) {
  const from = Math.max(0, Number(applied) || 0);
  const count = Math.max(0, Number(unseenCount) || 0);
  if (count > 0) {
    return from + count;
  }
  const reportedN = Number(reported);
  if (Number.isFinite(reportedN) && reportedN > from) {
    return reportedN;
  }
  return from;
}

export function nextEventCursor(from, reported, eventCount) {
  const unseen = Math.max(0, Number(eventCount) || 0);
  return nextAppliedCursor(from, unseen, reported);
}
