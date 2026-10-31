# pdfcraft-filters

PDF stream filters: the decoders and encoders behind `/Filter` and `/DecodeParms`.

- **Layer:** L0, standalone. No dependency on any other PdfCraft crate. `cos` depends on it (architecture §3 rule 3).
- **Licence:** MIT OR Apache-2.0. Clean-room: written from ISO 32000-2 §7.4, RFC 1950/1951 (through `flate2` with the pure-Rust `miniz_oxide` backend), TIFF 6.0 §14 (predictor 2) and RFC 2083 §6 (PNG filters). CCITT Group 4 encoding (ITU-T T.6) is the `fax` crate's (MIT, already in the tree through `tiff`), checked here by round trips through an independent decoder; CCITT decoding (T.4 and T.6) is the `hayro-ccitt` crate's (MIT OR Apache-2.0, already in the tree through hayro), bounded here. JBIG2 generic region encoding is written from the public ISO/IEC 14492 Final Committee Draft (WG1 N1359, 1999); Annex H.2 checks the arithmetic coder byte for byte.

- `#![forbid(unsafe_code)]`. Builds for `wasm32-unknown-unknown`.

## API

```rust
pub enum Filter { Flate, Lzw, AsciiHex, Ascii85, RunLength, Dct, Jpx, Jbig2, CcittFax, Crypt, Unknown(String) }
Filter::from_name(b"FlateDecode" | b"Fl" | ...) -> Filter   // full names and inline-image abbreviations
Filter::name(&self) -> &str; Filter::is_image_codec(&self) -> bool

pub struct Params { predictor, colors, bits_per_component, columns, early_change }  // i64s; Default = 1, 1, 8, 1, 1
pub enum FilterError { Corrupt { filter, detail }, LimitExceeded(usize), Unsupported(String) }

decode(chain, data, max_output) -> Result<Vec<u8>, FilterError>
decode_counted(chain, data, max_output) -> Result<(Vec<u8>, usize /*decoded*/), (FilterError, usize /*decoded*/)>
decode_tolerant(chain, data, max_output) -> Result<(Vec<u8>, bool /*partial*/), FilterError>
decode_tolerant_counted(chain, data, max_output) -> Result<(Vec<u8>, bool /*partial*/, usize /*decoded*/), (FilterError, usize /*decoded*/)>
encode(filter, params, data) -> Result<Vec<u8>, FilterError>
encode_flate(data) -> Vec<u8>   // zlib, level 6
encode_g4(data, columns, rows) -> Result<Vec<u8>, FilterError>   // 1-bit rows as CCITT Group 4; columns <= MAX_G4_COLUMNS
pub struct CcittParams { k, columns, rows, end_of_line, encoded_byte_align, end_of_block, black_is_1 }  // Table 11; Default = its defaults
decode_ccitt(data, &params, max_rows) -> Result<Vec<u8>, FilterError>   // CCITT fax as CCITTFaxDecode gives it; columns <= MAX_CCITT_COLUMNS
encode_jbig2_generic(data, columns, rows) -> Result<Vec<u8>, FilterError>   // 1-bit rows as lossless JBIG2 for JBIG2Decode; columns <= MAX_JBIG2_COLUMNS
encode_jbig2(data, columns, rows) -> Result<Vec<u8>, FilterError>   // 1-bit rows as lossless JBIG2 for JBIG2Decode, symbol coded when that is smaller; columns <= MAX_JBIG2_COLUMNS
```

