//! JBIG2 decoding for `JBIG2Decode` (ISO 32000-2 §7.4.7), through `hayro-jbig2`.

use crate::FilterError;
use crate::ccitt::Rows;

/// The largest JBIG2 page [`decode_jbig2_within`] decodes, in pixels. The decoder decodes a page whole,
/// and takes pages of up to about this many (20,000 × 10,000 decodes).
pub const MAX_JBIG2_PIXELS: u64 = 1 << 28;

/// A `JBIG2Decode` stream's `data`, with what its `/JBIG2Globals` stream decodes to, decoded as the
/// filter gives it: `rows` rows of `columns` 1-bit samples, most significant bit first, each
/// starting on a new byte, with black as 0 (JBIG2 codes black as 1).
///
/// The JBIG2 page has to be the image's size, `columns` × `rows`: PdfCraft's renderer draws the
/// whole page into the image, so the rows of a page of another height wouldn't be the image's. The
/// decoder decodes the page whole, so it is checked before it is decoded, and has to fit in `max`
/// bytes as such rows and have at most [`MAX_JBIG2_PIXELS`]. Regions inside it are bounded by the
/// decoder alone: a region may claim more pixels than its page, and is decoded before it is cut to
/// the page. With no rows asked for, nothing is decoded.
///
/// Errors: `data` or `globals` that don't decode, or a page of another size
/// ([`FilterError::Corrupt`]); a page past `max` ([`FilterError::LimitExceeded`]) or
/// [`MAX_JBIG2_PIXELS`] ([`FilterError::Unsupported`], or `Corrupt` from the decoder, which has the
/// same cap and can refuse the page first).
///
/// Decoding spends from `work`: what decoding may still cost across a run of images, in the
/// decoder's count of steps. There is no way in without it, so a run of images can't decode them
/// without sharing an allowance. Reading `data` and `globals` is charged first
/// ([`hayro_jbig2::charge_reading`]), so an image that turns out not to decode has paid for that,
/// then what decoding takes ([`hayro_jbig2::Image::decode_within`]). An image takes at most what
/// one decode may on top of its reading and its page, whatever `work` allows, and one that `work`
/// can't pay for spends all of it. With nothing left an image isn't read at all, so the images after
/// that one cost nothing but this check, however large their globals.
pub fn decode_jbig2_within(data: &[u8], globals: Option<&[u8]>, columns: u32, rows: u32, max: usize, work: &mut u64) -> Result<Vec<u8>, FilterError> {
    if rows == 0 {
        return Ok(Vec::new());
    }
    if *work == 0 {
        return Err(FilterError::Spent);
    }
    hayro_jbig2::charge_reading(work, data.len().saturating_add(globals.map_or(0, <[u8]>::len))).map_err(|_| FilterError::Spent)?;
    let corrupt = |detail: String| FilterError::Corrupt { filter: "JBIG2Decode", detail };
    let image = hayro_jbig2::Image::new_embedded(data, globals).map_err(|e| corrupt(e.to_string()))?;
    let (width, height) = (image.width(), image.height());
    if (width, height) != (columns, rows) {
        return Err(corrupt(format!("a page of {width} × {height} pixels for an image of {columns} × {rows}")));
    }
    if u64::from(width) * u64::from(height) > MAX_JBIG2_PIXELS {
        return Err(FilterError::Unsupported(format!("JBIG2 pages of {width} × {height} pixels")));
    }
    // u32 to usize: wasm32 and up have at least 32-bit usize.
    let (width, height) = (width as usize, height as usize);
    if width.div_ceil(8).checked_mul(height).is_none_or(|bytes| bytes > max) {
        return Err(FilterError::LimitExceeded(max));
    }
    let mut out = Rows::new(width, height, false);
    // Stopped by what was left, it isn't a damaged image, whatever the decoder calls it.
    image.decode_within(&mut out, work).map_err(|e| if *work == 0 { FilterError::Spent } else { corrupt(e.to_string()) })?;
    Ok(out.out)
}

