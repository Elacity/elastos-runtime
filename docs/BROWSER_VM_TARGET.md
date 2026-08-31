# Browser VM Target Contract

The Browser product target is a per-launch VM, not a host browser process and
not a Docker container. The Browser UI remains unchanged: it asks Runtime to
open a page, Runtime obtains an Exit stream, and the Browser Engine Adapter
launches an operator-approved VM engine target.

## Design Boundaries

Selkies is the in-guest display/input transport, not the isolation boundary.
The isolation boundary is the per-launch VM plus Runtime-owned Exit, wallet,
and control grants. A hosted Selkies or Apple-container target can test the
Browser Engine Adapter and WebRTC plumbing, but it is not the production
security boundary. Source-home config is VM-only and does not expose a
hosted-proof or container-backed Browser engine selector.

The Linux guest contract is the portable layer across Linux and macOS. Linux
uses crosvm/KVM and macOS uses Apple Virtualization.framework, but both
substrates must boot the same Browser VM target contract and expose the same
Runtime-facing control, media, and Exit relay surfaces.

The source-home Browser display path is WebRTC-only. It runs inside the
per-launch Browser VM and uses Runtime-owned Exit/control/wallet boundaries; it
is not a host-browser fallback. Runtime-frame display is not a source-home
product fallback and must not hide WebRTC/Carrier relay failures.

Normal-browser latency still requires warm or snapshot-capable VM sessions behind
the same per-principal/per-site isolation contract. Cold-booting a brand-new VM
on every click, navigation, or short-lived page is not the desired product
latency model. Source-home does not automatically start a hidden Browser VM on
Home boot; warm sessions must be Runtime/provider-owned, explicit, and proven
not to swallow user launch intent or race WebRTC media negotiation.

## Required Shape

```text
Browser UI capsule
  -> Runtime Browser route
  -> Exit stream receipt
  -> Browser Engine Adapter
  -> browser-vm-product / chromium_microvm
  -> per-launch Browser VM target
```

The VM image must contain `/etc/elastos/browser-vm-target.json` with:

```json
{
  "schema": "elastos.browser.vm-target/v1",
  "engine": "chromium_microvm",
  "network_mode": "runtime_net_only",
  "direct_network": false,
  "wallet_injection": false,
  "media_transport": "runtime_relay",
  "display_mode": "webrtc_remote_display",
  "display_backend": "vm_selkies_gstreamer_webrtc",
  "runtime_exit_transport": "carrier_stream",
  "control_transport": "vsock_relay",
  "control_port": 19092
}
```

`runtime_exit_transport` may be `carrier_stream` or `vsock_relay`. The product
invariant is the same either way: Chromium in the guest does not dial public
web directly. Browser egress crosses a Runtime-owned Exit relay.

`control_transport` is currently `vsock_relay`. The VM-local Browser control
service remains a Unix socket inside the guest; `/opt/elastos/bin/browser-vm-guest-control-bridge`
exposes that control socket to the host substrate over a VM transport. This is
the page/input/signaling path. Without it, a VM may boot but Browser UI commands
dead-end at the guest boundary.

## Required Guest Files

The minimal staged contract preflight currently requires:

- `/opt/elastos/bin/browser-vm-init`
- `/opt/elastos/bin/browser-native-proxy-engine`
- `/opt/elastos/bin/browser-vm-runtime-relay`
- `/opt/elastos/bin/browser-vm-guest-control-bridge`
- `/opt/elastos/bin/browser-selkies-control-service.mjs`
- `/opt/elastos/bin/browser-vm-selkies-start`
- a guest `node` binary
- a guest Chromium binary

Run:

```sh
scripts/browser-vm-target-preflight.sh --target-dir /path/to/staged-rootfs
```

The full bootable rootfs must also pass runtime dependency mode:

```sh
scripts/browser-vm-target-preflight.sh --target-dir /path/to/full-rootfs --require-runtime-deps
```