- `chain` is applied first to last, as in a `/Filter` array.
- Image codecs (DCT, JPX, JBIG2, CCITT), `Crypt` and unknown filters anywhere in a chain give `Unsupported`: callers keep such data encoded. So does a chain longer than `MAX_CHAIN` (8) filters, before any stage runs. Decryption belongs to `crypt`.
- `max_output` bounds every intermediate buffer (including the pre-predictor buffer) and gives `LimitExceeded(max_output)`. This also holds when Flate retries raw deflate after a misleading zlib header, in both strict and tolerant decoding.
- `decode_counted` and `decode_tolerant_counted` are for a caller charging a budget (`pdfcraft_cos::DecodeBudget`). `max_output` bounds all stages together (each stage gets what the earlier ones left), and they give the bytes every stage that ran decoded, on success and on failure. A stage's output counts as its decoder produced it, before a predictor or a later stage shrinks it, and a failed decode pays for what it decoded first.
- `decode_tolerant` turns `Corrupt` into "what was decoded so far" plus `partial = true`, then runs the rest of the chain on it (predictors are applied to partial output too). `LimitExceeded` and `Unsupported` are still errors.

## Behaviour

| Filter | Decode | Encode |
|---|---|---|
| FlateDecode | zlib header checked, then inflated raw: a missing or wrong Adler-32 is tolerated; no valid header → raw deflate; trailing bytes ignored; preset dictionary → Corrupt; truncated → Corrupt (partial in tolerant mode) | zlib level 6 |
| LZWDecode | 9–12-bit MSB-first codes, clear (256), EOD (257), `EarlyChange` 0/1; missing EOD/initial clear tolerated; a full table stops growing | greedy, leading clear, clear-table when the table fills, EOD |
| Predictors (Flate/LZW) | 2 = TIFF (1/2/4/8/16 bpc), 10–15 = PNG with per-row tags 0–4; partial final rows decoded; bad params or tags → Corrupt | 2, 10 None, 11 Sub, 12 Up, 13 Average, 14 Paeth, 15 per-row optimum (min sum of signed residuals) |
| ASCIIHexDecode | white space ignored, odd digit padded with 0, `>` EOD (optional), other characters → Corrupt | upper case, newline every 64 digits, `>` |
| ASCII85Decode | `<~` prefix, `z`, white space, `~>` (or bare `~`, or none) EOD, final partial group; `z` inside a group, a 1-char final group, overflow or bad characters → Corrupt | `z` for zero groups, newline every 75 characters, `~>` |
| RunLengthDecode | 0–127 literal, 129–255 repeat, 128 EOD (optional); truncated run → Corrupt | runs of ≥2 repeat, literals up to 128, 128 |
| CCITTFaxDecode | not in a chain (`Unsupported`); `decode_ccitt` decodes Group 3 and Group 4 on its own, as rows of 1-bit samples each on a new byte, black 0 unless `black_is_1`, at most `max_rows` rows (fewer with `/Rows`), the rows before any damage, up to 1,048,576 samples a row; Group 3 rows on bytes are read by their end-of-line codes when the data starts with one and on bytes otherwise, whatever `end_of_line` says, so data with codes but none before the first row (not as T.4 sends them) is read on bytes, as the renderer draws it, and at `k` 0 may come out as other rows (`pdfcraft_cos::Document::decoded_image` calls it for a fax image, after the filters before it) | `encode_g4`: Group 4 (`/K -1`) with an end-of-block mark, 0 bits coded black so `/BlackIs1 false` decodes the same bits; rows packed MSB first, each on a new byte, padding bits and trailing bytes ignored; up to 32,767 samples a row (the encoder counts positions in 16 bits); no rows or columns, or too wide → Unsupported, too few bytes → Corrupt |
| JBIG2Decode | not here (`Unsupported`; the renderer and image export decode it with `hayro-jbig2`) | `encode_jbig2_generic`: the embedded organisation (D.3) with a page information segment and an immediate lossless generic region (7.4.6), no `/JBIG2Globals`; template 0 with its nominal AT pixels and typical prediction (6.2.5), coded with the MQ arithmetic coder (Annex E, Table E.1) and ended by its 0xFF 0xAC marker. A 0 sample is coded as a JBIG2 1 (black), since `JBIG2Decode` gives black as 0, so the stream decodes to the same bits; rows packed MSB first, each on a new byte, padding bits and trailing bytes ignored; up to 1,048,576 samples a row; no rows or columns, or too wide → Unsupported, too few bytes → Corrupt. `encode_jbig2`: the smaller of that and symbol coding (`jbig2/symbols.rs`): each distinct mark (8-connected black pixels, at most 256 a side) stored once in a symbol dictionary (7.4.2; arithmetic coded, each bitmap a template 0 generic region without typical prediction, the bitmaps sharing their contexts) and placed by immediate lossless text regions referring to it (7.4.3; bottom-left reference corner, strips of 1, 2, 4 or 8 rows, whichever is smallest), each of at most 10,000 instances over a band of rows, as the decoder PdfCraft renders with (`hayro-jbig2`) takes no more in one region, with larger marks, and in a second plan marks that occur once, in a generic region over their box. Only exact copies share a symbol (no refinement), so the stream decodes to the same bits; fewer than two symbols, more than 2^20 black runs, 65,536 distinct marks or 16 MiB of their pixels, or more than 2^24 rows, and the generic coding is used; a plan of more than 32,767 symbols (that decoder refuses more) isn't made |

