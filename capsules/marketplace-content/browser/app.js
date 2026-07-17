/* app.js — marketplace-content shell. Pure UI: renders, requests UNSIGNED orders, routes signing
 * to the wallet. Holds no signer/token/CEK (Principle 16). Runs standalone via api.js mock fallback. */
(function () {
  const $ = (s, r = document) => r.querySelector(s);
  const view = $("#view"), modalRoot = $("#modal-root");
  const state = { kind: null, op: null, q: "", category: null };

  // Inline feather/lucide-style stroke icons (currentColor, stroke-width 2, viewBox 24) — same convention as
  // the sibling capsules/marketplace/browser/marketplace.js icons={}. No emoji (per-OS, off-palette, reads as
  // a prototype), no scratch art, no new pack. icon() returns an aria-hidden SVG that inherits text color.
  const ICONS = {
    compass: '<circle cx="12" cy="12" r="10"/><polygon points="16.24 7.76 14.12 14.12 7.76 16.24 9.88 9.88"/>',
    key: '<circle cx="7.5" cy="15.5" r="5.5"/><path d="m21 2-9.6 9.6"/><path d="m15.5 7.5 3 3L22 7l-3-3"/>',
    bell: '<path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9"/><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0"/>',
    clapperboard: '<path d="M20.2 6 3 11l-.9-2.4c-.3-1.1.3-2.2 1.3-2.5l13.5-4c1.1-.3 2.2.3 2.5 1.3Z"/><path d="m6.2 5.3 3.1 3.9"/><path d="m12.4 3.4 3.1 4"/><path d="M3 11h18v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/>',
    package: '<path d="m7.5 4.27 9 5.15"/><path d="M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z"/><path d="m3.3 7 8.7 5 8.7-5"/><path d="M12 22V12"/>',
    gamepad: '<line x1="6" x2="10" y1="11" y2="11"/><line x1="8" x2="8" y1="9" y2="13"/><line x1="15" x2="15.01" y1="12" y2="12"/><line x1="18" x2="18.01" y1="10" y2="10"/><path d="M17.32 5H6.68a4 4 0 0 0-3.978 3.59c-.006.052-.01.101-.017.152C2.604 9.416 2 14.456 2 16a3 3 0 0 0 3 3c1 0 1.5-.5 2-1l1.414-1.414A2 2 0 0 1 9.828 16h4.344a2 2 0 0 1 1.414.586L17 18c.5.5 1 1 2 1a3 3 0 0 0 3-3c0-1.544-.604-6.584-.685-7.258A4 4 0 0 0 17.32 5z"/>',
    tv: '<path d="m10 7 5 3-5 3Z"/><rect width="20" height="14" x="2" y="3" rx="2"/><path d="M12 17v4"/><path d="M8 21h8"/>',
    headphones: '<path d="M3 14h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-5a9 9 0 0 1 18 0v5a2 2 0 0 1-2 2h-1a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2h3"/>',
    book: '<path d="M12 7v14"/><path d="M3 18a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h5a4 4 0 0 1 4 4 4 4 0 0 1 4-4h5a1 1 0 0 1 1 1v13a1 1 0 0 1-1 1h-6a3 3 0 0 0-3 3 3 3 0 0 0-3-3z"/>',
    image: '<rect width="18" height="18" x="3" y="3" rx="2" ry="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"/>',
    box: '<path d="M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z"/><path d="m3.3 7 8.7 5 8.7-5"/><path d="M12 22V12"/>',
    unlock: '<rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 9.9-1"/>',
    cart: '<circle cx="8" cy="21" r="1"/><circle cx="19" cy="21" r="1"/><path d="M2.05 2.05h2l2.66 12.42a2 2 0 0 0 2 1.58h9.78a2 2 0 0 0 1.95-1.57l1.65-7.43H5.12"/>',
    recycle: '<path d="m17 2 4 4-4 4"/><path d="M3 11v-1a4 4 0 0 1 4-4h14"/><path d="m7 22-4-4 4-4"/><path d="M21 13v1a4 4 0 0 1-4 4H3"/>',
    link: '<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>',
    sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/>',
    moon: '<path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"/>',
    play: '<polygon points="6 3 20 12 6 21 6 3"/>',
    layers: '<path d="m12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83Z"/><path d="m22 17.65-9.17 4.16a2 2 0 0 1-1.66 0L2 17.65"/><path d="m22 12.65-9.17 4.16a2 2 0 0 1-1.66 0L2 12.65"/>',
    bookOpen: '<path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"/><path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"/>',
    fileText: '<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M10 9H8"/><path d="M16 13H8"/><path d="M16 17H8"/>',
    download: '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" x2="12" y1="15" y2="3"/>',
    folder: '<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>',
  };
  const icon = (name, cls) => `<svg class="ico${cls ? " " + cls : ""}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICONS[name] || ""}</svg>`;
  const MEDIA = { watch: "tv", listen: "headphones", read: "book", view: "image", explore: "box" };
  const mediaIcon = (m, cls) => icon(MEDIA[m] || "box", cls);
  // Display label for the medium bucket: the gateway's internal value (watch/listen/view/read/explore,
  // derived from MIME) shown to users as elacity's content-type names (Video/Audio/Images/Documents/Other).
  const MEDIUM_LABEL = { watch: "Video", listen: "Audio", read: "Documents", view: "Images", explore: "Other" };
  const mediumLabel = (m) => MEDIUM_LABEL[m] || (m ? m.charAt(0).toUpperCase() + m.slice(1) : "");
  // The "Type" axis is the gateway's finer normalised `category` (video/audio/image/document + the kinds MIME
  // alone can't express: 3d/comic/ebook/article). A row lacking a category (lean/legacy) falls back to its
  // coarse `medium` mapped into the same vocab, so nothing disappears from a Type filter.
  const MEDIUM_TO_KIND = { watch: "video", listen: "audio", view: "image", read: "document", explore: "other" };
  const kindOf = (l) => String(l.category || MEDIUM_TO_KIND[String(l.medium || "")] || "other");
  const inKind = (l, k) => !k || kindOf(l) === k;
  const KIND_LABEL = { video: "Video", audio: "Audio", image: "Images", document: "Documents", "3d": "3D", comic: "Comics", ebook: "e-books", article: "Articles", other: "Other" };
  const kindLabel = (k) => KIND_LABEL[k] || (k ? k.charAt(0).toUpperCase() + k.slice(1) : "");
  const CCY = "USDC"; // Base pay token (gas = ETH); on-chain in 6-decimal minor units
  const money = (v) => (v ? `${v} ${CCY}` : "Free");
  const esc = (s) => String(s == null ? "" : s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  // Asset descriptions are creator-authored HTML (e.g. `<p>…</p>`). Render them as readable text, not
  // literal tags: parse to a detached node (innerHTML assignment never executes scripts) and take its
  // textContent (decodes entities + strips tags). The result is plain text — esc() it at the insertion
  // point so any residual `<` can't re-open a tag.
  const plainText = (s) => { const d = document.createElement("div"); d.innerHTML = String(s == null ? "" : s); return (d.textContent || "").replace(/\s+/g, " ").trim(); };
  const short = (id) => String(id == null ? "" : id).slice(0, 8) + "…" + String(id == null ? "" : id).slice(-4);
  // Enriched poster: render image_url as a cover. http(s) and the runtime content routes (/content, /ipfs)
  // load as-is; ipfs://CID[/path] (and bare Qm/bafy CIDs) resolve through the runtime's content plane
  // (/content/<cid> — never raw public ipfs from the browser, P4); anything else keeps the medium glyph.
  const coverUrl = (u) => {
    const s = String(u || "").trim();
    if (!s) return null;
    if (/^https?:\/\//.test(s) || s.startsWith("/content/") || s.startsWith("/ipfs/")) return s;
    const cid = s.replace(/^ipfs:\/\//, "").replace(/^\/ipfs\//, "");
    return /^(Qm|bafy)/.test(cid) ? "/content/" + cid : null;
  };
  const coverBg = (l) => { const u = coverUrl(l && l.image_url); return u ? ` style="background-image:url('${esc(u)}');background-size:cover;background-position:center"` : ""; };
  // Real royalty splits: rendered ONLY from the gateway's on-chain read (operative royaltyInfo +
  // resellerCut, CoreStorage protocolShares). No fabricated role model — parties are wallet addresses with
  // their real percentage. fmtPct trims to 1dp; the bar/legend cycle the 4 split colors.
  const SPLIT_COLORS = ["s1", "s2", "s3", "s4"];
  const fmtPct = (p) => `${(Math.round((Number(p) || 0) * 10) / 10).toString().replace(/\.0$/, "")}%`;
  const splitBars = (dist) => dist.map((x, i) => `<i class="${SPLIT_COLORS[i % 4]}" style="width:${Math.max(0, Math.min(100, Number(x.pct) || 0))}%"></i>`).join("");
  const splitLegend = (dist) => dist.map((x, i) => `<span><span class="dot ${SPLIT_COLORS[i % 4]}"></span>${esc(short(String(x.address || "")))} <b>${fmtPct(x.pct)}</b></span>`).join("");
  const cap = (s) => { s = String(s == null ? "" : s); return s ? s[0].toUpperCase() + s.slice(1) : ""; };
  // Duration ms -> "M:SS" or "H:MM:SS" (mirrors elacity timeFormat(floor(ms/1000))).
  const fmtDuration = (ms) => {
    let s = Math.floor((Number(ms) || 0) / 1000); if (s <= 0) return "";
    const h = Math.floor(s / 3600); s -= h * 3600; const m = Math.floor(s / 60); s -= m * 60;
    const p = (n) => String(n).padStart(2, "0");
    return h > 0 ? `${h}:${p(m)}:${p(s)}` : `${m}:${p(s)}`;
  };
  // ISO timestamp -> "3 days ago" (elacity dayjs.fromNow). "" when absent/unparseable — never fabricated.
  const relTime = (iso) => {
    const t = Date.parse(iso); if (!t) return "";
    let s = Math.floor((Date.now() - t) / 1000); if (s < 0) s = 0;
    for (const [name, secs] of [["year", 31536000], ["month", 2592000], ["day", 86400], ["hour", 3600], ["minute", 60]]) {
      const n = Math.floor(s / secs); if (n >= 1) return `${n} ${name}${n > 1 ? "s" : ""} ago`;
    }
    return "just now";
  };
  const prettyLabel = (s) => cap(String(s == null ? "" : s).replace(/[-_]+/g, " ").trim());
  // USD-pegged pay tokens: for these, fiat == token amount 1:1, so a "≈ $X" line is truthful with NO oracle.
  // Non-stable tokens return "" (we don't guess a rate we can't read).
  const USD_STABLE = new Set(["USDC", "USDBC", "USDC.E", "DAI", "USDT", "PYUSD", "USDS", "GUSD"]);
  const fiatUsd = (amount, sym) => {
    const n = Number(amount);
    if (!isFinite(n) || n <= 0 || !USD_STABLE.has(String(sym || "").toUpperCase())) return "";
    return "≈ $" + (Math.round(n * 100) / 100).toFixed(2);
  };
  // Append one fetched init/segment into an MSE SourceBuffer, serialized on `updateend` (mirrors the
  // runtime's own elacity-player: MSE requires waiting between appends, and keeps strict in-order delivery).
  function appendURL(sb, url) {
    return fetch(url)
      .then((r) => { if (!r.ok) throw new Error("segment " + r.status); return r.arrayBuffer(); })
      .then((buf) => new Promise((resolve, reject) => {
        const done = () => { sb.removeEventListener("updateend", done); sb.removeEventListener("error", fail); resolve(); };
        const fail = () => { sb.removeEventListener("updateend", done); sb.removeEventListener("error", fail); reject(new Error("SourceBuffer append error")); };
        sb.addEventListener("updateend", done); sb.addEventListener("error", fail);
        try { sb.appendBuffer(new Uint8Array(buf)); } catch (e) { sb.removeEventListener("updateend", done); sb.removeEventListener("error", fail); reject(e); }
      }));
  }
  // Play the clear DASH preview into `mount` via MSE — one SourceBuffer per track (video AV1 + audio AAC),
  // fed by the gateway's cached preview route. Fail-soft: any unsupported codec / fetch error shows a note,
  // never a broken element. This is the public preview only; owned playback stays on the runtime player path.
  async function playPreview(l, mount) {
    if (!window.MediaSource) { mount.innerHTML = '<div class="prevmsg">This browser can\u2019t play the preview.</div>'; return; }
    mount.innerHTML = '<div class="prevmsg">Loading preview\u2026</div>';
    const plan = await window.API.previewPlan(l.token_uri);
    if (!plan) { mount.innerHTML = '<div class="prevmsg">Preview unavailable.</div>'; return; }
    const tracks = plan.tracks.filter((t) => window.MediaSource.isTypeSupported(t.mime));
    if (!tracks.length) { mount.innerHTML = '<div class="prevmsg">This browser can\u2019t decode the preview format (AV1).</div>'; return; }
    const video = document.createElement("video");
    video.className = "previewvid"; video.controls = true; video.autoplay = true; video.playsInline = true; video.muted = false;
    const ms = new MediaSource();
    video.src = URL.createObjectURL(ms);
    mount.innerHTML = ""; mount.appendChild(video);
    ms.addEventListener("sourceopen", async () => {
      try {
        const multi = tracks.length > 1;
        const bufs = tracks.map((t) => {
          const sb = ms.addSourceBuffer(t.mime);
          if (!multi && t.kind !== "video") sb.mode = "sequence";
          return { sb, t };
        });
        for (const b of bufs) await appendURL(b.sb, b.t.init_url);
        await Promise.all(bufs.map(async (b) => { for (const u of b.t.seg_urls) await appendURL(b.sb, u); }));
        if (ms.readyState === "open") ms.endOfStream();
      } catch (e) {
        mount.innerHTML = `<div class="prevmsg">Preview failed: ${esc(e.message || String(e))}</div>`;
      }
    });
  }
  // True only for assets whose preview is a clear DASH (.mpd) clip on a playable medium.
  const hasDashPreview = (l) => !!l.preview_url && /\.mpd(\?|$)/i.test(l.preview_url) && (l.medium === "watch" || l.medium === "listen");
  // Category + tag chips from real metadata.properties (mirrors elacity). Empty -> "" (nothing fabricated).
  const chipsRow = (l) => {
    const cats = (Array.isArray(l.categories) ? l.categories : []).map((c) => `<a class="chip chip-cat" href="#/discover" data-cat="${esc(c)}">${esc(c)}</a>`);
    const tags = (Array.isArray(l.tags) ? l.tags : []).map((t) => `<span class="chip">#${esc(t)}</span>`);
    const all = cats.concat(tags);
    return all.length ? `<div class="chips">${all.join("")}</div>` : "";
  };
  const fmtBytes = (n) => { let v = Number(n) || 0; if (v <= 0) return ""; const u = ["B", "KB", "MB", "GB", "TB"]; let i = 0; while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; } return `${i ? v.toFixed(1) : v} ${u[i]}`; };
  const CHAINS = { "8453": "Base", "20": "Elastos ESC", "1": "Ethereum" };
  const shortIfAddr = (v) => (/^0x[0-9a-fA-F]{40}$/.test(String(v)) ? short(String(v)) : v);
  // Properties panel — mirrors elacity MediaProperties: a CURATED, ordered list of real metadata/on-chain
  // fields (never a raw dump). Pure render of data the gateway already fetched/read; rows are omitted when
  // their source is absent, so nothing is fabricated. Extra creator attributes are appended (prettified).
  function propertiesPanel(l, oc, royalty) {
    const attr = {}; (Array.isArray(l.attributes) ? l.attributes : []).forEach((a) => { if (a && a.label != null) attr[String(a.label).toLowerCase()] = a.value; });
    const prop = {}; (Array.isArray(l.properties) ? l.properties : []).forEach((p) => { if (p && p.label != null) prop[String(p.label)] = p.value; });
    const rows = [];
    const ctype = l.content_type || attr["content-type"] || l.mime_type;
    if (ctype) rows.push(["Content type", ctype]);
    const dur = fmtDuration(l.duration != null ? l.duration : attr["duration"]);
    if (dur) rows.push(["Duration", dur]);
    rows.push(["Access type", l.preview_url ? "Protected · preview available" : "Protected"]);
    const total = attr["supply"];
    const avail = (oc && oc.supply_left != null && oc.supply_left !== "") ? oc.supply_left : null;
    if (total != null && total !== "") rows.push(["Supply", `${total}${avail != null ? ` · ${avail} for sale` : ""}`]);
    else if (avail != null) rows.push(["Supply available", String(avail)]);
    const when = relTime(l.created_at);
    if (when) rows.push(["Uploaded", when]);
    if (l.media_size) { const b = fmtBytes(l.media_size); if (b) rows.push(["File size", b]); }
    const resalePct = (royalty && royalty.reseller_cut_pct != null) ? royalty.reseller_cut_pct : l.resale_pct;
    if (l.op_type === "buy_and_resell" && resalePct != null) rows.push(["Resale royalty", fmtPct(resalePct)]);
    if (prop.distribution) rows.push(["Usage rights", prop.distribution]);
    if (prop.labelType) rows.push(["Label", prop.labelType]);
    if (l.operative_address) rows.push(["Access", "ERC1155 · " + short(l.operative_address)]);
    if (prop.authority) rows.push(["Authority", shortIfAddr(prop.authority)]);
    if (prop.publisher) rows.push(["Publisher", shortIfAddr(prop.publisher)]);
    if (prop.chainId) rows.push(["Blockchain", CHAINS[String(prop.chainId)] || prop.chainId]);
    rows.push(["Storage", "IPFS"]);
    // Any extra creator attributes not already represented above, prettified so none render label-less.
    const SHOWN = new Set(["content-type", "duration", "supply", "optype", "rrl-percent", "resell-allowed"]);
    (Array.isArray(l.attributes) ? l.attributes : []).forEach((a) => {
      if (!a || a.label == null || a.value == null || a.value === "") return;
      if (SHOWN.has(String(a.label).toLowerCase())) return;
      rows.push([prettyLabel(a.label), a.value]);
    });
    if (!rows.length) return "";
    const body = `${rows.map(([k, v]) => `<div class="kv"><span class="k">${esc(k)}</span><span class="small">${esc(String(v))}</span></div>`).join("")}
      <div class="note">Read from the asset's on-chain metadata &amp; operative contract.</div>`;
    return accCard("Properties", body, true);
  }
  // A collapsible detail section (native <details>; mirrors elacity's AccordionGroup). `open` expands it.
  const accCard = (title, bodyHTML, open) =>
    `<details class="cardx acc"${open ? " open" : ""}><summary><h4>${esc(title)}</h4></summary><div class="accbody">${bodyHTML}</div></details>`;
  function splitsPanel(royalty, opType) {
    if (opType === "free" || !royalty || !royalty.available) return "";
    const dist = Array.isArray(royalty.distributions) ? royalty.distributions : [];
    const split = dist.length ? `<div class="splits">${splitBars(dist)}</div><div class="legend">${splitLegend(dist)}</div>` : "";
    const rows = [];
    if (opType === "buy_and_resell" && royalty.reseller_cut_pct != null) rows.push(`<div class="kv"><span class="k">Resale royalty</span><span>${fmtPct(royalty.reseller_cut_pct)}</span></div>`);
    if (royalty.protocol_pct != null) rows.push(`<div class="kv"><span class="k">Protocol fee</span><span>${fmtPct(royalty.protocol_pct)}</span></div>`);
    if (!split && !rows.length) return "";
    return `<div class="cardx"><h4>Royalty split · read on-chain</h4>
      ${split}${rows.join("")}
      <div class="note">Read live from the asset's operative contract — paid automatically on every sale.</div></div>`;
  }

  function toast(msg) { const t = $("#toast"); t.textContent = msg; t.hidden = false; clearTimeout(t._t); t._t = setTimeout(() => (t.hidden = true), 3200); }
  // Accessible modal: role=dialog + aria-modal, Escape-to-close, focus moved in + trapped, focus restored on
  // close. The sheet inner HTML supplies a <button class="x" id="x" aria-label="Close"> and an h3 id for label.
  let _lastFocus = null;
  function _trapKey(e) {
    if (e.key === "Escape") { e.preventDefault(); closeModal(); return; }
    if (e.key !== "Tab") return;
    const f = [...modalRoot.querySelectorAll('a[href],button:not([disabled]),input:not([disabled]),[tabindex]:not([tabindex="-1"])')].filter((x) => x.offsetParent !== null);
    if (!f.length) return;
    const first = f[0], last = f[f.length - 1];
    if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
  }
  function mountModal(inner, labelId) {
    _lastFocus = document.activeElement;
    modalRoot.innerHTML = `<div class="overlay"><div class="sheet" role="dialog" aria-modal="true"${labelId ? ` aria-labelledby="${labelId}"` : ""}>${inner}</div></div>`;
    modalRoot.querySelector(".overlay").addEventListener("click", (e) => { if (e.target.classList.contains("overlay")) closeModal(); });
    $("#x")?.addEventListener("click", closeModal);
    document.addEventListener("keydown", _trapKey);
    const sheet = modalRoot.querySelector(".sheet");
    (sheet.querySelector('button:not([disabled]),a[href],input') || sheet).focus?.();
    return sheet;
  }
  function closeModal() {
    document.removeEventListener("keydown", _trapKey);
    const overlay = modalRoot.querySelector(".overlay");
    if (overlay) { overlay.classList.add("closing"); setTimeout(() => { modalRoot.innerHTML = ""; }, 150); }
    else { modalRoot.innerHTML = ""; }
    if (_lastFocus && _lastFocus.focus) { _lastFocus.focus(); _lastFocus = null; }
  }

  // Your own market identity (linked wallet + handle), populated once on boot via API.me() (gated). Empty in
  // demo/standalone or when no handle is set -> the address is shown (fail-closed). wallet is lower-cased.
  const me = { wallet: "", name: "" };
  // Owner display: your OWN assets (creator_address == your wallet) show your handle; otherwise short the
  // address when it's a real 0x address, else the raw label ("—"/channel name). Never names anyone else.
  const ownerLabel = (a) => {
    const s = String(a == null ? "" : a);
    if (me.name && me.wallet && s.toLowerCase() === me.wallet) return me.name;
    return /^0x[0-9a-fA-F]{6,}$/.test(s) ? short(s) : (s || "—");
  };
  // Creator avatar (elacity shows the owner's picture; we have only an address). Render a stable gradient
  // circle seeded by the address + its first hex char — visual distinction without fabricating an identity.
  function avatarHTML(addr) {
    const seed = String(addr == null ? "" : addr);
    let h = 0; for (let i = 0; i < seed.length; i++) h = (h * 31 + seed.charCodeAt(i)) >>> 0;
    const ch = (seed.replace(/^0x/i, "")[0] || "?").toUpperCase();
    return `<span class="ava" style="background:linear-gradient(135deg,hsl(${h % 360} 58% 46%),hsl(${(h >> 3) % 360} 58% 36%))" aria-hidden="true">${esc(ch)}</span>`;
  }
  function cardHTML(l) {
    const isFree = l.op_type === "free";
    // Real on-chain price from discovery enrichment (cheapest active listing). Falls back to the lean
    // listings[] (mock/standalone) and finally to a neutral placeholder — NEVER a fabricated number. When
    // the gateway read terms but found no active listing -> "Not listed"; when it hasn't priced this row
    // yet (over the per-sweep budget) -> "—".
    const listings = Array.isArray(l.listings) ? l.listings : [];
    const cheapest = listings.length ? listings.reduce((a, b) => (b.price < a.price ? b : a), listings[0]) : null;
    const sym = l.pay_token_symbol || CCY;
    const hasFmtPrice = l.price_formatted != null && String(l.price_formatted) !== "";
    const forSale = !isFree && (l.for_sale === true || (l.for_sale == null && !!cheapest)) && (hasFmtPrice || !!cheapest);
    // Price button (right of the title), mirroring CapsuleCard's PriceButton/sell pill.
    let priceCls = "", priceTxt;
    if (isFree) { priceCls = "free"; priceTxt = "Free"; }
    else if (forSale) priceTxt = hasFmtPrice ? `${l.price_formatted} ${sym}` : money(cheapest.price);
    else { priceCls = "muted"; priceTxt = l.terms_read ? "Not listed" : "—"; }
    const medium = String(l.medium || "");
    // Thumbnail overlay chips (elacity tm-listings supply top-left, tm-resell top-right).
    const supplyChip = forSale && l.supply_available != null
      ? `<span class="tm tm-listings">${icon("package")}${esc(String(l.supply_available))}</span>` : "";
    const resellChip = (l.op_type === "buy_and_resell" && l.resale_pct != null)
      ? `<div class="tm-resell"><span class="tm tm-xl">${esc(fmtPct(l.resale_pct))} Resell</span></div>` : "";
    const dur = fmtDuration(l.duration);
    const durationChip = dur ? `<span class="tm tm-duration">${esc(dur)}</span>` : "";
    const cover = coverUrl(l.image_url);
    // Keyboard-operable + screen-reader-named: a real <a href> (the hash router handles nav natively), with
    // a concise aria-label; the decorative thumb (poster/glyph) is aria-hidden so the label isn't noisy.
    const label = esc(`${l.name || "Asset"}${l.creator_address ? ", by " + l.creator_address : ""}, ${medium}${isFree ? ", free" : ", " + priceTxt}`);
    return `<a class="card" href="#/asset/${encodeURIComponent(l.content_id)}" data-id="${esc(l.content_id)}" aria-label="${label}">
      <div class="thumb g-${esc(medium)}"${coverBg(l)} aria-hidden="true">${cover ? "" : mediaIcon(medium, "ico-cover")}${supplyChip}${resellChip}${durationChip}</div>
      <div class="meta"><div class="head">${avatarHTML(l.creator_address)}
        <div class="who"><div class="t">${esc(l.name)}</div><div class="owner">${esc(ownerLabel(l.creator_address))}${relTime(l.created_at) ? " · " + esc(relTime(l.created_at)) : ""}</div></div>
        <div class="pricebtn ${priceCls}">${esc(priceTxt)}</div></div></div></a>`;
  }
  const grid = (items) => `<div class="cards">${items.map(cardHTML).join("")}</div>`;
  // Distinct categories present across a set of rows (real metadata only), de-duped case-insensitively.
  const distinctCategories = (items) => {
    const seen = new Map();
    items.forEach((l) => (Array.isArray(l.categories) ? l.categories : []).forEach((c) => { const k = String(c).trim(); if (k) seen.set(k.toLowerCase(), k); }));
    return [...seen.values()].sort((a, b) => a.localeCompare(b)).slice(0, 16);
  };
  // A clickable category facet bar (elacity-style). Empty when no row declares a category. `active` highlights
  // the current selection; an "All" chip clears it. Chips carry data-cat for the delegated handler in wire().
  const categoryBar = (items, active) => {
    const cats = distinctCategories(items);
    if (!cats.length) return "";
    const on = (c) => active && c && active.toLowerCase() === c.toLowerCase();
    const chip = (c) => `<button class="catchip${on(c) ? " on" : ""}" data-cat="${esc(c)}">${esc(c)}</button>`;
    return `<div class="catbar"><button class="catchip${active ? "" : " on"}" data-cat="">All</button>${cats.map(chip).join("")}</div>`;
  };
  const inCategory = (l, c) => !c || (Array.isArray(l.categories) && l.categories.some((x) => String(x).toLowerCase() === c.toLowerCase()));

  async function renderDiscover() {
    // Lean-first: paint cards from the instant lean response (cached descriptive, no on-chain terms),
    // then refresh with the full response (price/cover/duration). A per-call sequence number cancels an
    // in-flight paint if the user navigates/filters again, so a slow full fetch can't clobber a newer view.
    const myRun = (renderDiscover._seq = (renderDiscover._seq || 0) + 1);
    const live = () => myRun === renderDiscover._seq;
    const filtering = state.kind || state.op || state.q || state.category;
    if (filtering) {
      const paintFiltered = (all) => {
        if (!live()) return;
        const items = all.filter((l) => inKind(l, state.kind) && inCategory(l, state.category));
        const labelBits = [state.kind && kindLabel(state.kind), state.op && state.op.replace(/_/g, " "), state.category].filter(Boolean).map((s) => " · " + s).join("");
        view.innerHTML = `<div class="shelf"><h3>${items.length} result${items.length === 1 ? "" : "s"}${labelBits}</h3>
          <button class="more" id="clear-filters">Clear filters</button></div>${categoryBar(all, state.category)}${items.length ? grid(items) : '<div class="empty">No assets match. Clear filters to browse everything.</div>'}`;
        $("#clear-filters")?.addEventListener("click", () => { state.kind = state.op = state.category = null; state.q = ""; $("#search").value = ""; syncFacets(); renderDiscover(); });
      };
      const args = { op: state.op, q: state.q };
      paintFiltered(await window.API.search({ ...args, lean: true }));
      if (!live()) return;
      paintFiltered(await window.API.search(args));
      return;
    }
    view.innerHTML = `<section class="hero"><span class="lockchip">${icon("key")} Buy the right to open it — the file pins to your library</span>
      <h1>One market for every asset.</h1>
      <p class="muted">Discover, buy, and trade dDRM assets. On purchase the encrypted file pins to your library and opens in your player. Content today — apps &amp; games coming to the same marketplace. Keys are used, never owned.</p></section>
      <div id="catbar-home"></div>
      <div id="shelves"><div class="cards">${'<div class="skeleton"></div>'.repeat(4)}</div></div>`;
    const paintHome = (sections) => {
      if (!live() || !$("#shelves")) return;
      const allListings = sections.flatMap((s) => s.listings || []);
      $("#catbar-home").innerHTML = categoryBar(allListings, null);
      $("#shelves").innerHTML = sections.filter((s) => (s.listings || []).length).map((s) =>
        `<div class="shelf"><h3>${esc(s.title)}</h3><span class="more">See all</span></div>${grid(s.listings)}`).join("");
    };
    paintHome(await window.API.sections({ lean: true }));
    if (!live()) return;
    paintHome(await window.API.sections());
  }

  async function renderAsset(id) {
    view.innerHTML = `<a class="back" id="back" href="#/discover">← Back</a><div class="detail"><div class="stage"><div class="skeleton" style="aspect-ratio:16/9"></div></div><div class="side"><div class="cardx"><div class="sk" style="height:30px;width:55%;margin-bottom:16px"></div><div class="sk" style="height:13px;margin:11px 0"></div><div class="sk" style="height:13px;margin:11px 0;width:70%"></div><div class="sk" style="height:44px;margin-top:18px;border-radius:11px"></div></div></div></div>`;
    const { listing: l, on_chain: oc, royalty, listed } = await window.API.get(id);
    const owned = oc.has_access;
    // Copies of THIS listing the wallet holds (operative balanceOf at ACCESS_TOKEN=1) — distinct from
    // `has_access`, which is content-global. Absent/0 => no held-copies row.
    const heldCopies = oc.owned_balance != null ? parseInt(oc.owned_balance, 10) || 0 : 0;
    const isFree = l.op_type === "free";
    // Honest price/currency: human-formatted price + the real pay-token symbol from the gateway
    // (price_formatted/pay_token_symbol). Falls back to the raw price / CCY only in standalone/mock.
    const sym = oc.pay_token_symbol || CCY;
    const hasPrice = oc.price != null && oc.price !== "";
    const priceStr = oc.price_formatted != null ? oc.price_formatted : oc.price;
    // forSale = there is an active listing with a real price you can actually buy right now. When false we
    // must NOT show a fabricated "0 USDC / 0 copies" — show an honest "not listed for sale" state instead.
    const forSale = !isFree && listed && hasPrice;
    const buyable = owned || isFree || forSale;
    const desc = plainText(l.description);
    const cta = owned ? "Open in your library" : (isFree ? "Add to library · free" : (forSale ? (l.op_type === "buy_and_resell" ? "Buy access · resellable" : "Buy access") : "Not listed for sale"));
    view.innerHTML = `<a class="back" id="back" href="#/discover">← Back to Discover</a>
    <div class="detail">
      <div class="stage">
        <div class="cover g-${esc(l.medium)}" id="prev-stage"${coverBg(l)}>${coverUrl(l.image_url) ? "" : mediaIcon(l.medium, "ico-cover")}<div class="covertag">${esc(mediumLabel(l.medium))}${owned ? " · in your library" : ""}</div>${hasDashPreview(l) ? '<button class="prevplay" id="prev-play" aria-label="Play preview">' + icon("play") + " Preview</button>" : ""}</div>
        <div class="stagehead"><h2>${esc(l.name)}</h2>
          <div class="muted small">by ${esc(ownerLabel(l.creator_address))} · ${esc(mediumLabel(l.medium))}</div>
          <div class="verified">✓ Identity verified <span class="cid">contentId ${short(l.content_id)} (== KID)</span></div>
          ${chipsRow(l)}
          ${desc ? `<p class="muted small" style="margin-top:10px">${esc(desc)}</p>` : ""}
        </div>
      </div>
      <div class="side">
        <div class="cardx"><h4>${owned ? "You own access" : (forSale ? "Buy access" : "Access")}</h4>
          ${owned ? `<div class="muted small" style="margin-bottom:8px">Your wallet can open this content (on-chain access check). Access is per-content — the seller shown may be someone else.</div>` : ""}
          ${heldCopies > 0 ? `<div class="kv"><span class="k">You hold</span><span>${heldCopies} ${heldCopies === 1 ? "copy" : "copies"} <span class="muted small">· this listing, on-chain</span></span></div>` : ""}
          <div class="bigprice">${isFree ? "Free" : (forSale ? esc(priceStr) + " " + esc(sym) : "Not listed for sale")}${forSale && l.listings.length > 1 ? ' <span class="muted small">· cheapest of ' + l.listings.length + " listings</span>" : ""}</div>
          ${forSale && fiatUsd(priceStr, sym) ? `<div class="muted small fiat">${esc(fiatUsd(priceStr, sym))} USD</div>` : ""}
          ${forSale ? `<div class="kv"><span class="k">Pay token</span><span>${esc(sym)} <span class="muted small">· gas ETH</span></span></div>
          <div class="kv"><span class="k">Copies available</span><span>${esc(String(oc.supply_left))}</span></div>
          <div class="kv"><span class="k">Seller</span><span class="cid" title="${esc(oc.seller)}">${oc.seller === "primary" ? "Primary" : esc(short(oc.seller))}</span></div>` : ""}
          <button class="btn block" id="cta"${buyable ? "" : " disabled"}>${cta}</button>
          ${owned && forSale ? `<button class="btn ghost block" id="buy-again" style="margin-top:8px">Buy another copy · ${esc(priceStr)} ${esc(sym)}</button>` : ""}
          ${owned ? `<div class="ownedrow"><button class="btn ghost" id="download">${icon("download")} Download to your node</button><button class="btn ghost" id="reveal">${icon("folder")} Reveal in File Explorer</button></div>
          <div class="dlstatus muted small" id="dlstatus" hidden></div>` : ""}
          <div class="note">${owned ? "The encrypted file downloads to your node and lives in your library (Acquired) — open it in your player, the marketplace renders nothing." : (forSale ? "You buy the right to open it. On purchase the encrypted file pins to your library and opens in your player." : "No seller currently has this listed for sale. Royalty terms below are read live from the asset's contract.")}</div>
        </div>
        ${owned && l.op_type === "buy_and_resell" ? `<div class="cardx"><h4>Your rights</h4><div class="muted small">You own access to this asset.</div><div style="margin-top:10px"><button class="btn ghost block" id="resell">${icon("recycle")} List for resale</button></div></div>` : ""}
        ${splitsPanel(royalty, l.op_type)}
        ${propertiesPanel(l, oc, royalty)}
        ${desc ? accCard("About", `<div class="muted small">${esc(desc)}</div>`, true) : ""}
        ${accCard("Provenance", `<div class="kv"><span class="k">Mint</span><span class="small">AssetCreated event (minted in the creator app)</span></div>
          <div class="kv"><span class="k">tokenId</span><span class="small cid">${esc(oc.token_id)}</span></div>`, false)}
        ${accCard("History", `<div id="asset-history-body"><div class="muted small">Loading on-chain history…</div></div>`, true)}
      </div>
    </div>
    <div id="more-creator" class="more-rail"></div>`;
    $("#cta").addEventListener("click", () => {
      if (owned) return openInLibrary(l);
      if (isFree) return openBuy(l, oc); // free = a zero-price access grant — routes a real (unsigned) order to your wallet, then downloads
      if (!forSale) return; // not listed for sale — button is disabled, nothing to buy
      openBuy(l, oc);
    });
    $("#buy-again")?.addEventListener("click", () => openBuy(l, oc));
    $("#download")?.addEventListener("click", (e) => downloadToNode(l, e.currentTarget));
    $("#reveal")?.addEventListener("click", () => revealInExplorer(l));
    $("#resell")?.addEventListener("click", () => openResale(l, oc, royalty));
    $("#prev-play")?.addEventListener("click", () => playPreview(l, $("#prev-stage")));
    loadAssetHistory(l); // on-chain trade history, fetched after first paint and injected into the panel
    loadMoreFromCreator(l); // sibling assets from the same channel, fetched after first paint
  }

  // "More from this creator" — other assets minted to the same channel (elacity's creator rail). Fetched
  // after the detail paints; hidden entirely when the creator has no other discoverable assets.
  async function loadMoreFromCreator(l) {
    const el = document.getElementById("more-creator");
    if (!el || !l.channel_address) return;
    const sibs = (await window.API.search({ channel: l.channel_address })).filter((x) => x.content_id !== l.content_id);
    if (!sibs.length) return;
    el.innerHTML = `<div class="shelf"><h3>More from this creator</h3></div>${grid(sibs.slice(0, 8))}`;
  }

  // One on-chain trade-history row (ItemListed / ItemSold / ItemUnlisted) decoded by the gateway. Honest
  // fields only — price formatted with the pay-token decimals; block height + a BaseScan tx link; no emoji.
  const EXPLORER_TX = "https://basescan.org/tx/";
  function histRowReal(h) {
    const label = h.type === "sale" ? "Sold" : h.type === "list" ? "Listed" : h.type === "unlist" ? "Unlisted" : cap(h.type);
    const ic = h.type === "sale" ? "cart" : h.type === "unlist" ? "recycle" : "key";
    const sym = h.pay_token_symbol || "";
    const price = (h.price_formatted != null && h.price_formatted !== "") ? `${h.price_formatted}${sym ? " " + sym : ""}` : "";
    const who = h.type === "sale" ? (h.buyer ? `buyer ${short(h.buyer)}` : "") : (h.seller ? `seller ${short(h.seller)}` : "");
    const meta = [h.block ? `block ${h.block}` : "", h.tx ? `<a class="cid" href="${EXPLORER_TX}${esc(h.tx)}" target="_blank" rel="noopener">tx ${esc(short(h.tx))}</a>` : ""].filter(Boolean).join(" · ");
    return `<div class="cardx histrow"><span class="glyph">${icon(ic)}</span>
      <div style="flex:1"><b>${label}</b>${who ? ` · <span class="muted small">${esc(who)}</span>` : ""}${meta ? `<div class="muted small">${meta}</div>` : ""}</div>
      ${price ? `<span class="price">${esc(price)}</span>` : ""}</div>`;
  }

  async function renderActivity() {
    view.innerHTML = `<div class="shelf"><h3>${icon("bell")} Activity</h3></div>
      <div id="act-body"><div class="empty">Loading recent on-chain activity…</div></div>`;
    const hist = await window.API.history();
    const body = $("#act-body");
    if (body) body.innerHTML = (Array.isArray(hist) && hist.length)
      ? hist.map(histRowReal).join("")
      : '<div class="empty">No on-chain activity in the recent window. Listings and sales appear here as they happen.</div>';
  }

  async function loadAssetHistory(l) {
    const el = document.getElementById("asset-history-body");
    if (!el) return;
    const hist = await window.API.assetHistory(l.operative_address, l.token_id);
    el.innerHTML = (Array.isArray(hist) && hist.length)
      ? hist.map(histRowReal).join("")
      : '<div class="muted small">No on-chain trade history in the recent window.</div>';
  }

  // Hand off to the EXISTING runtime open path — the marketplace renders NOTHING. In the runtime this emits
  // a Library "open" launch (or POST /api/viewers/open { uri }); the runtime gates rights, recovers the CEK
  // inside its protected-content decrypt boundary, and opens elacity-player/ddrm-viewer (chosen by mime).
  // Standalone: mock the handoff.
  // Resolve an owned asset's Library URI: from the listing, then the Vault (by content CID), then — if it's
  // held on-chain but not yet downloaded locally — by ACQUIRING it now. Acquire re-checks
  // hasAccessByContentId server-side and holds no keys (P15); it fails closed if you don't actually own it.
  // Returns "" when it can't be resolved/downloaded. Caches the uri back onto the listing for later actions.
  async function resolveUri(l, { acquire = true } = {}) {
    if (l.uri) return l.uri;
    if (l.content_cid) {
      const owned = await window.API.vault();
      const hit = owned.find((o) => o.content_cid && o.content_cid === l.content_cid);
      if (hit && hit.uri) { l.uri = hit.uri; return hit.uri; }
    }
    if (acquire && l.content_id && l.content_cid) {
      const r = await window.API.acquire({ content_id: l.content_id, content_cid: l.content_cid, token_uri: l.token_uri, metadata: { name: l.name } });
      const uri = (r && (r.uri || r.library_uri)) || "";
      if (uri) l.uri = uri;
      return uri;
    }
    return "";
  }

  // Explicit "Download to your node" — kick off a BACKGROUND pin (the request doesn't block on a multi-GB
  // fetch) and poll the gateway's truthful state. Honest only: the pin is opaque, so there is NO fabricated
  // % — we report downloading → downloaded/failed from real server state (file-presence in Acquired, or the
  // in-flight run's error). On success the file is on your node and Reveal/Open light up.
  async function downloadToNode(l, btn) {
    const status = document.getElementById("dlstatus");
    const setStatus = (msg, cls) => { if (status) { status.hidden = false; status.textContent = msg; status.className = "dlstatus muted small" + (cls ? " " + cls : ""); } };
    const done = (msg, cls) => { setStatus(msg, cls); if (btn) btn.disabled = false; };
    if (btn) btn.disabled = true;
    if (l.uri) return done("Already downloaded — it's in your library (Acquired).");
    if (!l.content_id || !l.content_cid) return done("This asset has no resolvable content to download.", "err");
    setStatus("Starting the download to your node…");
    const started = await window.API.acquire({ content_id: l.content_id, content_cid: l.content_cid, token_uri: l.token_uri, metadata: { name: l.name }, background: true });
    if (!started) return done("Couldn’t start the download — the asset must be owned (held on-chain) to pull the encrypted file.", "err");
    // Poll real state; the pin keeps running server-side even past our UI deadline (then it shows in the Vault).
    const deadline = Date.now() + 5 * 60 * 1000;
    let dots = 0;
    let blanks = 0; // consecutive reads that report neither progress NOR completion (idle / unreadable status)
    const tick = async () => {
      const s = await window.API.acquireStatus({ cid: l.content_cid, token_uri: l.token_uri });
      if (s && s.state === "downloaded") { l.uri = s.uri || l.uri; return done("Downloaded — it's in your library (Acquired). Open it in your player or reveal it in File Explorer."); }
      if (s && s.state === "failed") return done("Download failed: " + (s.message || "unknown error") + ".", "err");
      if (Date.now() > deadline) return done("Still downloading in the background — it'll appear under your Vault’s Downloaded filter shortly.");
      // Truthful progress ONLY when the server confirms a run is in flight; never animate "downloading" for a
      // state that isn't actually running. "idle" (no in-flight run materialized — e.g. lost across a restart)
      // or an unreadable status are reported honestly after a short grace instead of a fake forever-spinner.
      if (s && s.state === "downloading") {
        blanks = 0;
        dots = (dots % 3) + 1;
        setStatus("Downloading the encrypted file to your node" + ".".repeat(dots));
      } else {
        blanks += 1;
        if (blanks >= 3) return done("Couldn’t confirm the download — it may still be pinning in the background. Check your Vault’s Downloaded filter, or try again.", "err");
        setStatus("Confirming the download to your node…");
      }
      setTimeout(tick, 2000);
    };
    setTimeout(tick, 1200);
  }

  // Reveal the downloaded file in the File Explorer (Library app), opening its containing folder. Downloads
  // first if needed (acquire), then asks Home to open the Library at that folder (PC2's openFolder parity).
  async function revealInExplorer(l) {
    toast(`Locating “${l.name}” in your library…`);
    const uri = await resolveUri(l, { acquire: true });
    if (uri && window.API.reveal(uri)) { toast(`Revealed “${l.name}” in File Explorer.`); return; }
    toast(uri ? `Couldn’t open File Explorer for “${l.name}”.` : `“${l.name}” isn’t downloaded yet — download it to your node first.`);
  }

  async function openInLibrary(l) {
    toast(`Opening “${l.name}”…`);
    const uri = await resolveUri(l, { acquire: true });
    // Preferred path: ask Home to launch the player (same seam the Library app uses); the viewer capsule
    // runs the rights/decrypt open. The marketplace renders nothing and holds no CEK (P15/P16).
    if (uri && window.API.openInPlayer({ uri, name: l.name, mime: l.mime_type, medium: l.medium, content_cid: l.content_cid })) {
      toast(`Opening “${l.name}” in your player…`); return;
    }
    // Standalone fallback (no Home parent): the HTTP open path still sets up the session.
    if (uri) { const opened = await window.API.open(uri); if (opened) { toast(`Opening “${l.name}” in your player…`); return; } }
    toast(uri ? `Couldn’t launch the player for “${l.name}”.` : `“${l.name}” isn’t in your library yet — download it to your node first.`);
  }

  // Pre-flight: only the check the gateway can actually verify right now — supply (real on-chain
  // supply_left). Wallet balance + ERC-20 allowance are verified by the wallet at signing time, so we do
  // NOT assert them here with fabricated ✓ ticks; the order note explains the wallet/abort-on-drift flow.
  function preflight(oc) {
    return [
      { label: `Supply available (${oc.supply_left} left)`, ok: Number(oc.supply_left) > 0 },
    ];
  }
  async function openBuy(l, oc) {
    const checks = preflight(oc);
    const blocked = checks.find((c) => !c.ok);
    const max = Math.max(1, oc.supply_left || 1);
    let qty = 1;
    // Human price + real symbol for display/totals; the raw oc.price (minor units) is still what
    // assembleOrder sends as expected_price for the gateway's abort-on-drift re-read.
    const sym = oc.pay_token_symbol || CCY;
    const priceHuman = oc.price_formatted != null ? parseFloat(oc.price_formatted) : (Number(oc.price) || 0);
    const totalStr = () => (l.op_type === "free" ? "Free" : (priceHuman * qty).toFixed(2) + " " + sym);
    // ORDER PREVIEW is client-rendered — the buy endpoint is the REAL buy (on a runtime-signing node it
    // signs with the managed wallet and BROADCASTS), so it is called EXACTLY ONCE, on the confirm click.
    const previewJson = () => JSON.stringify({
      to: "AuthorityGateway (live sellersOf/listings re-read at assembly)",
      selector: "buyAccess(...)", content_id: l.content_id, quantity: qty,
      seller: oc.seller, expected_price: oc.price != null ? String(oc.price) : undefined,
      expected_pay_token: oc.pay_token || undefined,
      note: "Nothing is signed or sent until you confirm below. Terms are re-verified from chain "
          + "and the buy aborts on drift before broadcast (Phase-1 invariant).",
    }, null, 2);
    // Mount ONCE; the qty stepper PATCHES the qty/total/preview in place (no sheet rebuild → no
    // modal-pop replay, no focus loss, and NO network call until confirm).
    const inner = `<button class="x" id="x" aria-label="Close">✕</button><h3 id="buy-title">Buy access</h3>
      <p class="muted small">${esc(l.name)} · ${priceHuman} ${esc(sym)} each</p>
      <div class="kv"><span class="k">You receive</span><span>An access right to ${short(l.content_id)}</span></div>
      <div class="stepper"><span class="k" style="flex:1">Quantity</span><button id="dec" aria-label="Decrease quantity" ${qty <= 1 ? "disabled" : ""}>−</button><b id="qty">${qty}</b><button id="inc" aria-label="Increase quantity" ${qty >= max ? "disabled" : ""}>+</button><span class="muted small">max ${max}</span></div>
      <div class="kv"><span class="k">Total</span><span class="price" id="total">${totalStr()}</span></div>
      <h4 style="margin:14px 0 6px;font-size:var(--fxs);color:var(--mut);text-transform:uppercase;letter-spacing:.7px">Pre-flight</h4>
      ${checks.map((c) => `<div class="kv"><span class="k">${c.ok ? "✓" : "✕"} ${esc(c.label)}</span></div>`).join("")}
      ${blocked
        ? `<p class="note" style="color:var(--warn)">Blocked: ${esc(blocked.label)} — fix before buying.</p>`
        : `<p class="muted small" style="margin:12px 0 6px">Order preview — nothing is signed or broadcast until you confirm. The node's wallet signs (human-in-loop); terms re-verified from chain and abort on drift before broadcast (Phase-1 invariant).</p>
           <div class="code" id="order-code">${esc(previewJson())}</div>
           <div class="dlstatus muted small" id="buy-status" hidden></div>
           <div style="margin-top:14px"><button class="btn block" id="sign">Confirm buy → sign &amp; broadcast</button></div>`}`;
    mountModal(inner, "buy-title");
    function setQty(n) {
      qty = Math.max(1, Math.min(max, n));
      $("#qty").textContent = qty;
      $("#total").textContent = totalStr();
      $("#dec").disabled = qty <= 1; $("#inc").disabled = qty >= max;
      const code = $("#order-code"); if (code) code.textContent = previewJson();
    }
    $("#dec")?.addEventListener("click", () => setQty(qty - 1));
    $("#inc")?.addEventListener("click", () => setQty(qty + 1));
    const status = (msg) => { const s = $("#buy-status"); if (s) { s.hidden = false; s.textContent = msg; } };
    $("#sign")?.addEventListener("click", async (e) => {
      const btn = e.currentTarget;
      if (btn.dataset.busy) return; // double-submit guard
      btn.dataset.busy = "1"; btn.disabled = true; btn.textContent = "Signing & broadcasting…";
      // THE buy — one call, on this click only.
      const order = await window.API.assembleOrder({ content_id: l.content_id, quantity: qty, seller: oc.seller, price: oc.price, pay_token: oc.pay_token });
      if (!order || order.error) {
        const msg = (order && order.error) || "buy failed — no response from the gateway";
        status(msg); toast(msg);
        delete btn.dataset.busy; btn.disabled = false; btn.textContent = "Confirm buy → sign & broadcast";
        return;
      }
      const tx = order.transaction_hash || "";
      status(tx ? `Broadcast accepted · tx ${tx.slice(0, 14)}… — waiting for on-chain confirmation, then downloading…` : "Buy accepted — confirming access…");
      toast(tx ? `Buy broadcast (tx ${tx.slice(0, 14)}…) — waiting for confirmation…` : "Buy accepted — confirming…");
      // The acquire gate reads hasAccessByContentId LIVE, so it refuses (403) until the tx confirms.
      // Poll it honestly instead of declaring success: ~6s interval, up to ~3 minutes.
      let res = null;
      for (let i = 0; i < 30; i++) {
        res = await window.API.acquire({ content_id: l.content_id, content_cid: l.content_cid, token_uri: l.token_uri, metadata: { name: l.name } });
        if (res) break;
        status(`Waiting for on-chain confirmation… (${(i + 1) * 6}s)${tx ? ` · tx ${tx.slice(0, 14)}…` : ""}`);
        await new Promise((r) => setTimeout(r, 6000));
      }
      closeModal();
      if (res) {
        toast(res.uri ? `Bought & downloaded “${l.name}” — it's in your library (Acquired).` : `Bought “${l.name}” — download finishing in the background. Check your Vault.`);
      } else {
        toast(`Buy broadcast${tx ? ` (tx ${tx.slice(0, 14)}…)` : ""}, but access hasn't confirmed yet — check the Vault in a minute.`);
      }
    });
  }

  function openResale(l, oc, royalty) {
    const sym = (oc && oc.pay_token_symbol) || CCY;
    const floor = (oc && oc.price_formatted != null) ? oc.price_formatted : (l.resale_floor || (l.listings[0] && l.listings[0].price) || "");
    // Real on-chain royalty split (read by the gateway); omit entirely if unavailable — no fabricated cut.
    const hasSplit = royalty && royalty.available && Array.isArray(royalty.distributions) && royalty.distributions.length;
    const splitBlock = hasSplit
      ? `<h4 style="margin:8px 0 6px;font-size:12px;color:var(--mut);text-transform:uppercase">Royalty split · read on-chain</h4>
         <div class="splits">${splitBars(royalty.distributions)}</div>
         <div class="legend">${splitLegend(royalty.distributions)}${royalty.reseller_cut_pct != null ? `<span><span class="dot s3"></span>your resale royalty <b>${fmtPct(royalty.reseller_cut_pct)}</b></span>` : ""}</div>`
      : `<p class="muted small">Royalties route automatically on-chain on every sale.</p>`;
    mountModal(`<button class="x" id="x" aria-label="Close">✕</button><h3 id="resale-title">List for resale</h3>
      <p class="muted small">${esc(l.name)}${floor !== "" ? ` · current floor ${esc(String(floor))} ${esc(sym)}` : ""}</p>
      <div class="field"><label class="muted small">Quantity</label><input id="rq" type="number" value="1" min="1"></div>
      <div class="field"><label class="muted small">Price (${esc(sym)})</label><input id="rp" type="number" value="${esc(String(floor))}" step="0.1"></div>
      ${splitBlock}
      <p class="muted small" style="margin-top:10px">Requires proof you hold access (anti-spoof). Lists an UNSIGNED order routed to your wallet; royalties route automatically on every hop.</p>
      <div style="margin-top:12px"><button class="btn block" id="list">Route listing to wallet →</button></div>`, "resale-title");
    $("#list").addEventListener("click", (e) => { const b = e.currentTarget; if (b.dataset.busy) return; b.dataset.busy = "1"; b.disabled = true; b.textContent = "Routing to wallet…"; setTimeout(() => { closeModal(); toast("Resale listing routed to your wallet — it appears as a secondary listing once signed."); }, 150); });
  }

  let vaultTab = "owned";
  let ownedFilter = "all"; // all | downloaded | onchain — the buyer's "have I pulled the file to my node yet?" axis
  // Downloaded = the encrypted file is materialized in your library (Acquired); on-chain only = you hold the
  // access token but haven't pulled the bytes to this node yet. Truthful flags from the gateway (acquired/held).
  const isDownloaded = (o) => o.acquired === true && !!o.uri;
  const inOwnedFilter = (o) => ownedFilter === "all" || (ownedFilter === "downloaded" ? isDownloaded(o) : !isDownloaded(o));
  function listedRow(it) {
    return `<div class="cardx" style="display:flex;align-items:center;gap:12px">
      <span class="glyph">${mediaIcon(it.medium)}</span>
      <div style="flex:1"><b>${esc(it.name)}</b><div class="muted small">${esc(String(it.my_qty))} listed · ${esc(String(it.my_price))} ${esc(it.pay_token_symbol || CCY)} each · resale</div></div>
      <button class="btn ghost" data-withdraw="${esc(it.listing_id)}">Withdraw</button></div>`;
  }
  async function renderVault() {
    const [owned, listed, history] = await Promise.all([window.API.vault(), window.API.listed(), window.API.history()]);
    const tab = (id, label, n) => `<button data-tab="${id}" class="${vaultTab === id ? "on" : ""}">${label}${n ? ` · ${n}` : ""}</button>`;
    const dl = owned.filter(isDownloaded).length;
    const fchip = (id, label, n) => `<button data-ofilter="${id}" class="facet${ownedFilter === id ? " on" : ""}">${label}${n != null ? ` · ${n}` : ""}</button>`;
    const ownedFilterBar = `<div class="facets vault-facets">${fchip("all", "All", owned.length)}${fchip("downloaded", "Downloaded", dl)}${fchip("onchain", "On-chain only", owned.length - dl)}</div>`;
    let body;
    if (vaultTab === "owned") {
      const shown = owned.filter(inOwnedFilter);
      body = (owned.length ? ownedFilterBar : "") + (shown.length ? grid(shown)
        : (owned.length ? '<div class="empty">Nothing in this view. Switch filters, or download an on-chain-only asset to your node.</div>'
                        : '<div class="empty">No assets yet. Discover something to buy — it downloads to your node.</div>'));
    }
    else if (vaultTab === "listed") body = listed.length ? listed.map(listedRow).join("") : '<div class="empty">No active resale listings. List a resellable asset you own from its page.</div>';
    else body = history.length ? history.map(histRowReal).join("") : '<div class="empty">No on-chain activity in the recent window.</div>';
    view.innerHTML = `<div class="shelf"><h3>${icon("key")} Vault — assets you own &amp; your listings</h3></div>
      <div class="tabs">${tab("owned", "Owned", owned.length)}${tab("listed", "Listed", listed.length)}${tab("history", "History")}</div>
      <div id="vault-body">${body}</div>
      <p class="muted small" style="margin-top:14px">Owned assets live in your library — open them in your player from there. Minting happens in the creator app.</p>`;
    view.querySelector(".tabs").addEventListener("click", (e) => { const b = e.target.closest("button[data-tab]"); if (b) { vaultTab = b.dataset.tab; renderVault(); } });
    view.querySelector("#vault-body").addEventListener("click", (e) => { const b = e.target.closest("button[data-ofilter]"); if (b) { ownedFilter = b.dataset.ofilter; renderVault(); } });
    view.querySelectorAll("[data-withdraw]").forEach((btn) => btn.addEventListener("click", () => openWithdraw(listed.find((x) => x.listing_id === btn.dataset.withdraw))));
  }
  async function openWithdraw(it) {
    const order = await window.API.assembleCancel({ operative: it.operative_address, quantity: it.my_qty });
    mountModal(`<button class="x" id="x" aria-label="Close">✕</button>
      <h3 id="wd-title">Withdraw listing</h3><p class="muted small">${esc(it.name)} · ${esc(String(it.my_qty))} copies @ ${esc(String(it.my_price))} ${CCY}</p>
      <p class="muted small" style="margin:12px 0 6px">Cancels your resale listing on-chain. Unsigned — routed to your wallet; your access right is unaffected, only the listing is withdrawn.</p>
      <div class="code">${esc(JSON.stringify(order.unsigned_tx, null, 2))}</div>
      <div style="margin-top:14px"><button class="btn block" id="wd">Route cancel to wallet →</button></div>`, "wd-title");
    $("#wd").addEventListener("click", (e) => { const b = e.currentTarget; if (b.dataset.busy) return; b.dataset.busy = "1"; b.disabled = true; b.textContent = "Routing to wallet…"; setTimeout(() => { closeModal(); toast("Cancel routed to your wallet — the listing is withdrawn once signed."); }, 150); });
  }

  // NOTE: minting lives in the runtime's `creator` capsule, not here. The marketplace is buy/trade only.
  // (Removed the in-shell Studio mint wizard — see docs/marketplace/SCOPE.md.)

  // ---- routing ----
  function router() {
    const h = location.hash.replace(/^#\/?/, "") || "discover";
    const [route, param] = h.split("/");
    document.querySelectorAll(".rail a").forEach((a) => {
      const on = a.dataset.route === route;
      a.classList.toggle("active", on);
      if (on) a.setAttribute("aria-current", "page"); else a.removeAttribute("aria-current");
    });
    if (route === "asset" && param) return renderAsset(decodeURIComponent(param));
    if (route === "vault") return renderVault();
    if (route === "activity") return renderActivity();
    return renderDiscover();
  }

  function syncFacets() {
    document.querySelectorAll("#medium-facets button").forEach((b) => b.classList.toggle("on", b.dataset.kind === state.kind));
    document.querySelectorAll("#op-facets button").forEach((b) => b.classList.toggle("on", b.dataset.op === state.op));
  }

  function wire() {
    // Cards + rail + back + vault-pill are real <a href> now — the hash router handles navigation natively
    // (keyboard-operable, focus-ring surfaced), so no JS click delegation is needed for them.
    // facets
    $("#medium-facets").addEventListener("click", (e) => { const b = e.target.closest("button"); if (!b) return; state.kind = state.kind === b.dataset.kind ? null : b.dataset.kind; syncFacets(); if ((location.hash || "").includes("asset")) location.hash = "#/discover"; else renderDiscover(); });
    $("#op-facets").addEventListener("click", (e) => { const b = e.target.closest("button"); if (!b) return; state.op = state.op === b.dataset.op ? null : b.dataset.op; syncFacets(); if ((location.hash || "").includes("asset")) location.hash = "#/discover"; else renderDiscover(); });
    // category facet (delegated: works for the home bar, the filtered-view bar, and detail-page chips)
    view.addEventListener("click", (e) => {
      const b = e.target.closest("[data-cat]"); if (!b) return;
      e.preventDefault();
      const c = b.dataset.cat || null;
      state.category = (state.category && c && state.category.toLowerCase() === c.toLowerCase()) ? null : c;
      if ((location.hash || "").includes("asset")) location.hash = "#/discover"; else renderDiscover();
    });
    // search (debounced)
    let t; $("#search").addEventListener("input", (e) => { clearTimeout(t); t = setTimeout(() => { state.q = e.target.value.trim(); if ((location.hash || "").includes("asset")) location.hash = "#/discover"; else renderDiscover(); }, 200); });
    // ⌘K / Ctrl+K focuses search — the badge on the field promises this.
    document.addEventListener("keydown", (e) => {
      if ((e.metaKey || e.ctrlKey) && String(e.key).toLowerCase() === "k") {
        e.preventDefault();
        const field = $("#search");
        field?.focus();
        field?.select();
      }
    });
    // theme — delegate to the shared runtime (elastos-theme.js) so the choice persists and syncs across frames
    $("#theme-toggle").addEventListener("click", () => { if (window.elastosTheme) window.elastosTheme.set(window.elastosTheme.resolved() === "light" ? "dark" : "light"); });
    window.addEventListener("hashchange", router);
    announceShellMenuManifest();
    window.addEventListener("message", onShellMenuCommand);
  }

  // Shell menu bar: declare File/Go/View menus to Home; commands come back as
  // elastos:menu-command and drive the same hash router the rail links use.
  function announceShellMenuManifest() {
    const homeToken = (() => { try { return new URL(location.href).searchParams.get("home_token") || ""; } catch { return ""; } })();
    if (!homeToken || window.parent === window) return;
    window.parent.postMessage({
      type: "home:menu-manifest",
      homeToken,
      menus: [
        {
          title: "File",
          items: [
            { label: "Close Window", cmd: "__close-window" },
          ],
        },
        {
          title: "Go",
          items: [
            { label: "Discover", cmd: "go-discover" },
            { label: "Vault", cmd: "go-vault" },
            { label: "Activity", cmd: "go-activity" },
          ],
        },
        {
          title: "View",
          items: [
            { label: "Find", cmd: "find" },
          ],
        },
      ],
    }, window.location.origin);
  }

  function onShellMenuCommand(event) {
    if (event.origin !== window.location.origin) return;
    const message = event.data;
    if (message?.type !== "elastos:menu-command" || typeof message.cmd !== "string") return;
    switch (message.cmd) {
      case "go-discover": location.hash = "#/discover"; return;
      case "go-vault": location.hash = "#/vault"; return;
      case "go-activity": location.hash = "#/activity"; return;
      case "find": { const field = $("#search"); field?.focus(); field?.select(); return; }
      default:
    }
  }

  async function boot() {
    // Inject the inline SVG icons into the static chrome (rail / facets / pills) — one icon source, no emoji.
    document.querySelectorAll("[data-icon]").forEach((el) => el.insertAdjacentHTML("afterbegin", icon(el.dataset.icon)));
    // Theme toggle: stack sun + moon for the cross-fade (CSS shows one per [data-el-theme]).
    const tt = $("#theme-toggle"); if (tt) tt.innerHTML = icon("sun", "ico-sun") + icon("moon", "ico-moon");
    wire(); router();
    // Resolve "me" (your wallet + handle) non-blocking, then re-render only if a handle actually applies, so
    // your own cards relabel from address -> your name. Fail-closed: no handle/no gateway -> no change.
    window.API.me().then((m) => {
      if (m && m.wallet) { me.wallet = String(m.wallet).toLowerCase(); me.name = m.display_name || ""; if (me.name) router(); }
    }).catch(() => {});
    // surface whether a real gateway answered, after first load
    setTimeout(() => { $("#wallet-pill").textContent = window.API.live ? "◎ live" : "◎ demo"; }, 400);
  }
  document.addEventListener("DOMContentLoaded", boot);
})();
