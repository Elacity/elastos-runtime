#!/usr/bin/env node
import http from "node:http";
import { createReadStream, existsSync } from "node:fs";
import { stat } from "node:fs/promises";
import path from "node:path";
import { createRequire } from "node:module";

const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");
const multiSelectModifier = process.platform === "darwin" ? "Meta" : "Control";

function browserAssetRoot(capsuleName) {
  const capsuleRoot = path.resolve("capsules", capsuleName);
  const browserRoot = path.join(capsuleRoot, "browser");
  return existsSync(browserRoot) ? browserRoot : capsuleRoot;
}

const capsuleRoot = browserAssetRoot("library");
const archiveManagerRoot = browserAssetRoot("archive-manager");
const token = "library-menu-smoke-token";
const principalRoot = "localhost://Users/smoke";
const desktopUri = `${principalRoot}/Desktop`;
const documentsUri = `${principalRoot}/Documents`;
const picturesUri = `${principalRoot}/Pictures`;
const videosUri = `${principalRoot}/Videos`;
const downloadsUri = `${principalRoot}/Downloads`;
const publicUri = `${principalRoot}/Public`;
const webspacesUri = "localhost://WebSpaces";
const webspaceMutableUri = `${webspacesUri}/Mutable`;
const ops = [];
const uploadSessions = new Map();
let revisionCounter = 10;

const SMOKE_LOCAL_CONTENT_CID = "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku";
const SMOKE_PUBLISHED_CID = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";

function objectVisibility(uri, kind, extra = {}) {
  const publishedCid = extra.published_cid || null;
  const inTrash = uri.includes("/.Trash");
  const inPublic = uri === publicUri || uri.startsWith(`${publicUri}/`);
  const placement = inTrash ? "trash" : inPublic ? "public_folder" : "private_folder";
  return {
    schema: "elastos.library.visibility/v1",
    placement,
    placement_label: placement === "public_folder" ? "Public folder" : placement === "trash" ? "Trash" : "Private folder",
    effective_access: publishedCid ? "public_content_link" : "principal_private",
    published: !!publishedCid,
    published_cid: publishedCid,
    published_link: publishedCid ? `elastos://${publishedCid}` : null,
    shared: !!extra.shared,
    share_policy: extra.shared ? "public_link" : "not_shared",
    public_folder_policy: "placement_only",
    publish_required_for_public_link: kind === "file" && !publishedCid,
    note: "Public folder placement is a user-facing Library projection. Public network access requires an explicit content-provider publish receipt.",
  };
}

function object(uri, name, kind, capabilities, extra = {}) {
  const metadata = extra.metadata
    ? { ...extra.metadata, visibility: extra.metadata.visibility || objectVisibility(uri, kind, extra) }
    : { schema: "elastos.library.object-metadata/v1", visibility: objectVisibility(uri, kind, extra) };
  return {
    schema: "elastos.library.object/v1",
    uri,
    name,
    kind,
    mime: kind === "directory" ? "inode/directory" : (extra.mime || "text/plain"),
    size: extra.size || 128,
    created_at: 1780000000,
    modified_at: 1780000000,
    revision: extra.revision || `rev:${name}`,
    viewer: null,
    viewers: extra.viewers || [],
    thumbnail_uri: null,
    availability: extra.availability || "local-only",
    blocked_reason: extra.blocked_reason || null,
    content_cid: extra.content_cid || (kind === "file" ? SMOKE_LOCAL_CONTENT_CID : null),
    published_cid: extra.published_cid || null,
    metadata,
    published: !!extra.published_cid || !!extra.published,
    shared: !!extra.shared,
    capabilities,
  };
}

function archiveSupport(family, status = "extractable") {
  return {
    schema: "elastos.library.archive-support/v1",
    family,
    status,
    implemented: {
      download_formats: ["zip", "tar.gz"],
      compress_to_library: ["zip"],
      extract_formats: ["zip", "tar", "tar.gz", "tgz"],
      safety: "relative UTF-8 file paths only; non-file archive entries are rejected",
    },
    policy_gate: status === "extractable" ? null : {
      required: true,
      reason: "generic archive support needs dependency and release-policy review before enabling",
      blocked_formats: ["7z", "rar", "tar.xz", "tar.bz2", "tar.zst", "xz", "bz2", "zst", "lz4", "gzip"],
    },
  };
}

const roots = [
  { schema: "elastos.library.root/v1", id: "home", label: "Home", uri: principalRoot, kind: "principal-root" },
  { schema: "elastos.library.root/v1", id: "desktop", label: "Desktop", uri: desktopUri, kind: "directory" },
  { schema: "elastos.library.root/v1", id: "documents", label: "Documents", uri: documentsUri, kind: "directory" },
  { schema: "elastos.library.root/v1", id: "pictures", label: "Pictures", uri: picturesUri, kind: "directory" },
  { schema: "elastos.library.root/v1", id: "videos", label: "Videos", uri: videosUri, kind: "directory" },
  { schema: "elastos.library.root/v1", id: "downloads", label: "Downloads", uri: downloadsUri, kind: "directory" },
  { schema: "elastos.library.root/v1", id: "public", label: "Public", uri: publicUri, kind: "directory" },
  {
    schema: "elastos.library.root/v1",
    id: "trash",
    label: "Trash",
    uri: `${principalRoot}/.Trash`,
    kind: "directory",
    metadata: { schema: "elastos.library.trash-root/v1", empty: false, item_count: 2 },
  },
  { schema: "elastos.library.root/v1", id: "webspaces", label: "Spaces", uri: webspacesUri, kind: "webspace-root" },
];

const localhostSpace = object(principalRoot, "Localhost", "directory", [
  "open",
  "list",
  "properties",
], {
  availability: "local-principal",
  metadata: {
    schema: "elastos.library.space-pointer/v1",
    space: "localhost",
    label: "Localhost",
    target_uri: principalRoot,
    provider: "object-provider",
    authority: "signed-principal-root",
    writable: true,
    note: "This opens the signed principal's mutable localhost object space. It is not a broad host filesystem grant.",
  },
});
const folder = object(`${documentsUri}/Projects`, "Projects", "directory", [
  "download",
  "compress_archive",
  "rename",
  "move",
  "copy",
  "trash",
  "properties",
]);
const file = object(`${documentsUri}/Readme.md`, "Readme.md", "file", [
  "download",
  "compress_archive",
  "rename",
  "move",
  "copy",
  "publish",
  "trash",
  "properties",
]);
const viewerFile = object(`${documentsUri}/Viewer.md`, "Viewer.md", "file", [
  "download",
  "compress_archive",
  "rename",
  "move",
  "copy",
  "publish",
  "trash",
  "properties",
], {
  viewers: [{ id: "documents", label: "Documents" }],
});
const gbaFile = object(`${documentsUri}/Game.gba`, "Game.gba", "file", [
  "download",
  "rename",
  "move",
  "copy",
  "trash",
  "properties",
], {
  mime: "application/x-gba-rom",
  viewers: [{ id: "gba-emulator", label: "GBA Emulator", default: true }],
});
const publicDraftFile = object(`${publicUri}/Public Draft.md`, "Public Draft.md", "file", [
  "download",
  "compress_archive",
  "rename",
  "move",
  "copy",
  "publish",
  "trash",
  "properties",
]);
const publishedFile = object(`${documentsUri}/Published.md`, "Published.md", "file", [
  "download",
  "compress_archive",
  "rename",
  "move",
  "copy",
  "unpublish",
  "repair",
  "share",
  "trash",
  "properties",
], {
  published_cid: SMOKE_PUBLISHED_CID,
  availability: "local_pinned",
});
const archiveFile = object(`${documentsUri}/Bundle.tar.gz`, "Bundle.tar.gz", "file", [
  "download",
  "compress_archive",
  "rename",
  "move",
  "copy",
  "extract_archive",
  "trash",
  "properties",
], {
  mime: "application/gzip",
  size: 512,
  viewers: [{ id: "archive-manager", label: "Archive" }],
  metadata: {
    schema: "elastos.library.object-metadata/v1",
    archive_support: archiveSupport("tar.gz"),
  },
});
const tarArchiveFile = object(`${documentsUri}/Plain.tar`, "Plain.tar", "file", [
  "download",
  "compress_archive",
  "rename",
  "move",
  "copy",
  "extract_archive",
  "trash",
  "properties",
], {
  mime: "application/x-tar",
  size: 512,
  viewers: [{ id: "archive-manager", label: "Archive" }],
  metadata: {
    schema: "elastos.library.object-metadata/v1",
    archive_support: archiveSupport("tar"),
  },
});
const zipArchiveFile = object(`${documentsUri}/Portable.zip`, "Portable.zip", "file", [
  "download",
  "compress_archive",
  "rename",
  "move",
  "copy",
  "extract_archive",
  "trash",
  "properties",
], {
  mime: "application/zip",
  size: 512,
  viewers: [{ id: "archive-manager", label: "Archive" }],
  metadata: {
    schema: "elastos.library.object-metadata/v1",
    archive_support: archiveSupport("zip"),
  },
});
const looseZipFile = object(`${documentsUri}/Loose.zip`, "Loose.zip", "file", [
  "download",
  "compress_archive",
  "rename",
  "move",
  "copy",
  "trash",
  "properties",
], {
  mime: "application/zip",
  size: 384,
});
const policyGatedArchiveFile = object(`${documentsUri}/Legacy.7z`, "Legacy.7z", "file", [
  "download",
  "compress_archive",
  "rename",
  "move",
  "copy",
  "trash",
  "properties",
], {
  mime: "application/x-7z-compressed",
  size: 768,
  viewers: [{ id: "archive-manager", label: "Archive" }],
  metadata: {
    schema: "elastos.library.object-metadata/v1",
    archive_support: archiveSupport("7z", "policy_gated_unsupported_archive_family"),
  },
});
const blockedFile = object(`${documentsUri}/secret.md`, "secret.md", "file", ["properties"], {
  blocked_reason: "protected principal-root object is not encrypted",
  availability: "blocked",
});
const hiddenFile = object(`${documentsUri}/.env`, ".env", "file", [
  "download",
  "compress_archive",
  "rename",
  "move",
  "copy",
  "publish",
  "trash",
  "properties",
]);
const trashFile = object(`${principalRoot}/.Trash/Deleted.txt`, "Deleted.txt", "file", [
  "restore",
  "delete_permanently",
  "properties",
], {
  metadata: {
    schema: "elastos.library.object-metadata/v1",
    visibility: {
      schema: "elastos.library.visibility/v1",
      placement: "trash",
      placement_label: "Trash",
      effective_access: "principal_private",
    },
    trash: {
      schema: "elastos.library.trash-record/v1",
      trash_uri: `${principalRoot}/.Trash/Deleted.txt`,
      original_uri: `${documentsUri}/Deleted.txt`,
      original_name: "Deleted.txt",
      trashed_at: 1_780_000_000,
    },
  },
});
const purgeFile = object(`${principalRoot}/.Trash/Purge.txt`, "Purge.txt", "file", [
  "restore",
  "delete_permanently",
  "properties",
], {
  metadata: {
    schema: "elastos.library.object-metadata/v1",
    visibility: {
      schema: "elastos.library.visibility/v1",
      placement: "trash",
      placement_label: "Trash",
      effective_access: "principal_private",
    },
    trash: {
      schema: "elastos.library.trash-record/v1",
      trash_uri: `${principalRoot}/.Trash/Purge.txt`,
      original_uri: `${documentsUri}/Purge.txt`,
      original_name: "Purge.txt",
      trashed_at: 1_780_000_001,
    },
  },
});
const webspaceElastos = object(`${webspacesUri}/Elastos`, "Elastos", "directory", [
  "open",
  "list",
  "properties",
], {
  availability: "resolver-owned",
  metadata: {
    schema: "elastos.library.webspace-object/v1",
    mount: "Elastos",
    target_uri: "elastos://",
    resolver: "builtin",
    webspace_kind: "dynamic-webspace",
    readonly: true,
  },
});
const webspaceContent = object(`${webspacesUri}/Elastos/content`, "content", "directory", [
  "open",
  "list",
  "properties",
], {
  availability: "resolver-owned",
  metadata: {
    schema: "elastos.library.webspace-object/v1",
    mount: "Elastos",
    target_uri: "elastos://content",
    resolver: "builtin",
    webspace_kind: "folder-handle",
    readonly: true,
  },
});
const webspaceFile = object(`${webspacesUri}/Elastos/content/bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi`, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi", "file", [
  "open",
  "read",
  "download",
  "properties",
], {
  availability: "resolver-owned",
  mime: "application/json",
  metadata: {
    schema: "elastos.library.webspace-object/v1",
    mount: "Elastos",
    target_uri: "elastos://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
    resolver: "builtin",
    webspace_kind: "file-endpoint",
    readonly: true,
  },
});
const webspaceCloud = object(`${webspacesUri}/Cloud`, "Cloud", "directory", [
  "open",
  "list",
  "properties",
], {
  availability: "resolver-owned",
  metadata: {
    schema: "elastos.library.webspace-object/v1",
    mount: "Cloud",
    target_uri: "cloud://drive",
    resolver: "cloud-drive",
    webspace_kind: "mounted-webspace",
    readonly: true,
  },
});
const webspaceCloudDrive = object(`${webspacesUri}/Cloud/Drive`, "Drive", "directory", [
  "open",
  "list",
  "properties",
], {
  availability: "resolver-owned",
  metadata: {
    schema: "elastos.library.webspace-object/v1",
    mount: "Cloud",
    target_uri: "cloud://drive/Drive",
    resolver: "cloud-drive",
    webspace_kind: "indexed-directory",
    readonly: true,
  },
});
const webspaceCloudProject = object(`${webspacesUri}/Cloud/Drive/Project X`, "Project X", "directory", [
  "open",
  "list",
  "properties",
], {
  availability: "resolver-owned",
  metadata: {
    schema: "elastos.library.webspace-object/v1",
    mount: "Cloud",
    target_uri: "cloud://drive/Drive/Project X",
    resolver: "cloud-drive",
    webspace_kind: "indexed-directory",
    readonly: true,
  },
});
const webspaceCloudFile = object(`${webspacesUri}/Cloud/Drive/Project X/file.pdf`, "file.pdf", "file", [
  "open",
  "read",
  "download",
  "properties",
], {
  availability: "resolver-owned",
  mime: "application/pdf",
  size: 256,
  metadata: {
    schema: "elastos.library.webspace-object/v1",
    mount: "Cloud",
    target_uri: "cloud://drive/Drive/Project X/file.pdf",
    resolver: "cloud-drive",
    webspace_kind: "indexed-file",
    readonly: true,
  },
});
const webspaceMutable = createWebspaceFolderObject(webspaceMutableUri, "Mutable", {
  targetUri: "local://mutable",
  webspaceKind: "mounted-webspace",
});

