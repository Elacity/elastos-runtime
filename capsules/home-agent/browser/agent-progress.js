/* Ambient progress narration — Progressive Reasoning Disclosure.
   Progress is a first-class stream with a reducer. The UI owns wording.
   Raw chain-of-thought never becomes the status line.
   Tip: home-20260814a */

export const ProgressPhase = {
  IDLE: "idle",
  PLANNING: "planning",
  SEARCHING: "searching",
  READING: "reading",
  ANALYZING: "analyzing",
  CODING: "coding",
  RUNNING: "running",
  VERIFYING: "verifying",
  SYNTHESIZING: "synthesizing",
  ANSWERING: "answering",
  DONE: "done",
  ERROR: "error",
  STOPPED: "stopped",
};

export const PROGRESS_REVEAL_DELAY_MS = 500;
export const MIN_STATUS_VISIBLE_MS = 1200;
export const MAX_STATUS_STALE_MS = 8000;
export const FINDING_VISIBLE_MS = 3000;
export const MAX_VISIBLE_MILESTONES = 4;
export const MEANINGFUL_ANSWER_THRESHOLD = 80;

const PRIORITY = {
  error: 100,
  stopped: 95,
  user_action_required: 90,
  finding: 80,
  verifying: 60,
  running: 55,
  coding: 50,
  searching: 40,
  reading: 35,
  analyzing: 30,
  planning: 28,
  synthesizing: 25,
  answering: 20,
  idle: 0,
  done: 0,
};

const STALE_VARIANTS = {
  planning: ["Working out the best approach…", "Checking the details…"],
  searching: ["Searching for relevant information…", "Looking for stronger sources…"],
  reading: ["Reviewing what I found…", "Checking the strongest results…"],
  analyzing: [
    "Analyzing the findings…",
    "Comparing the strongest findings…",
    "Checking for anything inconsistent…",
  ],
  coding: ["Working through the implementation…", "Checking the implementation path…"],
  running: ["Running the checks…", "Waiting on the result…"],
  verifying: ["Checking the result…", "Verifying the final details…"],
  synthesizing: ["Pulling the findings together…", "Putting the answer together…"],
  answering: ["Writing the answer…", "Checking the final details…"],
};

let progressSeq = 0;

export function createInitialProgress(generationId) {
  return {
    generationId,
    phase: ProgressPhase.IDLE,
    revealed: false,
    secondary: false,
    current: null,
    currentText: null,
    milestones: [],
    finding: null,
    staleIndex: 0,
    lastKey: "",
  };
}

export function narrateProgress(event) {
  const phase = event?.phase;
  const subject = sanitizeSubject(event?.subject || "");
  switch (phase) {
    case ProgressPhase.PLANNING:
      return subject
        ? `Working out the best approach for ${subject}…`
        : "Working out the best approach…";
    case ProgressPhase.SEARCHING:
      return subject ? `Searching for ${subject}…` : "Searching for relevant information…";
    case ProgressPhase.READING:
      return subject ? `Reviewing ${subject}…` : "Reviewing what I found…";
    case ProgressPhase.ANALYZING:
      return subject ? `Analyzing ${subject}…` : "Analyzing the findings…";
    case ProgressPhase.CODING:
      return subject ? `Working through ${subject}…` : "Working through the implementation…";
    case ProgressPhase.RUNNING:
      return subject ? `Running ${subject}…` : "Running the checks…";
    case ProgressPhase.VERIFYING:
      return subject ? `Verifying ${subject}…` : "Checking the result…";
    case ProgressPhase.SYNTHESIZING:
      return "Pulling the findings together…";
    case ProgressPhase.ANSWERING:
      return "Writing the answer…";
    case ProgressPhase.STOPPED:
      return "Stopped";
    case ProgressPhase.ERROR:
      return event?.text || "Connection interrupted";
    default:
      return null;
  }
}

/** Deterministic glyph for the activity rail. Phase only — never parse icons
 *  from model prose, and never invent tool rows without tool events. */
