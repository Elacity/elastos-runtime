# Anders ESP UX align — frozen tips

Safety snapshot before replay. Do not delete these refs.

| Ref | Value |
|-----|-------|
| Our UX tip (pre-align) | `4ee88d690` (`feat/shell-ui-esp`) |
| Backup branch | `backup/shell-ui-esp-pre-anders-align` → same tip |
| Backup tag | `backup/shell-ui-esp-4ee88d690` |
| Anders ESP tip | `70ef68532` (`feat/elastos-shell-protocol`) |
| Merge-base | `b2fea63db` |
| Integration branch | `feat/shell-ui-esp-on-protocol` (cut from Anders tip) |

## Rollback

```bash
git checkout backup/shell-ui-esp-pre-anders-align
# or
git checkout backup/shell-ui-esp-4ee88d690
```

## Ancestry proof after align

```bash
git merge-base --is-ancestor 70ef68532 HEAD   # Anders tip under us
git merge-base --is-ancestor 4ee88d690 backup/shell-ui-esp-pre-anders-align
```

## Conflict dossier (P0.3) — 24 UX commits oldest→newest

| # | Commit | Risk | Hot conflicts expected |
|---|--------|------|------------------------|
| A1 | `8e8c51361` design-system vendor | P2 | index.html theme links; vendor script additive |
| A2 | `f9a80777b` shell chrome onto ESP host | **P0** | shell-windows, shell-auth, host, home-gui.js — keep GUI bridge |
| A3 | `a24ffdf2f` overlays hidden | P2 | template |
| A4 | `a8aba77b8` menubar/Spotlight/Exposé/QL | **P1** | home-gui.js, shell-windows, host routing |
| B1 | `e7d4d8df6` welcome/arrival | **P0** | shell-auth = Anders; motion only ours |
| B2 | `3e453b08c` system menu | P1 | chrome/template |
| B3 | `c8feb011b` identity chip / projections | P1 | chrome; gateway Rust → Anders |
| B4 | `895c789a2` desktop objects | P1 | shell-core/surface |
| B5 | `352e1e248` Control Centre | P2 | additive module |
| B6 | `8f56186a3` About | P2 | template/home-gui |
| B7 | `a7a017306` errors/empty | P1 | spotlight/windows — no People embed |
| B8 | `8f48d1890` motion | P2 | CSS/CC |
| C1 | `0d5274287` 14-app UX | **P1** | wallet + many capsules; then C2 authority stop |
| C2 | (hard-stop) wallet passkey = Anders | **P0** | wallet-api must use parent mediation |
| D1 | `121a4d7a0` wallet rail | **P0** | launch via launchTarget hook |
| D2 | `994947af3` bar icon flush | P1 | rail CSS |
| D3–D5 | rail/wallet purple CSS | P2 | style only |
| D6 | `2171b6d47` Connect / trusted frame | **P0** | wallet + host; recheck C2 |
| D7 | `b47ecac28` ceremony sheet | **P0** | launchTarget for connector |
| D8 | `88b924e56` refresh after ceremony | P1 | connector + wallet |
| D9 | `600ab2069` dense chrome/brands | P1 | many tips + icons |
| D10 | `9fdad2d8b` drawers + entropy | P1 | entropy = Anders base + append |
| D11 | `4ee88d690` warm session 10/10 | **P0** | rail end-state + entropy append |
