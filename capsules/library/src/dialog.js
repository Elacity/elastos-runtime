import {
  contentCid,
  escapeHtml,
  formatBytes,
  formatTime,
  hasCapability,
  isDirectory,
  parentUri,
  publishedCid,
  shortUri,
  visibilityContract,
  viewerOptions,
} from "./model.js";

export function createLibraryDialog({
  copyText,
  dialog,
  hideMenu,
  objectByUri,
  onBeforeClose,
}) {
  let pendingDialogResolve = null;

  function showProperties(object) {
    const identity = smartWebIdentity(object);
    const availability = safeAvailabilitySummary(object.availability);
    const remoteAccess = safeRemoteAccessSummary({}, object);
    const archive = safeArchiveSummary(object);
    const viewers = viewerOptions(object).map((viewer) => viewer.label || viewer.id);
    const typeLabel = isDirectory(object) ? "Folder" : object.mime || "File";
    const location = parentUri(object.uri || "");
    const visibility = propertiesVisibilitySummary(object, identity, remoteAccess);
    const placement = propertiesPlacementSummary(object);
    const generalRows = [
      ["Name", object.name || "-"],
      ["Path", copyableValue(object.uri || "-", "path")],
      ["UID", identity.objectId],
      ["Type", typeLabel],
      ["Opens with", viewers.join(", ") || "-"],
      ["Where", location || "-"],
      ["Size", isDirectory(object) ? "-" : formatBytes(object.size)],
      ["Modified", formatTime(object.modified_at)],
      ["Created", formatTime(object.created_at)],
      ["SmartWeb Object", identity.kind],
      ["Content ID", copyableValue(identity.contentId, "content CID")],
      ["Published CID", copyableValue(identity.publishedId, "published CID")],
      ["Published Link", copyableValue(identity.publishedLink, "published link")],
      ["Placement", badgeValue(placement.label, placement.tone)],
      ["Visibility", badgeValue(visibility.label, visibility.tone)],
      ["Access granted to", remoteAccess.status],
    ];
    if (archive.relevant) {
      generalRows.push(["Archive", archive.status]);
    }
    const runtimeRows = [
      ["Provider", identity.provider],
      ["Object URI", copyableValue(object.uri || "-", "object URI")],
      ["Content URI", copyableValue(identity.contentUri, "content URI")],
      ["Head", copyableValue(identity.headId, "object head")],
      ["Revision", object.revision || "-"],
      ["Resolver", identity.resolver],
      ["Resolver Target", copyableValue(identity.resolverTarget, "resolver target")],
      ["Access Policy", identity.accessPolicy],
      ["Availability", availability.status],
      ["Replicas", availability.replicas],
      ["Live Proof", availability.liveProof],
      ["Quota", availability.quota],
      ["Repair", availability.repair],
      ["Storage Market", availability.storageMarket],
      ["Remote Open", remoteAccess.openStatus],
      ["Key Release", remoteAccess.keyRelease],
      ["Protection", object.blocked_reason || "ok"],
      ["Public Folder Policy", placement.policy],
      ["Visibility Contract", object.metadata?.visibility?.schema || "elastos.library.visibility/v1"],
    ];
    const archiveTab = archive.relevant
      ? `<div class="item-props-tab-btn antialiased disable-user-select" data-tab="archive">Archive</div>`
      : "";
    const archivePanel = archive.relevant
      ? propertiesPanel("archive", [
          ["Status", archive.status],
          ["Family", archive.details?.object?.family || "-"],
          ["Extractable", archive.details?.object?.extractable ? "yes" : "no"],
          ["Compressible", archive.details?.object?.compressible ? "yes" : "no"],
          ["Download formats", archive.details?.implemented?.download_formats?.join(", ") || "-"],
          ["Extract formats", archive.details?.implemented?.extract_formats?.join(", ") || "-"],
          ["Safety", archive.details?.implemented?.safety || "-"],
          ["Remaining policy", archive.details?.remaining_policy || "-"],
        ])
      : "";
    dialog.innerHTML = `
      <div class="dialog-card properties-card window-item-properties">
        <div class="properties-window-title">
          <span>${escapeHtml(object.name || "Object")} properties</span>
          <button type="button" aria-label="Close properties" data-dialog-close>&times;</button>
        </div>
        <div class="item-props-tabview">
          <div class="item-props-tab">
            <div class="item-props-tab-btn antialiased disable-user-select item-props-tab-selected" data-tab="general">General</div>
            <div class="item-props-tab-btn antialiased disable-user-select" data-tab="runtime">Runtime</div>
            ${archiveTab}
          </div>
          ${propertiesPanel("general", generalRows, true)}
          ${propertiesPanel("runtime", runtimeRows)}
          ${archivePanel}
        </div>
        <div class="properties-window-actions">
          <button class="btn btn-primary" type="button" data-dialog-close>Close</button>
        </div>
      </div>
    `;
    dialog.classList.remove("hidden");
  }

  function propertiesPanel(tab, rows, selected = false) {
    return `
      <div class="item-props-tab-content${selected ? " item-props-tab-content-selected" : ""}" data-tab="${escapeHtml(tab)}"${selected ? ' style="border-top-left-radius:0;"' : ""}>
        <table class="item-props-tbl">
          <tbody>
            ${rows.map(([label, value]) => propertiesRow(label, value)).join("")}
          </tbody>
        </table>
      </div>
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
      const text = displayValue(value.value);
      if (text === "-") return { title: text, html: escapeHtml(text) };
      const label = value.label || "value";
      return {
        title: text,
        html: `
          <span class="props-copy-value">
            <code class="props-copy-text">${escapeHtml(text)}</code>
            <button class="props-copy-btn" type="button" data-prop-copy="${escapeHtml(text)}" data-copy-label="${escapeHtml(label)}" title="Copy ${escapeHtml(label)}">
              ${copyIconSvg()}
            </button>
          </span>
        `,
      };
    }
    if (value && typeof value === "object" && value.kind === "badge") {
      const text = displayValue(value.value);
      return {
        title: text,
        html: `<span class="item-prop-badge item-prop-badge-${escapeHtml(value.tone || "neutral")}">${escapeHtml(text)}</span>`,
      };
    }
    const text = displayValue(value);
    return { title: text, html: escapeHtml(text) };
  }

  function copyableValue(value, label) {
    return { kind: "copyable", value, label };
  }

  function badgeValue(value, tone) {
    return { kind: "badge", value, tone };
  }

  function displayValue(value) {
    return value === null || value === undefined || value === "" ? "-" : String(value);
  }

  function copyIconSvg() {
    return '<svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg>';
  }

  function propertiesVisibilitySummary(object = {}, identity = {}, remoteAccess = {}) {
    const visibility = visibilityContract(object);
    if (visibility.effective_access === "public_content_link") {
      return object.shared
        ? { label: "Published and shared", tone: "public" }
        : { label: "Published link", tone: "public" };
    }
    if (visibility.effective_access === "recipient_scoped_link") {
      return { label: "Recipient scoped", tone: "shared" };
    }
    if (visibility.effective_access === "blocked") {
      return { label: "Blocked", tone: "readonly" };
    }
    if (visibility.placement === "public_folder") {
      return { label: "Private until published", tone: "staged" };
    }
    if (identity.publishedId && identity.publishedId !== "-") {
      return object.shared
        ? { label: "Published and shared", tone: "public" }
        : { label: "Published", tone: "public" };
    }
    if (remoteAccess.status === "recipient scoped") {
      return { label: "Recipient scoped", tone: "shared" };
    }
    if (object.shared) {
      return { label: "Shared", tone: "shared" };
    }
    if (object.metadata?.readonly === true) {
      return { label: "Mounted read-only", tone: "readonly" };
    }
    if (object.metadata?.readonly === false) {
      return { label: "Mounted writable", tone: "writable" };
    }
    return { label: "Private", tone: "private" };
  }

  function propertiesPlacementSummary(object = {}) {
    const visibility = visibilityContract(object);
    const placement = visibility.placement || "private_folder";
    if (placement === "public_folder") {
      return {
        label: visibility.placement_label || "Public folder",
        tone: "staged",
        policy: "Placement only; publish creates the public content link.",
      };
    }
    if (placement === "trash") {
      return {
        label: "Trash",
        tone: "readonly",
        policy: "Trash is not public and cannot publish until restored.",
      };
    }
    if (placement === "runtime_private") {
      return {
        label: "Runtime private",
        tone: "readonly",
        policy: "Runtime private objects are hidden from normal publishing.",
      };
    }
    return {
      label: visibility.placement_label || "Private folder",
      tone: "private",
      policy: "Private placement; publish creates an explicit content-provider receipt.",
    };
  }

  function smartWebIdentity(object = {}, record = null) {
    const metadata = object.metadata || {};
    const localContentId = contentCid(object);
    const publicContentId = record?.cid || publishedCid(object);
    const contentUri = publicContentId ? `elastos://${publicContentId}` : "-";
    const resolver = metadata.resolver || metadata.provider || "";
    const resolverTarget = metadata.target_uri || metadata.resolver_target || metadata.source_uri || "";
    const accessPolicy = metadata.access_policy || (
      metadata.readonly === false
        ? "owner-writable"
        : metadata.readonly === true
          ? "read-only"
          : "-"
    );
    const kind = publicContentId
      ? "Published content object"
      : localContentId
        ? "Local content object"
      : resolver
        ? "Mounted Space object"
        : "Local Library object";
    const provider = publicContentId
      ? "content-provider"
      : resolver
        ? `${resolver} resolver`
        : "object-provider";
    const objectId = metadata.object_id || object.uri || "-";
    return {
      kind,
      provider,
      contentId: localContentId || "-",
      publishedId: publicContentId || "-",
      contentUri,
      publishedLink: publicContentId ? `elastos://${publicContentId}` : "-",
      objectId,
      headId: metadata.head_id || object.revision || "-",
      resolver: resolver || "-",
      resolverTarget: resolverTarget || "-",
      accessPolicy: accessPolicy || "-",
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

  function safeShareReceiptSummary(payload) {
    if (!payload) return { status: "not_available" };
    return {
      schema: payload.schema || "elastos.library.share/v1",
      policy: payload.policy || null,
      uri: payload.uri || (payload.cid ? `elastos://${payload.cid}` : null),
      object_uri: payload.object_uri || payload.object?.uri || null,
      recipients: Array.isArray(payload.recipients) ? payload.recipients.length : 0,
      grants: Array.isArray(payload.grants) ? payload.grants.length : 0,
      availability: safeAvailabilitySummary(payload.availability || payload.object?.availability || null).details,
      remote_access: safeRemoteAccessSummary(payload, payload.object || {}).details,
      key_release_required: payload.key_release?.required === true,
      shared_at: payload.shared_at || null,
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

  function safeArchiveSummary(object = {}) {
    const backend = object.metadata?.archive_support || null;
    const name = String(object.name || "").toLowerCase();
    const mime = String(object.mime || "").toLowerCase();
    const family = backend?.family || archiveFamilyForName(name, mime);
    const extractable = hasCapability(object, "extract_archive")
      && (name.endsWith(".zip")
        || name.endsWith(".tar")
        || name.endsWith(".tar.gz")
        || name.endsWith(".tgz"));
    const policyGated = backend?.status === "policy_gated_unsupported_archive_family"
      || (!!family && !extractable && !["zip", "tar", "tar.gz"].includes(family));
    const compressible = hasCapability(object, "compress_archive");
    const archiveLike = extractable
      || policyGated
      || !!backend
      || mime.includes("zip")
      || mime.includes("tar")
      || name.endsWith(".gz");
    return {
      relevant: archiveLike,
      status: extractable
        ? "extractable"
        : policyGated
          ? "policy-gated archive"
        : compressible
          ? "can compress/download"
          : archiveLike
            ? "view only"
            : "-",
      details: {
        schema: "elastos.library.archive-support/v1",
        backend,
        implemented: {
          download_formats: ["zip", "tar.gz"],
          compress_to_library: ["zip"],
          extract_formats: ["zip", "tar", "tar.gz", "tgz"],
          safety: "relative UTF-8 file paths only; non-file archive entries are rejected",
        },
        object: {
          name: object.name || null,
          mime: object.mime || null,
          family,
          extractable,
          compressible,
          policy_gated: policyGated,
        },
        remaining_policy: policyGated
          ? "This archive family is recognized but disabled pending dependency and release-policy review."
          : "Other generic archive families need dependency and release-policy review before enabling.",
      },
    };
  }

  function archiveFamilyForName(name, mime = "") {
    if (name.endsWith(".tar.gz") || name.endsWith(".tgz")) return "tar.gz";
    if (name.endsWith(".tar")) return "tar";
    if (name.endsWith(".zip") || mime.includes("zip")) return "zip";
    if (name.endsWith(".tar.xz") || name.endsWith(".txz")) return "tar.xz";
    if (name.endsWith(".tar.bz2") || name.endsWith(".tbz2")) return "tar.bz2";
    if (name.endsWith(".tar.zst") || name.endsWith(".tzst")) return "tar.zst";
    if (name.endsWith(".7z")) return "7z";
    if (name.endsWith(".rar")) return "rar";
    if (name.endsWith(".xz")) return "xz";
    if (name.endsWith(".bz2")) return "bz2";
    if (name.endsWith(".zst")) return "zst";
    if (name.endsWith(".lz4")) return "lz4";
    if (name.endsWith(".gz")) return "gzip";
    if (mime.includes("tar")) return "tar";
    return "";
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
          <h2>${escapeHtml(object.name || "Object status")}</h2>
          <p class="subtitle">${escapeHtml(shortUri(object.uri || ""))}</p>
        </div>
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
        <div class="button-row">
          <button class="btn" type="button" data-dialog-action="properties">Properties</button>
          <button class="btn btn-primary" type="button" data-dialog-close>Close</button>
        </div>
      </div>
    `;
    dialog.dataset.previewUri = object.uri || "";
    dialog.classList.remove("hidden");
  }

  function showShareReceipt(payload) {
    const object = payload?.object || {};
    const uri = payload?.uri || (payload?.cid ? `elastos://${payload.cid}` : "");
    const availability = payload?.availability || object.availability || "unknown";
    const identity = smartWebIdentity(object, { cid: payload?.cid, availability });
    const availabilitySummary = safeAvailabilitySummary(availability);
    const policy = payload?.policy || "public_link";
    const recipientCount = Array.isArray(payload?.recipients) ? payload.recipients.length : 0;
    const grantCount = Array.isArray(payload?.grants) ? payload.grants.length : 0;
    const keyRelease = payload?.key_release || payload?.grants?.find((grant) => grant?.key_release)?.key_release || null;
    const contentSecurity = payload?.content_security || null;
    const remoteAccess = safeRemoteAccessSummary(payload, object);
    const protectedContent = payload?.protected_content || null;
    dialog.innerHTML = `
      <div class="dialog-card dialog-card-wide">
        <div>
          <p class="eyebrow">Share</p>
          <h2>${escapeHtml(object.name || "Published object")}</h2>
          <p class="subtitle">A published content link is ready. Recipient-scoped grants are recorded by the Runtime provider when recipients are supplied.</p>
        </div>
        <div class="details">
          <div><strong>Policy</strong><br>${escapeHtml(policy)}</div>
          <div><strong>Recipients</strong><br>${escapeHtml(String(recipientCount))}</div>
          <div><strong>Grants</strong><br>${escapeHtml(String(grantCount))}</div>
          <div><strong>Content URI</strong><br>${escapeHtml(uri || identity.contentUri)}</div>
          <div><strong>SmartWeb Object</strong><br>${escapeHtml(identity.kind)}</div>
          <div><strong>Shared At</strong><br>${escapeHtml(formatTime(payload?.shared_at))}</div>
          <div><strong>Availability</strong><br>${escapeHtml(availabilitySummary.status)}</div>
          <div><strong>Live Proof</strong><br>${escapeHtml(availabilitySummary.liveProof)}</div>
          <div><strong>Storage Market</strong><br>${escapeHtml(availabilitySummary.storageMarket)}</div>
          <div><strong>Payload</strong><br>${escapeHtml(contentSecurity?.published_payload || "-")}</div>
          <div><strong>Remote Open</strong><br>${escapeHtml(remoteAccess.openStatus)}</div>
          <div><strong>Provider Chain</strong><br>${escapeHtml(protectedContent?.encrypted_recipient_sharing?.status || "-")}</div>
          <div><strong>Key Release</strong><br>${escapeHtml(remoteAccess.keyRelease || keyRelease?.status || "not required")}</div>
          <div><strong>Object</strong><br>${escapeHtml(shortUri(payload?.object_uri || object.uri || ""))}</div>
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
          <strong>Recipient Grants / Key Release</strong>
          <pre>${escapeHtml(JSON.stringify({
            policy,
            grants: payload?.grants || [],
            key_release: keyRelease,
            content_security: contentSecurity,
          }, null, 2))}</pre>
        </div>
        <div class="details-json">
          <strong>Share Receipt Summary</strong>
          <pre>${escapeHtml(JSON.stringify(safeShareReceiptSummary(payload), null, 2))}</pre>
        </div>
        <div class="button-row">
          <button class="btn" type="button" data-dialog-action="copy-share-link" data-share-uri="${escapeHtml(uri)}">Copy Link</button>
          <button class="btn" type="button" data-dialog-action="properties">Properties</button>
          <button class="btn btn-primary" type="button" data-dialog-close>Close</button>
        </div>
      </div>
    `;
    dialog.dataset.previewUri = object.uri || payload?.object_uri || "";
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
          <h2>${escapeHtml(object.name || "Shared object")}</h2>
          <p class="subtitle">Runtime checked this object against the signed Home principal. Recipient proof is injected by Runtime only when the launch grant matches the requested recipient.</p>
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
        <div class="details-json">
          <strong>Open Contract</strong>
          <pre>${escapeHtml(JSON.stringify(open, null, 2))}</pre>
        </div>
        <div class="details-json">
          <strong>Remote Access Policy</strong>
          <pre>${escapeHtml(JSON.stringify(remoteAccess.details, null, 2))}</pre>
        </div>
        <div class="details-json">
          <strong>Shared Access Receipt</strong>
          <pre>${escapeHtml(JSON.stringify(access, null, 2))}</pre>
        </div>
        <div class="button-row">
          <button class="btn" type="button" data-dialog-action="properties">Properties</button>
          <button class="btn btn-primary" type="button" data-dialog-close>Close</button>
        </div>
      </div>
    `;
    dialog.dataset.previewUri = object.uri || "";
    dialog.classList.remove("hidden");
  }

  function showShareDialog(object) {
    hideMenu();
    hideDialog();
    return new Promise((resolve) => {
      pendingDialogResolve = resolve;
      dialog.innerHTML = `
        <div class="dialog-card dialog-card-wide">
          <form data-share-form>
            <div>
              <p class="eyebrow">Share</p>
              <h2>${escapeHtml(object.name || "Published object")}</h2>
              <p class="subtitle">Choose a Runtime share policy. Public links are open to anyone with the published content URI. Recipient-scoped sharing records explicit grants and fails closed for other recipients.</p>
            </div>
            <div class="share-options" role="radiogroup" aria-label="Share policy">
              <label class="share-option">
                <input type="radio" name="sharePolicy" value="public_link" checked>
                <span>
                  <strong>Public link</strong>
                  <small>Copy one published content URI.</small>
                </span>
              </label>
              <label class="share-option">
                <input type="radio" name="sharePolicy" value="recipient_scoped">
                <span>
                  <strong>Recipient scoped</strong>
                  <small>Record grants for DIDs, principal ids, people ids, or addresses.</small>
                </span>
              </label>
              <label class="share-option">
                <input type="radio" name="sharePolicy" value="encrypted_recipient" disabled>
                <span>
                  <strong>Encrypted recipients</strong>
                  <small>Requires drm/rights/key/decrypt providers and encrypted publish mode.</small>
                </span>
              </label>
            </div>
            <label class="dialog-field">
              <span>Recipients</span>
              <textarea name="shareRecipients" rows="4" placeholder="did:..., person:..., principal:..., or name@example.com"></textarea>
            </label>
            <p class="dialog-hint">Separate recipients with commas or new lines. Recipient-scoped access requires Runtime recipient proof; encrypted key release fails closed until drm/rights/key/decrypt providers and encrypted publish mode are configured.</p>
            <p class="dialog-error hidden" data-share-error></p>
            <div class="button-row">
              <button class="btn" type="button" data-dialog-close>Cancel</button>
              <button class="btn btn-primary" type="submit">Share</button>
            </div>
          </form>
        </div>
      `;
      dialog.classList.remove("hidden");
      dialog.querySelector("textarea")?.focus();
    });
  }

  function confirmDestructive({ title, message, confirmLabel }) {
    hideMenu();
    hideDialog();
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
    pendingDialogResolve = null;
    resolve(value);
  }

  function hideDialog() {
    onBeforeClose();
    delete dialog.dataset.previewUri;
    dialog.classList.add("hidden");
    dialog.innerHTML = "";
    resolveDialogDecision(false);
  }

  function bindDialogEvents() {
    dialog.addEventListener("submit", (event) => {
      const form = event.target.closest("[data-share-form]");
      if (!form) return;
      event.preventDefault();
      const formData = new FormData(form);
      const policy = String(formData.get("sharePolicy") || "public_link");
      const recipients = String(formData.get("shareRecipients") || "")
        .split(/[\n,]+/)
        .map((recipient) => recipient.trim())
        .filter(Boolean);
      const error = form.querySelector("[data-share-error]");
      if (policy === "recipient_scoped" && !recipients.length) {
        if (error) {
          error.textContent = "Recipient-scoped sharing requires at least one recipient.";
          error.classList.remove("hidden");
        }
        return;
      }
      resolveDialogDecision({ policy, recipients });
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
        if (value && copyText) {
          copyText(value, label).catch(() => {});
          propertyCopy.classList.add("copied");
          propertyCopy.setAttribute("aria-label", `Copied ${label}`);
          setTimeout(() => {
            propertyCopy.classList.remove("copied");
            propertyCopy.removeAttribute("aria-label");
          }, 1200);
        }
        return;
      }
      const propertiesTab = event.target.closest(".item-props-tab-btn[data-tab]");
      if (propertiesTab) {
        const card = propertiesTab.closest(".properties-card");
        const tab = propertiesTab.getAttribute("data-tab");
        card?.querySelectorAll(".item-props-tab-btn").forEach((button) => {
          button.classList.toggle("item-props-tab-selected", button === propertiesTab);
        });
        card?.querySelectorAll(".item-props-tab-content").forEach((panel) => {
          panel.classList.toggle("item-props-tab-content-selected", panel.getAttribute("data-tab") === tab);
        });
        return;
      }
      if (event.target.closest('[data-dialog-action="properties"]')) {
        const object = objectByUri(dialog.dataset.previewUri);
        if (object) showProperties(object);
        return;
      }
      const copyShare = event.target.closest('[data-dialog-action="copy-share-link"]');
      if (copyShare) {
        const uri = copyShare.getAttribute("data-share-uri") || "";
        if (uri && copyText) copyText(uri, "published link").catch(() => {});
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
    showObjectStatus,
    showProperties,
    showShareDialog,
    showShareReceipt,
    showSharedAccessReceipt,
  };
}
