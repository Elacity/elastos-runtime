# Browser Provider Bake-Off

This document records the decision process for replacing or keeping the current
hosted Browser provider. It is not the whole Browser strategy. The Browser ABI
is already the source of truth:

```text
Browser capsule
  -> Runtime Browser open route
  -> Browser Engine Adapter
  -> hosted_remote_browser or selkies_gstreamer provider
  -> Runtime Net/Exit policy
```

No candidate gets a new Browser ABI. Each candidate must satisfy the same
`elastos.browser.display-session/v1` product-compositor contract.

## Product Direction

The performance path is native/local first, hosted second:

1. Local Launcher, desktop, Jetson, and mobile builds should use a native or
   microVM browser adapter behind the Browser/Net/Exit ABI. This gives the
   browser engine a real compositor, native input, native audio/video, and the
   lowest latency while still denying ambient network and routing off-box
   effects through Runtime Exit.
2. Hosted Home needs a remote-browser provider for pure web access. That path
   should use a proven remote-browser isolation or workspace-streaming system
   when possible, not an open-ended custom compositor tuning loop.
3. Selkies remains the self-hosted baseline/proof until a better hosted provider
   passes the same gates. It is not the default product answer simply because it
   currently runs.

This split follows the pattern proven by production systems: high-performance
browser UX usually depends on either a native endpoint surface or a mature
remote-browser/desktop streaming stack. ElastOS should keep the authority model
in Runtime, but should not re-invent browser streaming if an operator-approved
provider can satisfy the same ABI.

## Current Baseline

Selkies/GStreamer is the current hosted baseline:

- passes product compositor WebRTC for controlled media,
- exposes audio and video tracks,
- accepts datachannel input,
- reports the display coordinate size used by datachannel input,
- supports Runtime/provider address navigation, back, forward, and reload,
- keeps browser networking behind Runtime Exit with `direct_network=false`,
- supports the Runtime-mediated wallet bridge and Glide connect-wallet smoke.

It is not accepted as the final browser UX:

- YouTube embeds can return `Error 153`,
- YouTube watch pages can decode initial media and then pause around ad/sign-in
  or profile flows,
- the current hosted target is effectively one compositor/session,
- user-visible quality and latency still feel below a product browser.

Before running provider smokes, operators can check for stale browser proof
processes without stopping the live baseline:

```bash
node scripts/browser-experiment-cleanup.mjs
node scripts/browser-experiment-cleanup.mjs --apply
```

The helper is dry-run by default. With `--apply`, it only kills orphaned 1x1
Xvfb proof displays and removes exited Selkies target containers; running
containers are reported and preserved.

## Hosted Candidate Order

### 1. Kasm Workspaces / KasmVNC

Kasm Workspaces is the first hosted production comparison candidate because it
has documented session lifecycle APIs and explicit audio controls. The Runtime
still must not leak Kasm URLs, tokens, API keys, or session authority to
capsules; an operator control service must translate Kasm sessions into the
same ElastOS `product_compositor` receipt used by every other provider.

Required ElastOS shape:

- create sessions through Kasm's API from an operator control service,
- wait for the session to reach `running` before returning a Browser receipt,
- enable and prove audio rather than assuming a standalone VNC image has it,
- return `engine=hosted_remote_browser`,
- return `display_backend=kasm_workspaces_webrtc` or `kasmvnc_webrtc`,
- prove the same Runtime/provider navigation, media, wallet, and Glide gates.

Operator prerequisites:

- Provision Kasm Workspaces outside the Runtime tree and create a Developer API
  key with the permissions needed to create and inspect sessions on behalf of a
  Runtime principal.
- Configure `KASM_BASE_URL`, `KASM_API_KEY`, and `KASM_API_KEY_SECRET` only in
  operator environment or secret storage; never commit them.
- The operator control service must call Kasm's session APIs, wait for the
  session status to become running, and expose only an ElastOS
  `product_compositor` display receipt to Runtime. It must not leak Kasm API
  credentials, session tokens, or raw session URLs into capsules.
