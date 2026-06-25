# Vendored fonts

`DejaVuSans.ttf` and `DejaVuSansMono.ttf` are part of the **DejaVu Fonts** family
(version 2.37), a freely-redistributable typeface derived from the Bitstream Vera
fonts and the Arev fonts.

These fonts are embedded (via `include_bytes!`) into the `decrypt-provider` capsule's
`pdf-render` build so the in-boundary document renderers can rasterise body text with a
real, anti-aliased vector face (instead of an 8x8 bitmap). No system fonts are required,
and the crate still builds to `wasm32-wasip1`.

## License

Bitstream Vera Fonts Copyright (c) 2003 by Bitstream, Inc.
DejaVu changes are in the public domain.

Permission is hereby granted, free of charge, to any person obtaining a copy of the
fonts accompanying this license ("Fonts") and associated documentation files (the
"Font Software"), to reproduce and distribute the Font Software, including without
limitation the rights to use, copy, merge, publish, distribute, and/or sell copies of
the Font Software, and to permit persons to whom the Font Software is furnished to do
so, subject to the conditions in the full Bitstream Vera license.

Full text: https://dejavu-fonts.github.io/License.html