const folders = new Map([
  [principalRoot, []],
  [desktopUri, []],
  [documentsUri, [folder, file, viewerFile, gbaFile, publishedFile, archiveFile, tarArchiveFile, zipArchiveFile, looseZipFile, policyGatedArchiveFile, blockedFile, hiddenFile]],
  [picturesUri, []],
  [videosUri, []],
  [downloadsUri, []],
  [publicUri, [publicDraftFile]],
  [`${principalRoot}/.Trash`, [trashFile, purgeFile]],
  [webspacesUri, [localhostSpace, webspaceElastos, webspaceCloud, webspaceMutable]],
  [`${webspacesUri}/Elastos`, [webspaceContent]],
  [`${webspacesUri}/Elastos/content`, [webspaceFile]],
  [`${webspacesUri}/Cloud`, [webspaceCloudDrive]],
  [`${webspacesUri}/Cloud/Drive`, [webspaceCloudProject]],
  [`${webspacesUri}/Cloud/Drive/Project X`, [webspaceCloudFile]],
  [webspaceMutableUri, []],
]);

const mimeByExt = new Map([
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".css", "text/css; charset=utf-8"],
  [".svg", "image/svg+xml"],
]);

function ok(data) {
  return JSON.stringify({ status: "ok", data });
}

function sendJson(res, statusCode, data) {
  res.writeHead(statusCode, { "content-type": "application/json" });
  res.end(data);
}

function baseName(uri) {
  return String(uri || "").replace(/\/+$/, "").split("/").pop() || "";
}

function parentUri(uri) {
  const clean = String(uri || "").replace(/\/+$/, "");
  const index = clean.lastIndexOf("/");
  return index > "localhost://".length ? clean.slice(0, index) : clean;
}

function childUri(parent, name) {
  return `${String(parent || "").replace(/\/+$/, "")}/${name}`;
}

function fileCapabilities(extra = []) {
  return ["download", "compress_archive", "rename", "move", "copy", "publish", "trash", "properties", ...extra];
}

function folderCapabilities(extra = []) {
  return ["download", "compress_archive", "rename", "move", "copy", "trash", "properties", ...extra];
}

function touch(item) {
  item.modified_at += 1;
  item.revision = `rev:${++revisionCounter}:${item.name}`;
  return item;
}

function objectsFor(parent) {
  if (!folders.has(parent)) folders.set(parent, []);
  return folders.get(parent);
}

function rootsWithTrashState() {
  const trashObjects = objectsFor(`${principalRoot}/.Trash`);
  return roots.map((root) => root.id === "trash"
    ? {
      ...root,
      metadata: {
        ...(root.metadata || {}),
        empty: trashObjects.length === 0,
        item_count: trashObjects.length,
      },
    }
    : root);
}

function findObject(uri) {
  for (const objects of folders.values()) {
    const found = objects.find((item) => item.uri === uri);
    if (found) return found;
  }
  return null;
}

function removeObject(uri) {
  for (const [parent, objects] of folders) {
    const index = objects.findIndex((item) => item.uri === uri);
    if (index !== -1) {
      const [removed] = objects.splice(index, 1);
      return { parent, object: removed };
    }
  }
  return { parent: "", object: null };
}

function putObject(parent, item) {
  const objects = objectsFor(parent);
  const index = objects.findIndex((candidate) => candidate.uri === item.uri);
  if (index === -1) objects.push(item);
  else objects[index] = item;
  if (item.kind === "directory") objectsFor(item.uri);
  return item;
}

function createFileObject(uri, name, extra = {}) {
  return object(uri, name, "file", fileCapabilities(extra.capabilities || []), extra);
}

function createFolderObject(uri, name, extra = {}) {
  return object(uri, name, "directory", folderCapabilities(extra.capabilities || []), extra);
}

function isWebSpaceUri(uri) {
  const value = String(uri || "").replace(/\/+$/, "");
  return value === webspacesUri || value.startsWith(webspacesUri + "/");
}

function webspaceRelativePath(uri) {
  const value = String(uri || "").replace(/\/+$/, "");
  return value.startsWith(webspaceMutableUri) ? value.slice(webspaceMutableUri.length).replace(/^\/+/, "") : "";
}

function createWebspaceMetadata(uri, kind, extra = {}) {
  const relative = webspaceRelativePath(uri);
  return {
    schema: "elastos.library.webspace-object/v1",
    mount: "Mutable",
    target_uri: extra.targetUri || `local://mutable${relative ? "/" + relative : ""}`,
    resolver: "local-materialized",
    webspace_kind: extra.webspaceKind || (kind === "file" ? "materialized-file" : "materialized-directory"),
    readonly: false,
    access_policy: "owner-writable",
    ...(extra.metadata || {}),
  };
}

function createWebspaceFileObject(uri, name, extra = {}) {
  return object(uri, name, "file", [
    "open",
    "read",
    "download",
    "write",
    "delete_permanently",
    "properties",
  ], {
    ...extra,
    availability: extra.availability || "local-materialized",
    metadata: createWebspaceMetadata(uri, "file", extra),
  });
}

function createWebspaceFolderObject(uri, name, extra = {}) {
  return object(uri, name, "directory", [
    "open",
    "list",
    "new_folder",
    "write",
    "properties",
  ], {
    ...extra,
    availability: extra.availability || "local-materialized",
    metadata: createWebspaceMetadata(uri, "directory", extra),
  });
}

function copyObject(source, targetParentUri) {
  const name = source.name || baseName(source.uri);
  const clone = {
    ...source,
    uri: childUri(targetParentUri, name),
    name,
    published_cid: null,
    published: false,
    shared: false,
    capabilities: source.kind === "directory" ? folderCapabilities() : fileCapabilities(),
  };
  touch(clone);
  putObject(targetParentUri, clone);
  return clone;
}

async function readBody(req) {
  return (await readRawBody(req)).toString("utf8");
}

async function readRawBody(req) {
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  return Buffer.concat(chunks);
}

async function serveStatic(req, res) {
  const url = new URL(req.url, "http://127.0.0.1");
  const root = url.pathname.startsWith("/apps/archive-manager/")
    ? archiveManagerRoot
    : capsuleRoot;
  let relative = url.pathname.replace(/^\/apps\/(?:library|archive-manager)\/?/, "") || "index.html";
  relative = decodeURIComponent(relative);
  const filePath = path.resolve(root, relative);
  if (!filePath.startsWith(root + path.sep) && filePath !== root) {
    res.writeHead(403).end("forbidden");
    return;
  }
  try {
    const info = await stat(filePath);
    if (!info.isFile()) throw new Error("not a file");
    const ext = path.extname(filePath);
    res.writeHead(200, { "content-type": mimeByExt.get(ext) || "application/octet-stream" });
    createReadStream(filePath).pipe(res);
  } catch {
    res.writeHead(404).end("not found");
  }
}

