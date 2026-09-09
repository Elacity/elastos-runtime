// Creator — uploads a file through the Library transport, protects it, and
// lists copies at a price, in one flow.
//
// The upload transport in `providerApi`/`uploadObject*` below is copied
// verbatim from capsules/library/browser/src/api.js:26-103 (minus the
// DELETE-based cancel-on-error step: that route stays Library-only),
// and `decimalIntegerToHexQuantity` from capsules/library/browser/src/model.js:
// 177-190. Capsules are independently sandboxed with no shared module graph,
// so this is a deliberate copy, not an import.
//
// This is a faithful structural port of the dkms Creator UI (title,
// description, cover thumbnail, access-method grid, category, resale
// royalty, royalty split, AI-licensing/adult/legal checkboxes, free-preview
// settings, the encode/publish tech-progress panel, and the enable-trading
// step). Today's publish API accepts exactly
// `{ uri, if_revision, protection: { mode: "runtime_custody", copies, price } }`
// (elastos-server/src/library.rs:378-380), so every one of those fields is
// rendered and interactive but NOT sent — each carries its own "Not sent —"
// hint so the page never claims to do more than it does. The one dkms
// surface omitted outright is the wallet/channel picker: those routes do not
// exist here and there is nothing to populate them from.

const CHUNKED_UPLOAD_THRESHOLD_BYTES = 512 * 1024;
const CHUNKED_UPLOAD_BYTES = 512 * 1024;
const CHUNKED_UPLOAD_TRANSPORT = "http-chunk-session";

const MAX_UINT256 = (1n << 256n) - 1n;
const POSITIVE_DECIMAL_INTEGER_RE = /^[0-9]+$/;
const CONTROL_CHAR_RE = /[\x00-\x1f\x7f]/;
const RUNTIME_CUSTODY_PENDING_MESSAGE_RE = /pending/i;

// Extension -> MIME for types browsers report unreliably (often "" or
// octet-stream). Ported from dkms's EXT_MIME table.
const EXT_MIME = {
  epub: "application/epub+zip",
  cbz: "application/vnd.comicbook+zip",
  pdf: "application/pdf",
  svg: "image/svg+xml",
  md: "text/markdown",
  markdown: "text/markdown",
  txt: "text/plain",
  json: "application/json",
  glb: "model/gltf-binary",
  gltf: "model/gltf+json",
  stl: "model/stl",
  obj: "model/obj",
  ply: "model/mesh",
};

// The chain's actual default primary-sale split (capsules/chain-provider/src/
// main.rs:48-51: CentralStorage.protocolShares() mints the creator 950/1000
// royalty units and the protocol owner 50/1000). Cosmetic here (see
// summarizeRoyaltyRows) — no publish field carries a royalty split — but the
// default rows shown to the creator reflect this real on-chain distribution
// rather than an invented one.
const DEFAULT_CREATOR_ROYALTY_PERCENT = 95;
const DEFAULT_PROTOCOL_ROYALTY_PERCENT = 5;
const ROYALTY_TOTAL_TARGET_PERCENT = 100;

// Decimal places each listed currency uses for its smallest base unit. Only
// used client-side to scale the human-entered price into the integer this
// runtime's publish request carries — the currency itself is never sent (see
// #currency's hint).
const CURRENCY_DECIMALS = {
  ELA: 18,
  ETH: 18,
  USDC: 6,
};

// ---------------------------------------------------------------------------
// Pure helpers — exported and covered by creator.test.mjs with `node --test`.
// ---------------------------------------------------------------------------

/**
 * Build the Library object URI a chosen file uploads to. Rejects file names
 * that could escape the Creator folder: this is a security boundary, not a
 * display nicety.
 */
export function targetUriFor(rootUri, fileName) {
  const name = String(fileName ?? "");
  if (!name || name === "." || name === "..") {
    throw new Error("Choose a file with a name.");
  }
  if (name.includes("/") || name.includes("..")) {
    throw new Error("File name must not contain a path separator.");
  }
  if (CONTROL_CHAR_RE.test(name)) {
    throw new Error("File name must not contain control characters.");
  }
  const root = String(rootUri || "").replace(/\/+$/, "");
  return `${root}/Creator/${name}`;
}

/**
 * "media" for video/audio (plays in Elacity Player once protected); "object"
 * for everything else (opens in Elacity Reader once protected).
 */
export function classifyProtection(mime) {
  const value = String(mime || "");
  if (value.startsWith("video/") || value.startsWith("audio/")) {
    return "media";
  }
  return "object";
}

