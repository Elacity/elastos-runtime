// Books are archives of XHTML chapters. A book is a file somebody else wrote
// and sold, so its markup is treated as hostile: a chapter is never handed to
// the page as markup and never given a frame of its own. It is parsed, walked
// once, and rebuilt as nodes from a fixed list of tags and attributes. Anything
// not on that list does not survive the walk.

import { findEntry, listEntries, readEntry } from "./zip.js";

export const MAX_CHAPTER_NODES = 50000;
const MAX_ELEMENT_DEPTH = 64;
const MAX_TEXT_ATTRIBUTE_LENGTH = 2048;
const MAX_COUNT_ATTRIBUTE = 10000;

const ELEMENT_NODE = 1;
const TEXT_NODE = 3;
const CDATA_NODE = 4;

const BOOK_DAMAGED = "This book is damaged.";
const BOOK_EMPTY = "This book has no readable pages.";
const NO_SUCH_CHAPTER = "This book does not have that chapter.";
const CHAPTER_UNREADABLE = "This chapter cannot be shown.";
const CHAPTER_TOO_LARGE = "This chapter is too large to show.";

const CONTAINER_PATH = "META-INF/container.xml";
const PACKAGE_MEDIA_TYPE = "application/oebps-package+xml";

// Tags a chapter may keep, each with the attributes allowed on it. A tag that
// is not here is unwrapped: its words survive, the tag itself does not.
const ALLOWED_TAGS = new Map([
  ["a", ["href"]],
  ["abbr", []],
  ["address", []],
  ["article", []],
  ["aside", []],
  ["b", []],
  ["bdi", []],
  ["bdo", []],
  ["blockquote", ["cite"]],
  ["br", []],
  ["caption", []],
  ["cite", []],
  ["code", []],
  ["dd", []],
  ["del", []],
  ["dfn", []],
  ["div", []],
  ["dl", []],
  ["dt", []],
  ["em", []],
  ["figcaption", []],
  ["figure", []],
  ["footer", []],
  ["h1", []],
  ["h2", []],
  ["h3", []],
  ["h4", []],
  ["h5", []],
  ["h6", []],
  ["header", []],
  ["hr", []],
  ["i", []],
  ["img", ["alt", "height", "width"]],
  ["ins", []],
  ["kbd", []],
  ["li", ["value"]],
  ["main", []],
  ["mark", []],
  ["ol", ["start"]],
  ["p", []],
  ["pre", []],
  ["q", ["cite"]],
  ["rp", []],
  ["rt", []],
  ["ruby", []],
  ["s", []],
  ["samp", []],
  ["section", []],
  ["small", []],
  ["span", []],
  ["strong", []],
  ["sub", []],
  ["sup", []],
  ["table", []],
  ["tbody", []],
  ["td", ["colspan", "rowspan"]],
  ["tfoot", []],
  ["th", ["colspan", "rowspan", "scope"]],
  ["thead", []],
  ["time", ["datetime"]],
  ["tr", []],
  ["u", []],
  ["ul", []],
  ["var", []],
  ["wbr", []],
]);

const GLOBAL_ATTRIBUTES = ["dir", "lang", "title"];
const VOID_TAGS = new Set(["br", "hr", "img", "wbr"]);

// Tags that go with everything inside them. Keeping their words would leak
// code as text, restyle the page, repoint every relative link, load something
// from off the page, or open a nested document that runs its own markup.
const DROPPED_SUBTREES = new Set([
  "applet",
  "area",
  "audio",
  "base",
  "button",
  "canvas",
  "datalist",
  "dialog",
  "embed",
  "fieldset",
  "form",
  "frame",
  "frameset",
  "head",
  "iframe",
  "input",
  "label",
  "legend",
  "link",
  "map",
  "marquee",
  "math",
  "meta",
  "meter",
  "noembed",
  "noframes",
  "noscript",
  "object",
  "optgroup",
  "option",
  "output",
  "param",
  "portal",
  "progress",
  "script",
  "select",
  "slot",
  "source",
  "style",
  "svg",
  "template",
  "textarea",
  "title",
  "track",
  "video",
  "xmp",
]);

const SAFE_LINK_SCHEMES = ["http:", "https:", "mailto:"];

