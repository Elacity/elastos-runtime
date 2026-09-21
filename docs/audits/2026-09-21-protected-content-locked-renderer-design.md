# Locked renderer for Elacity Reader — boundary design

Date: 2026-09-21. Status: design, for review before implementation.
Scope: the object path only. Media playback keeps its own CENC contract and is
out of scope here.

This document decides where the render boundary sits, what crosses it, who owns
each side, and which kinds are worth moving first. It records one finding that
changes the shape of the answer: a render intermediate representation does not
by itself keep a file out of the viewer.

## What the Reader holds today

`elacity-reader` receives the whole object as plaintext. Runtime reads bounded
fixed-size chunks from the decrypt provider, the Reader reassembles them into
one run of bytes, and `render.js` hands those bytes to the renderer for the
kind: pictures to an image element, PDF to pdf.js, text to the DOM, glTF, OBJ
and STL to three.js, EPUB and CBZ to their own readers.

This is deliberate and it is written down in `docs/PROTECTED_CONTENT.md`: the
containment claim is that the CEK and the custody shares stay inside the
provider boundary, and the claim explicitly stops short of preventing capture
of what is drawn. The Reader holding plaintext is a consequence of that scope,
not a defect against it.

## The finding

`ddrm-reader` is the reference. Its client renders a decoded render-IR rather
than the file, and its prime directive is that the encrypted bytes and the CEK
never leave the backend. Reading its actual contract in
`packages/ir-schema/src/render-ir.ts` shows the IR has four variants, and they
differ in kind, not only in payload:

| Variant | What crosses | Is the file still there? |
| --- | --- | --- |
| `raster-tile-set` | re-encoded image tiles per page | No. The original is gone. |
| `text-doc` | sanitized HTML and CSS fragments | No. The original is gone. |
| `doc-file` | sanitized, self-contained PDF or EPUB bytes | Yes, as a document. |
| `mesh-set` | sanitized, self-contained GLB, OBJ, STL bytes | Yes, as a model. |

So two of the four variants genuinely transform, and two ship the original
document in a cleaned container. A viewer built on that IR still receives a
PDF when the file is a PDF. Adopting the IR shape therefore buys a real
boundary for pictures and text, and buys sanitization rather than containment
for documents and models.

This matters because it sets the honest ceiling. Locking PDF, EPUB and 3D means
rasterizing them server-side into `raster-tile-set`, which is a rendering
engine inside a private provider, not a format change. That is a far larger
piece of work than adopting an IR, and it carries its own costs: text selection,
search and accessibility all come from the document, and a page of tiles has
none of them unless they are rebuilt alongside.

## The boundary

Runtime already owns the one place plaintext exists outside the Reader: the
protected-content decrypt provider reconstructs the CEK in its own process and
serves bounded chunk reads. Decoding belongs on that side of the line, because
it is the only side that holds the bytes.

Two placements work:

1. **Inside the decrypt provider.** It already holds the plaintext, so nothing
   new crosses a boundary. It gains image decoding, which is new attack surface
   in the process that holds the CEK.
2. **A sibling private provider** that Runtime hands plaintext to and takes IR
   back from. The CEK stays where it is and the decoder is isolated from it, at
   the cost of one more provider and one more plaintext crossing.

**Decided 2026-09-21 by the owner: placement 1, inside the decrypt provider,
on the constraint that the CEK never traverses any surface that could
compromise it.**

The constraint chooses the placement. Placement 2 keeps decoders away from key
material, but it does so by handing plaintext across a process boundary, which
is a surface that does not exist today. Placement 1 creates no new crossing at
all: the bytes are decoded where they already are.

That leaves one residual risk, and it is co-residency rather than traversal. A
decoder defect in the process holding the CEK could reach it without the CEK
crossing anything. The constraint is therefore met by keeping the decode path
unable to reach key material:

1. **Pure-Rust decoders only.** No C or C++ in this process. This is a hard
   rule, and it decides which kinds can take this path at all — see the table
   below, where PDF fails it.
2. **`forbid(unsafe_code)` on the decode module**, so the rule is held by the
   compiler rather than by review.
3. **Decode strictly after AEAD verification**, from a buffer the decoder owns.
   Unverified bytes never reach a decoder.
4. **The decode path receives plaintext and nothing else.** The CEK and every
   derived secret stay in `Zeroizing` values the decoder holds no reference to,
   and the plaintext is dropped before tiles are emitted.
5. **Bound dimensions and pixel count before allocating**, so a small file
   cannot ask for a large allocation.

If review later wants the co-residency risk removed rather than bounded, the
answer that honours the same constraint is a decoder sandboxed in wasm *within*
this provider: still inside the decrypt provider by ownership, no new plaintext
crossing, and no shared address space with key material. The workspace already
embeds `wasmtime` 36.0.13, so this is available rather than hypothetical. It is
worth doing only if the pure-Rust rule proves too limiting.

Whichever is chosen, the Reader's side is the same: it dispatches on the IR tag
and never on the declared content type, exactly as the reference does.

## What crosses, for ElastOS

One versioned contract, `elastos.protected-content.render-ir/v1`, carried over
the existing `read_viewer` reads so no new route and no new authority appear.
The session already binds principal, launch, actor and content kind; the IR is
a change of payload inside that session, not a change of who may open it.