That mode adds the guest Xvfb, Python, PipeWire, PipeWire Pulse, WirePlumber,
`pw-cli`, and `gst-inspect-1.0` checks required by the Selkies/WebRTC display
and audio path. Use it for debootstrap-built rootfs trees and any ext4 manifest
that will ship to Mac VZ or Linux crosvm targets. `browser-vm-artifact-preflight.sh`
enforces the same complete contract for `rootfs.ext4` through `debugfs` when
available, or through the rootfs sidecar manifest when direct ext4 inspection is
not available.

This is a static target-image gate. It also checks that `browser-vm-init`
starts the guest Runtime relay, starts the VM-local Selkies/Chromium stack, and
passes a generated `ELASTOS_BROWSER_SELKIES_CONTROL_CONFIG` to the guest
control service. It does not prove that crosvm or Apple VZ can boot the image
and it does not prove media quality. Those are separate substrate and UX gates.

To assemble the staged contract from explicit binaries, run:

```sh
scripts/build/stage-browser-vm-target.sh \
  --out-dir /tmp/elastos-browser-vm-target \
  --native-proxy-bin /path/to/browser-native-proxy-engine \
  --runtime-relay-bin /path/to/browser-vm-runtime-relay \
  --guest-control-bridge-bin /path/to/browser-vm-guest-control-bridge \
  --control-service /path/to/browser-selkies-control-service.mjs \
  --node-bin /path/to/node \
  --chromium-bin /path/to/chromium \
  --target-platform linux-amd64
```

The staging script intentionally does not install Chromium or claim a bootable
image by itself. It requires Linux guest binaries and rejects host binaries
such as Mach-O macOS executables, so Mac staging must pass explicit Linux ARM64
guest artifacts. It assembles the contract, writes `browser-vm-target.json`,
creates `browser-vm-init` and `browser-vm-selkies-start`, and then runs the
static target preflight. With `--rootfs-ext4`, it can also pack the staged tree
into a contract ext4 image. That option does not install a complete guest OS by
itself; production substrate launchers still need a bootable Linux rootfs with
the staged contract overlaid.

The VM Selkies launcher uses one explicit, operator-selected encoder path:
`ELASTOS_BROWSER_VM_SELKIES_ENCODER` defaults to `openh264enc`, a
Selkies-GStreamer-supported software encoder that avoids the macOS VM `x264enc`
startup failure seen in staging. Hardware or alternate software encoders can be
selected explicitly through the same variable. `ELASTOS_BROWSER_VM_SELKIES_FRAMERATE`
and `ELASTOS_BROWSER_VM_SELKIES_VIDEO_BITRATE` are validated before Selkies
starts, so a bad media profile fails at the guest boundary instead of silently
negotiating a blank remote display.

Audio is part of the default product VM target. Fresh rootfs artifacts install
PipeWire, pipewire-pulse, WirePlumber, `pw-cli`, and the GStreamer Pulse source;
the guest starts that stack before Selkies and negotiates a split audio offer
when those dependencies are present. Target and artifact preflights report
`audio_default_ready=true` for that state. `audio_default_ready=false` is a
failed product VM artifact: rebuild or restage the rootfs before using it for
Browser launches.

To build the development Browser VM rootfs artifact, run:

```sh
scripts/build/build-browser-vm-rootfs.sh \
  --out-dir /tmp/elastos-browser-vm-rootfs \
  --target-platform linux-arm64
```

This script builds the Linux guest binaries for the requested architecture,
assembles a Debian Browser filesystem with arm64/amd64 Chromium, Node, Xvfb,
Python, GStreamer, PipeWire, WirePlumber, and Selkies through `debootstrap`,
overlays the ElastOS Browser VM contract, preserves Chromium launcher arguments,
patches the Selkies caps/relay policy needed by the Runtime-owned WebRTC path,
creates a small controlled initrd, runs the strict rootfs preflight, and packs
`rootfs.ext4`. The runtime product path consumes the resulting ext4 image in
crosvm or Apple VZ.

## Runtime Relay

