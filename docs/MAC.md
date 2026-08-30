# Mac Runtime Notes

The current Mac path is source-home staging on Apple silicon
(`darwin-arm64`). It is the path used for local Mac verification while the
public installer and `.dmg` packaging are still separate release goals.

## From Fresh Mac To Home

Install the host tools first:

```bash
xcode-select --install
brew install node e2fsprogs coturn ffmpeg
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup target add wasm32-unknown-unknown
```

Setup imports `ffmpeg`/`ffprobe` for the media-provider prerequisite and
fail-closed rejects any group-writable ancestor of the resolved binaries. If
setup stops with `ffmpeg prerequisite parent is unsafe`, tighten the Homebrew
directory on that path (commonly `chmod g-w /opt/homebrew/Cellar`) and rerun.

Then get the repo and build/install the source-home runtime into an isolated
Mac test home:

```bash
mkdir -p "$HOME/Code"
git clone https://github.com/Elacity/elastos-runtime.git "$HOME/Code/elastos-runtime"
cd "$HOME/Code/elastos-runtime"

export MAC_TEST_HOME="$HOME/elastos-mac-test-home"
export USER_HOME="$HOME"

HOME="$MAC_TEST_HOME" \
CARGO_HOME="$USER_HOME/.cargo" \
RUSTUP_HOME="$USER_HOME/.rustup" \
PATH="$USER_HOME/.cargo/bin:/opt/homebrew/bin:$PATH" \
scripts/setup-source-home.sh
```

`setup-source-home.sh` builds the runtime server, native providers,
WASM Components, runtime-projection browser assets, Browser helper
scripts, Browser source-home provider config, the macOS Browser VZ supervisor,
and the local Kubo backend used by Library and Documents publish. It installs
the stable Runtime under the source-home data root at `bin/elastos`, writes the
owner-only `receipts/source-home-installation.json` receipt, and signs the
installed VZ supervisor on macOS.

For Home/passkey use without Browser VM proof artifacts, start the stable
installed gateway through the restart helper:

```bash
scripts/mac-source-home-restart.sh \
  --test-home "$MAC_TEST_HOME" \
  --addr localhost:61180

curl -fsS http://localhost:61180/api/health
open http://localhost:61180/apps/home/
```

On first launch, Home shows `Set up Home`. Enter a passkey name, choose
`Create admin passkey`, and complete the macOS passkey prompt. The first
passkey becomes the Home admin. Later launches show `Sign in` / `Use passkey`.
Use `http://localhost:61180/apps/home/`; do not switch the visible origin to
`127.0.0.1`, because passkey RP/origin state is bound to the browser origin.

## Browser VM Artifacts

The Browser capsule is VM-backed on Mac. A clean Browser setup needs these
artifacts in the source-home data dir before Browser VM proof can be claimed:

```text
$MAC_TEST_HOME/Library/Application Support/elastos/bin/vmlinux
$MAC_TEST_HOME/Library/Application Support/elastos/bin/initrd
$MAC_TEST_HOME/Library/Application Support/elastos/browser-vm/rootfs.ext4
$MAC_TEST_HOME/Library/Application Support/elastos/browser-vm/browser-vm-rootfs-manifest.json
```

These are Linux guest artifacts. Build them on a Linux/CI/operator machine with
`scripts/build/build-browser-vm-rootfs.sh`, or install a reviewed prebuilt
artifact set. After copying them into the paths above, rerun:

```bash
HOME="$MAC_TEST_HOME" \
CARGO_HOME="$USER_HOME/.cargo" \
RUSTUP_HOME="$USER_HOME/.rustup" \
PATH="$USER_HOME/.cargo/bin:/opt/homebrew/bin:$PATH" \
scripts/setup-source-home.sh

HOME="$MAC_TEST_HOME" scripts/browser-vm-artifact-preflight.sh
```

`ok=true` or `local_substrate_artifacts_ready=true` is the artifact gate for
local Browser VM launch readiness. If the rootfs sidecar manifest is missing or
its SHA-256 does not match the rootfs image, the Browser setup is not clean.

With Browser artifacts installed, use the restart helper for normal Mac staging
instead of hand-starting the gateway:

```bash
scripts/mac-source-home-restart.sh \
  --test-home "$MAC_TEST_HOME" \
  --addr localhost:61180
```

Do not set `HOME="$MAC_TEST_HOME"` when calling
`scripts/mac-source-home-restart.sh`; pass `--test-home` or `MAC_TEST_HOME`
instead. The restart helper verifies Home hash parity and Browser helper
freshness across source, installed scripts, initrd, and rootfs before it starts
the gateway. By default, it writes the owner-only active receipt to the stable
data-root `receipts/mac-source-home-restart.json` path.

Mac Browser staging uses Apple Virtualization.framework for the Browser VM
substrate. The installed `browser-vz-engine-supervisor` binary must be signed
with the `com.apple.security.virtualization` entitlement before it can validate
or start a VM.

`scripts/setup-source-home.sh` signs the installed helper automatically on
macOS. To sign a rebuilt helper manually:

