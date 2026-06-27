// Create portal — de-privileged app frame.
//
// PRINCIPLES THIS FILE OBEYS:
//   * No ambient authority (#3): the frame holds NO keys, NO chain RPC, NO wallet.
//     It carries only its launch capability (x-elastos-home-token) and asks the HOST
//     to orchestrate the producer spine under that capability.
//   * Carrier plane (#4): the frame talks capability-scoped host routes, never a
//     provider's internals, a raw socket, or a public-web endpoint.
//   * Fail closed, then explain (#11): if the host `creator` capability route is not
//     present, refuse with a clear message rather than pretend to mint.
//   * UI is not authority (#16): opening this page grants nothing; every action is
//     gated by the launch capability the host bound to this frame.
//
// THE SPINE THE HOST ROUTE DRIVES (already-proven providers):
//   encrypt-provider seal_inline_threshold  -> escrow CEK shares to the 2-of-3 quorum
//   publish-provider  prepare_publish        -> unsigned mint (contentId == bytes16 KID)
//   wallet            sign                    -> sign the mint
//   chain-provider    broadcast_transaction   -> broadcast to the chosen channel
// The CEK custody block is the dKMS escrow descriptor (cenc:elastos-pq-hybrid-threshold-v0),
// swapped in where PC2's creator wrote Lit (litCiphertext / litBackend:'chipotle').

const APP_ID = "creator";

// Defense in depth: key material must NEVER reach this frame. If a host regression
// ever surfaced it, refuse rather than risk leaking it.
const FORBIDDEN_KEY_FIELDS = [
  "raw_cek",
  "cek",
  "wrapped_cek",
  "sealed_cek",
  "private_key",
  "seed",
  "kms",
  "wallet_key",
];

const MEDIA_PREFIXES = ["video/", "audio/"];

let selectedFile = null;
let customThumbnail = null;
// PC2 access method: "free" | "buy_once" | "buy_and_resell". Drives the price/royalty/resell UI.
let accessMethod = "free";

const els = {
  drop: document.getElementById("drop"),
  dropTitle: document.getElementById("drop-title"),
  dropMeta: document.getElementById("drop-meta"),
  file: document.getElementById("file"),
  title: document.getElementById("title"),
  desc: document.getElementById("desc"),
  price: document.getElementById("price"),
  priceRow: document.getElementById("price-row"),
  currency: document.getElementById("currency"),
  copies: document.getElementById("copies"),
  category: document.getElementById("category"),
  methodGrid: document.getElementById("method-grid"),
  resellField: document.getElementById("resell-field"),
  resellerCut: document.getElementById("reseller-cut"),
  royaltyField: document.getElementById("royalty-field"),
  royaltyRows: document.getElementById("royalty-rows"),
  royaltyAdd: document.getElementById("royalty-add"),
  royaltyTotal: document.getElementById("royalty-total"),
  aiLicensing: document.getElementById("ai-licensing"),
  adultFlag: document.getElementById("adult-flag"),
  legalAttest: document.getElementById("legal-attest"),
  thumbDrop: document.getElementById("thumb-drop"),
  thumbInput: document.getElementById("thumb-input"),
  thumbPreviewWrap: document.getElementById("thumb-preview-wrap"),
  thumbPreviewImg: document.getElementById("thumb-preview-img"),
  thumbRemove: document.getElementById("thumb-remove"),
  previewSettings: document.getElementById("preview-settings"),
  previewEnabled: document.getElementById("preview-enabled"),
  previewControls: document.getElementById("preview-controls"),
  previewDuration: document.getElementById("preview-duration"),
  previewDurationDisplay: document.getElementById("preview-duration-display"),
  wallet: document.getElementById("wallet"),
  walletHint: document.getElementById("wallet-hint"),
  channel: document.getElementById("channel"),
  channelHint: document.getElementById("channel-hint"),
  channelManual: document.getElementById("channel-manual"),
  channelManualInput: document.getElementById("channel-manual-input"),
  channelManualHint: document.getElementById("channel-manual-hint"),
  createChannel: document.getElementById("create-channel"),
  channelName: document.getElementById("channel-name"),
  channelScope: document.getElementById("channel-scope"),
  createChannelBtn: document.getElementById("create-channel-btn"),
  createChannelHint: document.getElementById("create-channel-hint"),
  mint: document.getElementById("mint"),
  steps: document.getElementById("steps"),
  status: document.getElementById("status"),
  enableTrading: document.getElementById("enable-trading"),
  enableTradingHint: document.getElementById("enable-trading-hint"),
  mintAnother: document.getElementById("mint-another"),
};

// The sentinel channel option that reveals the inline create-channel form.
const CREATE_CHANNEL_VALUE = "__create__";
// The sentinel that reveals the manual channel-address input (fail-closed fallback for a
// channel discovery hasn't surfaced yet — still verified on-chain server-side before mint).
const MANUAL_CHANNEL_VALUE = "__manual__";
// Re-poll cadence while the on-chain channel index is still backfilling older channels.
const CHANNEL_POLL_MS = 2500;
let channelPollTimer = null;
// Monotonic token to cancel a stale mint-confirmation watcher when a new mint starts.
let mintWatchToken = 0;
// The just-minted asset's bytes16 content id (KID). Every confirm/approve check is PINNED to
// it so a fresh mint is never reported tradable off an EARLIER asset in the same channel
// (each asset is its own operative contract and needs its own gateway approval).
let currentMintContentId = "";
// The mint's wallet-approval request id. Once the owner approves the mint in the Wallet app, the
// completed approval records the broadcast tx hash; passing this id lets the host resolve that hash
// and confirm via the tx RECEIPT (one cheap call) instead of a slow log scan — so Step 2 unlocks
// the moment the mint mines (PC2 `tx.wait()` immediacy). Empty ⇒ host falls back to the log scan.
let currentMintRequestId = "";
// True while a freshly-minted asset's first tx (mint) is still pending confirmation: Step 2
// stays visible-but-disabled until the chain confirms it (PC2 gates the 2nd tx the same way).
let tradeGated = false;
const DEFAULT_TRADE_HINT =
  "Once the mint confirms on-chain, approve the gateway so others can trade your asset.";

function query(name) {
  try {
    return new URL(window.location.href).searchParams.get(name);
  } catch (_error) {
    return null;
  }
}

const homeToken = query("home_token");

function launchHeaders() {
  return homeToken ? { "x-elastos-home-token": homeToken } : {};
}

function appUrl(suffix) {
  return "/api/apps/" + encodeURIComponent(APP_ID) + suffix;
}

function setStatus(text, kind) {
  els.status.textContent = text || "";
  els.status.className = "status" + (kind ? " " + kind : "");
}

// ── Live pipeline progress ───────────────────────────────────────────────────
// An overall bar plus a default-open step roadmap (icon · label · status · thin bar).
// The backend reports coarse stage status (pending|active|done|err); there is NO per-frame
// ffmpeg telemetry yet, so we DO NOT fabricate a precise percentage. An active stage shows an
// indeterminate (sweeping) bar to convey activity, and the bar's *filled* width / numeric %
// move only on real, server-confirmed stage milestones. (Improvement A — measured transcode %
// via ffmpeg `-progress` + a job-keyed channel — will replace the indeterminate state.)
const STEP_ICON = { waiting: "○", active: "◉", done: "✓", err: "✗" };
const STEP_STATUS_TEXT = { waiting: "Waiting", active: "In progress", done: "Done", err: "Failed" };
// Relative weights — transcode dominates the wall-clock for video, so it owns the widest band.
const STEP_WEIGHTS = {
  analyze: 1, package: 6, encrypt: 2, publish: 2, sign: 1, broadcast: 3, approve: 1,
};

const progressState = { active: null, activeSince: 0, done: new Set(), errored: null, timer: null };

function stepRow(name) {
  return els.steps ? els.steps.querySelector('li[data-step="' + name + '"]') : null;
}

// The ordered list of currently-VISIBLE steps (media rows are hidden for non-media assets).
function visibleSteps() {
  if (!els.steps) return [];
  return Array.from(els.steps.querySelectorAll("li[data-step]"))
    .filter((li) => !li.classList.contains("hidden"))
    .map((li) => li.dataset.step);
}

// Cumulative [start,end] milestone band per visible step, weighted by STEP_WEIGHTS over 0..100.
function milestoneBands() {
  const steps = visibleSteps();
  const total = steps.reduce((a, s) => a + (STEP_WEIGHTS[s] || 1), 0) || 1;
  const bands = {};
  let acc = 0;
  for (const s of steps) {
    const start = (acc / total) * 100;
    acc += STEP_WEIGHTS[s] || 1;
    bands[s] = { start, end: (acc / total) * 100 };
  }
  return bands;
}

