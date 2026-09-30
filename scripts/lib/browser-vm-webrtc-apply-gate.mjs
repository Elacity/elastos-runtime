export function webrtcSignalKind(body) {
  const type = body?.signal?.type;
  if (type === "display_attach") return "display_attach";
  if (type === "answer") return "answer";
  if (type === "candidate" || type === "end_of_candidates") return "ice";
  return "other";
}

export function createGuestWebrtcApplyGate() {
  const gates = new Map();

  function gateKey(pageId, channel) {
    return `${pageId}:${channel === "audio" ? "audio" : "video"}`;
  }

  function gate(pageId, channel) {
    const key = gateKey(pageId, channel);
    let entry = gates.get(key);
    if (!entry) {
      entry = { answerApplied: false, expectedGeneration: "", pending: [] };
      gates.set(key, entry);
    }
    return entry;
  }

  function rejectPending(entry, error) {
    const pending = entry.pending.splice(0);
    for (const item of pending) {
      item.reject(error);
    }
  }

  function resetPage(pageId) {
    for (const channel of ["video", "audio"]) {
      const key = gateKey(pageId, channel);
      const entry = gates.get(key);
      if (entry) {
        rejectPending(
          entry,
          new Error("Browser display attachment replaced pending ICE"),
        );
      }
      gates.delete(key);
    }
  }

  async function flushPending(entry, forward) {
    const pending = entry.pending.splice(0);
    await Promise.all(
      pending.map((item) =>
        Promise.resolve()
          .then(() => forward(item.body))
          .then(item.resolve, item.reject),
      ),
    );
  }

  async function apply(pageId, body, forward) {
    const kind = webrtcSignalKind(body);
    if (kind === "display_attach") {
      resetPage(pageId);
      const result = await forward(body);
      const generation = result?.display_generation;
      if (typeof generation === "string" && generation) {
        gate(pageId, "video").expectedGeneration = generation;
        gate(pageId, "audio").expectedGeneration = generation;
      }
      return result;
    }
    const entry = gate(pageId, body?.channel);
    const generation = body?.signal?.display_generation;
    if (
      entry.expectedGeneration &&
      typeof generation === "string" &&
      generation !== entry.expectedGeneration
    ) {
      return forward(body);
    }
    if (kind === "answer") {
      try {
        const result = await forward(body);
        entry.answerApplied = true;
        await flushPending(entry, forward);
        return result;
      } catch (error) {
        rejectPending(entry, error);
        throw error;
      }
    }
    if (kind === "ice" && !entry.answerApplied) {
      return new Promise((resolve, reject) => {
        entry.pending.push({ body, resolve, reject });
      });
    }
    return forward(body);
  }

  return { apply, resetPage };
}