/// JBIG2 codes black as 1, which `JBIG2Decode` gives as 0.
impl hayro_jbig2::Decoder for Rows {
    fn push_pixel(&mut self, black: bool) {
        self.push(!black, 1);
    }
    fn push_pixel_chunk(&mut self, black: bool, chunks: u32) {
        self.push(!black, (chunks as usize).saturating_mul(8));
    }
    fn next_line(&mut self) {
        self.end_row();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{encode_jbig2, encode_jbig2_generic};

    /// [`decode_jbig2_within`] with no limit on the work.
    fn decode_jbig2(data: &[u8], globals: Option<&[u8]>, columns: u32, rows: u32, max: usize) -> Result<Vec<u8>, FilterError> {
        let mut unbounded = u64::MAX;
        decode_jbig2_within(data, globals, columns, rows, max, &mut unbounded)
    }

    /// `rows` rows of `columns` samples of noise, the bits that pad each row to a byte 0.
    fn noise(columns: u32, rows: u32) -> Vec<u8> {
        let stride = (columns as usize).div_ceil(8);
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut data: Vec<u8> = (0..stride * rows as usize)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed as u8
            })
            .collect();
        let pad = (stride * 8 - columns as usize) as u32;
        data.chunks_mut(stride).for_each(|row| *row.last_mut().unwrap() &= 0xFFu8.checked_shl(pad).unwrap_or(0));
        data
    }

    /// The same stream with its page information (the first segment's data, after its 11-byte
    /// header: width, then height) claiming a page of `width` × `height`.
    fn page_of(data: &[u8], width: u32, height: u32) -> Vec<u8> {
        let mut data = data.to_vec();
        data[11..15].copy_from_slice(&width.to_be_bytes());
        data[15..19].copy_from_slice(&height.to_be_bytes());
        data
    }

    /// Either coding decodes to its samples, black as 0, the bits that pad each row 0: a generic
    /// region of noise, and symbols for letters repeated across the page. With no rows asked for,
    /// nothing is decoded.
    #[test]
    fn both_codings_decode_to_their_samples() {
        let (w, h) = (203u32, 41u32);
        let bits = noise(w, h);
        let generic = encode_jbig2_generic(&bits, w, h).unwrap();
        assert_eq!(decode_jbig2(&generic, None, w, h, usize::MAX).unwrap(), bits);
        assert_eq!(decode_jbig2(b"not JBIG2", None, w, 0, usize::MAX).unwrap(), b"", "no rows asked for");

        // Four lines of three letters, black on white.
        const LETTERS: [[&str; 7]; 3] = [
            ["#####", "#....", "####.", "#....", "#....", "#....", "#####"],
            [".###.", "#...#", "#....", "#....", "#....", "#...#", ".###."],
            ["#...#", "#...#", "#####", "#...#", "#...#", "#...#", "#...#"],
        ];
        let (w, h) = (400u32, 40u32);
        let stride = (w as usize).div_ceil(8);
        let mut marks = vec![0xFFu8; stride * h as usize];
        for (line, top) in [2usize, 12, 22, 32].into_iter().enumerate() {
            for (k, x) in (3..w as usize - 6).step_by(7).enumerate() {
                for (dy, row) in LETTERS[(k + line) % 3].iter().enumerate() {
                    for (dx, _) in row.bytes().enumerate().filter(|(_, c)| *c == b'#') {
                        marks[(top + dy) * stride + (x + dx) / 8] &= !(0x80 >> ((x + dx) % 8));
                    }
                }
            }
        }
        let symbols = encode_jbig2(&marks, w, h).unwrap();
        assert!(symbols.len() < encode_jbig2_generic(&marks, w, h).unwrap().len(), "symbol coded");
        assert_eq!(decode_jbig2(&symbols, None, w, h, usize::MAX).unwrap(), marks);
    }

    /// What a page costs is the same however much a run has left. Given exactly that it decodes,
    /// and less stops it: spending all that's left, and what comes after isn't read. A page that
    /// fails still spends what it took.
    #[test]
    fn a_run_of_images_spends_its_allowance_down_to_the_step() {
        let (w, h) = (203u32, 41u32);
        let data = encode_jbig2_generic(&noise(w, h), w, h).unwrap();
        let whole = decode_jbig2(&data, None, w, h, usize::MAX).unwrap();
        let mut work = u64::MAX;
        assert_eq!(decode_jbig2_within(&data, None, w, h, usize::MAX, &mut work).unwrap(), whole);
        let cost = u64::MAX - work;
        assert!(cost >= u64::from(w) * u64::from(h), "a pixel at a time: {cost}");
        let mut work = cost;
        assert_eq!(decode_jbig2_within(&data, None, w, h, usize::MAX, &mut work).unwrap(), whole, "exactly enough");
        assert_eq!(work, 0);
        let mut work = cost + 7;
        decode_jbig2_within(&data, None, w, h, usize::MAX, &mut work).unwrap();
        assert_eq!(work, 7, "only what it took");
        let mut work = cost - 1;
        assert!(matches!(decode_jbig2_within(&data, None, w, h, usize::MAX, &mut work), Err(FilterError::Spent)), "a step short");
        assert_eq!(work, 0, "spends all that was left");
        // Nothing left: not read at all, so this garbage isn't reported as garbage.
        assert!(matches!(decode_jbig2_within(b"garbage", None, w, h, usize::MAX, &mut work), Err(FilterError::Spent)));
        // Too little even for its page.
        let mut work = 3;
        assert!(decode_jbig2_within(&data, None, w, h, usize::MAX, &mut work).is_err());
        assert_eq!(work, 0);
        // A region that can't be read after the one that can: the page fails having spent the
        // first, and the segment it fails on, and that isn't being out of allowance.
        let bad_region = [0, 0, 0, 9, 38, 0, 1, 0, 0, 0, 1, 0xFF];
        let mut work = u64::MAX;
        assert!(decode_jbig2_within(&[&data[..], &bad_region].concat(), None, w, h, usize::MAX, &mut work).is_err());
        assert_eq!(u64::MAX - work, cost + 1 + 4 * bad_region.len() as u64, "the first region, a segment more, and reading it");
        // Not a stream at all: it pays for being read, and globals count with it.
        let mut work = u64::MAX;
        assert!(matches!(decode_jbig2_within(b"garbage", Some(b"globals"), w, h, usize::MAX, &mut work), Err(FilterError::Corrupt { .. })));
        assert_eq!(u64::MAX - work, 4 * 14);
    }

    /// The page has to be the image's size, and is bounded before it is decoded, being decoded
    /// whole: within `max` as rows of one bit, and within [`MAX_JBIG2_PIXELS`]. Globals are given
    /// to the decoder: ones it skips decode, ones that aren't JBIG2 are an error, as is damaged
    /// data, which never panics.
    #[test]
    fn what_it_cannot_decode_is_an_error() {
        let (w, h) = (203u32, 41u32);
        let data = encode_jbig2_generic(&noise(w, h), w, h).unwrap();
        for (columns, rows) in [(w - 1, h), (w + 1, h), (w, h - 1), (w, h + 1)] {
            let e = decode_jbig2(&data, None, columns, rows, usize::MAX).unwrap_err();
            assert!(matches!(e, FilterError::Corrupt { .. }), "{columns} × {rows}: {e}");
        }
        let page = 26 * h as usize;
        assert!(matches!(decode_jbig2(&data, None, w, h, page - 1), Err(FilterError::LimitExceeded(m)) if m == page - 1), "a byte short");
        assert_eq!(decode_jbig2(&data, None, w, h, page).unwrap().len(), page, "the page within max");
        // 16,384 × 16,384 is 2^28 pixels, which decodes; a row more is past the cap, here or in the
        // decoder, which has the same one and can refuse it first.
        let large = |rows| decode_jbig2(&page_of(&data, 16_384, rows), None, 16_384, rows, usize::MAX);
        assert!(matches!(large(16_385), Err(FilterError::Unsupported(_) | FilterError::Corrupt { .. })), "{:?}", large(16_385));
        assert!(large(16_384).is_ok(), "{:?}", large(16_384).map(|page| page.len()));

        // An extension segment (ISO/IEC 14492 7.4.14), which a decoder skips: its 11-byte header
        // gives the length of the data after it.
        let skipped = [0, 0, 0, 0, 0x3E, 0, 0, 0, 0, 0, 4, 0, 0, 0, 1];
        let generic = decode_jbig2(&data, None, w, h, usize::MAX).unwrap();
        assert_eq!(decode_jbig2(&data, Some(&skipped), w, h, usize::MAX).unwrap(), generic);
        assert!(matches!(decode_jbig2(&data, Some(b"not JBIG2"), w, h, usize::MAX), Err(FilterError::Corrupt { .. })));
        for end in 0..data.len() {
            let _ = decode_jbig2(&data[..end], None, w, h, usize::MAX);
        }
    }

    /// Pixels past a bitmap's edge are 0 (ISO/IEC 14492 6.3.5.2). A page whose default pixel is 1,
    /// and an intermediate text or halftone region whose default pixel is 1, kept 1s past their
    /// width in their last word of each row, and a refinement, which reads its reference a word at
    /// a time, read them as pixels: of rows not a multiple of 32 wide, it decoded to other pixels
    /// than were coded. Each stream is a refinement coded by the standard's procedures from a
    /// reference read as 0 past its edge, and decodes to the picture given, `#` for black.
    /// Vendored hayro-jbig2 patch 13.
    #[test]
    fn a_refinement_reads_nothing_past_its_references_width() {
        // A page of 7 × 6, then of 33 × 6, whose default pixel is 1, refined whole.
        const PAGE_7: &str = "000000003000010000001300000007000000060000000000000000460000000000012a00010000001e000000070000000600000000000000000400ffffffffe42d7e889ebcffac";
        const PAGE_33: &str = "000000003000010000001300000021000000060000000000000000460000000000012a000100000034000000210000000600000000000000000400ffffffff314a8d598037eb3f915ab3d526ccae915ad6a6a7b814111b124ae60fffac";
        // An intermediate text region of 31 × 6 whose default pixel is 1, then one of 7 × 6 on a
        // page of 40, each refined into the page.
        const TEXT_31: &str = "00000000300001000000130000001f0000000600000000000000004200000000000100000100000030000003fffdff02fefefe00000001000000014f96dceb76c7f1ff585971969cd9f88e538261a6f713be68d77e8e1fffac00000002042001010000001b0000001f00000006000000000000000000029000000001a943ffac000000032a2002010000002f0000001f0000000600000000000000000400ffffffff9ac311688adbc8f570a2f96aef93d332e3b1f4b27b704bffac";
        const TEXT_7: &str = "000000003000010000001300000028000000060000000000000000420000000000010000010000001d000003fffdff02fefefe00000001000000014fe1f79f4a95c5b77fffac00000002042001010000001b0000000700000006000000000000000000029000000001a943ffac000000032a2002010000001e000000070000000600000000000000000400ffffffffa71dae12bd7fffac";
        // An intermediate halftone region of 33 × 6 whose default pixel is 1, refined into the page.
        const HALFTONE_33: &str = "000000003000010000001300000021000000060000000000000000420000000000011000010000000a000101000000011fffac00000002142001010000004100000021000000060000000000000000009000000021000000060000000000000000010000004ea6618f551d48ff24c2c5c98124d449f2adea6fca76e4297fffac000000032a20020100000032000000210000000600000000000000000400ffffffffe87db809393a3251e565e7a749e6470ab37f7aef186ceaf5167bffac";
        let cases: [(&str, &str, [&str; 6]); 5] = [
            ("page 7", PAGE_7, ["###.#.#", ".......", "#.####.", "######.", "#.#...#", ".#...##"]),
            (
                "page 33",
                PAGE_33,
                [
                    ".#..##........###..####....#.##.#",
                    ".#.###.###.##.##..#..#...#.....##",
                    ".....##..#....#.##.######.###..#.",
                    "##...#..##.###..###.##..#.#.#.#.#",
                    ".##.#........##.#.#.....###......",
                    "...#.#####.#..#.###......###..###",
                ],
            ),
            (
                "text 31",
                TEXT_31,
                [
                    "....##....###......#.##.#.#....",
                    ".#.....##.......##..###.#...#..",
                    ".#.#.#.####...#####.####...#.#.",
                    "......#...#...#.##...##...#..#.",
                    ".#..##.###.#.###..####...#...#.",
                    "#...#...#..#.#....#.##.#.....#.",
                ],
            ),
            (
                "text 7 on a page of 40",
                TEXT_7,
                [
                    "......#.................................",
                    ".....##.................................",
                    "##...##.................................",
                    "..#.....................................",
                    "##......................................",
                    "..#.....................................",
                ],
            ),
            (
                "halftone 33",
                HALFTONE_33,
                [
                    "##...#.###.#.###.....#.####..####",
                    "##.##.#...#.##.##.##.#...#.#.###.",
                    "###..#.#....##.###...#..##.##..##",
                    "##..#...###...##..#.####.###.#...",
                    "####.###.###.#..#..#.#.####...###",
                    "#.#..#........#....#.#.##.##..#.#",
                ],
            ),
        ];
        let hex = |text: &str| (0..text.len() / 2).map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap()).collect::<Vec<u8>>();
        for (form, stream, picture) in cases {
            let w = picture[0].len();
            // As the filter gives it: black as 0, each row padded to a byte with 0s.
            let mut rows = vec![0u8; w.div_ceil(8) * picture.len()];
            for (y, row) in picture.iter().enumerate() {
                for (x, _) in row.bytes().enumerate().filter(|(_, c)| *c == b'.') {
                    rows[y * w.div_ceil(8) + x / 8] |= 0x80 >> (x % 8);
                }
            }
            assert_eq!(decode_jbig2(&hex(stream), None, w as u32, 6, usize::MAX).unwrap(), rows, "{form}");
        }
    }
}