`/opt/elastos/bin/browser-vm-runtime-relay` is the guest-side boundary between
the existing native proxy engine and the host Runtime. It exposes the Unix Exit
socket expected by `browser-native-proxy-engine`, then forwards bytes to a
host-owned Runtime bridge. The relay config must use:

```json
{
  "schema": "elastos.browser.vm-runtime-relay.config/v1",
  "guest_relay_ipc_path": "/run/elastos/browser-exit.sock",
  "network_mode": "runtime_net_only",
  "direct_network": false,
  "transport": {
    "kind": "vsock_listen",
    "port": 19091
  }
}
```

The relay has a Unix-socket smoke transport for local tests, but product VM
launchers should use an explicit VM transport (`vsock_listen` or
`vsock_connect`) and a host bridge that maps the other side to a Runtime-owned
Exit stream. This preserves the invariant that the guest browser does not own
public-web networking.

## Guest Control Bridge

`/opt/elastos/bin/browser-vm-guest-control-bridge` is the guest-side boundary
for Browser page control and WebRTC signaling. It connects to the VM-local
`browser-selkies-control-service.mjs` Unix socket, then listens for the host
substrate on the explicit control transport:

```json
{
  "schema": "elastos.browser.vm-guest-control-bridge.config/v1",
  "guest_control_socket_path": "/run/elastos/browser-selkies-control.sock",
  "network_mode": "runtime_net_only",
  "direct_network": false,
  "control_socket_ready_timeout_ms": 60000,
  "control_request_timeout_ms": 90000,
  "transport": {
    "kind": "vsock_listen",
    "port": 19092
  }
}
```

The bridge forwards bytes only. The existing control service remains the HTTP
contract for `/status`, `/pages`, page input, clipboard, and shutdown. Accepted
host control connections wait briefly for the VM-local Unix socket so a request
that arrives before Selkies control finishes startup does not fail the launch
race. This keeps Browser semantics out of the VM transport helper and makes the
crosvm/VZ launcher responsible only for connecting the host side of the VM
transport to the Runtime-facing Browser control socket.

## No-KVM Gateway Hosts

A public gateway host does not need local KVM to use the Browser VM product
path. Local KVM only matters when that host is also the VM substrate. If the
gateway has no `/dev/kvm`, configure:

```sh
ELASTOS_BROWSER_VM_CONTROL_SOCKET=/run/elastos/browser-vm-control.sock
```

The Browser capsule should still be available on that gateway as the UI/client
surface. It should not advertise a separate non-VM Browser provider. The
provider role belongs to the local Runtime-facing Browser Engine Adapter, and
that adapter may delegate to a Mac VZ, Jetson crosvm, or other approved VM
control service. Home service offers expose this as
`backing_substrate=remote_operator_vm` or `local_microvm` with
`supported_guarantee_levels=["mechanism_microvm"]`; a host-webview or
diagnostic engine must use a weaker guarantee and must not satisfy the product
Browser VM path.

The control socket is a local Runtime-facing Unix socket. Its implementation may
be backed by a local crosvm/VZ substrate or by a remote/operator VM provider
reached through approved Runtime/Carrier/SSH-tunnel plumbing. Browser UI still
talks only to Runtime, and the Browser Engine Adapter still requires
`runtime_net_only`, `direct_network=false`, and `media_transport=runtime_relay`.
Source-home config writes a stable `/tmp/elastos-browser-vm-control-<platform>.sock`
control socket by default so the Browser VM supervisor has a single launch
target instead of relying on ambient shell state.