- Returning only Kasm's `kasm_url` is not an ElastOS Browser proof. The control
  service must adapt the running Kasm session to the Runtime-scoped
  `webrtc_remote_display` contract or fail closed; Browser UI must not embed a
  raw Kasm session URL as a hidden fallback.
- The Kasm-side session/cast configuration must enable audio explicitly
  (`allow_kasm_audio` / `kasm_audio_default_on` where applicable), then the
  ElastOS bake-off must prove decoded audio bytes. A visible audio toggle or
  config flag is not acceptance.
- `scripts/browser-kasm-control-service.mjs` is the fail-closed operator-side
  scaffold for this shape. It owns `request_kasm`, `get_kasm_status`, and
  `delete_kasm`, rejects URL-only sessions before calling the Kasm API, and
  delegates display to an explicit `product_display_bridge_socket` that must
  return the same `elastos.browser.engine.supervisor-result/v1` receipt as any
  other hosted provider.
- `scripts/browser-kasm-control-service-smoke.sh` proves the URL-only rejection,
  Kasm API lifecycle, product display bridge handoff, and session deletion on
  close against fake Kasm/display services. It is a contract smoke, not a real
  Kasm deployment proof.
- Then generate the Runtime-facing adapter config:

```bash
ELASTOS_BROWSER_KASM_CONTROL_CONFIG='{
  "schema": "elastos.browser.kasm-control.config/v1",
  "control_socket_path": "/run/elastos/kasm-workspaces-control.sock",
  "kasm_base_url": "https://kasm.example.invalid",
  "api_key": "...",
  "api_key_secret": "...",
  "user_id": "...",
  "image_id": "...",
  "product_display_bridge_socket": "/run/elastos/kasm-display-bridge.sock"
}' node scripts/browser-kasm-control-service.mjs

node scripts/browser-hosted-product-operator-config.mjs \
  --candidate kasm-workspaces \
  --out-dir /opt/elastos/kasm-workspaces \
  --supervisor-program /home/wau/elastos-runtime/scripts/browser-hosted-product-supervisor.mjs \
  --control-socket /run/elastos/kasm-workspaces-control.sock
```

### 2. BrowserBox

BrowserBox is the high-performance RBI comparison candidate because it is
browser-specific and advertises 60 FPS clientless remote browsing. It is also
beta/commercial software, so it must not be silently vendored or made the open
default. Use it early when an operator has already accepted the license and
wants to test the lowest-latency hosted RBI path; otherwise run Kasm first.

Required ElastOS shape:

- expose BrowserBox through an operator-owned control service,
- return `engine=hosted_remote_browser`,
- return `display_backend=browserbox_webrtc`,
- return `backend_class=product_compositor`,
- prove audio, video, input, and `direct_network=false`,
- route network egress through Runtime Exit or an operator-approved Exit
  provider,
- keep wallet access through `elastos://wallet/*`, not BrowserBox internals.

Operator prerequisites:

- Install and operate BrowserBox outside the Runtime tree using the official
  `bbx` path (`bbx install`, `bbx certify`, `bbx setup`, `bbx run`).
- Set `BROWSERBOX_LICENSE_CONFIRMED=1` only after the operator has confirmed
  licensing. This variable records operator intent; it is not proof of
  acceptance.
- Provide a Unix control socket that adapts BrowserBox sessions to
  `elastos.browser.engine.supervisor-result/v1` with
  `display_backend=browserbox_webrtc`, `backend_class=product_compositor`,
  `audio=true`, `video=true`, and `direct_network=false`.
- Then generate the Runtime-facing adapter config:

```bash
node scripts/browser-hosted-product-operator-config.mjs \
  --candidate browserbox \
  --out-dir /opt/elastos/browserbox \
  --supervisor-program /home/wau/elastos-runtime/scripts/browser-hosted-product-supervisor.mjs \
  --control-socket /run/elastos/browserbox-control.sock
```

### 3. Selkies Retained Or Rejected

Selkies stays only if it beats or matches the other candidates on the same
machine gates and manual UX review. Otherwise it remains a useful reference for
the Runtime Browser ABI and product-compositor contract.

## Machine Gate

