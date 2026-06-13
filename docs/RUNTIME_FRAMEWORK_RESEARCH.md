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

Questions to resolve:

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
ambient host access, process-shared vendor binaries, or a second runtime
authority model.
