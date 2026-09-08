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
    create({ root, capsule, token, buttonClass = "pc2-btn pc2-btn-secondary" }) {
      let visible = false, closed = false, generation = 0, busy = false, timer, reader;
      let model = null, methods = new Map(), message = "", loading = false, polls = 0;
      let unresolvedUse = null, reconcileRequired = false;
      let pendingFocus = null;
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
        check(catalog.schema === "elastos.capsules.catalog/v1" && Array.isArray(catalog.capsules) && catalog.capsules.length <= 1024);
        check(["verified", "unconfigured", "unavailable"].includes(catalog.model_catalog_state));
        if (catalog.model_catalog_state === "unavailable") throw new Error("Model trust unavailable");
        const entries = catalog.capsules.filter(e => e.source === "signed-model-catalog");
        check(entries.length <= 1 && (!entries.length || catalog.model_catalog_state === "verified"));
        if (!entries.length) return null;
        const entry = entries[0];
        check(entry.role === "content" && entry.installed === false && entry.launchable === false
          && CID.test(entry.cid) && text(entry.title) && text(entry.publisher_did, 256)
          && entry.publisher_did.startsWith("did:") && entry.signature_state === "catalog-signature-verified"
          && Number.isSafeInteger(entry.content_size_bytes) && entry.content_size_bytes > 0);
        parseRuntime(entry.model_runtime, entry.cid);
        return entry;
      }
      function bindMethods(registry) {
        check(Array.isArray(registry.interfaces) && registry.interfaces.length <= 1024);
        const found = new Map();
        for (const entry of registry.interfaces.filter(e => e.capsule === capsule)) {
          check(text(entry.interface?.id, 160) && /^[A-Za-z0-9_.:-]+$/.test(entry.interface.id));
          for (const method of entry.interface?.methods || []) {
            if (method.resource !== "elastos://capsules/*" || !["use", "status", "cancel", "retention"].includes(method.operation)) continue;
            check(!found.has(method.operation));
            const bindings = entry.bindings?.filter(b => b.method === method.id);
            check(method.id === `content.${method.operation}` && bindings?.length === 1
              && bindings[0].executable === true && method.approval === "runtime_policy"
              && method.risk === (method.operation === "status" ? "read" : "write"));
            found.set(method.operation, { id: method.id, interface: entry.interface.id });
          }
        }
        check(found.size === 4);
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
          bindMethods(interfaces); model = parseCatalog(catalog); polls = 0;
          reconcileRequired = false;
          if (model?.cid !== unresolvedUse?.cid || model?.model_runtime.preparation) unresolvedUse = null;
        } catch {
          if (epoch !== generation || !show()) return;
          model = null; message = "Models are unavailable. Check your connection and trusted catalog, then retry.";
        } finally {
          io.done();
          if (epoch === generation) { loading = false; render(); schedule(); }
        }
      }
      function schedule() {
        clearTimeout(timer);
        if (!show() || busy || message || !active(model?.model_runtime.preparation)) return;
        if (polls >= 120) { message = "Preparation is still pending. Refresh to check its status."; render(); return; }
        timer = setTimeout(poll, 1500);
      }
      async function poll() {
        if (!show() || busy || !model) return;
        const epoch = generation, cid = model.cid, id = model.model_runtime.preparation.operation_id;
        const io = controller(); reader = io; polls++;
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
            check(result.kept === keep && result.admitted === true);
            // Keep is confirmed only by the current caller's Runtime response.
            const candidate = operationRuntime(result, cid, model.model_runtime.preparation);
            check(candidate.kept === keep);
            model.model_runtime = candidate;
          } else {
            const r = operationRuntime(result, cid);
            check(r.preparation?.operation_id === result.operation_id && (operation !== "cancel" || result.operation_id === id));
            model.model_runtime = r; polls = 0;
            if (operation === "use") unresolvedUse = null;
          }
        } catch {
          if (epoch === generation && show()) {
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
        refresh.dataset.modelControl = "refresh"; root.append(refresh);
        const status = element("p", loading ? "Loading models…" : message);
        status.setAttribute("role", "status"); root.append(status);
        if (!model) {
          if (!loading && !message) root.append(element("p", "No verified model is available. Ask your administrator to configure a trusted catalog."));
          restoreFocus();
          return;
        }
        const r = model.model_runtime, p = r.preparation;
        const row = element("article", "", "model-row");
        row.append(element("h2", model.title), element("p", `Verified publisher: ${model.publisher_did}`),
          element("p", `Content ID: ${model.cid}`, "model-identity"), element("p", `${model.content_size_bytes.toLocaleString()} bytes`));
        const label = message ? "Current status unavailable" : r.dispatch_ready ? "Ready to use" : r.admitted ? "Prepared. The model offer is unavailable."
          : active(p) ? (p.cancel_requested ? "Cancelling preparation…" : p.state === "capacity_pending" ? "Waiting for local capacity…" : p.state === "uncertain" ? "Waiting for preparation to settle…" : "Preparing…")
            : p ? ({ reclaimed: "Model removed from local cache.", cancelled: "Preparation cancelled.", failed: failureText(p), expired: "Preparation expired." }[p.state] || "Waiting to prepare…") : "Not prepared";
        row.append(element("p", label));
        if (active(p)) {
          const progress = element("progress"); progress.max = p.total_bytes || 1; progress.value = p.completed_bytes;
          progress.setAttribute("aria-label", "Preparation progress"); row.append(progress, element("p", `${p.completed_bytes.toLocaleString()} of ${p.total_bytes.toLocaleString()} bytes`));
        }
        const controls = element("div", "", "model-controls");
        const use = button((p && p.state !== "reclaimed") || unresolvedUse ? "Retry" : "Use", () => void act("use"), active(p) || reconcileRequired);
        use.dataset.modelControl = "use";
        if (!active(p) && !r.dispatch_ready) controls.append(use);
        if (active(p)) { const cancel = button("Cancel preparation", () => void act("cancel"), p.cancel_requested || reconcileRequired); cancel.dataset.modelControl = "cancel"; controls.append(cancel); }
        const labelNode = element("label", "", "model-keep"), toggle = element("input");
        toggle.type = "checkbox"; toggle.checked = r.kept; toggle.disabled = !r.admitted || busy || loading || reconcileRequired; toggle.dataset.modelControl = "keep";
        toggle.addEventListener("change", () => void act("retention", toggle.checked));
        labelNode.append(toggle, document.createTextNode("Keep on this device")); controls.append(labelNode);
        row.append(controls); root.append(row);
        restoreFocus();
      }
      function setVisible(value) {
        if (visible === value) return;
        visible = value; root.hidden = !value; ++generation; stopRead();
        if (show()) void load();
      }
      document.addEventListener("visibilitychange", () => { ++generation; stopRead(); if (show()) void load(); });
      window.addEventListener("pagehide", () => { closed = true; ++generation; stopRead(); });
      return { setVisible, refresh: load };
    },
  };
})();
