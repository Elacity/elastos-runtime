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
| `home-shell-*.mjs` (bridge, regression, recovery, no-hint, stale-hint, switchback, system-switch, auth-gate) | GUI and host cache tips `home-20260715a` → `home-20260719d` (host JS changed, see host authority note); `FakeElement.append(...)` shim because our chrome uses `Element.append`; regression summary uses `desktopApps` (successor of `desktopHidden`) |
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

## Host authority note (deliberate policy extensions, all closed sets)

Your `SHELL_MESSAGE_OPEN_TARGET_SOURCES` gives `home-gui` the `visible-target`
policy, and connectors (`wallet-metamask`/`wallet-unisat`/`wallet-walletconnect`)
are hidden from the visible summary by design — so with the connector ceremony
sheet living in GUI chrome, every connector launch was denied ("Home denied the
shell launch"). We extended `canOpenTargetFromHomeMessage` so the GUI also
carries the wallet's connector set — the exact same closed set your `"wallet"`
entry holds (now shared as `WALLET_CONNECTOR_TARGETS`), nothing broader. The
entropy check asserts both the carve-out and that the set stays closed.

Two follow-ups found in live product smoke, same closed-set discipline:

- **Viewer-bound content inherits its viewer's grants.** `gba-ucity` speaks
  with its own target id but has no policy entry, so "Choose a game from
  Library" was silently ignored. `canOpenTargetFromHomeMessage` now falls back
  to the launch's `viewer` id (`gba-emulator` → Library) — never broader than
  the viewer itself.
- **MetaMask gets UniSat's top-level popup ceremony.** Extension content
  scripts crash in opaque-sandboxed frames (no `allow-same-origin`), so no
  provider injects into the embedded sheet and Connect was dead. The MetaMask
  connector now falls back to `window.open` of its own route (your UniSat
  pattern, verbatim), and `wallet-metamask` joins `wallet-unisat` in the
  connector popup sandbox extras. Two hardening notes from live smoke:
  MetaMask sometimes injects a *dead* provider into the sandboxed sheet
  (announces via EIP-6963, then hangs every request), so the connector probes
  `eth_chainId` under a 1.5s race before trusting it; and because nested
  sandboxes intersect, your host `active-shell-frame` sandbox now includes
  `allow-popups allow-popups-to-escape-sandbox` — without them the connector
  frames' own popup grants are inert and the ceremony cannot open. That is
  the only host sandbox delta; `escape-sandbox` is required or the popup
  would itself be opaque and break the extension identically.

## Opaque-frame origin sweep (wallet + browser capsules)

`window.location.origin` inside an opaque-sandboxed frame still returns the
URL origin, while the frame's *security* origin serializes to `"null"` — so
any capsule-side channel that posts to its GUI parent with a concrete URL
target, or filters inbound GUI messages against `location.origin`, silently
dies under your sandbox model. A live-browser sweep (Playwright, virtual
passkey, real gateway) found seven such sites and we converged them on one
idiom: outbound to the opaque parent posts with `"*"` (chrome hints only —
badge counts, privacy booleans, menu manifests carry no secrets), inbound
pins `event.origin === "null" && event.source === window.parent` (fail-closed
to the direct parent, same as the GUI rail's own filter):

- `wallet.js` — rail chrome commands (`elastos:wallet-chrome-command`), GUI
  menubar commands (`elastos:menu-command`), post-ceremony refresh
  (`elastos:wallet-refresh`) inbound filters; `wallet:pending-count` outbound.
- `wallet-preferences.js` — `wallet:privacy-state` outbound.
- `browser.js` — `home:menu-manifest` now goes to the token-authorizing host
  (`window.top`, your relay path) instead of the opaque parent;
  `elastos:menu-command` inbound filter.

Without these, the rail's Activity/Settings/privacy buttons were dead, the
approvals badge never updated, Wallet never refreshed after a connector
ceremony, and Browser menus were inert. The entropy check asserts the idiom
on both capsules, and the live probe (sign-in → rail → chrome command →
MetaMask ceremony → popup) runs with zero `postMessage` origin-mismatch
warnings.

## Connector popup relay (host-bridged; token'd APIs never leave the sheet)

Your gateway fail-closes any launch-token API call whose `Origin` is not
`null` (`require_capsule_browser_origin`) — correct, and we kept it. But it
means the top-level connector popup (the only place a wallet extension
injects; content scripts crash in opaque frames on `sessionStorage`) can
never call `/api/auth/evm/*` itself: real origin → 500 "home launch token
requires an opaque capsule origin". The popup also has no window handle back
to the sheet (escaping a sandbox implies `noopener`), and `BroadcastChannel`
is origin-partitioned, so popup and opaque sheet share nothing directly.

The host is the piece that shares the popup's real origin, so it bridges:

- popup drives the provider only (accounts, chain, `personal_sign`) and posts
  stages on a same-origin `BroadcastChannel` (`elastos:connector-popup`);
- the host forwards a stage only into the mounted frame whose launch token
  matches the stage's correlation tail *and* whose target is in the closed
  `WALLET_CONNECTOR_TARGETS` set;
- the sheet makes every token'd API call (challenge, verify) from its opaque
  frame and answers back through the host (`home:connector-popup-relay`,
  token-bound app-frame context only).

Stage payloads are an address, the SIWE challenge text, and its signature —
no launch token and no secrets cross the channel. In the embedded sheet the
Continue button opens the companion window synchronously on the click (no
discovery/probe first): no provider can ever exist in the opaque frame, and
burning the transient activation on a probe made strict popup blockers
(Brave) deny the window we always need. Verified live end-to-end with an
injected EIP-6963 fake provider: the ceremony reaches `verify` and fails
only on the fake's garbage signature (403), with zero origin errors.
`wallet-unisat` has the same latent issue on your tip (its popup calls
`/api/auth/btc/*` from a real origin); we left it untouched and flag it here
as a follow-up you may want the same bridge for.

## Shell UI preferences (theme / accent / dock / sounds) — host is the store

Opaque frames throw on every `localStorage` access, which silently killed the
old preference model (localStorage + `storage` events fanning out across
same-origin frames): theme clicks applied nothing, the dock auto-hide and
sounds toggles reset themselves, and System's Personalization controls were
dead. New canonical path, one direction:

- System (Personalization) and the GUI Control Centre write
  `home:ui-preference` to the host — closed key set (`theme`, `accent`,
  `dockAutoHide`, `sounds`), closed value enums, writers restricted to the
  System app-frame context and the `home-gui` shell-frame context;
- the host (the only real-origin document) persists to its own localStorage
  and relays a `ui-preference` gui-command; it replays stored preferences on
  every `home:shell-ready`;
- the GUI applies (theme runtime, dock class, sounds flag — all now with
  in-memory overrides so opaque storage failures cannot reset state) and
  fans `elastos:ui-preference` out to its app frames, whose vendored
  `elastos-theme.js` (regenerated via `vendor-ui`) accepts it from the opaque
  parent only.

Cosmetic state only — no authority moves. Live probe: System → Light re-themes
the GUI chrome instantly, dock auto-hide toggles the dock, and both survive a
full reload + re-sign-in.

## Known limitation inherited from the opaque-frame model (decision yours)

`gba-emulator` fail-closes with "This browser does not provide isolated
WebAssembly threads": the mGBA build needs `crossOriginIsolated` +
`SharedArrayBuffer`, and an opaque-sandboxed frame (no `allow-same-origin`)
can never be cross-origin isolated, even with the gateway's COOP/COEP headers.
This reproduces identically on your tip — we did not reintroduce
`allow-same-origin` to paper over it. Options if you want GBA playable:
a credentialless/COEP-frame carve-out, a non-threaded mGBA build, or a scoped
same-origin grant for the viewer. Your model, your call.

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