/** Verbatim copy of Library's model.js decimalIntegerToHexQuantity. */
export function decimalIntegerToHexQuantity(value) {
  const text = String(value ?? "").trim();
  if (!POSITIVE_DECIMAL_INTEGER_RE.test(text)) {
    throw new Error("Enter a positive whole number.");
  }
  const parsed = BigInt(text);
  if (parsed <= 0n || parsed > MAX_UINT256) {
    throw new Error("Enter a whole number from 1 to 2^256-1.");
  }
  return `0x${parsed.toString(16)}`;
}

/** Describes how a file of this size will upload, for the status copy. */
export function uploadPlan(size) {
  const bytes = Number(size) || 0;
  if (bytes <= CHUNKED_UPLOAD_THRESHOLD_BYTES) {
    return { mode: "single" };
  }
  return { mode: "chunked" };
}

/** Renders a byte count as a short human-readable size, e.g. "512.0 KB". */
export function humanSize(bytes) {
  const value = Number(bytes) || 0;
  if (value < 1024) {
    return `${value} B`;
  }
  if (value < 1024 * 1024) {
    return `${(value / 1024).toFixed(1)} KB`;
  }
  return `${(value / (1024 * 1024)).toFixed(1)} MB`;
}

/**
 * Resolve the canonical MIME type for a chosen file. Trusts the extension
 * map for types browsers report unreliably (often "" or octet-stream);
 * otherwise falls back to the browser-reported type, then octet-stream.
 * Ported from dkms's resolveMime/EXT_MIME.
 */
export function resolveMime(file) {
  const name = String(file?.name || "");
  const dot = name.lastIndexOf(".");
  const ext = dot >= 0 ? name.slice(dot + 1).toLowerCase() : "";
  if (EXT_MIME[ext]) {
    return EXT_MIME[ext];
  }
  return file?.type || "application/octet-stream";
}

/**
 * Sums royalty-row percentages and checks whether they total exactly 100%
 * (the default rows already do: 95% creator + 5% protocol). `overLimit`
 * distinguishes "still under 100%" from "already over" so the UI can block
 * an edit that would push the total past 100 with a clear reason, rather
 * than silently clamping it. Cosmetic today: no field in this runtime's
 * publish request carries a royalty split, so this never leaves the browser.
 */
export function summarizeRoyaltyRows(rows) {
  const sum = (rows || []).reduce((total, row) => total + (Number(row?.royalty) || 0), 0);
  const ok = Math.abs(sum - ROYALTY_TOTAL_TARGET_PERCENT) < 0.01;
  const overLimit = sum > ROYALTY_TOTAL_TARGET_PERCENT + 0.01;
  return { sum, target: ROYALTY_TOTAL_TARGET_PERCENT, ok, overLimit };
}

/**
 * Scales a human-entered decimal amount (e.g. "0.001") into the integer
 * base-units string for the given currency, using exact string/BigInt
 * arithmetic — never Number/parseFloat, which loses precision well before
 * 18 decimal places. Splits on ".", pads (or rejects) the fractional part
 * against the currency's decimal count, and returns a plain digit string
 * ready for decimalIntegerToHexQuantity. Throws with a clear message on
 * invalid input, zero, too many fractional digits, or an unknown currency.
 */
export function scalePriceToBaseUnits(amount, currency) {
  const decimals = CURRENCY_DECIMALS[currency];
  if (decimals === undefined) {
    throw new Error(`Unknown currency: ${currency}`);
  }
  const text = String(amount ?? "").trim();
  if (!/^[0-9]+(\.[0-9]+)?$/.test(text)) {
    throw new Error("Enter a positive amount, e.g. 0.001.");
  }
  const [wholePart, fractionalPart = ""] = text.split(".");
  if (fractionalPart.length > decimals) {
    throw new Error(`${currency} supports at most ${decimals} decimal place${decimals === 1 ? "" : "s"}.`);
  }
  const combinedDigits = `${wholePart}${fractionalPart.padEnd(decimals, "0")}`;
  const scaled = combinedDigits.replace(/^0+/, "") || "0";
  if (scaled === "0") {
    throw new Error("Enter an amount greater than zero.");
  }
  return scaled;
}

/**
 * Builds the exact request body `publishOnce` sends to
 * `/api/provider/object/publish`. This is the one place the publish payload
 * is assembled, kept deliberately narrow and pure so a test can assert its
 * key set never grows: today's server (`LibraryPublishProtectionRequest::
 * RuntimeCustody`) accepts only `copies` and `price`, and rejects unknown
 * fields outright — so if a future edit reads title/description/thumbnail/
 * category/reseller-cut/royalties/checkbox state into this object, publishing
 * breaks immediately and loudly instead of silently leaking form state.
 */