```bash
scripts/dev/sign-elastos-vz/sign.sh \
  "$MAC_TEST_HOME/Library/Application Support/elastos/bin/browser-vz-engine-supervisor"
```

## Browser VM Proof

Restart the Mac source-home gateway and verify Home hash parity plus Browser
VM helper freshness with:

```bash
scripts/mac-source-home-restart.sh \
  --test-home "$MAC_TEST_HOME" \
  --addr localhost:61180
```

Then collect a machine proof with:

```bash
scripts/browser-mac-vm-proof.sh --artifact-out /tmp/elastos-browser-mac-vm-proof.json
```

The handoff wrapper can do both steps for a fresh local proof:

```bash
MAC_TEST_HOME="$MAC_TEST_HOME" \
scripts/browser-mac-vm-acceptance-handoff.sh \
  --restart-source-home \
  --proof-out /tmp/elastos-browser-mac-vm-proof.json
```

The proof checks local Home HTTP, installed/source Home hash parity, Browser VM
control status, embedded WebRTC video/input/navigation, ela.city page
diagnostics, image loading, clean VM page shutdown, and explicit
`quality_gates` for remote-video readiness, navigation timing, decoded/dropped
frames, device pixel ratio, viewport size, and panel/video geometry. It
produces `elastos.browser.mac-vm-proof/v1` and intentionally records
`manual_acceptance.status=not_recorded`; it does not replace hash-bound manual UX,
product audio, or authenticated ela.city edit-profile acceptance.
To prove a non-default Browser viewport/zoom geometry, set the expected viewport
on the proof command; the wrapper passes those dimensions through to the
virtual-auth Browser open request unless the lower-level
`HOME_VIRTUAL_AUTH_BROWSER_OPEN_VIEWPORT_WIDTH/HEIGHT` variables are explicitly
set:

```bash
ELASTOS_BROWSER_MAC_VM_EXPECT_VIEWPORT_WIDTH=1000 \
ELASTOS_BROWSER_MAC_VM_EXPECT_VIEWPORT_HEIGHT=700 \
scripts/browser-mac-vm-proof.sh --artifact-out /tmp/elastos-browser-mac-vm-proof-resize.json
```

When the installed Browser VM control service has the current status contract,
the proof also records `vm_control.before.started_at` /
`vm_control.after.started_at` and `uptime_ms` so reviewers can bind the Browser
VM media proof to a concrete control-service run after restart. Final Mac VM
acceptance requires `vm_control.restart.fresh_after_restart=true`,
`vm_control.after.started_at`, and a positive bounded
`vm_control.after.uptime_ms`; older or stale machine artifacts without that
status contract remain diagnostics, not acceptance proof.

To prove cookie/localStorage/profile reset for the virtual test principal after
the proof pages have closed, opt in explicitly:

```bash
ELASTOS_BROWSER_MAC_VM_PROFILE_RESET_PROOF=1 \
scripts/browser-mac-vm-proof.sh --artifact-out /tmp/elastos-browser-mac-vm-proof-reset.json
```

The reset proof uses `/api/apps/browser/profile/reset`, removes only the active
principal's Browser VM profile disk under the principal-owned localhost root,
refuses live Browser pages,
and returns a receipt that does not expose the profile key, principal id, or
disk path. Mac VM acceptance requires `removed_profile_disk=true`; a reset that
finds no principal-owned profile disk is diagnostic, not proof that cookies and
localStorage were cleared.

After logging into ela.city inside the Mac Browser VM profile, an operator can
ask the proof to click sanitized authenticated controls and collect post-click
diagnostics before the page is closed:

```bash
ELASTOS_BROWSER_MAC_VM_PROOF_AUTH_PROFILE="$HOME/.local/share/elastos/mac-browser-vm-proof-auth" \
HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTIC_CLICK_TEXT_RE='Profile=>Edit Profile' \
HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTIC_CLICK_EXPECT_TEXT_RE='Edit Profile' \
scripts/browser-mac-vm-proof.sh --artifact-out /tmp/elastos-browser-mac-vm-proof.json
```

The default proof uses a disposable virtual passkey principal and removes it at
the end, so it will not see ela.city cookies from a human's normal Home profile.
Set `ELASTOS_BROWSER_MAC_VM_PROOF_AUTH_PROFILE` to reuse one virtual-auth browser
profile and preserve the same Browser VM principal across setup and proof runs;
the profile also carries an owner-only virtual authenticator credential store so
the same passkey can sign back in after a fresh proof process. The wrapper
defaults `HOME_VIRTUAL_AUTH_CLEANUP=0` in that mode unless the operator
overrides it. For first-time ela.city login setup, open a headed setup run with
the same persistent virtual-auth profile:

```bash
scripts/browser-mac-vm-auth-profile-setup.sh \
  --auth-profile "$HOME/.local/share/elastos/mac-browser-vm-proof-auth" \
  --receipt-out /tmp/elastos-browser-mac-vm-auth-setup.json
```

