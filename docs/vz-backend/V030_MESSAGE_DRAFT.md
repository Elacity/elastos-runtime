# Message draft to Anders — paste-ready

> Tone target: deferential to his time, evidence-led, no asks. Three short
> paragraphs. Pointer to the full memo for anyone who wants depth.
> Trim/edit as needed before sending.

---

**Subject:** quick v0.3.0 context — read at your leisure for v0.3.1

Anders, congrats on 0.3.0 — read it end-to-end this morning and the architecture
docs (PRINCIPLES, ROADMAP, the per-feature docs paired with `state.md`
"what is proven") are excellent. The fail-closed boundary discipline on the
new provider stack and the honest "Browser is a dangerous capsule, still a
capsule" framing both line up well with the security work we did on our two
branches; nothing in v0.3.0 surprised us in a bad way.

When you get to the v0.3.1 window, three things from our side worth knowing
without re-doing the homework yourself:

1. **`chore/runtime-cve-hygiene` (PR #1)** — `cargo audit` on v0.3.0 main is 35
   vulns / 12 warnings. The CVE branch closes ~20 of them mechanically when
   replayed (wasmtime 17 → 36, rustls-webpki chain, rustls-pemfile removal,
   `lru`, `cap-primitives`). Smaller PR, mostly mechanical, no API/UX impact.
   PR is CI-clean and ready to look at whenever; full reviewer checklist is in
   `docs/vz-backend/cve-hygiene/MERGE_PLAN.md`.

2. **`sash/local-test` (Mac VZ substrate)** — bigger and not as
   straightforward. Two real conflicts vs v0.3.0: `carrier_bridge.rs` (you +1002
   / we +917 in different directions) and `supervisor.rs` (you +148 / we +3800
   because of the whole VZ engine). Before any rebase: we'd want your call on
   whether native macOS Linux-microVM parity is in scope at all, or whether
   v0.3.1's macOS story stays strictly browser-hosted per the ROADMAP. Either
   answer is fine — but note that the cross-compile work on this branch
   (`elastos-crosvm` Mac stub) is the mechanical prerequisite for the
   `darwin\)` alignment gate in `scripts/check-wci-alignment.sh` regardless of
   the VZ decision; v0.3.0 main doesn't currently build on Mac (`elastos-server`
   path-deps `elastos-crosvm`, which hits BSD/Mac libc gaps).

3. **One thing worth flagging independent of our branches:** `rsa` 0.9.10
   landed transitively in v0.3.0 with RUSTSEC-2023-0071 (Marvin timing attack —
   the standard "do not use `rsa` for any secret-bearing operation" advisory,
   unfixable upstream). Worth a quick audit of whether any v0.3.0 capability
   path actually invokes `rsa` for decrypt/sign with key material. `elastos-auth`
   uses `k256` directly (good), so it might only be hit through a transitive
   dependency you don't care about — but worth one `cargo tree -i rsa` to be
   sure.

Full evidence-based memo with file-by-file conflict shape, dep delta, and
recommended sequencing lives at `docs/vz-backend/V030_INTEGRATION_NOTES.md` on
`sash/local-test`. **No action requested from you on any of this — both
branches are stable and CI-green, awaiting your v0.3.1 review window.** Just
giving you a head start so the review surface isn't a full investigation.