// Raster types only, and deliberately no drawing type. A drawing file is
// markup: the type the book declares would carry straight into the source the
// reader mints for it, and opening that source on its own would run the
// markup with this page's own standing. Inside a picture element it could not
// run, but the reader must not hand out a source that could, so a book's
// drawings simply do not get one.
const IMAGE_TYPES = new Map([
  ["avif", "image/avif"],
  ["bmp", "image/bmp"],
  ["gif", "image/gif"],
  ["jpeg", "image/jpeg"],
  ["jpg", "image/jpeg"],
  ["png", "image/png"],
  ["webp", "image/webp"],
]);
const ALLOWED_IMAGE_TYPES = new Set(IMAGE_TYPES.values());

function fail(message) {
  throw new Error(message);
}

function plainText(value) {
  const cleaned = value.replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g, "").trim();
  return cleaned.slice(0, MAX_TEXT_ATTRIBUTE_LENGTH);
}

function countValue(value) {
  const trimmed = value.trim();
  if (!/^\d{1,5}$/.test(trimmed)) return "";
  return Number(trimmed) <= MAX_COUNT_ATTRIBUTE ? String(Number(trimmed)) : "";
}

function enumValue(allowed) {
  return (value) => (allowed.includes(value.trim().toLowerCase()) ? value.trim().toLowerCase() : "");
}

function tokenValue(value) {
  const trimmed = value.trim();
  return /^[A-Za-z0-9-]{1,35}$/.test(trimmed) ? trimmed : "";
}

// A link is only kept when the scheme survives having every space and control
// character taken out of it, so `java&#9;script:` is refused the same way
// `javascript:` is.
function safeLink(value) {
  const collapsed = value.replace(/[\u0000-\u0020\u007f]/g, "").toLowerCase();
  const safe = SAFE_LINK_SCHEMES.some((scheme) => collapsed.startsWith(scheme));
  return safe ? value.trim().slice(0, MAX_TEXT_ATTRIBUTE_LENGTH) : "";
}

const ATTRIBUTE_RULES = new Map([
  ["alt", plainText],
  ["cite", safeLink],
  ["colspan", countValue],
  ["datetime", plainText],
  ["dir", enumValue(["ltr", "rtl", "auto"])],
  ["height", countValue],
  ["href", safeLink],
  ["lang", tokenValue],
  ["rowspan", countValue],
  ["scope", enumValue(["row", "col", "rowgroup", "colgroup"])],
  ["start", countValue],
  ["title", plainText],
  ["value", countValue],
  ["width", countValue],
]);

function childrenOf(node) {
  const children = node && node.childNodes;
  if (!children) return [];
  return Array.isArray(children) ? children : Array.from(children);
}

function tagOf(node) {
  return String(node.localName || node.nodeName || "").toLowerCase();
}

function elementsNamed(root, name) {
  const found = [];
  const stack = [root];
  while (stack.length > 0) {
    const current = stack.pop();
    if (!current) continue;
    if (current.nodeType === ELEMENT_NODE && tagOf(current) === name) found.push(current);
    const children = childrenOf(current);
    for (let index = children.length - 1; index >= 0; index -= 1) stack.push(children[index]);
  }
  return found;
}

function attributeOf(element, name) {
  if (typeof element.getAttribute === "function") return element.getAttribute(name);
  const attributes = element.attributes ? Array.from(element.attributes) : [];
  const found = attributes.find((attribute) => attribute.name === name);
  return found ? found.value : null;
}

/**
 * Turns a link inside the book into an archive path. Returns an empty string
 * for anything that points off the page or above the top of the archive.
 */
export function resolvePath(baseDir, href) {
  const raw = String(href || "").split("#")[0].split("?")[0];
  if (!raw) return "";
  let decoded = "";
  try {
    decoded = decodeURIComponent(raw);
  } catch {
    return "";
  }
  if (decoded.includes("\0") || decoded.includes("\\")) return "";
  const combined = decoded.startsWith("/") ? decoded.slice(1) : `${baseDir}${decoded}`;
  const parts = [];
  for (const segment of combined.split("/")) {
    if (segment === "" || segment === ".") continue;
    if (segment === "..") {
      if (parts.length === 0) return "";
      parts.pop();
      continue;
    }
    parts.push(segment);
  }
  return parts.join("/");
}

