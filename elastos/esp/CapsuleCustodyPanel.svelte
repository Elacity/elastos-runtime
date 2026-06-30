<script>
  // <CapsuleCustodyPanel> (W5b) — the Home capsule-detail custody panel.
  //
  // PURE PAINT. The only prop is a `HomeCustodyView` already projected by the
  // headless layer (`homeCustodyView` in esp/spend_audit.ts). This component
  // derives NO custody logic — it maps each already-decided honest state to a
  // display label + a `data-state` attribute, nothing more. There is deliberately
  // NO "all good" / green affordance keyed on anything but the honest sub-states,
  // so the panel cannot render reassurance over an unmetered/exhausted budget or an
  // absent/broken chain.
  let { view } = $props();

  const spend = view.spend;
  const audit = view.audit;
  const intent = view.intent;

  // Honest display labels — one per fail-honest state from the projection.
  const SPEND_LABEL = {
    unmetered: "Unmetered",
    ok: "Within budget",
    warning: "Near budget limit",
    exhausted: "Budget exhausted",
  };
  const AUDIT_LABEL = {
    absent: "No durable chain",
    verified: "Chain verified",
    broken: "Chain tampered",
  };
  // Intent-proof verdict — absence is NOT a pass; any flagged count is an alarm.
  const INTENT_LABEL = {
    absent: "No agent-intent custody",
    clean: "Intents within grant",
    flagged: "Intents flagged",
  };
</script>

<section class="custody-panel" data-testid="capsule-custody-panel">
  <div class="custody-channel spend" data-channel="spend" data-state={spend.state}>
    <span class="label">Spend</span>
    <span class="value">{SPEND_LABEL[spend.state]}</span>
    {#if spend.metered}
      <span class="detail">{spend.spent} / {spend.limit}</span>
    {/if}
  </div>

  <div class="custody-channel audit" data-channel="audit" data-state={audit.state}>
    <span class="label">Audit chain</span>
    <span class="value">{AUDIT_LABEL[audit.state]}</span>
    {#if audit.present}
      <span class="detail">{audit.records} records</span>
    {/if}
  </div>

  <div class="custody-channel intent" data-channel="intent" data-state={intent.state}>
    <span class="label">Agent intents</span>
    <span class="value">{INTENT_LABEL[intent.state]}</span>
    {#if intent.flagged > 0}
      <span class="detail">{intent.denied} denied · {intent.diverged} diverged · {intent.undelivered} undelivered</span>
    {/if}
  </div>
</section>