function handleProvider(op, payload, res) {
  ops.push({ op, payload });
  if (op === "roots") return sendJson(res, 200, ok({ roots: rootsWithTrashState() }));
  if (op === "list") return sendJson(res, 200, ok({
    objects: folders.get(payload.uri) || [],
    object: findObject(payload.uri) || null,
  }));
  if (op === "download" || op === "read") {
    const found = findObject(payload.uri) || file;
    const filename = found.kind === "directory" ? `${found.name}.tar.gz` : found.name;
    return sendJson(res, 200, ok({
      object: found,
      filename,
      data: Buffer.from(`Smoke file: ${found.name}`).toString("base64"),
    }));
  }
  if (op === "mkdir") {
    const uri = childUri(payload.parent_uri, payload.name);
    const created = putObject(
      payload.parent_uri,
      isWebSpaceUri(payload.parent_uri)
        ? createWebspaceFolderObject(uri, payload.name)
        : createFolderObject(uri, payload.name),
    );
    return sendJson(res, 200, ok({ object: created }));
  }
  if (op === "write") {
    const parent = parentUri(payload.uri);
    const created = putObject(parent, (
      isWebSpaceUri(payload.uri)
        ? createWebspaceFileObject(payload.uri, baseName(payload.uri), {
          mime: payload.mime || "text/plain",
          size: Buffer.byteLength(String(payload.data || ""), "base64"),
        })
        : createFileObject(payload.uri, baseName(payload.uri), {
          mime: payload.mime || "text/plain",
          size: Buffer.byteLength(String(payload.data || ""), "base64"),
        })
    ));
    return sendJson(res, 200, ok({ object: created }));
  }
  if (op === "rename") {
    const found = findObject(payload.uri);
    if (!found) return sendJson(res, 404, JSON.stringify({ status: "error", message: "not found" }));
    const sourceParent = parentUri(found.uri);
    const oldUri = found.uri;
    found.name = payload.name;
    found.uri = childUri(sourceParent, payload.name);
    touch(found);
    if (found.kind === "directory") {
      folders.set(found.uri, folders.get(oldUri) || []);
      folders.delete(oldUri);
    }
    return sendJson(res, 200, ok({ object: found }));
  }
  if (op === "copy") {
    const source = findObject(payload.uri);
    if (!source) return sendJson(res, 404, JSON.stringify({ status: "error", message: "not found" }));
    return sendJson(res, 200, ok({ object: copyObject(source, payload.target_parent_uri) }));
  }
  if (op === "move") {
    const { object: source } = removeObject(payload.uri);
    if (!source) return sendJson(res, 404, JSON.stringify({ status: "error", message: "not found" }));
    source.uri = childUri(payload.target_parent_uri, source.name || baseName(source.uri));
    touch(source);
    putObject(payload.target_parent_uri, source);
    return sendJson(res, 200, ok({ object: source }));
  }
  if (op === "trash") {
    const { object: source } = removeObject(payload.uri);
    if (!source) return sendJson(res, 404, JSON.stringify({ status: "error", message: "not found" }));
    const originalUri = payload.uri;
    source.uri = childUri(`${principalRoot}/.Trash`, source.name || baseName(source.uri));
    source.capabilities = ["restore", "delete_permanently", "properties"];
    source.metadata = {
      ...(source.metadata || {}),
      visibility: {
        schema: "elastos.library.visibility/v1",
        placement: "trash",
        placement_label: "Trash",
        effective_access: "principal_private",
      },
      trash: {
        schema: "elastos.library.trash-record/v1",
        trash_uri: source.uri,
        original_uri: originalUri,
        original_name: baseName(originalUri),
        trashed_at: 1_780_000_100,
      },
    };
    touch(source);
    putObject(parentUri(source.uri), source);
    return sendJson(res, 200, ok({ object: source, original_uri: originalUri }));
  }
  if (op === "restore") {
    const { object: source } = removeObject(payload.uri);
    if (!source) return sendJson(res, 404, JSON.stringify({ status: "error", message: "not found" }));
    const targetUri = payload.target_uri || source.metadata?.trash?.original_uri || childUri(documentsUri, source.name || baseName(source.uri));
    source.uri = targetUri;
    source.name = baseName(targetUri);
    source.capabilities = fileCapabilities();
    if (source.metadata?.trash) delete source.metadata.trash;
    touch(source);
    putObject(parentUri(source.uri), source);
    return sendJson(res, 200, ok({ object: source }));
  }
  if (op === "delete_permanently") {
    removeObject(payload.uri);
    return sendJson(res, 200, ok({ deleted: true }));
  }
  if (op === "empty_trash") {
    const trashObjects = objectsFor(`${principalRoot}/.Trash`);
    const deletedCount = trashObjects.length;
    folders.set(`${principalRoot}/.Trash`, []);
    return sendJson(res, 200, ok({ deleted_count: deletedCount }));
  }
  if (op === "publish") {
    const found = findObject(payload.uri);
    if (!found) return sendJson(res, 404, JSON.stringify({ status: "error", message: "not found" }));
    found.published_cid = SMOKE_PUBLISHED_CID;
    found.published = true;
    found.availability = "local_pinned";
    found.capabilities = ["download", "compress_archive", "rename", "move", "copy", "unpublish", "repair", "share", "trash", "properties"];
    touch(found);
    return sendJson(res, 200, ok({ object: found, uri: `elastos://${found.published_cid}`, cid: found.published_cid }));
  }
  if (op === "unpublish") {
    const found = findObject(payload.uri);
    if (!found) return sendJson(res, 404, JSON.stringify({ status: "error", message: "not found" }));
    found.published_cid = null;
    found.published = false;
    found.shared = false;
    found.availability = "local-only";
    found.capabilities = fileCapabilities();
    touch(found);
    return sendJson(res, 200, ok({ object: found }));
  }
  if (op === "repair") {
    const found = findObject(payload.uri);
    if (!found) return sendJson(res, 404, JSON.stringify({ status: "error", message: "not found" }));
    found.availability = "local_pinned";
    found.published_cid ||= SMOKE_PUBLISHED_CID;
    found.published = true;
    found.capabilities = ["download", "compress_archive", "rename", "move", "copy", "unpublish", "repair", "share", "trash", "properties"];
    touch(found);
    return sendJson(res, 200, ok({
      object: found,
      uri: `elastos://${found.published_cid}`,
      cid: found.published_cid,
      receipt: { status: "repaired" },
      availability: { status: found.availability },
    }));
  }
  if (op === "status") {
    const found = findObject(payload.uri);
    if (!found) return sendJson(res, 404, JSON.stringify({ status: "error", message: "not found" }));
    return sendJson(res, 200, ok({
      object: found,
      published: found.published_cid ? {
        schema: "elastos.library.publish-record/v1",
        object_uri: found.uri,
        cid: found.published_cid,
        published_at: 1780000001,
        shared_at: found.shared ? 1780000002 : null,
        share_policy: found.shared ? "recipient_scoped" : null,
        share_grants: found.shared ? [{
          schema: "elastos.library.share-grant/v1",
          grant_id: "share:smoke:status",
          recipient: "person:recipient-two",
          policy: "recipient_scoped",
          key_release: {
            schema: "elastos.library.key-release/v1",
            required: false,
            status: "not_required_for_plain_published_content",
          },
        }] : [],
        content_security: {
          schema: "elastos.library.published-content-security/v1",
          published_payload: "plain_content",
          status: "not_required_for_plain_published_content",
        },
        receipt: { status: "ok", provider: "smoke" },
        availability: { status: found.availability || "local_pinned" },
      } : null,
    }));
  }
  if (op === "share") {
    const found = findObject(payload.uri);
    if (!found) return sendJson(res, 404, JSON.stringify({ status: "error", message: "not found" }));
    found.shared = true;
    touch(found);
    const recipients = Array.isArray(payload.recipients) ? payload.recipients.filter(Boolean) : [];
    const policy = recipients.length ? "recipient_scoped" : "public_link";
    return sendJson(res, 200, ok({
      schema: "elastos.library.share/v1",
      object: found,
      uri: `elastos://${found.published_cid || "share/smoke"}`,
      cid: found.published_cid || "share/smoke",
      policy,
      recipients,
      grants: recipients.map((recipient, index) => ({
        schema: "elastos.library.share-grant/v1",
        grant_id: `share:smoke:${index + 1}`,
        recipient,
        policy,
        key_release: {
          schema: "elastos.library.key-release/v1",
          required: false,
          status: "not_required_for_plain_published_content",
        },
      })),
      key_release: {
        schema: "elastos.library.key-release/v1",
        required: false,
        status: "not_required_for_plain_published_content",
      },
      content_security: {
        schema: "elastos.library.published-content-security/v1",
        published_payload: "plain_content",
        status: "not_required_for_plain_published_content",
      },
      availability: { status: found.availability || "local_pinned" },
      shared_at: 1780000003,
      object_uri: found.uri,
    }));
  }
  if (op === "extract_archive") {
    const found = findObject(payload.uri);
    if (!found) return sendJson(res, 404, JSON.stringify({ status: "error", message: "not found" }));
    const parent = parentUri(found.uri);
    const name = baseName(found.uri)
      .replace(/\.tar\.gz$/i, "")
      .replace(/\.tgz$/i, "")
      .replace(/\.tar$/i, "")
      .replace(/\.zip$/i, "") || "Extracted Archive";
    const created = putObject(parent, createFolderObject(childUri(parent, name), name));
    return sendJson(res, 200, ok({ object: created, source_uri: found.uri }));
  }
  if (op === "archive_entries") {
    const found = findObject(payload.uri);
    if (!found) return sendJson(res, 404, JSON.stringify({ status: "error", message: "not found" }));
    if (found.name.endsWith(".7z")) {
      return sendJson(res, 200, JSON.stringify({
        status: "error",
        code: "library_error",
        message: "library archive listing only supports .tar, .tar.gz, .tgz, and .zip archives",
      }));
    }
    return sendJson(res, 200, ok({
      schema: "elastos.library.archive-entries/v1",
      object: found,
      uri: found.uri,
      family: found.name.endsWith(".zip") ? "zip" : (found.name.endsWith(".tar") ? "tar" : "tar.gz"),
      entries: [
        {
          id: "entry:0",
          path: "alpha.txt",
          name: "alpha.txt",
          kind: "file",
          size: 9,
          compressed_size: 7,
          modified_at: 1780000000,
          safety: { status: "safe", reason: null },
        },
        {
          id: "entry:1",
          path: "Nested/deep.txt",
          name: "deep.txt",
          kind: "file",
          size: 10,
          compressed_size: 8,
          modified_at: 1780000000,
          safety: { status: "safe", reason: null },
        },
        {
          id: "entry:2",
          path: "../escape.txt",
          name: "escape.txt",
          kind: "blocked",
          size: 6,
          compressed_size: 6,
          modified_at: null,
          safety: { status: "blocked", reason: "library archive entry path must be relative and safe" },
        },
      ],
      limits: {
        max_entries: 512,
        returned_entries: 3,
        truncated: false,
      },
    }));
  }
  if (op === "archive_preview_entry") {
    const found = findObject(payload.uri);
    if (!found) return sendJson(res, 404, JSON.stringify({ status: "error", message: "not found" }));
    if (String(payload.entry || "").includes("..")) {
      return sendJson(res, 200, JSON.stringify({ status: "error", message: "library archive entry path must be relative and safe" }));
    }
    return sendJson(res, 200, ok({
      schema: "elastos.library.archive-preview-entry/v1",
      object: found,
      uri: found.uri,
      family: found.name.endsWith(".zip") ? "zip" : "tar.gz",
      entry: {
        path: payload.entry,
        name: baseName(payload.entry),
        kind: "file",
        size: payload.entry === "Nested/deep.txt" ? 10 : 9,
        compressed_size: 8,
        modified_at: 1780000000,
        mime: "text/plain",
        safety: { status: "safe", reason: null },
        viewers: [{ id: "documents", label: "Documents", default: true }],
      },
      preview: {
        encoding: "base64",
        data: Buffer.from(payload.entry === "Nested/deep.txt" ? "zip nested" : "zip alpha").toString("base64"),
        text: payload.entry === "Nested/deep.txt" ? "zip nested" : "zip alpha",
        truncated: false,
        max_bytes: 65536,
        mode: "provider_bounded_safe_entry_preview",
      },
    }));
  }
  if (op === "archive_extract_entries") {
    const found = findObject(payload.uri);
    const destination = findObject(payload.destination_uri) || (folders.has(payload.destination_uri)
      ? createFolderObject(payload.destination_uri, baseName(payload.destination_uri))
      : null);
    if (!found || !destination) return sendJson(res, 404, JSON.stringify({ status: "error", message: "not found" }));
    const selectedEntries = Array.isArray(payload.entries) ? payload.entries.filter(Boolean) : [];
    if (payload.cancel === true) {
      return sendJson(res, 200, ok({
        schema: "elastos.library.archive-extract-entries/v1",
        object: destination,
        source_uri: found.uri,
        destination_uri: payload.destination_uri,
        family: found.name.endsWith(".zip") ? "zip" : "tar.gz",
        conflict_policy: payload.conflict_policy || "keep_both",
        written: [],
        skipped: [],
        blocked: [],
        receipt: {
          schema: "elastos.library.archive-extract-entries.receipt/v1",
          status: "cancelled",
          progress: {
            requested_entries: selectedEntries.length,
            processed_entries: 0,
            written_entries: 0,
            skipped_entries: 0,
            blocked_entries: 0,
          },
          cancel: {
            supported: true,
            requested: true,
            status: "cancelled_before_write",
            mode: "bounded_synchronous_provider_operation",
          },
        },
      }));
    }
    const written = selectedEntries
      .filter((entry) => !entry.includes(".."))
      .map((entry) => {
        const uri = childUri(payload.destination_uri, entry.split("/").pop());
        const created = putObject(payload.destination_uri, createFileObject(uri, entry.split("/").pop(), {
          mime: "text/plain",
          size: 64,
        }));
        return { path: entry, uri: created.uri, kind: "file", size: created.size };
      });
    return sendJson(res, 200, ok({
      schema: "elastos.library.archive-extract-entries/v1",
      object: destination,
      source_uri: found.uri,
      destination_uri: payload.destination_uri,
      family: found.name.endsWith(".zip") ? "zip" : "tar.gz",
      conflict_policy: payload.conflict_policy || "keep_both",
      written,
      skipped: [],
      blocked: selectedEntries.includes("../escape.txt") ? [{
        path: "../escape.txt",
        reason: "library archive entry path must be relative and safe",
      }] : [],
      receipt: {
        schema: "elastos.library.archive-extract-entries.receipt/v1",
        status: "completed",
        progress: {
          requested_entries: selectedEntries.length,
          processed_entries: selectedEntries.length,
          written_entries: written.length,
          skipped_entries: 0,
          blocked_entries: selectedEntries.includes("../escape.txt") ? 1 : 0,
        },
        cancel: {
          supported: true,
          requested: false,
          status: "not_requested",
          mode: "bounded_synchronous_provider_operation",
        },
      },
    }));
  }
  if (op === "compress_archive") {
    const selectedUris = Array.isArray(payload.uris) ? payload.uris.filter(Boolean) : [];
    const source = selectedUris.length ? findObject(selectedUris[0]) : findObject(payload.uri);
    if (!source) return sendJson(res, 404, JSON.stringify({ status: "error", message: "not found" }));
    const parent = parentUri(source.uri);
    const name = selectedUris.length > 1 ? `${baseName(parent)} Selection.zip` : `${source.name || baseName(source.uri)}.zip`;
    const created = putObject(parent, createFileObject(childUri(parent, name), name, {
      mime: "application/zip",
      size: 1024,
    }));
    return sendJson(res, 200, ok({ object: created }));
  }
  return sendJson(res, 200, ok({ object: findObject(payload.uri) || file, receipt: { status: "ok" } }));
}

