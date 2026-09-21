// Draws one decrypted file on the page.
//
// Every entry point here takes bytes that are already in hand and returns a
// view: a `release()` that gives back everything the view is holding, and, for
// anything read a page or a chapter at a time, a `pager` the page bar drives.
// Nothing here fetches; nothing here is given a way to.
//
// The two big renderers load their library only when a file of that kind is
// actually opened, so opening a picture never pays for the document or the
// model code.

import { createPager, openComic } from "./cbz.js";
import { openBook } from "./epub.js";

const PDFJS_VERSION = "pdfjs-4.10.38";
const THREE_VERSION = "three-0.160.0";

/** Types drawn as a single picture. Drawings are deliberately absent: see below. */
const IMAGE_TYPES = new Set([
  "image/avif",
  "image/bmp",
  "image/gif",
  "image/jpeg",
  "image/png",
  "image/webp",
]);

/** Types shown as words. */
const TEXT_TYPES = new Set([
  "application/json",
  "text/csv",
  "text/markdown",
  "text/plain",
]);

/** Types shown as words in a fixed-width column, without wrapping. */
const CODE_TYPES = new Set(["application/json"]);

const MODEL_TYPES = new Set([
  "model/gltf-binary",
  "model/gltf+json",
  "model/obj",
  "model/stl",
]);

const COMIC_TYPES = new Set(["application/vnd.comicbook+zip", "application/x-cbz"]);

const BOOK_TYPE = "application/epub+zip";
const PDF_TYPE = "application/pdf";

/** Most characters `renderText` will put on the page at once. */
export const MAX_TEXT_CHARACTERS = 1_000_000;
/** Widest a page canvas is drawn, in device pixels. */
export const MAX_PAGE_CANVAS_PIXELS = 2400;

const NOT_TEXT = "This file is not text this reader can show.";
const NOT_A_DOCUMENT = "This document could not be opened.";
const NOT_A_MODEL = "This model could not be opened.";
const NO_3D = "This device cannot show 3D.";
const MODEL_REACHES_OUT = "This model needs files it does not carry, so it cannot be shown.";

function fail(message) {
  throw new Error(message);
}

/**
 * Normalises a declared content type to the bare type, lowercased. Parameters
 * such as `; charset=utf-8` are dropped for the routing decision only — they
 * are never used to pick a different renderer.
 */
export function baseContentType(contentType) {
  return String(contentType || "")
    .split(";")[0]
    .trim()
    .toLowerCase();
}

/**
 * The kind of view a content type gets, or `""` when this reader has none.
 *
 * There is one arm per kind and no catch-all: a type this reader does not know
 * gets the honest empty answer rather than being folded into the nearest
 * neighbour. `text/x-…` is the one prefix rule, because that whole space is
 * source text by definition.
 */
export function kindFor(contentType) {
  const type = baseContentType(contentType);
  if (IMAGE_TYPES.has(type)) return "image";
  if (type === PDF_TYPE) return "document";
  if (TEXT_TYPES.has(type) || type.startsWith("text/x-")) return "text";
  if (MODEL_TYPES.has(type)) return "model";
  if (type === BOOK_TYPE) return "book";
  if (COMIC_TYPES.has(type)) return "comic";
  return "";
}

/** True when a text file is shown fixed-width and unwrapped. */
export function isCodeText(contentType) {
  const type = baseContentType(contentType);
  return CODE_TYPES.has(type) || type.startsWith("text/x-");
}

/** Plain words for the kind of file on screen. */
export function describeKind(kind) {
  if (kind === "image") return "Picture";
  if (kind === "document") return "Document";
  if (kind === "text") return "Text";
  if (kind === "model") return "3D model";
  if (kind === "book") return "Book";
  if (kind === "comic") return "Comic";
  return "File";
}

function clear(stage) {
  while (stage.firstChild) stage.removeChild(stage.firstChild);
}

