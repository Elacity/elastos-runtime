# 0.6.0 release acceptance

This checklist defines evidence to collect; it does not declare that a check
passed.

Current behavior and known limitations belong in [state.md](../state.md).
Open work belongs in [TASKS.md](../TASKS.md). Release history belongs in
[elastos/CHANGELOG.md](../elastos/CHANGELOG.md).

## Proof rules

- Record the exact branch, commit, tree, remote divergence, and dirty status.
- Review source, installed-product behavior, and public deployment as separate
  claims. Passing one does not prove the others.
- Use only the candidate's documented install path. Record source, built,
  installed, and served hashes for changed product artifacts.
- Preserve user data, passkeys, Wallet state, provider configuration, and a
  tested artifact rollback before replacing an installed candidate.
- Keep private hostnames, SSH details, credentials, data paths, and raw proof
  logs outside the repository.
- Do not turn a fixture, source test, HTTP 200, or successful launch into a
  broader product claim.
- Stop at the first unexplained product failure. Diagnose that boundary before
  retrying or widening the implementation.

## Candidate identity

Before review or installation:

```bash
git fetch origin
git status --short --branch
git log --oneline origin/feat/elastos-shell-protocol..HEAD
git diff --stat origin/feat/elastos-shell-protocol...HEAD
git rev-list --left-right --count \
  origin/feat/elastos-shell-protocol...HEAD
```

The 0.6 review line is
`fix/elastos-shell-protocol-browser-wallet-consolidation`. It is not a release
until the reviewed tree is merged to `main`, versioned, tagged, published, and
verified through the public install path.

## Source gate

Run from a clean candidate checkout:

```bash
git diff --check
node scripts/public-copy-entropy-check.mjs
node scripts/home-entropy-check.mjs
node scripts/browser-entropy-check.mjs
bash scripts/check-wci-alignment.sh
node scripts/check-elastos-bus-wit.mjs
node scripts/check-capsule-templates.mjs
bash scripts/audit-linux-runtime-portability.sh
bash scripts/protected-content-provider-contract-smoke.sh
(cd elastos && cargo fmt --all -- --check)
cargo fmt --manifest-path capsules/chain-provider/Cargo.toml -- --check
just verify
```

`protected-content-provider-contract-smoke.sh` is a fail-closed retirement
guard for the provisional provider capsules. It does not verify the canonical
v1 Runtime, rights, custody, and decrypt architecture.

`just verify` is the complete source gate in this tree. There is no separate
`terminology-lint` recipe. Any unavailable command or accepted target-specific
exception must be recorded explicitly rather than reported as a pass.

## Review order

Review the candidate in authority-owned slices:

1. Runtime audit, launch authority, and passkey step-up.
2. Wallet contract, Wallet Provider, approvals, and durable transaction effects.
3. Recovery and principal-root protection.
4. Home host, shell projections, Clipboard, and connector windows.
5. Browser Runtime, Engine Adapter, host adapter, display, networking, Wallet
   bridge, and terminal cleanup.
6. GBA/uCity and Library object behavior.
7. Release metadata, manifests, checksums, documentation, and installer truth.

Carrier reconciliation, the shell UI redesign, and extended AI UI work are not
part of 0.6.0. Do not pull them into release review opportunistically.

## Installed acceptance

Use the exact reviewed candidate and preserve the existing data root. Record
the target role and artifact hashes without committing operator details.

| Surface | Required installed evidence |
| --- | --- |
| Home and auth | Home loads; existing principal and passkey still work; expected windows restore once; sign-out control is visible for a signed-in Home GUI. |
| Wallet | Existing accounts remain; address copy works; MetaMask and UniSat open without replacing Wallet; one authorized request creates exactly one Inbox item and one terminal result. |
| Recovery | Status and export/import surfaces render without mutation. Any actual recovery operation requires a separate, explicit test plan. |
| Documents and Library | Open existing content, create or save one local object, and reopen it through the normal Runtime path. |
| Runtime-backed GBA | Launch the portable demo or full installed uCity profile; video, input, audio, save, reload, and terminal cleanup work without a 401 or 500. |
| Browser | One explicit open produces one visible session with decoded video, input, audio after user gesture, navigation through Runtime-only networking, refresh continuity, terminal close, and one explicit reopen. No automatic replacement or residual ownership remains. |
| Browser Wallet | The injected provider is visible. One account or transaction request creates exactly one matching Inbox request before signing or broadcast, and its terminal result reaches the page once. |

Library protected-content rail is visible only as disabled/read-only readiness/status
until production rights, key-release, decrypt, recipient-proof, and installed
provider evidence pass together.

The bounded Browser claim and its restart, login-retention,
profile-protection, and performance limitations are recorded in
[state.md](../state.md). Do not advertise general-purpose, cross-platform, or
arbitrary-dapp Browser reliability in 0.6.0.

## Release gate

Before merge and tag:

1. Confirm every reviewed commit is coherent and every correction is folded
   into the unpublished commit it fixes.
2. Confirm the final tree matches the tree that passed source and installed
   acceptance.
3. Verify `0.6.0` version metadata, `components.json`, provider manifests,
   checksums, installer metadata, and [CHANGELOG.md](../elastos/CHANGELOG.md).
4. Run `just verify` and the focused Wallet, Browser, Home, Recovery, GBA, and
   provider checks named by the changed slices.
5. Run `just verify-release` with the canonical publisher signer.
6. Merge the approved candidate to `main`, tag the exact merge result, and
   publish only with explicit user authorization.
7. Deploy manually from that named commit. Verify installed and served hashes,
   Home health, provider handshakes, and the public URL before moving any live
   marker.
8. Run the three public-install wrappers against the published release:

```bash
bash scripts/public-install-identity-smoke.sh
bash scripts/public-install-home-frontdoor-smoke.sh
bash scripts/public-install-operator-smoke.sh
```

## Release decision

Release only when all required source and installed checks pass for the exact
tree, every public claim has matching evidence, and remaining limitations are
written plainly in `state.md` and the changelog. Otherwise keep the branch as a
review candidate and name the first failing boundary.
