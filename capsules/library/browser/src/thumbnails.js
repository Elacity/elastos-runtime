// Lazy image thumbnails for Library items.
//
// Runtime models `thumbnail_uri` on objects but does not fill it yet. Until it
// does, Library renders a thumbnail from the image bytes it is already allowed
// to download. Only items on screen are fetched, a few at a time, and object
// URLs are released when the cache is full.

const THUMBNAIL_MAX_BYTES = 4 * 1024 * 1024;
const THUMBNAIL_CACHE_LIMIT = 160;
const THUMBNAIL_CONCURRENCY = 3;
const THUMBNAIL_ROOT_MARGIN = "200px";

export function isThumbnailCandidate(object) {
  if (!object || object.kind === "directory") return false;
  if (!String(object.mime || "").startsWith("image/")) return false;
  return Number(object.size || 0) <= THUMBNAIL_MAX_BYTES;
}

export function thumbnailSignature(object) {
  return [object.uri, object.revision || "", object.modified_at || 0, object.size || 0].join("\u0000");
}

export function createLibraryThumbnails({ readObjectBlob, root }) {
  const cache = new Map();
  const pending = new Map();
  const targets = new WeakMap();
  const queue = [];
  let active = 0;
  const observer = typeof IntersectionObserver === "function"
    ? new IntersectionObserver(onIntersect, { root, rootMargin: THUMBNAIL_ROOT_MARGIN })
    : null;

  function observe(node, object) {
    const signature = thumbnailSignature(object);
    targets.set(node, { object, signature });
    const cached = cache.get(signature);
    if (cached) {
      touch(signature, cached);
      applyThumbnail(node, signature, cached.objectUrl);
      return;
    }
    if (observer) {
      observer.observe(node);
      return;
    }
    enqueue(node);
  }

  function onIntersect(entries) {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      observer.unobserve(entry.target);
      enqueue(entry.target);
    }
  }

  function enqueue(node) {
    const target = targets.get(node);
    if (!target) return;
    const cached = cache.get(target.signature);
    if (cached) {
      applyThumbnail(node, target.signature, cached.objectUrl);
      return;
    }
    if (pending.has(target.signature)) {
      pending.get(target.signature).push(node);
      return;
    }
    pending.set(target.signature, [node]);
    queue.push(target);
    drain();
  }

  function drain() {
    while (active < THUMBNAIL_CONCURRENCY && queue.length) {
      const job = queue.shift();
      active += 1;
      void load(job).finally(() => {
        active -= 1;
        drain();
      });
    }
  }

  async function load({ object, signature }) {
    const nodes = pending.get(signature) || [];
    let objectUrl = "";
    try {
      objectUrl = URL.createObjectURL(await readObjectBlob(object.thumbnail_uri || object.uri));
    } catch (_error) {
      // Keep the generic file icon; the object may be unreadable or offline.
      pending.delete(signature);
      return;
    }
    pending.delete(signature);
    remember(signature, objectUrl);
    for (const node of nodes) {
      applyThumbnail(node, signature, objectUrl);
    }
  }

  function applyThumbnail(node, signature, objectUrl) {
    if (!node.isConnected || targets.get(node)?.signature !== signature) return;
    const img = node.querySelector(".file-icon img");
    if (!img || img.dataset.thumbApplied === objectUrl) return;
    img.dataset.thumbApplied = objectUrl;
    img.src = objectUrl;
    node.classList.add("item--thumbnail");
  }

  function remember(signature, objectUrl) {
    cache.set(signature, { objectUrl });
    while (cache.size > THUMBNAIL_CACHE_LIMIT) {
      const [oldest, entry] = cache.entries().next().value;
      cache.delete(oldest);
      URL.revokeObjectURL(entry.objectUrl);
    }
  }

  function touch(signature, entry) {
    cache.delete(signature);
    cache.set(signature, entry);
  }

  function release() {
    observer?.disconnect();
    for (const entry of cache.values()) URL.revokeObjectURL(entry.objectUrl);
    cache.clear();
    pending.clear();
    queue.length = 0;
  }

  return { observe, release };
}
