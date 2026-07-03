# Marketplace UI strategy — elacity-web (prod dApp) vs the runtime shell

> Decision document from the 2026-06-24 six-seat council deep-audit (Mara/Frontend, Devon/Design,
> Idris/Runtime, Sable/Security, Tao/Integration, Rhea/Delivery + chair synthesis). Question asked by the
> CEO: *could we use `elacity-web` (the production marketplace dApp, excellent UI/UX) AS the runtime
> marketplace app, served by a capsule and wired to our Rust/WASM/gateway?*

## Verdict — **No full port. Use elacity-web as a DESIGN DONOR, not a runtime dependency.**
Unanimous across all six seats. elacity-web is genuinely production-grade, but it is architected as a
**self-custody browser dApp** whose trust model is the **exact inverse** of the runtime's: the UI holds the
signer (Particle/UA + an embedded MPC wallet iframe), holds its own chain RPC, fetches media from raw public
IPFS, and renders/decrypts content itself — violating **P16, P3, and P4** in precisely the parts hardest to
retrofit. The vanilla shell already implements the principle-correct path (unsigned-buy → external wallet,
content via `/content/*`, home-token auth) in ~808 lines. **Keep the shell as the spine; transplant
elacity-web's brand identity into it.** ~80–90% of the visible premium for ~10% of the cost, zero P16
regression, no permanent fork-merge tax.

## Design map (what's actually worth taking)
| Dimension | elacity-web | Runtime shell | Takeaway |
|---|---|---|---|
| **Token core** | MUI/Emotion TS objects, no CSS-var layer | True CSS-var contract (4pt, easings, tabular-nums, reduced-motion) | **Shell is the stronger core.** elacity tokens are MUI-welded, not portable. |
| **Brand identity** | Turquoise `#5edad9` + gold `#DAA520`, gradient + frosted-glass signatures | Generic `#5b8cff` blue, no gradient signature | **elacity's clearest win — and it's values-only.** Transplantable as `:root` vars. |
| **Component craft** | 804 components, CapsuleCard info-design, MediaViewer | ~6-line `cardHTML`, no viewer (correct) | Harvest **card/skeleton/facet** patterns only; MediaViewer is **out of scope (P16)**. |
| **A11y** | 44px targets, focus-visible, reduced-motion | focus-visible rings, reduced-motion, tabular-nums | **Parity** — not a differentiator. |
| **Footprint** | 144 deps, 3.0G node_modules, 1402 files | Vanilla, ~63KB, ~zero npm trust surface | Shell is ~50× lighter; capsule budget is `memory_mb:24`. |

## Integration map (why the port breaks)
| Concern | elacity-web does | Runtime requires | Mapping |
|---|---|---|---|
| Build output | Vite → static `build/` | Capsule webview hosts static | **direct-map** ✅ |
| Content/IPFS | hardcoded `https://ipfs.ela.city` (`utils/ipfs.ts`) | `/content/<cid>` only (P4) | adapter (single util) |
| Routing | BrowserRouter + server `_redirects` | hash router (no rewrite engine) | adapter |
| Service worker | VitePWA intercepts fetch | collides w/ capability routing | adapter (disable) |
| Backend reads | **154 RTK Query ops** → GraphQL | ~6–8 `/api/market/*` routes | **no-equivalent** (comments/likes/follows/governance/royalties have no route) |
| Chain reads | in-browser `JsonRpcProvider` ×34 + baked Infura key | gateway/chain-provider only | **conflict** (P3) |
| Signing/buy | **UI is the signer** (UA `sendTransaction`, EIP-7702, 43 `Contract()` sites) | unsigned `{to,data,value}` → wallet capsule | **conflict** (P16) |
| Embedded wallet/login | Particle MPC iframe + JWT in localStorage | sign routes OUT; `x-elastos-home-token` | **conflict** (P16) |
| Playback/CEK | MediaViewer renders + ddrm-reader in browser | renders nothing; `POST /api/viewers/open` | **conflict** (out of scope) |

## The three options
1. **Full port** (XL, multi-month + permanent fork-merge tax) — re-architect the app's authority spine; own a divergent fork against an actively-shipping branch. **Unanimous reject.**
2. **Adapter/facade** (XL) — emulate the ela.city GraphQL + image proxy behind the gateway. Fixes reads only; **still requires all the P16 signer surgery**; perpetual schema-drift liability. **Reject.**
3. **Hybrid — design donor** (S–M tokens, M–L cherry-port) — keep the vanilla shell as data/sign/content spine; transplant brand tokens + the two signature effects + CapsuleCard info-design. **Recommended.**

## Execution plan
| # | Step | Who | Effort |
|---|---|---|---|
| 1 | **CEO: lock the brand accent** (turquoise `#5edad9` vs shell `#5b8cff`) — gates everything below | CEO | S |
| 2 | Consolidate the two storefronts into one canonical Marketplace (P10) | Cursor | M |
| 3 | Wire + schema-test the BUILT `/api/market/*` routes; forbid prod mock fallback | Cursor | M |
| 4 | Transplant brand tokens → shell `:root` (`design-tokens.json` → `styles.css`) | build-here | S |
| 5 | `.glassy` + `.grad-text` vanilla CSS utilities (the two signature effects) | build-here | M |
| 6 | Cherry-port CapsuleCard info-design as framework-free HTML/CSS (exclude MediaViewer) | build-here | L |
| 7 | Shared `design-tokens.json` SSOT feeding both products | **DONE** | M |
| 8 | Field-mapping table: elacity GraphQL fragments → lean runtime Listing (DIRECT/ADAPTER/ABSENT) | build-here | M |

## Hard truths
- **"Signing is contained to 7 files" was false for this question.** The Particle *signer lib* is contained, but chain *authority* is diffuse: ~36 `@particle-network` imports, 43 `Contract()` sites, 34 `JsonRpcProvider`. A port intercepts all of them.
- **It "looks done," so a port feels like reuse. It isn't — it's re-architecting the authority model.** The thing that makes it impressive is exactly what conflicts with every runtime principle.
- **The shell is not a prototype to replace.** On design-system fundamentals it's already the better core; a full port would *lower* the bar while importing violations.
- **The flagship MediaViewer is out of scope (P16).** Don't let it drive the decision.
- **Data wiring is the critical path; the skin is cosmetic.** The UI renders empty until Phase-2 KID/metadata enrichment. **Wire the routes before polishing the skin** (steps 2–3 before 4–6).
- **No third storefront before consolidating the two existing ones (P10).**
