// File-manager-style colour tags for library items, keyed by object URI — an organisation/testing
// aid that never touches the trusted runtime. A single colour per item (clicking a colour toggles
// it; "Clear" removes). The seven colours use a familiar desktop tag palette.
//
// SESSION-MEMORY ONLY (deliberately, not a port of the dkms original): the dkms version stored this
// map in `window.localStorage`, which trips the no-ambient-storage invariant `home-entropy-check.mjs`
// enforces for Home/Library surfaces. There is no library-provider API surface that persists
// arbitrary per-object client tags (checked: no tag-related route on the gateway or in
// `library.rs`), so tags reset on reload/relaunch — the same tradeoff `app.js` already makes for
// view preferences (`viewPreferenceStore`, an in-memory Map standing in for `localStorage`).

const tagsByUri = new Map();

export const TAG_COLORS = [
  { id: "red", label: "Red", hex: "#fc5b57" },
  { id: "orange", label: "Orange", hex: "#fba23a" },
  { id: "yellow", label: "Yellow", hex: "#f7cf45" },
  { id: "green", label: "Green", hex: "#5bc454" },
  { id: "blue", label: "Blue", hex: "#4a9bf6" },
  { id: "purple", label: "Purple", hex: "#c46be0" },
  { id: "grey", label: "Grey", hex: "#9aa0a6" },
];

// The colour id currently applied to `uri` ("" when untagged).
export function getTag(uri) {
  if (!uri) return "";
  return tagsByUri.get(uri) || "";
}

// Set (or with a falsy colour, clear) the tag for `uri`.
export function setTag(uri, colorId) {
  if (!uri) return;
  if (colorId) {
    tagsByUri.set(uri, colorId);
  } else {
    tagsByUri.delete(uri);
  }
}

export function clearTag(uri) {
  setTag(uri, "");
}

// Look up the colour descriptor for a tag id (or null).
export function tagColor(colorId) {
  return TAG_COLORS.find((color) => color.id === colorId) || null;
}