function addElement(parent, tag, className) {
  const element = parent.ownerDocument.createElement(tag);
  if (className) element.className = className;
  parent.appendChild(element);
  return element;
}

function addMessage(stage, className, lines) {
  const panel = addElement(stage, "div", className);
  for (const line of lines) {
    addElement(panel, "p", "").textContent = line;
  }
  return panel;
}

function decodeText(bytes) {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return fail(NOT_TEXT);
  }
}

// ---------------------------------------------------------------------------
// Pictures
// ---------------------------------------------------------------------------

/**
 * Shows one picture.
 *
 * The source is minted with the type the file was published as, and only ever
 * from the raster list above. A drawing file is markup, and a source minted
 * over one could be opened on its own and would then run that markup with this
 * page's own standing, so `kindFor` never routes a drawing here in the first
 * place.
 */
export function renderImage({ stage, bytes, contentType, urlObject = URL }) {
  const type = baseContentType(contentType);
  if (!IMAGE_TYPES.has(type)) fail("This picture could not be opened.");
  clear(stage);
  const url = urlObject.createObjectURL(new Blob([bytes], { type }));
  const frame = addElement(stage, "div", "view view-image");
  const image = addElement(frame, "img", "image-sheet");
  image.alt = "";
  image.decoding = "async";
  image.src = url;
  return {
    release() {
      image.removeAttribute("src");
      urlObject.revokeObjectURL(url);
    },
  };
}

// ---------------------------------------------------------------------------
// Words
// ---------------------------------------------------------------------------

/** Shows a text file, fixed-width when it is source text. */
export function renderText({ stage, bytes, contentType }) {
  const decoded = decodeText(bytes);
  const shown = decoded.length > MAX_TEXT_CHARACTERS ? decoded.slice(0, MAX_TEXT_CHARACTERS) : decoded;
  clear(stage);
  const frame = addElement(stage, "div", "view view-text");
  const body = addElement(frame, "pre", "text-sheet");
  body.dataset.code = isCodeText(contentType) ? "true" : "false";
  body.textContent = shown;
  if (shown.length < decoded.length) {
    addElement(frame, "p", "view-note").textContent =
      `Showing the first ${MAX_TEXT_CHARACTERS.toLocaleString("en-US")} characters of this file.`;
  }
  return { release() {} };
}

// ---------------------------------------------------------------------------
// Documents
// ---------------------------------------------------------------------------

async function loadPdfLibrary() {
  const library = await import(`./vendor/pdfjs/pdf.min.mjs?v=${PDFJS_VERSION}`);
  // The reading half is handed to the drawing half on this thread, before any
  // document is opened. This page runs at an origin of its own that matches
  // nothing, where a background thread cannot be started from a file address
  // and cannot be started from a blob address either, so the library is told
  // outright where its reading half already is and never tries. See
  // `vendor/pdfjs/README.md`.
  if (!globalThis.pdfjsWorker) {
    const { WorkerMessageHandler } = await import(
      `./vendor/pdfjs/pdf.worker.min.mjs?v=${PDFJS_VERSION}`
    );
    globalThis.pdfjsWorker = { WorkerMessageHandler };
  }
  library.GlobalWorkerOptions.workerSrc = `./vendor/pdfjs/pdf.worker.min.mjs?v=${PDFJS_VERSION}`;
  return library;
}

