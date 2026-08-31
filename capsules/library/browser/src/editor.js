import {
  childUri,
  nextObjectName,
} from "./model.js";

export function createLibraryEditor({
  content,
  loadCurrentFolder,
  providerApi,
  renderContent,
  setObjects,
  setStatus,
  showError,
  state,
}) {
  function startCreateObject(kind) {
    const isFolder = kind === "directory";
    const name = isFolder
      ? nextObjectName(state.objects, "New Folder")
      : nextObjectName(state.objects, "New File", ".txt");
    const now = Math.floor(Date.now() / 1000);
    const draft = {
      schema: "elastos.library.object/v1",
      uri: `draft:${++state.draftCounter}:${Date.now()}`,
      name,
      kind: isFolder ? "directory" : "file",
      mime: isFolder ? "inode/directory" : "text/plain",
      size: 0,
      created_at: now,
      modified_at: now,
      revision: "draft",
      viewer: null,
      viewers: [],
      thumbnail_uri: null,
      availability: "draft",
      blocked_reason: null,
      content_cid: null,
      published_cid: null,
      published: false,
      shared: false,
      capabilities: ["rename"],
    };
    setObjects([...state.objects, draft]);
    state.selectedUris = new Set([draft.uri]);
    renderContent();
    setStatus(isFolder ? "Name the new folder." : "Name the new text document.");
    window.requestAnimationFrame(() => {
      startNameEdit(draft, {
        editingStatusText: isFolder ? "Name the new folder." : "Name the new text document.",
        async commit(finalName) {
          if (isFolder) {
            await providerApi("mkdir", { parent_uri: state.currentUri, name: finalName });
          } else {
            await providerApi("write", {
              uri: childUri(state.currentUri, finalName),
              mime: "text/plain",
              create_only: true,
              data: "",
            });
          }
          setStatus(`Created ${finalName}.`);
          await loadCurrentFolder();
        },
        cancel() {
          setObjects(state.objects.filter((object) => object.uri !== draft.uri));
          state.selectedUris.delete(draft.uri);
          renderContent();
          setStatus("Ready.");
        },
      });
    });
  }

  function startRename(object) {
    startNameEdit(object, {
      async commit(name) {
        if (name && name !== object.name) {
          await providerApi("rename", { uri: object.uri, name, if_revision: object.revision });
          setStatus(`Renamed ${object.name} to ${name}.`);
          await loadCurrentFolder();
        } else {
          renderContent();
        }
      },
      cancel() {
        renderContent();
      },
    });
  }

  function startNameEdit(object, handlers) {
    const label = content.querySelector(`[data-name-uri="${CSS.escape(object.uri)}"]`);
    if (!label) return;
    const input = document.createElement("input");
    input.className = "rename-input";
    input.value = object.name || "";
    label.replaceWith(input);
    input.focus();
    input.select();
    let committed = false;
    let failed = false;
    async function commit() {
      if (committed) return;
      committed = true;
      const name = input.value.trim();
      if (!name) {
        handlers.cancel();
        return;
      }
      try {
        await handlers.commit(name);
      } catch (error) {
        committed = false;
        failed = true;
        throw error;
      }
    }
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        commit().catch(showError);
      }
      if (event.key === "Escape") {
        committed = true;
        handlers.cancel();
      }
    });
    input.addEventListener("input", () => {
      if (failed && handlers.editingStatusText) {
        failed = false;
        setStatus(handlers.editingStatusText);
      }
    });
    input.addEventListener("blur", () => commit().catch(showError));
  }

  return {
    startCreateObject,
    startRename,
  };
}
