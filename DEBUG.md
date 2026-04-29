# Debugging Policy

Use this file for stable debugging guidance only. Do not use it as a running
work log.

## Workflow

1. Observe the failure with a concrete command, URL, or user journey.
2. Write the smallest falsifiable hypothesis.
3. Run one experiment that can prove or disprove that hypothesis.
4. Record the root cause in the issue, PR, or commit message once the fix is
   verified.

## Verification

- Prefer product-level smoke tests for Home, app launch, document sharing,
  pairing, and mobile/touch behavior.
- Prefer `RUSTFLAGS='-D warnings' cargo check ...` when touching runtime
  module boundaries.
- Prefer `node scripts/home-entropy-check.mjs` after UI, naming, routing,
  token, or ontology changes.
- Keep historical debugging transcripts out of active docs. Git history already
  preserves them.
