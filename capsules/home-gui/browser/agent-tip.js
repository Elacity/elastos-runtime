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
 * [ ] Progressive Markdown on freeze/tail (not a full-document reparse)
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
 * Wave 5 tools (§AT):
 * [ ] library.read creates Inbox capability request (not mock mint)
 * [ ] Open Inbox / Deny cancel; Approve once → Desktop list on card
 * [ ] wallet.sign remains preview mock
 *
 * Wave 5.02 ADE panels (§AU):
 * [ ] Diff/Browser/Term stay disabled without ADE grant
 * [ ] Copy says needs ADE — no Open Terminal / Pyodide identity
 *
 * Wave 6.01 Local Library KB (§AV):
 * [ ] After Inbox library.read ready, Desktop attach extracts cited text
 * [ ] Live turn includes On-Home Library path listing when granted
 * [ ] Extract fails closed without grant / for binary files
 * [ ] No ambient /api/provider/object from home-gui
 *
 * Wave 6.02 Web search (§AW):
 * [ ] “search the web / online” → fail-closed Exit/net card (no scrape)
 * [ ] Gateway POST /agent/tools/web.search returns unavailable + citations:[]
 *
 * Wave 7 notes (§AY):
 * [ ] Settings → Prompt → Notes on this Home persist via session.agent
 * [ ] Live system prompt includes notes when set
 * [ ] library heuristics include desktop / what’s on my
 *
 * Wave 7.02–03 fork/tags/vision (§AZ):
 * [ ] Session ··· → Fork chat creates copy with Fork badge; persists
 * [ ] Edit tags… shows #tags on session row; persist reload
 * [ ] Image attach → vision · unsupported (no fake caption)
 *
 * Thinking markdown (§BB) + edit/table hang (§BC):
 * [ ] Edit user message: full-width textarea, no blue focus ring
 * [ ] Bold renders without visible ** markers
 * [ ] Markdown tables stream without Page Unresponsive; final paint is a real table
 * [ ] Stream paint is plain text; final paint is a real table
 * [ ] Chats survive refresh via host session.agent (no browser storage); replies flush on turn complete
 *
 * Thinking markdown (§BB):
 * [ ] Thinking block renders bold/lists/fences (not raw **)
 * [ ] Mid-stream thinking paints markdown; Stop keeps raw source
 *
 * Sparks Flash dogfood: OLLAMA_URL / OLLAMA_MODEL via local SPARK-FLASH-START recipe
 * (LAN endpoints stay operator-local — not committed).
 *
 * Studio Generate (§BD):
 * [ ] Agent → Studio shows CREATIVE profile / Generate ready
 * [ ] Prompt + duration → job poll → mp4 plays (needs CREATIVE_URL + 2×)
 * [ ] Character skill: face still → Ref2VA when Comfy up (CREATIVE_COMFY_URL)
 * [ ] Character + optional voice clip → ref_audios / &lt;Audio 1&gt;
 * [ ] Character honest when Comfy down (2× occupying spark3)
 * [ ] No client-supplied upstream URL (SSRF-closed gateway)
 * [ ] in-Home &lt;video&gt;: force video/mp4 blob + Save clip (ac)
 * [ ] N-GPU picker 1×/2×/4× from status.scale; Character locked to 1× (ad)
 * [ ] Character refs hidden under Generate (ae CSS)
 * [ ] Allocator prepare on submit (af)
 * [ ] Storyboard skill: shots → EDITING RULE compose → Generate (ag)
 *
 * Wave 4 streaming calmness (§AN):
 * [ ] Stop aborts transport on pointerdown; UI reconciles after
 * [ ] Composer stays typed/paste/scrollable while generating
 * [ ] Generating… clears at model-done, not after markdown
 * [ ] Hidden tab buffers; visible tab catches up once
 * [ ] Degraded mode: rich → simple → plain if render > 12ms
 * [ ] Copy uses canonical markdown, not DOM HTML
 * [ ] Selection / citation click pauses auto-follow
 * [ ] Partial answer survives errors; no wipe to "something went wrong"
 *
 * Wave 5 ambient progress (§AO):
 * [ ] Status is a reducer (phase+subject), not scraped Think tokens
 * [ ] Ambient line appears after ~500ms; fast answers skip it
 * [ ] Expandable milestones; raw thinking stays behind the existing toggle
 * [ ] Progress recedes once the answer is useful; completes as ✓ N steps
 *
 * Stream invariant (§AP):
 * [ ] tailChars stays ~200–1200 while answerChars grows
 * [ ] avgRenderMs, p95, max, slopeRenderPer10kChars ≈ 0; corrAnswerMarkdown ≈ 0
 * [ ] longTasks ~0, slowFrames low; tailInvariantViolations = 0
 * [ ] Markdown commits on freeze/tail only; unhealthy finalize keeps plain text
 * [ ] Large paste becomes a Composer Input Object (chip + editor); canSend from parts
 * [ ] Context Packing v1: next-turn cost ≈ current intent + relevant state, not lifetime transcript
 * [ ] One compiler: gather → compileLiveContext → serializeFlashContext → stream
 * [ ] turnId ≠ providerRunId; TurnManifest persisted CREATED→…→COMPLETED/FAILED
 * [ ] Compression may reduce density; it may never increase authority
 * [ ] TurnManifest persisted (input versions + RunConfig + ContextManifest + requested/effective)
 * [ ] Flash Med is honest: High/Low disabled until the offer honors effort
 * [ ] Think-storm: 558/5k/50k reasoning events stay coalesced; lastTurn never stuck streaming
 */

export const TIP = "home-20260814a";
