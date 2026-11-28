# hayro-interpret

[![Crates.io](https://img.shields.io/crates/v/hayro-interpret.svg)](https://crates.io/crates/hayro-interpret)
[![Documentation](https://docs.rs/hayro-interpret/badge.svg)](https://docs.rs/hayro-interpret)

<!-- cargo-rdme start -->

A crate for interpreting PDF files.

This crate provides an abstraction to interpret the content of a PDF file and render them
into an abstract [`Device`], which clients can implement as needed. This allows you, for
example, to render PDF files to bitmaps (which is what the `hayro` crate does), or other formats
such as SVG.

It should be noted that this crate is still very much in development. Therefore it currently
lacks pretty much any documentation on how to use it. It's current API also only really makes it
useful for rendering to PNG or SVG, though this will be improved upon in the future.

## Examples
See the `examples` folder on the GitHub repository. Apart from that, you can also consult
the implementation of `hayro` and `hayro-svg` to get an idea on how to use this crate.

## Safety
This crate forbids unsafe code via a crate-level attribute.

## Cargo features
This crate has one optional feature:
- `embed-fonts`: PDF processors are required to support 14 predefined fonts that do not need to be
  embedded into a PDF file. If you enable this feature, hayro will embed a (permissively-licensed)
  substitute for each font, so that you don't have to implement your custom font loading logic. This
  will add around ~240KB to your binary.

<!-- cargo-rdme end -->

## License
Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

## PdfCraft function limits

The PdfCraft patch rejects function construction after 64 nested functions or 10,000 total
function nodes, including repeated references, and detects cycles on the current path. Shared
children in different branches remain valid. Stitching functions reject empty or unresolved
child lists instead of silently truncating them. Calculator functions have a shared limit of 64
procedure levels and 10,000 parsed tokens (operators and procedure openings); evaluation has
its own 10,000-operator limit across selected branches. These are implementation resource
limits rather than PDF format limits. Rejection returns the existing `None`, and the optional
`logging` feature reports the reason and limit; no new interface warning is introduced.

Calculator integer division without an integer result is also rejected. Bit shifting discards
bits beyond the 32-bit word and fills with zeroes, including logical right shifts of negative
words; rotation accepts the full signed shift range without negating the minimum integer.
Fixed-size operand and procedure-stack overflow errors propagate instead of truncating work.
Normal function signatures and supported color/transfer-function behavior remain unchanged.