export function buildPublishBody({ uri, ifRevision, copies, price }) {
  return {
    uri,
    if_revision: ifRevision,
    protection: {
      mode: "runtime_custody",
      copies,
      price,
    },
  };
}

/**
 * A runtime-custody publish that has not yet settled Wallet/Chain approval
 * is a non-terminal waiting state, not a failure: the creator approves the
 * transaction in Wallet, then retries the same publish.
 */
export function isRuntimeCustodyPendingMessage(message) {
  return RUNTIME_CUSTODY_PENDING_MESSAGE_RE.test(String(message || ""));
}

// ---------------------------------------------------------------------------
// DOM flow — skipped under `node --test` (no `document`/`window` there).
// ---------------------------------------------------------------------------

function bootCreatorApp() {
  const query = new URLSearchParams(window.location.search);
  const homeToken = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";
  const homeParentOrigin = query.get("home_origin") || "";

  const els = {
    lockedShell: document.querySelector("#locked-shell"),
    shell: document.querySelector("#creator-shell"),

    drop: document.querySelector("#drop"),
    dropTitle: document.querySelector("#drop-title"),
    dropMetaText: document.querySelector("#drop-meta-text"),
    dropBadge: document.querySelector("#drop-badge"),
    fileInput: document.querySelector("#file-input"),

    title: document.querySelector("#title"),
    desc: document.querySelector("#desc"),

    thumbDrop: document.querySelector("#thumb-drop"),
    thumbInput: document.querySelector("#thumb-input"),
    thumbPreviewWrap: document.querySelector("#thumb-preview-wrap"),
    thumbPreviewImg: document.querySelector("#thumb-preview-img"),
    thumbRemove: document.querySelector("#thumb-remove"),

    methodGrid: document.querySelector("#method-grid"),
    methodHint: document.querySelector("#method-hint"),
    priceRow: document.querySelector("#price-row"),
    priceInput: document.querySelector("#price-input"),
    currency: document.querySelector("#currency"),

    copiesInput: document.querySelector("#copies-input"),
    category: document.querySelector("#category"),

    resellField: document.querySelector("#resell-field"),
    resellerCut: document.querySelector("#reseller-cut"),

    royaltyField: document.querySelector("#royalty-field"),
    royaltyRows: document.querySelector("#royalty-rows"),
    royaltyAdd: document.querySelector("#royalty-add"),
    royaltyTotal: document.querySelector("#royalty-total"),

    aiLicensing: document.querySelector("#ai-licensing"),
    adultFlag: document.querySelector("#adult-flag"),
    legalAttest: document.querySelector("#legal-attest"),

    previewSettings: document.querySelector("#preview-settings"),
    previewEnabled: document.querySelector("#preview-enabled"),
    previewControls: document.querySelector("#preview-controls"),
    previewDuration: document.querySelector("#preview-duration"),
    previewDurationDisplay: document.querySelector("#preview-duration-display"),

    submitButton: document.querySelector("#submit-button"),
    progressSteps: document.querySelector("#progress-steps"),
    statusText: document.querySelector("#status-text"),
    // #enable-trading-button is intentionally not cached here: it stays
    // permanently `disabled` in the markup (see its own hint) with no JS
    // behavior — this runtime has no separate trading-approval step, so
    // there is nothing for it to do.

    successPanel: document.querySelector("#success-panel"),
    mintIdText: document.querySelector("#mint-id-text"),
    openLibraryButton: document.querySelector("#open-library-button"),
    resetButton: document.querySelector("#reset-button"),
  };

  let selectedFile = null;
  let customThumbnail = null;
  // Only "buy_once" (Buy now) is connected to a real publish request; "free"
  // and "buy_and_resell" are rendered (dkms parity) but block submission —
  // see refreshSubmitEnabled and syncMethodUI's #method-hint copy.
  let accessMethod = "buy_once";
  let submitting = false;
  let listed = false;
  let homeChromeReady = false;
  let activeStageName = "";

  boot();

  function boot() {
    if (!homeToken) {
      els.lockedShell.classList.remove("hidden");
      return;
    }
    els.shell.classList.remove("hidden");
    bindEvents();
    syncMethodUI();
    seedDefaultRoyaltyRows();
    announceHomeChrome();
  }

  function announceHomeChrome() {
    if (homeChromeReady || !homeParentOrigin || window.top === window) {
      return;
    }
    window.top.postMessage({ type: "home:app-ready", homeToken }, homeParentOrigin);
    homeChromeReady = true;
  }

  function bindEvents() {
    els.drop.addEventListener("click", () => els.fileInput.click());
    els.drop.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        els.fileInput.click();
      }
    });
    els.drop.addEventListener("dragenter", (event) => {
      event.preventDefault();
      els.drop.classList.add("over");
    });
    els.drop.addEventListener("dragover", (event) => event.preventDefault());
    els.drop.addEventListener("dragleave", () => els.drop.classList.remove("over"));
    els.drop.addEventListener("drop", (event) => {
      event.preventDefault();
      els.drop.classList.remove("over");
      const file = event.dataTransfer?.files?.[0];
      if (file) onFile(file);
    });
    els.fileInput.addEventListener("change", () => {
      const file = els.fileInput.files?.[0];
      if (file) onFile(file);
    });

    els.thumbDrop.addEventListener("click", () => els.thumbInput.click());
    els.thumbDrop.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        els.thumbInput.click();
      }
    });
    els.thumbDrop.addEventListener("dragenter", (event) => {
      event.preventDefault();
      els.thumbDrop.classList.add("over");
    });
    els.thumbDrop.addEventListener("dragover", (event) => event.preventDefault());
    els.thumbDrop.addEventListener("dragleave", () => els.thumbDrop.classList.remove("over"));
    els.thumbDrop.addEventListener("drop", (event) => {
      event.preventDefault();
      els.thumbDrop.classList.remove("over");
      const file = event.dataTransfer?.files?.[0];
      if (file) setCustomThumbnail(file);
    });
    els.thumbInput.addEventListener("change", () => setCustomThumbnail(els.thumbInput.files?.[0]));
    els.thumbRemove.addEventListener("click", clearCustomThumbnail);

    els.methodGrid.querySelectorAll(".method").forEach((card) => {
      const pick = () => {
        accessMethod = card.dataset.method;
        syncMethodUI();
      };
      card.addEventListener("click", pick);
      card.addEventListener("keydown", (event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          pick();
        }
      });
    });

    els.royaltyAdd.addEventListener("click", () => addRoyaltyRow("", ""));

    els.previewEnabled.addEventListener("change", () => {
      els.previewControls.classList.toggle("hidden", !els.previewEnabled.checked);
    });
    els.previewDuration.addEventListener("input", () => {
      els.previewDurationDisplay.textContent = `${els.previewDuration.value}s`;
    });

    els.copiesInput.addEventListener("input", refreshSubmitEnabled);
    els.priceInput.addEventListener("input", refreshSubmitEnabled);
    // A currency change alone can invalidate an already-typed amount (e.g.
    // more fractional digits than USDC's 6 decimals allow), so re-check.
    els.currency.addEventListener("change", refreshSubmitEnabled);
    els.submitButton.addEventListener("click", () => {
      protectAndList().catch((error) => {
        if (error?.pending) {
          showPending();
        } else {
          showFailure(error);
        }
      });
    });
    els.openLibraryButton.addEventListener("click", openLibrary);
    els.resetButton.addEventListener("click", resetForAnotherFile);
  }

  function onFile(file) {
    if (!file || submitting) return;
    selectedFile = file;
    listed = false;
    const mime = resolveMime(file);
    const kind = classifyProtection(mime);
    const opensIn = kind === "media" ? "plays in Elacity Player" : "opens in Elacity Reader";
    els.dropTitle.textContent = file.name;
    els.dropMetaText.textContent = `${humanSize(file.size)} · ${mime} · ${opensIn}`;
    els.dropBadge.textContent = kind;
    els.dropBadge.classList.remove("hidden");
    if (!els.title.value.trim()) {
      els.title.value = file.name.replace(/\.[^.]+$/, "");
    }
    const isMedia = kind === "media";
    els.previewSettings.classList.toggle("hidden", !isMedia);
    els.previewEnabled.checked = isMedia;
    els.previewControls.classList.toggle("hidden", !isMedia);
    // "Transcode & fragment" only applies to media; see the progress-stage
    // mapping notes by setStage/resetStages for what this tracker can and
    // cannot actually observe.
    const transcodeStage = els.progressSteps.querySelector('li[data-stage="transcode"]');
    if (transcodeStage) transcodeStage.classList.toggle("hidden", !isMedia);
    resetStages();
    setStatus("");
    els.successPanel.classList.add("hidden");
    refreshSubmitEnabled();
  }

  // -- Cover thumbnail: local preview only (see #thumb-drop's hint) --------

  function setCustomThumbnail(file) {
    if (!file || !file.type.startsWith("image/")) return;
    customThumbnail = file;
    els.thumbPreviewImg.src = URL.createObjectURL(file);
    els.thumbPreviewWrap.classList.remove("hidden");
    els.thumbDrop.classList.add("hidden");
  }

  function clearCustomThumbnail() {
    customThumbnail = null;
    els.thumbInput.value = "";
    els.thumbPreviewWrap.classList.add("hidden");
    els.thumbDrop.classList.remove("hidden");
  }

  // -- Access method (only "buy_once" is wired) -----------------------------

  function isPaidMethod() {
    return accessMethod === "buy_once" || accessMethod === "buy_and_resell";
  }

  function syncMethodUI() {
    els.methodGrid.querySelectorAll(".method").forEach((card) => {
      const selected = card.dataset.method === accessMethod;
      card.classList.toggle("sel", selected);
      card.setAttribute("aria-pressed", String(selected));
    });
    els.priceRow.classList.toggle("hidden", !isPaidMethod());
    els.resellField.classList.toggle("hidden", accessMethod !== "buy_and_resell");
    els.royaltyField.classList.toggle("hidden", !isPaidMethod());
    els.methodHint.textContent = accessMethod === "buy_once"
      ? "Buy now is the only method connected in this build."
      : "Select Buy now to continue — this method isn't connected yet.";
    refreshSubmitEnabled();
  }

  // -- Royalty split: cosmetic, never sent (see #royalty-field's hint) -----
  //
  // Seeded with the chain's real default distribution (95% creator / 5%
  // protocol — see DEFAULT_CREATOR_ROYALTY_PERCENT above) so "+ Add payee"
  // starts from a running total that already means something, instead of an
  // empty, confusing control. The total may never exceed 100%: an edit that
  // would push it over is reverted (enforceRoyaltyCap), with a visible reason
  // rather than a silent clamp. There is no wallet integration to resolve the
  // creator's own address client-side, so that row is honestly labeled "You"
  // rather than carrying an invented 0x address.

  function addRoyaltyRow(address, percent) {
    const row = document.createElement("div");
    row.className = "royalty-row";
    const addr = document.createElement("input");
    addr.type = "text";
    addr.className = "ry-addr";
    addr.placeholder = "0x… payee address";
    addr.value = address || "";
    const pct = document.createElement("input");
    pct.type = "number";
    pct.className = "ry-pct";
    pct.min = "0";
    pct.max = String(ROYALTY_TOTAL_TARGET_PERCENT);
    pct.step = "0.1";
    pct.placeholder = "%";
    const initialPercent = percent != null && percent !== "" ? String(percent) : "";
    pct.value = initialPercent;
    pct.dataset.lastValue = initialPercent || "0";
    const del = document.createElement("button");
    del.type = "button";
    del.className = "ry-del";
    del.textContent = "×";
    del.title = "Remove payee";
    del.addEventListener("click", () => {
      row.remove();
      refreshRoyaltyTotal();
    });
    addr.addEventListener("input", refreshRoyaltyTotal);
    pct.addEventListener("input", refreshRoyaltyTotal);
    pct.addEventListener("change", () => enforceRoyaltyCap(pct));
    row.append(addr, pct, del);
    els.royaltyRows.appendChild(row);
    refreshRoyaltyTotal();
  }

  function seedDefaultRoyaltyRows() {
    els.royaltyRows.innerHTML = "";
    addRoyaltyRow("You", DEFAULT_CREATOR_ROYALTY_PERCENT);
    addRoyaltyRow("Elacity", DEFAULT_PROTOCOL_ROYALTY_PERCENT);
  }

  function collectRoyaltyRows() {
    return Array.from(els.royaltyRows.querySelectorAll(".royalty-row"))
      .map((row) => ({
        address: row.querySelector(".ry-addr").value.trim(),
        royalty: Number.parseFloat(row.querySelector(".ry-pct").value) || 0,
      }))
      .filter((row) => row.address || row.royalty);
  }

  // Reverts a percent field to its last accepted value if the committed edit
  // (on blur/enter, not every keystroke — so typing isn't interrupted) would
  // push the group total over 100%, and says so plainly rather than clamping
  // the number to whatever happened to fit.
  function enforceRoyaltyCap(pctInput) {
    const { overLimit } = summarizeRoyaltyRows(collectRoyaltyRows());
    if (!overLimit) {
      pctInput.dataset.lastValue = pctInput.value || "0";
      // Always re-render on commit, not just on rejection: a `change` event
      // can fire without a preceding `input` (e.g. a scripted value set), and
      // without this an earlier rejection message would linger stale.
      refreshRoyaltyTotal();
      return;
    }
    pctInput.value = pctInput.dataset.lastValue || "0";
    refreshRoyaltyTotal();
    els.royaltyTotal.textContent = "That amount would push the total over 100% — change reverted.";
    els.royaltyTotal.className = "royalty-total bad";
  }

  function refreshRoyaltyTotal() {
    const rows = collectRoyaltyRows();
    if (!rows.length) {
      els.royaltyTotal.textContent = "";
      els.royaltyTotal.className = "royalty-total";
      return;
    }
    const { sum, ok, overLimit } = summarizeRoyaltyRows(rows);
    let text;
    let variant = "";
    if (overLimit) {
      text = `Total royalty is ${sum.toFixed(1)}% — must not exceed 100%.`;
      variant = "bad";
    } else if (ok) {
      text = "Total royalty is 100% ✓";
      variant = "ok";
    } else {
      text = `Total royalty is ${sum.toFixed(1)}% of 100%.`;
    }
    els.royaltyTotal.textContent = text;
    els.royaltyTotal.className = `royalty-total ${variant}`.trim();
  }

  // -- Submission gate -------------------------------------------------------

  function validCopiesAndPrice() {
    try {
      const copies = decimalIntegerToHexQuantity(els.copiesInput.value);
      const baseUnits = scalePriceToBaseUnits(els.priceInput.value, els.currency.value);
      const price = decimalIntegerToHexQuantity(baseUnits);
      return { copies, price };
    } catch {
      return null;
    }
  }

  function refreshSubmitEnabled() {
    const methodWired = accessMethod === "buy_once";
    els.submitButton.disabled = submitting || !selectedFile || listed || !methodWired || !validCopiesAndPrice();
  }

  // -- Progress tracker -------------------------------------------------------
  //
  // One tracker, sequenced by content kind (see onFile's transcode toggle):
  //   media:     Analyze source -> Transcode & fragment -> Encrypt & escrow
  //              -> Publish to storage -> Assemble listing
  //   non-media: the same minus Transcode & fragment
  //
  // Only three moments are actually observable from this client: the upload
  // finishing, the protect-and-list request being in flight, and the listing
  // coming back. Those three drive exactly three of the five named stages —
  // "analyze" (upload done), "encrypt" (request sent), and "assemble"
  // (response returned). "transcode" and "publish" have no client-observable
  // signal at all (there is no per-stage progress feed from the server), so
  // setStage is never called for them: their dots stay in the default,
  // never-active, never-done resting state for the whole flow, and their
  // labels in the markup carry a "(not tracked)" note rather than pretending
  // to be live.

  function setStage(name, state) {
    if (state === "active") activeStageName = name;
    const li = els.progressSteps.querySelector(`li[data-stage="${name}"]`);
    if (li) li.className = state || "";
  }

  function resetStages() {
    activeStageName = "";
    ["analyze", "encrypt", "assemble"].forEach((stage) => setStage(stage, ""));
  }

  function setStatus(text, kind) {
    els.statusText.textContent = text || "";
    els.statusText.className = `status${kind ? ` ${kind}` : ""}`;
  }

  async function protectAndList() {
    if (submitting || !selectedFile) return;
    const terms = validCopiesAndPrice();
    if (!terms) return;

    submitting = true;
    refreshSubmitEnabled();
    setStatus("");

    try {
      // "Analyze source" covers everything up to and including the upload:
      // this client did inspect the file (resolveMime/classifyProtection)
      // and transport it to Library storage.
      setStage("analyze", "active");
      const roots = await providerApi("roots", {});
      const homeRoot = (roots.roots || []).find((root) => root.id === "home");
      if (!homeRoot) {
        throw new Error("Could not find your Library folder.");
      }
      const targetUri = targetUriFor(homeRoot.uri, selectedFile.name);
      const mime = resolveMime(selectedFile);
      const plan = uploadPlan(selectedFile.size);
      setStatus(
        plan.mode === "single"
          ? `Uploading ${selectedFile.name}...`
          : `Uploading ${selectedFile.name} in parts...`,
      );
      await uploadObject({ uri: targetUri, file: selectedFile, mime });
      setStage("analyze", "done");

      // "Encrypt & escrow" goes active for the whole protect-and-list
      // request — encryption, storage publish, and listing assembly all
      // happen server-side inside this one round trip, so there is no
      // client-observable moment that separates them. On success, "assemble"
      // (the last named stage) is marked done because that is exactly the
      // "listing returned" signal; "publish" in between stays untouched (see
      // its "(not tracked)" label) rather than guessing at its timing.
      setStage("encrypt", "active");
      setStatus("Protecting and listing...");
      const published = await publishWithRevisionRetry(targetUri, terms);
      setStage("encrypt", "done");
      setStage("assemble", "done");

      const mintId = published?.content_security?.mint_id || "";
      showSuccess(mintId);
    } finally {
      submitting = false;
      refreshSubmitEnabled();
    }
  }

  async function publishWithRevisionRetry(targetUri, terms) {
    const stat = await providerApi("stat", { uri: targetUri });
    try {
      return await publishOnce(targetUri, stat.object?.revision, terms);
    } catch (error) {
      if (!/revision/i.test(String(error?.message || ""))) {
        throw error;
      }
      const freshStat = await providerApi("stat", { uri: targetUri });
      return await publishOnce(targetUri, freshStat.object?.revision, terms);
    }
  }

  async function publishOnce(targetUri, revision, terms) {
    const response = await providerApiEnvelope(
      "publish",
      buildPublishBody({ uri: targetUri, ifRevision: revision, copies: terms.copies, price: terms.price }),
    );
    if (response.status === "error") {
      const message = response.message || "Protecting this file failed.";
      const error = new Error(message);
      if (isRuntimeCustodyPendingMessage(message)) {
        error.pending = true;
      }
      throw error;
    }
    return response.data || {};
  }

  function showSuccess(mintId) {
    listed = true;
    setStatus("Listed for sale.", "ok");
    els.mintIdText.textContent = mintId ? `Listing ${mintId}` : "";
    els.successPanel.classList.remove("hidden");
    refreshSubmitEnabled();
  }

  // A pending publish is waiting on the creator's approval in Wallet, not a
  // failure: the Protect step stays in its in-progress state (set by
  // protectAndList before the publish call) and the file, copies, and price
  // stay in place so clicking "Protect and list" again resumes the listing.
  function showPending() {
    setStatus("Approve this transaction in Wallet, then click Protect and list again.");
  }

  function showFailure(error) {
    if (activeStageName) setStage(activeStageName, "err");
    setStatus(String(error?.message || error || "Protecting this file failed."), "err");
  }

  function resetForAnotherFile() {
    selectedFile = null;
    listed = false;
    els.fileInput.value = "";
    els.dropTitle.textContent = "Choose a file";
    els.dropMetaText.textContent = "Any file · click or drop";
    els.dropBadge.textContent = "";
    els.dropBadge.classList.add("hidden");
    els.drop.classList.remove("over");

    els.title.value = "";
    els.desc.value = "";
    clearCustomThumbnail();
    els.category.value = "";

    accessMethod = "buy_once";
    syncMethodUI();

    els.copiesInput.value = "";
    els.priceInput.value = "";
    els.currency.value = "ELA";
    els.resellerCut.value = "90";
    seedDefaultRoyaltyRows();

    els.aiLicensing.checked = false;
    els.adultFlag.checked = false;
    els.legalAttest.checked = false;

    els.previewSettings.classList.add("hidden");
    els.previewControls.classList.add("hidden");
    els.previewEnabled.checked = false;
    els.previewDuration.value = "15";
    els.previewDurationDisplay.textContent = "15s";

    const transcodeStage = els.progressSteps.querySelector('li[data-stage="transcode"]');
    if (transcodeStage) transcodeStage.classList.add("hidden");

    els.successPanel.classList.add("hidden");
    resetStages();
    setStatus("");
    refreshSubmitEnabled();
  }

  function openLibrary() {
    if (homeParentOrigin && window.top && window.top !== window) {
      window.top.postMessage({ type: "home:open-target", target: "library", homeToken }, homeParentOrigin);
    }
  }

  // -- Library upload transport, copied verbatim (see file header) --------

  async function providerApiEnvelope(op, payload) {
    const response = await fetch(`/api/provider/object/${encodeURIComponent(op)}`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-elastos-home-token": homeToken,
      },
      body: JSON.stringify(payload || {}),
    });
    const envelope = await response.json().catch(() => ({}));
    if (!response.ok && envelope.status !== "error") {
      throw new Error(envelope.message || envelope.error || `Creator request failed: ${response.status}`);
    }
    return envelope;
  }

  async function providerApi(op, payload) {
    const envelope = await providerApiEnvelope(op, payload);
    if (envelope.status === "error") {
      throw new Error(envelope.message || "Runtime object provider failed.");
    }
    return envelope.data || envelope;
  }

  function uploadObject({ uri, file, mime }) {
    if (file?.size > CHUNKED_UPLOAD_THRESHOLD_BYTES) {
      return uploadObjectChunked({ uri, file, mime });
    }
    return uploadObjectRaw({ uri, file, mime });
  }

  function uploadObjectRaw({ uri, file, mime }) {
    return new Promise((resolve, reject) => {
      const url = new URL("/api/provider/object/upload", window.location.origin);
      url.searchParams.set("uri", uri);
      const xhr = new XMLHttpRequest();
      xhr.open("PUT", url.pathname + url.search);
      xhr.setRequestHeader("x-elastos-home-token", homeToken);
      xhr.setRequestHeader("content-type", mime || file?.type || "application/octet-stream");
      xhr.onerror = () => reject(new Error("Upload failed before Runtime accepted the object."));
      xhr.onload = () => {
        let envelope = {};
        try {
          envelope = JSON.parse(xhr.responseText || "{}");
        } catch {
          envelope = { message: xhr.responseText || "" };
        }
        if (xhr.status < 200 || xhr.status >= 300) {
          reject(new Error(uploadFailureMessage(xhr, envelope)));
          return;
        }
        if (envelope.status === "error") {
          reject(new Error(envelope.message || "Runtime object provider upload failed."));
          return;
        }
        resolve(envelope.data || envelope);
      };
      xhr.send(file);
    });
  }

  async function uploadObjectChunked({ uri, file, mime }) {
    const contentType = mime || file?.type || "application/octet-stream";
    const start = await uploadJsonRequest("/api/provider/object/upload/start", {
      uri,
      mime: contentType,
      size_bytes: file.size,
      transport: CHUNKED_UPLOAD_TRANSPORT,
    });
    const uploadId = start.upload_id;
    try {
      let offset = 0;
      while (offset < file.size) {
        const end = Math.min(offset + CHUNKED_UPLOAD_BYTES, file.size);
        const chunk = file.slice(offset, end, contentType);
        await uploadChunkRequest({ uploadId, offset, chunk, mime: contentType });
        offset = end;
      }
      return await uploadJsonRequest(`/api/provider/object/upload/${encodeURIComponent(uploadId)}/finish`, null);
    } catch (error) {
      // Library's copied transport cancels the upload session here, but the
      // DELETE upload route stays Library-only (Creator is not in its
      // allowlist, by design). A cancel call from Creator would only ever
      // 403, so Creator relies on Runtime's stale-session expiry instead.
      throw error;
    }
  }

  async function uploadJsonRequest(path, payload) {
    const response = await fetch(path, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-elastos-home-token": homeToken,
      },
      body: payload ? JSON.stringify(payload) : undefined,
    });
    const text = await response.text();
    const envelope = parseEnvelope(text);
    if (!response.ok || envelope.status === "error") {
      throw new Error(uploadFailureMessage(responseLike(response, text), envelope));
    }
    return envelope.data || envelope;
  }

  function uploadChunkRequest({ uploadId, offset, chunk, mime }) {
    return new Promise((resolve, reject) => {
      const xhr = new XMLHttpRequest();
      xhr.open("PUT", `/api/provider/object/upload/${encodeURIComponent(uploadId)}/chunk`);
      xhr.setRequestHeader("x-elastos-home-token", homeToken);
      xhr.setRequestHeader("x-elastos-upload-offset", String(offset));
      xhr.setRequestHeader("content-type", mime || "application/octet-stream");
      xhr.onerror = () => reject(new Error("Upload chunk failed before Runtime accepted the object."));
      xhr.onload = () => {
        const envelope = parseEnvelope(xhr.responseText || "");
        if (xhr.status < 200 || xhr.status >= 300 || envelope.status === "error") {
          reject(new Error(uploadFailureMessage(xhr, envelope)));
          return;
        }
        resolve(envelope.data || envelope);
      };
      xhr.send(chunk);
    });
  }

  function parseEnvelope(text) {
    try {
      return JSON.parse(text || "{}");
    } catch {
      return { message: text || "" };
    }
  }

  function responseLike(response, text) {
    return { status: response.status, responseText: text };
  }

  function uploadFailureMessage(xhr, envelope) {
    const body = String(envelope.message || envelope.error || xhr.responseText || "");
    if (xhr.status === 413 || /request entity too large|nginx/i.test(body)) {
      return "This file is too large for the current upload service.";
    }
    return body || `Upload failed: ${xhr.status}`;
  }
}

if (typeof window !== "undefined" && typeof document !== "undefined") {
  bootCreatorApp();
}
