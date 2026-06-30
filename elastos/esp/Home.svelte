<script>
  // <Home> (W5b) — the Home FLEET landing surface.
  //
  // PURE PAINT over a HomeView already projected by `homeView` (esp/home.ts). Each
  // capsule row paints the SAME two independent channels as <CapsuleDetail> (trust +
  // custody), inlined here so the server-side render harness compiles a single file.
  //
  // There is NO fleet-level "all good" affordance: the only summary is the honest
  // `needsAttention` count, which can only draw the eye toward a wrong capsule. A
  // verified-but-exhausted capsule and an unsigned-but-clean capsule both paint their
  // honest states side by side; nothing here can render the fleet green over them.
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
</script>

<main class="home" data-testid="home">
  <header class="home-header">
    <h1 class="home-title">Capsules</h1>
    <span class="fleet-summary" data-total={view.total} data-attention={view.needsAttention}>
      {view.needsAttention} of {view.total} need attention
    </span>
  </header>

  <ul class="capsule-list">
    {#each view.capsules as capsule (capsule.name)}
      <li class="capsule-row" data-testid="capsule-row" data-name={capsule.name}>
        <span class="capsule-title">{capsule.title}</span>
        <span class="capsule-name">{capsule.name}</span>

        <span class="channel trust" data-channel="trust" data-trust={capsule.trust}>
          {TRUST_LABEL[capsule.trust]}
        </span>

        <span class="channel spend" data-channel="spend" data-state={capsule.custody.spend.state}>
          {SPEND_LABEL[capsule.custody.spend.state]}
          {#if capsule.custody.spend.metered}
            <span class="detail">{capsule.custody.spend.spent} / {capsule.custody.spend.limit}</span>
          {/if}
        </span>

        <span class="channel audit" data-channel="audit" data-state={capsule.custody.audit.state}>
          {AUDIT_LABEL[capsule.custody.audit.state]}
          {#if capsule.custody.audit.present}
            <span class="detail">{capsule.custody.audit.records} records</span>
          {/if}
        </span>
      </li>
    {/each}
  </ul>
</main>