The Reader's existing bounds carry over unchanged: 64 MiB for one object and
64 parts of 1 MiB. Tiles are bounded the same way, and a page whose tiles
exceed the bound is refused rather than truncated.

## Which kinds move, and when

**P3b, pictures.** A picture becomes `raster-tile-set`. The transform is real:
the file the person bought never reaches the page. This is the variant worth
proving first, because it is the one where the boundary actually differs.

**Decided 2026-09-21 by the owner: lock the five picture types that decode in
pure Rust, and leave AVIF exactly as it works today.** The instruction was to
take whichever option is simple while breaking neither security nor a working
journey, and this is that option on both counts. AVIF's usual decoder is
`dav1d`, which is C, so locking it would mean putting C in the process that
holds the CEK — the one thing the placement rule forbids. Leaving AVIF alone
changes nothing a person can currently do: an AVIF picture keeps rendering the
way it renders now.

The split costs less than it appears to. The Reader dispatches on the IR tag,
and a picture delivered as plaintext simply carries no tag, so the branch is
one field on a session the Reader already reads — not a second pipeline. The
exact crate and features are checked against the pinned toolchain and
`Cargo.lock` before anything is added.

What the split does cost is a product claim that is true of five formats and
not a sixth. That is a real cost and it is recorded rather than smoothed over:
see the claim section below, and open question 3.

**The rest, judged by one question.**

A lock is possible only where the rendered form differs from the file. Where
the two are the same thing there is nothing to withhold, and an IR would move
the same bytes under a new name. Reading each renderer in `render.js` against
that question gives the real table:

| Kind | The file | What the person sees | Lock possible? | Cost, and what is lost |
| --- | --- | --- | --- | --- |
| Picture | PNG, JPEG, GIF, BMP, WebP, AVIF | pixels | **Yes** | Small, except AVIF as above. |
| Comic, CBZ | zip of images | pixels, one page at a time | **Yes** | Small, and it reuses the picture path exactly: unzip, then decode each page. The best value after pictures. |
| PDF | PDF | pixels, one page at a time | In principle | **Fails the pure-Rust rule.** No mature pure-Rust PDF rasterizer exists; the real options are pdfium and mupdf, both C++, in the process holding the CEK. It also loses text selection, search and screen-reader access, which come from the document. |
| Book, EPUB | zip of HTML and CSS | laid-out, reflowable text | Only by changing what it is | Rasterizing needs a layout engine, so a browser server-side. The achievable version is sanitized HTML fragments, which needs an HTML sanitizer and still hands the person a document. Reflow, selection and search survive; the lock does not. |
| Text, code, CSV, JSON, Markdown | text | **the same text** | **No** | `render.js` puts the decoded bytes straight into `textContent`; nothing is converted. The rendered form is the file, so an IR withholds nothing. |
| 3D | glTF, GLB, OBJ, STL | an interactive model | Only as a different product | A locked 3D view means rendering server-side and streaming frames, and the person loses free orbit and inspection, which is the point of a 3D view. |

Two corrections to the earlier reading in this document. Text is not second in
line: it cannot be locked at all, because the rendered form and the file are
identical. And CBZ does not belong with PDF and EPUB. A comic is pages of
images, so it follows pictures almost for free and belongs immediately after
them.

The order worth building is pictures, then comics. PDF is gated by the
pure-Rust rule rather than by effort. EPUB and 3D are product changes rather
than format changes. Text is out permanently.

## What the claim becomes

With P3b landed, the Reader's claim is: PNG, JPEG, GIF, BMP and WebP pictures
are delivered as re-encoded tiles, so the page never holds the purchased file.
AVIF pictures and every other kind are delivered as plaintext, as they are
today.

That is a narrower claim than "the Reader is locked", and it is the one the
product can defend. Neither claim is hardware attestation, and neither prevents
a person photographing the screen or reading tiles out of the canvas. The lock changes
what a page holds, not what a viewer can see. Saying more than that would
repeat the mistake the old viewer's pixel and HTML lock modes invite, where a
software boundary gets described as if it were a physical one.

## What is settled, and what returns here later

Settled on 2026-09-21 by the owner, both recorded above:

- **Decoder placement.** Inside the decrypt provider, on the constraint that
  the CEK never traverses any surface that could compromise it. The five
  containment rules that make co-residency safe are part of the decision.
- **AVIF.** Lock the five pure-Rust picture types; AVIF keeps its current path.

Open, to pick up when this work resumes:

1. Whether pictures alone justify the contract, or whether comics land in the
   same slice, since they reuse the picture path almost entirely.
2. Accessibility: a tiled picture needs its description carried beside it, and
   the Reader has no source for one today.
3. Whether the Marketplace and Library say which kinds are locked. The AVIF
   decision makes this sharper rather than softer: the honest statement is now
   about five formats and not a sixth, and a product that cannot explain that
   distinction is better off not making the claim at all.
4. Whether a pure-Rust AVIF decoder has become trustworthy enough to close the
   split. Worth re-checking whenever this is picked up, rather than assuming
   today's answer holds.

## Related

- [Protected content](../PROTECTED_CONTENT.md) — the containment claim this
  design extends.
- [Protected-content crypto review](../PROTECTED_CONTENT_CRYPTO_REVIEW.md) —
  the reviewer-facing package for the primitives underneath it.
