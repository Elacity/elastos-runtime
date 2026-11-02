/* Shared model presentation. Runtime owns admission, execution and retention. */
(() => {
  const CID = /^bafybei[a-z2-7]{52}$/;
  const STATES = new Set(["capacity_pending", "reserved", "preparing", "verifying", "admission_pending", "admitted", "reclaimed", "failed", "uncertain", "cancelled", "expired"]);
  const active = p => p && ["capacity_pending", "reserved", "preparing", "verifying", "admission_pending", "uncertain"].includes(p.state);
  const MEMORY_FIT_TEXT = Object.freeze({
    fits: "This device has enough memory for this model. Replies also need some of it free when the model runs.",
    insufficient: "This device has less memory than this model requires. Choose a smaller model.",
    unavailable: "This Home could not check the device memory. Ask the Home owner to check the memory limits, then try again.",
  });
  // The Marketplace detail shows one short memory line under the model's status.
  const COMPACT_MEMORY_FIT_TEXT = Object.freeze({
    fits: "Fits this device's memory. It needs some free memory to reply.",
    insufficient: "Needs more memory than this device has. Choose a smaller model.",
    unavailable: "Couldn't check this device's memory. Ask the Home owner to check the memory limits.",
  });
  const FAILURE_TEXT = Object.freeze({
    authorization_unavailable: "Preparation stopped. Open Models again to check access.",
    policy_unavailable: "Preparation stopped. Model policy is unavailable.",
    capacity_unavailable: "Not enough free disk space for this model. Free some space, then Retry.",
    download_interrupted: "The download stopped before it finished. Retry.",
    storage_service_unavailable: "The storage service is unavailable. Retry when it is running.",
    cache_budget_unavailable: "This model does not fit the local cache budget. Remove an unneeded model, choose a smaller model, or increase the budget, then Retry.",
    memory_budget_unavailable: "This model exceeds the local model memory budget. Choose a smaller model or increase the budget, then Retry.",
    model_memory_insufficient: MEMORY_FIT_TEXT.insufficient,
    model_memory_observation_unavailable: MEMORY_FIT_TEXT.unavailable,
    request_expired: "This request is no longer valid. Check your device's clock, then try again.",
    content_unavailable: "Preparation failed. Model content could not be read.",
    local_storage_unavailable: "Preparation failed. Local storage could not be written.",
    verification_failed: "Preparation failed. Model verification did not pass.",
    preparation_unavailable: "Preparation failed.",
    activation_unavailable: "Local AI could not start. The model stays on this device. Retry.",
  });
  const failureText = p => FAILURE_TEXT[p.failure_class] || "Preparation failed.";
  const DISPATCH_UNAVAILABLE_TEXT = Object.freeze({
    unsupported_host: "Local AI is not supported on this device.",
    source_engine_required: "This source Home needs its local model engine. Install the engine through source setup, then Retry.",
    catalog_updated: "Model list updated. Press Use to keep using this model.",
    cache_budget_unavailable: FAILURE_TEXT.cache_budget_unavailable,
    memory_budget_unavailable: FAILURE_TEXT.memory_budget_unavailable,
  });
  // One phase per Runtime projection. The full view (System) and the compact
  // Marketplace detail read their copy from the same phase.
  function phaseOf(r, p, message, finishingExpired = false) {
    if (message) return "unavailable";
    if (r.dispatch_ready) return "ready";
    // Admitted content whose engine is still being activated is finishing,
    // not broken: Runtime clears activation_pending once dispatch can start.
    // An unresolved finish has a bounded wait before offering Retry.
    if (r.admitted) return activating(r, p) && !finishingExpired ? "finishing" : "service_unavailable";
    if (active(p)) return p.cancel_requested ? "cancelling" : p.state === "capacity_pending" ? "capacity" : p.state === "uncertain" ? "settling" : p.state === "verifying" ? "verifying" : "preparing";
    if (!p) return "absent";
    return p.state;
  }
  const FINISHING_DEADLINE_MS = 120000;
  const activating = (r, p) => Boolean(r.admitted && p?.activation_pending && !p.failure_class && !r.dispatch_unavailable_reason);
  const PHASE_TEXT = Object.freeze({
    unavailable: "Current status unavailable", ready: "Available on this device",
    service_unavailable: "Available on this device. Model service unavailable.",
    finishing: "Finishing setup…",
    cancelling: "Cancelling preparation…", settling: "Waiting for preparation to settle…",
    verifying: "Checking model files…", preparing: "Preparing local files…", absent: "Ready to prepare",
    reclaimed: "Model removed from local cache.", cancelled: "Preparation cancelled.", expired: "Preparation expired.",
  });
  // The compact detail sits under the model's own title, so it speaks about
  // this device and the Get action instead of the preparation mechanism.
  const COMPACT_PHASE_TEXT = Object.freeze({
    ...PHASE_TEXT, absent: "Not on this device yet", preparing: "Getting the model…",
    reclaimed: "Removed from this device", cancelled: "Stopped before finishing", expired: "Not finished in time",
  });
  const RECLAIM_CONFIRM_TEXT = "This removes prepared model files from this device. Conversations stay. Other copies, if any, keep their files.";
  const RECLAIM_BUSY_TEXT = "Stop the current reply in Assistant, then try Remove again.";
  const text = (v, max = 256) => typeof v === "string" && v.length > 0 && v.length <= max && !/[\u0000-\u001f]/.test(v);
  function check(ok) { if (!ok) throw new Error("Invalid model response"); }
  function parseRuntime(r, cid) {
    check(r && [r.admitted, r.kept, r.dispatch_ready].every(v => typeof v === "boolean"));
    check(r.dispatch_ready ? r.admitted && /^model:[0-9a-f]{64}$/.test(r.offer_id) : r.offer_id === null);
    check(r.dispatch_unavailable_reason == null || (r.admitted && !r.dispatch_ready &&
      typeof r.dispatch_unavailable_reason === "string" && Object.hasOwn(DISPATCH_UNAVAILABLE_TEXT, r.dispatch_unavailable_reason)));
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
        && Number.isSafeInteger(item.content_size_bytes) && item.content_size_bytes > 0
        && typeof item.model_memory_fit === "string" && Object.hasOwn(MEMORY_FIT_TEXT, item.model_memory_fit));
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
  // Waiting for space names how much this model still needs, so the user knows what to free.
  const capacityText = p => p.failure_class === "cache_budget_unavailable"
    ? "Waiting for room in the local cache budget. Remove an unneeded model or increase the budget and it continues."
    : `Waiting for free space. This model needs ${formatBytes(p.total_bytes - p.completed_bytes)} on this Home, plus 2 GB kept free. Free some space and it continues.`;
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
      dispatch_ready: output.dispatch_ready, dispatch_unavailable_reason: output.dispatch_unavailable_reason,
      offer_id: output.offer_id, preparation }, cid);
  }
  window.ElastosModelManagement = {
    // Compact mode is the Marketplace detail: root is its Status list, so each
    // fact is a list row led by a copy of itemMarker; the actions go to
    // actionsRoot (the detail footer) and Refresh appears only when a read failed.
    create({ root, capsule, token, buttonClass = "pc2-btn pc2-btn-secondary", primaryButtonClass = buttonClass,
      cid: requiredCid = null, compact = false, onReadyOpen = null, actionsRoot = null, itemMarker = null }) {
      let visible = false, closed = false, generation = 0, busy = false, timer, reader;
      let selectedCid = requiredCid, choices = [], model = null, methods = new Map(), message = "", loading = false;
      let unresolvedUse = null, reconcileRequired = false, refusal = "";
      let pendingFocus = null;
      let reclaimStep = "idle";
      // Consecutive failed status reads while a preparation is pending. A short
      // run keeps polling with backoff; only a long run gives up to Refresh.
      let statusFailures = 0, statusFailingSince = 0;
      // When this view first saw the current operation finishing setup.
      let finishingOperation = "", finishingSince = 0;
      // The full view brings its own styles; compact rows use the host's list styles.
      if (!compact) root.classList.add("model-management");
      root.tabIndex = -1;
      const show = () => visible && !closed && !document.hidden;
      function stopRead() { clearTimeout(timer); reader?.abort(); reader = null; }
      async function request(path, body, signal) {
        const response = await fetch(path, { method: body ? "POST" : "GET", headers: {
          "x-elastos-home-token": token, ...(body ? { "content-type": "application/json" } : {}),
        }, ...(body ? { body: JSON.stringify(body) } : {}), signal });
        const raw = await response.text();
        check(raw.length <= 2 * 1024 * 1024);
        const result = JSON.parse(raw);
        if (!response.ok) {
          // Only the exact invoked operation can return a final typed denial.
          const bound = body && result?.schema === "elastos.capsules.invoke-result/v1" && result.status === "error"
            && ["capsule", "interface", "method", "request_id"].every(key => result[key] === body[key]);
          let denial = bound && ["cache_budget_unavailable", "memory_budget_unavailable", "model_memory_insufficient", "model_memory_observation_unavailable", "request_expired"].includes(result.code)
            ? FAILURE_TEXT[result.code] : "";
          if (bound && result.code === "not_enough_free_space" && text(result.message, 300)
            && /^Not enough free disk space on this Home: this needs \d+(?:\.\d)? GB plus 2 GB kept free\. Free some space, then select Get again\.$/.test(result.message)) denial = result.message;
          if (denial) throw Object.assign(new Error(denial), { refusal: denial, code: result.code });
          throw new Error("Model request failed");
        }
        return result;
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
        loading = true; message = ""; refusal = ""; statusFailures = 0; render();
        try {
          const [catalog, interfaces] = await Promise.all([
            request("/api/capsules/catalog", null, io.signal), request("/api/capsules/interfaces", null, io.signal),
          ]);
          if (epoch !== generation || !show()) return;
          bindMethods(interfaces); model = parseCatalog(catalog);
          reconcileRequired = false;
          reclaimStep = "idle";
          if (model?.cid !== unresolvedUse?.cid || model?.model_runtime.preparation || unresolvedUse?.expiredConfirmed) unresolvedUse = null;
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
        timer = setTimeout(poll, statusFailures ? Math.min(1500 * 2 ** statusFailures, 10000) : 1500);
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
          statusFailures = 0;
        } catch {
          if (epoch !== generation || !show()) return;
          if (!statusFailures++) statusFailingSince = Date.now();
          if (statusFailures >= 5 && Date.now() - statusFailingSince >= 60000) {
            message = "Status is unavailable. Refresh to check before trying again.";
          }
        } finally { io.done(); if (epoch === generation) { render(); schedule(); } }
      }
      async function act(operation, keep) {
        if (busy || loading || !show() || !model || reconcileRequired) return;
        if (operation === "use" && !unresolvedUse && model.model_memory_fit !== "fits") return;
        stopRead(); const epoch = ++generation, cid = model.cid;
        const freshUse = operation === "use" && !unresolvedUse;
        const id = model.model_runtime.preparation?.operation_id;
        busy = true; message = ""; refusal = ""; statusFailures = 0; render();
        const io = controller();
        try {
          const opening = operation === "open";
          const input = opening ? { cid, keep: true } : operation === "retention" ? { cid, keep } : operation === "cancel" ? { operation_id: id } : { cid };
          if (freshUse) unresolvedUse = { cid, id: `model-v1:${Date.now()}:${crypto.randomUUID()}` };
          const result = await invoke(opening ? "retention" : operation, input, io.signal, operation === "use" ? unresolvedUse.id : undefined);
          if (epoch !== generation || !show() || model?.cid !== cid) return;
          check(result.cid === cid);
          if (operation === "retention" || opening) {
            check(result.kept === (opening ? true : keep) && typeof result.admitted === "boolean");
            if (opening) check(result.admitted);
            const catalog = await request("/api/capsules/catalog", null, io.signal);
            if (epoch !== generation || !show() || model?.cid !== cid) return;
            const candidate = parseCatalog(catalog);
            check(candidate?.cid === cid);
            model = candidate;
            if (opening) {
              check(model.model_runtime.kept && model.model_runtime.dispatch_ready);
              onReadyOpen({ cid, offer_id: model.model_runtime.offer_id });
            }
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
        } catch (error) {
          if (epoch === generation && show() && operation === "use" && error?.refusal) {
            if (error.code === "request_expired" && !freshUse) {
              unresolvedUse.expiredConfirmed = true;
              reconcileRequired = true;
            }
            else unresolvedUse = null;
            refusal = error.refusal;
          } else if (epoch === generation && show()) {
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
            message = operation === "open"
              ? "Could not confirm that this model is kept. Refresh, then open it again."
              : "Model action could not be confirmed. Refresh to check its status.";
          }
        } finally {
          io.done(); busy = false;
          if (epoch === generation) { render(); schedule(); }
          else if (show()) void load();
        }
      }
      function button(label, run, disabled = false, className = buttonClass) {
        const node = element("button", label, className);
        node.type = "button"; node.disabled = disabled || busy || loading; node.addEventListener("click", run); return node;
      }
      // One line of status: a paragraph in the full view, a list row in compact.
      function fact(value, className) {
        if (!compact) return element("p", value, className);
        const item = element("li", "", className);
        if (itemMarker) item.append(itemMarker.cloneNode(true));
        item.append(element("span", value));
        return item;
      }
      function render() {
        const hosts = [root, actionsRoot].filter(Boolean);
        const focused = hosts.some(host => host.contains(document.activeElement)) ? document.activeElement?.dataset.modelControl || pendingFocus : null;
        pendingFocus = focused;
        const restoreFocus = () => {
          if (!focused) return;
          const target = hosts.map(host => host.querySelector(`[data-model-control="${focused}"]`)).find(Boolean);
          if (target && !target.disabled) { target.focus(); pendingFocus = null; }
          else root.focus();
        };
        for (const host of hosts) host.replaceChildren();
        const refresh = button(compact ? "Refresh" : "Refresh models", () => void load(), loading);
        refresh.dataset.modelControl = "refresh";
        if (!compact) root.append(refresh);
        else if (message || refusal || statusFailures) (actionsRoot || root).append(refresh);
        const statusText = loading ? "Loading models…" : message || refusal || (statusFailures ? "Checking status…" : "");
        const status = fact(statusText);
        status.setAttribute("role", "status"); status.hidden = compact && !statusText; root.append(status);
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
            restoreFocus();
            return;
          }
          if (!loading && !message) root.append(fact("No verified model is available. Ask your administrator to configure a trusted catalog."));
          restoreFocus();
          return;
        }
        const r = model.model_runtime, p = r.preparation;
        const row = compact ? root : element("article", "", "model-row");
        if (!compact) {
          row.append(element("h2", model.title), element("p", formatBytes(model.content_size_bytes), "model-size"));
          const identity = element("details", "", "model-details");
          identity.append(element("summary", "Content details"), element("p", `Verified publisher: ${model.publisher_did}`),
            element("p", `Content ID: ${model.cid}`, "model-identity"));
          row.append(identity);
        }
        if (!activating(r, p)) finishingOperation = "";
        else if (finishingOperation !== p.operation_id) { finishingOperation = p.operation_id; finishingSince = Date.now(); }
        const phase = phaseOf(r, p, message, Boolean(finishingOperation) && Date.now() - finishingSince >= FINISHING_DEADLINE_MS);
        const label = (phase === "failed" || (["cancelled", "expired"].includes(phase) && p?.failure_class)) ? failureText(p)
          : phase === "settling" && p?.failure_class === "storage_service_unavailable" ? "The storage service is unavailable. Waiting for preparation to settle."
          : phase === "capacity" ? capacityText(p) : phase === "service_unavailable" && r.dispatch_unavailable_reason
          ? DISPATCH_UNAVAILABLE_TEXT[r.dispatch_unavailable_reason] : phase === "service_unavailable" && p?.failure_class
            ? failureText(p) : (compact ? COMPACT_PHASE_TEXT : PHASE_TEXT)[phase];
        row.append(fact(label));
        if (compact) row.append(fact(`Download size: ${formatBytes(model.content_size_bytes)}`, "model-size"));
        row.append(fact((compact ? COMPACT_MEMORY_FIT_TEXT : MEMORY_FIT_TEXT)[model.model_memory_fit], "model-memory-fit"));
        if (active(p)) {
          const progress = element("progress"); progress.max = p.total_bytes || 1; progress.value = p.completed_bytes;
          progress.setAttribute("aria-label", "Preparation progress");
          const done = fact(`${formatBytes(p.completed_bytes)} of ${formatBytes(p.total_bytes)}`);
          if (compact) { done.append(progress); row.append(done); } else row.append(progress, done);
        }
        const controls = actionsRoot || element("div", "", "model-controls");
        const useLabel = r.dispatch_unavailable_reason === "catalog_updated" ? "Use"
          : (p && p.state !== "reclaimed") || unresolvedUse ? "Retry" : compact ? "Get" : "Download";
        const use = button(useLabel, () => void act("use"), active(p) || reconcileRequired
          || (!unresolvedUse && model.model_memory_fit !== "fits"), primaryButtonClass);
        use.dataset.modelControl = "use";
        if (!active(p) && !r.dispatch_ready && phase !== "finishing") controls.append(use);
        if (r.dispatch_ready && typeof onReadyOpen === "function") {
          const open = button("Open in Assistant", () => void act("open"), reconcileRequired, primaryButtonClass);
          open.dataset.modelControl = "open-assistant";
          controls.append(open);
        }
        if (r.admitted && !active(p) && reclaimStep === "busy") {
          row.append(fact(RECLAIM_BUSY_TEXT, "model-reclaim-busy"));
        }
        if (!compact && r.admitted && !active(p) && reclaimStep === "confirm") {
          row.append(element("p", RECLAIM_CONFIRM_TEXT, "model-reclaim-confirm"));
          const decline = button("Keep this model", () => { reclaimStep = "idle"; render(); }, reconcileRequired);
          decline.dataset.modelControl = "reclaim-decline";
          const confirm = button("Remove now", () => { reclaimStep = "idle"; void act("reclaim"); }, reconcileRequired);
          confirm.dataset.modelControl = "reclaim-confirm";
          controls.append(decline, confirm);
        } else if (!compact && r.admitted && !active(p)) {
          const reclaim = button("Remove from this device", () => { reclaimStep = "confirm"; render(); }, reconcileRequired);
          reclaim.dataset.modelControl = "reclaim";
          controls.append(reclaim);
        }
        if (active(p)) { const cancel = button(compact ? "Stop" : "Cancel preparation", () => void act("cancel"), p.cancel_requested || reconcileRequired, primaryButtonClass); cancel.dataset.modelControl = "cancel"; controls.append(cancel); }
        if (!compact && reclaimStep !== "confirm") {
          const labelNode = element("label", "", "model-keep"), toggle = element("input");
          toggle.type = "checkbox"; toggle.checked = r.kept; toggle.disabled = !(r.admitted || active(p)) || p?.cancel_requested || busy || loading || reconcileRequired; toggle.dataset.modelControl = "keep";
          toggle.addEventListener("change", () => void act("retention", toggle.checked));
          labelNode.append(toggle, document.createTextNode("Keep on this device")); controls.append(labelNode);
        }
        if (!actionsRoot) row.append(controls);
        if (!compact) {
          row.append(element("p", r.kept
            ? (r.admitted ? "Kept on this device. Release this choice to allow cache cleanup." : "Keep choice saved for this preparation.")
            : "Prepared files can be removed during cache cleanup.", "model-hint"));
          row.append(element("p", "The model loads into memory when you use it in Assistant.", "model-hint"));
        }
        if (!compact) root.append(row);
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