function setStepVisual(name, vstate) {
  const li = stepRow(name);
  if (!li) return;
  li.classList.remove("active", "done", "err");
  if (vstate === "active") li.classList.add("active");
  else if (vstate === "done") li.classList.add("done");
  else if (vstate === "err") li.classList.add("err");
  const icon = li.querySelector(".step-icon");
  const status = li.querySelector(".step-status");
  const key = vstate || "waiting";
  if (icon) icon.textContent = STEP_ICON[key] || STEP_ICON.waiting;
  if (status) status.textContent = STEP_STATUS_TEXT[key] || STEP_STATUS_TEXT.waiting;
  const fill = li.querySelector(".step-bar-fill");
  if (fill) {
    if (vstate === "active") {
      // Indeterminate sweep — no fabricated fill level; activity without a fake measurement.
      fill.classList.add("indeterminate");
      fill.style.width = "";
    } else {
      fill.classList.remove("indeterminate");
      // done/err widths are forced by CSS; waiting resets to 0.
      if (!vstate) fill.style.width = "0%";
    }
  }
}

function setOverall(pct, label) {
  const fill = document.getElementById("overall-progress-fill");
  const pctEl = document.getElementById("overall-progress-pct");
  const labelEl = document.getElementById("overall-progress-label");
  const wrap = document.getElementById("overall-progress");
  const clamped = Math.max(0, Math.min(100, pct));
  if (fill) {
    fill.style.width = clamped.toFixed(1) + "%";
    fill.classList.toggle("indeterminate", clamped < 100);
  }
  // No fabricated precise %: the number appears only at true completion. Until then the moving
  // (indeterminate) bar conveys activity and the filled width reflects only real stage milestones.
  if (pctEl) pctEl.textContent = clamped >= 100 ? "100%" : "";
  if (label && labelEl) labelEl.textContent = label;
  if (wrap) wrap.classList.toggle("done", clamped >= 100);
}

// Drive a stage to a MEASURED percentage: determinate step bar + overall within its band.
// Used only when the server reports a real pct (e.g. ffmpeg transcode progress).
function setStepPct(name, pct) {
  const li = stepRow(name);
  if (!li) return;
  const clamped = Math.max(0, Math.min(100, pct));
  const fill = li.querySelector(".step-bar-fill");
  if (fill) {
    fill.classList.remove("indeterminate");
    fill.style.width = clamped + "%";
  }
  const band = milestoneBands()[name];
  if (band) {
    const frac = clamped / 100;
    setOverall(band.start + frac * (band.end - band.start), currentStageLabel(name));
  }
}

function showOverall(show) {
  const wrap = document.getElementById("overall-progress");
  if (wrap) wrap.classList.toggle("hidden", !show);
  if (show && wrap) wrap.setAttribute("aria-hidden", "false");
  else if (wrap) wrap.setAttribute("aria-hidden", "true");
}

function stopProgressLoop() {
  if (progressState.timer) {
    clearInterval(progressState.timer);
    progressState.timer = null;
  }
}

// Drive one step. Accepts the same coarse states the server reports (active|done|err) plus ""
// (reset to waiting). Maintains the creep engine + overall bar; later-stage "done" implicitly
// completes earlier visible stages, matching the server's monotonic advance.
function setStep(name, state) {
  if (!stepRow(name)) return;
  const bands = milestoneBands();
  if (state === "active") {
    if (progressState.active && progressState.active !== name) {
      progressState.done.add(progressState.active);
      setStepVisual(progressState.active, "done");
    }
    progressState.active = name;
    progressState.activeSince = Date.now();
    setStepVisual(name, "active");
    if (bands[name]) setOverall(bands[name].start, currentStageLabel(name));
  } else if (state === "done") {
    // Complete this step and every earlier visible step (server advance is monotonic).
    const order = visibleSteps();
    const idx = order.indexOf(name);
    for (let i = 0; i <= idx; i += 1) {
      progressState.done.add(order[i]);
      setStepVisual(order[i], "done");
    }
    if (progressState.active === name) progressState.active = null;
    if (bands[name]) setOverall(bands[name].end, currentStageLabel(name));
  } else if (state === "err") {
    progressState.errored = name;
    setStepVisual(name, "err");
    if (progressState.active === name) progressState.active = null;
    stopProgressLoop();
  } else {
    setStepVisual(name, "");
  }
}

const STAGE_LABEL = {
  analyze: "Analyzing source…",
  package: "Transcoding & packaging…",
  encrypt: "Encrypting & escrowing to your quorum…",
  publish: "Publishing to IPFS…",
  sign: "Assembling the mint…",
  broadcast: "Waiting for the on-chain mint…",
  approve: "Enabling trading…",
};
function currentStageLabel(name) {
  return STAGE_LABEL[name] || "Working…";
}

// Mark the whole pipeline complete: every visible step done, overall at 100%.
function completeProgress() {
  visibleSteps().forEach((s) => {
    progressState.done.add(s);
    setStepVisual(s, "done");
  });
  progressState.active = null;
  stopProgressLoop();
  setOverall(100, "Published successfully");
}

// Show/hide the media-only timeline rows (Analyze / Transcode) for non-media assets.
function setMediaStepsVisible(visible) {
  els.steps
    .querySelectorAll(".media-step")
    .forEach((li) => li.classList.toggle("hidden", !visible));
}

// ── Live encode/publish progress ─────────────────────────────────────────────
// The mint stays a single blocking POST; the server records coarse stage progress
// (analyze → package → encrypt → publish → sign) under a client job id, which we
// poll @1.5s to drive the SAME unified timeline instead of showing a frozen spinner.
// The server stage names map 1:1 onto the timeline's data-step ids.
let techPollTimer = null;

function setTechStep(name, state) {
  setStep(name, state);
}

function stopTechProgress() {
  if (techPollTimer) {
    clearInterval(techPollTimer);
    techPollTimer = null;
  }
}

function startTechProgress(jobId) {
  ["analyze", "package", "encrypt", "publish", "sign"].forEach((s) => setTechStep(s, ""));
  stopTechProgress();
  const poll = async () => {
    try {
      const resp = await fetch(
        appUrl("/prepare-progress/" + encodeURIComponent(jobId)),
        { headers: { ...launchHeaders() } },
      );
      if (!resp.ok) return; // 404 until the job registers / after it prunes
      const data = await resp.json();
      if (Array.isArray(data.stages)) {
        for (const st of data.stages) {
          const cls = st.status === "done" || st.status === "active" || st.status === "err"
            ? st.status
            : "";
          setTechStep(st.name, cls);
          // Measured transcode % (Improvement A): when the server reports a real pct for the
          // active stage, switch that stage from indeterminate to a determinate bar and drive
          // the overall bar within the stage's milestone band. Absent ⇒ stays indeterminate.
          if (cls === "active" && typeof st.pct === "number") {
            setStepPct(st.name, st.pct);
          }
        }
      }
      if (data.done) stopTechProgress();
    } catch (_err) {
      /* transient; keep polling */
    }
  };
  poll();
  techPollTimer = setInterval(poll, 1500);
}

function resetSteps() {
  mintWatchToken += 1; // cancel any in-flight mint-confirmation watcher from a prior mint
  tradeGated = false;
  stopProgressLoop();
  progressState.active = null;
  progressState.activeSince = 0;
  progressState.done = new Set();
  progressState.errored = null;
  // The step roadmap stays visible (open by default); only reset rows to "Waiting" and hide
  // the overall bar until a mint actually starts.
  ["analyze", "package", "encrypt", "publish", "sign", "broadcast", "approve"].forEach((s) => {
    setStepVisual(s, "");
    const li = stepRow(s);
    const fill = li && li.querySelector(".step-bar-fill");
    if (fill) fill.style.width = "0%";
  });
  stopTechProgress();
  showOverall(false);
  setOverall(0, "Preparing your asset…");
  if (els.enableTrading) els.enableTrading.classList.remove("is-ready");
  if (els.enableTrading) els.enableTrading.disabled = false;
  if (els.enableTradingHint) els.enableTradingHint.textContent = DEFAULT_TRADE_HINT;
  if (els.mintAnother) els.mintAnother.hidden = true;
  // Button visibility is governed by the wallet+channel selection (refreshTradeEnabled),
  // so it stays available for the latest minted asset; only re-enable it here.
  refreshTradeEnabled();
}

// Reset the asset-specific fields so the creator can mint another without reloading. The wallet
// and channel selections are KEPT (the next asset usually targets the same channel); only the
// file, title/description, cover, category, preview and progress are cleared.
function resetForMintAnother() {
  selectedFile = null;
  customThumbnail = null;
  autoCoverUrl = "";
  currentMintContentId = "";
  currentMintRequestId = "";
  if (els.file) els.file.value = "";
  if (els.dropTitle) els.dropTitle.textContent = "Choose a file";
  if (els.dropMeta) els.dropMeta.innerHTML = "media or any document &middot; click or drop";
  if (els.title) els.title.value = "";
  if (els.desc) els.desc.value = "";
  if (els.category) els.category.value = "";
  // Cover thumbnail back to "auto-generate".
  if (els.thumbInput) els.thumbInput.value = "";
  if (els.thumbPreviewWrap) els.thumbPreviewWrap.classList.add("hidden");
  if (els.thumbDrop) els.thumbDrop.classList.remove("hidden");
  // Free-preview section hides until a new media file is chosen.
  if (els.previewSettings) els.previewSettings.classList.add("hidden");
  if (els.previewControls) els.previewControls.classList.add("hidden");
  if (els.previewEnabled) els.previewEnabled.checked = false;
  resetSteps();
  setStatus("", "");
  refreshMintEnabled();
  // Bring the file picker back into view so the next mint starts where the eye is.
  if (els.drop) els.drop.scrollIntoView({ behavior: "smooth", block: "center" });
}