/** Shows a document one page at a time. */
export async function renderPdf({ stage, bytes, setStatus = () => {}, loadLibrary = loadPdfLibrary }) {
  const library = await loadLibrary();
  let document_ = null;
  try {
    document_ = await library.getDocument({
      // A copy: the library takes the buffer over and leaves the original
      // empty, and the reader still needs its bytes if the view is rebuilt.
      data: bytes.slice(),
      isEvalSupported: false,
      disableAutoFetch: true,
      disableStream: true,
      // No character maps and no substitute font set are carried, so the
      // library is told plainly not to look for them.
      useSystemFonts: true,
    }).promise;
  } catch {
    fail(NOT_A_DOCUMENT);
  }
  const count = Number(document_.numPages);
  if (!Number.isSafeInteger(count) || count < 1) {
    document_.destroy?.();
    fail(NOT_A_DOCUMENT);
  }
  clear(stage);
  const frame = addElement(stage, "div", "view view-paged");
  const canvas = addElement(frame, "canvas", "page-sheet");
  const context = canvas.getContext("2d");
  if (!context) {
    document_.destroy?.();
    fail(NOT_A_DOCUMENT);
  }
  let task = null;

  async function show(index) {
    setStatus(`Drawing page ${index + 1} of ${count}...`);
    task?.cancel?.();
    task = null;
    const page = await document_.getPage(index + 1);
    const natural = page.getViewport({ scale: 1 });
    const scale = Math.min(2, MAX_PAGE_CANVAS_PIXELS / Math.max(natural.width, natural.height));
    const viewport = page.getViewport({ scale: scale > 0 ? scale : 1 });
    canvas.width = Math.max(1, Math.floor(viewport.width));
    canvas.height = Math.max(1, Math.floor(viewport.height));
    task = page.render({ canvasContext: context, viewport });
    await task.promise;
    task = null;
    page.cleanup?.();
    setStatus("");
  }

  return {
    pager: { count, unit: "Page", show },
    release() {
      task?.cancel?.();
      task = null;
      document_.destroy?.();
    },
  };
}

// ---------------------------------------------------------------------------
// Comics
// ---------------------------------------------------------------------------

/** Shows a comic one page at a time, in reading order. */
export async function renderComic({ stage, bytes, urlObject = URL }) {
  const comic = await openComic(bytes);
  clear(stage);
  const frame = addElement(stage, "div", "view view-paged");
  const image = addElement(frame, "img", "page-sheet");
  image.alt = "";
  image.decoding = "async";
  let url = "";

  function drop() {
    if (!url) return;
    image.removeAttribute("src");
    urlObject.revokeObjectURL(url);
    url = "";
  }

  async function show(index) {
    const bytesOfPage = await comic.readPage(index);
    drop();
    url = urlObject.createObjectURL(
      new Blob([bytesOfPage], { type: comic.pages[index].contentType }),
    );
    image.src = url;
  }

  return {
    pager: { count: comic.pageCount, unit: "Page", show },
    release: drop,
  };
}

// ---------------------------------------------------------------------------
// Books
// ---------------------------------------------------------------------------

/**
 * Shows a book one chapter at a time.
 *
 * A chapter arrives as nodes and is put on the page as nodes. It is never
 * turned back into markup on the way in, and never given a frame of its own:
 * turning it back into markup would undo the whole point of rebuilding it from
 * a fixed list of tags in the first place.
 */
export async function renderBook({ stage, bytes }) {
  const book = await openBook(bytes);
  clear(stage);
  const frame = addElement(stage, "div", "view view-book");
  const article = addElement(frame, "article", "book-sheet");
  // The book says which way its pages run, and the page is laid out that way:
  // a right-to-left book reads right to left, and its Previous and Next swap
  // sides with it so Next is always the way the book itself goes on.
  const rightToLeft = book.pageProgressionDirection === "rtl";
  article.dir = rightToLeft ? "rtl" : "ltr";

  async function show(index) {
    // Order matters. The chapter on screen comes off the page first, then the
    // sources its pictures were holding are given back, and only then is the
    // next chapter read: releasing while the old chapter is still on the page
    // would blank its pictures, and releasing after the read would give back
    // the new chapter's sources along with the old ones.
    while (article.firstChild) article.removeChild(article.firstChild);
    book.release();
    const chapter = await book.readChapter(index);
    article.appendChild(chapter.fragment);
    frame.scrollTop = 0;
  }

  return {
    pager: { count: book.chapterCount, unit: "Chapter", show, rightToLeft },
    release() {
      book.release();
    },
  };
}

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

