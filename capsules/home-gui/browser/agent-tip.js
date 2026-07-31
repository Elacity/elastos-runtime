/* Single cache-bust tip for home-gui browser assets.
 *
 * Bump procedure:
 * 1. Change TIP below (e.g. home-YYYYMMDDxx).
 * 2. Replace prior tip string across capsules/home-gui/browser (imports + index.html).
 * 3. Set homeGuiAssetVersion in scripts/home-entropy-check.mjs to the same string.
 * 4. Run: node scripts/home-entropy-check.mjs
 * 5. rsync capsule to $MAC_TEST_HOME/Library/Application Support/elastos/capsules/home-gui/
 * 6. Hard-refresh Home at http://localhost:61180/apps/home/
 *
 * Track A product acceptance (§AK) — manual checklist before claiming AI readiness:
 * [ ] Preview banner visible in Agent room (not live inference)
 * [ ] Settings Get / Mine copy does not imply real weight downloads
 * [ ] Usage page says preview / not live metering
 * [ ] Empty state teaches: locality · tools at zero · grants ask once
 * [ ] Grant card: Deny / Allow once; Allow once one-shot; no Capsule call
 * [ ] Offline flips inference status copy (still preview path)
 * [ ] Brand reads Agent · On this Home (Home harness, not CLI operator)
 * [ ] Shelf/Home enter-leave still works; no ambient grant path
 *
 * w1 live chat acceptance (§AL) — manual checklist with llama-server running:
 * [ ] Status shows Live · local model — <name> only after real probe
 * [ ] Prompt answered by the local model (full reply, progressive reveal)
 * [ ] Reasoning (<think>/reasoning_content) lands in the Thinking disclosure
 * [ ] Kill llama-server → next turn falls back to mock, banner back to Preview
 * [ ] Model switcher / Mine shows the real GGUF as Live; stubs stay preview
 * [ ] Grant cards unchanged — live chat mints no tool or capsule authority
 */

export const TIP = "home-20260728ag";
