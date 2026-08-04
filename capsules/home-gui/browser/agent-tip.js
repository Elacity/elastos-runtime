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
 * w1 live chat acceptance (§AL) — llama-server OR Sparks OpenAI-compat:
 * [ ] Status shows Live only after real probe (llama healthy or ai ping)
 * [ ] Prompt answered by the live model (full reply, progressive reveal)
 * [ ] Reasoning (<think>/reasoning_content) lands in Thinking; content:null
 *     Flash replies still show text (reasoning fallback)
 * [ ] Kill backend → next turn falls back to mock, banner back to Preview
 * [ ] Model switcher / Mine shows the live model; stubs stay preview
 * [ ] Grant cards unchanged — live chat mints no tool or capsule authority
 *
 * Wave 1 chat-feel (§AM):
 * [ ] Edit user message → truncate trailing turns → restream Live
 * [ ] Per-message Delete with confirm; persists session.agent
 * [ ] Markdown tables/lists/links/fences + Copy
 * [ ] Live stream paints markdown (not only on complete)
 * [ ] Connecting / Generating status; Jump to latest when scrolled up
 * [ ] Stop persists partial; no stuck busy; queue can drain
 * [ ] Regenerate replaces last agent turn (does not append duplicate)
 * [ ] Archive soft-hide; Export/Import JSON; search titles+bodies
 * [ ] Live errors honest (not silent mock without label)
 *
 * Wave 2 prompt/params (§AQ):
 * [ ] Settings → Prompt edits system prompt; Reset restores default
 * [ ] Temperature + max tokens pass through Live stream body
 * [ ] Prefs persist across reload via session.agent
 * [ ] Settings → backends PUT validates OpenAI-compat URLs (no SSRF)
 * [ ] Model menu pair rows come from GET /agent/backends
 * [ ] Auto-title strips markdown / takes first sentence
 *
 * Wave 3 attach/context (§AR):
 * [ ] Attach menu lists Desktop objects + device files
 * [ ] Text files size-capped into Live message; Desktop needs grant note
 * [ ] Workbench Library shows Desktop objects (not fake name chips)
 *
 * Wave 4 honesty (§AS):
 * [ ] Live turn records usage (upstream or estimate); Usage page non-zero or honest omit
 * [ ] Message meta shows latency · tokens · live/est
 * [ ] Usage heatmap empty until Live — never invents preview activity
 * [ ] Stream/probe failures surface on status strip; healthy Live stays quiet
 * [ ] usageTurns persist via session.agent
 *
 * Sparks Flash dogfood: OLLAMA_URL=http://192.168.1.147:8888/v1/chat/completions
 * OLLAMA_MODEL=deepseek-v4-flash — see ~/elastos-mac-test-home/SPARK-FLASH-START.md
 */

export const TIP = "home-20260804as";
