    import { createHomeNavigationClient } from "/apps/home/home-navigation-client.js";
    const params = new URLSearchParams(window.location.search);
    const homeToken = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";
    const homeParentOrigin = params.get("home_origin") || "";
    const homeNavigation = createHomeNavigationClient({ homeToken, homeOrigin: homeParentOrigin });
    let archiveSelectionSequence = 0;
    const libraryPickerDocumentNonce = crypto.randomUUID();
    let libraryPickerRequest = null;
    let objectUri = params.get("objectUri") || params.get("uri") || "";
    const objectName = params.get("name") || objectUri.split("/").pop() || "Archive";
    const launchPolicy = parseArchiveSupport(params.get("archiveSupport"));
    let homeChromeReady = false;
    let lastHomeMenuManifestSignature = "";
    const state = {
      name: objectName,
      uri: objectUri,
      mime: params.get("mime") || "",
      contentCid: params.get("contentCid") || "",
      policy: launchPolicy,
      entries: [],
      roots: [],
      entryStatus: "Not loaded",
      entryQuery: "",
      selectedEntries: new Set(),
      focusedEntry: "",
      preview: null,
      previewStatus: "Select a file",
      extractStatus: "Select files to extract.",
      extracting: false,
    };

    document.querySelector("#open-existing-archive").addEventListener("click", () => openLibraryPicker("open"));
    document.querySelector("#make-new-archive").addEventListener("click", () => openLibraryPicker("create"));
    document.querySelector("#open-archive-button").addEventListener("click", () => openLibraryPicker("open"));
    document.querySelector("#new-archive-button").addEventListener("click", () => openLibraryPicker("create"));
    document.querySelector("#entry-search").addEventListener("input", (event) => {
      state.entryQuery = event.target.value || "";
      renderEntries();
    });
    document.querySelector("#destination-uri").value = parentUri(objectUri);
    document.querySelector("#select-all-safe").addEventListener("click", () => {
      for (const entry of safeVisibleEntries()) state.selectedEntries.add(entry.path);
      renderEntries();
    });
    document.querySelector("#clear-selection").addEventListener("click", () => {
      state.selectedEntries.clear();
      renderEntries();
    });
    document.querySelector("#destination-roots").addEventListener("click", (event) => {
      const button = event.target.closest("[data-destination-uri]");
      if (!button) return;
      document.querySelector("#destination-uri").value = button.dataset.destinationUri || "";
    });
    document.querySelector("#entry-list").addEventListener("click", (event) => {
      const row = event.target.closest(".entry-row");
      if (!row || event.target.matches(".entry-check")) return;
      const path = row.dataset.path || "";
      if (path) selectPreviewEntry(path);
    });
    document.querySelector("#entry-list").addEventListener("keydown", handleEntryKeyboard);
    document.querySelector("#entry-list").addEventListener("change", (event) => {
      if (!event.target.matches(".entry-check")) return;
      if (event.target.checked) {
        state.selectedEntries.add(event.target.value);
      } else {
        state.selectedEntries.delete(event.target.value);
      }
      renderExtractControls();
    });
    document.querySelector("#extract-selected").addEventListener("click", () => {
      extractSelectedEntries().catch((error) => {
        state.extractStatus = "Extract failed: " + error.message;
        renderExtractControls();
      });
    });
    document.querySelector("#extract-all").addEventListener("click", () => {
      extractAllEntries().catch((error) => {
        state.extractStatus = "Extract failed: " + error.message;
        renderExtractControls();
      });
    });
    window.addEventListener("message", handleTrustedHomeMessage);

    announceHomeChrome();
    render(state);
    if (!objectUri) {
      showEmptyState();
      homeNavigation.setQuery({});
    } else {
      showWorkspace();
      hydrateDestinationRoots().catch(() => {});
      const sequence = archiveSelectionSequence;
      hydrateStatOnly().then((loaded) => loaded && sequence === archiveSelectionSequence && hydrateArchiveEntries()).catch((error) => {
        if (sequence !== archiveSelectionSequence) return;
        state.entryStatus = "Could not load this archive: " + error.message;
        state.entries = [];
        renderEntries();
      });
    }

    function canAnnounceHomeChrome() {
      return !!(homeToken && homeParentOrigin && window.top && window.top !== window);
    }

    function announceHomeChrome() {
      if (!canAnnounceHomeChrome()) return;
      if (!homeChromeReady) {
        window.top.postMessage({ type: "home:app-ready", homeToken }, homeParentOrigin);
        homeChromeReady = true;
      }
      syncHomeMenuManifest();
    }

    function syncHomeMenuManifest() {
      if (!canAnnounceHomeChrome() || !homeChromeReady) return;
      const manifest = {
        type: "home:menu-manifest",
        homeToken,
        menus: [
          {
            title: "File",
            items: [
              { label: "Open Archive...", cmd: "open-archive" },
              { label: "New Archive...", cmd: "new-archive" },
              "-",
              { label: "Close Window", cmd: "__close-window" },
            ],
          },
          {
            title: "Edit",
            items: [
              { label: "Select All Safe Files", cmd: "select-all-safe" },
              { label: "Clear Selection", cmd: "clear-selection" },
            ],
          },
        ],
      };
      const signature = JSON.stringify(manifest.menus);
      if (signature === lastHomeMenuManifestSignature) return;
      lastHomeMenuManifestSignature = signature;
      window.top.postMessage(manifest, homeParentOrigin);
    }

    function isTrustedHomeMessage(event) {
      return event.origin === "null" && event.source === window.parent;
    }

    function handleTrustedHomeMessage(event) {
      const data = event.data || {};
      if (data.type === "archive:open-library-object") {
        if (event.origin !== homeParentOrigin || event.source !== window.top) return;
        acceptLibraryPickerObject(data).catch(() => false).then((accepted) => {
          window.top.postMessage({
            type: "home:picker-accepted", homeToken,
            pickerId: data.pickerId, requestId: data.requestId,
            documentNonce: data.documentNonce, deliveryId: data.deliveryId, accepted,
          }, homeParentOrigin);
        });
        return;
      }
      if (!isTrustedHomeMessage(event)) return;
      if (data.type !== "elastos:menu-command" || typeof data.cmd !== "string") return;
      handleHomeMenuCommand(data.cmd);
    }

    async function acceptLibraryPickerObject(data) {
      const request = libraryPickerRequest;
      if (!request || request.consumed || data.requestId !== request.id ||
          data.documentNonce !== libraryPickerDocumentNonce || !data.pickerId || !data.deliveryId) return false;
      request.consumed = true;
      try {
        if (await openLibraryObject(data.object) === false) return false;
        return libraryPickerRequest === request;
      } catch (_error) {
        if (libraryPickerRequest === request) {
          state.entryStatus = "Could not open this Library archive. Choose it again from Library.";
          renderEntries();
        }
        return false;
      } finally {
        if (libraryPickerRequest === request) libraryPickerRequest = null;
      }
    }

    function handleHomeMenuCommand(command) {
      switch (command) {
        case "open-archive":
          document.querySelector("#open-archive-button").click();
          return;
        case "new-archive":
          document.querySelector("#new-archive-button").click();
          return;
        case "select-all-safe":
          document.querySelector("#select-all-safe").click();
          return;
        case "clear-selection":
          document.querySelector("#clear-selection").click();
          return;
        default:
      }
    }

    function showEmptyState() {
      document.querySelector("#empty-state").hidden = false;
      document.querySelector("#archive-workspace").hidden = true;
    }

    function showWorkspace() {
      document.querySelector("#empty-state").hidden = true;
      document.querySelector("#archive-workspace").hidden = false;
    }

    function openLibraryPicker(intent) {
      if (libraryPickerRequest?.consumed) return;
      const query = {
        mode: intent === "create" ? "archive-create" : "archive-open",
        returnTarget: "archive-manager",
      };
      if (homeToken && homeParentOrigin && window.top && window.top !== window) {
        libraryPickerRequest = { id: crypto.randomUUID(), consumed: false };
        window.top.postMessage({
          type: "home:open-target",
          target: "library",
          query,
          homeToken,
          requestId: libraryPickerRequest.id,
          documentNonce: libraryPickerDocumentNonce,
        }, homeParentOrigin);
        return;
      }
      state.entryStatus = "Open Archive from Home to choose a Library item.";
      renderEntries();
    }

    async function openLibraryObject(object) {
      if (!object?.uri) {
        throw new Error("Library did not return an archive object.");
      }
      objectUri = object.uri;
      const sequence = ++archiveSelectionSequence;
      state.name = object.name || object.uri.split("/").pop() || "Archive";
      state.uri = object.uri;
      state.mime = object.mime || "";
      state.contentCid = object.contentCid || object.content_cid || "";
      state.policy = parseArchiveSupport(object.archiveSupport || object.archive_support);
      state.entries = [];
      state.entryStatus = "Loading archive files...";
      state.entryQuery = "";
      state.selectedEntries = new Set();
      state.focusedEntry = "";
      state.preview = null;
      state.previewStatus = "Select a file";
      state.extractStatus = "Select files to extract.";
      document.querySelector("#entry-search").value = "";
      document.querySelector("#destination-uri").value = parentUri(objectUri);
      showWorkspace();
      render(state);
      await hydrateDestinationRoots().catch(() => {});
      if (sequence !== archiveSelectionSequence) return false;
      if (!await hydrateStatOnly() || sequence !== archiveSelectionSequence) return false;
      const loaded = await hydrateArchiveEntries();
      if (sequence !== archiveSelectionSequence) return false;
      if (loaded === false) throw new Error("Archive files could not be loaded.");
      announceHomeChrome();
    }

    async function hydrateStatOnly() {
      const token = homeToken;
      const sequence = archiveSelectionSequence;
      const uri = objectUri;
      if (!token || !objectUri) {
        return;
      }
      const url = new URL("/api/viewers/archive-manager/library-object", window.location.origin);
      url.searchParams.set("uri", objectUri);
      url.searchParams.set("stat_only", "true");
      const response = await fetch(url.pathname + url.search, {
        headers: { "x-elastos-home-token": token },
      });
      const envelope = await response.json().catch(() => ({}));
      if (sequence !== archiveSelectionSequence || uri !== objectUri) return false;
      if (!response.ok) {
        throw new Error(envelope.message || envelope.error || "viewer stat failed");
      }
      const object = envelope?.data?.object || {};
      if (object.uri !== uri) return false;
      Object.assign(state, {
        name: object.name || state.name,
        uri: object.uri || state.uri,
        mime: object.mime || state.mime,
        contentCid: object.content_cid || state.contentCid,
        policy: object.metadata?.archive_support || state.policy,
      });
      render(state);
      return true;
    }

    async function hydrateArchiveEntries() {
      const token = homeToken;
      const sequence = archiveSelectionSequence;
      const uri = objectUri;
      if (!token || !objectUri) {
        state.entryStatus = "Opened with launch metadata only.";
        renderEntries();
        return;
      }
      if (state.policy?.status && state.policy.status !== "extractable") {
        state.entryStatus = "This archive format needs review before extraction.";
        renderEntries();
        return;
      }
      const url = new URL("/api/viewers/archive-manager/library-object", window.location.origin);
      url.searchParams.set("uri", objectUri);
      url.searchParams.set("entries", "true");
      const response = await fetch(url.pathname + url.search, {
        headers: { "x-elastos-home-token": token },
      });
      const envelope = await response.json().catch(() => ({}));
      if (sequence !== archiveSelectionSequence || uri !== objectUri) return false;
      if (!response.ok) {
        state.entryStatus = envelope.message || envelope.error || "Could not read this archive.";
        state.entries = [];
        renderEntries();
        return false;
      }
      const data = envelope?.data || {};
      if (!Array.isArray(data.entries) || (data.object && data.object.uri !== uri)) return false;
      state.entries = Array.isArray(data.entries) ? data.entries : [];
      state.selectedEntries = new Set([...state.selectedEntries].filter((path) =>
        state.entries.some((entry) => entry.path === path && entry?.safety?.status !== "blocked")
      ));
      const returned = data.limits?.returned_entries ?? state.entries.length;
      const truncated = data.limits?.truncated ? " truncated" : "";
      state.entryStatus = `${returned} entries${truncated}`;
      if (data.object) {
        state.name = data.object.name || state.name;
        state.uri = data.object.uri || state.uri;
        state.mime = data.object.mime || state.mime;
        state.contentCid = data.object.content_cid || state.contentCid;
        state.policy = data.object.metadata?.archive_support || state.policy;
      }
      render(state);
      homeNavigation.setQuery({ objectUri: uri });
      return true;
    }

    async function hydrateDestinationRoots() {
      const token = homeToken;
      if (!token) return;
      const response = await fetch("/api/viewers/archive-manager/library-roots", {
        headers: { "x-elastos-home-token": token },
      });
      const envelope = await response.json().catch(() => ({}));
      if (!response.ok) {
        throw new Error(envelope.message || envelope.error || "viewer roots failed");
      }
      state.roots = Array.isArray(envelope?.data?.roots) ? envelope.data.roots : [];
      renderDestinationRoots();
    }

    async function extractSelectedEntries() {
      await extractEntries([...state.selectedEntries], "Extracting selected files through Runtime...");
    }

    async function extractAllEntries() {
      const entries = safeEntries().map((entry) => entry.path);
      state.selectedEntries = new Set(entries);
      renderEntries();
      await extractEntries(entries, "Extracting all safe files through Runtime...");
    }

    async function extractEntries(entries, progressText) {
      const token = homeToken;
      const destinationUri = document.querySelector("#destination-uri").value.trim();
      if (!token || !objectUri) throw new Error("launch token missing");
      if (!destinationUri) throw new Error("destination URI is required");
      if (!entries.length) throw new Error("select at least one safe file");
      state.extracting = true;
      state.extractStatus = progressText;
      renderExtractControls();
      const url = new URL("/api/viewers/archive-manager/library-object", window.location.origin);
      url.searchParams.set("uri", objectUri);
      try {
        const response = await fetch(url.pathname + url.search, {
          method: "POST",
          headers: {
            "content-type": "application/json",
            "x-elastos-home-token": token,
          },
          body: JSON.stringify({
            destination_uri: destinationUri,
            entries,
            conflict_policy: document.querySelector("#conflict-policy").value,
          }),
        });
        const envelope = await response.json().catch(() => ({}));
        if (!response.ok || envelope.status === "error") {
          throw new Error(envelope.message || envelope.error || "archive extract failed");
        }
        const receipt = envelope?.data?.receipt || {};
        const progress = receipt.progress || {};
        state.extractStatus = `Extract ${receipt.status || "completed"}: ${progress.written_entries || 0} written, ${progress.skipped_entries || 0} skipped, ${progress.blocked_entries || 0} blocked.`;
      } finally {
        state.extracting = false;
        renderExtractControls();
      }
    }

    async function selectPreviewEntry(path) {
      const entry = (state.entries || []).find((row) => row.path === path);
      if (!entry || entry?.safety?.status === "blocked" || entry.kind !== "file") {
        state.focusedEntry = path;
        state.preview = null;
        state.previewStatus = "Preview is available only for safe file entries.";
        renderEntries();
        renderEntryPreview();
        return;
      }
      state.focusedEntry = path;
      state.preview = null;
      state.previewStatus = "Loading bounded preview...";
      renderEntries();
      renderEntryPreview();
      const token = homeToken;
      if (!token || !objectUri) {
        state.previewStatus = "Launch token missing; preview unavailable.";
        renderEntryPreview();
        return;
      }
      const url = new URL("/api/viewers/archive-manager/library-object", window.location.origin);
      url.searchParams.set("uri", objectUri);
      url.searchParams.set("preview_entry", path);
      const response = await fetch(url.pathname + url.search, {
        headers: { "x-elastos-home-token": token },
      });
      const envelope = await response.json().catch(() => ({}));
      if (!response.ok || envelope.status === "error") {
        state.previewStatus = envelope.message || envelope.error || "Preview unavailable.";
        state.preview = null;
      } else {
        state.preview = envelope?.data || null;
        state.previewStatus = "Preview loaded through Runtime.";
      }
      renderEntryPreview();
    }

    function parseArchiveSupport(value) {
      if (!value) return null;
      if (typeof value === "object") return value;
      try {
        return JSON.parse(value);
      } catch {
        return null;
      }
    }

    function render(next) {
      document.querySelector("#title").textContent = next.name || "Archive";
      renderDestinationRoots();
      renderEntries();
      renderEntryPreview();
    }

    function renderEntries() {
      document.querySelector("#empty-status").textContent = !objectUri && state.entryStatus !== "Not loaded" ? state.entryStatus : "";
      const pill = document.querySelector("#entries-pill");
      const rows = filteredEntries();
      const hasBlocked = rows.some((entry) => entry?.safety?.status === "blocked");
      pill.textContent = selectionSummary(rows);
      pill.hidden = rows.length === 0;
      pill.classList.toggle("safe", rows.length > 0 && !hasBlocked);
      pill.classList.toggle("blocked", hasBlocked || /blocked|gated|unsupported|unavailable|review/i.test(state.entryStatus));
      const list = document.querySelector("#entry-list");
      if (!rows.length) {
        list.innerHTML = `<div class="entry-empty"><div class="archive-empty-list"><strong>No files shown</strong><span>${escapeHtml(emptyEntryMessage())}</span></div></div>`;
        renderExtractControls();
        return;
      }
      list.innerHTML = rows.map((entry) => {
        const blocked = entry?.safety?.status === "blocked";
        const selectable = !blocked && entry.kind !== "blocked";
        const checked = state.selectedEntries.has(entry.path || "");
        const selected = state.focusedEntry === (entry.path || "");
        const modified = entry.modified_at ? new Date(entry.modified_at * 1000).toLocaleString() : "-";
        return `<div class="entry-row${blocked ? " blocked" : ""}${selected ? " selected" : ""}" role="button" tabindex="${selectable ? "0" : "-1"}" data-path="${escapeHtml(entry.path || "")}" aria-selected="${selected ? "true" : "false"}">
          <div><input class="entry-check" type="checkbox" value="${escapeHtml(entry.path || "")}" ${checked ? "checked" : ""} ${selectable ? "" : "disabled"} aria-label="Select ${escapeHtml(entry.path || entry.name || "entry")}"></div>
          <div class="entry-name">
            <span class="entry-icon" aria-hidden="true">${escapeHtml(entryIcon(entry))}</span>
            <span class="entry-title" title="${escapeHtml(entry.path || entry.name || "entry")}">${escapeHtml(entry.path || entry.name || "entry")}</span>
          </div>
          <span class="entry-cell">${escapeHtml(formatBytes(entry.size))}</span>
          <span class="entry-cell">${escapeHtml(modified)}</span>
        </div>`;
      }).join("");
      renderExtractControls();
    }

    function renderDestinationRoots() {
      const container = document.querySelector("#destination-roots");
      const roots = (state.roots || []).filter(isWritableDestinationRoot);
      if (!roots.length) {
        container.innerHTML = "";
        return;
      }
      container.innerHTML = roots.map((root) =>
        `<button type="button" data-destination-uri="${escapeHtml(root.uri)}" title="${escapeHtml(root.uri)}">${escapeHtml(root.label || root.id || "Root")}</button>`
      ).join("");
    }

    function renderEntryPreview() {
      const preview = document.querySelector("#entry-preview");
      const data = state.preview || {};
      const entry = data.entry || null;
      const previewData = data.preview || null;
      if (!entry || !previewData) {
        preview.innerHTML = `<div class="preview-empty"><strong>${escapeHtml(state.previewStatus || "Select a file")}</strong><span>Preview appears here.</span></div>`;
        return;
      }
      const text = typeof previewData.text === "string" ? previewData.text : "";
      const body = text
        ? `<pre>${escapeHtml(text)}</pre>`
        : `<p>Binary preview is not shown. Extract the file to open it from Library.</p>`;
      preview.innerHTML = `
        <div><strong>${escapeHtml(entry.path || "entry")}</strong></div>
        <div>${escapeHtml(entry.mime || "application/octet-stream")} · ${formatBytes(entry.size)}${previewData.truncated ? " · truncated" : ""}</div>
        ${body}
      `;
    }

    function isWritableDestinationRoot(root) {
      if (!root?.uri) return false;
      if (root.id === "trash" || root.uri.includes("/.Trash")) return false;
      if (root.kind === "webspace-root") return false;
      if (root.metadata?.readonly === true) return false;
      return root.kind === "directory" || root.kind === "principal-root";
    }

    function filteredEntries() {
      const query = String(state.entryQuery || "").trim().toLowerCase();
      if (!query) return state.entries || [];
      return (state.entries || []).filter((entry) =>
        String(entry.path || entry.name || "").toLowerCase().includes(query)
      );
    }

    function safeEntries() {
      return (state.entries || []).filter((entry) =>
        entry.path && entry.kind !== "blocked" && entry?.safety?.status !== "blocked"
      );
    }

    function safeVisibleEntries() {
      return filteredEntries().filter((entry) =>
        entry.path && entry.kind !== "blocked" && entry?.safety?.status !== "blocked"
      );
    }

    function toggleEntrySelection(path) {
      const entry = (state.entries || []).find((row) => row.path === path);
      if (!entry || entry.kind === "blocked" || entry?.safety?.status === "blocked") return;
      if (state.selectedEntries.has(path)) {
        state.selectedEntries.delete(path);
      } else {
        state.selectedEntries.add(path);
      }
      renderEntries();
    }

    function handleEntryKeyboard(event) {
      const row = event.target.closest(".entry-row");
      if (!row) return;
      const rows = Array.from(document.querySelectorAll(".entry-row[tabindex='0']"));
      const index = rows.indexOf(row);
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        const delta = event.key === "ArrowDown" ? 1 : -1;
        const next = rows[index + delta] || rows[index];
        next?.focus();
        if (next?.dataset?.path) selectPreviewEntry(next.dataset.path);
      } else if (event.key === " " || event.key === "Enter") {
        event.preventDefault();
        const path = row.dataset.path || "";
        if (event.key === " ") {
          toggleEntrySelection(path);
        } else if (path) {
          selectPreviewEntry(path);
        }
      }
    }

    function emptyEntryMessage() {
      const status = String(state.entryStatus || "").trim();
      if (!objectUri) return "Open an archive or create a new ZIP.";
      if (!status || status === "Not loaded" || status === "Loading archive files...") return "Files will appear here.";
      if (/could not read|entry listing unavailable|archive_entries|viewer|unavailable/i.test(status)) {
        return "Could not read this archive yet. Try recreating it or opening another archive.";
      }
      return status;
    }

    function renderExtractControls() {
      const selectableCount = safeVisibleEntries().length;
      const allSafeCount = safeEntries().length;
      const policyBlocked = state.policy?.status === "policy_gated_unsupported_archive_family";
      const selectedEnabled = state.selectedEntries.size > 0 && !policyBlocked;
      document.querySelector("#select-all-safe").disabled = selectableCount === 0 || state.extracting;
      document.querySelector("#select-all-safe").hidden = selectableCount === 0;
      document.querySelector("#clear-selection").disabled = state.selectedEntries.size === 0 || state.extracting;
      document.querySelector("#clear-selection").hidden = selectableCount === 0 && state.selectedEntries.size === 0;
      document.querySelector("#extract-selected").disabled = !selectedEnabled || state.extracting;
      document.querySelector("#extract-all").disabled = allSafeCount === 0 || policyBlocked || state.extracting;
      document.querySelector("#extract-status").textContent = state.extractStatus || "Select files to extract.";
    }

    function selectionSummary(rows) {
      const count = rows.length;
      const selected = [...state.selectedEntries].filter((path) =>
        rows.some((entry) => entry.path === path)
      ).length;
      if (!count) return state.entryStatus || "Not loaded";
      return selected ? `${selected} selected of ${count}` : state.entryStatus || `${count} entries`;
    }

    function entryIcon(entry) {
      if (entry?.safety?.status === "blocked") return "!";
      if (entry?.kind === "directory") return "/";
      return "F";
    }

    function parentUri(uri) {
      const clean = String(uri || "").replace(/\/+$/, "");
      const index = clean.lastIndexOf("/");
      return index > "localhost://".length ? clean.slice(0, index) : clean;
    }

    function formatBytes(value) {
      if (value == null || Number.isNaN(Number(value))) return "size unknown";
      const bytes = Number(value);
      if (bytes < 1024) return `${bytes} B`;
      if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
      return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
    }

    function escapeHtml(value) {
      return String(value || "")
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;")
        .replace(/"/g, "&quot;");
    }
