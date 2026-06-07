export function escapeHtml(value) {
  return String(value || "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

export function shortUri(uri) {
  if (!uri) return "";
  if (uri.length <= 44) return uri;
  return uri.slice(0, 24) + "..." + uri.slice(-14);
}

export function baseName(uri) {
  const clean = String(uri || "").replace(/\/+$/, "");
  return clean.split("/").pop() || "Library";
}

export function parentUri(uri) {
  const clean = String(uri || "").replace(/\/+$/, "");
  const index = clean.lastIndexOf("/");
  return index > "localhost://".length ? clean.slice(0, index) : clean;
}

export function childUri(parent, name) {
  return String(parent || "").replace(/\/+$/, "") + "/" + encodeURIComponent(name).replace(/%20/g, " ");
}

export function nextObjectName(objects, base, extension = "") {
  const existing = new Set(objects.map((object) => String(object.name || "").toLowerCase()));
  const initial = `${base}${extension}`;
  if (!existing.has(initial.toLowerCase())) return initial;
  for (let index = 2; index < 1000; index += 1) {
    const candidate = `${base} ${index}${extension}`;
    if (!existing.has(candidate.toLowerCase())) return candidate;
  }
  return `${base} ${Date.now()}${extension}`;
}

export function isDirectory(object) {
  return object && object.kind === "directory";
}

export function inTrash(object) {
  return isTrashUri(object?.uri);
}

export function isTrashUri(uri) {
  const value = String(uri || "");
  return value.includes("/.Trash/");
}

export function isTrashRootUri(uri) {
  return String(uri || "").endsWith("/.Trash");
}

export function isBlockedObject(object) {
  return !!object?.blocked_reason;
}

export function hasCapability(object, capability) {
  const capabilities = object && object.capabilities;
  return !Array.isArray(capabilities) || capabilities.includes(capability);
}

export function isWebSpaceUri(uri) {
  const value = String(uri || "").replace(/\/+$/, "");
  return value === "localhost://WebSpaces" || value.startsWith("localhost://WebSpaces/");
}

export function viewerOptions(object) {
  return Array.isArray(object && object.viewers) ? object.viewers : [];
}

export function isArchiveObject(object) {
  if (!object || isDirectory(object)) return false;
  const capabilities = Array.isArray(object.capabilities) ? object.capabilities : [];
  const archiveSupport = object.metadata?.archive_support;
  const name = String(object.name || object.uri || "").toLowerCase();
  const mime = String(object.mime || "").toLowerCase();
  return viewerOptions(object).some((viewer) => viewer?.id === "archive-manager") ||
    !!archiveSupport ||
    capabilities.includes("extract_archive") ||
    isArchiveName(name) ||
    ["application/zip", "application/x-tar", "application/gzip", "application/x-7z-compressed"].includes(mime);
}

export function archiveLibraryObjectPayload(object) {
  const payload = {
    uri: object?.uri || "",
    name: object?.name || "",
    mime: object?.mime || "application/octet-stream",
  };
  const localCid = contentCid(object);
  if (localCid) payload.contentCid = localCid;
  if (object?.metadata?.archive_support) payload.archiveSupport = object.metadata.archive_support;
  return payload;
}

export function contentCid(object) {
  return String(object?.content_cid || "").trim();
}

export function publishedCid(object) {
  return String(object?.published_cid || "").trim();
}

export function visibilityContract(object) {
  const visibility = object?.metadata?.visibility;
  if (visibility && typeof visibility === "object") return visibility;
  return {
    schema: "elastos.library.visibility/v1",
    placement: inTrash(object) ? "trash" : "private_folder",
    placement_label: inTrash(object) ? "Trash" : "Private folder",
    effective_access: object?.published ? "public_content_link" : "principal_private",
    published: !!object?.published,
    published_cid: publishedCid(object) || null,
    published_link: publishedCid(object) ? `elastos://${publishedCid(object)}` : null,
    shared: !!object?.shared,
    share_policy: object?.shared ? "shared" : "not_shared",
    public_folder_policy: "placement_only",
    publish_required_for_public_link: !isDirectory(object) && !object?.published,
  };
}

export function previewKind(object) {
  if (!object || isDirectory(object)) return "";
  const mime = String(object.mime || "");
  const name = String(object.name || "").toLowerCase();
  if (mime.startsWith("image/")) return "image";
  if (mime.startsWith("video/")) return "video";
  if (mime.startsWith("audio/")) return "audio";
  if (mime === "application/pdf" || name.endsWith(".pdf")) return "pdf";
  if (
    mime.startsWith("text/") ||
    [
      ".txt",
      ".md",
      ".json",
      ".csv",
      ".log",
      ".html",
      ".css",
      ".js",
      ".ts",
      ".rs",
      ".toml",
      ".yaml",
      ".yml",
    ].some((ext) => name.endsWith(ext))
  ) {
    return "text";
  }
  return "";
}

export function canPreviewObject(object) {
  return !!previewKind(object);
}

export function iconFor(object) {
  if (inTrash(object)) return "icons/trash.svg";
  if (isDirectory(object)) return "icons/folder.svg";
  const mime = String(object.mime || "");
  const name = String(object.name || "").toLowerCase();
  if (mime.startsWith("image/")) return "icons/file-image.svg";
  if (mime.startsWith("video/")) return "icons/file-video.svg";
  if (mime.startsWith("audio/")) return "icons/file-audio.svg";
  if (mime.includes("pdf")) return "icons/file-pdf.svg";
  if (isArchiveName(name)) return "icons/file-zip.svg";
  if (name.endsWith(".json")) return "icons/file-json.svg";
  if (name.endsWith(".md")) return "icons/file-md.svg";
  if (mime.startsWith("text/")) return "icons/file-text.svg";
  return "icons/file.svg";
}

function isArchiveName(name) {
  return [
    ".zip",
    ".tar",
    ".tar.gz",
    ".tgz",
    ".tar.xz",
    ".txz",
    ".tar.bz2",
    ".tbz2",
    ".tar.zst",
    ".tzst",
    ".7z",
    ".rar",
    ".xz",
    ".bz2",
    ".zst",
    ".lz4",
    ".gz",
  ].some((extension) => name.endsWith(extension));
}

export function formatBytes(bytes) {
  const value = Number(bytes || 0);
  if (!value) return "-";
  const units = ["B", "KB", "MB", "GB"];
  let size = value;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${size.toFixed(size >= 10 || unit === 0 ? 0 : 1)} ${units[unit]}`;
}

export function formatTime(timestamp) {
  const value = Number(timestamp || 0);
  if (!value) return "-";
  return new Date(value * 1000).toLocaleString();
}

export function fileToBase64(file, onProgress) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error("Could not read file."));
    reader.onprogress = (event) => {
      if (event.lengthComputable && typeof onProgress === "function") {
        onProgress(event.loaded / Math.max(event.total, 1));
      }
    };
    reader.onload = () => {
      const dataUrl = String(reader.result || "");
      resolve(dataUrl.split(",", 2)[1] || "");
    };
    reader.readAsDataURL(file);
  });
}

export function base64ToBlob(data, mime) {
  const binary = atob(data || "");
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return new Blob([bytes], { type: mime || "application/octet-stream" });
}
