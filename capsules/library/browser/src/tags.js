// File-manager-style colour tags for library items. Stored LOCALLY (this browser, via localStorage)
// keyed by object URI — instant and backend-free, an organisation/testing aid that never touches
// the trusted runtime. A single colour per item (clicking a colour toggles it; "Clear" removes).
// The seven colours use a familiar desktop tag palette.

const STORAGE_KEY = "elastos.library.tags.v1";

export const TAG_COLORS = [
  { id: "red", label: "Red", hex: "#fc5b57" },
  { id: "orange", label: "Orange", hex: "#fba23a" },
  { id: "yellow", label: "Yellow", hex: "#f7cf45" },
  { id: "green", label: "Green", hex: "#5bc454" },
  { id: "blue", label: "Blue", hex: "#4a9bf6" },
  { id: "purple", label: "Purple", hex: "#c46be0" },
  { id: "grey", label: "Grey", hex: "#9aa0a6" },
];

function readAll() {
  try {
    return JSON.parse(window.localStorage.getItem(STORAGE_KEY) || "{}") || {};
  } catch (_err) {
    return {};
  }
}

function writeAll(map) {
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(map));
  } catch (_err) {
    /* storage full / disabled — tagging is a best-effort local convenience */
  }
}

// The colour id currently applied to `uri` ("" when untagged).
export function getTag(uri) {
  if (!uri) return "";
  return readAll()[uri] || "";
}

// Set (or with a falsy colour, clear) the tag for `uri`.
export function setTag(uri, colorId) {
  if (!uri) return;
  const map = readAll();
  if (colorId) {
    map[uri] = colorId;
  } else {
    delete map[uri];
  }
  writeAll(map);
}

export function clearTag(uri) {
  setTag(uri, "");
}

// Look up the colour descriptor for a tag id (or null).
export function tagColor(colorId) {
  return TAG_COLORS.find((color) => color.id === colorId) || null;
}
