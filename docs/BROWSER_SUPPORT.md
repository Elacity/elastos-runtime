# Browser device and operator support

This is the Browser qualification matrix. A supported entry requires installed
evidence for the exact device roles, software versions, codecs and topology.
Current verdicts and target observations live in
[state.md](../state.md#browser-contract-and-device-qualification).
Open qualification work lives in [TASKS.md](../TASKS.md#browser-maturity-workstream).

The matrix deliberately separates a viewer from the Runtime that hosts Engine
and the Runtime that hosts Exit. A device with insufficient local Engine
capabilities can be a viewer when it has an approved compatible remote Engine.
Runtime detects and adapts the host; capsules use the
[same Browser contract](BROWSER_PROTOCOL.md).

## Qualification matrix

The requirements below identify implementation candidates and proposed test
baselines. They do not establish measured minimum hardware or certified support.
Each candidate stays unverified until its acceptance receipt exists. OS and
browser version ranges are certified from tested versions, with explicit lower
bounds; successful compilation alone cannot supply a lower bound.

| Role and target | OS / architecture | Resource requirement or proposed baseline | Virtualization | Viewer and media requirement | Qualification evidence required |
| --- | --- | --- | --- | --- | --- |
| Engine host: Mac VZ | macOS / ARM64 | Current guest default: 2 vCPUs and 2 GiB RAM. Proposed host baseline: 8 GiB RAM, plus verified image/profile disk headroom. Minimum OS and host resources await measurement. | Apple Virtualization.framework, usable hardware support, authorized launcher and required devices | Engine emits the negotiated WebRTC video/audio; current VM encoder default is OpenH264. | Installed launch, page/input, media, close/reopen, profile binding and compatibility rejection on a named Mac and OS. |
| Engine host: Linux crosvm | Linux / AMD64 | Current guest default: 2 vCPUs and 3 GiB RAM. Proposed host baseline: 8 GiB RAM plus image/profile headroom. Kernel/distribution minimum awaits qualification. | Working KVM API and process access, compatible crosvm, kernel and guest image | Same Browser media and operator contract | Installed evidence on a physical or appropriately virtualized KVM host. |
| Engine host: Linux crosvm / Jetson | Linux / ARM64 | Current guest default: 4 vCPUs and 2 GiB RAM. Proposed host baseline: 8 GiB RAM plus image/profile headroom. Exact Jetson/OS minimum awaits qualification. | Working KVM API and access, compatible ARM64 host and guest artifacts | Same contract; any hardware encoder requires its own artifact and media proof | Installed evidence on the named Jetson or ARM64 host. |
| Viewer: desktop | Supported browser on macOS, Linux or Windows / host-supported architecture | Working WebRTC receive/decode, datachannel input and sufficient resources for the negotiated viewport. Measured minimum CPU/RAM awaits the viewer run. | Engine virtualization belongs to the selected Engine host | Qualify Brave/Chromium, Firefox and Safari separately by exact version, media negotiation and input behavior. | Installed Runtime plus real Browser page, audio/video/input, resize, reconnect and close. Each browser/version is a separate entry. |
| Viewer: phone/tablet | Browser on iOS/iPadOS or Android / device-supported architecture | Touch/keyboard focus, foreground/background recovery and negotiated media within measured device resources | Engine can run on an approved remote Runtime | Qualify actual mobile browser/OS combinations; desktop emulation does not certify them | Physical-device media, touch, text composition, orientation, permissions and recovery proof. |
| Exit host / remote Engine consumer | Supported Runtime on Linux AMD64/ARM64 or macOS ARM64 | Authorized Net/Exit provider, bounded connections and measured per-stream resource budget; minimum resources await qualification | Local Engine virtualization is independent of Exit and consumer roles | Consumer needs a compatible selected Engine; a graphical browser is required only on the viewer | Controlled destination and DNS evidence, grant enforcement, independent Engine/Exit placement, cleanup and version rejection. |
| Windows Runtime host | WSL2 Linux path on supported Windows hardware; native Windows is a separate candidate | WSL-first Runtime installation and measured role-specific resources | A local Engine additionally requires usable KVM/nested virtualization; otherwise use an approved remote Engine | Windows viewer qualification is independent of WSL Engine qualification | Fresh WSL installation and installed Runtime/Exit tests; local Engine evidence only when its required virtualization works. |
| Intel Mac Engine host | macOS / AMD64 | A compatible implementation and artifacts need a separate qualification decision | Current VZ Browser source path targets ARM64 | Intel Mac viewer use can be tested independently with remote Engine | No local Engine support claim from a generic macOS package or viewer result. |

Disk admission must account for the actual image manifest, profile, temporary
staging, concurrent sessions and host free-space policy. Guest allocation is
not a host-memory minimum. The Browser capsule's own 32 MiB manifest budget
describes its UI projection, not Chromium, VM, media or Runtime consumption.

## Detection and compatibility rules

Runtime's installer selects platform artifacts using its platform identity and
verified component manifest. The source-home Browser configuration currently
has payload targets `darwin-arm64`, `linux-amd64` and `linux-arm64`. Target
presence establishes an implementation path; B02 still requires complete
artifact and permission readiness before advertising a launchable service.

The Mac host adapter queries `VZVirtualMachine.isSupported` on Apple Silicon.
The Linux adapter opens KVM with process permissions and queries its API version
before admitting a local VM. The kernel defines API version
12 as the stable baseline and provides capability queries for additional
requirements. A successful query is an eligibility check, not a completed VM
launch. [Linux KVM API](https://docs.kernel.org/virt/kvm/api.html).

The web Runtime adapter checks WebRTC peer connections, transceivers,
datachannels and reported receive codecs before it closes an existing page or
requests a new Engine page. It reports typed viewer incompatibility without
allocating a peer connection or requesting camera or microphone access. A
viewer whose browser lacks a codec query proceeds to actual SDP negotiation;
its unreported codecs remain unknown. This eligibility check does not establish
codec interoperability or installed media support.

Viewer adaptation uses WebRTC capability negotiation and actual media results.
Qualify decoding, audio playback policy, datachannels, focus and input on the
selected browser. Browser names and user-agent strings alone are insufficient
compatibility evidence. [WebRTC specification](https://www.w3.org/TR/webrtc/).

The current guest baseline uses an OpenH264 video encoder and a separate audio
offer when the required audio stack is present. The receipt must record the
actual negotiated codec, profile, channels, resolution and rate. Alternative
encoders and codecs have separate qualification results. An image or video-only
session does not establish full Browser media support.

## Evidence required to mark an entry supported

Each receipt must identify the source commit and tree; Browser capsule identity
and content hash; Runtime, Engine and host-helper hashes; VM/image manifest where
used; device model, OS, architecture and resource budget; viewer and operator
versions; actual negotiated codecs; Engine and Exit identities; placement and
network profile; exact commands; and matching manual UX/media proof.

The installed suite must open and operate the controlled page, reject an
incompatible Engine before page/profile/stream effects, enforce explicit
selection and authority, recover within the applicable goal's limits, and close
its resources. All roles claimed for the device need their own proof. Required
Browser media and objective-audit gates still apply.

Evidence for one Mac, one successful page, an older source tree, a mock remote
provider, or a software-emulated mobile viewport cannot certify additional
devices or a newer installation. Availability, protocol compatibility, artifact
readiness, capacity, and support verdict remain separate facts.
