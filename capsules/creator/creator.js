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

const els = {
  drop: document.getElementById("drop"),
  dropTitle: document.getElementById("drop-title"),
  dropMeta: document.getElementById("drop-meta"),
  file: document.getElementById("file"),
  title: document.getElementById("title"),
  desc: document.getElementById("desc"),
  price: document.getElementById("price"),
  currency: document.getElementById("currency"),
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
};

// The sentinel channel option that reveals the inline create-channel form.
const CREATE_CHANNEL_VALUE = "__create__";
// The sentinel that reveals the manual channel-address input (fail-closed fallback for a
// channel discovery hasn't surfaced yet — still verified on-chain server-side before mint).
const MANUAL_CHANNEL_VALUE = "__manual__";
// Re-poll cadence while the on-chain channel index is still backfilling older channels.
const CHANNEL_POLL_MS = 2500;
let channelPollTimer = null;

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

function setStep(name, state) {
  const li = els.steps.querySelector('li[data-step="' + name + '"]');
  if (li) li.className = state || "";
}

function resetSteps() {
  ["encrypt", "publish", "sign", "broadcast", "approve"].forEach((s) => setStep(s, ""));
  if (els.enableTrading) els.enableTrading.disabled = false;
  // Button visibility is governed by the wallet+channel selection (refreshTradeEnabled),
  // so it stays available for the latest minted asset; only re-enable it here.
  refreshTradeEnabled();
}

function classifyMedia(mime) {
  return MEDIA_PREFIXES.some((p) => (mime || "").startsWith(p));
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
  const mime = file.type || "application/octet-stream";
  const kind = classifyMedia(mime) ? "media" : "object";
  els.dropTitle.textContent = file.name;
  els.dropMeta.innerHTML =
    humanSize(file.size) + " &middot; " + mime + ' <span class="badge">' + kind + "</span>";
  if (!els.title.value) els.title.value = file.name.replace(/\.[^.]+$/, "");
  refreshMintEnabled();
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

function refreshMintEnabled() {
  // Fail-closed: a wallet AND a real channel selection are required — no silent default.
  els.mint.disabled = !(
    selectedFile &&
    els.title.value.trim() &&
    els.wallet.value &&
    selectedChannel()
  );
  refreshTradeEnabled();
}

// The trade-enabling 2nd tx targets the newest minted asset in the selected channel, so the
// "Enable trading" action is available whenever a wallet + a real channel are chosen — both
// right after a mint AND for an asset minted earlier. It's confirmation-gated server-side.
function refreshTradeEnabled() {
  if (!els.enableTrading) return;
  const ready = Boolean(els.wallet.value && selectedChannel());
  els.enableTrading.hidden = !ready;
  if (els.enableTradingHint) els.enableTradingHint.hidden = !ready;
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

async function mint() {
  if (!selectedFile) return;
  if (!(await preflight())) return;

  els.mint.disabled = true;
  resetSteps();
  setStatus("Encrypting…", null);
  setStep("encrypt", "active");

  const mime = selectedFile.type || "application/octet-stream";
  const meta = {
    title: els.title.value.trim(),
    description: els.desc.value.trim(),
    price: els.price.value || "0",
    currency: els.currency.value,
    channel: selectedChannel(),
    creatorAddress: els.wallet.value,
    mime: mime,
    isMedia: classifyMedia(mime),
    fileName: selectedFile.name,
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

  try {
    const resp = await fetch(appUrl("/prepare-mint"), {
      method: "POST",
      headers: { ...launchHeaders(), "Content-Type": "application/json" },
      body: JSON.stringify({ file_b64: fileB64, meta: meta }),
    });
    const result = await resp.json().catch(() => ({}));
    assertNoKeyMaterial(result);

    if (!resp.ok) {
      const stage = result.stage || "encrypt";
      setStep(stage, "err");
      setStatus(result.error || "Prepare failed: " + resp.status, "err");
      els.mint.disabled = false;
      return;
    }

    setStep("encrypt", "done");
    setStep("publish", "done");
    // The runtime prepared everything AND queued a wallet approval for the mint
    // transaction. The user completes it in the Wallet app (eth_sendTransaction),
    // so the OWNER is msg.sender / the on-chain creator. The runtime never signs.
    const id = result.content_id || result.kid || "";
    const approval = result.mint_approval || {};
    if (approval.request_id) {
      setStep("sign", "done");
      setStep("broadcast", "active");
      setStatus(
        "Mint prepared" + (id ? " — contentId " + id.substring(0, 12) + "…" : "") +
          ". Open the Wallet app and approve the mint transaction to sign &amp; broadcast it from your wallet.",
        "ok"
      );
      // PC2's 2nd mint tx: once the mint confirms on-chain, the owner approves the gateway so
      // the asset is tradable. The "Enable trading" action (shown whenever a wallet + channel
      // are selected) is confirmation-gated server-side — it discovers the operative from the
      // mint's on-chain `AssetCreated` event, so it works once the mint lands.
      refreshTradeEnabled();
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
      body: JSON.stringify({ channel: channel, creatorAddress: creatorAddress }),
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
      setStatus("Gateway already approved — your asset is tradable.", "ok");
      els.enableTrading.hidden = true;
      if (els.enableTradingHint) els.enableTradingHint.hidden = true;
      return;
    }
    if (result.approval && result.approval.request_id) {
      setStatus(
        "Gateway approval prepared — open the Wallet app and approve the second transaction to make your asset tradable.",
        "ok"
      );
      els.enableTrading.hidden = true;
      if (els.enableTradingHint) els.enableTradingHint.hidden = true;
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
els.title.addEventListener("input", refreshMintEnabled);
els.wallet.addEventListener("change", () => loadChannels());
els.channel.addEventListener("change", onChannelChange);
els.channelManualInput.addEventListener("input", () => {
  const v = els.channelManualInput.value.trim();
  els.channelManualHint.textContent = v && !isEvmAddress(v)
    ? "That doesn't look like a 0x… contract address."
    : "Paste your channel's contract address. It's verified on-chain (must be created by your wallet) before minting.";
  refreshMintEnabled();
});
els.createChannelBtn.addEventListener("click", createChannel);
els.mint.addEventListener("click", mint);
if (els.enableTrading) els.enableTrading.addEventListener("click", enableTrading);

preflight().then((ok) => {
  if (ok) loadWallets();
});