const GLB_MAGIC = 0x46546c67;
const GLB_JSON_CHUNK = 0x4e4f534a;

/**
 * Pulls the description out of a packed model file.
 *
 * A packed model is a small header, then a run of chunks; the first chunk is
 * the description and the rest is the mesh data. Only the description is read
 * here, and only so the references in it can be checked.
 */
export function packedModelDescription(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (bytes.byteLength < 20 || view.getUint32(0, true) !== GLB_MAGIC) fail(NOT_A_MODEL);
  const declared = view.getUint32(8, true);
  if (declared > bytes.byteLength) fail(NOT_A_MODEL);
  let at = 12;
  while (at + 8 <= declared) {
    const length = view.getUint32(at, true);
    const kind = view.getUint32(at + 4, true);
    const start = at + 8;
    if (length > declared - start) fail(NOT_A_MODEL);
    if (kind === GLB_JSON_CHUNK) {
      return decodeText(bytes.subarray(start, start + length));
    }
    at = start + length + ((4 - (length % 4)) % 4);
  }
  return fail(NOT_A_MODEL);
}

/**
 * Refuses a model that names a file it does not carry.
 *
 * A model may point its mesh data or its pictures at another address. This
 * reader has one file and no way to fetch a second one, and a model that
 * reached out would be asking the page to talk to somewhere it must not, so a
 * reference that is not carried inside the file itself is refused outright
 * rather than left to fail as a blocked request.
 */
export function requireSelfContainedModel(description) {
  let parsed = null;
  try {
    parsed = JSON.parse(description);
  } catch {
    fail(NOT_A_MODEL);
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) fail(NOT_A_MODEL);
  for (const key of ["buffers", "images"]) {
    for (const item of Array.isArray(parsed[key]) ? parsed[key] : []) {
      const uri = item && typeof item.uri === "string" ? item.uri.trim() : "";
      if (uri && !uri.toLowerCase().startsWith("data:")) fail(MODEL_REACHES_OUT);
    }
  }
  return parsed;
}

async function loadThreeParts(contentType) {
  const three = await import("three");
  const type = baseContentType(contentType);
  if (type === "model/gltf-binary" || type === "model/gltf+json") {
    const { GLTFLoader } = await import(`./vendor/three/loaders/GLTFLoader.js?v=${THREE_VERSION}`);
    return { three, Loader: GLTFLoader };
  }
  if (type === "model/obj") {
    const { OBJLoader } = await import(`./vendor/three/loaders/OBJLoader.js?v=${THREE_VERSION}`);
    return { three, Loader: OBJLoader };
  }
  const { STLLoader } = await import(`./vendor/three/loaders/STLLoader.js?v=${THREE_VERSION}`);
  return { three, Loader: STLLoader };
}

async function parseModel(three, Loader, contentType, bytes) {
  const type = baseContentType(contentType);
  if (type === "model/stl") {
    const geometry = new Loader().parse(
      bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength),
    );
    const mesh = new three.Mesh(
      geometry,
      new three.MeshStandardMaterial({ color: 0xc8ccd6, metalness: 0.1, roughness: 0.8 }),
    );
    const group = new three.Group();
    group.add(mesh);
    return group;
  }
  if (type === "model/obj") {
    return new Loader().parse(decodeText(bytes));
  }
  const description =
    type === "model/gltf-binary" ? packedModelDescription(bytes) : decodeText(bytes);
  requireSelfContainedModel(description);
  const loader = new Loader();
  const parsed = await new Promise((resolve, reject) => {
    const source =
      type === "model/gltf-binary"
        ? bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength)
        : description;
    loader.parse(source, "", resolve, () => reject(new Error(NOT_A_MODEL)));
  });
  return parsed.scene;
}