`scripts/browser-vm-control-service.mjs` is the local Unix-socket control-plane
contract. It serves `GET /status`, `POST /pages`, page-scoped `POST /shutdown`,
identity-bound `POST /launches/reconcile`, and an identity-bound
service-shutdown route, accepts only `chromium_microvm` launch requests, and
delegates actual VM launch to an explicit operator launcher program. Prewarm
and page launch calculate one canonical
control-service fingerprint; per-invocation prewarm, open-request, and
shutdown-request flags are excluded. A mismatched generation is replaceable
only while status proves that it owns no active pages, active or warm VMs, or
pending launches. An absent socket permits startup; an existing socket whose
status is unavailable, timed out, malformed, or foreign remains untouched and
blocks startup. Idle replacement uses an exact fingerprint-and-start-time-bound
service shutdown and waits for the owned socket to disappear before starting a
successor. An owner-only startup lock serializes this check. Lock acquisition
is fail closed: an existing regular lock is never renamed or removed
automatically, even when its recorded process is absent or its contents are
incomplete. An operator may remove a stale lock only after separately proving
its exact ownership and identity. This avoids a check/rename race, and no
launcher has authority to unlink or replace an unverified lock or control
socket.

The service validates the returned
`elastos.browser.engine.supervisor-result/v1` before handing it to Runtime.
It supports one-shot launchers and persistent launchers. Persistent launchers
are required for Apple VZ because the process that owns `VZVirtualMachine` must
stay alive for the VM lifetime; `/shutdown` terminates that launcher after the
page closes. SIGTERM and SIGINT stop new launches, cancel pending launches,
terminate and reap every launcher child owned by that service, and only then
remove the service socket. It is not a hosted-browser fallback; without a real
VM launcher behind it, it is only a contract endpoint.

Launch reconciliation is a bounded control-plane obligation, not a process or
path heuristic. The service writes at most 128 exact generation/stream records
to a `0600`, current-user-owned journal adjacent to its control socket.
Only `did_not_act` and `terminal_post_effect_cleanup` records are evictable.
`cleanup_pending` and `effect_acquired` records retain capacity until exact
cleanup is terminal; when all 128 slots are unresolved, a new launch is
rejected before guest-open dispatch or launcher spawn.
Pre-dispatch validation and capacity failures retain `did_not_act`;
`cleanup_pending` is committed synchronously only immediately before the guest
open request or launcher spawn can act. Malformed output, typed launcher
failure, timeout, cancellation, and guest-open failure become
`terminal_post_effect_cleanup` only after the exact owned launcher or VM is
reaped. If another healthy page prevents VM retirement, or exact termination
fails, the record remains `cleanup_pending`; retiring that same VM later
settles its attached failed opens terminally. Timeout and cancellation reserve
their result before signaling the child, so the child-exit event cannot replace
the requested outcome with a timing-dependent error.

An acquired page persists its exact Runtime cleanup binding with the
generation/stream record; the durable journal does not retain the larger
supervisor result as a second source of truth. After an Adapter restart, launch
reconciliation validates the journal's exact generation, stream, page,
principal, adapter, engine, display, guarantee, control/shutdown socket,
isolation, and process binding before reconstructing the same page-control
session used by canonical close. A missing, malformed, substituted,
unavailable, conflicting, or ambiguous binding remains `cleanup_pending`.
Cleanup transitions that record to
`cleanup_pending` before acting and retains the in-memory page/VM owner until
the shutdown hook, launcher child, control socket, page route, and VM absence
are proven. A failed cleanup remains retryable under the same binding.

On control-service restart, durable `did_not_act` and terminal records remain
available to `POST /launches/reconcile`. A formerly acquired effect reloads as
`cleanup_pending`; an exact cleanup request cannot become terminal merely
because the new process has empty page/VM maps. The request must match the
persisted cleanup binding, and every bound process and socket must be absent.
A surviving or otherwise unprovable resource remains pending. Runtime owns the
separate stream cleanup obligation and clears the full lifecycle only after
both the engine receipt and stream cleanup are terminal.

Runtime owns one lifecycle reconciliation service for these durable
obligations. It scans at gateway startup, is notified when a new launch,
exact-engine-cleanup, or stream-cleanup obligation is durably committed, and
retries those claims without requiring another Browser open. The service
uses one worker lane, capped batches, bounded provider calls, and exponential
backoff capped at 30 seconds. Unresolved ownership can therefore remain
retryable indefinitely without a task per obligation or a busy loop. Gateway
shutdown explicitly cancels and joins the service.

## Artifact Preflight

Use the artifact preflight when Browser VM mode does not launch:

```sh
scripts/browser-vm-artifact-preflight.sh
```

It reports one canonical answer for the current host:

- `launch_ready`: a Browser VM control socket is already running, so Runtime can delegate launches.
- `local_substrate_artifacts_ready`: the local VM substrate artifacts are present, but the control socket still needs to be started.
- `missing_for_local_substrate`: concrete missing pieces such as `/dev/kvm`, `crosvm`, `vmlinux`, `browser-vz-engine-supervisor`, or the Browser VM rootfs contract.
- `rootfs_contract`: the Browser VM guest contract check. Staged directories reuse `browser-vm-target-preflight.sh`; ext4 images are inspected with `debugfs` when available, without mounting.

macOS VZ readiness requires `bin/browser-vz-engine-supervisor`, `bin/vmlinux`,
the matching `bin/initrd` when initramfs boot is used, and
`browser-vm/rootfs.ext4` with its matching
`browser-vm/browser-vm-rootfs-manifest.json`. Linux crosvm readiness requires
`/dev/kvm`, `bin/crosvm`, `bin/vmlinux`, `browser-vm/initrd`, and the same
rootfs contract. A no-KVM public gateway can still be launch-ready if
`ELASTOS_BROWSER_VM_CONTROL_SOCKET` points to a local Runtime-facing control
socket backed by an approved remote/operator VM provider.

`scripts/setup-source-home.sh` refreshes the VM guest manifest and startup
scripts in the installed script mirror, `browser-vm/initrd` and `bin/initrd`
when present, and `browser-vm/rootfs.ext4` when present. Existing VM artifacts
are backed up before the refresh and verified by reading the updated files back
from the artifact, so source-home Browser diagnostics, control behavior, and
Selkies startup behavior do not drift from the checked-in source. This refresh
path cannot add guest OS packages, change Python site-packages, or replace the
complete guest image contract; those changes require a rebuilt rootfs artifact.
`ELASTOS_BROWSER_VM_INITRD` and
`ELASTOS_BROWSER_VM_INITRAMFS` can narrow the initrd refresh to explicit
artifact paths for crosvm and VZ target maintenance.

## Source-Home Install Truth

`components.json` currently owns the packaged provider binaries such as
`browser-engine-adapter`, `browser-engine-supervisor`, `browser-stream-bridge`,
and `browser-local-exit`. It does not currently own the source-home Browser VM
helper script wrappers or VM guest artifacts.

Full source-home setup installs the Runtime at the stable platform data-root
path `bin/elastos` and writes
`receipts/source-home-installation.json`. Platform restart helpers validate
that receipt, current source identity, and installed artifact parity before
they stop or start a Runtime.

For source-home runtimes, `scripts/setup-source-home.sh` is the canonical
generator/stager for:

- `bin/browser-vm-engine-supervisor`
- `bin/browser-vm-control-service`
- `bin/browser-vm-local-crosvm-launcher`
- `bin/browser-vm-remote-vz-launcher`
- `bin/browser-vm-prepare-rootfs-pool`
- `bin/browser-vz-engine-supervisor` on macOS after the VZ helper is built
- `browser-vm/rootfs.ext4`, `browser-vm/initrd`, and their refreshed guest
  scripts when those artifacts already exist

For already-provisioned targets, `scripts/browser-vm-target-refresh.sh` is the
canonical drift-repair path. Do not repair target hosts by hand-copying one
helper and editing `components.json`; that creates a second install truth. A
future release may promote Browser VM helpers and rootfs/initrd artifacts into
explicit component entries, but until that decision is made the renewable
source-home setup/refresh scripts plus artifact preflight are the reviewable
contract.

## Target Refresh

Use `scripts/setup-source-home.sh` when building or provisioning a source-home
runtime from source. It requires the normal source toolchain because it can build
Rust helpers and WASM capsules.