Sign into ela.city inside the Browser VM, let the setup close cleanly, then
rerun the proof or handoff headless with the same `--auth-profile` path and
pass the setup receipt with `--auth-setup-receipt`. The setup receipt is a
local operator artifact that records the profile path, setup URL, hold time, and
exact follow-up handoff command; the Browser machine proof still records only
whether a persistent virtual-auth profile was used, not the local profile path.
The final acceptance audit recomputes the receipt SHA-256 from that receipt path
before accepting the handoff chain, and requires the setup receipt timestamp to
be no later than the machine proof timestamp.

The click sequence matches only sanitized diagnostics such as visible text,
ARIA labels, titles, and test ids, sends Runtime-mediated Browser input, and
records post-click diagnostics for the acceptance audit. In this Mac proof, a
missing diagnostic click target is recorded in the artifact instead of aborting
the machine proof; set `HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTIC_CLICK_OPTIONAL=0`
when you want the authenticated click sequence itself to be strict. It is
evidence for the authenticated modal path, not a substitute for the hash-bound
manual report.

To bind Mac manual acceptance to that exact machine artifact:

```bash
scripts/browser-mac-vm-acceptance-handoff.sh \
  --machine-proof /tmp/elastos-browser-mac-vm-proof.json \
  --source-home-restart-receipt /tmp/elastos-mac-source-home-restart.json \
  --manual-out /tmp/elastos-browser-mac-vm-manual-ux.json
```

The handoff writes a hash-bound manual UX template, an expected failing
acceptance audit without manual evidence, and a short summary of the exact gaps
left before Mac Browser acceptance. It exits non-zero until a headed auth setup receipt
and persistent proof profile are hash-bound, even when the machine proof itself
is ready. If the handoff should collect a fresh authenticated machine
proof first, omit `--machine-proof`, optionally add `--restart-source-home` so
the source-home restart receipt is generated and hash-bound automatically, and
pass `--auth-profile "$HOME/.local/share/elastos/mac-browser-vm-proof-auth"`
plus `--auth-setup-receipt /tmp/elastos-browser-mac-vm-auth-setup.json`. To
prepare a redacted operator checklist and an `ok=false` manual draft for review:

```bash
node scripts/browser-mac-vm-manual-review-packet.mjs \
  --machine-proof /tmp/elastos-browser-mac-vm-proof.json \
  --handoff-summary /tmp/elastos-browser-mac-vm-proof-handoff-summary.json \
  --out-dir /tmp/elastos-browser-mac-vm-review
```

The review packet is not acceptance; the reviewer must still fill evidence and
add at least one separate redacted screen recording, screenshot, or screenshot
set to the manual report. To generate and validate the template manually
instead:

```bash
node scripts/browser-manual-ux-report.mjs \
  --template \
  --machine-artifact /tmp/elastos-browser-mac-vm-proof.json \
  > /tmp/elastos-browser-mac-vm-manual-ux.json
node scripts/browser-manual-ux-report.mjs \
  --input /tmp/elastos-browser-mac-vm-manual-ux.json
```

Set the manual report to `ok=true` only after reviewing the restarted Mac
gateway, visible remote video, typing/input latency, address bar stability,
scroll/click fidelity, performance and zoom gates, ela.city URL sync, visible
image settling, authenticated edit-profile modal behavior, no raw authority
exposure, and session cleanup. Add at least one redacted `review_artifacts`
entry for a screen recording, screenshot, or screenshot set, with
`redacted=true`, its local path, and SHA-256 digest, so the manual review
evidence is hash-bound.
The validator rejects shallow Mac artifacts, local review artifacts that contain
obvious raw authority text, and reports without evidence text for every Mac VM
manual check. The contract smoke is:

```bash
scripts/browser-mac-vm-manual-ux-smoke.sh
```

Before claiming Mac Browser acceptance, run the acceptance audit against the
machine proof, the completed manual report, and the handoff summary:

```bash
node scripts/browser-mac-vm-acceptance-audit.mjs \
  --machine-proof /tmp/elastos-browser-mac-vm-proof.json \
  --manual-ux /tmp/elastos-browser-mac-vm-manual-ux.json \
  --handoff-summary /tmp/elastos-browser-mac-vm-proof-handoff-summary.json
```

The audit intentionally exits non-zero when the machine proof is good but the
manual report is missing, when ela.city diagnostics still look logged out, when
the in-page URL-sync click does not move to a changed URL matching the recorded
expected URL regex, when there is no authenticated profile/edit-profile signal,
when authenticated evidence was collected from a disposable virtual-auth
profile, when the handoff summary is missing or not bound to the same machine
proof, source-home restart receipt, and auth setup receipt, when either receipt
digest does not match its path, when source-home Browser helper hashes do not
match source/installed/initrd/rootfs, when setup/restart/proof/handoff timestamps
are out of order, or when the machine proof contains raw DOM HTML or CDP event
dumps.
Browser page diagnostics include
sanitized visible text samples and visible dialog/modal candidates so the audit
does not rely on raw HTML dumps for profile or edit-profile evidence.
Its contract smoke is:

```bash
scripts/browser-mac-vm-acceptance-audit-smoke.sh
```