function directoryOf(path) {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? "" : path.slice(0, slash + 1);
}

function decodeText(bytes) {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return fail(BOOK_DAMAGED);
  }
}

function parseXml(markup, parseMarkup, message) {
  let document = null;
  try {
    document = parseMarkup(markup, "application/xml");
  } catch {
    return fail(message);
  }
  if (!document || elementsNamed(document, "parsererror").length > 0) fail(message);
  return document;
}

// A chapter is XHTML by the book format's own rules, but plenty of books ship
// markup that only an HTML parser will take. The rebuild that follows is the
// same either way, so the second attempt costs nothing in safety.
function parseChapterMarkup(markup, parseMarkup) {
  let document = null;
  try {
    document = parseMarkup(markup, "application/xhtml+xml");
  } catch {
    document = null;
  }
  if (document && elementsNamed(document, "parsererror").length === 0) return document;
  try {
    document = parseMarkup(markup, "text/html");
  } catch {
    return fail(CHAPTER_UNREADABLE);
  }
  if (!document || elementsNamed(document, "parsererror").length > 0) fail(CHAPTER_UNREADABLE);
  return document;
}

/** Reads the pointer that says where the book's package file lives. */
export function parseContainer(markup, parseMarkup) {
  const document = parseXml(markup, parseMarkup, BOOK_DAMAGED);
  const rootfiles = elementsNamed(document, "rootfile");
  if (rootfiles.length === 0) fail(BOOK_DAMAGED);
  const chosen = rootfiles.find(
    (rootfile) => String(attributeOf(rootfile, "media-type") || "").trim() === PACKAGE_MEDIA_TYPE,
  );
  if (!chosen) fail(BOOK_DAMAGED);
  const path = resolvePath("", String(attributeOf(chosen, "full-path") || "").trim());
  if (!path) fail(BOOK_DAMAGED);
  return path;
}

/** Reads the book's file list and the order its chapters are meant to be read in. */
export function parsePackage(markup, packagePath, parseMarkup) {
  const document = parseXml(markup, parseMarkup, BOOK_DAMAGED);
  const manifestElement = elementsNamed(document, "manifest")[0];
  const spineElement = elementsNamed(document, "spine")[0];
  if (!manifestElement || !spineElement) fail(BOOK_DAMAGED);

  const baseDir = directoryOf(packagePath);
  const manifest = new Map();
  for (const item of elementsNamed(manifestElement, "item")) {
    const id = String(attributeOf(item, "id") || "").trim();
    const href = String(attributeOf(item, "href") || "").trim();
    if (!id || !href) fail(BOOK_DAMAGED);
    const path = resolvePath(baseDir, href);
    if (!path) fail(BOOK_DAMAGED);
    manifest.set(id, {
      id,
      path,
      mediaType: String(attributeOf(item, "media-type") || "").trim(),
    });
  }

  const spine = [];
  for (const reference of elementsNamed(spineElement, "itemref")) {
    const idref = String(attributeOf(reference, "idref") || "").trim();
    if (!idref) fail(BOOK_DAMAGED);
    const item = manifest.get(idref);
    if (!item) fail(BOOK_DAMAGED);
    const declared = attributeOf(reference, "linear");
    const linear = declared === null || declared === undefined || declared === "" ? "yes" : declared;
    if (linear !== "yes" && linear !== "no") fail(BOOK_DAMAGED);
    spine.push({ ...item, linear: linear === "yes" });
  }

  // Which way the pages run is the book's to say, and a book that says
  // something else is not one this reader will guess at.
  const declaredDirection = String(
    attributeOf(spineElement, "page-progression-direction") || "default",
  ).trim();
  if (!["ltr", "rtl", "default"].includes(declaredDirection)) fail(BOOK_DAMAGED);

  return {
    manifest,
    spine,
    readingOrder: spine.filter((item) => item.linear),
    pageProgressionDirection: declaredDirection,
  };
}

function findChapterRoot(source) {
  const body = elementsNamed(source, "body")[0];
  if (body) return body;
  if (source.documentElement) return source.documentElement;
  return source;
}

function attributeListOf(element) {
  const attributes = element && element.attributes;
  if (!attributes) return [];
  return Array.isArray(attributes) ? attributes : Array.from(attributes);
}

