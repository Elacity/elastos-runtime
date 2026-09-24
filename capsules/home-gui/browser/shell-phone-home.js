/* Phone Home screen. On a phone the resting screen is the app grid, as on
   every phone OS, not the desktop: no files on the wallpaper (Library has the
   Desktop and the Bin) and no Apps sheet to slide up. The Dock holds the
   first PHONE_DOCK_SLOTS Shelf pins, the Shelf the desktop Dock shows too, so
   there is one layout, not a second phone one. An app is in the Dock or in
   the grid, never both; the grid keeps the launcher's order (apps, then
   Library items) and does not reshuffle as apps run. docs/HOME_MOBILE.md. */

export const PHONE_DOCK_SLOTS = 4;

// The Assistant's own Dock tile shows whenever this Home has the Assistant
// (syncAssistantFaceAvailability), so it never takes a grid slot.
const DOCK_TILE_TARGETS = ["assistant"];

// `pins` is the Shelf order, already limited to targets this Home has.
export function phoneDockTargetIds(pins) {
  return pins.slice(0, PHONE_DOCK_SLOTS);
}

export function phoneHomeTargets(targets, pins) {
  const inDock = new Set([...DOCK_TILE_TARGETS, ...phoneDockTargetIds(pins)]);
  const onHome = targets.filter((app) => !inDock.has(app.target));
  const isObject = (app) => app.target_kind === "object";
  return [...onHome.filter((app) => !isObject(app)), ...onHome.filter(isObject)];
}
