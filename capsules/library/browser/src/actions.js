import {
  archiveLibraryObjectPayload,
  baseName,
  canPublishObject,
  canPreviewObject,
  childUri,
  contentCid,
  decimalIntegerToHexQuantity,
  hasCapability,
  inTrash,
  isArchiveObject,
  isBlockedObject,
  isDirectory,
  isRuntimeCustodyProtectableVideo,
  isTrashRootUri,
  isTrashUri,
  isWebSpaceUri,
  libraryErrorText,
  parentUri,
  publishFailureText,
  publishedLink,
  viewerOptions,
} from "./model.js";

export function createLibraryActions({
  clearSelection,
  closeSelf,
  confirmDestructive,
  confirmPublish,
  currentFolderReadOnly,
  deliverToTarget,
  downloadObjectRaw,
  loadCurrentFolder,
  loadRoots,
  navigate,
  offerDownload,
  openTarget,
  previewObject,
  providerApi,
  renderUploads,
  selectedObjects,
  setStatus,
  setUploadProgress,
  showMenuForObject,
  showObjectStatus,
  showProtectAndListDialog,
  showProperties,
  showPublishedLink,
  showSharedAccessReceipt,
  startCreateObject,
  state,
  uploadObject,
  writeResourceIdentifier,
  writeResourceUri,
}) {
  const RUNTIME_CUSTODY_METADATA_SCHEMA = "elastos.library.protected-content-identity/v1";
  // Chat accepts files up to 8 MiB; Library does not read a larger file for it.
  const CHAT_ATTACHMENT_MAX_BYTES = 8 * 1024 * 1024;
  const MINT_ID_HEX_RE = /^[0-9a-f]{64}$/;
  const protectAndListInFlight = new Map();
  const activeUploads = new Set();
  let uploadBatchId = 0;

  async function openObject(object) {
    if (!object) return;
    if (isDirectory(object)) {
      await navigate(object.uri);
      return;
    }
    if (isBlockedObject(object)) {
      setStatus("This item is unavailable because it is not protected for this account.");
      showProperties(object);
      return;
    }
    if (isAttachMode()) {
      await attachObject(object);
      return;
    }
    if (isArchiveOpenMode()) {
      await deliverArchiveObject(object);
      return;
    }
    if (isRuntimeCustodyProtectedVideo(object)) {
      if (openProtectedVideo(object)) {
        return;
      }
      setStatus("Protected video is unavailable.");
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
    if (hasCapability(object, "download")) {
      if (await offerDownload(object)) {
        await downloadObject(object);
        setStatus(`Downloaded ${object.name}.`);
      }
      return;
    }
    setStatus("No installed app can open this item.");
    showProperties(object);
  }

  function isRuntimeCustodyProtectedVideo(object) {
    return (
      String(object?.mime || "").startsWith("video/") &&
      object?.metadata?.protected_content?.schema === RUNTIME_CUSTODY_METADATA_SCHEMA
    );
  }

  function openProtectedVideo(object) {
    const mintId = String(object?.metadata?.protected_content?.mint_id || "").trim();
    if (!MINT_ID_HEX_RE.test(mintId)) {
      return false;
    }
    return openTarget("elacity-player", { mint_id: mintId });
  }

  function openWithViewer(object, viewer) {
    if (!viewer) return false;
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

  // Chat's Community posts a Library file by its address and revision, so it
  // gets no bytes for a file over the Chat limit; a local room still sends the
  // bytes it is given. Browser always gets the file's bytes.
  async function attachObject(object) {
    const targetLabel = state.returnTarget === "browser" ? "Browser" : "Chat Room";
    setStatus("Preparing attachment...");
    try {
      const chat = state.returnTarget === "chat-room";
      const raw = chat && Number(object.size || 0) > CHAT_ATTACHMENT_MAX_BYTES
        ? null
        : await downloadObjectRaw({ uri: object.uri });
      const payload = {
        type: chat ? "chat-room:attach-library-item" : "browser:file-picker-selection",
        fileName: object.name || raw?.filename || "Library item",
        mimeType: object.mime || raw?.blob?.type || "application/octet-stream",
        sizeBytes: raw?.blob?.size ?? Number(object.size || 0),
        title: object.name || "",
        objectUri: object.uri,
      };
      if (raw) payload.blob = raw.blob;
      if (chat) payload.revision = String(object.revision || "");
      if (await deliverToTarget(state.returnTarget, payload)) {
        setStatus(`Attached to ${targetLabel}.`);
        closeSelf();
        return;
      }
      setStatus(`${targetLabel} did not confirm this item. Check it before trying again.`);
    } catch (error) {
      setStatus(error?.message || "Could not attach this Library item.");
    }
  }

  async function deliverArchiveObject(object) {
    if (!isArchiveObject(object)) {
      setStatus("Select a ZIP, tar, tar.gz, or tgz archive.");
      return false;
    }
    const payload = {
      type: "archive:open-library-object",
      object: archiveLibraryObjectPayload(object),
    };
    if (await deliverToTarget("archive-manager", payload)) {
      setStatus("Opening in Archive.");
      closeSelf();
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
    const destinationUri = state.currentUri;
    const batch = ++uploadBatchId;
    const uploads = list.map((file, index) => ({
      id: `${batch}:${index}`,
      name: file.name,
      progress: 0,
      status: "Queued",
    }));
    // A new selection replaces old results while active batches keep their rows.
    state.uploads = state.uploads.filter((upload) => activeUploads.has(upload)).concat(uploads);
    for (const upload of uploads) activeUploads.add(upload);
    renderUploads();
    try {
      for (let index = 0; index < list.length; index += 1) {
        const file = list[index];
        const upload = uploads[index];
        try {
          setStatus(`Uploading ${file.name}...`);
          setUploadProgress(upload.id, { status: "Preparing", progress: 4 });
          await uploadObject({
            uri: childUri(destinationUri, file.name),
            file,
            mime: file.type || "application/octet-stream",
            onProgress: (fraction) => {
              setUploadProgress(upload.id, {
                status: "Uploading",
                progress: Math.max(4, Math.round(fraction * 96)),
              });
            },
          });
          setUploadProgress(upload.id, {
            status: "Saving",
            progress: 98,
          });
          setUploadProgress(upload.id, { status: "Complete", progress: 100 });
        } catch (error) {
          setUploadProgress(upload.id, {
            status: "Upload not confirmed. Check the destination before retrying.",
          });
          for (const pending of uploads.slice(index + 1)) {
            setUploadProgress(pending.id, { status: "Not started" });
          }
          throw error;
        }
      }
      const completedStatus = `Uploaded ${list.length} file${list.length === 1 ? "" : "s"}.`;
      try {
        await loadCurrentFolder();
        setStatus(completedStatus);
      } catch {
        setStatus(`${completedStatus} Refresh Library to see the latest items.`);
      }
    } finally {
      for (const upload of uploads) activeUploads.delete(upload);
      window.setTimeout(() => {
        if (uploads.every((upload) => upload.progress >= 100)) {
          const completedIds = new Set(uploads.map((upload) => upload.id));
          state.uploads = state.uploads.filter((upload) => !completedIds.has(upload.id));
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
    setStatus(`Downloaded ${objects.length} selected items.`);
  }

  async function downloadSelectedObjectsAsZip() {
    const objects = downloadableSelectedObjects();
    if (objects.length < 2) return;
    const data = await downloadObjectRaw({
      uris: objects.map((object) => object.uri),
      archive: "zip",
    });
    saveDownloadBlob(data.blob, data.filename || "Library Selection.zip");
    setStatus(`Downloaded ${objects.length} selected items as ZIP.`);
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
    setStatus(`Compressing ${objects.length} selected items...`);
    await providerApi("compress_archive", { uris: objects.map((object) => object.uri) });
    setStatus(`Compressed ${objects.length} selected items to ZIP.`);
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

  // A shared file is in the local content store, not a Library folder. Read it
  // and save it like any Library download: a link to /content would navigate
  // this frame, and the gateway refuses that navigation. The gateway serves
  // content only to a request carrying this Home's token.
  async function downloadSharedFile(cid, name) {
    const response = await fetch(`/content/${encodeURIComponent(cid)}`, {
      cache: "no-store",
      headers: { "x-elastos-home-token": state.homeToken || "" },
    });
    if (!response.ok) throw new Error("Could not read the shared file. Open the link in Chat again.");
    saveDownloadBlob(await response.blob(), name || cid);
  }

  async function protectAndListObject(object) {
    if (!isRuntimeCustodyProtectableVideo(object)) return;
    const operationKey = String(object.uri || "");
    const existing = protectAndListInFlight.get(operationKey);
    if (existing) {
      setStatus(`Protection for ${object.name} is already in progress.`);
      return existing;
    }
    const operation = (async () => {
      const decision = await showProtectAndListDialog(object);
      if (!decision) return;
      const copies = decimalIntegerToHexQuantity(decision.copies);
      const price = decimalIntegerToHexQuantity(decision.price);
      setStatus(
        `Protecting ${object.name} as ${decision.copies} ${decision.copies === "1" ? "copy" : "copies"} at ${decision.price} base units...`,
      );
      try {
        await providerApi("publish", {
          uri: object.uri,
          if_revision: object.revision,
          protection: {
            mode: "runtime_custody",
            copies,
            price,
          },
        });
      } catch (error) {
        setStatus(protectAndListFailureStatus(object, error));
        return;
      }
      setStatus(`Protected and listed ${object.name}.`);
      await loadCurrentFolder();
    })();
    protectAndListInFlight.set(operationKey, operation);
    try {
      return await operation;
    } finally {
      if (protectAndListInFlight.get(operationKey) === operation) {
        protectAndListInFlight.delete(operationKey);
      }
    }
  }

  function protectAndListFailureStatus(object, error) {
    const message = String(error?.message || error || "");
    if (/pending/i.test(message)) {
      return `Protection for ${object.name} is pending approval or confirmation.`;
    }
    if (/denied|forbidden|unauthorized|requires verified/i.test(message)) {
      return `Protection for ${object.name} was denied.`;
    }
    if (/unavailable|not configured|missing/i.test(message)) {
      return `Protection for ${object.name} is unavailable.`;
    }
    return `Protection for ${object.name} failed.`;
  }

  async function unpublishObject(object) {
    setStatus(`Stopping sharing ${object.name}...`);
    await providerApi("unpublish", { uri: object.uri, if_revision: object.revision });
    setStatus(`Stopped sharing ${object.name}.`);
    await loadCurrentFolder();
  }

  async function publishObject(object) {
    if (!canPublishObject(object)) return;
    if (!await confirmPublish({ files: [object], excluded: [] })) return;
    setStatus(`Sharing ${object.name}...`);
    try {
      await providerApi("publish", { uri: object.uri, if_revision: object.revision });
    } catch (error) {
      setStatus(publishFailureText(error, object.name));
      return;
    }
    await showAfterReload(`Shared ${object.name} with your contacts.`);
  }

  // Reloading the folder writes its own status, so a result shows after it.
  async function showAfterReload(status) {
    try {
      await loadCurrentFolder();
    } finally {
      setStatus(status);
    }
  }

  // One confirmation for the whole selection: it lists the files that will be
  // published and the items left out (folders, files already published).
  async function publishSelectedObjects() {
    const selection = selectedObjects().filter(Boolean);
    const files = selection.filter(canPublishObject);
    if (!files.length) {
      setStatus("Select files to share. Folders can't be shared.");
      return;
    }
    const excluded = selection.filter((object) => !files.includes(object));
    if (!await confirmPublish({ files, excluded })) return;
    const failed = [];
    for (const object of files) {
      setStatus(`Sharing ${object.name}...`);
      try {
        await providerApi("publish", { uri: object.uri, if_revision: object.revision });
      } catch (error) {
        failed.push({ object, text: publishFailureText(error, object.name) });
      }
    }
    clearSelection(false);
    const published = files.length - failed.length;
    const count = (value) => `${value} file${value === 1 ? "" : "s"}`;
    if (!failed.length) {
      await showAfterReload(`Shared ${count(published)} with your contacts.`);
      return;
    }
    const details = failed.map(({ object, text }) => `${object.name}: ${text}`).join(" ");
    await showAfterReload(`Shared ${published} of ${count(files.length)}. ${details}`);
  }

  // Copies the recorded publication link. When the clipboard refuses, the link
  // shows as selectable text so it can still be copied by hand.
  async function copyPublishedLink(object) {
    const link = publishedLink(object);
    if (!object?.published || !link) return;
    try {
      await writeResourceUri(link);
    } catch {
      showPublishedLink({ name: object.name, link });
      setStatus("Copying didn't work. Select the link to copy it.");
      return;
    }
    setStatus(`Copied the link to ${object.name}.`);
  }

  // Share again: the existing repair, for a shared file whose last repair
  // could not pin it.
  async function shareAgain(object) {
    setStatus(`Sharing ${object.name} again...`);
    let repaired;
    try {
      repaired = await providerApi("repair", { uri: object.uri });
    } catch (error) {
      setStatus(libraryErrorText(error) || `${object.name} couldn't be shared again. Try again later.`);
      return;
    }
    // Repair answers with the availability it reached; repair_needed means it failed.
    const availability = repaired?.availability;
    const failed = (typeof availability === "string" ? availability : availability?.status) === "repair_needed";
    await showAfterReload(failed
      ? `${object.name} couldn't be shared again. Try again later.`
      : `${object.name} is shared again.`);
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
        : `${object.name} will be permanently removed from this location. This cannot be restored.`,
      confirmLabel: "Delete Permanently",
      targetUris: [object.uri],
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
    setStatus(`${label} ${list.length} item${list.length === 1 ? "" : "s"}...`);
    for (const object of list) {
      await action(object);
    }
    clearSelection(false);
    setStatus(`${label} ${list.length} item${list.length === 1 ? "" : "s"}.`);
    await loadCurrentFolder();
  }

  async function unpublishSelectedObjects() {
    const objects = selectedObjects().filter((object) => !isDirectory(object) && !inTrash(object) && object.published && hasCapability(object, "unpublish"));
    await runBatchAction("Stopped sharing", objects, (object) => providerApi("unpublish", {
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
    setStatus(`${op === "move" ? "Cut" : "Copied"} ${uris.length} item${uris.length === 1 ? "" : "s"}.`);
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
    setStatus(`${op === "move" ? "Moved" : "Copied"} ${uris.length} item${uris.length === 1 ? "" : "s"}.`);
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
      setStatus(`${op === "move" ? "Moved" : "Copied"} ${changed} item${changed === 1 ? "" : "s"}.`);
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
        ? `${describeObjects(objects)} will be removed from Trash and cannot be restored.`
        : `${describeObjects(objects)} will be permanently removed. This cannot be restored.`,
      confirmLabel: "Delete Permanently",
      targetUris: objects.map((object) => object.uri),
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
      message: "Every item in Trash will be permanently deleted. This cannot be restored.",
      confirmLabel: "Empty Trash",
    });
    if (!confirmed) return;
    const result = await providerApi("empty_trash", {});
    const count = Number(result?.deleted_count || 0);
    setStatus(`Emptied Trash${count ? ` (${count} item${count === 1 ? "" : "s"})` : ""}.`);
    await loadRoots?.();
    await loadCurrentFolder();
  }

  async function copyText(value, label, purpose = "resource.uri") {
    if (purpose === "resource.identifier") {
      await writeResourceIdentifier(value);
    } else if (purpose === "resource.uri") {
      await writeResourceUri(value);
    } else {
      throw new Error("Library Clipboard purpose is denied.");
    }
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
    copyPublishedLink,
    copyText,
    createFolder,
    createTextDocument,
    deleteObject,
    deleteSelectedObjects,
    downloadObject,
    downloadObjectAsZip,
    downloadSelectedObjects,
    downloadSelectedObjectsAsZip,
    downloadSharedFile,
    emptyTrash,
    extractArchiveObject,
    moveSelectedObjectsTo,
    openObject,
    openWithViewer,
    pasteClipboardTo,
    protectAndListObject,
    publishObject,
    publishSelectedObjects,
    repairObject,
    restoreObject,
    restoreSelectedObjects,
    setClipboard,
    shareAgain,
    showStatusObject,
    trashObject,
    trashSelectedObjects,
    unpublishObject,
    unpublishSelectedObjects,
    uploadFiles,
  };
}

function describeObjects(objects, limit = 3) {
  const count = `${objects.length} item${objects.length === 1 ? "" : "s"}`;
  const names = objects.slice(0, limit).map((object) => object.name || object.uri);
  const more = objects.length - names.length;
  return `${count} (${names.join(", ")}${more ? ` and ${more} more` : ""})`;
}