function createServer() {
  return http.createServer(async (req, res) => {
    const url = new URL(req.url, "http://127.0.0.1");
    if (url.pathname === "/host.html") {
      res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      res.end(`<!doctype html>
<html>
<head>
  <style>
    html, body { margin: 0; width: 100%; height: 100%; }
    #library-frame { border: 0; width: 100vw; height: 100vh; display: block; }
  </style>
</head>
<body>
  <iframe id="library-frame" src="/apps/library/?home_token=${encodeURIComponent(token)}"></iframe>
  <script>
    window.__shellMessages = [];
    window.addEventListener("message", (event) => {
      if (event.origin === window.location.origin) window.__shellMessages.push(event.data);
    });
  </script>
</body>
</html>`);
      return;
    }
    if (url.pathname === "/api/provider/object/events/stream") {
      res.writeHead(200, {
        "content-type": "text/event-stream",
        "cache-control": "no-cache",
        connection: "keep-alive",
      });
      res.write(":ok\n\n");
      return;
    }
    if (url.pathname === "/api/provider/object/upload/start") {
      if (req.method !== "POST" || req.headers["x-elastos-home-token"] !== token) {
        return sendJson(res, 403, JSON.stringify({ status: "error", message: "forbidden" }));
      }
      const payload = JSON.parse(await readBody(req) || "{}");
      if (!payload.uri) {
        return sendJson(res, 400, JSON.stringify({ status: "error", message: "uri is required" }));
      }
      const uploadId = `upload-${uploadSessions.size + 1}`;
      const session = {
        uploadId,
        uri: payload.uri,
        mime: payload.mime || "application/octet-stream",
        totalBytes: payload.size_bytes,
        receivedBytes: 0,
        chunkCount: 0,
        chunks: [],
      };
      uploadSessions.set(uploadId, session);
      ops.push({ op: "upload_session_start", payload: { uri: session.uri, upload_id: uploadId, total_bytes: session.totalBytes } });
      return sendJson(res, 200, ok({
        schema: "elastos.object.upload-session/v1",
        upload_id: uploadId,
        uri: session.uri,
        received_bytes: 0,
        total_bytes: session.totalBytes,
        chunk_count: 0,
        chunk_size: 786432,
        transport: "http-chunk-session",
      }));
    }
    const uploadChunkMatch = url.pathname.match(/^\/api\/provider\/object\/upload\/([^/]+)\/chunk$/);
    if (uploadChunkMatch) {
      if (req.method !== "PUT" || req.headers["x-elastos-home-token"] !== token) {
        return sendJson(res, 403, JSON.stringify({ status: "error", message: "forbidden" }));
      }
      const uploadId = decodeURIComponent(uploadChunkMatch[1]);
      const session = uploadSessions.get(uploadId);
      if (!session) {
        return sendJson(res, 404, JSON.stringify({ status: "error", message: "upload session not found" }));
      }
      const offset = Number(req.headers["x-elastos-upload-offset"] || -1);
      if (offset !== session.receivedBytes) {
        return sendJson(res, 400, JSON.stringify({ status: "error", message: "offset mismatch" }));
      }
      const body = await readRawBody(req);
      if (body.length > 786432) {
        return sendJson(res, 413, JSON.stringify({ status: "error", message: "chunk too large" }));
      }
      session.chunks.push(body);
      session.receivedBytes += body.length;
      session.chunkCount += 1;
      ops.push({ op: "upload_chunk", payload: { upload_id: uploadId, offset, size: body.length } });
      return sendJson(res, 200, ok({
        schema: "elastos.object.upload-session/v1",
        upload_id: uploadId,
        uri: session.uri,
        received_bytes: session.receivedBytes,
        total_bytes: session.totalBytes,
        chunk_count: session.chunkCount,
        chunk_size: 786432,
        transport: "http-chunk-session",
      }));
    }
    const uploadFinishMatch = url.pathname.match(/^\/api\/provider\/object\/upload\/([^/]+)\/finish$/);
    if (uploadFinishMatch) {
      if (req.method !== "POST" || req.headers["x-elastos-home-token"] !== token) {
        return sendJson(res, 403, JSON.stringify({ status: "error", message: "forbidden" }));
      }
      const uploadId = decodeURIComponent(uploadFinishMatch[1]);
      const session = uploadSessions.get(uploadId);
      if (!session) {
        return sendJson(res, 404, JSON.stringify({ status: "error", message: "upload session not found" }));
      }
      const body = Buffer.concat(session.chunks);
      const parent = parentUri(session.uri);
      const created = putObject(parent, (
        isWebSpaceUri(session.uri)
          ? createWebspaceFileObject(session.uri, baseName(session.uri), {
            mime: session.mime,
            size: body.length,
          })
          : createFileObject(session.uri, baseName(session.uri), {
            mime: session.mime,
            size: body.length,
          })
      ));
      uploadSessions.delete(uploadId);
      ops.push({ op: "upload", payload: { uri: session.uri, mime: session.mime, size: body.length, transport: "http-chunk-session", chunks: session.chunkCount } });
      return sendJson(res, 200, ok({
        object: created,
        transport: "raw-body",
        browser_transport: "http-chunk-session",
        upload_session: {
          schema: "elastos.object.upload-session/v1",
          upload_id: uploadId,
          uri: session.uri,
          received_bytes: body.length,
          total_bytes: session.totalBytes,
          chunk_count: session.chunkCount,
          transport: "http-chunk-session",
        },
        receipt: {
          schema: "elastos.object.transfer.receipt/v1",
          op: "upload",
          uri: session.uri,
          status: "completed",
          bytes: body.length,
          total_bytes: session.totalBytes,
          transport: "http-chunk-session",
        },
      }));
    }
    const uploadCancelMatch = url.pathname.match(/^\/api\/provider\/object\/upload\/([^/]+)$/);
    if (uploadCancelMatch && req.method === "DELETE") {
      uploadSessions.delete(decodeURIComponent(uploadCancelMatch[1]));
      return sendJson(res, 200, ok({ status: "cancelled" }));
    }
    if (url.pathname === "/api/provider/object/upload") {
      if (req.method !== "PUT" || req.headers["x-elastos-home-token"] !== token) {
        return sendJson(res, 403, JSON.stringify({ status: "error", message: "forbidden" }));
      }
      const uri = url.searchParams.get("uri") || "";
      if (!uri) {
        return sendJson(res, 400, JSON.stringify({ status: "error", message: "uri is required" }));
      }
      if (uri.endsWith("/TooLarge.txt")) {
        res.writeHead(413, { "content-type": "text/html; charset=utf-8" });
        res.end("<html><body><h1>413 Request Entity Too Large</h1><hr><center>nginx</center></body></html>");
        return;
      }
      const body = await readRawBody(req);
      const mime = String(req.headers["content-type"] || "application/octet-stream");
      const parent = parentUri(uri);
      const created = putObject(parent, (
        isWebSpaceUri(uri)
          ? createWebspaceFileObject(uri, baseName(uri), {
            mime,
            size: body.length,
          })
          : createFileObject(uri, baseName(uri), {
            mime,
            size: body.length,
          })
      ));
      ops.push({ op: "upload", payload: { uri, mime, size: body.length, transport: "raw-body" } });
      return sendJson(res, 200, ok({ object: created, transport: "raw-body" }));
    }
    if (url.pathname === "/api/provider/object/download/raw") {
      if (req.method !== "GET" || req.headers["x-elastos-home-token"] !== token) {
        res.writeHead(403).end("forbidden");
        return;
      }
      const uris = url.searchParams.getAll("uri").filter(Boolean);
      const archive = url.searchParams.get("archive") || "";
      const uri = uris[0] || "";
      const found = findObject(uri);
      if (!found) {
        res.writeHead(404).end("not found");
        return;
      }
      const archiveExt = archive === "zip" ? "zip" : "tar.gz";
      const archiveMime = archive === "zip" ? "application/zip" : "application/gzip";
      const filename = uris.length > 1
        ? `Documents Selection.${archiveExt}`
        : (found.kind === "directory" ? `${found.name}.${archiveExt}` : found.name);
      const mime = uris.length > 1 || found.kind === "directory" ? archiveMime : (found.mime || "application/octet-stream");
      ops.push({ op: "download_raw", payload: { uri, uris, archive, filename, transport: "raw-body" } });
      res.writeHead(200, {
        "content-type": mime,
        "content-disposition": `attachment; filename="${filename}"`,
      });
      res.end(Buffer.from(uris.length > 1 ? `Smoke selected archive: ${uris.join(",")}` : `Smoke raw download: ${found.name}`));
      return;
    }
    if (url.pathname === "/api/viewers/archive-manager/library-roots") {
      if (req.method !== "GET" || req.headers["x-elastos-home-token"] !== token) {
        return sendJson(res, 403, JSON.stringify({ status: "error", message: "forbidden" }));
      }
      return handleProvider("roots", {}, res);
    }
    if (url.pathname === "/api/viewers/archive-manager/library-object") {
      if (!["GET", "POST"].includes(req.method) || req.headers["x-elastos-home-token"] !== token) {
        return sendJson(res, 403, JSON.stringify({ status: "error", message: "forbidden" }));
      }
      const uri = url.searchParams.get("uri") || "";
      const found = findObject(uri);
      if (!found) return sendJson(res, 404, JSON.stringify({ status: "error", message: "not found" }));
      if (req.method === "POST") {
        const payload = JSON.parse(await readBody(req) || "{}");
        return handleProvider("archive_extract_entries", { uri, ...payload }, res);
      }
      if (url.searchParams.get("entries") === "true") {
        return handleProvider("archive_entries", { uri }, res);
      }
      if (url.searchParams.has("preview_entry")) {
        return handleProvider("archive_preview_entry", { uri, entry: url.searchParams.get("preview_entry") || "" }, res);
      }
      return sendJson(res, 200, ok({ object: found }));
    }
    const providerMatch = url.pathname.match(/^\/api\/provider\/object\/([^/]+)$/);
    if (providerMatch) {
      if (req.method !== "POST" || req.headers["x-elastos-home-token"] !== token) {
        return sendJson(res, 403, JSON.stringify({ status: "error", message: "forbidden" }));
      }
      const body = await readBody(req);
      const payload = body ? JSON.parse(body) : {};
      return handleProvider(decodeURIComponent(providerMatch[1]), payload, res);
    }
    return serveStatic(req, res);
  });
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

async function waitForOp(predicate, message) {
  const deadline = Date.now() + 2_000;
  while (Date.now() < deadline) {
    if (ops.some(predicate)) return;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  throw new Error(message);
}

async function menuRows(page) {
  await page.locator("#context-menu:not(.hidden)").waitFor();
  const rows = await page.locator("#context-menu > .menu-entry > .menu-item").evaluateAll((buttons) =>
    buttons.map((button) => {
      const rect = button.getBoundingClientRect();
      return {
        label: button.textContent.trim(),
        disabled: button.disabled,
        left: rect.left,
        right: rect.right,
        top: rect.top,
        bottom: rect.bottom,
      };
    }),
  );
  const menuBox = await page.locator("#context-menu").evaluate((menu) => {
    const rect = menu.getBoundingClientRect();
    return { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom };
  });
  assert(rows.length > 0, "context menu should render action rows");
  assert(rows.every((row) => !row.disabled), "context menu should not expose disabled/no-op rows");
  assert(
    rows.every((row) => row.left >= menuBox.left && row.right <= menuBox.right && row.top >= menuBox.top && row.bottom <= menuBox.bottom),
    "context menu rows must stay inside the menu box",
  );
  return rows.map((row) => row.label.replace(/[\u2713\u203a]/g, "").trim());
}

async function submenuRows(page, parentLabel) {
  const parent = page.locator("#context-menu > .menu-entry > .menu-item").filter({ hasText: parentLabel }).first();
  await parent.click();
  const entry = parent.locator("xpath=..");
  const submenu = entry.locator(".menu-submenu").first();
  await submenu.locator(".menu-item").first().waitFor({ state: "visible" });
  return submenu.locator(".menu-item").evaluateAll((buttons) =>
    buttons
      .filter((button) => {
        const rect = button.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      })
      .map((button) => button.textContent.trim().replace(/[\u2713\u203a]/g, "").trim()),
  );
}

async function openItemMenu(page, name) {
  const keyboard = page.keyboard || page.page?.().keyboard;
  await keyboard.press("Escape");
  await page.waitForFunction(() => !document.querySelector("#context-menu:not(.hidden)"));
  return openItemMenuWithoutClearingSelection(page, name);
}

async function openItemMenuWithoutClearingSelection(page, name) {
  await page.locator(".item").filter({ hasText: name }).first().click({ button: "right" });
  return menuRows(page);
}

async function openBackgroundMenu(page) {
  await page.keyboard.press("Escape");
  await page.waitForFunction(() => !document.querySelector("#context-menu:not(.hidden)"));
  const box = await page.locator("#content").boundingBox();
  assert(box, "content pane must be visible");
  await page.locator("#content").evaluate((content, point) => {
    content.dispatchEvent(new MouseEvent("contextmenu", {
      bubbles: true,
      cancelable: true,
      button: 2,
      buttons: 2,
      clientX: point.x,
      clientY: point.y,
    }));
  }, { x: box.x + box.width - 18, y: box.y + box.height - 18 });
  return menuRows(page);
}

async function activePlaceLabels(page) {
  return page.locator('.place[data-active="true"]').evaluateAll((buttons) =>
    buttons.map((button) => button.textContent.trim()),
  );
}

async function assertOnlyActivePlace(page, label) {
  const labels = await activePlaceLabels(page);
  assert(labels.length === 1 && labels[0] === label, `sidebar must mark only ${label} active, got ${labels.join(", ") || "none"}`);
}

async function assertContextMenuSuppressedOnLocator(page, locator, label) {
  await page.keyboard.press("Escape");
  await page.waitForFunction(() => !document.querySelector("#context-menu:not(.hidden)"));
  const result = await locator.evaluate((node) => {
    const event = new MouseEvent("contextmenu", {
      bubbles: true,
      cancelable: true,
      button: 2,
      buttons: 2,
      clientX: 12,
      clientY: 12,
    });
    const dispatched = node.dispatchEvent(event);
    return { defaultPrevented: event.defaultPrevented, dispatched };
  });
  await page.waitForTimeout(50);
  const openMenus = await page.locator("#context-menu:not(.hidden)").count();
  assert(result.defaultPrevented && result.dispatched === false, `${label} right-click must cancel the browser context menu`);
  assert(openMenus === 0, `${label} right-click must not open the Library context menu`);
}

async function openPlaceMenu(page, label) {
  await page.keyboard.press("Escape");
  await page.waitForFunction(() => !document.querySelector("#context-menu:not(.hidden)"));
  const result = await page.locator(".place").filter({ hasText: label }).first().evaluate((node) => {
    const rect = node.getBoundingClientRect();
    const event = new MouseEvent("contextmenu", {
      bubbles: true,
      cancelable: true,
      button: 2,
      buttons: 2,
      clientX: rect.left + 12,
      clientY: rect.top + 12,
    });
    const dispatched = node.dispatchEvent(event);
    return { defaultPrevented: event.defaultPrevented, dispatched };
  });
  assert(result.defaultPrevented && result.dispatched === false, `${label} sidebar place right-click must cancel the browser context menu`);
  return menuRows(page);
}

async function assertSidebarContextMenus(page, label) {
  await assertContextMenuSuppressedOnLocator(page, page.locator(".sidebar").first(), "Sidebar");
  await assertContextMenuSuppressedOnLocator(page, page.locator(".window-sidebar-title").first(), "Sidebar title");
  includesAll(await openPlaceMenu(page, label), ["Open", "Open in New Window"], `${label} sidebar place menu`);
}

async function sidebarLabels(page) {
  return page.locator(".place").evaluateAll((places) =>
    places.map((place) => place.textContent.trim()),
  );
}

async function reorderSidebarPlace(page, sourceLabel, targetLabel, placement) {
  await page.locator(".place").filter({ hasText: sourceLabel }).first().evaluate((source, options) => {
    const target = Array.from(document.querySelectorAll(".place"))
      .find((place) => place.textContent.trim() === options.targetLabel);
    if (!target) throw new Error(`Missing sidebar target ${options.targetLabel}`);
    const dataTransfer = new DataTransfer();
    const sourceRect = source.getBoundingClientRect();
    const targetRect = target.getBoundingClientRect();
    const clientY = options.placement === "after" ? targetRect.bottom - 1 : targetRect.top + 1;
    source.dispatchEvent(new DragEvent("dragstart", {
      bubbles: true,
      cancelable: true,
      clientY: sourceRect.top + sourceRect.height / 2,
      dataTransfer,
    }));
    target.dispatchEvent(new DragEvent("dragover", {
      bubbles: true,
      cancelable: true,
      clientY,
      dataTransfer,
    }));
    target.dispatchEvent(new DragEvent("drop", {
      bubbles: true,
      cancelable: true,
      clientY,
      dataTransfer,
    }));
    source.dispatchEvent(new DragEvent("dragend", {
      bubbles: true,
      cancelable: true,
      clientY,
      dataTransfer,
    }));
  }, { targetLabel, placement });
}

async function assertEmptyStateCentered(page, title) {
  await page.locator(".empty").filter({ hasText: title }).first().waitFor();
  const metrics = await page.locator(".empty-inner").first().evaluate((inner) => {
    const content = document.querySelector("#content");
    const contentRect = content.getBoundingClientRect();
    const innerRect = inner.getBoundingClientRect();
    return {
      horizontalDelta: Math.abs((innerRect.left + innerRect.width / 2) - (contentRect.left + contentRect.width / 2)),
      verticalDelta: Math.abs((innerRect.top + innerRect.height / 2) - (contentRect.top + contentRect.height / 2)),
      contentWidth: contentRect.width,
      contentHeight: contentRect.height,
    };
  });
  assert(metrics.horizontalDelta <= Math.max(2, metrics.contentWidth * 0.02), "empty state must be centered horizontally in the content pane");
  assert(metrics.verticalDelta <= Math.max(12, metrics.contentHeight * 0.08), "empty state must be centered vertically in the content pane");
}

async function assertPublishedBadgeVisible(page, view) {
  await page.locator(".item").filter({ hasText: "Published.md" }).first().waitFor();
  const metrics = await page.locator(".item").filter({ hasText: "Published.md" }).first().evaluate((item) => {
    const badge = item.querySelector(".badge-published");
    const name = item.querySelector(".item-name");
    const itemRect = item.getBoundingClientRect();
    const badgeRect = badge?.getBoundingClientRect();
    const nameRect = name?.getBoundingClientRect();
    return {
      hasBadge: !!badge,
      itemTop: itemRect.top,
      itemRight: itemRect.right,
      itemBottom: itemRect.bottom,
      itemLeft: itemRect.left,
      itemCenterY: itemRect.top + itemRect.height / 2,
      badgeTop: badgeRect?.top || 0,
      badgeRight: badgeRect?.right || 0,
      badgeBottom: badgeRect?.bottom || 0,
      badgeLeft: badgeRect?.left || 0,
      badgeWidth: badgeRect?.width || 0,
      badgeHeight: badgeRect?.height || 0,
      badgeCenterY: badgeRect ? badgeRect.top + badgeRect.height / 2 : 0,
      nameBottom: nameRect?.bottom || 0,
    };
  });
  assert(metrics.hasBadge && metrics.badgeWidth > 0 && metrics.badgeHeight > 0, `Published badge must be visible in ${view} view`);
  assert(
    metrics.badgeLeft >= metrics.itemLeft && metrics.badgeRight <= metrics.itemRight && metrics.badgeTop >= metrics.itemTop && metrics.badgeBottom <= metrics.itemBottom,
    `Published badge must stay inside the item in ${view} view`,
  );
  if (view === "grid") {
    assert(metrics.badgeTop >= metrics.nameBottom - 1, "Published badge must sit below the filename in icon view");
  }
  if (view === "list") {
    assert(Math.abs(metrics.badgeCenterY - metrics.itemCenterY) <= 4, "Published badge must align with the row in list view");
  }
}

async function clickMenu(page, label) {
  await page.locator("#context-menu > .menu-entry > .menu-item").filter({ hasText: label }).first().click();
}

async function clickSubmenu(page, parentLabel, childLabel) {
  const parent = page.locator("#context-menu > .menu-entry > .menu-item").filter({ hasText: parentLabel }).first();
  await parent.click();
  const entry = parent.locator("xpath=..");
  await entry.locator(".menu-submenu .menu-item").filter({ hasText: childLabel }).first().click();
}

async function renameVisibleInput(page, name) {
  await page.locator(".rename-input").fill(name);
  await page.keyboard.press("Enter");
  await page.locator(".item").filter({ hasText: name }).first().waitFor();
}

async function createFromBackgroundMenu(page, menuLabel, name) {
  await openBackgroundMenu(page);
  await clickSubmenu(page, "New", menuLabel === "New Folder" ? "Folder" : menuLabel);
  await page.locator(".rename-input").waitFor();
  await renameVisibleInput(page, name);
}

async function dropFileOnContent(page, name, text) {
  await page.locator("#content").evaluate((content, payload) => {
    const dataTransfer = new DataTransfer();
    dataTransfer.items.add(new File([payload.text], payload.name, { type: "text/plain" }));
    content.dispatchEvent(new DragEvent("drop", {
      bubbles: true,
      cancelable: true,
      dataTransfer,
    }));
  }, { name, text });
}

async function dropSelectionOnFolder(page, folderName, options = {}) {
  await page.locator(".item").filter({ hasText: folderName }).first().evaluate((item, copy) => {
    const dataTransfer = new DataTransfer();
    item.dispatchEvent(new DragEvent("drop", {
      bubbles: true,
      cancelable: true,
      dataTransfer,
      altKey: !!copy,
    }));
  }, !!options.copy);
}

function includesAll(actual, expected, context) {
  for (const label of expected) {
    assert(
      actual.includes(label),
      `${context} missing menu action: ${label}; actual=[${actual.join(", ")}]`,
    );
  }
}

function excludesAll(actual, forbidden, context) {
  for (const label of forbidden) {
    assert(
      !actual.includes(label),
      `${context} must not expose menu action: ${label}; actual=[${actual.join(", ")}]`,
    );
  }
}

async function run() {
  const server = createServer();
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  const browser = await chromium.launch({ headless: true });
  let context;
  try {
    context = await browser.newContext({ acceptDownloads: true });
    const page = await context.newPage();
    page.on("pageerror", (error) => {
      throw error;
    });
    await page.addInitScript(() => {
      window.__promptCalls = 0;
      window.__promptQueue = [];
      window.__confirmCalls = 0;
      window.__clipboardText = "";
      window.prompt = () => {
        window.__promptCalls += 1;
        return window.__promptQueue.length ? window.__promptQueue.shift() : null;
      };
      window.confirm = () => {
        window.__confirmCalls += 1;
        return true;
      };
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: {
          writeText: async (value) => {
            window.__clipboardText = String(value);
          },
        },
      });
    });
    await page.goto(`http://127.0.0.1:${port}/apps/library/?home_token=${encodeURIComponent(token)}`);
    await page.locator(".item").filter({ hasText: "Readme.md" }).first().waitFor();

    for (const label of ["Home", "Desktop", "Documents", "Pictures", "Videos", "Downloads", "Public", "Trash", "Spaces"]) {
      await page.locator(".place").filter({ hasText: label }).first().click();
      await page.locator(".crumb-current").filter({ hasText: label }).first().waitFor();
      await assertOnlyActivePlace(page, label);
    }
    await reorderSidebarPlace(page, "Public", "Desktop", "before");
    assert(
      (await sidebarLabels(page)).slice(0, 3).join("|") === "Home|Public|Desktop",
      "Sidebar place drag must reorder roots visually",
    );
    assert(
      await page.evaluate(() => (window.__libraryPerf?.sidebarReorderAnimationCount || 0) >= 1),
      "Sidebar place drag must animate reorder instead of hard-redrawing",
    );
    assert(
      await page.evaluate(() => JSON.parse(localStorage.getItem("library.sidebarOrder") || "[]").slice(0, 3).join("|")) === "home|public|desktop",
      "Sidebar place drag must persist root order by root id",
    );
    await page.reload();
    await page.locator(".item").filter({ hasText: "Readme.md" }).first().waitFor();
    assert(
      (await sidebarLabels(page)).slice(0, 3).join("|") === "Home|Public|Desktop",
      "Sidebar root order must survive reload",
    );
    await page.locator(".place").filter({ hasText: "Desktop" }).first().click();
    await assertOnlyActivePlace(page, "Desktop");
    await assertEmptyStateCentered(page, "This folder is empty");
    await assertSidebarContextMenus(page, "Desktop");
    await clickMenu(page, "Open");
    await page.locator(".crumb-current").filter({ hasText: "Desktop" }).first().waitFor();
    await createFromBackgroundMenu(page, "New Folder", "Desktop Folder");
    assert(ops.some((entry) => entry.op === "mkdir" && entry.payload.parent_uri === desktopUri && entry.payload.name === "Desktop Folder"), "Desktop New Folder must call mkdir under the Desktop URI");
    assert(await page.evaluate(() => window.__promptCalls) === 0, "New Folder must use inline naming, not window.prompt");

    await page.locator(".place").filter({ hasText: "Documents" }).first().click();
    await page.locator(".item").filter({ hasText: "Readme.md" }).first().waitFor();

    const projectsItem = page.locator(".item").filter({ hasText: "Projects" }).first();
    await projectsItem.click();
    await projectsItem.locator(".item-name").dblclick();
    await page.locator(".crumb-current").filter({ hasText: "Projects" }).first().waitFor();
    assert(await page.locator(".rename-input").count() === 0, "Double-clicking a selected folder name must open it instead of starting rename");
    await page.evaluate(() => window.history.back());
    await page.locator(".crumb-current").filter({ hasText: "Documents" }).first().waitFor();
    await page.evaluate(() => window.history.forward());
    await page.locator(".crumb-current").filter({ hasText: "Projects" }).first().waitFor();
    await page.evaluate(() => window.history.back());
    await page.locator(".crumb-current").filter({ hasText: "Documents" }).first().waitFor();

    includesAll(await openBackgroundMenu(page), [
      "Sort By",
      "View",
      "Refresh",
      "Show Hidden",
      "New",
      "Upload Here",
      "Properties",
    ], "background menu");
    includesAll(await submenuRows(page, "Sort By"), ["Name", "Date Modified", "Type", "Size", "Ascending", "Descending"], "Sort By submenu");
    includesAll(await submenuRows(page, "View"), ["Icons", "Details"], "View submenu");
    includesAll(await submenuRows(page, "New"), ["Folder", "Text Document"], "New submenu");

    assert(await page.locator("#content").getAttribute("data-view") === "grid", "Library should boot in grid view");
    await page.locator("#list-button").click();
    assert(await page.locator("#content").getAttribute("data-view") === "list", "List view button must switch to list view");
    assert(await page.locator(".explore-table-headers").count() === 1, "List view must render details headers");
    await assertPublishedBadgeVisible(page, "list");
    const listProjectsItem = page.locator(".item").filter({ hasText: "Projects" }).first();
    await listProjectsItem.click();
    await listProjectsItem.locator(".item-name").dblclick();
    await page.locator(".crumb-current").filter({ hasText: "Projects" }).first().waitFor();
    assert(await page.locator(".rename-input").count() === 0, "List-view double-clicking a selected folder name must open it instead of starting rename");
    await page.evaluate(() => window.history.back());
    await page.locator(".crumb-current").filter({ hasText: "Documents" }).first().waitFor();
    await page.locator("#list-button").click();
    assert(await page.locator("#content").getAttribute("data-view") === "list", "History return should keep list view available");
    const listFileItem = page.locator(".item").filter({ hasText: "Readme.md" }).first();
    await listFileItem.click();
    await listFileItem.locator(".item-name").dblclick();
    await page.locator(".dialog-card").filter({ hasText: "Readme.md" }).first().waitFor();
    await page.waitForTimeout(650);
    assert(await page.locator(".rename-input").count() === 0, "List-view double-clicking a selected file name must open it without later starting rename");
    await page.locator("[data-dialog-close]").click();
    await listFileItem.click();
    const readsBeforeEditorDoubleClick = ops.filter((entry) => entry.op === "read" && entry.payload.uri.endsWith("/Readme.md")).length;
    await page.keyboard.press("F2");
    await page.locator(".rename-input").waitFor();
    await page.locator(".rename-input").dblclick();
    await page.waitForTimeout(250);
    assert(
      ops.filter((entry) => entry.op === "read" && entry.payload.uri.endsWith("/Readme.md")).length === readsBeforeEditorDoubleClick,
      "List-view double-clicking the active rename editor must not also open/read the file",
    );
    await page.keyboard.press("Escape");
    assert(await page.locator(".rename-input").count() === 0, "Escape must close the rename editor after the editor double-click guard");
    await page.locator(".item").nth(0).click();
    await page.locator(".item").nth(2).click({ modifiers: ["Shift"] });
    assert(await page.locator(".item[data-selected='true']").count() === 3, "Shift-click must select a visible range in list view");
    await listFileItem.click();
    const readsBeforeEnterOpen = ops.filter((entry) => entry.op === "read" && entry.payload.uri.endsWith("/Readme.md")).length;
    await page.keyboard.press("Enter");
    await page.locator(".dialog-card").filter({ hasText: "Readme.md" }).first().waitFor();
    assert(
      ops.filter((entry) => entry.op === "read" && entry.payload.uri.endsWith("/Readme.md")).length === readsBeforeEnterOpen + 1,
      "Enter on a selected file must open/read it",
    );
    await page.locator("[data-dialog-close]").click();
    await listFileItem.click();
    await page.keyboard.press("Shift+F10");
    includesAll(await menuRows(page), ["Open", "Download", "Rename", "Properties"], "Shift-F10 selected file menu");
    await page.keyboard.press("Escape");
    await page.locator(".item").filter({ hasText: "Readme.md" }).first().click();
    await page.locator(".item").filter({ hasText: "Published.md" }).first().click({ modifiers: [multiSelectModifier] });
    assert(await page.locator(".item[data-selected='true']").count() === 2, "Two files must stay selected before multi-Enter open");
    const readsBeforeMultiEnterReadme = ops.filter((entry) => entry.op === "read" && entry.payload.uri.endsWith("/Readme.md")).length;
    const readsBeforeMultiEnterPublished = ops.filter((entry) => entry.op === "read" && entry.payload.uri.endsWith("/Published.md")).length;
    await page.keyboard.press("Enter");
    await page.locator(".dialog-card").filter({ hasText: "Published.md" }).first().waitFor();
    assert(
      ops.filter((entry) => entry.op === "read" && entry.payload.uri.endsWith("/Readme.md")).length === readsBeforeMultiEnterReadme + 1 &&
      ops.filter((entry) => entry.op === "read" && entry.payload.uri.endsWith("/Published.md")).length === readsBeforeMultiEnterPublished + 1,
      "Enter on multiple selected files must open every selected item",
    );
    await page.locator("[data-dialog-close]").click();
    await page.locator("#grid-button").click();
    assert(await page.locator("#content").getAttribute("data-view") === "grid", "Grid view button must switch to grid view");
    assert(await page.locator(".explore-table-headers").count() === 0, "Grid view must remove details headers");
    await assertPublishedBadgeVisible(page, "grid");

    await openBackgroundMenu(page);
    await clickSubmenu(page, "Sort By", "Size");
    assert(await page.locator("#sort-select").inputValue() === "size", "Sort By Size menu action must update sort state");
    await openBackgroundMenu(page);
    await clickSubmenu(page, "Sort By", "Descending");
    assert(await page.evaluate(() => localStorage.getItem("library.sortOrder")) === "desc", "Descending menu action must persist sort order");
    await openBackgroundMenu(page);
    await clickSubmenu(page, "Sort By", "Ascending");
    assert(await page.evaluate(() => localStorage.getItem("library.sortOrder")) === "asc", "Ascending menu action must persist sort order");

    includesAll(await openItemMenu(page, "Readme.md"), [
      "Open",
      "Download",
      "Compress to ZIP",
      "Publish",
      "Cut",
      "Copy",
      "Delete",
      "Rename",
      "Copy Content CID",
      "Properties",
    ], "file menu");
    excludesAll(await openItemMenu(page, "Readme.md"), ["Open With", "Copy Published Link"], "file menu without installed viewer or published link");
    await openItemMenu(page, "Readme.md");
    await clickMenu(page, "Properties");
    await page.locator(".window-item-properties").filter({ hasText: "Readme.md properties" }).first().waitFor();
    await page.locator(".item-props-tab-selected").filter({ hasText: "General" }).first().waitFor();
    await page.locator(".item-props-tbl").filter({ hasText: "Placement" }).filter({ hasText: "Private folder" }).first().waitFor();
    await page.locator(".item-props-tbl").filter({ hasText: "Visibility" }).first().waitFor();
    await page.locator(".item-props-tbl").filter({ hasText: "Visibility" }).filter({ hasText: "Private" }).first().waitFor();
    assert(await page.locator(".item-props-tab-btn").filter({ hasText: "Archive" }).count() === 0, "Normal file Properties must not show the Archive tab");
    await page.locator(".item-props-tab-btn").filter({ hasText: "Technical" }).first().click();
    await page.locator('.item-props-tab-content-selected[data-tab="technical"]').filter({ hasText: "Object kind" }).first().waitFor();
    await page.locator('.item-props-tab-content-selected[data-tab="technical"]').filter({ hasText: "Content ID" }).first().waitFor();
    await page.locator('.item-props-tab-content-selected[data-tab="technical"]').filter({ hasText: "Resolver Target" }).first().waitFor();
    await page.locator('.item-props-tab-content-selected[data-tab="technical"]').filter({ hasText: "Public Folder Policy" }).first().waitFor();
    await page.locator('.props-copy-btn[title="Copy content ID"]').first().waitFor();
    await page.locator(".properties-window-actions [data-dialog-close]").click();

    await page.locator(".place").filter({ hasText: "Public" }).first().click();
    await page.locator(".item").filter({ hasText: "Public Draft.md" }).first().waitFor();
    await page.locator(".item").filter({ hasText: "Public Draft.md" }).filter({ hasText: "Public folder" }).first().waitFor();
    includesAll(await openItemMenu(page, "Public Draft.md"), ["Publish", "Properties"], "public folder unpublished file menu");
    excludesAll(await openItemMenu(page, "Public Draft.md"), ["Copy Published Link"], "public folder unpublished file menu");
    await clickMenu(page, "Properties");
    await page.locator(".window-item-properties").filter({ hasText: "Public Draft.md properties" }).first().waitFor();
    await page.locator(".item-props-tbl").filter({ hasText: "Placement" }).filter({ hasText: "Public folder" }).first().waitFor();
    await page.locator(".item-props-tbl").filter({ hasText: "Visibility" }).filter({ hasText: "Private until published" }).first().waitFor();
    await page.locator(".item-props-tab-btn").filter({ hasText: "Technical" }).first().click();
    await page.locator('.item-props-tab-content-selected[data-tab="technical"]').filter({ hasText: "Placement only; publish creates the public content link." }).first().waitFor();
    await page.locator(".properties-window-actions [data-dialog-close]").click();

    await page.locator(".place").filter({ hasText: "Documents" }).first().click();
    await page.locator(".item").filter({ hasText: "Readme.md" }).first().waitFor();

    includesAll(await openItemMenu(page, "Viewer.md"), ["Open", "Open With", "Download", "Publish"], "viewer file menu");
    includesAll(await submenuRows(page, "Open With"), ["Documents"], "Open With submenu");

    await clickMenu(page, "Copy");
    includesAll(await openItemMenu(page, "Projects"), [
      "Open",
      "Open in New Window",
      "Download",
      "Download as ZIP",
      "Compress to ZIP",
      "Cut",
      "Copy",
      "Paste Into Folder",
      "Delete",
      "Rename",
      "Properties",
    ], "folder menu after copy");

    const folderDownloadPromise = page.waitForEvent("download");
    await clickMenu(page, "Download");
    await folderDownloadPromise;
    assert(ops.some((entry) => entry.op === "download_raw" && entry.payload.uri.endsWith("/Projects") && entry.payload.transport === "raw-body"), "Folder Download must use the raw Library download transport");

    includesAll(await openItemMenu(page, "Projects"), ["Download as ZIP"], "folder ZIP download menu");
    const folderZipDownloadPromise = page.waitForEvent("download");
    await clickMenu(page, "Download as ZIP");
    await folderZipDownloadPromise;
    assert(
      ops.some((entry) =>
        entry.op === "download_raw" &&
        entry.payload.uri.endsWith("/Projects") &&
        entry.payload.archive === "zip" &&
        entry.payload.filename === "Projects.zip"),
      "Download as ZIP must use the raw Library download transport with archive=zip",
    );

    includesAll(await openItemMenu(page, "Projects"), ["Compress to ZIP"], "folder compress menu");
    await clickMenu(page, "Compress to ZIP");
    await page.locator(".item").filter({ hasText: "Projects.zip" }).first().waitFor();
    assert(
      ops.some((entry) => entry.op === "compress_archive" && entry.payload.uri.endsWith("/Projects")),
      "Compress to ZIP must call compress_archive for the selected folder",
    );

    await page.locator(".item").filter({ hasText: "Readme.md" }).first().click();
    await page.locator(".item").filter({ hasText: "Projects" }).first().click({ modifiers: [multiSelectModifier] });
    includesAll(await openItemMenuWithoutClearingSelection(page, "Projects"), ["Download Selected", "Download Selected as ZIP", "Compress Selected to ZIP", "Cut", "Copy", "Delete"], "multi-select menu");
    const selectedDownloadPromise = page.waitForEvent("download");
    await clickMenu(page, "Download Selected");
    await selectedDownloadPromise;
    assert(
      ops.some((entry) =>
        entry.op === "download_raw" &&
        entry.payload.uris?.length === 2 &&
        entry.payload.uris.some((uri) => uri.endsWith("/Readme.md")) &&
        entry.payload.uris.some((uri) => uri.endsWith("/Projects")) &&
        entry.payload.filename === "Documents Selection.tar.gz"),
      "Download Selected must use the raw Library download transport with both selected URIs",
    );

    includesAll(await openItemMenuWithoutClearingSelection(page, "Projects"), ["Download Selected as ZIP"], "multi-select ZIP menu");
    const selectedZipDownloadPromise = page.waitForEvent("download");
    await clickMenu(page, "Download Selected as ZIP");
    await selectedZipDownloadPromise;
    assert(
      ops.some((entry) =>
        entry.op === "download_raw" &&
        entry.payload.uris?.length === 2 &&
        entry.payload.uris.some((uri) => uri.endsWith("/Readme.md")) &&
        entry.payload.uris.some((uri) => uri.endsWith("/Projects")) &&
        entry.payload.archive === "zip" &&
        entry.payload.filename === "Documents Selection.zip"),
      "Download Selected as ZIP must use the raw Library download transport with archive=zip",
    );

    includesAll(await openItemMenuWithoutClearingSelection(page, "Projects"), ["Compress Selected to ZIP"], "multi-select compress menu");
    await clickMenu(page, "Compress Selected to ZIP");
    await page.locator(".item").filter({ hasText: "Documents Selection.zip" }).first().waitFor();
    assert(
      ops.some((entry) =>
        entry.op === "compress_archive" &&
        entry.payload.uris?.length === 2 &&
        entry.payload.uris.some((uri) => uri.endsWith("/Readme.md")) &&
        entry.payload.uris.some((uri) => uri.endsWith("/Projects"))),
      "Compress Selected to ZIP must call compress_archive with both selected URIs",
    );

    const publishedRows = await openItemMenu(page, "Published.md");
    includesAll(publishedRows, [
      "Open",
      "Download",
      "Compress to ZIP",
      "Status",
      "Repair",
      "Share",
      "Unpublish",
      "Copy Content CID",
      "Copy Published Link",
      "Properties",
    ], "published file menu");
    excludesAll(publishedRows, ["Publish"], "published file menu");
    await clickMenu(page, "Status");
    await page.locator(".dialog-card").filter({ hasText: "Availability" }).first().waitFor();
    await page.locator(".dialog-card details").filter({ hasText: "Technical details" }).first().locator("summary").click();
    await page.locator(".dialog-card").filter({ hasText: "Share Grants / Key Release" }).first().waitFor();
    await page.locator(".dialog-card").filter({ hasText: "plain_content" }).first().waitFor();
    await page.locator(".dialog-card").filter({ hasText: "Publish Receipt" }).first().waitFor();
    assert(ops.some((entry) => entry.op === "status" && entry.payload.uri.endsWith("/Published.md")), "Status must call status");
    await page.locator("[data-dialog-close]").click();
    await openItemMenu(page, "Published.md");
    await clickMenu(page, "Repair");
    assert(ops.some((entry) => entry.op === "repair" && entry.payload.uri.endsWith("/Published.md")), "Repair must call repair");

    includesAll(await openItemMenu(page, "Bundle.tar.gz"), ["Open", "Download", "Extract Here", "Cut", "Copy", "Delete", "Rename", "Properties"], "archive file menu");
    excludesAll(await openItemMenu(page, "Readme.md"), ["Extract Here"], "non-archive file menu");
    await openItemMenu(page, "Bundle.tar.gz");
    await clickMenu(page, "Extract Here");
    await page.locator('.item[data-kind="directory"]').filter({ hasText: "Bundle" }).first().waitFor();
    assert(ops.some((entry) => entry.op === "extract_archive" && entry.payload.uri.endsWith("/Bundle.tar.gz")), "Extract Here must call extract_archive");
    includesAll(await openItemMenu(page, "Plain.tar"), ["Extract Here"], "plain tar archive file menu");
    await clickMenu(page, "Extract Here");
    await page.locator('.item[data-kind="directory"]').filter({ hasText: "Plain" }).first().waitFor();
    assert(ops.some((entry) => entry.op === "extract_archive" && entry.payload.uri.endsWith("/Plain.tar")), "Extract Here must call extract_archive for .tar");
    includesAll(await openItemMenu(page, "Portable.zip"), ["Extract Here"], "zip archive file menu");
    await clickMenu(page, "Extract Here");
    await page.locator('.item[data-kind="directory"]').filter({ hasText: "Portable" }).first().waitFor();
    assert(ops.some((entry) => entry.op === "extract_archive" && entry.payload.uri.endsWith("/Portable.zip")), "Extract Here must call extract_archive for .zip");
    const policyGatedArchiveRows = await openItemMenu(page, "Legacy.7z");
    includesAll(policyGatedArchiveRows, ["Open", "Open With", "Archive Support", "Download", "Properties"], "policy-gated archive file menu");
    excludesAll(policyGatedArchiveRows, ["Extract Here"], "policy-gated archive file menu");
    includesAll(await submenuRows(page, "Open With"), ["Archive"], "policy-gated archive viewer submenu");
    await page.keyboard.press("Escape");
    await openItemMenu(page, "Legacy.7z");
    await clickMenu(page, "Archive Support");
    await page.locator(".window-item-properties").filter({ hasText: "Legacy.7z properties" }).first().waitFor();
    await page.locator(".item-props-tab-btn").filter({ hasText: "Archive" }).first().click();
    await page.locator('.item-props-tab-content-selected[data-tab="archive"]').filter({ hasText: "policy-gated archive" }).first().waitFor();
    await page.locator('.item-props-tab-content-selected[data-tab="archive"]').filter({ hasText: "dependency and release-policy review" }).first().waitFor();
    await page.locator(".properties-window-actions [data-dialog-close]").click();

    const blockedRows = await openItemMenu(page, "secret.md");
    includesAll(blockedRows, ["Properties"], "blocked file menu");
    excludesAll(blockedRows, ["Open", "Delete", "Rename"], "blocked file menu");

    await page.locator(".place").filter({ hasText: "Trash" }).first().click();
    await page.locator(".item").filter({ hasText: "Deleted.txt" }).first().waitFor();
    includesAll(await openPlaceMenu(page, "Trash"), ["Open", "Open in New Window", "Empty Trash"], "Trash sidebar place menu");
    await page.keyboard.press("Escape");

    includesAll(await openItemMenu(page, "Deleted.txt"), [
      "Restore",
      "Delete Permanently",
      "Properties",
    ], "trash file menu");
    await openItemMenu(page, "Purge.txt");
    await clickMenu(page, "Delete Permanently");
    await page.locator("#dialog").filter({ hasText: "Delete permanently?" }).first().waitFor();
    await page.locator("#dialog button").filter({ hasText: "Delete Permanently" }).first().click();
    await page.waitForFunction(() => !Array.from(document.querySelectorAll(".item")).some((item) => item.textContent.includes("Purge.txt")));
    assert(ops.some((entry) => entry.op === "delete_permanently" && entry.payload.uri.endsWith("/Purge.txt")), "Delete Permanently must call delete_permanently");
    assert(await page.evaluate(() => window.__confirmCalls) === 0, "Delete Permanently must use in-app confirmation, not window.confirm");

    includesAll(await openItemMenu(page, "Deleted.txt"), [
      "Restore",
      "Delete Permanently",
      "Properties",
    ], "trash file menu after permanent delete");
    await clickMenu(page, "Restore");
    await page.waitForFunction(() => !document.querySelector("#context-menu:not(.hidden)"));
    assert(await page.evaluate(() => window.__promptCalls) === 0, "Restore must not expose a raw URI prompt");
    assert(ops.some((entry) => entry.op === "restore"), "Restore must call the Runtime provider");

    await page.locator(".place").filter({ hasText: "Spaces" }).first().click();
    await assertOnlyActivePlace(page, "Spaces");
    await page.waitForFunction(() => document.querySelector("#footer-left")?.textContent?.includes("4 items"));
    await page.locator(".item").filter({ hasText: "Localhost" }).first().waitFor();
    includesAll(await openItemMenu(page, "Localhost"), ["Open", "Open in New Window", "Properties"], "Localhost Spaces pointer menu");
    excludesAll(await openItemMenu(page, "Localhost"), ["Download", "Compress to ZIP", "Publish", "Delete", "Rename"], "Localhost Spaces pointer menu");
    await page.locator(".item").filter({ hasText: "Localhost" }).first().dblclick();
    await page.locator(".crumb-current").filter({ hasText: "Home" }).first().waitFor();
    await page.locator(".place").filter({ hasText: "Spaces" }).first().click();
    await assertOnlyActivePlace(page, "Spaces");
    const webspaceRows = await openBackgroundMenu(page);
    excludesAll(webspaceRows, ["New", "Paste", "Upload Here"], "Spaces background menu");
    includesAll(webspaceRows, ["Sort By", "Refresh", "Properties"], "Spaces background menu");
    includesAll(await openItemMenu(page, "Cloud"), ["Open", "Open in New Window", "Properties"], "mounted WebSpace menu");
    excludesAll(await openItemMenu(page, "Cloud"), ["Download", "Compress to ZIP", "Publish", "Delete", "Rename", "Paste Into Folder"], "mounted WebSpace menu");

    await page.locator(".item").filter({ hasText: "Cloud" }).first().dblclick();
    await page.locator(".crumb-current").filter({ hasText: "Cloud" }).first().waitFor();
    await page.locator(".item").filter({ hasText: "Drive" }).first().dblclick();
    await page.locator(".crumb-current").filter({ hasText: "Drive" }).first().waitFor();
    await page.locator(".item").filter({ hasText: "Project X" }).first().dblclick();
    await page.locator(".crumb-current").filter({ hasText: "Project X" }).first().waitFor();
    const indexedWebspaceRows = await openItemMenu(page, "file.pdf");
    includesAll(indexedWebspaceRows, ["Open", "Download", "Properties"], "indexed WebSpace file menu");
    excludesAll(indexedWebspaceRows, ["Publish", "Delete", "Rename", "Compress to ZIP", "Paste Into Folder"], "indexed WebSpace file menu");
    const webspaceDownloadPromise = page.waitForEvent("download");
    await clickMenu(page, "Download");
    await webspaceDownloadPromise;
    assert(
      ops.some((entry) => entry.op === "download_raw" && entry.payload.uri.endsWith("/WebSpaces/Cloud/Drive/Project X/file.pdf") && entry.payload.transport === "raw-body"),
      "Indexed WebSpace Download must use the raw Library download transport",
    );

    await page.locator(".place").filter({ hasText: "Spaces" }).first().click();
    await page.locator(".item").filter({ hasText: "Elastos" }).first().dblclick();
    await page.locator(".item").filter({ hasText: "content" }).first().dblclick();
    const contentRows = await openItemMenu(page, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi");
    includesAll(contentRows, ["Open", "Download", "Properties"], "Elastos content WebSpace file menu");
    excludesAll(contentRows, ["Publish", "Delete", "Rename", "Compress to ZIP", "Paste Into Folder"], "Elastos content WebSpace file menu");

    await page.locator(".place").filter({ hasText: "Spaces" }).first().click();
    await page.locator(".item").filter({ hasText: "Mutable" }).first().dblclick();
    await page.locator(".crumb-current").filter({ hasText: "Mutable" }).first().waitFor();
    const mutableBackgroundRows = await openBackgroundMenu(page);
    includesAll(mutableBackgroundRows, ["New", "Upload Here", "Properties"], "mutable WebSpace background menu");
    excludesAll(mutableBackgroundRows, ["Paste"], "mutable WebSpace background menu");
    await createFromBackgroundMenu(page, "New Folder", "Mutable Folder");
    assert(
      ops.some((entry) => entry.op === "mkdir" && entry.payload.parent_uri === webspaceMutableUri && entry.payload.name === "Mutable Folder"),
      "Mutable WebSpace New Folder must call mkdir under the mounted WebSpace URI",
    );
    await page.setInputFiles("#file-input", {
      name: "Mutable Upload.txt",
      mimeType: "text/plain",
      buffer: Buffer.from("Uploaded through mutable WebSpace smoke"),
    });
    await page.locator(".item").filter({ hasText: "Mutable Upload.txt" }).first().waitFor();
    assert(
      ops.some((entry) => entry.op === "upload" && entry.payload.uri.endsWith("/WebSpaces/Mutable/Mutable Upload.txt") && entry.payload.transport === "raw-body"),
      "Mutable WebSpace upload must use the raw provider upload transport",
    );
    const mutableUploadRows = await openItemMenu(page, "Mutable Upload.txt");
    includesAll(mutableUploadRows, ["Open", "Download", "Delete Permanently", "Properties"], "mutable WebSpace file menu");
    excludesAll(mutableUploadRows, ["Publish", "Rename", "Compress to ZIP", "Delete"], "mutable WebSpace file menu");
    await clickMenu(page, "Delete Permanently");
    await page.locator("#dialog").filter({ hasText: "Delete permanently?" }).first().waitFor();
    await page.locator("#dialog button").filter({ hasText: "Delete Permanently" }).first().click();
    await page.waitForFunction(() => !Array.from(document.querySelectorAll(".item")).some((item) => item.textContent.includes("Mutable Upload.txt")));
    assert(
      ops.some((entry) => entry.op === "delete_permanently" && entry.payload.uri.endsWith("/WebSpaces/Mutable/Mutable Upload.txt")),
      "Mutable WebSpace delete must call delete_permanently",
    );

    await page.locator(".place").filter({ hasText: "Documents" }).first().click();
    await page.locator(".item").filter({ hasText: "Readme.md" }).first().waitFor();

    assert(await page.locator(".item").filter({ hasText: ".env" }).count() === 0, "hidden files should be hidden by default");
    await openBackgroundMenu(page);
    await clickMenu(page, "Show Hidden");
    await page.locator(".item").filter({ hasText: ".env" }).first().waitFor();

    await createFromBackgroundMenu(page, "New Folder", "Smoke Folder");
    assert(ops.some((entry) => entry.op === "mkdir" && entry.payload.name === "Smoke Folder"), "New Folder must call mkdir");

    await createFromBackgroundMenu(page, "Text Document", "Notes.txt");
    assert(ops.some((entry) => entry.op === "write" && entry.payload.uri.endsWith("/Notes.txt")), "Text Document must call write");
    assert(await page.evaluate(() => window.__promptCalls) === 0, "Text Document must use inline naming, not window.prompt");

    await page.setInputFiles("#file-input", {
      name: "Upload.txt",
      mimeType: "text/plain",
      buffer: Buffer.from("Uploaded through Library smoke"),
    });
    await page.locator(".item").filter({ hasText: "Upload.txt" }).first().waitFor();
    assert(ops.some((entry) => entry.op === "upload" && entry.payload.uri.endsWith("/Upload.txt") && entry.payload.transport === "raw-body"), "Upload must use the raw Library upload transport");

    await page.setInputFiles("#file-input", {
      name: "LargeVideo.mp4",
      mimeType: "video/mp4",
      buffer: Buffer.alloc(640 * 1024, 7),
    });
    await page.locator(".item").filter({ hasText: "LargeVideo.mp4" }).first().waitFor();
    assert(
      ops.some((entry) => entry.op === "upload_session_start" && entry.payload.uri.endsWith("/LargeVideo.mp4")),
      "Large upload must start a Runtime object upload session",
    );
    assert(
      ops.some((entry) => entry.op === "upload_chunk"),
      "Large upload must send bounded chunks through Runtime",
    );
    assert(
      ops.some((entry) => entry.op === "upload" && entry.payload.uri.endsWith("/LargeVideo.mp4") && entry.payload.transport === "http-chunk-session"),
      "Large upload must commit after the chunk session, not through raw single-body upload",
    );

    await page.setInputFiles("#file-input", {
      name: "TooLarge.txt",
      mimeType: "text/plain",
      buffer: Buffer.from("edge proxy should reject this synthetic upload"),
    });
    await page.locator("#status-text").filter({ hasText: "too large for the current upload service" }).first().waitFor();
    assert(
      !ops.some((entry) => entry.op === "upload" && entry.payload.uri.endsWith("/TooLarge.txt")),
      "413 uploads must fail before object-provider commit",
    );

    await dropFileOnContent(page, "Dropped.txt", "Dropped through Library smoke");
    await page.locator(".item").filter({ hasText: "Dropped.txt" }).first().waitFor();
    assert(ops.some((entry) => entry.op === "upload" && entry.payload.uri.endsWith("/Dropped.txt") && entry.payload.transport === "raw-body"), "Drop upload must use the raw Library upload transport");

    await page.locator(".item").filter({ hasText: "Readme.md" }).first().click();
    await page.keyboard.press("F2");
    await renameVisibleInput(page, "Guide.md");
    assert(ops.some((entry) => entry.op === "rename" && entry.payload.name === "Guide.md"), "F2 rename must call rename");

    await openItemMenu(page, "Smoke Folder");
    await clickMenu(page, "Rename");
    await renameVisibleInput(page, "Work");
    assert(ops.some((entry) => entry.op === "rename" && entry.payload.name === "Work"), "Context-menu Rename must call rename");

    await page.locator(".item").filter({ hasText: "Dropped.txt" }).first().click();
    await page.locator(".item").filter({ hasText: "Dropped.txt" }).first().locator(".item-name").click();
    await page.waitForTimeout(320);
    assert(await page.locator(".rename-input").count() === 0, "Selected-name click must not start rename; rename is explicit through context menu or F2");
    await openItemMenu(page, "Dropped.txt");
    await clickMenu(page, "Rename");
    await renameVisibleInput(page, "Dropped Final.txt");
    assert(ops.some((entry) => entry.op === "rename" && entry.payload.name === "Dropped Final.txt"), "Explicit Rename must call rename for dropped files");

    await page.locator(".item").filter({ hasText: "Guide.md" }).first().dblclick();
    await page.locator(".dialog-card").filter({ hasText: "Guide.md" }).first().waitFor();
    assert(ops.some((entry) => entry.op === "read" && entry.payload.uri.endsWith("/Guide.md")), "Double-click preview without an installed viewer must call read");
    await page.locator("[data-dialog-close]").click();

    const downloadPromise = page.waitForEvent("download");
    await openItemMenu(page, "Guide.md");
    await clickMenu(page, "Download");
    await downloadPromise;
    assert(ops.some((entry) => entry.op === "download_raw" && entry.payload.uri.endsWith("/Guide.md") && entry.payload.transport === "raw-body"), "Download must use the raw Library download transport");

    await openItemMenu(page, "Guide.md");
    await clickMenu(page, "Copy");
    await openItemMenu(page, "Work");
    await clickMenu(page, "Paste Into Folder");
    assert(
      ops.some((entry) => entry.op === "copy" && entry.payload.target_parent_uri.endsWith("/Work")),
      "Copy/Paste Into Folder must call copy with the destination folder",
    );

    await openItemMenu(page, "Dropped Final.txt");
    await clickMenu(page, "Cut");
    await openItemMenu(page, "Work");
    await clickMenu(page, "Paste Into Folder");
    await page.waitForFunction(() => !Array.from(document.querySelectorAll(".item")).some((item) => item.textContent.includes("Dropped Final.txt")));
    assert(
      ops.some((entry) => entry.op === "move" && entry.payload.uri.endsWith("/Dropped Final.txt") && entry.payload.target_parent_uri.endsWith("/Work")),
      "Cut/Paste Into Folder must call move with the destination folder",
    );

    await page.locator(".item").filter({ hasText: "Notes.txt" }).first().click();
    await dropSelectionOnFolder(page, "Work");
    await page.waitForFunction(() => !Array.from(document.querySelectorAll(".item")).some((item) => item.textContent.includes("Notes.txt")));
    assert(
      ops.some((entry) => entry.op === "move" && entry.payload.uri.endsWith("/Notes.txt") && entry.payload.target_parent_uri.endsWith("/Work")),
      "Drag/drop move must call move with the destination folder",
    );

    await page.locator(".item").filter({ hasText: "Upload.txt" }).first().click();
    await dropSelectionOnFolder(page, "Work", { copy: true });
    await page.locator(".item").filter({ hasText: "Upload.txt" }).first().waitFor();
    assert(
      ops.some((entry) => entry.op === "copy" && entry.payload.uri.endsWith("/Upload.txt") && entry.payload.target_parent_uri.endsWith("/Work")),
      "Alt-drag/drop copy must call copy with the destination folder",
    );

    await openItemMenu(page, "Guide.md");
    await clickMenu(page, "Publish");
    await page.waitForFunction(() => document.querySelector("#footer-left")?.textContent?.includes("item"));
    assert(ops.some((entry) => entry.op === "publish" && entry.payload.uri.endsWith("/Guide.md")), "Publish must call publish");
    const guidePublishedRows = await openItemMenu(page, "Guide.md");
    includesAll(guidePublishedRows, ["Status", "Repair", "Unpublish", "Share", "Copy Content CID", "Copy Published Link"], "published Guide menu");
    await clickMenu(page, "Share");
    await page.locator(".dialog-card").filter({ hasText: "Choose who can access this item" }).first().waitFor();
    await page.locator(".dialog-card button").filter({ hasText: "Share" }).first().click();
    assert(
      ops.some((entry) => entry.op === "share" && entry.payload.uri.endsWith("/Guide.md") && entry.payload.policy === "public_link"),
      "Share must call share with the public_link policy",
    );
    await page.locator(".dialog-card").filter({ hasText: "public_link" }).first().waitFor();
    await page.locator(".dialog-card").filter({ hasText: "Recipients" }).first().waitFor();
    assert(
      await page.evaluate(() => window.__clipboardText) === `elastos://${SMOKE_PUBLISHED_CID}`,
      "Share must copy the published elastos:// link",
    );
    await page.locator(".dialog-card button").filter({ hasText: "Copy Link" }).first().click();
    assert(
      await page.evaluate(() => window.__clipboardText) === `elastos://${SMOKE_PUBLISHED_CID}`,
      "Share receipt Copy Link must copy the published elastos:// link",
    );
    await page.locator("[data-dialog-close]").click();

    await openItemMenu(page, "Guide.md");
    await clickMenu(page, "Share");
    await page.locator(".dialog-card").filter({ hasText: "Choose who can access this item" }).first().waitFor();
    await page.locator('input[name="sharePolicy"][value="recipient_scoped"]').check();
    await page.locator('textarea[name="shareRecipients"]').fill("did:key:recipient-one\nperson:recipient-two");
    await page.locator(".dialog-card button").filter({ hasText: "Share" }).first().click();
    assert(
      ops.some((entry) =>
        entry.op === "share" &&
        entry.payload.uri.endsWith("/Guide.md") &&
        entry.payload.policy === "recipient_scoped" &&
        entry.payload.recipients?.length === 2 &&
        entry.payload.recipients.includes("did:key:recipient-one") &&
        entry.payload.recipients.includes("person:recipient-two")),
      "Recipient-scoped Share must call share with explicit recipients",
    );
    await page.locator(".dialog-card").filter({ hasText: "recipient_scoped" }).first().waitFor();
    await page.locator(".dialog-card").filter({ hasText: "Grants" }).first().waitFor();
    await page.locator(".dialog-card details").filter({ hasText: "Technical details" }).first().locator("summary").click();
    await page.locator(".dialog-card").filter({ hasText: "Share Receipt Summary" }).first().waitFor();
    await page.locator(".dialog-card").filter({ hasText: "not_required_for_plain_published_content" }).first().waitFor();
    await page.locator("[data-dialog-close]").click();

    await openItemMenu(page, "Guide.md");
    await clickMenu(page, "Copy Content CID");
    assert(
      await page.evaluate(() => window.__clipboardText) === SMOKE_LOCAL_CONTENT_CID,
      "Copy Content CID must write the current file-byte CID to clipboard",
    );
    await openItemMenu(page, "Guide.md");
    await clickMenu(page, "Copy Published Link");
    assert(
      await page.evaluate(() => window.__clipboardText) === `elastos://${SMOKE_PUBLISHED_CID}`,
      "Copy Published Link must write the elastos:// link to clipboard",
    );
    await openItemMenu(page, "Guide.md");
    await clickMenu(page, "Unpublish");
    await waitForOp((entry) => entry.op === "unpublish" && entry.payload.uri.endsWith("/Guide.md"), "Unpublish must call unpublish");

    await openItemMenu(page, "Guide.md");
    await clickMenu(page, "Delete");
    await page.waitForFunction(() => !Array.from(document.querySelectorAll(".item")).some((item) => item.textContent.includes("Guide.md")));
    assert(ops.some((entry) => entry.op === "trash" && entry.payload.uri.endsWith("/Guide.md")), "Delete must move the object to Trash");

    const readsBeforeViewerOpen = ops.filter((entry) => entry.op === "read" && entry.payload.uri.endsWith("/Viewer.md")).length;
    await page.goto(`http://127.0.0.1:${port}/host.html`);
    const libraryFrameHandle = await page.locator("#library-frame").elementHandle();
    const libraryFrame = await libraryFrameHandle?.contentFrame();
    assert(libraryFrame, "host page must frame Library for Home message smoke coverage");
    await libraryFrame.locator(".place").filter({ hasText: "Desktop" }).first().evaluate((node) => {
      const rect = node.getBoundingClientRect();
      const event = new MouseEvent("contextmenu", {
        bubbles: true,
        cancelable: true,
        button: 2,
        buttons: 2,
        clientX: rect.left + 12,
        clientY: rect.top + 12,
      });
      node.dispatchEvent(event);
    });
    await libraryFrame.locator("#context-menu > .menu-entry > .menu-item").filter({ hasText: "Open in New Window" }).first().click();
    await page.waitForFunction((uri) =>
      window.__shellMessages?.some((message) =>
        message?.type === "home:open-target" &&
        message?.target === "library" &&
        message?.query?.uri === uri),
      desktopUri,
    );
    await libraryFrame.locator(".place").filter({ hasText: "Documents" }).first().click();
    await libraryFrame.locator(".item").filter({ hasText: "Viewer.md" }).first().waitFor();
    await libraryFrame.locator(".item").filter({ hasText: "Projects" }).first().evaluate((node) => {
      const rect = node.getBoundingClientRect();
      const event = new MouseEvent("contextmenu", {
        bubbles: true,
        cancelable: true,
        button: 2,
        buttons: 2,
        clientX: rect.left + 12,
        clientY: rect.top + 12,
      });
      node.dispatchEvent(event);
    });
    await libraryFrame.locator("#context-menu > .menu-entry > .menu-item").filter({ hasText: "Open in New Window" }).first().click();
    await page.waitForFunction((uri) =>
      window.__shellMessages?.some((message) =>
        message?.type === "home:open-target" &&
        message?.target === "library" &&
        message?.query?.uri === uri),
      `${documentsUri}/Projects`,
    );
    const readsBeforeArchiveViewerOpen = ops.filter((entry) => entry.op === "read" && entry.payload.uri.endsWith("/Legacy.7z")).length;
    const archiveViewerItem = libraryFrame.locator(".item").filter({ hasText: "Legacy.7z" }).first();
    await archiveViewerItem.click();
    await archiveViewerItem.locator(".item-name").dblclick();
    await page.waitForFunction(() =>
      window.__shellMessages?.some((message) =>
        message?.type === "home:open-target" &&
        message?.target === "archive-manager" &&
        message?.query?.objectUri?.endsWith("/Legacy.7z") &&
        message?.query?.archiveSupport?.includes("policy_gated_unsupported_archive_family")),
    );
    assert(
      ops.filter((entry) => entry.op === "read" && entry.payload.uri.endsWith("/Legacy.7z")).length === readsBeforeArchiveViewerOpen,
      "Double-click with Archive must not fall back to preview/read or unsafe extraction",
    );
    const looseZipItem = libraryFrame.locator(".item").filter({ hasText: "Loose.zip" }).first();
    await looseZipItem.click();
    await looseZipItem.locator(".item-name").dblclick();
    await page.waitForFunction(() =>
      window.__shellMessages?.some((message) =>
        message?.type === "home:open-target" &&
        message?.target === "archive-manager" &&
        message?.query?.objectUri?.endsWith("/Loose.zip")),
    );
    const viewerItem = libraryFrame.locator(".item").filter({ hasText: "Viewer.md" }).first();
    await viewerItem.click();
    await viewerItem.locator(".item-name").dblclick();
    await page.waitForFunction(() =>
      window.__shellMessages?.some((message) =>
        message?.type === "home:open-target" &&
        message?.target === "documents" &&
        message?.query?.objectUri?.endsWith("/Viewer.md")),
    );
    assert(await libraryFrame.locator(".rename-input").count() === 0, "Double-clicking a selected file name must open it instead of starting rename");
    assert(
      ops.filter((entry) => entry.op === "read" && entry.payload.uri.endsWith("/Viewer.md")).length === readsBeforeViewerOpen,
      "Double-click with an installed viewer must not fall back to preview/read",
    );
    const gbaItem = libraryFrame.locator(".item").filter({ hasText: "Game.gba" }).first();
    await gbaItem.click();
    await gbaItem.locator(".item-name").dblclick();
    await page.waitForFunction(() =>
      window.__shellMessages?.some((message) =>
        message?.type === "home:open-target" &&
        message?.target === "gba-emulator" &&
        message?.query?.objectUri?.endsWith("/Game.gba") &&
        message?.query?.mime === "application/x-gba-rom"),
    );

    await page.evaluate(() => {
      window.__shellMessages = [];
    });
    await libraryFrame.goto(`http://127.0.0.1:${port}/apps/library/?home_token=${encodeURIComponent(token)}&mode=attach&returnTarget=browser`);
    await libraryFrame.locator("#picker-action-button").filter({ hasText: "Select for Browser" }).first().waitFor();
    await libraryFrame.locator("#status-text").filter({ hasText: "Choose an item for Browser." }).first().waitFor();
    const browserAttachRows = await openItemMenu(libraryFrame, "Viewer.md");
    includesAll(browserAttachRows, ["Select for Browser", "Download", "Properties"], "Browser attach file menu");
    excludesAll(browserAttachRows, ["Open With"], "Browser attach file menu");
    await libraryFrame.locator(".item").filter({ hasText: "Viewer.md" }).first().click();
    await libraryFrame.locator("#picker-action-button").filter({ hasText: "Select for Browser" }).first().click();
    await page.waitForFunction(() =>
      window.__shellMessages?.some((message) =>
        message?.type === "home:deliver-to-target" &&
        message?.target === "browser" &&
        message?.payload?.type === "browser:file-picker-selection" &&
        message?.payload?.fileName === "Viewer.md" &&
        message?.payload?.sizeBytes > 0),
    );

    await page.evaluate(() => {
      window.__shellMessages = [];
    });
    await libraryFrame.goto(`http://127.0.0.1:${port}/apps/library/?home_token=${encodeURIComponent(token)}&mode=archive-open&returnTarget=archive-manager`);
    await libraryFrame.locator("#picker-action-button").filter({ hasText: "Open in Archive" }).first().waitFor();
    await libraryFrame.waitForFunction(() => document.querySelector("#status-text")?.classList.contains("hidden"));
    const pickerZipItem = libraryFrame.locator(".item").filter({ hasText: "Loose.zip" }).first();
    await pickerZipItem.click();
    await pickerZipItem.locator(".item-name").dblclick();
    await page.waitForFunction(() =>
      window.__shellMessages?.some((message) =>
        message?.type === "home:deliver-to-target" &&
        message?.target === "archive-manager" &&
        message?.payload?.type === "archive:open-library-object" &&
        message?.payload?.object?.uri?.endsWith("/Loose.zip")),
    );

    const archivePage = await context.newPage();
    await archivePage.goto(
      `http://127.0.0.1:${port}/apps/archive-manager/?home_token=${encodeURIComponent(token)}&objectUri=${encodeURIComponent(`${documentsUri}/Portable.zip`)}&name=Portable.zip`,
    );
    await archivePage.locator("#entry-list").filter({ hasText: "Nested/deep.txt" }).first().waitFor();
    await archivePage.locator("#entry-list").filter({ hasText: "../escape.txt" }).first().waitFor();
    await archivePage.locator("#entries-pill").filter({ hasText: "3 entries" }).first().waitFor();
    await archivePage.locator("#destination-roots").filter({ hasText: "Documents" }).first().waitFor();
    await archivePage.locator("#entry-list").filter({ hasText: "Nested/deep.txt" }).first().click();
    await archivePage.locator("#entry-preview").filter({ hasText: "zip nested" }).first().waitFor();
    await archivePage.locator("#entry-search").fill("deep");
    await archivePage.locator("#entry-list").filter({ hasText: "Nested/deep.txt" }).first().waitFor();
    assert(
      await archivePage.locator("#entry-list").filter({ hasText: "alpha.txt" }).count() === 0,
      "Archive search must filter entry rows",
    );
    await archivePage.locator("#extract-all").waitFor();
    await archivePage.locator("#select-all-safe").click();
    await archivePage.locator("#extract-selected").click();
    await archivePage.locator("#extract-status").filter({ hasText: "1 written" }).first().waitFor();
    await archivePage.close();
    const archiveBlankPage = await context.newPage();
    await archiveBlankPage.goto(
      `http://127.0.0.1:${port}/apps/archive-manager/?home_token=${encodeURIComponent(token)}`,
    );
    await archiveBlankPage.locator("#open-existing-archive").click();
    await archiveBlankPage.waitForURL((url) =>
      url.pathname === "/apps/library/" &&
        url.searchParams.get("mode") === "archive-open" &&
        url.searchParams.get("returnTarget") === "archive-manager",
    );
    await archiveBlankPage.locator("#picker-action-button").filter({ hasText: "Open in Archive" }).first().waitFor();
    await archiveBlankPage.goto(
      `http://127.0.0.1:${port}/apps/archive-manager/?home_token=${encodeURIComponent(token)}`,
    );
    await archiveBlankPage.locator("#make-new-archive").click();
    await archiveBlankPage.waitForURL((url) =>
      url.pathname === "/apps/library/" &&
        url.searchParams.get("mode") === "archive-create" &&
        url.searchParams.get("returnTarget") === "archive-manager",
    );
    await archiveBlankPage.locator("#picker-action-button").filter({ hasText: "Create ZIP" }).first().waitFor();
    await archiveBlankPage.close();
    const archiveMessagePage = await context.newPage();
    await archiveMessagePage.goto(
      `http://127.0.0.1:${port}/apps/archive-manager/?home_token=${encodeURIComponent(token)}`,
    );
    await archiveMessagePage.evaluate((uri) => {
      window.postMessage({
        type: "archive:open-library-object",
        object: {
          uri,
          name: "Portable.zip",
          mime: "application/zip",
        },
      }, window.location.origin);
    }, `${documentsUri}/Portable.zip`);
    await archiveMessagePage.locator("#entry-list").filter({ hasText: "Nested/deep.txt" }).first().waitFor();
    await archiveMessagePage.close();
    assert(
      ops.some((entry) => entry.op === "roots"),
      "Archive destination picker must load roots through the Runtime viewer route",
    );
    assert(
      ops.some((entry) => entry.op === "archive_entries" && entry.payload.uri.endsWith("/Portable.zip")),
      "Archive must load entries through the Runtime viewer route archive_entries bridge",
    );
    assert(
      ops.some((entry) =>
        entry.op === "archive_preview_entry" &&
        entry.payload.uri.endsWith("/Portable.zip") &&
        entry.payload.entry === "Nested/deep.txt"),
      "Archive must preview entries through the Runtime viewer route archive_preview_entry bridge",
    );
    assert(
      ops.some((entry) =>
        entry.op === "archive_extract_entries" &&
        entry.payload.uri.endsWith("/Portable.zip") &&
        entry.payload.destination_uri === documentsUri &&
        entry.payload.entries?.includes("Nested/deep.txt")),
      "Archive Extract selected must use the Runtime viewer route archive_extract_entries bridge",
    );

    console.log("PASS Library menu smoke");
  } finally {
    await context?.close().catch(() => {});
    await browser.close().catch(() => {});
    await new Promise((resolve) => server.close(resolve));
  }
}

run().catch((error) => {
  console.error(error);
  process.exit(1);
});
