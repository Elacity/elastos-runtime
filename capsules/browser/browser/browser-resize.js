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
      // A failed request leaves the guest geometry unknown, so the same size
      // must be sendable again to recover.
      if (next.owner === getOwner()) { applied = null; onError(error); }
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

// Pointer and wheel input map through the decoded frame. They stay blocked from
// a resize request until a frame with the guest's geometry decodes; a late
// frame still clears the fence. A request that fails without a refusal leaves
// the geometry unknown, so input waits for reset() or a later accepted resize.
export function createBrowserFrameFence(video, { wait = ms => new Promise(resolve => setTimeout(resolve, ms)),
  now = Date.now, timeout = 1000 } = {}) {
  let generation = 0, blocked = false, target = null;
  const shown = () => video.videoWidth === target.width && video.videoHeight === target.height;
  async function late(current) {
    while (current === generation && !shown()) await wait(250);
    if (current === generation) blocked = false;
  }
  return {
    blocked: () => blocked,
    reset() { generation++; blocked = false; target = null; },
    async run(send) {
      const current = ++generation;
      const prior = blocked ? target : video.videoWidth > 0 && video.videoHeight > 0
        ? { width: video.videoWidth, height: video.videoHeight } : null;
      blocked = true; target = null;
      const result = await send();
      if (current !== generation) return result;
      if (result?.accepted === true) {
        if (Number.isInteger(result.width) && Number.isInteger(result.height)) target = { width: result.width, height: result.height };
      } else if (result?.resize_error) target = prior; // A refusal keeps the earlier geometry.
      if (!target) return result;
      const deadline = now() + timeout;
      while (current === generation && !shown() && now() < deadline) await wait(16);
      // Past the deadline the fence keeps waiting for the frame, off the request path.
      if (current === generation) void late(current);
      return result;
    },
  };
}