Use `scripts/browser-vm-target-refresh.sh` for already-provisioned Browser VM
targets when the reviewed source checkout is current but the installed Browser
VM helper scripts or guest script artifacts may have drifted. It does not build
Rust/WASM artifacts. It copies the Browser VM script helpers, refreshes the
guest target manifest, `browser-selkies-control-service.mjs`, and
`browser-vm-selkies-start` inside existing rootfs artifacts, refreshes the
initrd control service helper, preserves `browser-vm/initrd` and
`browser-vm/rootfs.ext4` symlinks, and creates timestamped backups before
changed writes. If the guest-control bridge binary has already been built for
the Linux guest architecture, pass `--guest-control-bridge-bin <path>` to
refresh `/opt/elastos/bin/browser-vm-guest-control-bridge` inside the rootfs
with the same backup and `debugfs` verification path. Refresh-only is not
sufficient for package/dependency changes, Chromium wrapper changes, Selkies
Python package patches, or other complete guest image changes; rebuild/restage
the Browser VM rootfs and run the artifact preflight before claiming parity.

Run the target refresh in two phases: `--verify-only` first, then the write pass
only if drift is reported. This keeps target closeout reviewable because the
operator can see exactly whether source, installed helpers, or VM artifacts
changed before mutating the target.

Typical Jetson source-home maintenance:

```sh
cd <target-source-checkout>
HOME=<target-home> \
XDG_DATA_HOME=<target-xdg-data-home> \
scripts/browser-vm-target-refresh.sh --verify-only

HOME=<target-home> \
XDG_DATA_HOME=<target-xdg-data-home> \
scripts/browser-vm-target-refresh.sh

HOME=<target-home> \
XDG_DATA_HOME=<target-xdg-data-home> \
scripts/linux-source-home-restart.sh \
  --home <target-home> \
  --xdg-data-home <target-xdg-data-home> \
  --addr <target-loopback-addr>
```

The Linux helper writes its receipt to
`<target-elastos-data-dir>/receipts/linux-source-home-restart.json`.

After the target refresh, prove the target from the reconciliation host:

```sh
scripts/jetson-browser-runtime-audit.mjs \
  --host <target-host> \
  --user <target-user> \
  --data-dir <target-elastos-data-dir> \
  --source-dir <target-source-checkout> \
  --require-parity
```

The target refresh script is intentionally narrower than full setup. If the
kernel, crosvm/VZ supervisor, native proxy binary, Chromium wrapper, Selkies app
patch, PipeWire/WirePlumber/GStreamer dependency set, target manifest, or
complete guest image contract changed, run the full provisioning path, replace
the rootfs artifact, run `scripts/browser-vm-artifact-preflight.sh`, and then
restart the source-home gateway with `scripts/linux-source-home-restart.sh`
before re-running the parity audit. A full setup rebuild can replace the
gateway binary; the live gateway will then exit as a stale host until the
restart helper brings the front door back.

Mac VZ target maintenance uses the same source checkout and default macOS data
dir unless the operator is testing an isolated data dir. Keep production/stable
target runtimes separate from the review checkout and explicit test data dir.

## Cross-Platform Substrate

Linux launches the same target through crosvm/KVM. macOS launches the same
target through Apple Virtualization.framework, using the `elastos-vz` lessons
from `sash/local-test-v030`.

The host substrate launcher is responsible for mapping the VM-local control and
media surfaces back to Runtime-scoped Browser routes. VM WebRTC must report
`media_transport=runtime_relay`; direct VM/LAN ICE candidates are not accepted.
For per-page VM launches, the requested Browser viewport is passed to the guest
as `elastos.browser_width` and `elastos.browser_height` boot args before Xvfb,
Chromium, and Selkies start. The stream surface and Runtime view should agree at
first paint instead of relying on a later Chromium-only resize.
The VM guest start script accepts explicit relay configuration through
`ELASTOS_BROWSER_VM_ICE_SERVER`, `ELASTOS_BROWSER_VM_ICE_SERVERS_JSON`,
`ELASTOS_BROWSER_VM_ICE_USERNAME`, `ELASTOS_BROWSER_VM_ICE_CREDENTIAL`, and
`ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY`, then copies the same ICE server list
into the typed display session and Selkies RTC config on the Linux/crosvm path.
Those environment and boot-argument settings are not an Apple VZ compatibility
path.

