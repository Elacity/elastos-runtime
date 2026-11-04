// Home owns the authenticated read. Each opaque frame owns its Blob URL.
export function createHomeBackgroundImage({ homeOrigin, homeToken, apply, onError = () => {} }) {
  let key = null;
  let pending = null;
  let objectUrl = "";

  function replace(url) {
    const previous = objectUrl;
    objectUrl = url;
    apply(url);
    if (previous) URL.revokeObjectURL(previous);
  }

  function cancel() {
    if (pending) clearTimeout(pending.timer);
    pending = null;
  }

  function fail() {
    cancel();
    key = ""; // A later summary can retry the read.
    replace("");
    onError();
  }

  window.addEventListener("message", (event) => {
    const data = event.data;
    if (event.source !== window.top || event.origin !== homeOrigin
      || data?.type !== "home:background-image-result"
      || !pending || data.requestId !== pending.id) return;
    const blob = data.blob;
    if (!(blob instanceof Blob) || !blob.size || blob.size > 5 * 1024 * 1024
      || !["image/png", "image/jpeg", "image/webp", "image/gif"].includes(blob.type)) {
      fail();
      return;
    }
    cancel();
    replace(URL.createObjectURL(blob));
  });

  function update(imageKey) {
    const next = typeof imageKey === "string" ? imageKey : "";
    if (next === key) return;
    cancel();
    key = next;
    replace("");
    if (!key) return;
    const id = window.crypto.randomUUID();
    pending = { id, timer: setTimeout(fail, 15_000) };
    window.top.postMessage({ type: "home:background-image", requestId: id,
      homeToken }, homeOrigin);
  }

  window.addEventListener("pagehide", () => { cancel(); key = ""; replace(""); });
  return { update };
}
