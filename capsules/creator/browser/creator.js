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
// settings, and the encode/publish tech-progress panel). dkms's separate
// enable-trading step is deliberately NOT ported into the MINT flow: the
// contract fulfils the mint without an ERC-1155 operator approval, so a mint
// raises exactly one wallet effect and there is no second step to offer while
// listing. An operator approval is still how secondary trading works -- an
// owner selling some of the Access Tokens or Royalty Shares they hold must
// authorize the operator that moves them -- so a resale surface will need its
// own equivalent of this step. It just does not belong here.
//
// The publish request carries `protection.listing` with the title,
// description, cover, category and the adult/licensing/legal flags, which
// become the Elacity metadata folder the mint's token URI resolves to. Four
// fields are still rendered without being sent — currency (the price is
// scaled locally; the chain mints against the native token), and resale
// royalty and free-preview, neither supported yet. Each keeps a hint saying
// so, and no field claims more than it does. The royalty split IS sent, but
// only the creator's own rows and only when they name real addresses: see
// collectRoyaltyUnits. The one dkms surface omitted outright is the
// wallet/channel picker: those routes do not exist here.

// How a pending publish is resumed. The person budget is the generous one:
// approving in a connector means leaving this page, finding the wallet and
// coming back. The chain budget is short because nothing is being asked of
// anyone — if Base has not produced evidence in two minutes, something is
// wrong and saying so beats spinning.
const PENDING_POLL_INTERVAL_MS = 3000;
const PENDING_PERSON_BUDGET_MS = 5 * 60 * 1000;
const PENDING_CHAIN_BUDGET_MS = 2 * 60 * 1000;

const CHUNKED_UPLOAD_THRESHOLD_BYTES = 512 * 1024;
const CHUNKED_UPLOAD_BYTES = 512 * 1024;
const CHUNKED_UPLOAD_TRANSPORT = "http-chunk-session";

const MAX_UINT256 = (1n << 256n) - 1n;
const POSITIVE_DECIMAL_INTEGER_RE = /^[0-9]+$/;
const CONTROL_CHAR_RE = /[\x00-\x1f\x7f]/;

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

// The real default primary-sale split applied when an asset is listed: the
// creator receives 950 of 1000 royalty units and the protocol owner 50. The
// rows show both so the running total means something; only the creator's own
// rows are ever sent, and only when they name real addresses.
const DEFAULT_CREATOR_ROYALTY_PERCENT = 95;
const DEFAULT_PROTOCOL_ROYALTY_PERCENT = 5;
const ROYALTY_TOTAL_TARGET_PERCENT = 100;

// The creator's whole share in ERC-1155 ROYALTY_SHARE units: 950 of the 1000
// that exist per asset, the protocol's 50 being minted by the contracts. One
// unit is 0.1% of the sale.
const CREATOR_ROYALTY_UNITS = 950;
const EVM_ADDRESS_RE = /^0x[0-9a-f]{40}$/;

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

/**
 * Reads the server's "this request settled nothing" marker off a publish
 * answer's `content_security`.
 *
 * The server sets `settled_before_this_request` only when the mint was already
 * terminal on arrival: no effect raised, no transaction sent, just the listing
 * re-published from the existing record. Returns null for a fresh mint — and
 * for every server that predates the marker, which is why absence has to mean
 * "fresh" rather than "unknown".
 */