Before claiming the Browser objective is complete, run the objective audit for
the product path that actually passed. Do not pass placeholder paths for the
path you did not prove.

```bash
node scripts/browser-objective-audit.mjs \
  --hosted-bakeoff /path/to/accepted-hosted-bakeoff.json \
  --manual-ux /path/to/manual-ux.json

node scripts/browser-objective-audit.mjs \
  --native-preflight /path/to/accepted-native-preflight.json \
  --manual-ux /path/to/manual-ux.json
```

This audit intentionally fails until at least one product provider has real
media proof and the manual UX evidence is present. It is the completion gate,
not a replacement for the candidate-specific smokes.
Native evidence must be the typed
`elastos.browser.native-target-preflight/v1` receipt emitted by
`scripts/browser-native-target-preflight.sh`, and product media readiness
requires both `native_audio_proven=true` and `native_video_proven=true`.
Use `--artifact-out` for the exact native machine proof file that the manual UX
report will hash:

```bash
scripts/browser-native-target-preflight.sh \
  --out-dir /opt/elastos/native-browser \
  --browser-program <chromium-or-cef> \
  --native-audio \
  --native-video \
  --require-native-media \
  --artifact-out /opt/elastos/native-browser/native-preflight.json
```

For a quick target-host readiness check before generating the full native config
bundle, run:

```bash
node scripts/browser-native-host-capability.mjs \
  --browser-program <chromium-or-cef> \
  --require-product-native
```
Generate and validate the manual UX evidence with:

```bash
node scripts/browser-manual-ux-report.mjs \
  --template \
  --machine-artifact /path/to/accepted-hosted-or-native-proof.json \
  > manual-ux.json
node scripts/browser-manual-ux-report.mjs --input manual-ux.json
```

The manual report must include `machine_artifact.schema`,
`machine_artifact.sha256`, and `machine_artifact.path` for the exact accepted
hosted bake-off or native preflight artifact reviewed by the human tester. The
objective audit rejects manual UX evidence whose path, schema, or hash does not
match the accepted machine proof passed to the audit. The template command
pre-fills provider and target from the machine artifact when those fields are
available; reviewers should only adjust them if the visible runtime target
differs from the artifact metadata.
For hosted WebRTC candidates, the report must also fill evidence text for
`display_session_audio_advertised`, `audio_unlock_gesture`,
`remote_audio_unmuted_status`, and `received_audio_evidence`; checkmarks alone
are not sufficient audio evidence.
Hosted bake-off artifacts also include `manual_ux_schema` and
`manual_ux_checks` from the same shared checklist used by
`browser-manual-ux-report.mjs`. Treat those fields as review guidance, not as a
substitute for a signed-off manual UX report.

The audit itself has a regression smoke:

```bash
scripts/browser-objective-audit-smoke.sh
```

That smoke uses temporary fake fixtures only to verify the audit rejects
declaration-only native media, shallow hosted `ok=true` artifacts, and hosted
artifacts that skip YouTube stress. It also proves detached manual UX evidence
is rejected and hash-bound manual UX evidence can satisfy the final gate when
paired with a strict machine artifact. It is not product acceptance evidence.

Before running the bake-off, run the candidate preflight:

```bash
node scripts/browser-provider-decision-report.mjs

node scripts/browser-hosted-product-operator-config.mjs \
  --candidate browserbox \
  --out-dir /path/to/browserbox-config \
  --supervisor-program /path/to/browser-hosted-product-supervisor.mjs \
  --control-socket /path/to/browserbox-control.sock

node scripts/browser-hosted-provider-preflight.mjs \
  --candidate browserbox \
  --adapter-config /path/to/browserbox-config/browser-engine-adapter.json
```

When the preflight is ready, its `next_command` must include
`--artifact-out <hosted-bakeoff.json>` so the later manual UX report can hash
the exact machine proof. A preflight command without an artifact output is not a
completion path.

