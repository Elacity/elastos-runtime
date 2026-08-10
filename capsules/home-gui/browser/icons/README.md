# ElastOS first-party app icons

Runtime copy of the Final Master (liquid) pack for Shelf, Apps, desktop,
Spotlight, window chrome, and App Store (via `/apps/home-gui/icons/…`).

- IDs match capsule `target` ids (plus `apps-launcher`, `bin`, `elastos`,
  `intelligence`, …)
- Sizes shipped: 32 / 64 / 128 / 256
- Transparent RGBA masters
- Apps launcher dock variants:
  - `apps-launcher/dark-dock/` — lighter cells for dark mode
  - `apps-launcher/light-dock/` — darker cells for light mode

Keep this folder as the product source of truth.


## Expansion (first-party catalogue)

Additional IDs from `elastos-first-party-icon-expansion` (liquid/glass family):

- browser-engine, browser-exit, chains, content-index, content-storage
- identity, network, storage, wallet-security, webspaces
- home-cli, home-gui

Provider capsules map to these IDs via aliases in `resolveAppIconId` /
marketplace `resolveFirstPartyIconId`.


## Approval-provider marks (vendor)

Official connector marks used by Wallet and App Store listings. Not ElastOS
original artwork — attributed to the respective products:

- `metamask/` — MetaMask (from `capsules/wallet/browser/icons/metamask.png`)
- `unisat/` — UniSat (from `capsules/wallet/browser/icons/unisat.png`)
- `walletconnect/` — WalletConnect (ingested mark; transparent RGBA ladder)

These remain recognisable vendor marks without ElastOS restyling.
