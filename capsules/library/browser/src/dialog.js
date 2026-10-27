import {
  contentCid,
  escapeHtml,
  formatBytes,
  formatTime,
  hasCapability,
  decimalIntegerToHexQuantity,
  inTrash,
  isBlockedObject,
  isDirectory,
  isWebSpaceUri,
  parentUri,
  pathCrumbs,
  protectionLabel,
  publishedCid,
  visibilityContract,
  viewerOptions,
} from "./model.js";

export function createLibraryDialog({
  allVisible,
  copyText,
  dialog,
  hideMenu,
  objectByUri,
  onBeforeClose,
  roots,
}) {
  let pendingDialogResolve = null;
  // Item URIs a decision dialog acts on; it never confirms once any is hidden (#441).
  let decisionTargetUris = [];
  const propertyCopyResetTimers = new WeakMap();

  // General shows what a person can read or act on, omitting a row rather
  // than showing "-"; Details keeps the one address and content ID.
  function showProperties(object) {
    const file = !isDirectory(object);
    const trashed = inTrash(object);
    const blocked = isBlockedObject(object);
    const generalRows = [["Name", object.name]];
    if (pathCrumbs(roots(), object.uri).length > 1) {
      generalRows.push(["Location", placePath(parentUri(object.uri))]);
    }
    generalRows.push(["Type", file ? object.mime || "File" : "Folder"]);
    if (file) {
      const viewers = viewerOptions(object).map((viewer) => viewer.label || viewer.id);
      generalRows.push(["Opens with", viewers.join(", ") || "No installed app"]);
      generalRows.push(["Size", Number(object.size) > 0 ? formatBytes(object.size) : "0 B"]);
    }
    if (object.modified_at) generalRows.push(["Modified", formatTime(object.modified_at)]);
    if (object.created_at) generalRows.push(["Created", formatTime(object.created_at)]);
    if (file && !trashed && !blocked && !isWebSpaceUri(object.uri)) {
      generalRows.push(["Sharing", sharingValue(object)]);
    }
    if (object.metadata?.readonly === true || object.metadata?.access_policy === "read-only") {
      generalRows.push(["Access", badgeValue("Read-only", "readonly")]);
    }
    const archive = archiveSummary(object);
    if (archive) generalRows.push(["Archive", archive]);
    const originalUri = object.metadata?.trash?.original_uri;
    if (trashed && originalUri) generalRows.push(["Original location", placePath(parentUri(originalUri))]);
    if (blocked) generalRows.push(["Problem", protectionLabel(object)]);

    const detailRows = [["Object URI", copyableValue(object.uri, "object URI")]];
    const localContentId = contentCid(object);
    if (file && localContentId) {
      detailRows.push(["Content ID", copyableValue(localContentId, "content ID", "resource.identifier")]);
    }
    const source = String(object.metadata?.target_uri || "");
    // A Space's own target is shown only when it is not another localhost://
    // address and not an elastos:// link a recipient cannot open yet.
    if (isWebSpaceUri(object.uri) && source && !source.startsWith("localhost://") && !source.startsWith("elastos://")) {
      detailRows.push(["Source", source]);
    }
    dialog.innerHTML = `
      <div class="dialog-card properties-card window-item-properties">
        <div class="properties-window-title">
          <span>${escapeHtml(object.name || "Object")} properties</span>
          <button type="button" aria-label="Close properties" data-dialog-close>&times;</button>
        </div>
        <div class="item-props-body">
          ${propertiesTable(generalRows.filter(([, value]) => value))}
          <details class="item-props-details">
            <summary>Details</summary>
            ${propertiesTable(detailRows)}
          </details>
        </div>
        <div class="properties-window-actions">
          <button class="btn btn-primary" type="button" data-dialog-close>Close</button>
        </div>
      </div>
    `;
    dialog.classList.remove("hidden");
  }

  // A folder as the breadcrumbs name it, e.g. "Documents / Projects"; empty
  // when no place holds it, so General never shows a raw localhost:// URI.
  function placePath(uri) {
    return pathCrumbs(roots(), uri).map((crumb) => crumb.label).join(" / ");
  }

  function sharingValue(object) {
    if (object.published) {
      return badgeValue("Published", "public", hasCapability(object, "unpublish") ? "Unpublish from the item menu" : "");
    }
    return badgeValue(
      "Private",
      "private",
      visibilityContract(object).placement === "public_folder" ? "Items here stay private." : "",
    );
  }

  function archiveSummary(object) {
    const support = object.metadata?.archive_support;
    if (support?.status === "extractable" && hasCapability(object, "extract_archive")) return "Can be extracted";
    if (support?.status === "policy_gated_unsupported_archive_family") {
      return `Library can't extract ${support.family || "these"} archives yet`;
    }
    return "";
  }

  function propertiesTable(rows) {
    return `
      <table class="item-props-tbl">
        <tbody>
          ${rows.map(([label, value]) => propertiesRow(label, value)).join("")}
        </tbody>
      </table>
    `;
  }

  function propertiesRow(label, value) {
    const rendered = propertiesValue(value);
    return `
      <tr>
        <td class="item-prop-label">${escapeHtml(label)}</td>
        <td class="item-prop-val" title="${escapeHtml(rendered.title)}">${rendered.html}</td>
      </tr>
    `;
  }

  function propertiesValue(value) {
    if (value && typeof value === "object" && value.kind === "copyable") {
      const text = String(value.value);
      return {
        title: text,
        html: `
          <span class="props-copy-value">
            <code class="props-copy-text">${escapeHtml(text)}</code>
            <button class="props-copy-btn el-copy-btn" type="button" data-prop-copy="${escapeHtml(text)}" data-copy-label="${escapeHtml(value.label)}" data-copy-purpose="${escapeHtml(value.purpose)}" title="Copy ${escapeHtml(value.label)}">
              ${copyIconSvg()}
            </button>
          </span>
        `,
      };
    }
    if (value && typeof value === "object" && value.kind === "badge") {
      const text = String(value.value);
      const hint = value.hint ? `<span class="item-prop-hint">${escapeHtml(value.hint)}</span>` : "";
      return {
        title: value.hint ? `${text}. ${value.hint}` : text,
        html: `<span class="item-prop-badge item-prop-badge-${escapeHtml(value.tone)}">${escapeHtml(text)}</span>${hint}`,
      };
    }
    const text = String(value);
    return { title: text, html: escapeHtml(text) };
  }

  function copyableValue(value, label, purpose = "resource.uri") {
    return { kind: "copyable", value, label, purpose };
  }

  function badgeValue(value, tone, hint = "") {
    return { kind: "badge", value, tone, hint };
  }

  function copyIconSvg() {
    return [
      '<svg class="el-copy-icon" aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg>',
      '<svg class="el-copy-check" aria-hidden="true" width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" hidden><path d="M3.25 8.25l3 3 6.5-6.5"></path></svg>',
    ].join("");
  }

  function setPropertyCopyFeedback(button, label, copied) {
    const pendingReset = propertyCopyResetTimers.get(button);
    if (pendingReset) {
      clearTimeout(pendingReset);
      propertyCopyResetTimers.delete(button);
    }
    const copyIcon = button.querySelector(".el-copy-icon");
    const checkIcon = button.querySelector(".el-copy-check");
    if (!copied) {
      button.classList.remove("copied");
      delete button.dataset.copied;
      button.removeAttribute("aria-label");
      if (copyIcon) copyIcon.hidden = false;
      if (checkIcon) checkIcon.hidden = true;
      return;
    }
    button.classList.add("copied");
    button.dataset.copied = "true";
    button.setAttribute("aria-label", `Copied ${label}`);
    if (copyIcon) copyIcon.hidden = true;
    if (checkIcon) checkIcon.hidden = false;
    const timer = setTimeout(() => {
      propertyCopyResetTimers.delete(button);
      setPropertyCopyFeedback(button, label, false);
    }, 1400);
    propertyCopyResetTimers.set(button, timer);
  }

  function smartWebIdentity(object = {}, record = null) {
    const metadata = object.metadata || {};
    const publicContentId = record?.cid || publishedCid(object);
    const resolver = metadata.resolver || metadata.provider || "";
    const kind = publicContentId
      ? "Published content object"
      : contentCid(object)
        ? "Local content object"
      : resolver
        ? "Mounted Space object"
        : "Local Library object";
    return {
      kind,
      contentUri: publicContentId ? `elastos://${publicContentId}` : "-",
      objectId: metadata.object_id || object.uri || "-",
    };
  }

  function safeAvailabilitySummary(availability) {
    if (!availability || typeof availability === "string") {
      const status = availability || "local-only";
      return {
        status,
        replicas: "-",
        liveProof: "no",
        quota: "-",
        repair: status === "repair_needed" ? "needed" : "-",
        storageMarket: "-",
        details: {
          schema: "elastos.library.availability-summary/v1",
          status,
          proof: "not_provided",
        },
      };
    }
    const peerSelection = availability.peer_selection || {};
    const quota = availability.quota || {};
    const repair = availability.repair_worker || {};
    const accounting = availability.accounting || null;
    const abuseControls = availability.abuse_controls || null;
    const storageMarket = availability.storage_market || {};
    const remoteReplicas = Number(peerSelection.remote_replicas ?? countRemotePeerRows(peerSelection));
    const liveProof = peerSelection.live_multi_peer_proof === true;
    const quotaStatus = quota.status || (quota.enforced ? "enforced" : "not_enforced");
    const repairStatus = repair.status || (availability.status === "repair_needed" ? "needed" : "-");
    return {
      status: availability.status || "unknown",
      replicas: Number.isFinite(Number(availability.replicas)) ? String(availability.replicas) : "-",
      liveProof: liveProof ? "yes" : "no",
      quota: quotaStatus || "-",
      repair: repairStatus,
      storageMarket: storageMarket.status || storageMarket.settlement || "-",
      details: {
        schema: "elastos.library.availability-summary/v1",
        status: availability.status || "unknown",
        provider: availability.provider || "-",
        policy: availability.policy || "-",
        replicas: availability.replicas ?? null,
        peer_selection: {
          mode: peerSelection.mode || "-",
          strategy: peerSelection.strategy || "-",
          live_multi_peer_proof: liveProof,
          remote_replicas: remoteReplicas,
          truncated: peerSelection.replicas_truncated ?? peerSelection.recent_remote_replicas_truncated ?? false,
        },
        quota: {
          status: quota.status || null,
          enforced: quota.enforced === true,
          used_replicas: quota.used_replicas ?? null,
          effective_max_replicas: quota.effective_max_replicas ?? null,
        },
        repair_worker: {
          status: repair.status || null,
          scheduled: repair.scheduled === true,
        },
        storage_market: {
          mode: storageMarket.mode || null,
          status: storageMarket.status || null,
          settlement: storageMarket.settlement || null,
          quota_enforced: storageMarket.quota_enforced === true,
        },
        accounting: accounting ? {
          content_bytes: accounting.content_bytes ?? null,
          replica_bytes_estimate: accounting.replica_bytes_estimate ?? null,
          storage_quota_status: accounting.storage_quota?.status || accounting.storage_quota_status || null,
        } : null,
        abuse_controls: abuseControls ? {
          policy: abuseControls.policy || null,
          enforced: abuseControls.enforced === true,
          attempted_operations: abuseControls.attempted_operations ?? null,
          failed_operations: abuseControls.failed_operations ?? null,
          throttled: abuseControls.throttled === true,
        } : null,
      },
    };
  }

  function countRemotePeerRows(peerSelection) {
    const replicas = Array.isArray(peerSelection?.replicas) ? peerSelection.replicas : [];
    return replicas.filter((replica) => replica?.role === "remote").length;
  }

  function safePublishReceiptSummary(receipt) {
    if (!receipt) return { status: "not_available" };
    const payload = receipt.payload || receipt;
    return {
      schema: payload.schema || "elastos.content.availability.receipt/v1",
      status: payload.status || null,
      provider: payload.provider || null,
      policy: payload.policy || null,
      replicas: payload.replicas ?? null,
      checked_at: payload.checked_at ?? null,
      signer_did: receipt.signer_did || payload.signer_did || null,
      verified: receipt.verified === true || payload.verified === true,
    };
  }

  function safeRemoteAccessSummary(payload = {}, object = {}) {
    const metadata = object.metadata || payload.metadata || {};
    const providerStatus = payload.protected_content
      || metadata.protected_content
      || null;
    const grants = Array.isArray(payload.grants)
      ? payload.grants
      : Array.isArray(payload.share_grants)
        ? payload.share_grants
        : [];
    const firstGrantKeyRelease = grants.find((grant) => grant?.key_release)?.key_release || null;
    const keyRelease = payload.key_release
      || firstGrantKeyRelease
      || metadata.key_release
      || null;
    const remoteEnforcement = payload.remote_enforcement
      || payload.access?.open?.remote_enforcement
      || payload.open?.remote_enforcement
      || metadata.remote_enforcement
      || null;
    const policy = payload.policy
      || payload.share_policy
      || metadata.share_policy
      || (object.shared ? "shared" : "not_shared");
    const keyReleaseRequired = keyRelease?.required === true || remoteEnforcement?.key_release_required === true;
    const recipientProofRequired = remoteEnforcement?.recipient_proof_required === true || policy === "recipient_scoped";
    const keyReleaseStatus = keyRelease?.status
      || remoteEnforcement?.key_release_status
      || (keyReleaseRequired ? "required" : "not_required");
    const openStatus = remoteEnforcement?.status
      || (keyReleaseRequired
        ? "blocked_until_drm_rights_key_decrypt_providers"
        : recipientProofRequired
          ? "recipient_proof_enforced_by_runtime"
          : policy === "public_link"
            ? "public_link_ready"
            : "not_shared");
    const status = policy === "recipient_scoped"
      ? "recipient scoped"
      : policy === "public_link"
        ? "public link"
        : object.shared
          ? "shared"
          : "not shared";
    return {
      status,
      openStatus,
      keyRelease: keyReleaseStatus,
      details: {
        schema: "elastos.library.remote-access-summary/v1",
        policy,
        provider_gate: remoteEnforcement?.provider_gate || "object-provider shared_access",
        recipient_proof_required: recipientProofRequired,
        key_release_required: keyReleaseRequired,
        key_release_status: keyReleaseStatus,
        open_status: openStatus,
        required_providers: remoteEnforcement?.required_providers || keyRelease?.required_providers || null,
        provider_invocation: remoteEnforcement?.provider_invocation || null,
        provider_status: providerStatus,
        next: remoteEnforcement?.next || keyRelease?.next || null,
      },
    };
  }

  function showObjectStatus(payload) {
    const object = payload?.object || {};
    const record = payload?.published || null;
    const availability = record?.availability || object.availability || "local-only";
    const identity = smartWebIdentity(object, record);
    const availabilitySummary = safeAvailabilitySummary(availability);
    const receipt = record?.receipt || null;
    const shareGrants = Array.isArray(record?.share_grants) ? record.share_grants : [];
    const firstGrantKeyRelease = shareGrants.find((grant) => grant?.key_release)?.key_release || null;
    const contentSecurity = record?.content_security || null;
    const remoteAccess = safeRemoteAccessSummary({
      policy: record?.share_policy,
      grants: shareGrants,
      key_release: firstGrantKeyRelease,
      remote_enforcement: record?.remote_enforcement,
      content_security: contentSecurity,
      protected_content: payload?.protected_content || record?.protected_content,
    }, object);
    const protectedContent = payload?.protected_content || record?.protected_content || null;
    dialog.innerHTML = `
      <div class="dialog-card dialog-card-wide">
        <div>
          <p class="eyebrow">Availability</p>
          <h2>${escapeHtml(object.name || "Item status")}</h2>
          <p class="subtitle">Availability and sharing</p>
        </div>
        <div class="details">
          <div><strong>Stored</strong><br>${object.published ? "Published" : "On this device"}</div>
          <div><strong>Shared</strong><br>${object.shared ? "Yes" : "No"}</div>
        </div>
        <details class="details-json">
          <summary>Technical details</summary>
          <div class="details">
          <div><strong>SmartWeb Object</strong><br>${escapeHtml(identity.kind)}</div>
          <div><strong>Content URI</strong><br>${escapeHtml(identity.contentUri)}</div>
          <div><strong>Availability</strong><br>${escapeHtml(availabilitySummary.status)}</div>
          <div><strong>Live Proof</strong><br>${escapeHtml(availabilitySummary.liveProof)}</div>
          <div><strong>Replicas</strong><br>${escapeHtml(availabilitySummary.replicas)}</div>
          <div><strong>Quota</strong><br>${escapeHtml(availabilitySummary.quota)}</div>
          <div><strong>Repair</strong><br>${escapeHtml(availabilitySummary.repair)}</div>
          <div><strong>Storage Market</strong><br>${escapeHtml(availabilitySummary.storageMarket)}</div>
          <div><strong>Published</strong><br>${object.published ? "yes" : "no"}</div>
          <div><strong>Shared</strong><br>${object.shared ? "yes" : "no"}</div>
          <div><strong>Object ID</strong><br>${escapeHtml(identity.objectId)}</div>
          <div><strong>Published At</strong><br>${escapeHtml(formatTime(record?.published_at))}</div>
          <div><strong>Unpublished At</strong><br>${escapeHtml(formatTime(record?.unpublished_at))}</div>
          <div><strong>Shared At</strong><br>${escapeHtml(formatTime(record?.shared_at))}</div>
          <div><strong>Share Policy</strong><br>${escapeHtml(record?.share_policy || "not shared")}</div>
          <div><strong>Share Grants</strong><br>${escapeHtml(String(shareGrants.length))}</div>
          <div><strong>Remote Open</strong><br>${escapeHtml(remoteAccess.openStatus)}</div>
          <div><strong>Provider Chain</strong><br>${escapeHtml(protectedContent?.encrypted_recipient_sharing?.status || "-")}</div>
          <div><strong>Payload</strong><br>${escapeHtml(contentSecurity?.published_payload || "-")}</div>
          <div><strong>Key Release</strong><br>${escapeHtml(remoteAccess.keyRelease || contentSecurity?.status || "not required")}</div>
          <div><strong>Revision</strong><br>${escapeHtml(object.revision || "-")}</div>
          </div>
          <div class="details-json">
          <strong>Remote Access Policy</strong>
          <pre>${escapeHtml(JSON.stringify(remoteAccess.details, null, 2))}</pre>
          </div>
          ${protectedContent ? `<div class="details-json">
          <strong>Protected Content Providers</strong>
          <pre>${escapeHtml(JSON.stringify(protectedContent, null, 2))}</pre>
          </div>` : ""}
          <div class="details-json">
          <strong>Availability Summary</strong>
          <pre>${escapeHtml(JSON.stringify(availabilitySummary.details, null, 2))}</pre>
          </div>
          <div class="details-json">
          <strong>Share Grants / Key Release</strong>
          <pre>${escapeHtml(JSON.stringify({
            policy: record?.share_policy || null,
            grants: shareGrants,
            content_security: contentSecurity,
          }, null, 2))}</pre>
          </div>
          <div class="details-json">
          <strong>Publish Receipt Summary</strong>
          <pre>${escapeHtml(JSON.stringify(safePublishReceiptSummary(receipt), null, 2))}</pre>
          </div>
        </details>
        <div class="button-row">
          <button class="btn" type="button" data-dialog-action="properties">Properties</button>
          <button class="btn btn-primary" type="button" data-dialog-close>Close</button>
        </div>
      </div>
    `;
    dialog.dataset.previewUri = object.uri || "";
    dialog.classList.remove("hidden");
  }

  function showSharedAccessReceipt(payload) {
    const object = payload?.object || {};
    const access = payload?.access || {};
    const decision = access.decision || {};
    const open = access.open || {};
    const keyRelease = access.key_release || open.key_release || null;
    const remoteAccess = safeRemoteAccessSummary({
      access,
      key_release: keyRelease,
      policy: decision.policy || open.policy,
      protected_content: payload?.protected_content,
    }, object);
    dialog.innerHTML = `
      <div class="dialog-card dialog-card-wide">
        <div>
          <p class="eyebrow">Access Check</p>
          <h2>${escapeHtml(object.name || "Shared item")}</h2>
          <p class="subtitle">Access was checked for this account.</p>
        </div>
        <div class="details">
          <div><strong>Decision</strong><br>${escapeHtml(decision.allowed === false ? "denied" : "allowed")}</div>
          <div><strong>Policy</strong><br>${escapeHtml(decision.policy || open.policy || "-")}</div>
          <div><strong>Recipient</strong><br>${escapeHtml(decision.recipient || open.recipient || "-")}</div>
          <div><strong>Reason</strong><br>${escapeHtml(decision.reason || "-")}</div>
          <div><strong>Content URI</strong><br>${escapeHtml(payload?.uri || (payload?.cid ? `elastos://${payload.cid}` : "-"))}</div>
          <div><strong>Open Status</strong><br>${escapeHtml(open.status || remoteAccess.openStatus || "-")}</div>
          <div><strong>Key Release</strong><br>${escapeHtml(remoteAccess.keyRelease || keyRelease?.status || "not required")}</div>
          <div><strong>Payload</strong><br>${escapeHtml(open.published_payload || "-")}</div>
        </div>
        <details class="details-json">
          <summary>Technical details</summary>
          <pre>${escapeHtml(JSON.stringify({
            open,
            remote_access: remoteAccess.details,
            receipt: access,
          }, null, 2))}</pre>
        </details>
        <div class="button-row">
          <button class="btn" type="button" data-dialog-action="properties">Properties</button>
          <button class="btn btn-primary" type="button" data-dialog-close>Close</button>
        </div>
      </div>
    `;
    dialog.dataset.previewUri = object.uri || "";
    dialog.classList.remove("hidden");
  }

  function showProtectAndListDialog(object) {
    hideMenu();
    hideDialog();
    decisionTargetUris = [object.uri];
    return new Promise((resolve) => {
      pendingDialogResolve = resolve;
      dialog.innerHTML = `
        <div class="dialog-card" role="dialog" aria-modal="true" aria-labelledby="protect-and-list-title">
          <form data-protect-and-list-form aria-describedby="protect-and-list-summary protect-and-list-hint">
            <div>
              <p class="eyebrow">Protect and List</p>
              <h2 id="protect-and-list-title" data-protect-and-list-object>${escapeHtml(object.name || "Video")}</h2>
              <p class="subtitle">Review the listing terms before you submit them.</p>
            </div>
            <label class="dialog-field">
              <span>Copies</span>
              <input name="copies" type="text" inputmode="numeric" autocomplete="off" value="1" maxlength="78" required>
            </label>
            <label class="dialog-field">
              <span>Price (base units)</span>
              <input name="price" type="text" inputmode="numeric" autocomplete="off" placeholder="1000000000000000000" maxlength="78" required>
            </label>
            <p class="dialog-hint protect-and-list-summary" id="protect-and-list-summary" data-protect-and-list-summary></p>
            <p class="dialog-hint" id="protect-and-list-hint">Price and copies use positive whole-number base units.</p>
            <p class="dialog-error hidden" role="alert" data-protect-and-list-error></p>
            <div class="button-row">
              <button class="btn" type="button" data-dialog-close>Cancel</button>
              <button class="btn btn-primary" type="submit">Protect and List</button>
            </div>
          </form>
        </div>
      `;
      dialog.classList.remove("hidden");
      const form = dialog.querySelector("[data-protect-and-list-form]");
      updateProtectAndListSummary(form);
      form?.querySelector('input[name="copies"]')?.focus();
    });
  }

  function updateProtectAndListSummary(form) {
    if (!form) return;
    const objectName = form.querySelector("[data-protect-and-list-object]")?.textContent || "video";
    const copies = String(form.elements?.copies?.value || "").trim();
    const price = String(form.elements?.price?.value || "").trim();
    const summary = form.querySelector("[data-protect-and-list-summary]");
    if (!summary) return;
    summary.textContent = copies && price
      ? `Protect ${objectName} and request a listing for ${copies} ${copies === "1" ? "copy" : "copies"} at ${price} base units each.`
      : `Protect ${objectName}, then create one listing with the copies and price shown here.`;
  }

  function confirmDestructive({ title, message, confirmLabel, targetUris = [] }) {
    hideMenu();
    hideDialog();
    decisionTargetUris = targetUris;
    return new Promise((resolve) => {
      pendingDialogResolve = resolve;
      dialog.innerHTML = `
        <div class="dialog-card">
          <div>
            <p class="eyebrow">Confirm</p>
            <h2>${escapeHtml(title)}</h2>
            <p class="subtitle">${escapeHtml(message)}</p>
          </div>
          <div class="button-row">
            <button class="btn" type="button" data-dialog-close>Cancel</button>
            <button class="btn btn-danger" type="button" data-dialog-confirm>${escapeHtml(confirmLabel)}</button>
          </div>
        </div>
      `;
      dialog.classList.remove("hidden");
    });
  }

  function resolveDialogDecision(value) {
    if (!pendingDialogResolve) return;
    const resolve = pendingDialogResolve;
    const targetsVisible = allVisible(decisionTargetUris);
    pendingDialogResolve = null;
    decisionTargetUris = [];
    resolve(value && !targetsVisible ? false : value);
  }

  function hideDialog() {
    onBeforeClose();
    delete dialog.dataset.previewUri;
    dialog.classList.add("hidden");
    dialog.innerHTML = "";
    resolveDialogDecision(false);
  }

  function hideDialogIfTargetsHidden() {
    if (pendingDialogResolve && decisionTargetUris.length && !allVisible(decisionTargetUris)) hideDialog();
  }

  function bindDialogEvents() {
    dialog.addEventListener("input", (event) => {
      const form = event.target.closest("[data-protect-and-list-form]");
      if (form) updateProtectAndListSummary(form);
    });
    dialog.addEventListener("submit", (event) => {
      const protectForm = event.target.closest("[data-protect-and-list-form]");
      if (!protectForm) return;
      event.preventDefault();
      const formData = new FormData(protectForm);
      const copies = String(formData.get("copies") || "").trim();
      const price = String(formData.get("price") || "").trim();
      const error = protectForm.querySelector("[data-protect-and-list-error]");
      try {
        decimalIntegerToHexQuantity(copies);
      } catch (validationError) {
        if (error) {
          error.textContent = validationError?.message || "Copies must be a valid quantity.";
          error.classList.remove("hidden");
        }
        return;
      }
      try {
        decimalIntegerToHexQuantity(price);
      } catch (validationError) {
        if (error) {
          error.textContent = validationError?.message || "Price must be a valid quantity.";
          error.classList.remove("hidden");
        }
        return;
      }
      resolveDialogDecision({ copies, price });
      hideDialog();
    });

    dialog.addEventListener("click", (event) => {
      if (event.target.closest("[data-dialog-confirm]")) {
        resolveDialogDecision(true);
        hideDialog();
        return;
      }
      const propertyCopy = event.target.closest("[data-prop-copy]");
      if (propertyCopy) {
        const value = propertyCopy.getAttribute("data-prop-copy") || "";
        const label = propertyCopy.getAttribute("data-copy-label") || "value";
        const purpose =
          propertyCopy.getAttribute("data-copy-purpose") || "resource.uri";
        if (value && copyText) {
          copyText(value, label, purpose)
            .then(() => {
              setPropertyCopyFeedback(propertyCopy, label, true);
            })
            .catch(() => {
              setPropertyCopyFeedback(propertyCopy, label, false);
            });
        }
        return;
      }
      if (event.target.closest('[data-dialog-action="properties"]')) {
        const object = objectByUri(dialog.dataset.previewUri);
        if (object) showProperties(object);
        return;
      }
      if (event.target === dialog || event.target.closest("[data-dialog-close]")) {
        hideDialog();
      }
    });
  }

  return {
    bindDialogEvents,
    confirmDestructive,
    hideDialog,
    hideDialogIfTargetsHidden,
    showProtectAndListDialog,
    showObjectStatus,
    showProperties,
    showSharedAccessReceipt,
  };
}
