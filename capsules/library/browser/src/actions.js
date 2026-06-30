import {
  archiveLibraryObjectPayload,
  baseName,
  canPreviewObject,
  childUri,
  contentCid,
  hasCapability,
  inTrash,
  isArchiveObject,
  isBlockedObject,
  isDirectory,
  isTrashRootUri,
  isTrashUri,
  isWebSpaceUri,
  parentUri,
  publishedCid,
  viewerOptions,
} from "./model.js";

export function createLibraryActions({
  clearSelection,
  closeSelf,
  confirmDestructive,
  currentFolderReadOnly,
  deliverToTarget,
  downloadObjectRaw,
  loadCurrentFolder,
  loadRoots,
  navigate,
  openPublishedUri,
  openTarget,
  previewObject,
  providerApi,
  renderUploads,
  selectedObjects,
  setStatus,
  setUploadProgress,
  showMenuForObject,
  showObjectStatus,
  showProperties,
  showShareDialog,
  showShareReceipt,
  showSharedAccessReceipt,
  startCreateObject,
  state,
  uploadObject,
}) {
  async function openObject(object) {
    if (!object) return;
    if (isDirectory(object)) {
      await navigate(object.uri);
      return;
    }
    if (isBlockedObject(object)) {
      setStatus("This object is blocked because it is not encrypted for the protected principal root.");
      showProperties(object);
      return;
    }
    if (isAttachMode()) {
      await attachObject(object);
      return;
    }
    if (isArchiveOpenMode()) {
      deliverArchiveObject(object);
      return;
    }
    if (isArchiveObject(object) && openWithViewer(object, "archive-manager")) {
      return;
    }
    const viewer = viewerOptions(object)[0];
    if (viewer && openWithViewer(object, viewer.id)) {
      return;
    }
    if (canPreviewObject(object)) {
      await previewObject(object);
      return;
    }
    setStatus("No installed viewer for this object.");
    showProperties(object);
  }

  function openWithViewer(object, viewer) {
    if (!viewer) return false;
    const cid = publishedCid(object);
    if (object.published && cid) {
      openPublishedUri("elastos://" + cid, viewer);
      return true;
    }
    const query = {
      objectUri: object.uri,
      uri: object.uri,
      name: object.name || "",
      mime: object.mime || "application/octet-stream",
    };
    const localCid = contentCid(object);
    if (localCid) query.contentCid = localCid;
    if (viewer === "archive-manager" && object.metadata?.archive_support) {
      query.archiveSupport = JSON.stringify(object.metadata.archive_support);
    }
    return openTarget(viewer, query);
  }

  async function attachObject(object) {
    const targetLabel = state.returnTarget === "browser" ? "Browser" : "Chat Room";
    setStatus("Preparing attachment...");
    try {
      const raw = await downloadObjectRaw({ uri: object.uri });
      const cid = publishedCid(object);
      const payload = {
        type: state.returnTarget === "browser"
          ? "browser:file-picker-selection"
          : "chat-room:attach-library-item",
        blob: raw.blob,
        fileName: object.name || raw.filename || "Library item",
        mimeType: object.mime || raw.blob?.type || "application/octet-stream",
        sizeBytes: raw.blob?.size || object.size || 0,
        title: object.name || "",
        objectUri: object.uri,
      };
      if (state.returnTarget === "chat-room" && object.published && cid) {
        payload.publishedUri = "elastos://" + cid;
      }
      if (deliverToTarget(state.returnTarget, payload)) {
        setStatus(`Attached to ${targetLabel}.`);
        window.setTimeout(closeSelf, 80);
        return;
      }
      setStatus(`Open ${targetLabel} from Home.`);
    } catch (error) {
      setStatus(error?.message || "Could not attach this Library item.");
    }
  }

  function deliverArchiveObject(object) {
    if (!isArchiveObject(object)) {
      setStatus("Select a ZIP, tar, tar.gz, or tgz archive.");
      return false;
    }
    const payload = {
      type: "archive:open-library-object",
      object: archiveLibraryObjectPayload(object),
    };
    if (deliverToTarget("archive-manager", payload)) {
      setStatus("Opening in Archive.");
      window.setTimeout(closeSelf, 80);
      return true;
    }
    if (openWithViewer(object, "archive-manager")) {
      return true;
    }
    setStatus("Open Archive from Home, then choose this archive again.");
    return false;
  }

  async function createFolder() {
    if (currentFolderReadOnly()) {
      setStatus("This Space is read-only.");
      return;
    }
    startCreateObject("directory");
  }

  async function uploadFiles(files) {
    const list = Array.from(files || []);
    if (!list.length) return;
    if (currentFolderReadOnly()) {
      setStatus("This Space is read-only.");
      return;
    }
    const batch = Date.now().toString(36);
    state.uploads = list.map((file, index) => ({
      id: `${batch}:${index}`,
      name: file.name,
      progress: 0,
      status: "Queued",
    }));
    renderUploads();
    try {
      for (let index = 0; index < list.length; index += 1) {
        const file = list[index];
        const upload = state.uploads[index];
        try {
          setStatus(`Uploading ${file.name}...`);
          setUploadProgress(upload.id, { status: "Preparing", progress: 4 });
          await uploadObject({
            uri: childUri(state.currentUri, file.name),
            file,
            mime: file.type || "application/octet-stream",
            onProgress: (fraction) => {
              setUploadProgress(upload.id, {
                status: "Uploading through Runtime provider",
                progress: Math.max(4, Math.round(fraction * 96)),
              });
            },
          });
          setUploadProgress(upload.id, {
            status: "Committing through Runtime provider",
            progress: 98,
          });
          setUploadProgress(upload.id, { status: "Complete", progress: 100 });
        } catch (error) {
          setUploadProgress(upload.id, { status: error?.message || "Upload failed" });
          throw error;
        }
      }
      setStatus(`Uploaded ${list.length} file${list.length === 1 ? "" : "s"}.`);
      await loadCurrentFolder();
    } finally {
      window.setTimeout(() => {
        if (state.uploads.every((upload) => upload.progress >= 100)) {
          state.uploads = [];
          renderUploads();
        }
      }, 1_200);
    }
  }

  async function downloadObject(object) {
    const data = await downloadObjectRaw({ uri: object.uri });
    saveDownloadBlob(data.blob, data.filename || object.name || "download");
  }

  async function downloadObjectAsZip(object) {
    if (!isDirectory(object)) return;
    const data = await downloadObjectRaw({ uri: object.uri, archive: "zip" });
    saveDownloadBlob(data.blob, data.filename || `${object.name || "Library"}.zip`);
    setStatus(`Downloaded ${object.name} as ZIP.`);
  }

  async function downloadSelectedObjects() {
    const objects = downloadableSelectedObjects();
    if (objects.length < 2) return;
    const data = await downloadObjectRaw({ uris: objects.map((object) => object.uri) });
    saveDownloadBlob(data.blob, data.filename || "Library Selection.tar.gz");
    setStatus(`Downloaded ${objects.length} selected objects.`);
  }

  async function downloadSelectedObjectsAsZip() {
    const objects = downloadableSelectedObjects();
    if (objects.length < 2) return;
    const data = await downloadObjectRaw({
      uris: objects.map((object) => object.uri),
      archive: "zip",
    });
    saveDownloadBlob(data.blob, data.filename || "Library Selection.zip");
    setStatus(`Downloaded ${objects.length} selected objects as ZIP.`);
  }

  async function compressObjectToZip(object) {
    if (!object || !hasCapability(object, "compress_archive")) return;
    setStatus(`Compressing ${object.name}...`);
    await providerApi("compress_archive", { uri: object.uri, if_revision: object.revision });
    setStatus(`Compressed ${object.name} to ZIP.`);
    await loadCurrentFolder();
  }

  async function compressSelectedObjectsToZip() {
    const objects = compressibleSelectedObjects();
    if (objects.length < 2) return;
    setStatus(`Compressing ${objects.length} selected objects...`);
    await providerApi("compress_archive", { uris: objects.map((object) => object.uri) });
    setStatus(`Compressed ${objects.length} selected objects to ZIP.`);
    await loadCurrentFolder();
  }

  async function extractArchiveObject(object) {
    setStatus(`Extracting ${object.name}...`);
    await providerApi("extract_archive", { uri: object.uri, if_revision: object.revision });
    setStatus(`Extracted ${object.name}.`);
    await loadCurrentFolder();
  }

  function downloadableSelectedObjects() {
    return selectedObjects().filter((object) => (
      object &&
      !isBlockedObject(object) &&
      !inTrash(object) &&
      !isWebSpaceUri(object.uri) &&
      hasCapability(object, "download")
    ));
  }

  function compressibleSelectedObjects() {
    const objects = selectedObjects();
    const compressible = objects.filter((object) => (
      object &&
      !isBlockedObject(object) &&
      !inTrash(object) &&
      !isWebSpaceUri(object.uri) &&
      hasCapability(object, "compress_archive")
    ));
    if (compressible.length !== objects.length || compressible.length < 2) return [];
    const parent = parentUri(compressible[0].uri);
    return compressible.every((object) => parentUri(object.uri) === parent) ? compressible : [];
  }

  function saveDownloadBlob(blob, filename) {
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = filename;
    document.body.appendChild(link);
    link.click();
    link.remove();
    URL.revokeObjectURL(url);
  }

  async function publishObject(object) {
    setStatus(`Publishing ${object.name}...`);
    await providerApi("publish", { uri: object.uri, if_revision: object.revision });
    setStatus(`Published ${object.name}.`);
    await loadCurrentFolder();
  }

  async function unpublishObject(object) {
    setStatus(`Unpublishing ${object.name}...`);
    await providerApi("unpublish", { uri: object.uri, if_revision: object.revision });
    setStatus(`Unpublished ${object.name}.`);
    await loadCurrentFolder();
  }

  async function repairObject(object) {
    setStatus(`Repairing availability for ${object.name}...`);
    await providerApi("repair", { uri: object.uri });
    setStatus(`Repaired availability for ${object.name}.`);
    await loadCurrentFolder();
  }

  async function showStatusObject(object) {
    setStatus(`Checking availability for ${object.name}...`);
    const status = await providerApi("status", { uri: object.uri });
    showObjectStatus(status);
    setStatus(`Availability ready for ${object.name}.`);
  }

  async function shareObject(object) {
    const decision = await showShareDialog(object);
    if (!decision) return;
    const payload = { uri: object.uri, policy: decision.policy };
    if (decision.policy === "recipient_scoped") {
      payload.recipients = decision.recipients;
    }
    const share = await providerApi("share", payload);
    await copyText(share.uri, "share URI");
    showShareReceipt(share);
    await loadCurrentFolder();
  }

  async function checkMyAccess(object) {
    const principalId = principalIdFromHomeToken(state.homeToken);
    if (!principalId) {
      setStatus("A signed Home principal is required to check recipient access.");
      return;
    }
    setStatus(`Checking access for ${object.name}...`);
    const access = await providerApi("shared_access", {
      uri: object.uri,
      recipient: principalId,
    });
    showSharedAccessReceipt(access);
    setStatus(`Access check ready for ${object.name}.`);
  }

  async function trashObject(object) {
    await providerApi("trash", { uri: object.uri, if_revision: object.revision });
    setStatus(`Moved ${object.name} to Trash.`);
    await loadRoots?.();
    await loadCurrentFolder();
  }

  async function restoreObject(object) {
    const name = object.name || baseName(object.uri);
    await providerApi("restore", { uri: object.uri, if_revision: object.revision });
    setStatus(`Restored ${name}.`);
    await loadRoots?.();
    await loadCurrentFolder();
  }

  async function deleteObject(object) {
    const trash = inTrash(object);
    const confirmed = await confirmDestructive({
      title: "Delete permanently?",
      message: trash
        ? `${object.name} will be removed from Trash and cannot be restored.`
        : `${object.name} will be permanently removed from this provider-owned location. This cannot be restored.`,
      confirmLabel: "Delete Permanently",
    });
    if (!confirmed) return;
    await providerApi("delete_permanently", { uri: object.uri, if_revision: object.revision });
    setStatus(`Deleted ${object.name}.`);
    await loadRoots?.();
    await loadCurrentFolder();
  }

  async function runBatchAction(label, objects, action) {
    const list = objects.filter(Boolean);
    if (!list.length) return;
    setStatus(`${label} ${list.length} object${list.length === 1 ? "" : "s"}...`);
    for (const object of list) {
      await action(object);
    }
    clearSelection(false);
    setStatus(`${label} ${list.length} object${list.length === 1 ? "" : "s"}.`);
    await loadCurrentFolder();
  }

  async function publishSelectedObjects() {
    const objects = selectedObjects().filter((object) => !isDirectory(object) && !inTrash(object) && !object.published && hasCapability(object, "publish"));
    await runBatchAction("Published", objects, (object) => providerApi("publish", {
      uri: object.uri,
      if_revision: object.revision,
    }));
  }

  async function unpublishSelectedObjects() {
    const objects = selectedObjects().filter((object) => !isDirectory(object) && !inTrash(object) && object.published && hasCapability(object, "unpublish"));
    await runBatchAction("Unpublished", objects, (object) => providerApi("unpublish", {
      uri: object.uri,
      if_revision: object.revision,
    }));
  }

  async function trashSelectedObjects() {
    const objects = selectedObjects().filter((object) => !inTrash(object) && hasCapability(object, "trash"));
    await runBatchAction("Moved to Trash", objects, (object) => providerApi("trash", {
      uri: object.uri,
      if_revision: object.revision,
    }));
    await loadRoots?.();
  }

  function setClipboard(op, objects) {
    const uris = objects
      .filter((object) => object && !isBlockedObject(object) && !inTrash(object) && hasCapability(object, op))
      .map((object) => object.uri);
    state.clipboard = {
      op,
      uris,
    };
    setStatus(`${op === "move" ? "Cut" : "Copied"} ${uris.length} object${uris.length === 1 ? "" : "s"}.`);
  }

  function canPasteInto(targetParentUri) {
    return !!(
      targetParentUri &&
      !isWebSpaceUri(targetParentUri) &&
      state.clipboard.uris.length &&
      !isTrashRootUri(targetParentUri) &&
      !isTrashUri(targetParentUri) &&
      (state.clipboard.op === "copy" || state.clipboard.op === "move")
    );
  }

  async function pasteClipboardTo(targetParentUri) {
    if (!canPasteInto(targetParentUri)) return;
    const uris = [...state.clipboard.uris];
    const op = state.clipboard.op;
    for (const uri of uris) {
      if (targetParentUri === uri || targetParentUri.startsWith(uri + "/")) {
        continue;
      }
      await providerApi(op === "move" ? "move" : "copy", {
        uri,
        target_parent_uri: targetParentUri,
      });
    }
    if (op === "move") {
      state.clipboard = { op: "", uris: [] };
    }
    setStatus(`${op === "move" ? "Moved" : "Copied"} ${uris.length} object${uris.length === 1 ? "" : "s"}.`);
    await loadCurrentFolder();
  }

  async function transferSelectedObjectsTo(targetParentUri, op) {
    if (!targetParentUri || isWebSpaceUri(targetParentUri) || isTrashRootUri(targetParentUri) || isTrashUri(targetParentUri)) return;
    const capability = op === "move" ? "move" : "copy";
    const objects = selectedObjects().filter((object) => (
      object &&
      !isBlockedObject(object) &&
      !inTrash(object) &&
      hasCapability(object, capability)
    ));
    let changed = 0;
    for (const object of objects) {
      if (targetParentUri === object.uri || targetParentUri.startsWith(object.uri + "/")) {
        continue;
      }
      if (op === "move" && parentUri(object.uri) === targetParentUri) {
        continue;
      }
      await providerApi(op, {
        uri: object.uri,
        target_parent_uri: targetParentUri,
        if_revision: object.revision,
      });
      changed += 1;
    }
    if (changed) {
      setStatus(`${op === "move" ? "Moved" : "Copied"} ${changed} object${changed === 1 ? "" : "s"}.`);
      await loadCurrentFolder();
    }
  }

  async function moveSelectedObjectsTo(targetParentUri) {
    await transferSelectedObjectsTo(targetParentUri, "move");
  }

  async function copySelectedObjectsTo(targetParentUri) {
    await transferSelectedObjectsTo(targetParentUri, "copy");
  }

  async function createTextDocument() {
    if (currentFolderReadOnly()) {
      setStatus("This Space is read-only.");
      return;
    }
    startCreateObject("file");
  }

  async function restoreSelectedObjects() {
    const objects = selectedObjects().filter(inTrash);
    await runBatchAction("Restored", objects, (object) => providerApi("restore", {
      uri: object.uri,
      if_revision: object.revision,
    }));
    await loadRoots?.();
  }

  async function deleteSelectedObjects() {
    const objects = selectedObjects().filter((object) => (
      inTrash(object) || (!inTrash(object) && hasCapability(object, "delete_permanently"))
    ));
    if (!objects.length) return;
    const allTrash = objects.every(inTrash);
    const confirmed = await confirmDestructive({
      title: "Delete permanently?",
      message: allTrash
        ? `${objects.length} object${objects.length === 1 ? "" : "s"} will be removed from Trash and cannot be restored.`
        : `${objects.length} object${objects.length === 1 ? "" : "s"} will be permanently removed from provider-owned locations. This cannot be restored.`,
      confirmLabel: "Delete Permanently",
    });
    if (!confirmed) return;
    await runBatchAction("Deleted", objects, (object) => providerApi("delete_permanently", {
      uri: object.uri,
      if_revision: object.revision,
    }));
    await loadRoots?.();
  }

  async function emptyTrash() {
    const confirmed = await confirmDestructive({
      title: "Empty Trash?",
      message: "Every object in Trash will be permanently deleted. This cannot be restored.",
      confirmLabel: "Empty Trash",
    });
    if (!confirmed) return;
    const result = await providerApi("empty_trash", {});
    const count = Number(result?.deleted_count || 0);
    setStatus(`Emptied Trash${count ? ` (${count} object${count === 1 ? "" : "s"})` : ""}.`);
    await loadRoots?.();
    await loadCurrentFolder();
  }

  async function copyText(value, label) {
    await navigator.clipboard.writeText(value);
    setStatus(`Copied ${label}.`);
  }

  function isAttachMode() {
    return (
      state.mode === "attach" &&
      (state.returnTarget === "chat-room" || state.returnTarget === "browser")
    );
  }

  function isArchiveOpenMode() {
    return state.mode === "archive-open" && state.returnTarget === "archive-manager";
  }

  function principalIdFromHomeToken(token) {
    try {
      const decoded = JSON.parse(atob(base64Padding(String(token || "").replace(/-/g, "+").replace(/_/g, "/"))));
      return typeof decoded?.payload?.principal_id === "string"
        ? decoded.payload.principal_id.trim()
        : "";
    } catch {
      return "";
    }
  }

  function base64Padding(value) {
    const remainder = value.length % 4;
    return remainder ? value + "=".repeat(4 - remainder) : value;
  }

  return {
    attachObject,
    canPasteInto,
    checkMyAccess,
    compressObjectToZip,
    compressSelectedObjectsToZip,
    copySelectedObjectsTo,
    copyText,
    createFolder,
    createTextDocument,
    deleteObject,
    deleteSelectedObjects,
    downloadObject,
    downloadObjectAsZip,
    downloadSelectedObjects,
    downloadSelectedObjectsAsZip,
    emptyTrash,
    extractArchiveObject,
    moveSelectedObjectsTo,
    openObject,
    openWithViewer,
    pasteClipboardTo,
    publishObject,
    publishSelectedObjects,
    repairObject,
    restoreObject,
    restoreSelectedObjects,
    setClipboard,
    shareObject,
    showStatusObject,
    trashObject,
    trashSelectedObjects,
    unpublishObject,
    unpublishSelectedObjects,
    uploadFiles,
  };
}
