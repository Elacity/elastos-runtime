#!/usr/bin/env node
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import process from "node:process";
import playwright from "../elastos/tools/browser-playwright-engine/node_modules/playwright/index.js";

function usage() {
  console.error(`Usage:
  scripts/browser-hosted-product-webrtc-smoke.mjs \\
    --adapter-config /path/to/browser-engine-adapter.json \\
    [--adapter-bin capsules/browser-engine-adapter/target/debug/browser-engine-adapter] \\
    [--hold-ms 0] \\
    [--min-video-width 1280] \\
    [--min-video-height 720] \\
    [--min-video-fps 24] \\
    [--max-video-drop-ratio 0.10] \\
    [--resize-width 0 --resize-height 0] \\
    [--url https://example.com/] \\
    [--timeout-ms 30000]
`);
}

function parseArgs(argv) {
  const args = {
    adapterBin: "capsules/browser-engine-adapter/target/debug/browser-engine-adapter",
    adapterConfig: "",
    cdpEndpoint: "",
    holdMs: 0,
    maxVideoDropRatio: 0.10,
    minVideoFps: 24,
    minVideoHeight: 720,
    minVideoWidth: 1280,
    requireMedia: false,
    resizeHeight: 0,
    resizeWidth: 0,
    url: "https://example.com/",
    timeoutMs: 30_000,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--adapter-bin") {
      args.adapterBin = argv[++index] || "";
    } else if (arg === "--adapter-config") {
      args.adapterConfig = argv[++index] || "";
    } else if (arg === "--cdp-endpoint") {
      args.cdpEndpoint = argv[++index] || "";
    } else if (arg === "--hold-ms") {
      args.holdMs = Number(argv[++index] || "0");
    } else if (arg === "--min-video-width") {
      args.minVideoWidth = Number(argv[++index] || "0");
    } else if (arg === "--min-video-height") {
      args.minVideoHeight = Number(argv[++index] || "0");
    } else if (arg === "--min-video-fps") {
      args.minVideoFps = Number(argv[++index] || "0");
    } else if (arg === "--max-video-drop-ratio") {
      args.maxVideoDropRatio = Number(argv[++index] || "0");
    } else if (arg === "--resize-width") {
      args.resizeWidth = Number(argv[++index] || "0");
    } else if (arg === "--resize-height") {
      args.resizeHeight = Number(argv[++index] || "0");
    } else if (arg === "--require-media") {
      args.requireMedia = true;
    } else if (arg === "--url") {
      args.url = argv[++index] || "";
    } else if (arg === "--timeout-ms") {
      args.timeoutMs = Number(argv[++index] || "0");
    } else if (arg === "--help" || arg === "-h") {
      usage();
      process.exit(0);
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  if (!args.adapterConfig) {
    throw new Error("--adapter-config is required");
  }
  if (!Number.isInteger(args.timeoutMs) || args.timeoutMs < 5_000 || args.timeoutMs > 120_000) {
    throw new Error("--timeout-ms must be 5000..120000");
  }
  if (!Number.isInteger(args.holdMs) || args.holdMs < 0 || args.holdMs > 300_000) {
    throw new Error("--hold-ms must be 0..300000");
  }
  if (!Number.isInteger(args.minVideoWidth) || args.minVideoWidth < 0 || args.minVideoWidth > 7680) {
    throw new Error("--min-video-width must be 0..7680");
  }
  if (!Number.isInteger(args.minVideoHeight) || args.minVideoHeight < 0 || args.minVideoHeight > 4320) {
    throw new Error("--min-video-height must be 0..4320");
  }
  if (!Number.isFinite(args.minVideoFps) || args.minVideoFps < 0 || args.minVideoFps > 240) {
    throw new Error("--min-video-fps must be 0..240");
  }
  if (!Number.isFinite(args.maxVideoDropRatio) || args.maxVideoDropRatio < 0 || args.maxVideoDropRatio > 1) {
    throw new Error("--max-video-drop-ratio must be 0..1");
  }
  const hasResize = args.resizeWidth > 0 || args.resizeHeight > 0;
  if (hasResize) {
    if (
      !Number.isInteger(args.resizeWidth) ||
      !Number.isInteger(args.resizeHeight) ||
      args.resizeWidth < 320 ||
      args.resizeWidth > 7680 ||
      args.resizeHeight < 240 ||
      args.resizeHeight > 4320
    ) {
      throw new Error("--resize-width/--resize-height must both be set and within 320x240..7680x4320");
    }
  }
  if (!/^https?:\/\//.test(args.url)) {
    throw new Error("--url must use http or https");
  }
  if (args.requireMedia && !/^https?:\/\/(127\.0\.0\.1|localhost):\d+\/?$/.test(args.cdpEndpoint)) {
    throw new Error("--require-media requires --cdp-endpoint with an operator-private loopback HTTP endpoint");
  }
  return args;
}

function stripTrickleCandidatesFromSdp(sdp) {
  return String(sdp || "")
    .split(/\r?\n/)
    .filter((line) => line && !line.startsWith("a=candidate:") && line !== "a=end-of-candidates")
    .join("\r\n");
}

function waitFor(predicate, timeoutMs, label) {
  const startedAt = Date.now();
  return new Promise((resolve, reject) => {
    const timer = setInterval(async () => {
      let matched = false;
      try {
        matched = await predicate();
      } catch (error) {
        clearInterval(timer);
        reject(error);
        return;
      }
      if (matched) {
        clearInterval(timer);
        resolve();
        return;
      }
      if (Date.now() - startedAt > timeoutMs) {
        clearInterval(timer);
        reject(new Error(`timed out waiting for ${label}`));
      }
    }, 50);
  });
}

function selkiesMessagesForInput(event) {
  if (event.type === "click") {
    const x = Math.round(Number(event.x || 0));
    const y = Math.round(Number(event.y || 0));
    return [`m,${x},${y},0,0`, `m,${x},${y},1,0`, `m,${x},${y},0,0`];
  }
  if (event.type === "key") {
    if (typeof event.key === "string" && [...event.key].length === 1) {
      return [`co,end,${event.key}`];
    }
    const keysymByKey = {
      " ": 32,
      Enter: 65293,
      Escape: 65307,
      ArrowLeft: 65361,
      ArrowUp: 65362,
      ArrowRight: 65363,
      ArrowDown: 65364,
    };
    const keysym = keysymByKey[event.key] ?? (typeof event.key === "string" && [...event.key].length === 1
      ? event.key.codePointAt(0)
      : null);
    return keysym == null ? [] : [`kd,${keysym}`, `ku,${keysym}`];
  }
  return [];
}

class AdapterClient {
  constructor(adapterBin) {
    this.child = spawn(adapterBin, [], {
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.pending = [];
    this.stderr = "";
    this.closed = false;
    createInterface({ input: this.child.stdout }).on("line", (line) => this.handleLine(line));
    this.child.stderr.on("data", (chunk) => {
      this.stderr += chunk.toString("utf8");
      process.stderr.write(chunk);
    });
    this.child.on("exit", (code, signal) => {
      this.closed = true;
      for (const pending of this.pending.splice(0)) {
        pending.reject(new Error(`browser-engine-adapter exited with ${signal || code}`));
      }
    });
  }

  handleLine(line) {
    const pending = this.pending.shift();
    if (!pending) {
      return;
    }
    try {
      pending.resolve(JSON.parse(line));
    } catch (error) {
      pending.reject(new Error(`adapter returned non-JSON line: ${line}`));
    }
  }

  request(payload) {
    if (this.closed) {
      return Promise.reject(new Error("browser-engine-adapter is closed"));
    }
    return new Promise((resolve, reject) => {
      this.pending.push({ resolve, reject });
      this.child.stdin.write(`${JSON.stringify(payload)}\n`);
    });
  }

  async close(pageId) {
    if (!this.closed && pageId) {
      await this.request({ op: "close_page", page_id: pageId }).catch(() => {});
    }
    if (!this.closed) {
      await this.request({ op: "shutdown" }).catch(() => {});
      this.child.stdin.end();
      this.child.kill();
    }
  }
}

function expectOk(response, label) {
  if (!response || response.status !== "ok") {
    throw new Error(`${label} failed: ${response?.code || "unknown"} ${response?.message || ""}`);
  }
  return response.data || {};
}

function streamSessionFor(url) {
  const target = new URL(url);
  const port = target.port || (target.protocol === "https:" ? "443" : "80");
  const scheme = target.protocol === "https:" ? "tls" : "tcp";
  return {
    schema: "elastos.exit.stream-session/v1",
    stream_id: "stream:hosted-product-webrtc-smoke",
    target: `${scheme}://${target.hostname}:${port}`,
    byte_transport: "adapter_ipc",
    adapter_ipc: {
      schema: "elastos.adapter-ipc/v1",
      kind: "unix_socket",
      path: "/tmp/elastos-browser-product-webrtc-smoke-adapter.sock",
      stream_id: "stream:hosted-product-webrtc-smoke",
      runtime_stream_path: "/tmp/elastos-browser-product-webrtc-smoke-runtime.sock",
    },
  };
}

function assertQualityGate(stats, args, options = {}) {
  if (!stats) {
    throw new Error("WebRTC quality stats are unavailable");
  }
  const decodedFrames = Math.max(
    Number(stats.video_frames_decoded || 0),
    Number(stats.video_element_decoded_frames || 0),
  );
  const droppedFrames = Math.max(
    Number(stats.video_frames_dropped || 0),
    Number(stats.video_element_dropped_frames || 0),
  );
  if (decodedFrames <= 0) {
    throw new Error(`WebRTC quality gate failed: no decoded video frames (${JSON.stringify(stats)})`);
  }
  if (Number(stats.video_bytes_received || 0) <= 0) {
    throw new Error(`WebRTC quality gate failed: no received video bytes (${JSON.stringify(stats)})`);
  }
  if (options.requireAudioBytes && Number(stats.audio_bytes_received || 0) <= 0) {
    throw new Error(`WebRTC quality gate failed: no received audio bytes (${JSON.stringify(stats)})`);
  }
  if (Number(stats.video_element_width || 0) < args.minVideoWidth || Number(stats.video_element_height || 0) < args.minVideoHeight) {
    throw new Error(`WebRTC quality gate failed: rendered video is below ${args.minVideoWidth}x${args.minVideoHeight} (${JSON.stringify(stats)})`);
  }
  if (Number(stats.video_fps || 0) > 0 && Number(stats.video_fps || 0) < args.minVideoFps) {
    throw new Error(`WebRTC quality gate failed: video FPS is below ${args.minVideoFps} (${JSON.stringify(stats)})`);
  }
  const dropRatio = droppedFrames / Math.max(1, decodedFrames + droppedFrames);
  if (dropRatio > args.maxVideoDropRatio) {
    throw new Error(`WebRTC quality gate failed: video drop ratio ${dropRatio.toFixed(4)} exceeds ${args.maxVideoDropRatio} (${JSON.stringify(stats)})`);
  }
  return {
    decoded_frames: decodedFrames,
    dropped_frames: droppedFrames,
    drop_ratio: dropRatio,
    min_video_width: args.minVideoWidth,
    min_video_height: args.minVideoHeight,
    min_video_fps: args.minVideoFps,
    max_video_drop_ratio: args.maxVideoDropRatio,
    audio_bytes_required: options.requireAudioBytes === true,
  };
}

function displayPointForMediaElement(video, pageMetrics, displaySize) {
  const displayWidth = Number(displaySize?.width || pageMetrics?.viewport_width || 1280);
  const displayHeight = Number(displaySize?.height || pageMetrics?.viewport_height || 720);
  const viewportWidth = Math.max(1, Number(pageMetrics?.viewport_width || displayWidth));
  const viewportHeight = Math.max(1, Number(pageMetrics?.viewport_height || displayHeight));
  const frameOffsetX = Number(video.frame_offset_x || 0);
  const frameOffsetY = Number(video.frame_offset_y || 0);
  const pageX = frameOffsetX + Number(video.center_x || viewportWidth / 2);
  const pageY = frameOffsetY + Number(video.center_y || viewportHeight / 2);
  return {
    x: Math.round(Math.max(0, Math.min(displayWidth, pageX * (displayWidth / viewportWidth)))),
    y: Math.round(Math.max(0, Math.min(displayHeight, pageY * (displayHeight / viewportHeight)))),
  };
}

async function remoteVideoElementSize(page) {
  return await page.evaluate(() => {
    const video = globalThis.__elastosRemoteVideo;
    if (!video) {
      return null;
    }
    const rect = video.getBoundingClientRect();
    return {
      video_width: Number(video.videoWidth || 0),
      video_height: Number(video.videoHeight || 0),
      client_width: Number(rect.width || 0),
      client_height: Number(rect.height || 0),
    };
  });
}

async function assertRemoteViewportResize(adapter, pageId, page, width, height) {
  const response = expectOk(
    await adapter.request({
      op: "input",
      page_id: pageId,
      event: {
        type: "resize",
        viewport: { width, height },
      },
    }),
    "runtime/provider viewport resize",
  );
  if (response.accepted !== true || response.direct_network !== false) {
    throw new Error(`Runtime/provider resize was not accepted fail-closed (${JSON.stringify(response)})`);
  }
  if (Math.abs(Number(response.width || 0) - width) > 2 || Math.abs(Number(response.height || 0) - height) > 2) {
    throw new Error(`Runtime/provider resize did not return requested viewport (${JSON.stringify(response)})`);
  }
  let lastSize = null;
  await waitFor(
    async () => {
      lastSize = await remoteVideoElementSize(page);
      return Number(lastSize?.video_width || 0) > 0 && Number(lastSize?.video_height || 0) > 0;
    },
    10_000,
    "remote video dimensions after provider viewport resize",
  );
  return {
    requested_width: width,
    requested_height: height,
    css_width: Number(response.width || 0),
    css_height: Number(response.height || 0),
    video_width: Number(lastSize?.video_width || 0),
    video_height: Number(lastSize?.video_height || 0),
    client_width: Number(lastSize?.client_width || 0),
    client_height: Number(lastSize?.client_height || 0),
  };
}

async function assertRemoteMediaPlayback(cdpEndpoint, targetUrl, timeoutMs, sendInput, displaySize) {
  const { chromium } = playwright;
  const browser = await chromium.connectOverCDP(cdpEndpoint);
  try {
    const deadline = Date.now() + timeoutMs;
    let remotePage = null;
    while (Date.now() < deadline && !remotePage) {
      for (const candidate of browser.contexts().flatMap((context) => context.pages())) {
        if (candidate.url() === targetUrl) {
          remotePage = candidate;
          break;
        }
      }
      if (!remotePage) {
        await new Promise((resolve) => setTimeout(resolve, 250));
      }
    }
    if (!remotePage) {
      throw new Error(`hosted media page was not found at ${targetUrl}`);
    }
    await remotePage.bringToFront().catch(() => {});
    let first = null;
    let last = null;
    let lastMedia = null;
    let lastActivationAt = 0;
    const activateVideo = async (video, pageMetrics) => {
      const { x, y } = displayPointForMediaElement(video, pageMetrics, displaySize);
      lastActivationAt = Date.now();
      await sendInput({ type: "click", x, y }).catch(() => {});
      await sendInput({ type: "key", key: " " }).catch(() => {});
      await sendInput({ type: "key", key: "k" }).catch(() => {});
    };
    for (let attempt = 0; attempt < Math.ceil(timeoutMs / 1000); attempt += 1) {
      await remotePage.waitForTimeout(1000);
      const media = await remoteMediaSnapshot(remotePage);
      lastMedia = media;
      const text = media.frame_summaries.map((item) => `${item.title} ${item.text_sample}`).join(" ");
      if (/not a bot|kein bot|confirm you.?re not a bot|sign in to confirm/i.test(text)) {
        throw new Error(`YouTube upstream bot challenge on selected Browser Exit: ${text.slice(0, 320)}`);
      }
      const video = media.elements.find((item) => item.tag === "video" && item.ready_state >= 2);
      if (!video) {
        continue;
      }
      if (!first) {
        first = video;
        if (video.paused) {
          await activateVideo(video, media.page_metrics);
        }
      }
      last = video;
      const timeDelta = Number(last.current_time || 0) - Number(first.current_time || 0);
      const videoDelta = Number(last.video_decoded_bytes || 0) - Number(first.video_decoded_bytes || 0);
      const audioDelta = Number(last.audio_decoded_bytes || 0) - Number(first.audio_decoded_bytes || 0);
      if ((last.paused || timeDelta < 0.5) && Date.now() - lastActivationAt > 2500) {
        await activateVideo(last, media.page_metrics);
      }
      if (timeDelta >= 2 && videoDelta > 0 && audioDelta > 0 && !last.paused && !last.muted) {
        return {
          current_time_delta: timeDelta,
          video_decoded_delta: videoDelta,
          audio_decoded_delta: audioDelta,
          actual_url: remotePage.url(),
        };
      }
    }
    throw new Error(`hosted media playback did not reach stable video+audio decode: ${JSON.stringify({ first, last, lastMedia })}`);
  } finally {
    await browser.close().catch(() => {});
  }
}

async function remoteMediaSnapshot(page) {
  const elements = [];
  const frame_summaries = [];
  const page_metrics = await page.evaluate(() => ({
    viewport_width: Number(window.innerWidth || 0),
    viewport_height: Number(window.innerHeight || 0),
    device_pixel_ratio: Number(window.devicePixelRatio || 1),
  })).catch(() => ({
    viewport_width: 0,
    viewport_height: 0,
    device_pixel_ratio: 1,
  }));
  for (const [frameIndex, frame] of page.frames().entries()) {
    const summary = await frame.evaluate((frameIndexInPage) => ({
      frame_index: frameIndexInPage,
      frame_url: String(window.location.href || ""),
      title: String(document.title || ""),
      text_sample: String(document.body?.innerText || "").replace(/\s+/g, " ").trim().slice(0, 240),
    }), frameIndex).catch(() => null);
    if (summary) {
      frame_summaries.push(summary);
    }
    const items = await frame.evaluate((frameIndexInPage) => Array.from(document.querySelectorAll("video,audio")).map((element, index) => {
      const rect = element.getBoundingClientRect();
      let frameOffsetX = 0;
      let frameOffsetY = 0;
      try {
        const frameElement = window.frameElement;
        if (frameElement) {
          const frameRect = frameElement.getBoundingClientRect();
          frameOffsetX = Number(frameRect.left || 0);
          frameOffsetY = Number(frameRect.top || 0);
        }
      } catch {}
      return {
        frame_index: frameIndexInPage,
        frame_url: String(window.location.href || ""),
        title: String(document.title || ""),
        text_sample: String(document.body?.innerText || "").replace(/\s+/g, " ").trim().slice(0, 240),
        index,
        tag: element.tagName.toLowerCase(),
        current_time: Number(element.currentTime || 0),
        duration: Number.isFinite(Number(element.duration)) ? Number(element.duration) : null,
        paused: Boolean(element.paused),
        muted: Boolean(element.muted),
        volume: Number(element.volume),
        ready_state: Number(element.readyState),
        current_src: String(element.currentSrc || element.src || ""),
        video_width: Number(element.videoWidth || 0),
        video_height: Number(element.videoHeight || 0),
        client_width: Number(rect.width || 0),
        client_height: Number(rect.height || 0),
        center_x: Number(rect.left + rect.width / 2),
        center_y: Number(rect.top + rect.height / 2),
        frame_offset_x: frameOffsetX,
        frame_offset_y: frameOffsetY,
        audio_decoded_bytes: Number(element.webkitAudioDecodedByteCount || 0),
        video_decoded_bytes: Number(element.webkitVideoDecodedByteCount || 0),
      };
    }), frameIndex).catch(() => []);
    elements.push(...items);
  }
  return { elements, frame_summaries, page_metrics };
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const config = JSON.parse(await import("node:fs").then((fs) => fs.readFileSync(args.adapterConfig, "utf8")));
  const adapter = new AdapterClient(args.adapterBin);
  let pageId = "";
  let browser = null;
  let page = null;
  const signalPromises = new Set();
  let canSignalCandidates = false;
  const queuedCandidates = [];

  const signal = async (payload) => {
    const response = expectOk(
      await adapter.request({
        op: "webrtc_signal",
        page_id: pageId,
        signal: payload,
        principal_id: "person:local:hosted-product-webrtc-smoke",
      }),
      payload.type || "webrtc_signal",
    );
    if (response.candidates?.length && page) {
      await page.evaluate(async (candidates) => {
        for (const candidate of candidates) {
          await globalThis.__elastosPeer.addIceCandidate(candidate).catch(() => {});
        }
      }, response.candidates);
    }
    return response;
  };

  const enqueueSignal = (payload) => {
    if (!canSignalCandidates && payload.type !== "answer") {
      queuedCandidates.push(payload);
      return;
    }
    const promise = signal(payload).finally(() => signalPromises.delete(promise));
    signalPromises.add(promise);
  };

  try {
    expectOk(await adapter.request({ op: "init", config }), "init");
    const launchedPage = expectOk(
      await adapter.request({
        op: "launch",
        url: args.url,
        stream_session: streamSessionFor(args.url),
        principal_id: "person:local:hosted-product-webrtc-smoke",
        reason: "verify hosted product WebRTC media",
        display_mode: "webrtc_remote_display",
        viewport: { width: 1280, height: 720 },
      }),
      "launch",
    );
    pageId = launchedPage.page_id;
    const session = launchedPage.display_session || {};
    if (
      session.schema !== "elastos.browser.display-session/v1" ||
      session.mode !== "webrtc_remote_display" ||
      session.backend_class !== "product_compositor" ||
      session.audio !== true ||
      session.video !== true ||
      session.offerer !== "engine" ||
      session.input !== "datachannel" ||
      session.input_protocol !== "selkies_v1"
    ) {
      throw new Error(`unexpected hosted display session: ${JSON.stringify(session)}`);
    }
    const initialOffer = session.initial_offer || {};
    if (
      initialOffer.schema !== "elastos.browser.webrtc-offer/v1" ||
      initialOffer.type !== "offer" ||
      typeof initialOffer.sdp !== "string"
    ) {
      throw new Error("hosted display session missing engine WebRTC offer");
    }

    const { chromium } = playwright;
    browser = await chromium.launch({
      headless: true,
      args: ["--autoplay-policy=no-user-gesture-required"],
    });
    page = await browser.newPage();
    await page.goto("about:blank");
    const answerSdp = await page.evaluate(async ({ sdp, iceServers }) => {
      const state = {
        tracks: [],
        dataChannelOpen: false,
        candidates: [],
        connectionState: "new",
        iceConnectionState: "new",
      };
      globalThis.__elastosWebrtcState = state;
      const peer = new RTCPeerConnection({
        iceServers,
        bundlePolicy: "max-bundle",
        rtcpMuxPolicy: "require",
      });
      globalThis.__elastosPeer = peer;
      globalThis.__elastosRemoteStream = new MediaStream();
      const remoteVideo = document.createElement("video");
      remoteVideo.autoplay = true;
      remoteVideo.muted = true;
      remoteVideo.playsInline = true;
      remoteVideo.srcObject = globalThis.__elastosRemoteStream;
      globalThis.__elastosRemoteVideo = remoteVideo;
      document.body.appendChild(remoteVideo);
      globalThis.__elastosInputChannel = null;
      globalThis.__elastosDataChannelLabels = [];
      globalThis.__elastosSendInputMessages = (messages) => {
        const channel = globalThis.__elastosInputChannel;
        if (!channel || channel.readyState !== "open") {
          throw new Error("input datachannel is not open");
        }
        for (const message of messages) {
          channel.send(message);
        }
      };
      peer.addEventListener("track", (event) => {
        if (event.track?.kind && !state.tracks.includes(event.track.kind)) {
          state.tracks.push(event.track.kind);
        }
        if (event.track && !globalThis.__elastosRemoteStream.getTracks().some((track) => track.id === event.track.id)) {
          globalThis.__elastosRemoteStream.addTrack(event.track);
          remoteVideo.play().catch(() => {});
        }
      });
      peer.addEventListener("datachannel", (event) => {
        const channel = event.channel;
        state.dataChannelLabels = state.dataChannelLabels || [];
        state.dataChannelLabels.push(channel.label || "");
        globalThis.__elastosDataChannelLabels.push(channel.label || "");
        channel.addEventListener("open", () => {
          state.dataChannelOpen = true;
          if (channel.label === "input" || !globalThis.__elastosInputChannel) {
            globalThis.__elastosInputChannel = channel;
            channel.send("m,0,0,0,0");
          }
        });
      });
      peer.addEventListener("connectionstatechange", () => {
        state.connectionState = peer.connectionState;
      });
      peer.addEventListener("iceconnectionstatechange", () => {
        state.iceConnectionState = peer.iceConnectionState;
      });
      peer.addEventListener("icecandidate", (event) => {
        state.candidates.push(event.candidate ? event.candidate.toJSON() : null);
      });
      await peer.setRemoteDescription({ type: "offer", sdp });
      const answer = await peer.createAnswer();
      await peer.setLocalDescription(answer);
      return peer.localDescription.sdp;
    }, {
      sdp: initialOffer.sdp,
      iceServers: session.ice_servers || [],
    });

    await signal({
      schema: "elastos.browser.webrtc-answer/v1",
      type: "answer",
      sdp: stripTrickleCandidatesFromSdp(answerSdp),
    });
    canSignalCandidates = true;
    for (const candidate of queuedCandidates.splice(0)) {
      enqueueSignal(candidate);
    }

    await waitFor(
      async () => {
        const candidates = await page.evaluate(() => globalThis.__elastosWebrtcState.candidates.splice(0));
        for (const candidate of candidates) {
          enqueueSignal(candidate
            ? {
                schema: "elastos.browser.webrtc-candidate/v1",
                type: "candidate",
                candidate,
              }
            : {
                schema: "elastos.browser.webrtc-end-of-candidates/v1",
                type: "end_of_candidates",
              });
        }
        const state = await page.evaluate(() => ({ ...globalThis.__elastosWebrtcState }));
        return (
          state.tracks.includes("video") &&
          state.tracks.includes("audio") &&
          state.dataChannelOpen &&
          ["connected", "completed"].includes(state.iceConnectionState)
        );
      },
      args.timeoutMs,
      "audio/video tracks, datachannel input, and connected ICE",
    );
    await Promise.all([...signalPromises]);
    const state = await page.evaluate(() => ({ ...globalThis.__elastosWebrtcState }));
    const sendInput = async (event) => {
      const messages = selkiesMessagesForInput(event);
      if (messages.length === 0) {
        return;
      }
      await page.evaluate((items) => globalThis.__elastosSendInputMessages(items), messages);
    };
    const displaySize = {
      width: Number(session.width || launchedPage.view?.width || 1280),
      height: Number(session.height || launchedPage.view?.height || 720),
    };
    const resizeGate = args.resizeWidth > 0
      ? await assertRemoteViewportResize(adapter, launchedPage.page_id, page, args.resizeWidth, args.resizeHeight)
      : null;
    const effectiveDisplaySize = resizeGate
      ? { width: resizeGate.video_width, height: resizeGate.video_height }
      : displaySize;
    const media = args.requireMedia
      ? await assertRemoteMediaPlayback(args.cdpEndpoint, args.url, args.timeoutMs, sendInput, effectiveDisplaySize)
      : null;
    if (args.holdMs > 0) {
      const deadline = Date.now() + args.holdMs;
      while (Date.now() < deadline) {
        const heldState = await page.evaluate(() => ({ ...globalThis.__elastosWebrtcState }));
        if (["failed", "closed", "disconnected"].includes(heldState.connectionState) ||
          ["failed", "closed", "disconnected"].includes(heldState.iceConnectionState)) {
          throw new Error(`WebRTC session did not remain stable during hold: ${JSON.stringify(heldState)}`);
        }
        await page.waitForTimeout(Math.min(1000, Math.max(1, deadline - Date.now())));
      }
    }
    const webrtcStats = await page.evaluate(async () => {
      const peer = globalThis.__elastosPeer;
      if (!peer || typeof peer.getStats !== "function") {
        return null;
      }
      const report = await peer.getStats();
      const stats = {};
      for (const item of report.values()) {
        const mediaKind = item.kind || item.mediaType;
        const isInboundVideo =
          item.type === "inbound-rtp" &&
          (mediaKind === "video" || "framesDecoded" in item || "framesPerSecond" in item);
        const isInboundAudio =
          item.type === "inbound-rtp" &&
          !isInboundVideo &&
          (mediaKind === "audio" || "audioLevel" in item || "totalAudioEnergy" in item);
        if (isInboundVideo) {
          stats.video_frames_decoded = Number(item.framesDecoded || 0);
          stats.video_frames_dropped = Number(item.framesDropped || 0);
          stats.video_fps = Number(item.framesPerSecond || 0);
          stats.video_bytes_received = Number(item.bytesReceived || 0);
          stats.video_packets_lost = Number(item.packetsLost || 0);
        } else if (isInboundAudio) {
          stats.audio_bytes_received = Number(item.bytesReceived || 0);
          stats.audio_packets_lost = Number(item.packetsLost || 0);
        } else if (item.type === "candidate-pair" && item.state === "succeeded" && item.nominated) {
          stats.rtt_ms = Number(item.currentRoundTripTime || 0) * 1000;
          stats.available_incoming_bitrate = Number(item.availableIncomingBitrate || 0);
        }
      }
      const video = globalThis.__elastosRemoteVideo;
      if (video) {
        stats.video_element_width = Number(video.videoWidth || 0);
        stats.video_element_height = Number(video.videoHeight || 0);
        stats.video_element_decoded_frames = Number(video.webkitDecodedFrameCount || 0);
        stats.video_element_dropped_frames = Number(video.webkitDroppedFrameCount || 0);
      }
      return stats;
    });
    const qualityGate = args.requireMedia || args.holdMs > 0
      ? assertQualityGate(webrtcStats, args, { requireAudioBytes: args.requireMedia })
      : null;

    console.log(JSON.stringify({
      ok: true,
      schema: "elastos.browser.hosted-product-webrtc-smoke/v1",
      page_id: pageId,
      display_backend: session.display_backend,
      backend_class: session.backend_class,
      audio_track: state.tracks.includes("audio"),
      video_track: state.tracks.includes("video"),
      datachannel_input: state.dataChannelOpen,
      ice_connection_state: state.iceConnectionState,
      media,
      held_ms: args.holdMs,
      webrtc_stats: webrtcStats,
      quality_gate: qualityGate,
      resize_gate: resizeGate,
      direct_network: false,
    }));
  } finally {
    await browser?.close().catch(() => {});
    await adapter.close(pageId);
  }
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
});