function applyAttributes(source, element, tag, allowedForTag, resolveImage) {
  const attributes = attributeListOf(source);
  let href = "";

  if (tag === "img") {
    let declaredSource = "";
    for (const attribute of attributes) {
      const rawName = String(attribute.name || attribute.localName || "");
      if (rawName.toLowerCase() === "src" && !rawName.includes(":") && attribute.namespaceURI == null) {
        declaredSource = String(attribute.value ?? "");
      }
    }
    const resolved = resolveImage(declaredSource);
    if (!resolved) return false;
    element.setAttribute("src", resolved);
  }

  for (const attribute of attributes) {
    const rawName = String(attribute.name || attribute.localName || "");
    // A prefixed or namespaced attribute is never on the list: `xlink:href` and
    // `xml:base` both carry a URL that this walk would otherwise not see.
    if (rawName.includes(":") || attribute.namespaceURI != null) continue;
    const name = rawName.toLowerCase();
    if (name.startsWith("on")) continue;
    if (tag === "img" && name === "src") continue;
    if (!GLOBAL_ATTRIBUTES.includes(name) && !allowedForTag.includes(name)) continue;
    const rule = ATTRIBUTE_RULES.get(name);
    if (!rule) continue;
    const value = rule(String(attribute.value ?? ""));
    if (value === "") continue;
    element.setAttribute(name, value);
    if (tag === "a" && name === "href") href = value;
  }

  if (href) {
    element.setAttribute("rel", "noopener noreferrer");
    element.setAttribute("target", "_blank");
  }
  return true;
}

/**
 * Rebuilds a parsed chapter as nodes the page can hold. Nothing from the
 * chapter is ever turned back into markup, so nothing in it can be parsed a
 * second time.
 *
 * @param {Document | Element} source parsed chapter
 * @param {{ document: Document, resolveImage?: (src: string) => string }} options
 * @returns {DocumentFragment}
 */
export function sanitizeChapter(source, options = {}) {
  const target = options.document || globalThis.document;
  if (!target || typeof target.createElement !== "function") fail(CHAPTER_UNREADABLE);
  const resolveImage = typeof options.resolveImage === "function" ? options.resolveImage : () => "";

  const fragment = target.createDocumentFragment();
  let produced = 0;
  const work = [];
  const pushChildren = (node, parent, depth) => {
    const children = childrenOf(node);
    for (let index = children.length - 1; index >= 0; index -= 1) {
      work.push({ node: children[index], parent, depth });
    }
  };
  pushChildren(findChapterRoot(source), fragment, 0);

  while (work.length > 0) {
    const { node, parent, depth } = work.pop();
    if (!node) continue;
    if (produced >= MAX_CHAPTER_NODES) fail(CHAPTER_TOO_LARGE);

    if (node.nodeType === TEXT_NODE || node.nodeType === CDATA_NODE) {
      const data = typeof node.data === "string" ? node.data : "";
      if (data !== "") {
        parent.appendChild(target.createTextNode(data));
        produced += 1;
      }
      continue;
    }
    if (node.nodeType !== ELEMENT_NODE) continue;

    const tag = tagOf(node);
    if (DROPPED_SUBTREES.has(tag)) continue;

    const allowedForTag = ALLOWED_TAGS.get(tag);
    // An unknown tag, or one nested past the depth this reader lays out, keeps
    // its words and loses its box.
    if (!allowedForTag || depth >= MAX_ELEMENT_DEPTH) {
      pushChildren(node, parent, depth);
      continue;
    }

    const element = target.createElement(tag);
    if (!applyAttributes(node, element, tag, allowedForTag, resolveImage)) continue;
    parent.appendChild(element);
    produced += 1;
    if (!VOID_TAGS.has(tag)) pushChildren(node, element, depth + 1);
  }
  return fragment;
}

function imageTypeFor(path, declaredType) {
  if (ALLOWED_IMAGE_TYPES.has(declaredType)) return declaredType;
  // A book that declares a type this reader will not show is refused on that
  // declaration alone. Reading the file name instead would hand back a
  // different type than the book itself named, which is a substitution the
  // reader has no grounds to make. The file name is read only when the book
  // says nothing about the file at all.
  if (declaredType) return "";
  const dot = path.lastIndexOf(".");
  if (dot < 0) return "";
  return IMAGE_TYPES.get(path.slice(dot + 1).toLowerCase()) || "";
}