Apple VZ accepts only the complete
`elastos.browser.vz-transport-authority/v1` plus its private, hash-bound launch
secret over the launcher stdin pipe. Runtime is the sole source of the
generation, page, VM, ordinary/media streams, exact Runtime socket paths, vsock
ports, TURN endpoint and relay range, and expiry. The native supervisor starts
the VM with zero VZ network devices, sends the bounded authority over bootstrap
vsock, and starts the Browser stack only after the guest validates the
descriptor and proves loopback-only network state. Ordinary egress and media
use their fixed vsock-to-Runtime bridges. VZ NAT, a media NIC, a default guest
route, legacy ICE boot configuration, and VZ hibernation are not available.
Missing, stale, disabled, partial, or mixed legacy/VZ configuration fails before
VM dispatch.

Source-home config also loads Runtime-owned TURN credentials from
`$HOME/runtime-turn/turn-credentials.env` or
`$DATA_DIR/runtime-turn/turn-credentials.env` when explicit operator ICE
environment variables are not already set for configurations that still use
that Linux/crosvm input. The VZ launcher does not read that file or inherit
those ICE variables; Runtime issues a launch-scoped TURN authority instead.

## Apple VZ Launcher

`elastos-vz` includes a native `browser-vz-engine-supervisor` binary for the
Mac product path. It is not a browser implementation and it is not a container
wrapper. It owns one Apple Virtualization.framework VM for the lifetime of one
Browser page, then:

- boots the Browser rootfs with `init=/opt/elastos/bin/browser-vm-init`;
- includes `root=/dev/vda rootfstype=ext4 rw` in boot args for full ext4
  Browser VM rootfs images;
- prepares a per-launch writable `rootfs.ext4` under the VM session directory
  before attaching `/dev/vda`. On APFS this uses clone-on-write `cp -c` with a
  byte-copy fallback. The installed `browser-vm/rootfs.ext4` is the immutable
  base image, not the live writable boot disk;
- requires a signed native supervisor with the
  `com.apple.security.virtualization` entitlement and preflights the native
  binary, private-stdin wrapper, kernel, rootfs/initramfs artifacts, exact Unix
  socket paths, and TURN/listener/relay ports before the first launch effect;
- derives a short owner-only control-socket root from the exact authority
  binding hash, independently of long data, session, or evidence paths;
- configures zero VZ network devices and disables hibernation for every launch;
- bootstraps the exact ordinary stream, media stream, and TURN authority over a
  launch-bound guest vsock before the Browser stack starts;
- exposes guest control vsock port `19092` only after the VM-local Browser
  control socket is ready, then bridges it to a page-scoped host Unix socket;
- bridges guest egress vsock port `19091` to the Runtime-owned
  `adapter_ipc.runtime_stream_path`; the runtime stream client sends the typed
  `elastos.exit.relay-open/v1` handshake and Gateway forwards that bounded
  first line to the private Exit relay before relaying bytes;
- translates the guest Selkies open contract back into the Runtime-facing
  `chromium_microvm` Browser supervisor result;
- exits only when the Browser VM control service terminates it on page shutdown.

The native supervisor and remote VZ wrapper return
`elastos.browser.vz-launch-settlement/v1` on launch failure. It preserves the
exact binding hash, generation, page, VM, ordinary stream, media stream, and
effect fields. Effect booleans are conservative, exact-binding
`may_have_acted` markers; they never prove acquisition or cleanup by
themselves. `did_not_act` is emitted only after the complete launch identity
and authority have validated. Malformed or unbound input does not receive an
apparently exact settlement with null identity fields.
`terminal_post_effect_cleanup` is valid only when every owned child, VM,
control socket, stream bridge, TURN listener/relay range, route, and session
directory is independently proven absent by bounded wait/join, terminal VM
status, exact path checks, and port rebinding; otherwise the result is
`cleanup_pending`. The remote wrapper also gives its native supervisor process
a non-secret exact-binding command-line marker, so cleanup and absence checks
can find that process even if failure occurs before its PID file is durable. The
control service validates that binding, persists it without transport secrets,
and propagates the settled error rather than the original process error.