export function settledFrom(contentSecurity) {
  if (!contentSecurity || contentSecurity.settled_before_this_request !== true) {
    return null;
  }
  return {
    before: true,
    at: contentSecurity.settled_at,
    transactionHash: contentSecurity.transaction_hash || "",
    sellerAddress: contentSecurity.settled_seller_address || "",
  };
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
export function buildPublishBody({ uri, ifRevision, copies, price, listing }) {
  const protection = {
    mode: "runtime_custody",
    copies,
    price,
  };
  // Omitted rather than sent empty: absent means "no Elacity listing folder",
  // which is a different thing from a listing whose every field is blank.
  if (listing) {
    protection.listing = listing;
  }
  return {
    uri,
    if_revision: ifRevision,
    protection,
  };
}

/**
 * Maps the server's journal-derived publish progress onto this page's stage
 * list.
 *
 * The server names the phases it owns; this page's list also carries
 * "analyze", which is the client's own upload and which no server record
 * describes. Keeping the two vocabularies separate and mapping between them
 * is deliberate: the alternative is the page inventing server-side stage names
 * and drifting from the journal the moment either side changes.
 *
 * Unknown ids are ignored rather than guessed at, so a server that grows a
 * phase this page does not render degrades to showing one stage fewer instead
 * of throwing.
 */
export function stagesFromProgress(progress) {
  const stages = progress && Array.isArray(progress.stages) ? progress.stages : [];
  const byServerId = { escrow: "encrypt", publish: "publish", listing: "assemble" };
  const mapped = [];
  for (const stage of stages) {
    const name = byServerId[stage && stage.id];
    if (!name) continue;
    mapped.push({ name, state: String((stage && stage.state) || "") });
  }
  return mapped;
}

/**
 * Reads the server's typed waiting state off an error envelope.
 *
 * A runtime-custody publish that has not settled Wallet/Chain approval is a
 * non-terminal waiting state, not a failure. This used to be detected by
 * testing the message against /pending/i — control flow decided by matching an
 * English word in a sentence written for a person, which no test pinned and
 * which any rewording would have broken silently.
 *
 * `awaitsPerson` is the part that matters: only an external wallet with an
 * outstanding approval needs the creator to do anything. A managed approval and
 * a chain wait both resolve on their own, and telling someone to go and approve
 * something they already approved is how the old shared message misled.
 *
 * Returns null when the envelope carries no waiting state, which includes every
 * server that predates the typed answer — so absence means "not pending"
 * rather than "unknown".
 */
export function effectPendingFrom(envelope) {
  const pending = envelope && envelope.effect_pending;
  if (!pending || typeof pending !== "object") {
    return null;
  }
  return {
    reason: String(pending.reason || ""),
    awaitsPerson: pending.awaits_person === true,
    connectorId: pending.connector_id ? String(pending.connector_id) : "",
  };
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

    successPanel: document.querySelector("#success-panel"),
    successTitle: document.querySelector("#success-title"),
    successDetail: document.querySelector("#success-detail"),
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
          showPending(error.pending);
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

  // The listing a marketplace displays, read from the form.
  //
  // The cover is the only field that is bytes rather than a statement, so it is
  // the only one that has to be read asynchronously; everything else is a
  // value already on the page. Royalties are deliberately absent — see the
  // royalty hint: the rows describe all 1000 units (95 to the creator, 5 to
  // the protocol) while the publish contract takes the creator's 950 alone, so
  // sending them as entered would describe a payout the chain will not make.
  async function collectListing() {
    const listing = {
      title: els.title.value.trim(),
      description: els.desc.value.trim(),
      category: els.category.value.trim(),
      tags: [],
      adult: els.adultFlag.checked,
      licensing: { ai_training: els.aiLicensing.checked },
      legal_attestation: { owns_distribution_rights: els.legalAttest.checked },
    };
    if (customThumbnail) {
      listing.thumbnail = {
        mime: customThumbnail.type,
        bytes_base64: await fileToBase64(customThumbnail),
      };
    }
    const royalties = collectRoyaltyUnits();
    if (royalties) {
      listing.royalties = royalties;
    }
    return listing;
  }

  /**
   * The creator's own royalty split, as ERC-1155 ROYALTY_SHARE units.
   *
   * The rows show all 1000 units — 950 the creator's, 50 the protocol's, which
   * the contracts mint themselves — so the protocol row is excluded and what
   * remains must come to the creator's 950. One unit is 0.1%, so a row's
   * percent times ten is its units, and no other conversion happens anywhere.
   *
   * The protocol row is excluded and fixed, and the total is capped at 100%,
   * so whenever the rows do add up the rest come to exactly 950.
   *
   * Returns null, meaning "say nothing and let the chain default apply", when
   * the rows are still the seeded default: "You" is a label rather than an
   * address, and the chain default already pays the creator's whole share to
   * the creator. Only a split naming real addresses is worth sending.
   */
  function collectRoyaltyUnits() {
    const rows = Array.from(els.royaltyRows.querySelectorAll(".royalty-row"))
      .filter((row) => row.dataset.protocol !== "true")
      .map((row) => ({
        address: row.querySelector(".ry-addr").value.trim().toLowerCase(),
        units: Math.round((Number.parseFloat(row.querySelector(".ry-pct").value) || 0) * 10),
      }));
    if (!rows.length || !rows.every((row) => EVM_ADDRESS_RE.test(row.address))) {
      return null;
    }
    const total = rows.reduce((sum, row) => sum + row.units, 0);
    if (total !== CREATOR_ROYALTY_UNITS) {
      return null;
    }
    return rows;
  }

  // Base64 without a data: URL round trip, and in chunks: a single
  // String.fromCharCode over a whole image blows the argument limit.
  async function fileToBase64(file) {
    const bytes = new Uint8Array(await file.arrayBuffer());
    let binary = "";
    for (let offset = 0; offset < bytes.length; offset += 0x8000) {
      binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
    }
    return btoa(binary);
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

  // -- Royalty split (see #royalty-field's hint and collectRoyaltyUnits) --
  //
  // Seeded with the chain's real default distribution (95% creator / 5%
  // protocol — see DEFAULT_CREATOR_ROYALTY_PERCENT above) so "+ Add payee"
  // starts from a running total that already means something, instead of an
  // empty, confusing control. The total may never exceed 100%: an edit that
  // would push it over is reverted (enforceRoyaltyCap), with a visible reason
  // rather than a silent clamp. There is no wallet integration to resolve the
  // creator's own address client-side, so that row is honestly labeled "You"
  // rather than carrying an invented 0x address.

  function addRoyaltyRow(address, percent, isProtocol = false) {
    const row = document.createElement("div");
    row.className = "royalty-row";
    // The protocol's share is minted by the contracts themselves; it is shown
    // so the running total means something, and never sent as a payee.
    if (isProtocol) {
      row.dataset.protocol = "true";
    }
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
    if (isProtocol) {
      // The contracts mint this share from CentralStorage.protocolShares()
      // whatever the creator does, so an input that accepted edits would be
      // lying — and an edit would silently change what the other rows have to
      // come to. Shown, counted in the total, and fixed.
      addr.readOnly = true;
      pct.readOnly = true;
      del.disabled = true;
      del.title = "The protocol share is set by the contract";
      row.append(addr, pct, del);
    } else {
      addr.addEventListener("input", refreshRoyaltyTotal);
      pct.addEventListener("input", refreshRoyaltyTotal);
      pct.addEventListener("change", () => enforceRoyaltyCap(pct));
      row.append(addr, pct, del);
    }
    els.royaltyRows.appendChild(row);
    refreshRoyaltyTotal();
  }

  function seedDefaultRoyaltyRows() {
    els.royaltyRows.innerHTML = "";
    addRoyaltyRow("You", DEFAULT_CREATOR_ROYALTY_PERCENT);
    addRoyaltyRow("Elacity", DEFAULT_PROTOCOL_ROYALTY_PERCENT, true);
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

  // Paint what the server's journal says, rather than what this page guessed.
  // "pending" leaves the dot untouched: a stage the server has not started is
  // not a stage this page should mark, and blanking it would undo "analyze",
  // which the server does not describe at all.
  function applyServerProgress(progress) {
    for (const stage of stagesFromProgress(progress)) {
      if (stage.state === "pending") continue;
      setStage(stage.name, stage.state === "done" ? "done" : stage.state);
    }
  }

  function resetStages() {
    activeStageName = "";
    ["analyze", "encrypt", "publish", "assemble"].forEach((stage) => setStage(stage, ""));
  }

  function setStatus(text, kind) {
    els.statusText.textContent = text || "";
    els.statusText.className = `status${kind ? ` ${kind}` : ""}`;
  }

  async function protectAndList() {
    if (submitting || !selectedFile) return;
    const terms = validCopiesAndPrice();
    if (!terms) return;
    terms.listing = await collectListing();

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
      const published = await publishUntilSettled(targetUri, terms);
      // A returned listing is the proof for all three server stages at once:
      // the escrow settled, the ciphertext is verifiably available, and the
      // listing projected. "publish" is no longer exempt from this.
      setStage("encrypt", "done");
      setStage("publish", "done");
      setStage("assemble", "done");

      const mintId = published?.content_security?.mint_id || "";
      showSuccess(mintId, settledFrom(published?.content_security));
    } finally {
      submitting = false;
      refreshSubmitEnabled();
    }
  }

  // Resume a pending publish on its own instead of asking for another click.
  //
  // A publish that comes back pending is not finished and not failed: the
  // wallet approval or the chain evidence is still outstanding. The Creator
  // used to stop here and tell the creator to click "Protect and list" again,
  // which is work the page can do itself — and which read as a dead end,
  // because the same message appeared whether the creator had something to do
  // or not.
  //
  // So: poll the same publish. Say whose turn it is, from the server's typed
  // answer rather than from the prose. Stop at a bound rather than forever,
  // and when the bound is reached say plainly that the listing will finish on
  // its own and can be picked up by protecting the same file again — which is
  // true, because the mint keeps its identity across retries.
  async function publishUntilSettled(targetUri, terms) {
    const started = Date.now();
    for (;;) {
      try {
        return await publishWithRevisionRetry(targetUri, terms);
      } catch (error) {
        const pending = error?.pending;
        if (!pending) {
          throw error;
        }
        const waited = Date.now() - started;
        const budget = pending.awaitsPerson
          ? PENDING_PERSON_BUDGET_MS
          : PENDING_CHAIN_BUDGET_MS;
        if (waited >= budget) {
          throw error;
        }
        applyServerProgress(error.progress);
        setStatus(pendingStatusText(pending, waited));
        await sleep(PENDING_POLL_INTERVAL_MS);
      }
    }
  }

  function pendingStatusText(pending, waitedMs) {
    const seconds = Math.round(waitedMs / 1000);
    const elapsed = seconds >= 5 ? ` (${seconds}s)` : "";
    if (pending.awaitsPerson) {
      const where = pending.connectorId || "your wallet";
      return `Waiting for you to approve this transaction in ${where}${elapsed}...`;
    }
    if (pending.reason === "chain_settlement") {
      return `Approved. Waiting for the network to confirm the transaction${elapsed}...`;
    }
    return `Completing the wallet approval${elapsed}...`;
  }

  function sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
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
      buildPublishBody({
        uri: targetUri,
        ifRevision: revision,
        copies: terms.copies,
        price: terms.price,
        listing: terms.listing,
      }),
    );
    if (response.status === "error") {
      const message = response.message || "Protecting this file failed.";
      const error = new Error(message);
      // The actionable sentence the server puts beneath the stable one. Set
      // before any branch below returns, so every failure carries it.
      if (response.detail) {
        error.detail = String(response.detail);
      }
      // Typed refusals an app can act on rather than re-read.
      if (response.wallet_default) {
        error.walletDefault = response.wallet_default;
      }
      if (response.wallet_drift) {
        error.walletDrift = response.wallet_drift;
      }
      // A closed approval is terminal, and deliberately NOT read as pending:
      // polling on it would wait out the whole budget for something that can
      // never complete.
      if (response.approval_closed) {
        error.approvalClosed = response.approval_closed;
        throw error;
      }
      const pending = effectPendingFrom(response);
      if (pending) {
        error.pending = pending;
        // Rides the pending answer, which is the poll response — see
        // RuntimeCustodyEffectPending's own note on why it is not a second
        // channel.
        error.progress = response.effect_pending.progress;
      }
      throw error;
    }
    return response.data || {};
  }

  // `settledBefore` means the server raised no effect and sent no transaction:
  // the mint was already terminal when this request arrived, and all that
  // happened was the listing being re-published from the existing record.
  //
  // Saying "Listed for sale" for that is what made a replay read as fresh
  // work. The listing IS live either way, so this is not an error -- but the
  // creator has to be able to tell the two apart, especially since a mint
  // keeps the account it started with and that account may no longer be the
  // one they would choose today.
  function showSuccess(mintId, settled) {
    listed = true;
    if (settled && settled.before) {
      setStatus("Already listed.", "ok");
      els.successTitle.textContent = "Already listed.";
      els.successDetail.textContent = settledDetail(settled);
      els.successDetail.hidden = false;
    } else {
      setStatus("Listed for sale.", "ok");
      els.successTitle.textContent = "Listed for sale.";
      els.successDetail.textContent = "";
      els.successDetail.hidden = true;
    }
    els.mintIdText.textContent = mintId ? `Listing ${mintId}` : "";
    els.successPanel.classList.remove("hidden");
    refreshSubmitEnabled();
  }

  // Seconds since the epoch, as the mint journal records it.
  function settledDetail(settled) {
    const when = Number(settled.at);
    const on = Number.isFinite(when) && when > 0
      ? ` on ${new Date(when * 1000).toLocaleString()}`
      : "";
    const tx = settled.transactionHash
      ? ` Transaction ${settled.transactionHash}.`
      : "";
    // A settled mint is never refused over a changed transaction default --
    // the chain effect is done -- so naming the account it minted on is the
    // only way a creator who has switched wallets since can see that this
    // listing is not on the account they would pick today.
    const seller = settled.sellerAddress
      ? ` It was minted on ${settled.sellerAddress}.`
      : "";
    return `This file was already minted${on}, so nothing new was sent to the chain.${seller}${tx}`;
  }


  // Reached only after the poll budget is spent, so this is "still waiting",
  // not "your turn and nobody told you". The Protect step stays in its
  // in-progress state and the file, copies and price stay in place, so
  // protecting the same file again picks the same mint back up — the mint
  // keeps its identity across retries, which is what makes that safe.
  function showPending(pending) {
    if (pending && pending.awaitsPerson) {
      const where = pending.connectorId || "your wallet";
      setStatus(
        `Still waiting for approval in ${where}. Approve it there and protect this file again to finish the listing.`,
      );
      return;
    }
    setStatus(
      "The transaction is approved and the network has not confirmed it yet. Protect this file again in a moment to finish the listing.",
    );
  }

  // Prefer the server's actionable sentence over its stable one: the stable
  // message is deliberately the same across many causes, so on its own it
  // tells the creator nothing they can act on.
  function showFailure(error) {
    if (activeStageName) setStage(activeStageName, "err");
    const actionable = error?.detail || error?.message;
    setStatus(String(actionable || error || "Protecting this file failed."), "err");
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
