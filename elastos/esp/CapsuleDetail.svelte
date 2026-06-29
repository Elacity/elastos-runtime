<script>
  // <CapsuleDetail> (W5b) — the Home capsule-detail surface.
  //
  // PURE PAINT over a CapsuleDetailView already projected by `capsuleDetailView`
  // (esp/capsule_detail.ts). Trust (Channel 1) and custody (Channel 2) are painted
  // as TWO INDEPENDENT channels — there is no blended "overall safe" affordance, so a
  // verified capsule still shows an exhausted budget / broken chain, and an unsigned
  // one is never dressed up by a clean custody panel. Self-contained (no nested
  // component) so the server-side render harness compiles a single file.
  let { view } = $props();

  const TRUST_LABEL = {
    verified: "Verified",
    content_addressed: "Content-addressed",
    unsigned: "Unsigned",
  };
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

  const spend = view.custody.spend;
  const audit = view.custody.audit;
</script>

<article class="capsule-detail" data-testid="capsule-detail">
  <header class="capsule-header">
    <h2 class="capsule-title">{view.title}</h2>
    <span class="capsule-name">{view.name}</span>
  </header>

  <div class="channel trust" data-channel="trust" data-trust={view.trust}>
    <span class="label">Trust</span>
    <span class="value">{TRUST_LABEL[view.trust]}</span>
  </div>

  <div class="channel custody-spend" data-channel="spend" data-state={spend.state}>
    <span class="label">Spend</span>
    <span class="value">{SPEND_LABEL[spend.state]}</span>
    {#if spend.metered}
      <span class="detail">{spend.spent} / {spend.limit}</span>
    {/if}
  </div>

  <div class="channel custody-audit" data-channel="audit" data-state={audit.state}>
    <span class="label">Audit chain</span>
    <span class="value">{AUDIT_LABEL[audit.state]}</span>
    {#if audit.present}
      <span class="detail">{audit.records} records</span>
    {/if}
  </div>
</article>
