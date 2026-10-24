# pdfcraft-filters

PDF stream filters: the decoders and encoders behind `/Filter` and `/DecodeParms`.

- **Layer:** L0, standalone. No dependency on any other PdfCraft crate. `cos` depends on it (architecture §3 rule 3).
- **Licence:** MIT OR Apache-2.0. Clean-room: written from ISO 32000-2 §7.4, RFC 1950/1951 (through `flate2` with the pure-Rust `miniz_oxide` backend), TIFF 6.0 §14 (predictor 2) and RFC 2083 §6 (PNG filters). CCITT Group 4 encoding (ITU-T T.6) is the `fax` crate's (MIT, already in the tree through `tiff`), checked here by round trips through an independent decoder. JBIG2 generic region encoding is written here from the JBIG committee's ISO/IEC 14492 Final Committee Draft (WG1 N1359, 1999), the public draft of ITU-T T.88; its Annex H.2 test sequence checks the arithmetic coder byte for byte.
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
encode_jbig2_generic(data, columns, rows) -> Result<Vec<u8>, FilterError>   // 1-bit rows as lossless JBIG2 for JBIG2Decode; columns <= MAX_JBIG2_COLUMNS
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
| CCITTFaxDecode | not here (`Unsupported`; renderers decode it) | `encode_g4`: Group 4 (`/K -1`) with an end-of-block mark, 0 bits coded black so `/BlackIs1 false` decodes the same bits; rows packed MSB first, each on a new byte, padding bits and trailing bytes ignored; up to 32,767 samples a row (the encoder counts positions in 16 bits); no rows or columns, or too wide → Unsupported, too few bytes → Corrupt |
| JBIG2Decode | not here (`Unsupported`; the renderer and image export decode it with `hayro-jbig2`) | `encode_jbig2_generic`: the embedded organisation (D.3) with a page information segment and an immediate lossless generic region (7.4.6), no `/JBIG2Globals`; template 0 with its nominal AT pixels and typical prediction (6.2.5), coded with the MQ arithmetic coder (Annex E, Table E.1) and ended by its 0xFF 0xAC marker. A 0 sample is coded as a JBIG2 1 (black), since `JBIG2Decode` gives black as 0, so the stream decodes to the same bits; rows packed MSB first, each on a new byte, padding bits and trailing bytes ignored; up to 1,048,576 samples a row; no rows or columns, or too wide → Unsupported, too few bytes → Corrupt |

## Tests

- Unit tests per filter with hand-computed vectors (the ISO 32000-2 LZW example, RFC 1951 stored blocks, PNG/TIFF rows).
- LZW is cross-checked against `weezl` (dev-dependency) in both directions for both early-change settings.
- `tests/proptests.rs`: encode→decode round trips for every encodable filter × predictor × colors × bpc × columns, two-filter chains, output-limit checks, and fuzz-style no-panic tests (random bytes, random/nonsense params, bit-flipped valid streams).
- `ccitt.rs`: Group 4 output decoded by `hayro-ccitt` (dev-dependency, an independent decoder) matches the input bits: a text-like page, padding bits, the widest row, and property tests over bitmaps of noise and of runs; any input returns rather than panics.
- `mq.rs`: the arithmetic coder reproduces Annex H.2's 30 bytes from its 256 decisions.
- `jbig2.rs`: output decoded by `hayro-jbig2` (dev-dependency, an independent decoder) matches the input bits: a text-like page (smaller than Group 4), edges, padding, all-white and all-black regions (typical rows), and property tests over bitmaps of noise and of runs; any input returns rather than panics.
- `fuzz/`: a detached cargo-fuzz crate (`cargo +nightly fuzz run decode`, run from `crates/filters/`).

## Status

M1.1/M1.2 for the general-purpose filters are done. Not yet here: the image codecs (DCT via zune-jpeg, JPX via hayro-jpeg2000, JBIG2 and CCITT decode, DCT encode), CCITT Group 3 encoding, JBIG2 symbol coding (text regions, symbol dictionaries, lossy) and Flate encode levels other than 6.
