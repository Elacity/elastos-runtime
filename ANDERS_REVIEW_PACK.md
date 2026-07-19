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

## Forbidden paths (ripgrep-clean on this tip)

- No `openPeopleWindow` / embedded People DOM
- No Wallet `/api/auth/passkey` local refresh
- No GUI-frame `fetchJson("/api/apps/home/launch")` bypass
- No `HOME_GUI_MODULE_URL` direct GUI mount

## Gates

- `just verify` — green
- `./scripts/vendor-ui-tokens.sh --check` — OK
- `node scripts/home-entropy-check.mjs` — PASS (Anders base + UX asserts)

## What we are asking

Please review whether this UI/UX sits correctly on your contracts. Prefer merging/pulling **this branch** into yours when happy — we are not asking you to re-merge protocol work.

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