## Tests

- Unit tests per filter with hand-computed vectors (the ISO 32000-2 LZW example, RFC 1951 stored blocks, PNG/TIFF rows).
- LZW is cross-checked against `weezl` (dev-dependency) in both directions for both early-change settings.
- `tests/proptests.rs`: encode→decode round trips for every encodable filter × predictor × colors × bpc × columns, two-filter chains, output-limit checks, and fuzz-style no-panic tests (random bytes, random/nonsense params, bit-flipped valid streams).
- `ccitt.rs`: `decode_ccitt` gives back the rows `encode_g4` was given (with `/BlackIs1`, `/Rows` and a row limit), refuses bad widths, and bounds its output for any input; byte-aligned Group 3 in each layout, over 32 seeded pictures, decodes to its picture, and a byte short to fewer rows. Group 4 output decoded by `hayro-ccitt` (an independent decoder) matches the input bits: a text-like page, padding bits, the widest row, and property tests over bitmaps of noise and of runs; any input returns rather than panics.
- `mq.rs`: the arithmetic coder reproduces Annex H.2's 30 bytes from its 256 decisions.
- `jbig2.rs`: output decoded by `hayro-jbig2` (dev-dependency, an independent decoder) matches the input bits: a text-like page (smaller than Group 4), edges, padding, all-white and all-black regions (typical rows), and property tests over bitmaps of noise and of runs; any input returns rather than panics.
- `jbig2/symbols.rs`: the integer coders against Annex A's example and Table A.1; symbol-coded output decoded by `hayro-jbig2` matches the input bits for every plan and strip size: text whose letters repeat (under half the generic coding's size, the dictionary retained and referred to), large and lone marks left to a generic region, marks on the edges, touching at corners and inside each other's boxes, and property tests over glyphs scattered over each other and the edges with stray pixels (never larger than the generic coding); one shape, a blank page and noise are coded generically; marks joined at a corner, either way, are one mark, and 256 pixels a side is a symbol while 257 wide or tall is left out; black padding bits are ignored; coordinates past 4,435 use Annex A's 32-bit range and PREV stays below 512; past 2^20 runs, 2^24 rows, 65,536 distinct marks or 2^24 symbol pixels the image is coded generically, and at each cap it is still looked at; a plan codes at most 32,767 symbols, which decode, and past that the second plan still codes the repeated ones; more than 10,000 marks are placed by several text regions, numbered in turn, the dictionary retained for each but the last; any input returns rather than panics.
- `fuzz/`: a detached cargo-fuzz crate (`cargo +nightly fuzz run decode`, run from `crates/filters/`).

## Status

M1.1/M1.2 for the general-purpose filters are done. Not yet here: the image decoders (DCT via zune-jpeg, JPX via hayro-jpeg2000, JBIG2), DCT encoding, CCITT Group 3 encoding and Flate encode levels other than 6.


Lossless symbol coding is also implemented: exact-copy dictionaries/text regions, bounded by the encoder; no approximate or lossy symbol substitution.
