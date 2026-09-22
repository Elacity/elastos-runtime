/* Shared model presentation. Runtime owns admission, execution and retention. */
(() => {
  const CID = /^bafybei[a-z2-7]{52}$/;
  const STATES = new Set(["capacity_pending", "reserved", "preparing", "verifying", "admission_pending", "admitted", "reclaimed", "failed", "uncertain", "cancelled", "expired"]);
  const active = p => p && ["capacity_pending", "reserved", "preparing", "verifying", "admission_pending", "uncertain"].includes(p.state);
  const FAILURE_TEXT = Object.freeze({
    authorization_unavailable: "Preparation stopped. Open Models again to check access.",
    policy_unavailable: "Preparation stopped. Model policy is unavailable.",
    capacity_unavailable: "Preparation stopped. Storage capacity is unavailable.",
    content_unavailable: "Preparation failed. Model content could not be read.",
    local_storage_unavailable: "Preparation failed. Local storage could not be written.",
    verification_failed: "Preparation failed. Model verification did not pass.",
    preparation_unavailable: "Preparation failed.",
  });
  const failureText = p => FAILURE_TEXT[p.failure_class] || "Preparation failed.";
  // One phase per Runtime projection. The full view (System) and the compact
  // Marketplace detail read their copy from the same phase.
  function phaseOf(r, p, message) {
    if (message) return "unavailable";
    if (r.dispatch_ready) return "ready";
    if (r.admitted) return "service_unavailable";
    if (active(p)) return p.cancel_requested ? "cancelling" : p.state === "capacity_pending" ? "capacity" : p.state === "uncertain" ? "settling" : p.state === "verifying" ? "verifying" : "preparing";
    if (!p) return "absent";
    return p.state;
  }
  const PHASE_TEXT = Object.freeze({
    unavailable: "Current status unavailable", ready: "Available on this device",
    service_unavailable: "Available on this device. Model service unavailable.",
    cancelling: "Cancelling preparation…", capacity: "Waiting for local capacity…", settling: "Waiting for preparation to settle…",
    verifying: "Checking model files…", preparing: "Preparing local files…", absent: "Ready to prepare",
    reclaimed: "Model removed from local cache.", cancelled: "Preparation cancelled.", expired: "Preparation expired.",
  });
  // The compact detail sits under the model's own title, so it speaks about
  // this device and the Get action instead of the preparation mechanism.
  const COMPACT_PHASE_TEXT = Object.freeze({
    ...PHASE_TEXT, absent: "Not on this device yet", preparing: "Getting the model…",
    reclaimed: "Removed from this device", cancelled: "Stopped before finishing", expired: "Not finished in time",
  });
  const RECLAIM_CONFIRM_TEXT = Object.freeze({
    compact: "This removes prepared model files from this device. Conversations stay.",
    full: "This removes prepared model files from this device. Conversations stay. Other copies, if any, keep their files.",
  });
  const RECLAIM_BUSY_TEXT = "Stop the current reply in Assistant, then try Remove again.";
  const text = (v, max = 256) => typeof v === "string" && v.length > 0 && v.length <= max && !/[\u0000-\u001f]/.test(v);
  function check(ok) { if (!ok) throw new Error("Invalid model response"); }
  function parseRuntime(r, cid) {
    check(r && [r.admitted, r.kept, r.dispatch_ready].every(v => typeof v === "boolean"));
    check(r.dispatch_ready ? r.admitted && /^model:[0-9a-f]{64}$/.test(r.offer_id) : r.offer_id === null);
    const p = r.preparation;
    if (p !== null) {
      check(p && p.cid === cid && text(p.operation_id, 160) && STATES.has(p.state));
      check([p.total_bytes, p.completed_bytes].every(n => Number.isSafeInteger(n) && n >= 0) && p.completed_bytes <= p.total_bytes);
      check(typeof p.cancel_requested === "boolean" && typeof p.activation_pending === "boolean" && typeof p.admitted === "boolean");
      check(p.admitted === (p.state === "admitted") && r.admitted === p.admitted);
      check(p.failure_class == null || (typeof p.failure_class === "string" && Object.hasOwn(FAILURE_TEXT, p.failure_class)));
    } else check(!r.admitted && !r.kept && !r.dispatch_ready);
    return r;
  }
  function catalogEntries(catalog) {
    check(catalog.schema === "elastos.capsules.catalog/v1" && Array.isArray(catalog.capsules) && catalog.capsules.length <= 1024);
    check(["verified", "unconfigured", "unavailable"].includes(catalog.model_catalog_state));
    if (catalog.model_catalog_state === "unavailable") throw new Error("Model trust unavailable");
    const entries = catalog.capsules.filter(e => e.source === "signed-model-catalog");
    check(entries.length <= 8);
    if (!entries.length) return [];
    check(catalog.model_catalog_state === "verified");
    const seen = new Set();
    for (const item of entries) {
      check(typeof item.cid === "string" && !seen.has(item.cid));
      seen.add(item.cid);
      check(item.role === "content" && item.installed === false && item.launchable === false
        && CID.test(item.cid) && text(item.title) && text(item.publisher_did, 256)
        && item.publisher_did.startsWith("did:") && item.signature_state === "catalog-signature-verified"
        && Number.isSafeInteger(item.content_size_bytes) && item.content_size_bytes > 0);
      parseRuntime(item.model_runtime, item.cid);
    }
    return entries;
  }
  function selectCatalogEntry(entries, selectedCid) {
    if (selectedCid) {
      const matches = entries.filter(item => item.cid === selectedCid);
      return matches.length === 1 ? matches[0] : null;
    }
    return entries.length === 1 ? entries[0] : null;
  }
  function formatBytes(bytes) {
    const units = ["B", "KB", "MB", "GB", "TB"];
    const rank = bytes ? Math.min(4, Math.floor(Math.log(bytes) / Math.log(1000))) : 0;
    return `${(bytes / 1000 ** rank).toLocaleString(undefined, { maximumFractionDigits: rank ? 2 : 0 })} ${units[rank]}`;
  }
  function element(tag, value, className) {
    const node = document.createElement(tag);
    if (value) node.textContent = value;
    if (className) node.className = className;
    return node;
  }
  // Typed operation outputs are flat. Only catalog rows nest model_runtime.
  function operationRuntime(output, cid, retainedPreparation) {
    const preparation = retainedPreparation || Object.fromEntries([
      "operation_id", "cid", "state", "total_bytes", "completed_bytes", "cancel_requested", "admitted", "activation_pending", "failure_class",
    ].map(key => [key, output[key]]));
    return parseRuntime({ admitted: output.admitted, kept: output.kept,
      dispatch_ready: output.dispatch_ready, offer_id: output.offer_id, preparation }, cid);
  }
  window.ElastosModelManagement = {
    create({ root, capsule, token, buttonClass = "pc2-btn pc2-btn-secondary", cid: requiredCid = null, compact = false, onReadyOpen = null }) {
      let visible = false, closed = false, generation = 0, busy = false, timer, reader;
      let selectedCid = requiredCid, choices = [], model = null, methods = new Map(), message = "", loading = false;
      let unresolvedUse = null, reconcileRequired = false;
      let pendingFocus = null;
      let reclaimStep = "idle";
      root.classList.add("model-management");
      root.tabIndex = -1;
      const show = () => visible && !closed && !document.hidden;
      function stopRead() { clearTimeout(timer); reader?.abort(); reader = null; }
      async function request(path, body, signal) {
        const response = await fetch(path, { method: body ? "POST" : "GET", headers: {
          "x-elastos-home-token": token, ...(body ? { "content-type": "application/json" } : {}),
        }, ...(body ? { body: JSON.stringify(body) } : {}), signal });
        if (!response.ok) throw new Error("Model request failed");
        const raw = await response.text();
        check(raw.length <= 2 * 1024 * 1024);
        return JSON.parse(raw);
      }
      async function invoke(operation, input, signal, requestId) {
        const method = methods.get(operation);
        check(method);
        const body = { capsule, interface: method.interface, method: method.id,
          request_id: requestId || `model-${crypto.randomUUID()}`, input };
        const result = await request("/api/capsules/interfaces/invoke", body, signal);
        check(result.schema === "elastos.capsules.invoke-result/v1" && result.status === "ok"
          && ["capsule", "interface", "method", "request_id"].every(key => result[key] === body[key]));
        return result.output;
      }
      function controller() {
        const value = new AbortController();
        const timeout = setTimeout(() => value.abort(), 35000);
        return { signal: value.signal, abort: () => value.abort(), done: () => clearTimeout(timeout) };
      }
      function parseCatalog(catalog) {
        const entries = catalogEntries(catalog);
        choices = entries;
        if (!entries.length) return null;
        return selectCatalogEntry(entries, requiredCid || selectedCid);
      }
      function bindMethods(registry) {
        check(Array.isArray(registry.interfaces) && registry.interfaces.length <= 1024);
        const found = new Map();
        for (const entry of registry.interfaces.filter(e => e.capsule === capsule)) {
          check(text(entry.interface?.id, 160) && /^[A-Za-z0-9_.:-]+$/.test(entry.interface.id));
          for (const method of entry.interface?.methods || []) {
            if (method.resource !== "elastos://capsules/*" || !["use", "status", "cancel", "retention", "reclaim"].includes(method.operation)) continue;
            check(!found.has(method.operation));
            const bindings = entry.bindings?.filter(b => b.method === method.id);
            check(method.id === `content.${method.operation}` && bindings?.length === 1
              && bindings[0].executable === true && method.approval === "runtime_policy"
              && method.risk === (method.operation === "status" ? "read" : "write"));
            found.set(method.operation, { id: method.id, interface: entry.interface.id });
          }
        }
        check(found.size === 5);
        methods = found;
      }
      async function load() {
        if (!show() || busy) return;
        stopRead(); const epoch = ++generation; const io = controller(); reader = io;
        loading = true; message = ""; render();
        try {
          const [catalog, interfaces] = await Promise.all([
            request("/api/capsules/catalog", null, io.signal), request("/api/capsules/interfaces", null, io.signal),
          ]);
          if (epoch !== generation || !show()) return;
          bindMethods(interfaces); model = parseCatalog(catalog);
          reconcileRequired = false;
          reclaimStep = "idle";
          if (model?.cid !== unresolvedUse?.cid || model?.model_runtime.preparation) unresolvedUse = null;
        } catch {
          if (epoch !== generation || !show()) return;
          model = null; choices = []; message = "Models are unavailable. Check your connection and trusted catalog, then retry.";
        } finally {
          io.done();
          if (epoch === generation) { loading = false; render(); schedule(); }
        }
      }
      function schedule() {
        clearTimeout(timer);
        const runtime = model?.model_runtime, preparation = runtime?.preparation;
        const pending = active(preparation) || (preparation && runtime.admitted && !runtime.dispatch_ready);
        if (!show() || busy || message || !pending) return;
        timer = setTimeout(poll, 1500);
      }
      async function poll() {
        if (!show() || busy || !model) return;
        const epoch = generation, cid = model.cid, id = model.model_runtime.preparation.operation_id;
        const io = controller(); reader = io;
        try {
          const result = await invoke("status", { operation_id: id }, io.signal);
          if (epoch !== generation || !show() || model?.cid !== cid) return;
          check(result.cid === cid && result.operation_id === id);
          const r = operationRuntime(result, cid);
          check(r.preparation?.operation_id === id);
          model.model_runtime = r;
        } catch {
          if (epoch !== generation || !show()) return;
          message = "Status is unavailable. Refresh to check before trying again.";
        } finally { io.done(); if (epoch === generation) { render(); schedule(); } }
      }
      async function act(operation, keep) {
        if (busy || loading || !show() || !model || reconcileRequired) return;
        stopRead(); const epoch = ++generation, cid = model.cid;
        const id = model.model_runtime.preparation?.operation_id;
        busy = true; message = ""; render();
        const io = controller();
        try {
          const input = operation === "retention" ? { cid, keep } : operation === "cancel" ? { operation_id: id } : { cid };
          if (operation === "use" && !unresolvedUse) unresolvedUse = { cid, id: `model-${crypto.randomUUID()}` };
          const result = await invoke(operation, input, io.signal, operation === "use" ? unresolvedUse.id : undefined);
          if (epoch !== generation || !show() || model?.cid !== cid) return;
          check(result.cid === cid);
          if (operation === "retention") {
            check(result.kept === keep && typeof result.admitted === "boolean");
            const catalog = await request("/api/capsules/catalog", null, io.signal);
            if (epoch !== generation || !show() || model?.cid !== cid) return;
            const candidate = parseCatalog(catalog);
            check(candidate?.cid === cid);
            model = candidate;
          } else if (operation === "reclaim") {
            check(typeof result.admitted === "boolean");
            const catalog = await request("/api/capsules/catalog", null, io.signal);
            if (epoch !== generation || !show() || model?.cid !== cid) return;
            const candidate = parseCatalog(catalog);
            check(candidate?.cid === cid);
            model = candidate;
          } else {
            const r = operationRuntime(result, cid);
            check(r.preparation?.operation_id === result.operation_id && (operation !== "cancel" || result.operation_id === id));
            model.model_runtime = r;
            if (operation === "use") unresolvedUse = null;
          }
        } catch {
          if (epoch === generation && show()) {
            if (operation === "reclaim") {
              try {
                const catalog = await request("/api/capsules/catalog", null, io.signal);
                if (epoch === generation && show() && model?.cid === cid) {
                  const candidate = parseCatalog(catalog);
                  if (candidate?.cid === cid && candidate.model_runtime.admitted) {
                    model = candidate;
                    reclaimStep = "busy";
                    return;
                  }
                }
              } catch {}
            }
            reconcileRequired = true;
            message = "Model action could not be confirmed. Refresh to check its status.";
          }
        } finally {
          io.done(); busy = false;
          if (epoch === generation) { render(); schedule(); }
          else if (show()) void load();
        }
      }
      function button(label, run, disabled = false) {
        const node = element("button", label, buttonClass);
        node.type = "button"; node.disabled = disabled || busy || loading; node.addEventListener("click", run); return node;
      }
      function render() {
        const focused = root.contains(document.activeElement) ? document.activeElement?.dataset.modelControl || pendingFocus : null;
        pendingFocus = focused;
        const restoreFocus = () => {
          if (!focused) return;
          const target = root.querySelector(`[data-model-control="${focused}"]`);
          if (target && !target.disabled) { target.focus(); pendingFocus = null; }
          else root.focus();
        };
        root.replaceChildren();
        const refresh = button("Refresh models", () => void load(), loading);
        refresh.dataset.modelControl = "refresh";
        if (!compact) root.append(refresh);
        const status = element("p", loading ? "Loading models…" : message);
        status.setAttribute("role", "status"); status.hidden = compact && !status.textContent; root.append(status);
        if (!model) {
          if (!loading && !message && !requiredCid && choices.length > 1) {
            root.append(element("p", "Select a verified model. Keep and removal apply to the model you select."));
            const list = element("ul", "", "model-list");
            for (const item of choices) {
              const row = element("li");
              const pick = button(item.title, () => { selectedCid = item.cid; void load(); });
              pick.dataset.modelControl = `select-${item.cid}`;
              row.append(pick, element("span", formatBytes(item.content_size_bytes), "model-size"));
              list.append(row);
            }
            root.append(list);
            if (compact) root.append(refresh);
            restoreFocus();
            return;
          }
          if (!loading && !message) root.append(element("p", "No verified model is available. Ask your administrator to configure a trusted catalog."));
          if (compact) root.append(refresh);
          restoreFocus();
          return;
        }
        const r = model.model_runtime, p = r.preparation;
        const row = element("article", "", "model-row");
        if (!compact) {
          row.append(element("h2", model.title), element("p", formatBytes(model.content_size_bytes), "model-size"));
          const identity = element("details", "", "model-details");
          identity.append(element("summary", "Content details"), element("p", `Verified publisher: ${model.publisher_did}`),
            element("p", `Content ID: ${model.cid}`, "model-identity"));
          row.append(identity);
        }
        const phase = phaseOf(r, p, message);
        const label = phase === "failed" ? failureText(p) : (compact ? COMPACT_PHASE_TEXT : PHASE_TEXT)[phase];
        row.append(element("p", label));
        if (active(p)) {
          const progress = element("progress"); progress.max = p.total_bytes || 1; progress.value = p.completed_bytes;
          progress.setAttribute("aria-label", "Preparation progress"); row.append(progress, element("p", `${formatBytes(p.completed_bytes)} of ${formatBytes(p.total_bytes)} prepared`));
        }
        const controls = element("div", "", "model-controls");
        const use = button((p && p.state !== "reclaimed") || unresolvedUse ? "Retry" : compact ? "Get" : "Use", () => void act("use"), active(p) || reconcileRequired);
        use.dataset.modelControl = "use";
        if (!active(p) && !r.dispatch_ready) controls.append(use);
        if (r.dispatch_ready && typeof onReadyOpen === "function") {
          const open = button("Open in Assistant", () => onReadyOpen({ cid: model.cid, offer_id: r.offer_id }));
          open.dataset.modelControl = "open-assistant";
          controls.append(open);
        }
        if (r.admitted && !active(p) && reclaimStep === "busy") {
          row.append(element("p", RECLAIM_BUSY_TEXT, "model-reclaim-busy"));
        }
        if (r.admitted && !active(p) && reclaimStep === "confirm") {
          row.append(element("p", compact ? RECLAIM_CONFIRM_TEXT.compact : RECLAIM_CONFIRM_TEXT.full, "model-reclaim-confirm"));
          const decline = button("Keep this model", () => { reclaimStep = "idle"; render(); }, reconcileRequired);
          decline.dataset.modelControl = "reclaim-decline";
          const confirm = button("Remove now", () => { reclaimStep = "idle"; void act("reclaim"); }, reconcileRequired);
          confirm.dataset.modelControl = "reclaim-confirm";
          controls.append(decline, confirm);
        } else if (r.admitted && !active(p)) {
          const reclaim = button("Remove from this device", () => { reclaimStep = "confirm"; render(); }, reconcileRequired);
          reclaim.dataset.modelControl = "reclaim";
          controls.append(reclaim);
        }
        if (active(p)) { const cancel = button("Cancel preparation", () => void act("cancel"), p.cancel_requested || reconcileRequired); cancel.dataset.modelControl = "cancel"; controls.append(cancel); }
        if (reclaimStep !== "confirm") {
          const labelNode = element("label", "", "model-keep"), toggle = element("input");
          toggle.type = "checkbox"; toggle.checked = r.kept; toggle.disabled = !(r.admitted || active(p)) || p?.cancel_requested || busy || loading || reconcileRequired; toggle.dataset.modelControl = "keep";
          toggle.addEventListener("change", () => void act("retention", toggle.checked));
          labelNode.append(toggle, document.createTextNode("Keep on this device")); controls.append(labelNode);
        }
        row.append(controls);
        if (!compact) {
          row.append(element("p", r.kept
            ? (r.admitted ? "Kept on this device. Release this choice to allow cache cleanup." : "Keep choice saved for this preparation.")
            : "Prepared files can be removed during cache cleanup.", "model-hint"));
          row.append(element("p", "The model loads into memory when you use it in Assistant.", "model-hint"));
        }
        root.append(row);
        if (compact) root.append(refresh);
        if (!requiredCid && choices.length > 1) {
          const back = button("Choose another model", () => { selectedCid = null; model = null; void load(); });
          back.dataset.modelControl = "choose-another";
          root.append(back);
        }
        restoreFocus();
      }
      function setVisible(value) {
        if (visible === value) return;
        visible = value; root.hidden = !value; ++generation; stopRead();
        if (show()) void load();
      }
      const onVisibility = () => { ++generation; stopRead(); if (show()) void load(); };
      function destroy() {
        closed = true; ++generation; stopRead();
        document.removeEventListener("visibilitychange", onVisibility);
        window.removeEventListener("pagehide", destroy);
      }
      document.addEventListener("visibilitychange", onVisibility);
      window.addEventListener("pagehide", destroy);
      return { setVisible, refresh: load, destroy };
    },
  };
})();
