# Anders review pack — `feat/shell-ui-esp-on-protocol`

Paste-ready summary for reviewing / pulling our UI on top of your ESP tip.

## Ancestry

```bash
git merge-base --is-ancestor 70ef68532 HEAD   # must succeed
```

- Your tip: `70ef68532` (`feat/elastos-shell-protocol`)
- Integration branch: `feat/shell-ui-esp-on-protocol` (cut from your tip, UX replayed on top)
- Pre-align UX backup (ours, untouched): `backup/shell-ui-esp-pre-anders-align` / tag `backup/shell-ui-esp-4ee88d690`

## What you should see

| Yours (authority / protocol) | Ours (presentation) |
|------------------------------|---------------------|
| Home host iframe GUI bridge | Design system + shell chrome |
| People as standalone capsule | Control Centre / menubar / Spotlight / Exposé |
| Wallet passkey via parent mediation | Wallet rail + connector ceremony sheet |
| Capsule launch via host `launchTarget` | Dense Wallet chrome / warm session |

**Intentional behavior change vs our old tip:** People is a capsule (`openTarget("people")`), not an embedded Home window.

## Full diff census vs your tip (149 files — nothing outside these buckets)

| Bucket | Files | What it is |
|--------|-------|------------|
| Presentation (capsule UI) | 79 | Home GUI chrome modules, wallet rail/ceremony UX, per-app CSS/HTML/JS visual ports |
| Vendored design tokens | 51 | `_shared` token sheet + `elastos-theme.js`/`elastos-ui.css`/Inter font stamped per capsule |
| Ops/gate scripts | 14 | 13 of your smokes updated (reasons below) + new `vendor-ui-tokens.sh` |
| Docs | 4 | `ALIGN_TIPS.md`, this pack, `docs/DESIGN_SYSTEM.md`, `state.md` (append-only entry) |
| `justfile` | 1 | Adds `vendor-ui` recipe and `vendor-ui-tokens.sh --check` to `verify` |
| **`elastos/` backend** | **0** | **Byte-identical to your tip** (an earlier 8-line web-projection launch guard was reverted as redundant — `is_runtime_projection()` already covers `execution=web-projection`) |

## Your smoke/gate files we touched (each with the reason)

| File | Why |
|------|-----|
| `home-shell-*.mjs` (bridge, regression, recovery, no-hint, stale-hint, switchback, system-switch, auth-gate) | GUI cache tip `home-20260715a` → `home-20260719a`; `FakeElement.append(...)` shim because our chrome uses `Element.append`; regression summary uses `desktopApps` (successor of `desktopHidden`) |
| `home-passkey-virtual-auth-smoke.mjs` | First boot now opens on a welcome beat; smoke conditionally clicks "Get started" before the create-passkey form |
| `wallet-product-safety-smoke.sh` | New assert: MetaMask connect must revoke + re-prompt `eth_accounts` so a second account can be linked |
| `wallet-connector-transaction-smoke.mjs` | Mock provider answers `wallet_requestPermissions` / `wallet_revokePermissions` used by the connect ceremony |
| `browser-entropy-check.mjs` | Browser accent asserts moved to the shared `--el-accent` token |
| `home-entropy-check.mjs` | Your tip's file as base + our UX asserts appended (rail, connector sheet, desktop objects, tokens, wallet microcopy). A handful of our stale asserts were replaced by successors (e.g. `desktopHidden` → `desktopApps`); none of your protocol asserts were weakened or removed. Also adds a regression assert that `configureWindowHooks` forwards `launchTarget` (see incident note) |

## Incident note (fixed, gated)

During integration the `launchTarget` forward was briefly dropped from
`configureWindowHooks` in `home-gui.js` — your hook contract fails closed, so
the GUI threw at module load (black desktop). Fixed in `9356944eb`, and the
entropy check now asserts the wiring so `just verify` catches this class of
regression (assert verified to fail on the broken commit).

## Forbidden paths (ripgrep-clean on this tip)

- No `openPeopleWindow` / embedded People DOM
- No Wallet `/api/auth/passkey` local refresh
- No GUI-frame `fetchJson("/api/apps/home/launch")` bypass
- No `HOME_GUI_MODULE_URL` direct GUI mount

## Bridge conformance (positive checks, not just absence)

- Every `home:*` message the GUI sends has a host handler; every `home:gui-command` the host sends has a GUI handler (inventories match)
- Wallet rail + connector sheet launch through `launchHomeTarget()` → your `launchTarget` hook, and take sandbox/allow from your `iframeSandboxForLaunch` / `iframeAllowForLaunch` (constants unchanged from your tip)
- Wallet fresh authority: `home:request-passkey-authority` via `window.top` only
- Spotlight People result activates `openTarget("people")`

## Gates (run on this exact tip)

- `just verify` — green (one macOS-local flaky: `elastos-vz bridge_propagates_runtime_eof_to_guest_and_exits` timed out once, passed 3/3 on rerun and the full gate passed end-to-end)
- `./scripts/vendor-ui-tokens.sh --check` — OK (runs inside verify)
- `node scripts/home-entropy-check.mjs` — PASS (your base + UX asserts)

## What we are asking

Please review whether this UI/UX sits correctly on your contracts. Prefer merging/pulling **this branch** into yours when happy — we are not asking you to re-merge protocol work. Pulling is fast-forward from your tip.

We will **not** retarget `feat/shell-ui-esp` until Katie explicitly approves.

## 12-path smoke (for local product check)

1. Boot Home / unlock  
2. Spotlight → People (capsule window)  
3. Control Centre / theme / fullscreen  
4. Menubar + About  
5. Desktop icon open / remove / add  
6. Wallet rail open / edge peek  
7. Wallet Send / Receive flip  
8. Wallet Settings currency  
9. Edge peek leave during grace  
10. Wallet → Open window (rail session retired)  
11. Connect MetaMask (ceremony sheet; accounts refresh)  
12. Open .gba / uCity  