export function progressGlyph(phase, kind = "") {
  if (kind === "finding") {
    return "finding";
  }
  if (phase === ProgressPhase.SEARCHING) {
    return "search";
  }
  if (phase === ProgressPhase.READING) {
    return "read";
  }
  if (phase === ProgressPhase.CODING || phase === ProgressPhase.RUNNING) {
    return "code";
  }
  if (phase === ProgressPhase.VERIFYING) {
    return "verify";
  }
  if (phase === ProgressPhase.ERROR) {
    return "error";
  }
  return "reason";
}

export function shortenNarration(text) {
  const source = String(text || "");
  const m = source.match(/^([A-Z][a-z]+(?:ing)?)/);
  return m ? `${m[1]}…` : source;
}

export function milestoneLabel(current) {
  if (!current) {
    return "";
  }
  if (current.kind === "finding") {
    return current.text;
  }
  const text = String(current.text || "").replace(/…$/, "");
  return text
    .replace(/^Searching for /i, "Searched for ")
    .replace(/^Reviewing /i, "Reviewed ")
    .replace(/^Analyzing /i, "Analyzed ")
    .replace(/^Working out the best approach(?: for )?/i, "Chose an approach")
    .replace(/^Working through /i, "Worked through ")
    .replace(/^Running /i, "Ran ")
    .replace(/^Verifying /i, "Verified ")
    .replace(/^Checking /i, "Checked ")
    .replace(/^Pulling the findings together/i, "Pulled the findings together")
    .replace(/^Writing the answer/i, "Wrote the answer")
    .replace(/^Looking for /i, "Looked for ")
    .replace(/^Comparing /i, "Compared ");
}