function classifyMedia(mime) {
  return MEDIA_PREFIXES.some((p) => (mime || "").startsWith(p));
}

// Extension → MIME for types browsers report unreliably (often "" or octet-stream). The MIME we
// persist drives the viewer routing (EPUB→html-lock reader, CBZ→comic pager, 3D→model viewer,
// PDF/image/text→pixel-lock), so a correct type here is what makes each asset open in the right
// viewer. Mirrors PC2's creator (elacity-creator app.js) so the on-chain metadata stays aligned.
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

// Resolve the canonical MIME for a file: trust our extension map for the known set (we classify
// these better than the browser), else fall back to the browser type, else octet-stream.
function resolveMime(file) {
  const ext = (file.name.split(".").pop() || "").toLowerCase();
  if (EXT_MIME[ext]) return EXT_MIME[ext];
  return file.type || "application/octet-stream";
}

function assertNoKeyMaterial(payload) {
  const lowered = JSON.stringify(payload || {}).toLowerCase();
  for (const field of FORBIDDEN_KEY_FIELDS) {
    if (lowered.includes('"' + field + '"')) {
      throw new Error("host response carried a forbidden key field: " + field);
    }
  }
}

function humanSize(bytes) {
  if (bytes < 1024) return bytes + " B";
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + " KB";
  return (bytes / (1024 * 1024)).toFixed(1) + " MB";
}

function onFile(file) {
  if (!file) return;
  selectedFile = file;
  const mime = resolveMime(file);
  const kind = classifyMedia(mime) ? "media" : "object";
  els.dropTitle.textContent = file.name;
  els.dropMeta.innerHTML =
    humanSize(file.size) + " &middot; " + mime + ' <span class="badge">' + kind + "</span>";
  if (!els.title.value) els.title.value = file.name.replace(/\.[^.]+$/, "");
  // The free-preview clip only applies to time-based media (video/audio). Auto-SELECT it for
  // media (a free preview is the expected default for a sellable track) and reveal its controls;
  // the creator can still untick it. Non-media hides the whole section.
  const isMedia = kind === "media";
  if (els.previewSettings) els.previewSettings.classList.toggle("hidden", !isMedia);
  if (els.previewEnabled) els.previewEnabled.checked = isMedia;
  if (els.previewControls) els.previewControls.classList.toggle("hidden", !isMedia);
  // Reflect the file type in the step roadmap immediately: media gets the Analyze/Transcode
  // rows; non-media skips them. Shows the creator exactly what will happen before they mint.
  setMediaStepsVisible(isMedia);
  autoDetectCategory(file, mime);
  refreshMintEnabled();
  // Generate the file-derived cover so the live preview shows the actual cover the asset will
  // use (a frame for video, a waveform card for audio, a blurred still for images, etc.) —
  // not just a type chip. A custom upload still overrides it.
  autoCoverUrl = "";
  generateAutoThumbnail(file, mime)
    .then((thumb) => {
      if (selectedFile === file && thumb && thumb.b64) {
        autoCoverUrl = "data:" + (thumb.mime || "image/jpeg") + ";base64," + thumb.b64;
        renderPreview();
      }
    })
    .catch(() => {});
}

// Source-code mimes that should land in the "document" category (mirrors the dDRM viewer's code
// renderer set). Kept here so the category auto-detect matches what actually renders.
const CODE_MIMES = [
  "application/json",
  "application/javascript",
  "application/xml",
  "application/x-yaml",
  "application/yaml",
  "application/toml",
  "application/x-sh",
];

// Auto-pick the category dropdown from the file when the creator hasn't chosen one. Every type we
// can actually open in the viewer maps to a category here (3D, AI model, ebook, comic, video,
// music, image, document for PDF/text/markdown/code). PC2 app.js:992-995 seeds the model rules.
function autoDetectCategory(file, mime) {
  if (!els.category || els.category.value) return;
  const ext = (file.name.split(".").pop() || "").toLowerCase();
  let cat = "";
  if (["glb", "gltf", "obj", "fbx", "stl", "usdz", "ply"].includes(ext)) cat = "3d-model";
  else if (["safetensors", "ckpt", "gguf", "onnx", "pt", "pth", "bin"].includes(ext))
    cat = "ai-model";
  else if (ext === "epub" || mime === "application/epub+zip") cat = "ebook";
  else if (
    ext === "cbz"
    || mime === "application/vnd.comicbook+zip"
    || mime === "application/x-cbz"
  ) cat = "comic";
  else if (mime.startsWith("video/")) cat = "video";
  else if (mime.startsWith("audio/")) cat = "music";
  else if (mime.startsWith("image/")) cat = "image";
  else if (mime === "application/pdf") cat = "document";
  else if (mime.startsWith("text/") || CODE_MIMES.includes(mime)) cat = "document";
  if (cat) els.category.value = cat;
}

function isEvmAddress(v) {
  return /^0x[0-9a-fA-F]{40}$/.test((v || "").trim());
}

function selectedChannel() {
  const v = els.channel.value;
  if (v === MANUAL_CHANNEL_VALUE) {
    const manual = (els.channelManualInput.value || "").trim();
    return isEvmAddress(manual) ? manual : "";
  }
  return v && v !== CREATE_CHANNEL_VALUE ? v : "";
}

// Join a list into a readable "a, b and c" clause for the requirements hint.
function listToText(items) {
  if (items.length <= 1) return items.join("");
  return items.slice(0, -1).join(", ") + " and " + items[items.length - 1];
}

function refreshMintEnabled() {
  // Fail-closed: a wallet AND a real channel selection are required — no silent default.
  const missing = [];
  if (!selectedFile) missing.push("a file");
  if (!els.title.value.trim()) missing.push("a title");
  if (!els.wallet.value) missing.push("a wallet");
  if (!selectedChannel()) missing.push("a channel");
  els.mint.disabled = missing.length > 0;
  const req = document.getElementById("mint-requirements");
  if (req) req.textContent = missing.length ? "Add " + listToText(missing) + " to mint." : "";
  refreshTradeEnabled();
  renderPreview();
}

// The trade-enabling 2nd tx targets the newest minted asset in the selected channel, so the
// "Enable trading" action is available whenever a wallet + a real channel are chosen — both
// right after a mint AND for an asset minted earlier. It's confirmation-gated server-side.
function refreshTradeEnabled() {
  if (!els.enableTrading) return;
  const ready = Boolean(els.wallet.value && selectedChannel());
  els.enableTrading.hidden = !ready;
  if (els.enableTradingHint) els.enableTradingHint.hidden = !ready;
  // While a fresh mint's tx1 is pending, keep Step 2 un-clickable (PC2 parity) even if the
  // wallet/channel selection re-runs this. The mint-confirmation watcher lifts the gate.
  if (tradeGated) {
    els.enableTrading.disabled = true;
    els.enableTrading.classList.remove("is-ready");
  }
}

// After a fresh mint is broadcast: show Step 2 but DISABLE it until the mint confirms on-chain.
function gateTradeUntilConfirmed() {
  tradeGated = true;
  refreshTradeEnabled();
  if (els.enableTradingHint) {
    els.enableTradingHint.hidden = !(els.wallet.value && selectedChannel());
    els.enableTradingHint.textContent =
      "Waiting for the mint to confirm on-chain… Step 2 unlocks automatically.";
  }
}

// ── wallet + channel discovery ───────────────────────────────────────────────
// Learn the principal's linked Base wallet(s) from the host (the frame holds no wallet
// authority — #3). Populate the picker; the chosen address is the mint/deploy signer.
async function loadWallets() {
  try {
    const resp = await fetch(appUrl("/wallet"), { headers: { ...launchHeaders() } });
    if (!resp.ok) {
      els.walletHint.textContent =
        "No linked Base wallet — link your wallet on Base in the Wallet app, then reopen Create.";
      return;
    }
    const info = await resp.json();
    const addrs = (info && info.addresses) || [];
    els.wallet.innerHTML = "";
    if (addrs.length === 0) {
      els.wallet.innerHTML = '<option value="">No wallet linked on Base</option>';
      els.walletHint.textContent =
        "No linked Base wallet — link your wallet on Base in the Wallet app, then reopen Create.";
      return;
    }
    addrs.forEach((addr) => {
      const opt = document.createElement("option");
      opt.value = addr;
      opt.textContent = addr.slice(0, 8) + "…" + addr.slice(-6);
      els.wallet.appendChild(opt);
    });
    els.wallet.value = addrs[0];
    els.walletHint.textContent = "Signs the mint on Base.";
    await loadChannels();
  } catch (err) {
    els.walletHint.textContent = "Could not load wallet: " + err.message;
  }
}