The gateway-facing process remains `browser-vm-control-service.mjs`. Configure
that service with `persistent_launcher: true` and
`launcher_program=/path/to/browser-vz-engine-supervisor`; the Browser Engine
Adapter still talks only to the local control socket. This lets a no-KVM gateway
host delegate to a Mac VZ provider without granting Browser UI direct host or
public-network authority.

The current VZ rootfs clone isolates VM boot state and avoids Apple VZ rejecting
overlapping writable attachments of the same disk image. Browser profile state
is deliberately not stored on that disposable boot disk. Runtime chooses an
active-principal `localhost://Users/<root>/BrowserProfiles/default/profile.ext4`
profile disk and passes that private host path to the Browser Engine Adapter
launch descriptor. The VZ launcher attaches that principal-owned persistent ext4
data disk as `/dev/vdb`, passes
`elastos.browser_profile=<profile-key>` and
`elastos.browser_profile_disk=required`, and the guest mounts that disk at
`/var/lib/elastos/browser-profile-disk` before Chromium starts. The host
`<profile-key>` is a non-reversible SHA-256 key derived from the signed Browser
launch principal, not a readable DID or path label. Cookies, localStorage,
IndexedDB, service workers, history, and other Chromium profile state live under
`/var/lib/elastos/browser-profile-disk/profiles/<profile-key>`.
If the required profile disk is missing or cannot be mounted/formatted, the
Browser VM fails closed instead of falling back to an ephemeral profile.
The VZ launcher also holds non-blocking kernel lifetime locks on the principal
profile disk and on any shared writable rootfs. A second VM
cannot attach those resources: launch returns the typed `resources_in_use`
outcome. Kernel ownership releases the lock automatically if the launcher dies;
PID files are not lifecycle authority.

New disks are sparse ext4 files. The default size is 2048 MiB and can be adjusted with
`ELASTOS_BROWSER_VM_PROFILE_DISK_MIB`. Resetting profile state is a
Runtime-owned operation: Browser calls `POST /api/apps/browser/profile/reset`
with its app launch token, Runtime refuses while that principal has live Browser
sessions, then removes only that principal's profile disk.

Profile storage boundary: this ext4 disk is the principal-owned Browser
profile lane, not yet protected principal-root object storage and not yet
Recovery Kit exported/imported state. Reset proof is required evidence for the
current lane, but it is not a claim that Chromium cookies, localStorage,
IndexedDB, service workers, history, or downloads are encrypted or recoverable.
Runtime Browser profile descriptors and reset receipts must declare
`storage_posture=principal_owned_reset_scoped_unprotected`,
`protected_storage=false`, `encrypted=false`, and `recoverable=false`.

Browser open failures carry `elastos.browser.open-outcome/v1`. Runtime reports
`terminal_pre_effect_failure` after dispatch only when the exact lifecycle
generation and stream have an independent `did_not_act` proof.
Transport loss, malformed or unsafe success replies, and post-launch validation
failures are reconciled against that same identity. An exact recovered effect
is never adopted after the open has failed: Runtime promotes it directly into
exact cleanup and keeps replacement blocked until the typed terminal engine
receipt and stream cleanup are durably committed. An exact terminal proof reports
`terminal_post_effect_cleanup`. If neither an effect nor no-effect can be
proved, Runtime durably retains the stream and launch ownership as
`cleanup_pending`, reports page and VM acquisition as indeterminate, and blocks
replacement while the lifecycle service continues reconciliation. A late
failure likewise retains cleanup ownership until a typed terminal receipt;
absence, timeout, and provider failure never imply `did_not_act` or successful
closure. Browser UI renders these states
directly and does not infer lifecycle ownership from process-error text or claim
a missing terminal page close for an indeterminate or pre-effect failure.
