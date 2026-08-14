/* Thin harness bridge — breaks shelf ↔ harness circular import.
   Shelf calls these; harness registers implementations at bind time.
   UI ≠ authority: still mock-only until runtime tools. */

/** @type {Record<string, Function>} */
let api = {};

export function registerAgentHarnessApi(next = {}) {
  api = { ...api, ...next };
}

export function agentHarnessActive() {
  return Boolean(api.agentHarnessActive?.());
}

export function showAgentHarness(opts) {
  return api.showAgentHarness?.(opts);
}

export function hideAgentHarness(opts) {
  return api.hideAgentHarness?.(opts);
}

export async function sendToAgentHarness(prompt, opts) {
  if (!api.sendToAgentHarness) {
    throw new Error("Agent send handler not registered — call bindAgentHarness first");
  }
  return api.sendToAgentHarness(prompt, opts);
}

export function stopAgentHarnessStream() {
  api.stopAgentHarnessStream?.();
}

export function abortAgentStreamNow() {
  api.abortAgentStreamNow?.();
}