// Discover the channels the selected wallet already owns (host scans ChannelCreated logs).
// No silent default: if there are none, the only path forward is "+ Create a new channel".
// Discover the wallet's channels. The host index is RESUMABLE: deep (older) channels surface
// across calls, so while `indexing` is true we re-poll and show progress. The current
// selection is preserved across re-polls so the user can pick as soon as their channel lands.
async function loadChannels(opts) {
  const isPoll = opts && opts.poll;
  if (channelPollTimer && !isPoll) {
    clearTimeout(channelPollTimer);
    channelPollTimer = null;
  }
  const wallet = els.wallet.value;
  if (!wallet) {
    els.channel.innerHTML = '<option value="">Select a wallet first…</option>';
    els.channelManual.classList.add("hidden");
    refreshMintEnabled();
    return;
  }
  if (!isPoll) {
    els.channel.innerHTML = '<option value="">Loading channels…</option>';
  }
  const prevSelection = els.channel.value;
  try {
    const resp = await fetch(
      appUrl("/channels?creator=" + encodeURIComponent(wallet)),
      { headers: { ...launchHeaders() } }
    );
    const info = await resp.json().catch(() => ({}));
    if (!resp.ok) {
      els.channel.innerHTML = '<option value="">Channel discovery failed</option>';
      els.channelHint.textContent = info.error || "Could not discover channels.";
      addManualOption();
      addCreateOption();
      refreshMintEnabled();
      return;
    }
    const channels = (info && info.channels) || [];
    const indexing = !!(info && info.indexing);
    els.channel.innerHTML = "";
    const placeholder = document.createElement("option");
    placeholder.value = "";
    placeholder.textContent = channels.length ? "Select a channel…" : "No channels found yet";
    els.channel.appendChild(placeholder);
    channels.forEach((ch) => {
      const addr = ch.address || "";
      const opt = document.createElement("option");
      opt.value = addr;
      opt.textContent = addr.slice(0, 10) + "…" + addr.slice(-6);
      els.channel.appendChild(opt);
    });
    addManualOption();
    addCreateOption();
    // Restore the prior selection if it still exists (don't yank it mid-poll).
    if (prevSelection) {
      els.channel.value = prevSelection;
    }

    if (channels.length > 0) {
      els.channelHint.textContent = indexing
        ? channels.length + " channel(s) found — still scanning for older ones…"
        : channels.length + " channel(s) found on-chain.";
    } else if (indexing) {
      els.channelHint.textContent =
        "Scanning Base for your channels… " + indexingProgress(info) +
        " (or paste your channel address below).";
    } else {
      els.channelHint.textContent =
        "No channels owned by this wallet. Create one, or paste its address below.";
    }

    // Keep polling while the backfill is incomplete, so deep channels appear without a reload.
    if (indexing) {
      channelPollTimer = setTimeout(() => loadChannels({ poll: true }), CHANNEL_POLL_MS);
    } else if (channelPollTimer) {
      clearTimeout(channelPollTimer);
      channelPollTimer = null;
    }
  } catch (err) {
    els.channel.innerHTML = '<option value="">Channel discovery failed</option>';
    els.channelHint.textContent = "Could not discover channels: " + err.message;
    addManualOption();
    addCreateOption();
  }
  refreshMintEnabled();
}

// Rough % of the factory history scanned so far (latest..deploy down to scanned_floor).
function indexingProgress(info) {
  const latest = Number(info.latest_block),
    deploy = Number(info.deploy_block),
    floor = Number(info.scanned_floor);
  if (!latest || !deploy || !floor || latest <= deploy) return "";
  const pct = Math.min(100, Math.max(0, ((latest - floor) / (latest - deploy)) * 100));
  return pct.toFixed(0) + "%";
}

function addManualOption() {
  const opt = document.createElement("option");
  opt.value = MANUAL_CHANNEL_VALUE;
  opt.textContent = "Enter channel address manually…";
  els.channel.appendChild(opt);
}

function addCreateOption() {
  const opt = document.createElement("option");
  opt.value = CREATE_CHANNEL_VALUE;
  opt.textContent = "+ Create a new channel…";
  els.channel.appendChild(opt);
}

function onChannelChange() {
  const creating = els.channel.value === CREATE_CHANNEL_VALUE;
  const manual = els.channel.value === MANUAL_CHANNEL_VALUE;
  els.createChannel.classList.toggle("hidden", !creating);
  els.channelManual.classList.toggle("hidden", !manual);
  refreshMintEnabled();
}

// Prepare an UNSIGNED createChannel and queue a wallet approval. The owner deploys it; once
// confirmed on-chain, refreshing channels surfaces it for selection (no auto-mint).
async function createChannel() {
  const wallet = els.wallet.value;
  const name = els.channelName.value.trim();
  if (!wallet) {
    els.createChannelHint.textContent = "Select a wallet first.";
    return;
  }
  if (!name) {
    els.createChannelHint.textContent = "Enter a channel name.";
    return;
  }
  els.createChannelBtn.disabled = true;
  els.createChannelHint.textContent = "Preparing channel…";
  try {
    const resp = await fetch(appUrl("/create-channel"), {
      method: "POST",
      headers: { ...launchHeaders(), "Content-Type": "application/json" },
      body: JSON.stringify({
        name: name,
        scope: els.channelScope.value,
        creator_address: wallet,
      }),
    });
    const result = await resp.json().catch(() => ({}));
    assertNoKeyMaterial(result);
    if (!resp.ok) {
      els.createChannelHint.textContent = result.error || "Create failed: " + resp.status;
      els.createChannelBtn.disabled = false;
      return;
    }
    const approval = result.channel_approval || {};
    if (approval.request_id) {
      els.createChannelHint.textContent =
        "Channel prepared — approve it in the Wallet app, then click Refresh to select it.";
    } else {
      els.createChannelHint.textContent =
        "Prepared but no wallet approval was queued — connect your wallet on Base.";
    }
  } catch (err) {
    els.createChannelHint.textContent = "Create failed: " + err.message;
  }
  els.createChannelBtn.disabled = false;
}

// ── host capability preflight ────────────────────────────────────────────────
// Confirm the runtime exposes the `creator` capability route for this launch
// BEFORE we let the user try to mint. Fail closed with a clear message otherwise.
async function preflight() {
  if (!homeToken) {
    setStatus(
      "No launch capability — open Create from Home so it can be granted mint authority.",
      "err"
    );
    return false;
  }
  try {
    const resp = await fetch(appUrl("/status"), { headers: { ...launchHeaders() } });
    if (resp.status === 404 || resp.status === 501) {
      setStatus(
        "The runtime's Create capability route is not available yet. Mint is fail-closed until the host wires the producer spine.",
        "err"
      );
      els.mint.disabled = true;
      return false;
    }
    if (!resp.ok) {
      setStatus("Create capability unavailable: " + resp.status, "err");
      return false;
    }
    const info = await resp.json();
    if (info && info.quorum) {
      setStatus("Ready — escrow target: " + info.quorum, "ok");
    } else {
      setStatus("Ready.", "ok");
    }
    return true;
  } catch (err) {
    setStatus("Create capability unavailable: " + err.message, "err");
    return false;
  }
}

// ── cover thumbnail generation (browser-side; the frame already holds the bytes) ─────────────
// Mirrors PC2's `elacity-creator` cascade (app.js:4265): a degraded, public teaser derived from
// the asset. Custom upload wins; otherwise a low-res BLURRED still for images, a frame for video,
// a synthetic waveform for audio, a canvas teaser for text, and a generative gradient template
// for anything else. The host pins whatever bytes come back and sets `metadata.image`.
function blobToBase64(blob) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error("thumbnail read error"));
    reader.onload = () => {
      const r = reader.result || "";
      const comma = r.indexOf(",");
      resolve(comma >= 0 ? r.slice(comma + 1) : r);
    };
    reader.readAsDataURL(blob);
  });
}

async function canvasToThumb(canvas, quality) {
  const blob = await new Promise((res) => canvas.toBlob(res, "image/jpeg", quality || 0.85));
  if (!blob) return null;
  return { b64: await blobToBase64(blob), mime: "image/jpeg" };
}

