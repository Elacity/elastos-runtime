/* HOME_RENDERER_FREEZE_THINK_STORM — QoS lanes for live stream ingest.
   Answer deltas: lossless, ordered, high priority.
   Raw thinking.delta: telemetry only. Zero Markdown, KaTeX, DOM, or
   full-string reconstruction on the live path. Progress is latest-wins. */

export const MAX_DISPATCH_DEPTH = 8;
export const FLUSH_DEADLINE_MS = 4;
export const YIELD_EVENT_SLICE = 32;
export const YIELD_MS = 4;

export function createStreamMetrics() {
  return {
    thinkEventsReceived: 0,
    thinkCharsReceived: 0,
    thinkUiCommits: 0,
    rawThinkMarkdownRenders: 0,
    rawThinkKatexRenders: 0,
    answerEventsReceived: 0,
    answerCharsReceived: 0,
    answerUiCommits: 0,
    maxDispatchDepth: 0,
    maxQueueDepth: 0,
    dispatchRecursionFailures: 0,
    flushReentryBlocks: 0,
    maxEventLoopLagMs: 0,
  };
}

export function createStreamQos({ throwOnRecursion = true, now = () => Date.now() } = {}) {
  const metrics = createStreamMetrics();
  const queues = {
    answer: [],
    latestProgress: null,
  };
  let dispatchDepth = 0;
  let flushing = false;
  let flushPending = false;
  let thinkingCommitted = false;
  let answeringCommitted = false;

  const ingestEvent = (event) => {
    dispatchDepth += 1;
    metrics.maxDispatchDepth = Math.max(metrics.maxDispatchDepth, dispatchDepth);
    try {
      if (dispatchDepth > MAX_DISPATCH_DEPTH) {
        metrics.dispatchRecursionFailures += 1;
        if (throwOnRecursion) {
          throw new Error("Stream dispatch recursion");
        }
        return;
      }
      const type = event?.type;
      if (type === "thinking.delta") {
        const delta = typeof event.delta === "string" ? event.delta : "";
        metrics.thinkEventsReceived += 1;
        metrics.thinkCharsReceived += delta.length;
        if (!answeringCommitted) {
          queues.latestProgress = { type: "progress", label: "Thinking…", key: "thinking" };
        }
        return;
      }
      if (type === "answer.delta") {
        const delta = typeof event.delta === "string" ? event.delta : "";
        metrics.answerEventsReceived += 1;
        metrics.answerCharsReceived += delta.length;
        queues.answer.push(delta);
        metrics.maxQueueDepth = Math.max(metrics.maxQueueDepth, queues.answer.length);
        queues.latestProgress = {
          type: "progress",
          label: "Writing the answer…",
          key: "answering",
        };
        return;
      }
      if (type === "completed") {
        queues.latestProgress = { type: "progress", label: null, key: "done" };
      }
    } finally {
      dispatchDepth -= 1;
    }
  };

  const flushPresentation = (handlers = {}) => {
    if (flushing) {
      flushPending = true;
      metrics.flushReentryBlocks += 1;
      return { reschedule: true, answerApplied: 0, progressApplied: 0 };
    }
    flushing = true;
    let answerApplied = 0;
    let progressApplied = 0;
    try {
      flushPending = false;
      const t0 = now();
      while (queues.answer.length && now() - t0 < FLUSH_DEADLINE_MS) {
        const delta = queues.answer.shift();
        handlers.applyAnswer?.(delta);
        answerApplied += 1;
        metrics.answerUiCommits += 1;
      }
      if (now() - t0 < FLUSH_DEADLINE_MS && queues.latestProgress) {
        const progress = queues.latestProgress;
        queues.latestProgress = null;
        if (progress.key === "thinking") {
          if (!thinkingCommitted) {
            thinkingCommitted = true;
            handlers.applyProgress?.(progress);
            progressApplied += 1;
            metrics.thinkUiCommits += 1;
          }
        } else if (progress.key === "answering") {
          answeringCommitted = true;
          handlers.applyProgress?.(progress);
          progressApplied += 1;
        } else {
          handlers.applyProgress?.(progress);
          progressApplied += 1;
        }
      }
    } finally {
      flushing = false;
    }
    const reschedule = flushPending || queues.answer.length > 0 || Boolean(queues.latestProgress);
    return { reschedule, answerApplied, progressApplied };
  };

  return {
    ingestEvent,
    flushPresentation,
    metrics,
    queues,
    get flushing() {
      return flushing;
    },
  };
}

export function recoverStalePersistedTurn(session) {
  if (!session || typeof session !== "object") {
    return session;
  }
  const turn = session.lastTurn;
  if (!turn || typeof turn !== "object") {
    return session;
  }
  const state = String(turn.state || "");
  if (state !== "submitted" && state !== "streaming") {
    return session;
  }
  return {
    ...session,
    lastTurn: {
      ...turn,
      state: "interrupted",
      error: "client_stream_lost",
      completedAt: Number(turn.completedAt) || Date.now(),
    },
  };
}

export async function yieldToBrowser() {
  await new Promise((resolve) => {
    setTimeout(resolve, 0);
  });
}