Use `--candidate selkies`, `--candidate browserbox`,
`--candidate kasm-workspaces`, or `--candidate kasmvnc` rather than hand-matching
engine kinds and display backend strings. The presets still only generate the
Runtime-facing config; they do not install or license a vendor product.
The decision report reads the live adapter/service state and the objective
audit, then prints the current recommendation without launching a browser or
stopping any service. It also runs the native host capability probe and emits a
top-level `goal_status`, a structured `next_action`, a `blocked_by` list, and a
hosted-candidate readiness matrix: Selkies uses the live adapter config, while
BrowserBox/Kasm candidates are marked blocked until an operator supplies a
matching generated adapter config and control socket. When a non-Selkies
candidate config is not supplied, the hosted-candidate readiness matrix remains
blocked rather than pretending a vendor backend exists. The report generates a
temporary preset config only long enough to run preflight,
then removes it and reports the real blockers such as missing control socket,
license confirmation, CLI, or API credentials.
Generated placeholder socket paths must not be shown as operator instructions;
they are normalized to "operator control socket not provisioned" until a durable
candidate control socket is configured.
For the current Selkies baseline, the report also reads the control-service
status. If `single_session=true` and `active_pages>0`, do not run a product
bake-off against that target; close the active Browser page or use a separate
provider instance. In that state, Selkies is marked `ready_for_bakeoff=false`
even if its preflight passed; the underlying preflight result is preserved as
`preflight_ready_for_bakeoff` so operators can distinguish configuration
readiness from live-session availability. An active single-session target is a serialization limit,
not an audio acceptance result.

When proof artifacts are supplied, the decision report summarizes them
explicitly. `--hosted-bakeoff` emits a top-level `hosted_bakeoff` summary and
adds `hosted_bakeoff_rejected` when the candidate gate or YouTube/product media
stress fails. `--native-preflight` emits a top-level `native_preflight` summary
and adds `native_preflight_rejected` when the native preflight is structurally
successful but does not prove required native audio/video readiness. This keeps
artifact failures visible as evidence, not hidden behind generic missing-proof
language.
If the supplied artifacts make `scripts/browser-objective-audit.mjs` accepted,
the decision report must clear unrelated live-host and candidate-readiness
blockers from `blocked_by` and set `next_action=keep_accepted_browser_artifacts`.
Accepted evidence may come from a different native/hosted target; this server's
missing compositor, audio service, network namespace, or vendor credentials must
not be reported as blockers after artifact-bound acceptance.

Keep the decision-report smoke green before handing status to an operator:

```bash
scripts/browser-provider-decision-report-smoke.sh
```

The smoke proves the report has the expected schema, a structured
`goal_status`/`next_action`, visible `blocked_by` entries, and candidate
readiness for Selkies, Kasm Workspaces, BrowserBox, and KasmVNC. It also proves
that rejected hosted and native artifacts produce explicit blocked evidence,
that accepted native/manual artifacts clear stale live-host blockers and route
to preserving accepted artifacts, that a busy single-session Selkies target is
not reported as bake-off-ready, and
that the structured next action is operator-owned, blocked, and points to
closing/isolating Selkies or using a separate provider instance instead of more
Selkies tuning. In the current non-accepted state it must exit non-zero and keep
`audio_product_proven` plus `manual_user_acceptance` visible. If that smoke
fails, do not trust prose status or run another provider bake-off until the
report contract is fixed.

For Browser proof-tool review, use the focused gates directly:

```bash
scripts/browser-objective-audit-smoke.sh
scripts/browser-provider-decision-report-smoke.sh
scripts/browser-provider-runbook-smoke.sh
scripts/browser-hosted-product-config-smoke.sh
```

These gates keep the objective audit, provider-decision report, runbook, and
hosted-product config structured. The live decision report and objective audit
must either exit accepted with matching accepted state or fail closed with the
current audio/manual UX blockers visible.

Use the runbook view when handing this to an operator:

```bash
node scripts/browser-provider-runbook.mjs
```

When a hosted/native proof or manual UX report already exists, generate the
runbook from those exact artifacts instead of relying on live/default status:

```bash
node scripts/browser-provider-runbook.mjs \
  --hosted-bakeoff /path/to/hosted-bakeoff.json \
  --manual-ux /path/to/manual-ux.json

node scripts/browser-provider-runbook.mjs \
  --native-preflight /path/to/native-preflight.json \
  --manual-ux /path/to/manual-ux.json
```