// Brand placeholder tokens (canvas literals) — aligned to the first-party ElastOS cluster
// palette so the generated cover matches the rest of the product (Library / Marketplace are
// light). A soft white→lavender card, a periwinkle file-type chip, ink/muted text.
const PLACEHOLDER_BG = "#ffffff"; // --panel-strong (top of the gradient)
const PLACEHOLDER_BG2 = "#edf1fb"; // --bg (bottom of the gradient)
const PLACEHOLDER_BORDER = "rgba(83,103,164,0.22)"; // --line-strong
const PLACEHOLDER_CHIP_BG = "#e8edff"; // --accent-soft
const PLACEHOLDER_CHIP_TEXT = "#3c53a7"; // --accent-deep
const PLACEHOLDER_TITLE = "#1d2438"; // --ink
const PLACEHOLDER_SUBTLE = "#66708a"; // --muted

function roundRectPath(ctx, x, y, w, h, r) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

function drawCardBorder(ctx, w, h) {
  ctx.strokeStyle = PLACEHOLDER_BORDER;
  ctx.lineWidth = 1;
  ctx.strokeRect(0.5, 0.5, w - 1, h - 1);
}

async function thumbFromCustom(file) {
  const img = await createImageBitmap(file);
  const max = 1280;
  const scale = Math.min(max / img.width, max / img.height, 1);
  const c = document.createElement("canvas");
  c.width = Math.round(img.width * scale);
  c.height = Math.round(img.height * scale);
  c.getContext("2d").drawImage(img, 0, 0, c.width, c.height);
  return canvasToThumb(c, 0.85);
}

async function thumbFromImage(file) {
  // Low-res + slight blur + darken so it can't substitute for the real content.
  const img = await createImageBitmap(file);
  const max = 200;
  const scale = Math.min(max / img.width, max / img.height, 1);
  const c = document.createElement("canvas");
  c.width = Math.round(img.width * scale);
  c.height = Math.round(img.height * scale);
  const ctx = c.getContext("2d");
  ctx.filter = "blur(1px)";
  ctx.drawImage(img, 0, 0, c.width, c.height);
  ctx.filter = "none";
  ctx.fillStyle = "rgba(0,0,0,0.08)";
  ctx.fillRect(0, 0, c.width, c.height);
  return canvasToThumb(c, 0.6);
}

function thumbFromVideo(file) {
  return new Promise((resolve) => {
    const v = document.createElement("video");
    v.preload = "auto";
    v.muted = true;
    v.playsInline = true;
    const url = URL.createObjectURL(file);
    v.src = url;
    let done = false;
    const finish = (val) => {
      if (done) return;
      done = true;
      URL.revokeObjectURL(url);
      v.src = "";
      resolve(val);
    };
    v.addEventListener("seeked", async () => {
      const vw = v.videoWidth || 640;
      const vh = v.videoHeight || 360;
      const scale = Math.min(640 / vw, 640 / vh, 1);
      const c = document.createElement("canvas");
      c.width = Math.round(vw * scale);
      c.height = Math.round(vh * scale);
      c.getContext("2d").drawImage(v, 0, 0, c.width, c.height);
      finish(await canvasToThumb(c, 0.85));
    }, { once: true });
    v.addEventListener("loadeddata", () => {
      v.currentTime = Math.min(2, (v.duration || 20) * 0.1);
    }, { once: true });
    v.addEventListener("error", () => finish(null), { once: true });
    setTimeout(() => finish(null), 15000);
    v.load();
  });
}

async function thumbFromText(file) {
  // Teaser: first lines with a fade-out gradient so the bottom is unreadable.
  const text = await file.text();
  const lines = text.substring(0, 800).split("\n").slice(0, 12);
  const c = document.createElement("canvas");
  c.width = 400;
  c.height = 300;
  const ctx = c.getContext("2d");
  ctx.fillStyle = "#ffffff";
  ctx.fillRect(0, 0, 400, 300);
  ctx.fillStyle = "#1d2438";
  ctx.font = "13px monospace";
  ctx.textBaseline = "top";
  let y = 16;
  for (const line of lines) {
    if (y + 16 > 284) break;
    let s = line;
    while (ctx.measureText(s + "...").width > 368 && s.length > 0) s = s.slice(0, -1);
    if (s !== line) s += "...";
    ctx.fillText(s, 16, y);
    y += 16;
  }
  const g = ctx.createLinearGradient(0, 120, 0, 300);
  g.addColorStop(0, "rgba(255,255,255,0)");
  g.addColorStop(0.6, "rgba(255,255,255,0.85)");
  g.addColorStop(1, "rgba(255,255,255,1)");
  ctx.fillStyle = g;
  ctx.fillRect(0, 120, 400, 180);
  return canvasToThumb(c, 0.8);
}

async function thumbGeneric(file, mime) {
  // Brand placeholder card: graphite surface + soft border, a muted file-type chip (e.g. "CBZ",
  // "STL") and the name/mime in muted white. No purple, no accent — matches every other surface.
  const c = document.createElement("canvas");
  c.width = 640;
  c.height = 360;
  const ctx = c.getContext("2d");
  const grad = ctx.createLinearGradient(0, 0, 0, 360);
  grad.addColorStop(0, PLACEHOLDER_BG);
  grad.addColorStop(1, PLACEHOLDER_BG2);
  ctx.fillStyle = grad;
  ctx.fillRect(0, 0, 640, 360);
  // File-type chip on a muted surface.
  const badge = (file.name.split(".").pop() || "?").toUpperCase().slice(0, 5);
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.font = "bold 40px sans-serif";
  const chipW = Math.max(ctx.measureText(badge).width + 56, 120);
  const chipH = 84;
  roundRectPath(ctx, 320 - chipW / 2, 150 - chipH / 2, chipW, chipH, 16);
  ctx.fillStyle = PLACEHOLDER_CHIP_BG;
  ctx.fill();
  ctx.fillStyle = PLACEHOLDER_CHIP_TEXT;
  ctx.fillText(badge, 320, 152);
  // Name + mime.
  ctx.fillStyle = PLACEHOLDER_TITLE;
  ctx.font = "16px sans-serif";
  ctx.fillText((file.name || "").substring(0, 44), 320, 244);
  ctx.fillStyle = PLACEHOLDER_SUBTLE;
  ctx.font = "13px sans-serif";
  ctx.fillText(mime || "unknown", 320, 270);
  drawCardBorder(ctx, 640, 360);
  return canvasToThumb(c, 0.85);
}

// The file-derived cover cascade (ignores any custom upload): a blurred still for images, a
// frame for video, a teaser card for text, a branded type card otherwise. Used both to show a
// real cover in the live preview and as the mint-time fallback when no custom cover is chosen.
async function generateAutoThumbnail(file, mime) {
  if (mime.startsWith("image/")) return await thumbFromImage(file);
  if (mime.startsWith("video/")) {
    const t = await thumbFromVideo(file);
    if (t) return t;
  } else if (mime === "text/plain" || mime.startsWith("text/")) {
    const t = await thumbFromText(file);
    if (t) return t;
  }
  return await thumbGeneric(file, mime);
}

// Produce { b64, mime } for the cover, or null. Never throws — a thumbnail failure must not
// block the mint (the asset just lists with the type-icon placeholder).
async function generateThumbnail(file, mime) {
  try {
    if (customThumbnail) return await thumbFromCustom(customThumbnail);
    return await generateAutoThumbnail(file, mime);
  } catch (err) {
    try {
      return await thumbGeneric(file, mime);
    } catch (_e) {
      return null;
    }
  }
}