export function sanitizeSubject(raw) {
  let s = String(raw || "")
    .replace(/[*_`#>]+/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .replace(/[.]+$/, "");
  s = s.replace(
    /^(analyzing|searching|reviewing|reading|checking|comparing|planning|verifying|implementing)\s+/i,
    "",
  );
  if (!s) {
    return "";
  }
  if (s.length > 48) {
    return "";
  }
  if (s.split(/\s+/).length > 8) {
    return "";
  }
  if (/\b(i|i'm|i’ll|i'll|maybe|perhaps|because|think that|it seems)\b/i.test(s)) {
    return "";
  }
  return s;
}

function semanticKey(phase, subject, kind = "phase") {
  return `${kind}:${phase}:${String(subject || "*").toLowerCase()}`;
}

function shouldReplaceStatus(current, next, at) {
  if (!current) {
    return true;
  }
  if ((next.priority || 0) > (current.priority || 0)) {
    return true;
  }
  if (next.key && next.key === current.key) {
    return false;
  }
  if (next.phase === current.phase && next.kind !== "finding") {
    return false;
  }
  return at - current.shownAt >= MIN_STATUS_VISIBLE_MS;
}

function promoteMilestone(state, current, at) {
  if (!current?.meaningful) {
    return state.milestones;
  }
  if (state.milestones.some((m) => m.key === current.key)) {
    return state.milestones;
  }
  const item = {
    id: current.id,
    key: current.key,
    phase: current.phase,
    kind: current.kind || "phase",
    text: milestoneLabel(current),
    completedAt: at,
  };
  return [...state.milestones, item].slice(-MAX_VISIBLE_MILESTONES);
}

function makeCurrent({ phase, subject, text, kind, at, source }) {
  const priority = PRIORITY[kind === "finding" ? "finding" : phase] || 0;
  const label = text || narrateProgress({ phase, subject }) || "Working on it…";
  return {
    id: `progress_${(progressSeq += 1)}`,
    phase,
    subject: subject || "",
    text: label,
    kind: kind || "phase",
    key: semanticKey(phase, subject, kind || "phase"),
    priority,
    source: source || "system",
    meaningful: kind === "finding" || (phase !== ProgressPhase.ANSWERING && phase !== ProgressPhase.IDLE),
    shownAt: at,
  };
}

export function progressReducer(state, event) {
  if (!state) {
    return state;
  }
  if (event.generationId != null && event.generationId !== state.generationId) {
    return state;
  }
  const at = event.at || Date.now();

  switch (event.type) {
    case "REVEAL": {
      if (state.revealed || state.phase === ProgressPhase.DONE) {
        return state;
      }
      const current =
        state.current ||
        makeCurrent({
          phase: ProgressPhase.PLANNING,
          at,
          source: "system",
        });
      return {
        ...state,
        revealed: true,
        phase: current.phase,
        current,
        currentText: current.text,
      };
    }

    case "PHASE_CHANGED": {
      if (state.phase === ProgressPhase.DONE || state.phase === ProgressPhase.STOPPED) {
        return state;
      }
      const source = event.source || "system";
      if (source === "model" && state.current?.source === "tool") {
        return state;
      }
      const next = makeCurrent({
        phase: event.phase,
        subject: event.subject,
        at,
        source,
      });
      if (next.key === state.lastKey || next.key === state.current?.key) {
        return state;
      }
      if (!shouldReplaceStatus(state.current, next, at)) {
        return state;
      }
      const milestones = promoteMilestone(state, state.current, at);
      return {
        ...state,
        phase: next.phase,
        current: next,
        currentText: state.secondary ? shortenNarration(next.text) : next.text,
        milestones,
        lastKey: next.key,
        staleIndex: 0,
        finding: null,
      };
    }

    case "FINDING": {
      if (state.phase === ProgressPhase.DONE || state.phase === ProgressPhase.STOPPED) {
        return state;
      }
      const text = String(event.text || "").trim();
      if (!text) {
        return state;
      }
      const next = makeCurrent({
        phase: state.phase === ProgressPhase.IDLE ? ProgressPhase.ANALYZING : state.phase,
        text,
        kind: "finding",
        at,
        source: event.source || "system",
      });
      next.key = event.key || next.key;
      next.meaningful = true;
      if (next.key === state.lastKey) {
        return state;
      }
      const milestones = promoteMilestone(state, state.current, at);
      return {
        ...state,
        revealed: true,
        current: next,
        currentText: text,
        milestones,
        lastKey: next.key,
        finding: { text, shownAt: at },
      };
    }

    case "TOOL_STARTED": {
      return progressReducer(state, {
        type: "PHASE_CHANGED",
        phase: event.phase || ProgressPhase.RUNNING,
        subject: event.subject,
        source: "tool",
        generationId: event.generationId,
        at,
      });
    }

    case "TOOL_FINISHED": {
      return progressReducer(state, {
        type: "PHASE_CHANGED",
        phase: event.phase || ProgressPhase.ANALYZING,
        subject: event.subject,
        source: "system",
        generationId: event.generationId,
        at,
      });
    }

    case "ANSWER_STARTED": {
      const chars = Number(event.chars) || 0;
      const secondary = chars >= MEANINGFUL_ANSWER_THRESHOLD;
      let nextState = state;
      if (state.phase !== ProgressPhase.ANSWERING && state.phase !== ProgressPhase.DONE) {
        const next = makeCurrent({
          phase: ProgressPhase.ANSWERING,
          at,
          source: "system",
        });
        const milestones = promoteMilestone(state, state.current, at);
        nextState = {
          ...state,
          phase: ProgressPhase.ANSWERING,
          current: next,
          currentText: secondary ? shortenNarration(next.text) : next.text,
          milestones,
          lastKey: next.key,
          finding: null,
        };
      }
      if (secondary && !nextState.secondary) {
        return {
          ...nextState,
          secondary: true,
          currentText: shortenNarration(nextState.currentText || nextState.current?.text || ""),
        };
      }
      return nextState;
    }

    case "STALE_REFRESH": {
      if (!state.current || state.finding) {
        return state;
      }
      if (state.phase === ProgressPhase.DONE || state.phase === ProgressPhase.STOPPED) {
        return state;
      }
      if (at - state.current.shownAt < MAX_STATUS_STALE_MS) {
        return state;
      }
      const variants = STALE_VARIANTS[state.phase];
      if (!variants?.length) {
        return state;
      }
      const idx = (state.staleIndex + 1) % variants.length;
      const text = variants[idx];
      if (text === state.currentText) {
        return { ...state, staleIndex: idx, current: { ...state.current, shownAt: at } };
      }
      return {
        ...state,
        staleIndex: idx,
        current: { ...state.current, text, shownAt: at },
        currentText: state.secondary ? shortenNarration(text) : text,
      };
    }

    case "FINDING_EXPIRE": {
      if (!state.finding) {
        return state;
      }
      const text = narrateProgress({
        phase: state.phase === ProgressPhase.IDLE ? ProgressPhase.ANALYZING : state.phase,
        subject: state.current?.subject,
      });
      return {
        ...state,
        finding: null,
        currentText: state.secondary ? shortenNarration(text) : text,
        current: state.current
          ? { ...state.current, text, kind: "phase", shownAt: at, priority: PRIORITY[state.phase] || 0 }
          : state.current,
      };
    }

    case "GENERATION_DONE": {
      const milestones = promoteMilestone(state, state.current, at);
      const n = milestones.length;
      return {
        ...state,
        phase: ProgressPhase.DONE,
        revealed: n > 0 ? true : state.revealed,
        secondary: true,
        current: null,
        currentText: n ? `✓ ${n} step${n === 1 ? "" : "s"}` : null,
        milestones,
        finding: null,
      };
    }

    case "GENERATION_ERROR": {
      const milestones = promoteMilestone(state, state.current, at);
      const text = event.text || "Connection interrupted";
      return {
        ...state,
        phase: ProgressPhase.ERROR,
        revealed: true,
        secondary: false,
        current: makeCurrent({
          phase: ProgressPhase.ERROR,
          text,
          at,
          source: "system",
        }),
        currentText: text,
        milestones,
        finding: null,
      };
    }

    case "GENERATION_STOPPED": {
      const milestones = promoteMilestone(state, state.current, at);
      return {
        ...state,
        phase: ProgressPhase.STOPPED,
        revealed: true,
        secondary: true,
        current: makeCurrent({
          phase: ProgressPhase.STOPPED,
          text: "Stopped",
          at,
          source: "system",
        }),
        currentText: "Stopped",
        milestones,
        finding: null,
      };
    }

    default:
      return state;
  }
}

export function createProgressController({ generationId, onChange, now = () => Date.now() } = {}) {
  let state = createInitialProgress(generationId);
  let staleTimer = 0;
  let findingTimer = 0;
  const notify = (prev) => {
    onChange?.(state, prev);
  };
  const clearTimers = () => {
    if (staleTimer) {
      window.clearTimeout(staleTimer);
      staleTimer = 0;
    }
    if (findingTimer) {
      window.clearTimeout(findingTimer);
      findingTimer = 0;
    }
  };
  const armStale = () => {
    if (staleTimer) {
      window.clearTimeout(staleTimer);
      staleTimer = 0;
    }
    if (
      !state.current ||
      state.phase === ProgressPhase.DONE ||
      state.phase === ProgressPhase.ERROR ||
      state.phase === ProgressPhase.STOPPED
    ) {
      return;
    }
    staleTimer = window.setTimeout(() => {
      dispatch({ type: "STALE_REFRESH" });
    }, MAX_STATUS_STALE_MS);
  };
  const dispatch = (event) => {
    const prev = state;
    state = progressReducer(state, {
      ...event,
      generationId: event.generationId ?? generationId,
      at: event.at || now(),
    });
    if (state === prev) {
      return state;
    }
    if (event.type === "FINDING") {
      if (findingTimer) {
        window.clearTimeout(findingTimer);
      }
      findingTimer = window.setTimeout(() => {
        dispatch({ type: "FINDING_EXPIRE" });
      }, FINDING_VISIBLE_MS);
    }
    armStale();
    notify(prev);
    return state;
  };
  return {
    dispatch,
    getState: () => state,
    destroy: clearTimers,
  };
}

export function snapshotProgress(state) {
  if (!state?.milestones?.length && !state?.currentText) {
    return null;
  }
  return {
    phase: state.phase,
    label: state.currentText,
    milestones: state.milestones.map((m) => ({
      key: m.key,
      phase: m.phase,
      kind: m.kind,
      text: m.text,
    })),
  };
}