Do not combine these proof flags with `--decision-report`. A precomputed
decision report is already the source of truth; mixing it with newer artifacts
would make the operator handoff stale.

The runbook renders the objective checklist, structured next action, and the
same `blocked_by` summary before candidate-specific commands, so missing product
audio proof, missing manual UX, Selkies session serialization, native-host
blockers, and Kasm or BrowserBox prerequisites are visible before anyone starts
another long provider run. It also renders a `Current Host Stop Condition`
section. In the current hosted-server state, that section must say the host is
not accepted as product Browser proof, must not keep tuning the running Selkies
baseline as product architecture, and must route product proof to either an
operator-owned hosted candidate or a native target with browser/compositor/audio
and network isolation.

When the live Selkies baseline is busy, the runbook includes the active page id
and an explicit close helper:

```bash
node scripts/browser-selkies-close-page.mjs --control-socket <socket> --page-id <page-id>
node scripts/browser-selkies-close-page.mjs --control-socket <socket> --page-id <page-id> --confirm-close
```

The first command is dry-run only. The second command mutates the live Selkies
session and is operator-owned; the runbook never runs it automatically.
`scripts/browser-selkies-close-page-smoke.sh` proves the helper requires the
explicit confirmation flag and fails closed when the control socket is missing.

The preflight is intentionally fail-closed. It does not install vendor software
or treat CLI presence as proof. It checks that the candidate is exposed through
the expected Browser Engine Adapter kind, display backend, and operator control
socket. BrowserBox additionally requires explicit operator license confirmation;
Kasm Workspaces requires operator API credentials. Passing preflight only means
the candidate can be tested; it is not acceptance.

Every candidate must run:

```bash
scripts/browser-hosted-provider-bakeoff.sh \
  --candidate <candidate-id> \
  --adapter-config /path/to/browser-engine-adapter.json \
  --cdp-endpoint http://127.0.0.1:<private-cdp-port> \
  --artifact-out /path/to/hosted-bakeoff.json
```

Do not use `--skip-youtube` for acceptance. That flag is a partial diagnostic
escape hatch only; the bake-off artifact remains rejected until YouTube/media
stress runs and passes. Use `--artifact-out` for the exact machine proof file
that the manual UX report will hash.

The script wraps:

- `scripts/browser-hosted-provider-candidate-smoke.sh`
- `scripts/browser-hosted-product-webrtc-smoke.sh` against YouTube stress

The candidate gate verifies:

- product compositor display, not CDP/JPEG proof frames,
- WebRTC audio track, video track, datachannel input, and connected ICE,
- controlled media playback with decoded audio/video bytes, at least a
  five-second hold, rendered size, and drop-ratio quality floor,
- remote compositor resize proof, so stream dimensions match the Browser panel
  after a window-ratio change instead of relying on local video stretch,
- Runtime/provider navigation: address navigate, back, forward, reload,
- Runtime-mediated wallet bridge,
- Glide connect-wallet flow,
- `direct_network=false`.

The YouTube stress gate is intentionally stricter than controlled media. A
candidate that fails YouTube may still be useful research, but it is not the
default product browser.

On 2026-05-13, a patched Selkies bake-off with explicit display coordinates and
click/keyboard activation still failed the YouTube stress gate: candidate gates
passed, the page loaded, audio/video bytes decoded, and media time advanced to
2.750s, but playback ended paused before stable audible playback. The rejected
artifact is `/tmp/elastos-browser-bakeoff/selkies-patched-hosted-bakeoff.json`.
Treat that as evidence against more Selkies tuning as the default branch task,
not as evidence that the Browser ABI is wrong.