async function mint() {
  if (!selectedFile) return;
  if (!(await preflight())) return;

  // Legal attestation is required (PC2 parity) — the creator must affirm distribution rights.
  if (els.legalAttest && !els.legalAttest.checked) {
    setStatus("Please confirm you own or have the rights to distribute this content.", "err");
    return;
  }
  // For paid sales, the royalty rows (if any) must total 95% with the 5% protocol cut.
  if (isPaidMethod()) {
    const rows = collectRoyalties();
    if (rows.length) {
      const sum = rows.reduce((a, r) => a + r.royalty, 0);
      if (Math.abs(sum + ELACITY_ROYALTY_PERCENT - 100) > 0.01) {
        setStatus(
          "Royalty payees must total " +
            (100 - ELACITY_ROYALTY_PERCENT) +
            "% (the protocol takes " +
            ELACITY_ROYALTY_PERCENT +
            "%).",
          "err"
        );
        return;
      }
    }
  }

  els.mint.disabled = true;
  resetSteps();
  setStatus("Encrypting…", null);

  const mime = resolveMime(selectedFile);
  setMediaStepsVisible(classifyMedia(mime));
  // Reveal the overall smooth bar and open the step panel now that the mint is running.
  showOverall(true);
  const stepsPanel = document.getElementById("steps-panel");
  if (stepsPanel) stepsPanel.open = true;
  // For media, the server drives analyze→package→… via the progress poll; for non-media the
  // first real stage is encrypt. Start the engine on the first stage either way.
  setStep(classifyMedia(mime) ? "analyze" : "encrypt", "active");
  // Access-token supply: how many editions/holders can be granted access. Clamp to >=1;
  // the host falls back to its default when unset.
  const copies = Math.max(1, parseInt(els.copies && els.copies.value, 10) || 0) || undefined;

  // Derive the public cover thumbnail (degraded teaser) from bytes we already hold.
  const thumb = await generateThumbnail(selectedFile, mime);

  // Free-preview length for media (capped at 60s host-side); 0 disables it.
  let previewDuration = 0;
  if (
    classifyMedia(mime) &&
    els.previewEnabled &&
    els.previewEnabled.checked &&
    els.previewDuration
  ) {
    previewDuration = Math.min(60, parseInt(els.previewDuration.value, 10) || 0);
  }

  // PC2 royalty/licensing terms. Resale royalty (RRL) is stored as deci-percent (90% => 900);
  // the host re-derives RRL-Percent = reseller_cut / 10.
  const royalties = isPaidMethod() ? collectRoyalties() : [];
  const resellerCut =
    accessMethod === "buy_and_resell"
      ? Math.round((parseFloat(els.resellerCut && els.resellerCut.value) || 0) * 10)
      : undefined;
  const aiTraining = els.aiLicensing && els.aiLicensing.checked;
  const licensing = aiTraining
    ? {
        type: "training-rights",
        terms: { commercial: true, modification: false, redistribution: false, attribution: true, exclusivity: false },
        aiTraining: {
          permitted: true,
          scope: "commercial",
          modelTypes: ["llm", "vision", "audio", "code", "multimodal", "diffusion", "embedding"],
          attribution: true,
          derivativeWorks: false,
          outputOwnership: "licensee",
        },
      }
    : {
        type: "perpetual",
        terms: { commercial: true, modification: false, redistribution: false, attribution: true, exclusivity: false },
      };
  const legalAttestation =
    els.legalAttest && els.legalAttest.checked
      ? { owns: true, attestedAt: new Date().toISOString(), attestedBy: els.wallet.value }
      : null;

  const meta = {
    title: els.title.value.trim(),
    description: els.desc.value.trim(),
    price: isPaidMethod() ? els.price.value || "0" : "0",
    currency: els.currency.value,
    channel: selectedChannel(),
    creatorAddress: els.wallet.value,
    mime: mime,
    isMedia: classifyMedia(mime),
    fileName: selectedFile.name,
    copies: copies,
    category: (els.category && els.category.value.trim()) || "",
    thumbnailB64: (thumb && thumb.b64) || "",
    thumbnailMime: (thumb && thumb.mime) || "",
    previewDuration: previewDuration,
    accessMethod: accessMethod,
    resellerCut: resellerCut,
    royalties: royalties,
    licensing: licensing,
    legalAttestation: legalAttestation,
    isAdult: !!(els.adultFlag && els.adultFlag.checked),
  };

  // The frame ships the bytes + listing terms to the host capability route. The
  // host runs the spine (encrypt-provider escrow -> content publish -> publish-provider
  // prepare). The raw CEK is minted and Shamir-split INSIDE encrypt-provider; only
  // sealed shares + the dKMS protections block ever exist outside that boundary. The
  // host returns the UNSIGNED mint — YOU complete it by signing in your wallet.
  let fileB64;
  try {
    fileB64 = await fileToBase64(selectedFile);
  } catch (err) {
    setStep("encrypt", "err");
    setStatus("Could not read file: " + err.message, "err");
    els.mint.disabled = false;
    return;
  }

  // One job id per mint: the server keys live stage progress on it; we poll it for the
  // media pipeline panel (analyze → package → encrypt → publish → sign).
  const jobId =
    (window.crypto && window.crypto.randomUUID && window.crypto.randomUUID()) ||
    "job-" + Date.now() + "-" + Math.random().toString(16).slice(2);
  if (meta.isMedia) {
    startTechProgress(jobId);
  }

  try {
    const resp = await fetch(appUrl("/prepare-mint"), {
      method: "POST",
      headers: { ...launchHeaders(), "Content-Type": "application/json" },
      body: JSON.stringify({ file_b64: fileB64, meta: meta, job_id: jobId }),
    });
    const result = await resp.json().catch(() => ({}));
    assertNoKeyMaterial(result);
    stopTechProgress();

    if (!resp.ok) {
      const stage = result.stage || "encrypt";
      setStep(stage, "err");
      if (meta.isMedia) setTechStep(stage, "err");
      setStatus(result.error || "Prepare failed: " + resp.status, "err");
      els.mint.disabled = false;
      return;
    }
    if (meta.isMedia) {
      ["analyze", "package", "encrypt", "publish", "sign"].forEach((s) => setTechStep(s, "done"));
    }

    setStep("encrypt", "done");
    setStep("publish", "done");
    // The runtime prepared everything AND queued a wallet approval for the mint
    // transaction. The user completes it in the Wallet app (eth_sendTransaction),
    // so the OWNER is msg.sender / the on-chain creator. The runtime never signs.
    const id = result.content_id || result.kid || "";
    // Pin every subsequent confirm/approve check to THIS asset's KID.
    currentMintContentId = id;
    const approval = result.mint_approval || {};
    // Remember the mint's approval id so the confirmation poll can resolve its broadcast tx hash
    // and confirm via the receipt (fast) instead of a log scan.
    currentMintRequestId = approval.request_id || "";
    if (approval.request_id) {
      setStep("sign", "done");
      setStep("broadcast", "active");
      setStatus(
        "Mint prepared" + (id ? " — contentId " + id.substring(0, 12) + "…" : "") +
          ". Open the Wallet app and approve the mint transaction to sign &amp; broadcast it from your wallet.",
        "ok"
      );
      // PC2's 2nd mint tx: the owner approves the gateway only AFTER the mint confirms on-chain.
      // Show Step 2 but keep it disabled until the chain confirms tx1 — the watcher below lifts
      // the gate (and promotes the button) the moment the mint's `AssetCreated` event lands.
      gateTradeUntilConfirmed();
      // The mint is on its way — let the creator start the next asset right away (Step 2 for this
      // one stays available and is confirmation-gated independently).
      if (els.mintAnother) els.mintAnother.hidden = false;
      // Poll the chain (read-only) so the Broadcast step advances to "done" the moment the mint
      // lands on-chain, and Step 2 is visibly promoted — instead of a dead spinner that never
      // updates after you approve the tx in the Wallet app. Pinned to THIS asset's KID.
      watchMintConfirmation(meta.channel, meta.creatorAddress, id);
    } else {
      setStep("sign", "active");
      setStatus(
        "Prepared" + (id ? " — contentId " + id.substring(0, 12) + "…" : "") +
          " · no wallet approval was queued — connect your wallet on Base and retry.",
        "err"
      );
    }
    els.mint.disabled = false;
  } catch (err) {
    stopTechProgress();
    setStatus("Prepare failed: " + err.message, "err");
    els.mint.disabled = false;
  }
}

function fileToBase64(file) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error("read error"));
    reader.onload = () => {
      const result = reader.result || "";
      const comma = result.indexOf(",");
      resolve(comma >= 0 ? result.slice(comma + 1) : result);
    };
    reader.readAsDataURL(file);
  });
}

