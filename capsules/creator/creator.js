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
  channel: document.getElementById("channel"),
  mint: document.getElementById("mint"),
  steps: document.getElementById("steps"),
  status: document.getElementById("status"),
};

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
  ["encrypt", "publish", "sign", "broadcast"].forEach((s) => setStep(s, ""));
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

function refreshMintEnabled() {
  els.mint.disabled = !(selectedFile && els.title.value.trim());
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
    channel: els.channel.value.trim(),
    mime: mime,
    isMedia: classifyMedia(mime),
    size: selectedFile.size,
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
els.mint.addEventListener("click", mint);

preflight();
