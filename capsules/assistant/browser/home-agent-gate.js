/* Home may send open or shelf-handover before this document has read its
   workspace (a stale ready from the document this one replaced reaches Home
   first). Raising the room or focusing the composer then would let the
   workspace snapshot overwrite what the person typed. Until initialization
   completes, the gate holds the latest open and handover (a close drops them)
   and keeps the composer read-only; then it applies them once, in order. */
export function createHomeMessageGate({ apply, composer }) {
  let initialized = false;
  let open = null;
  let handover = null;
  if (composer) composer.readOnly = true;
  return Object.freeze({
    receive(message) {
      if (initialized) {
        apply(message);
        return;
      }
      if (message.type === "home-agent:open") open = message;
      else if (message.type === "home-agent:shelf-handover") handover = message;
      else if (message.type === "home-agent:close") {
        open = null;
        handover = null;
      }
    },
    initialized() {
      if (initialized) return;
      initialized = true;
      if (composer) composer.readOnly = false;
      const held = [open, handover].filter(Boolean);
      open = null;
      handover = null;
      for (const message of held) apply(message);
    },
  });
}