/** Shows a 3D model the viewer can turn. */
export async function renderModel({ stage, bytes, contentType, loadParts = loadThreeParts }) {
  if (!MODEL_TYPES.has(baseContentType(contentType))) fail(NOT_A_MODEL);
  const probe = stage.ownerDocument.createElement("canvas");
  if (!probe.getContext("webgl2") && !probe.getContext("webgl")) fail(NO_3D);
  const { three, Loader } = await loadParts(contentType);
  const { OrbitControls } = await import(
    `./vendor/three/controls/OrbitControls.js?v=${THREE_VERSION}`
  );
  let object = null;
  try {
    object = await parseModel(three, Loader, contentType, bytes);
  } catch (error) {
    fail(error instanceof Error && error.message === MODEL_REACHES_OUT ? MODEL_REACHES_OUT : NOT_A_MODEL);
  }
  clear(stage);
  const frame = addElement(stage, "div", "view view-model");

  const scene = new three.Scene();
  scene.background = new three.Color(0x14171f);
  scene.add(new three.HemisphereLight(0xffffff, 0x404050, 2.2));
  const key = new three.DirectionalLight(0xffffff, 1.4);
  key.position.set(3, 5, 4);
  scene.add(key);
  scene.add(object);

  const box = new three.Box3().setFromObject(object);
  const size = box.getSize(new three.Vector3());
  const centre = box.getCenter(new three.Vector3());
  const reach = Math.max(size.x, size.y, size.z) || 1;
  const camera = new three.PerspectiveCamera(45, 1, reach / 1000, reach * 1000);
  camera.position.set(centre.x + reach, centre.y + reach * 0.6, centre.z + reach * 1.6);

  const renderer = new three.WebGLRenderer({ antialias: true, alpha: false });
  renderer.setPixelRatio(Math.min(2, stage.ownerDocument.defaultView?.devicePixelRatio || 1));
  frame.appendChild(renderer.domElement);
  renderer.domElement.className = "model-sheet";

  const controls = new OrbitControls(camera, renderer.domElement);
  controls.target.copy(centre);
  controls.enableDamping = true;
  controls.update();

  let running = true;
  function resize() {
    const width = Math.max(1, frame.clientWidth || 1);
    const height = Math.max(1, frame.clientHeight || 1);
    camera.aspect = width / height;
    camera.updateProjectionMatrix();
    renderer.setSize(width, height, false);
  }
  function frameLoop() {
    if (!running) return;
    resize();
    controls.update();
    renderer.render(scene, camera);
    renderer.domElement.ownerDocument.defaultView?.requestAnimationFrame(frameLoop);
  }
  frameLoop();

  return {
    release() {
      running = false;
      controls.dispose?.();
      scene.traverse((node) => {
        node.geometry?.dispose?.();
        const material = node.material;
        for (const one of Array.isArray(material) ? material : [material]) {
          one?.map?.dispose?.();
          one?.dispose?.();
        }
      });
      renderer.dispose?.();
      renderer.forceContextLoss?.();
    },
  };
}

// ---------------------------------------------------------------------------
// No renderer
// ---------------------------------------------------------------------------

/**
 * Says plainly that this reader has nothing that can show this file, and names
 * the type so the answer is checkable. It does not offer a download, because
 * the file is protected and there is nothing behind such an offer.
 */
export function renderUnsupported({ stage, contentType }) {
  clear(stage);
  const type = baseContentType(contentType);
  addMessage(stage, "view view-empty", [
    "This reader has no way to show this file.",
    type ? `The file is a ${type} file.` : "The file did not say what it is.",
    "You still own it. A later version of this reader may be able to show it.",
  ]);
  return { release() {} };
}

const RENDERERS = new Map([
  ["image", renderImage],
  ["document", renderPdf],
  ["text", renderText],
  ["model", renderModel],
  ["book", renderBook],
  ["comic", renderComic],
]);

/**
 * The renderer for a content type, or the honest empty state. There is one
 * entry per kind and no shared arm: a type this reader does not know never
 * lands in another type's renderer.
 */
export function rendererFor(contentType) {
  return RENDERERS.get(kindFor(contentType)) || renderUnsupported;
}