function resolveDependencies(dependencies) {
  return {
    parseMarkup:
      dependencies.parseMarkup ||
      ((markup, type) => new DOMParser().parseFromString(markup, type)),
    document: dependencies.document || globalThis.document,
    createObjectURL: dependencies.createObjectURL || ((blob) => URL.createObjectURL(blob)),
    revokeObjectURL: dependencies.revokeObjectURL || ((url) => URL.revokeObjectURL(url)),
  };
}

/**
 * Opens a book and returns its chapters in reading order. Each chapter comes
 * back as nodes, already rebuilt from the allowed tag list.
 *
 * The `mimetype` file a book carries is not read: the type of the file is
 * already known before this runs, and a second copy of it inside the archive
 * would only be an author's claim about the archive it sits in.
 *
 * @param {Uint8Array} bytes
 * @returns {Promise<{
 *   opfPath: string,
 *   chapterCount: number,
 *   chapters: { path: string, mediaType: string }[],
 *   readChapter: (index: number) => Promise<{ path: string, fragment: DocumentFragment }>,
 *   release: () => void,
 * }>}
 */
export async function openBook(bytes, dependencies = {}) {
  const { parseMarkup, document, createObjectURL, revokeObjectURL } =
    resolveDependencies(dependencies);
  const entries = listEntries(bytes);

  const containerEntry = findEntry(entries, CONTAINER_PATH);
  if (!containerEntry) fail(BOOK_DAMAGED);
  const packagePath = parseContainer(
    decodeText(await readEntry(bytes, containerEntry)),
    parseMarkup,
  );

  const packageEntry = findEntry(entries, packagePath);
  if (!packageEntry) fail(BOOK_DAMAGED);
  const parsed = parsePackage(
    decodeText(await readEntry(bytes, packageEntry)),
    packagePath,
    parseMarkup,
  );
  const order = parsed.readingOrder;
  if (order.length === 0) fail(BOOK_EMPTY);

  const declaredTypes = new Map();
  for (const item of parsed.manifest.values()) declaredTypes.set(item.path, item.mediaType);
  const imageUrls = new Map();

  async function imageUrlFor(path) {
    if (imageUrls.has(path)) return imageUrls.get(path);
    const entry = findEntry(entries, path);
    if (!entry) return "";
    const type = imageTypeFor(path, declaredTypes.get(path) || "");
    if (!type) return "";
    const url = createObjectURL(new Blob([await readEntry(bytes, entry)], { type }));
    imageUrls.set(path, url);
    return url;
  }

  return {
    opfPath: packagePath,
    pageProgressionDirection: parsed.pageProgressionDirection,
    chapterCount: order.length,
    chapters: order.map((item) => ({ path: item.path, mediaType: item.mediaType })),

    async readChapter(index) {
      if (!Number.isSafeInteger(index) || index < 0 || index >= order.length) fail(NO_SUCH_CHAPTER);
      const item = order[index];
      const entry = findEntry(entries, item.path);
      if (!entry) fail(BOOK_DAMAGED);
      const source = parseChapterMarkup(
        decodeText(await readEntry(bytes, entry)),
        parseMarkup,
      );
      const baseDir = directoryOf(item.path);

      // Images are pulled out of the archive first so the rebuild below stays a
      // single pass with no waiting in the middle of it.
      const resolvedImages = new Map();
      for (const image of elementsNamed(source, "img")) {
        const declared = String(attributeOf(image, "src") || "").trim();
        if (!declared || resolvedImages.has(declared)) continue;
        // Anything carrying a scheme or an authority points off the book.
        if (/^[A-Za-z][A-Za-z0-9+.-]*:/.test(declared) || declared.startsWith("//")) {
          resolvedImages.set(declared, "");
          continue;
        }
        const path = resolvePath(baseDir, declared);
        resolvedImages.set(declared, path ? await imageUrlFor(path) : "");
      }

      const fragment = sanitizeChapter(source, {
        document,
        resolveImage: (declared) => resolvedImages.get(String(declared).trim()) || "",
      });
      return { path: item.path, fragment };
    },

    release() {
      for (const url of imageUrls.values()) revokeObjectURL(url);
      imageUrls.clear();
    },
  };
}