// PC2's 2nd mint tx (`setApprovalForAll(gateway, true)` on the asset's operative). The host
// discovers the operative from the mint's on-chain `AssetCreated` event — so this is
// confirmation-gated: if the mint hasn't landed yet the host returns `mint_not_confirmed`
// and we ask the owner to confirm the mint first, then retry.
async function enableTrading() {
  const channel = selectedChannel();
  const creatorAddress = els.wallet.value;
  if (!channel || !creatorAddress) return;
  els.enableTrading.disabled = true;
  setStep("approve", "active");
  setStatus("Checking the mint is confirmed on-chain…", "");
  try {
    const resp = await fetch(appUrl("/prepare-trade-approval"), {
      method: "POST",
      headers: { ...launchHeaders(), "Content-Type": "application/json" },
      // Pin the approval to THIS asset's operative (its own contract) — never the channel's
      // newest mint — so each asset gets its own gateway approval (PC2's per-asset 2nd tx).
      // requestId lets the host resolve the operative from the mint receipt (fast) vs a log scan.
      body: JSON.stringify({ channel: channel, creatorAddress: creatorAddress, contentId: currentMintContentId || undefined, requestId: currentMintRequestId || undefined }),
    });
    const result = await resp.json().catch(() => ({}));
    if (!resp.ok) {
      const msg = result.error || "Could not prepare the gateway approval: " + resp.status;
      // Not yet mined — let the owner confirm the mint, then retry.
      if (/not_confirmed|not confirmed/i.test(msg)) {
        setStep("approve", "active");
        setStatus(
          "Mint isn't confirmed on-chain yet — approve & confirm the mint in the Wallet app, then click “Enable trading” again.",
          ""
        );
      } else {
        setStep("approve", "err");
        setStatus(msg, "err");
      }
      els.enableTrading.disabled = false;
      return;
    }
    if (result.already_approved) {
      setStep("approve", "done");
      completeProgress();
      setStatus("Gateway already approved — your asset is tradable.", "ok");
      els.enableTrading.hidden = true;
      if (els.enableTradingHint) els.enableTradingHint.hidden = true;
      return;
    }
    if (result.approval && result.approval.request_id) {
      // Keep the Approve step spinning while the owner signs the 2nd tx, then poll the chain
      // read-only and tick it green once `isApprovedForAll` flips true.
      setStep("approve", "active");
      setStatus(
        "Gateway approval prepared — open the Wallet app and approve the second transaction. This step ticks green automatically once it confirms on-chain.",
        "ok"
      );
      els.enableTrading.hidden = true;
      if (els.enableTradingHint) els.enableTradingHint.hidden = true;
      watchTradeApproval(channel, creatorAddress, currentMintContentId);
    } else {
      setStep("approve", "err");
      setStatus("No wallet approval was queued — connect your wallet on Base and retry.", "err");
      els.enableTrading.disabled = false;
    }
  } catch (err) {
    setStep("approve", "err");
    setStatus("Could not enable trading: " + err.message, "err");
    els.enableTrading.disabled = false;
  }
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// Promote "Enable trading" so the owner sees it's the next action once the mint lands.
function promoteEnableTrading() {
  if (!els.enableTrading) return;
  tradeGated = false; // mint confirmed — Step 2 is now actionable
  els.enableTrading.hidden = false;
  els.enableTrading.disabled = false;
  els.enableTrading.classList.add("is-ready");
  if (els.enableTradingHint) {
    els.enableTradingHint.hidden = false;
    els.enableTradingHint.textContent = DEFAULT_TRADE_HINT;
  }
}

// Read-only poll of the chain (no side effects server-side) so the "Broadcast" step advances
// to done the moment the mint lands on-chain — the same `AssetCreated` signal Step 2 is gated
// on — instead of a dead spinner. Cancelled when a new mint starts (token bump in resetSteps).
async function watchMintConfirmation(channel, creatorAddress, contentId) {
  if (!channel || !creatorAddress) return;
  const token = ++mintWatchToken;
  const started = Date.now();
  const maxMs = 10 * 60 * 1000; // a Base mint can take a while to confirm; keep watching
  let delay = 4000;
  while (mintWatchToken === token && Date.now() - started < maxMs) {
    await sleep(delay);
    if (mintWatchToken !== token) return; // superseded by a newer mint
    let confirmed = false;
    let alreadyApproved = false;
    try {
      const resp = await fetch(appUrl("/mint-status"), {
        method: "POST",
        headers: { ...launchHeaders(), "Content-Type": "application/json" },
        // Pin to THIS asset: the host only reports confirmed/already_approved for the mint
        // whose KID matches, so an earlier approved asset can't end the watch prematurely.
        // Omitted when unknown (falls back to the legacy newest-in-channel resolution).
        // requestId lets the host confirm via the broadcast tx receipt (fast) vs a log scan.
        body: JSON.stringify({ channel: channel, creatorAddress: creatorAddress, contentId: contentId || undefined, requestId: currentMintRequestId || undefined }),
      });
      const result = await resp.json().catch(() => ({}));
      if (resp.ok) {
        confirmed = Boolean(result.confirmed);
        alreadyApproved = Boolean(result.already_approved);
      }
    } catch (_err) {
      // Transient (RPC hiccup / offline) — keep polling.
    }
    if (mintWatchToken !== token) return;
    if (confirmed) {
      setStep("broadcast", "done");
      if (alreadyApproved) {
        setStep("approve", "done");
        completeProgress();
        setStatus("Mint confirmed on-chain ✓ — your asset is already tradable.", "ok");
        if (els.enableTrading) els.enableTrading.hidden = true;
        if (els.enableTradingHint) els.enableTradingHint.hidden = true;
      } else {
        setStatus(
          "Mint confirmed on-chain ✓ — click “Step 2 — Enable trading” to make it tradable.",
          "ok"
        );
        promoteEnableTrading();
      }
      return;
    }
    delay = Math.min(delay + 2000, 12000); // gentle backoff, capped
  }
}

// After the owner approves the gateway (2nd tx), poll the chain read-only until
// `isApprovedForAll` flips true, then tick the Approve step green — instead of leaving the
// spinner hanging after the wallet confirms. Cancelled if a new mint starts (token bump).
async function watchTradeApproval(channel, creatorAddress, contentId) {
  if (!channel || !creatorAddress) return;
  const token = ++mintWatchToken; // the mint is already confirmed here; supersede its watcher
  const started = Date.now();
  const maxMs = 10 * 60 * 1000;
  let delay = 4000;
  while (mintWatchToken === token && Date.now() - started < maxMs) {
    await sleep(delay);
    if (mintWatchToken !== token) return;
    let approved = false;
    try {
      const resp = await fetch(appUrl("/mint-status"), {
        method: "POST",
        headers: { ...launchHeaders(), "Content-Type": "application/json" },
        // Pinned to THIS asset's operative: isApprovedForAll is read on the just-minted contract.
        body: JSON.stringify({ channel: channel, creatorAddress: creatorAddress, contentId: contentId || undefined, requestId: currentMintRequestId || undefined }),
      });
      const result = await resp.json().catch(() => ({}));
      if (resp.ok) approved = Boolean(result.already_approved);
    } catch (_err) {
      // Transient — keep polling.
    }
    if (mintWatchToken !== token) return;
    if (approved) {
      setStep("approve", "done");
      completeProgress();
      setStatus("Gateway approved ✓ — your asset is now tradable.", "ok");
      if (els.enableTrading) els.enableTrading.hidden = true;
      if (els.enableTradingHint) els.enableTradingHint.hidden = true;
      return;
    }
    delay = Math.min(delay + 2000, 12000);
  }
}

// ── live listing preview ─────────────────────────────────────────────────────
// A presentational mirror of how the asset will appear once listed. Reads the same
// form state the mint payload uses — it holds no authority and triggers no network.
const preview = {
  img: document.getElementById("lp-cover-img"),
  ph: document.getElementById("lp-cover-ph"),
  type: document.getElementById("lp-cover-type"),
  title: document.getElementById("lp-title"),
  price: document.getElementById("lp-price"),
  method: document.getElementById("lp-method"),
  badges: document.getElementById("lp-badges"),
};
let previewCoverUrl = "";
// Data URL of the auto-generated cover (frame/blur/waveform/type-card) derived from the file.
let autoCoverUrl = "";
const METHOD_LABELS = { free: "Free", buy_once: "Buy now", buy_and_resell: "Buy & resell" };

function prettyCategory(value) {
  return (value || "").replace(/-/g, " ");
}

// Point the preview cover at a source, revoking any prior object URL. `isObjectUrl` marks blob
// URLs (which must be revoked); data URLs (the auto cover) are passed with isObjectUrl=false.
function setPreviewCover(src, isObjectUrl) {
  if (previewCoverUrl) {
    URL.revokeObjectURL(previewCoverUrl);
    previewCoverUrl = "";
  }
  if (isObjectUrl) previewCoverUrl = src;
  preview.img.src = src;
  preview.img.classList.remove("hidden");
  preview.ph.classList.add("hidden");
}

function renderPreview() {
  if (!preview.title) return;
  preview.title.textContent = (els.title && els.title.value.trim()) || "Untitled asset";
  if (preview.method) preview.method.textContent = METHOD_LABELS[accessMethod] || "Free";
  if (preview.price) {
    if (isPaidMethod()) {
      const amount = (els.price && els.price.value) || "0";
      const currency = (els.currency && els.currency.value) || "USDC";
      preview.price.textContent = (Number(amount) > 0 ? amount : "0") + " " + currency;
    } else {
      preview.price.textContent = "Free";
    }
  }
  // Cover priority: a custom upload wins; otherwise an image file shows directly; otherwise the
  // auto-generated cover (frame/blur/waveform); otherwise a branded type chip while none exists.
  const isImageSource =
    selectedFile && resolveMime(selectedFile).startsWith("image/") ? selectedFile : null;
  const coverFile = customThumbnail || isImageSource;
  if (coverFile && preview.img && preview.ph) {
    setPreviewCover(URL.createObjectURL(coverFile), true);
  } else if (autoCoverUrl && preview.img && preview.ph) {
    setPreviewCover(autoCoverUrl, false);
  } else if (preview.img && preview.ph) {
    if (previewCoverUrl) {
      URL.revokeObjectURL(previewCoverUrl);
      previewCoverUrl = "";
    }
    preview.img.classList.add("hidden");
    preview.ph.classList.remove("hidden");
    if (preview.type) {
      const category = (els.category && els.category.value) || "";
      if (category) preview.type.textContent = prettyCategory(category);
      else if (selectedFile)
        preview.type.textContent = (selectedFile.name.split(".").pop() || "file").toUpperCase();
      else preview.type.textContent = "Asset";
    }
  }
  if (preview.badges) {
    preview.badges.innerHTML = "";
    const chips = [];
    const category = (els.category && els.category.value) || "";
    if (category) chips.push({ text: prettyCategory(category), warn: false });
    if (accessMethod === "buy_and_resell") chips.push({ text: "Resale royalty", warn: false });
    if (els.aiLicensing && els.aiLicensing.checked) chips.push({ text: "AI training", warn: false });
    if (els.adultFlag && els.adultFlag.checked) chips.push({ text: "18+", warn: true });
    chips.forEach((chip) => {
      const span = document.createElement("span");
      span.className = "lp-chip" + (chip.warn ? " warn" : "");
      span.textContent = chip.text;
      preview.badges.appendChild(span);
    });
  }
}

// ── wiring ────────────────────────────────────────────────────────────────────
els.drop.addEventListener("click", () => els.file.click());
els.drop.addEventListener("keydown", (e) => {
  if (e.key === "Enter" || e.key === " ") els.file.click();
});
els.file.addEventListener("change", (e) => onFile(e.target.files && e.target.files[0]));
els.drop.addEventListener("dragover", (e) => {
  e.preventDefault();
  els.drop.classList.add("over");
});
els.drop.addEventListener("dragleave", () => els.drop.classList.remove("over"));
els.drop.addEventListener("drop", (e) => {
  e.preventDefault();
  els.drop.classList.remove("over");
  onFile(e.dataTransfer.files && e.dataTransfer.files[0]);
});
els.title.addEventListener("input", () => {
  const titleErr = document.getElementById("title-err");
  if (titleErr && els.title.value.trim()) titleErr.textContent = "";
  refreshMintEnabled();
});
els.title.addEventListener("blur", () => {
  const titleErr = document.getElementById("title-err");
  if (titleErr) titleErr.textContent = els.title.value.trim() ? "" : "A title is required.";
});
// Keep the live preview in sync with monetization + detail changes.
if (els.price) els.price.addEventListener("input", renderPreview);
if (els.currency) els.currency.addEventListener("change", renderPreview);
if (els.category) els.category.addEventListener("change", renderPreview);
if (els.adultFlag) els.adultFlag.addEventListener("change", renderPreview);
if (els.aiLicensing) els.aiLicensing.addEventListener("change", renderPreview);
els.wallet.addEventListener("change", () => loadChannels());
els.channel.addEventListener("change", onChannelChange);
els.channelManualInput.addEventListener("input", () => {
  const v = els.channelManualInput.value.trim();
  els.channelManualHint.textContent = v && !isEvmAddress(v)
    ? "That doesn't look like a 0x… contract address."
    : "Paste your channel's contract address. It's verified on-chain (must be created by your wallet) before minting.";
  refreshMintEnabled();
});
// Custom cover thumbnail (optional). The chosen image overrides the auto-generated teaser.
function setCustomThumbnail(file) {
  if (!file || !file.type.startsWith("image/")) return;
  customThumbnail = file;
  if (els.thumbPreviewImg) els.thumbPreviewImg.src = URL.createObjectURL(file);
  if (els.thumbPreviewWrap) els.thumbPreviewWrap.classList.remove("hidden");
  if (els.thumbDrop) els.thumbDrop.classList.add("hidden");
  renderPreview();
}
if (els.thumbDrop) {
  els.thumbDrop.addEventListener("click", () => els.thumbInput && els.thumbInput.click());
  els.thumbDrop.addEventListener("dragover", (e) => {
    e.preventDefault();
    els.thumbDrop.classList.add("over");
  });
  els.thumbDrop.addEventListener("dragleave", () => els.thumbDrop.classList.remove("over"));
  els.thumbDrop.addEventListener("drop", (e) => {
    e.preventDefault();
    els.thumbDrop.classList.remove("over");
    setCustomThumbnail(e.dataTransfer.files && e.dataTransfer.files[0]);
  });
}
if (els.thumbInput) {
  els.thumbInput.addEventListener("change", (e) =>
    setCustomThumbnail(e.target.files && e.target.files[0]),
  );
}
if (els.thumbRemove) {
  els.thumbRemove.addEventListener("click", () => {
    customThumbnail = null;
    if (els.thumbInput) els.thumbInput.value = "";
    if (els.thumbPreviewWrap) els.thumbPreviewWrap.classList.add("hidden");
    if (els.thumbDrop) els.thumbDrop.classList.remove("hidden");
    renderPreview();
  });
}
if (els.previewEnabled) {
  els.previewEnabled.addEventListener("change", () => {
    if (els.previewControls)
      els.previewControls.classList.toggle("hidden", !els.previewEnabled.checked);
  });
}
if (els.previewDuration) {
  els.previewDuration.addEventListener("input", () => {
    if (els.previewDurationDisplay)
      els.previewDurationDisplay.textContent = els.previewDuration.value + "s";
  });
}

// ---- Access method + royalty split (PC2 parity) -----------------------------

const ELACITY_ROYALTY_PERCENT = 5; // protocol cut; creator rows own the remaining 95%

function isPaidMethod() {
  return accessMethod === "buy_once" || accessMethod === "buy_and_resell";
}

function syncMethodUI() {
  if (els.methodGrid) {
    els.methodGrid.querySelectorAll(".method").forEach((card) => {
      const selected = card.dataset.method === accessMethod;
      card.classList.toggle("sel", selected);
      card.setAttribute("aria-pressed", selected ? "true" : "false");
    });
  }
  if (els.priceRow) els.priceRow.classList.toggle("hidden", !isPaidMethod());
  // Resale royalty only applies to buy_and_resell; royalty split only to paid sales.
  if (els.resellField)
    els.resellField.classList.toggle("hidden", accessMethod !== "buy_and_resell");
  if (els.royaltyField)
    els.royaltyField.classList.toggle("hidden", !isPaidMethod());
  renderPreview();
}

function addRoyaltyRow(address, percent) {
  if (!els.royaltyRows) return;
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
  pct.max = "95";
  pct.step = "0.1";
  pct.placeholder = "%";
  pct.value = percent != null ? String(percent) : "";
  const del = document.createElement("button");
  del.type = "button";
  del.className = "ry-del";
  del.textContent = "\u00d7";
  del.title = "Remove payee";
  del.addEventListener("click", () => {
    row.remove();
    refreshRoyaltyTotal();
  });
  addr.addEventListener("input", refreshRoyaltyTotal);
  pct.addEventListener("input", refreshRoyaltyTotal);
  row.appendChild(addr);
  row.appendChild(pct);
  row.appendChild(del);
  els.royaltyRows.appendChild(row);
  refreshRoyaltyTotal();
}

function collectRoyalties() {
  const rows = [];
  if (!els.royaltyRows) return rows;
  els.royaltyRows.querySelectorAll(".royalty-row").forEach((row) => {
    const address = (row.querySelector(".ry-addr").value || "").trim();
    const royalty = parseFloat(row.querySelector(".ry-pct").value) || 0;
    if (address || royalty) rows.push({ address: address, royalty: royalty });
  });
  return rows;
}

function refreshRoyaltyTotal() {
  if (!els.royaltyTotal) return;
  const rows = collectRoyalties();
  if (!rows.length) {
    els.royaltyTotal.textContent = "";
    els.royaltyTotal.className = "royalty-total";
    return;
  }
  const sum = rows.reduce((a, r) => a + r.royalty, 0);
  const target = 100 - ELACITY_ROYALTY_PERCENT;
  const ok = Math.abs(sum - target) < 0.01;
  els.royaltyTotal.textContent =
    "Payees: " +
    sum.toFixed(1) +
    "% + protocol " +
    ELACITY_ROYALTY_PERCENT +
    "% = " +
    (sum + ELACITY_ROYALTY_PERCENT).toFixed(1) +
    "% " +
    (ok ? "\u2713" : "(must total 100%)");
  els.royaltyTotal.className = "royalty-total " + (ok ? "ok" : "bad");
}

if (els.methodGrid) {
  els.methodGrid.querySelectorAll(".method").forEach((card) => {
    const pick = () => {
      accessMethod = card.dataset.method;
      syncMethodUI();
    };
    card.addEventListener("click", pick);
    card.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        pick();
      }
    });
  });
}
if (els.royaltyAdd) {
  els.royaltyAdd.addEventListener("click", () => addRoyaltyRow("", ""));
}
syncMethodUI();

// Initial roadmap: hide the media-only rows until a media file is chosen, so the default panel
// shows the generic path (encrypt → publish → sign → broadcast → enable trading).
setMediaStepsVisible(false);

els.createChannelBtn.addEventListener("click", createChannel);
els.mint.addEventListener("click", mint);
if (els.enableTrading) els.enableTrading.addEventListener("click", enableTrading);
if (els.mintAnother) els.mintAnother.addEventListener("click", resetForMintAnother);

preflight().then((ok) => {
  if (ok) loadWallets();
});
