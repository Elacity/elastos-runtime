// CSS pixels are also guest pixels. Host DPR and preview transforms do not
// change page layout; ResizeObserver reports the stage's content box.
export function browserStageSize(width, height) {
  if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) return null;
  width = Math.max(320, Math.min(3840, Math.round(width / 2) * 2));
  height = Math.max(240, Math.min(2160, Math.round(height / 2) * 2));
  const scale = Math.min(1, Math.sqrt(2_073_600 / (width * height)));
  width = Math.floor(width * scale / 2) * 2;
  height = Math.floor(height * scale / 2) * 2;
  return { width, height };
}

export function observeBrowserStage({ stage, getOwner, send, onError,
  ResizeObserverClass = globalThis.ResizeObserver,
  setTimer = setTimeout, clearTimer = clearTimeout,
  initialSequence = Date.now() * 1000,
}) {
  let timer, pending = null, inFlight = false, stopped = false;
  let applied = null, sequence = initialSequence;
  const visible = () => !stage.hidden && stage.ownerDocument?.visibilityState !== "hidden" &&
    stage.getClientRects().length > 0 && (stage.checkVisibility?.({ visibilityProperty: true }) ?? true);
  const same = (a, b) => a && b && a.owner === b.owner &&
    a.width === b.width && a.height === b.height;
  async function flush() {
    timer = undefined;
    if (stopped || inFlight || !pending || !visible()) return;
    const next = pending;
    pending = null;
    if (next.owner !== getOwner() || same(next, applied)) return;
    inFlight = true;
    try {
      const result = await send({ type: "resize", width: next.width, height: next.height,
        sequence: ++sequence }, next.owner);
      if (result?.accepted === true && next.owner === getOwner()) applied = next;
      else if (result?.resize_error && next.owner === getOwner()) onError(result.resize_error);
    } catch (error) {
      if (next.owner === getOwner()) onError(error);
    } finally {
      inFlight = false;
      // A size that is still settling keeps its own debounce deadline.
      if (pending && timer === undefined) void flush();
    }
  }
  function schedule(width, height, immediate = false) {
    clearTimer(timer);
    timer = undefined;
    const size = browserStageSize(width, height), owner = getOwner();
    pending = size && owner && visible() ? { ...size, owner } : null;
    if (!pending) return;
    if (immediate) void flush();
    else timer = setTimer(() => { void flush(); }, 150);
  }
  const observer = new ResizeObserverClass(entries => {
    const entry = entries.find(value => value.target === stage);
    if (entry) schedule(entry.contentRect.width, entry.contentRect.height);
  });
  observer.observe(stage, { box: "content-box" });
  const resumed = () => schedule(stage.clientWidth, stage.clientHeight);
  stage.ownerDocument?.addEventListener("visibilitychange", resumed);
  return {
    opened() {
      applied = null;
      // The stage has no padding. client dimensions exclude its border and
      // remain in CSS pixels even while Home transforms a window preview.
      schedule(stage.clientWidth, stage.clientHeight, true);
    },
    stop() {
      stopped = true; pending = null; clearTimer(timer); observer.disconnect();
      stage.ownerDocument?.removeEventListener("visibilitychange", resumed);
    },
  };
}

// Hold pointer mapping from the request until a frame with the ACKed geometry
// decodes. A stalled stream releases the fence after one second.
export function createBrowserFrameFence(video, { wait = ms => new Promise(resolve => setTimeout(resolve, ms)),
  now = Date.now, timeout = 1000 } = {}) {
  let generation = 0, blocked = false;
  return {
    blocked: () => blocked,
    reset() { generation++; blocked = false; },
    async run(send) {
      const current = ++generation;
      blocked = true;
      try {
        const result = await send();
        const deadline = now() + timeout;
        while (current === generation && result?.accepted === true &&
          (video.videoWidth !== result.width || video.videoHeight !== result.height) && now() < deadline) await wait(16);
        return result;
      } finally { if (current === generation) blocked = false; }
    },
  };
}