The live Selkies target also failed the dedicated resize proof on 2026-05-13:
`scripts/browser-hosted-product-webrtc-smoke.mjs --resize-width 1000
--resize-height 700` timed out waiting for the remote video stream to adopt the
requested compositor size. The target wrapper now defaults Selkies to dynamic
resolution instead of manual fixed-size mode, enables remote resize explicitly,
and keeps manual mode available only through operator opt-in. A follow-up live
test still failed to observe a resized video stream. A 1280x720 Selkies
surface then produced zero decoded WebRTC frames, so the live operator baseline
returned to the known-rendering 1920x1080 stream and uses Chromium
`--force-device-scale-factor=1.5` to match the Home window's normal-browser
CSS viewport while the dynamic resize gate remains open. A later live check
proved arbitrary CDP viewport resize can leave blank right/bottom capture
regions inside the fixed Selkies stream, so the current baseline keeps a stable
1280x720 CSS viewport and treats true responsive compositor resize as unaccepted
provider work.
Selkies remains a hosted proof/bake-off baseline, not accepted as
normal-browser-equivalent UX, until the provider passes the dynamic viewport
resize gate.

## Manual UX Gate

Machine gates are necessary but not sufficient. A candidate must also pass a
manual hosted Home review:

- typing in the address bar does not fight polling or lose focus,
- page typing latency is acceptable,
- scrolling and click fidelity feel browser-like,
- resizing preserves page ratio and page scale instead of stretching or staying
  fixed-size,
- hosted WebRTC providers show `audio=true`, unlock remote audio through an
  explicit user gesture, report an unmuted/remote-audio-enabled status, and
  expose received-audio evidence,
- YouTube plays with audible audio,
- Glide connects wallet from the visible Browser UI,
- no raw wallet, node, filesystem, or host network authority is exposed to web
  pages,
- closing the Browser cleans up the remote session.

## Decision Rule

Pick the simplest candidate that passes both gates:

1. Kasm Workspaces/KasmVNC if it passes with audio and does not require leaking
   Kasm session authority into capsules.
2. BrowserBox if licensing and operator packaging are acceptable and it passes.
3. Selkies only if it passes the same UX and media gates after measured tuning.

If none pass, the hosted remote-browser path remains a research or operator-only
deployment mode, and the product path should shift to native/microVM Browser
adapters using the same Browser/Net/Exit ABI. Do not add a hidden fallback from
one display path to another.

## Production Precedents

- Kasm Workspaces documents remote, containerized browser isolation rendered in
  the user's browser, plus a Workspaces flow for launching/resuming sessions:
  <https://docs.kasm.com/docs/latest/guide/browser_isolation/index.html>.
  KasmVNC alone is not sufficient unless the ElastOS gate proves audio.
  The current Developer API documents API key authentication, permissions,
  `request_kasm`, `get_kasm_status`, and `kasm_url` session links:
  <https://docs.kasm.com/docs/developers/developer_api/index.html>.
  The API also documents `allow_kasm_audio` / `kasm_audio_default_on` controls:
  <https://www.kasmweb.com/docs/latest/developers/developer_api.html>.
- BrowserBox advertises clientless remote browser isolation at 60 FPS and
  embeddable sessions, but it is beta/commercial software and must be operator
  licensed before use: <https://browserbox.io/>.
  The public repository documents the `bbx` CLI install/run path:
  <https://github.com/BrowserBox/BrowserBox>.
- Cloudflare Browser Isolation executes active web content in an isolated remote
  browser as part of a Zero Trust/SWG product. It proves the category, but it is
  a managed cloud product rather than an ElastOS provider implementation:
  <https://developers.cloudflare.com/cloudflare-one/remote-browser-isolation/>.
- Apache Guacamole and noVNC prove mature browser-based remote display/input
  gateways, but they are remote desktop protocols, not full modern browser
  product answers for YouTube-quality media by themselves:
  <https://guacamole.apache.org/doc/1.5.2/gug/guacamole-architecture.html> and
  <https://github.com/novnc/noVNC>.
- Citrix Browser Content Redirection is the important cautionary precedent:
  enterprise systems offload selected heavy browser rendering, including
  YouTube-class workloads, to client-side browser engines to reduce server load.
  That supports the ElastOS native/local adapter priority for product quality:
  <https://docs.citrix.com/en-us/citrix-virtual-apps-desktops/multimedia/browser-content-redirection.html>.
