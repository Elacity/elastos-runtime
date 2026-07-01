# Runtime Framework Research

This document tracks external runtime/framework inputs that may inform ElastOS
without becoming immediate dependencies.

## COMO / C++ Component Model

Source: <https://gitee.com/tjopenlab/como>

COMO is related to Android-compatible C/C++ smartphone work from 2016 and has
been considered as a possible World Computer framework reference. The public
university repo currently presents COMO as a C++ Component Model with:

- `cdlc`: component description compiler
- `comort`: component runtime
- `libcore`: core library
- interface-oriented programming
- C++ runtime reflection
- Linux x64 and Android aarch64 build targets

Current ElastOS position:

- Treat COMO as a research input for interface definition, runtime reflection,
  MetaClass-style packaging, and generated adaptation glue.
- Do not make COMO the Runtime trusted core, a shared-library plugin path, or an
  execution substrate dependency before review.
- Keep the current foundation Rust/Wasm/WASI-first: Runtime owns authority,
  capsules remain isolated, and effects go through the Carrier/provider plane.

Decision update 2026-06-11:

- Adopt the COMO lesson, not the COMO runtime: ElastOS should use signed,
  self-describing interface descriptors for capsule/provider contracts so
  binaries can declare what they provide and require without teaching the
  trusted core every provider noun.
- Keep those descriptors as data attached to capsule manifests and interface
  registry records. Generated Rust/Wasm stubs or skeletons are acceptable if
  they preserve the same capability boundary.
- Reject COMO as a direct C++/DBus dependency for the Runtime trusted core. That
  would add a second authority model, process-shared native component concerns,
  and Linux-desktop IPC assumptions that conflict with the Rust/Wasm capsule
  kernel and Carrier/provider plane.
- Keep invocation location-explicit. A typed interface descriptor plus Carrier
  identity must produce capability-checked message passing, not DCOM-style
  transparent remote objects where local and remote calls pretend to have the
  same failure model.
- The next implementation artifact should be the ElastOS interface registry and
  one small reference contract, not a COMO port.

Questions to resolve before any implementation beyond descriptors:

- Does COMO enforce separate address spaces, or does it mainly provide component
  abstraction and reflection?
- Which Android-compatible pieces still exist outside the public university
  repository?
- Can COMO interface descriptions compile into a stable WASI/capsule-kernel ABI
  or generate Rust/Wasm bindings?
- Which parts passed real reliability, accountability, redundancy, or
  fault-tolerance review for high-speed rail scenarios?
- Can its MetaClass model improve signed capsule manifests, interface
  descriptors, and AI-generated glue without widening capsule authority?

Decision gate:

COMO can influence ElastOS only where it strengthens typed interfaces,
capability-scoped ABI generation, and capsule portability. It must not introduce
ambient host access, process-shared vendor binaries, transparent remote objects,
or a second runtime authority model.

## Cosmopolitan Libc / Actually Portable Executables

Source: <https://github.com/jart/cosmopolitan>

Cosmopolitan Libc makes C/C++ programs buildable as actually portable
executables across several host operating systems. It is relevant as a possible
input for tiny support utilities or C/C++ helper portability, not as a Runtime
substrate.

Current ElastOS position:

- Treat Cosmopolitan as research for small helper binaries only.
- Do not use it to replace the Rust workspace, Wasm/WASI capsule boundary,
  Browser Engine ABI, or microVM/container isolation.
- Do not treat it as a macOS `.dmg`, Chromium, WebView, GPU/audio, app signing,
  notarization, or update-channel solution.

Best-fit ElastOS areas:

- Single-file diagnostic helpers that need only stdio, filesystem reads, hashes,
  signatures, and sockets, such as offline `components.json` inspection,
  capsule-manifest verification, provider-install sanity checks, or source-home
  repair diagnostics.
- Bootstrap or rescue utilities that can run before the full Rust Runtime is
  installed, provided they verify signed release artifacts and do not become a
  second installer authority.
- Tiny C/C++ compatibility sidecars where a proven C library is needed and a
  Rust/Wasm replacement would add more risk than value. These sidecars must stay
  behind the normal provider/capability boundary.
- Long-lived archival tools where the "actually portable executable plus ZIP
  payload" shape could simplify offline review of source, checksums, fixtures,
  or small test assets.
- Research and education around ABI portability. `emulator.com` is useful for
  understanding x86_64 Linux program execution, not for booting arbitrary OS or
  Browser images.

Low-risk first experiment:

- Build one non-production `elastos-doctor.com` style helper that prints local
  Runtime install health, verifies a manifest/hash, and exits. Compare its
  signing, reproducibility, macOS Gatekeeper behavior, Linux execution behavior,
  Windows execution behavior, and update-channel fit against the existing Rust
  toolchain. Drop the experiment if it weakens release verification or operator
  clarity.

Explicit non-goals:

- Do not run arbitrary x86 disk images through Cosmopolitan or `emulator.com`.
- Do not run Chromium, the product Browser, WebRTC media, wallet authority, dDRM
  key handling, or Carrier transports inside a Cosmopolitan compatibility path.
- Do not use APE ZIP payloads as an unreviewed capsule/package format.

Questions to resolve:

- Can any current C/C++ helper benefit from an actually-portable executable
  without weakening platform-specific sandboxing or update verification?
- Does the APE loader/binfmt story create operator friction that is worse than
  shipping normal per-platform binaries?
- Would use of Cosmopolitan complicate reproducible release checksums or signed
  component manifests?

Decision gate:

Cosmopolitan can be adopted only for isolated support utilities where it reduces
per-platform packaging without adding a second authority model, ambient host
access, or weaker signed-release verification.
