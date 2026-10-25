//! PdfCraft patch (54): images too large to decode at their full size are decoded at a reduced
//! one.
//!
//! An image over [`MAX_FULL_PIXELS`] (a 36 × 24 in plan scanned at 600 dpi is 311 million) was
//! not drawn at all (#4071). Its filters still produce their samples at full size, packed as the
//! stream holds them (a 1-bit scan stays 1 bit per pixel, so 311 million pixels are 39 MB), and
//! [`reduce`] shrinks them row by row by whole factors before anything expands a sample to a byte
//! or a colour: the full-size image never exists as bytes per pixel, RGB or RGBA. The reduced
//! size follows the size the image is drawn at, within [`MAX_REDUCED_BYTES`] once converted, so
//! what the renderer holds afterwards is no more than for an ordinary image.

/// Largest image decoded at its full size, in pixels: the renderer's `MAX_IMAGE_PIXELS`.
pub(crate) const MAX_FULL_PIXELS: u64 = 1 << 28;

/// Largest image decoded at all, in pixels (a reduced decode still reads every sample). About
/// a gigapixel: an A0 sheet at 600 dpi is 558 million, a 48 × 36 in one 622 million. A fuzzed
/// `/W 4294967295` is refused before anything is decoded or allocated.
pub(crate) const MAX_SOURCE_PIXELS: u64 = 1 << 30;

/// Most bytes of decoded samples an image over [`MAX_FULL_PIXELS`] may have at its full size
/// (what its last filter produces): a gigapixel at 1 bit is 128 MiB, and 8-bit gray stops at
/// 512 million pixels. Larger images, among them colour ones past 2^28 pixels, are not drawn.
pub(crate) const MAX_SOURCE_BYTES: u64 = 512 << 20;

/// Most bytes a reduced image takes once converted for drawing (1 a pixel for gray and masks, 3
/// for colour): within the decoded-image cache's budget, so the tiles of a page share one decode.
const MAX_REDUCED_BYTES: u64 = 32 << 20;

/// Longest side of a reduced image (the renderer's pixmaps hold at most `u16::MAX`).
const MAX_REDUCED_SIDE: u64 = 1 << 15;

/// Most components a reduced image may have (DeviceN allows 32).
const MAX_COMPONENTS: u8 = 32;

/// Whether an image of `width` × `height` pixels is decoded at a reduced size.
pub(crate) fn is_over_full_size(width: u32, height: u32) -> bool {
    u64::from(width) * u64::from(height) > MAX_FULL_PIXELS
}

/// The bytes of samples an image of `width` × `height` pixels with `components` components of
/// `bpc` bits has at its full size, rows padded to whole bytes as streams hold them.
pub(crate) fn source_bytes(width: u32, height: u32, components: u8, bpc: u8) -> u64 {
    let row_bits = u64::from(width) * u64::from(components) * u64::from(bpc);
    row_bits.div_ceil(8).saturating_mul(u64::from(height))
}

/// Whether [`reduce`] takes samples of `bpc` bits and `components` components.
pub(crate) fn reducible(components: u8, bpc: u8) -> bool {
    (1..=MAX_COMPONENTS).contains(&components) && (1..=16).contains(&bpc)
}

/// The whole factors (x, y) by which an image of `width` × `height` pixels is reduced to be
/// drawn at `target` device pixels, `bytes_per_pixel` bytes a pixel once reduced and converted:
/// the largest that keep it at least the size of `target`, then larger if the result would
/// exceed [`MAX_REDUCED_BYTES`] or [`MAX_REDUCED_SIDE`]. Never larger than the image itself.
pub(crate) fn factors(
    width: u32,
    height: u32,
    target: Option<(u32, u32)>,
    bytes_per_pixel: u64,
) -> (u32, u32) {
    let (w, h) = (u64::from(width.max(1)), u64::from(height.max(1)));
    let budget = (MAX_REDUCED_BYTES / bytes_per_pixel.max(1)).max(1);
    let (tw, th) = target.map_or((w, h), |(tw, th)| {
        (u64::from(tw.max(1)), u64::from(th.max(1)))
    });
    let fx = (w / tw).max(w.div_ceil(MAX_REDUCED_SIDE)).max(1);
    let fy = (h / th).max(h.div_ceil(MAX_REDUCED_SIDE)).max(1);
    // The smallest common factor within the budget. The reduced size only shrinks as `k` grows,
    // so a binary search finds it (at most 32 steps); at the longer side it is one pixel.
    let fits = |k: u64| w.div_ceil(fx.max(k)).saturating_mul(h.div_ceil(fy.max(k))) <= budget;
    let (mut low, mut k) = (1, w.max(h));
    while low < k {
        let mid = low + (k - low) / 2;
        if fits(mid) {
            k = mid;
        } else {
            low = mid + 1;
        }
    }
    let clamp = |f: u64, side: u64| u32::try_from(f.max(k).min(side)).unwrap_or(u32::MAX);
    (clamp(fx, w), clamp(fy, h))
}

/// How [`reduce`] makes one pixel of a block of source pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sampling {
    /// The mean of each component, as 8 bits (a box filter). For components whose decoded value
    /// is linear in the sample: not palette indices, not samples compared with a colour key.
    /// The mean is taken of the raw samples, before colour conversion: exact for gray and RGB
    /// up to gamma, and an approximation where the conversion isn't linear (DeviceN and
    /// Separation tint transforms, Lab, ICC profiles, CMYK), as any resampling before
    /// conversion is.
    Average,
    /// The block's middle pixel, at the image's own bit depth.
    Nearest,
}

/// An image reduced by [`reduce`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Reduced {
    /// Rows of `width` pixels, each padded to a whole byte. Only the rows made from source rows
    /// present: a short stream gives fewer than `height` rows, as it does unreduced.
    pub(crate) data: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) bits_per_component: u8,
}

/// `data`, rows of `width` pixels of `components` components of `bpc` bits each (rows padded to
/// whole bytes; missing bytes read as zero), reduced by `factors`. `None` for a depth this does
/// not take ([`reducible`]), an image over [`MAX_SOURCE_PIXELS`], or when its buffer can't be
/// allocated.
pub(crate) fn reduce(
    data: &[u8],
    width: u32,
    height: u32,
    components: u8,
    bpc: u8,
    factors: (u32, u32),
    sampling: Sampling,
) -> Option<Reduced> {
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 || pixels > MAX_SOURCE_PIXELS || !reducible(components, bpc) {
        return None;
    }
    let (fx, fy) = (factors.0.clamp(1, width), factors.1.clamp(1, height));
    let n = usize::from(components);
    let row_bytes = usize::try_from(source_bytes(width, 1, components, bpc)).ok()?;
    let rows_present = data.len().div_ceil(row_bytes).min(usize::try_from(height).ok()?);
    let (fx_u, fy_u) = (usize::try_from(fx).ok()?, usize::try_from(fy).ok()?);
    let width_u = usize::try_from(width).ok()?;
    let out_width = width.div_ceil(fx);
    let out_width_u = usize::try_from(out_width).ok()?;
    let out_rows = rows_present.div_ceil(fy_u);
    let out_bpc = match sampling {
        Sampling::Average => 8,
        Sampling::Nearest => bpc,
    };
    let out_row_bytes = usize::try_from(source_bytes(out_width, 1, components, out_bpc)).ok()?;
    let mut out = Vec::new();
    out.try_reserve_exact(out_rows.checked_mul(out_row_bytes)?).ok()?;
    let row = |y: usize| -> &[u8] {
        let start = y.saturating_mul(row_bytes);
        let end = start.saturating_add(row_bytes).min(data.len());
        data.get(start..end).unwrap_or(&[])
    };

    match sampling {
        Sampling::Average => {
            let mut sums = Vec::new();
            sums.try_reserve_exact(out_width_u.checked_mul(n)?).ok()?;
            sums.resize(out_width_u * n, 0u64);
            let max = (1u64 << bpc) - 1;
            for oy in 0..out_rows {
                sums.fill(0);
                let y0 = oy * fy_u;
                let y1 = y0.saturating_add(fy_u).min(rows_present);
                for y in y0..y1 {
                    add_row(row(y), width_u, n, bpc, fx_u, &mut sums);
                }
                let rows_in = (y1 - y0) as u64;
                for (ox, block) in sums.chunks_exact(n).enumerate() {
                    let cols_in = (width_u - ox * fx_u).min(fx_u) as u64;
                    // A block has at most 2^30 samples of at most 16 bits: within u64.
                    let whole = cols_in * rows_in * max;
                    for sum in block {
                        out.push(((sum * 255 + whole / 2) / whole.max(1)) as u8);
                    }
                }
            }
        }
        Sampling::Nearest => {
            let mut out_row = Vec::new();
            out_row.try_reserve_exact(out_row_bytes).ok()?;
            out_row.resize(out_row_bytes, 0u8);
            for oy in 0..out_rows {
                let source = row((oy * fy_u + fy_u / 2).min(rows_present - 1));
                out_row.fill(0);
                let mut at = 0;
                for ox in 0..out_width_u {
                    let x = (ox * fx_u + fx_u / 2).min(width_u - 1);
                    for c in 0..n {
                        put_sample(&mut out_row, at, bpc, sample(source, x * n + c, bpc));
                        at += usize::from(bpc);
                    }
                }
                out.extend_from_slice(&out_row);
            }
        }
    }

    Some(Reduced {
        data: out,
        width: out_width,
        height: height.div_ceil(fy),
        bits_per_component: out_bpc,
    })
}

/// Adds one source row's samples to the sums of the blocks of `fx` pixels they fall in.
fn add_row(row: &[u8], width: usize, n: usize, bpc: u8, fx: usize, sums: &mut [u64]) {
    if bpc == 1 && n == 1 {
        // The common large image, a 1-bit scan: count the set bits of each block at once.
        for (ox, sum) in sums.iter_mut().enumerate() {
            let start = ox * fx;
            *sum += ones(row, start, (start + fx).min(width));
        }
        return;
    }
    let mut ox = 0;
    let mut in_block = 0;
    for x in 0..width {
        for c in 0..n {
            let i = x * n + c;
            let value = if bpc == 8 {
                row.get(i).copied().map_or(0, u32::from)
            } else {
                sample(row, i, bpc)
            };
            if let Some(sum) = sums.get_mut(ox * n + c) {
                *sum += u64::from(value);
            }
        }
        in_block += 1;
        if in_block == fx {
            in_block = 0;
            ox += 1;
        }
    }
}

/// The set bits of `row` from bit `start` up to (not including) `end`, most significant first.
fn ones(row: &[u8], start: usize, end: usize) -> u64 {
    let mut count = 0;
    let mut bit = start;
    while bit < end {
        let offset = bit % 8;
        let take = (8 - offset).min(end - bit);
        // Bits `offset..offset + take` of the byte, counted from its most significant bit.
        let mask = ((0xffu16 >> offset) & (0xffu16 << (8 - offset - take))) as u8;
        count += u64::from((row.get(bit / 8).copied().unwrap_or(0) & mask).count_ones());
        bit += take;
    }
    count
}

/// Sample `index` of `row` at `bpc` bits (1 to 16), most significant bit first; zero past the end.
fn sample(row: &[u8], index: usize, bpc: u8) -> u32 {
    let bit = index.saturating_mul(usize::from(bpc));
    let byte = bit / 8;
    let shift = (bit % 8) as u32;
    let mut window = 0u32;
    for i in 0..3 {
        let b = byte.checked_add(i).and_then(|i| row.get(i)).copied();
        window = (window << 8) | u32::from(b.unwrap_or(0));
    }
    // 24 bits hold the sample: it starts at most 7 bits in and is at most 16 bits long.
    (window >> (24 - shift - u32::from(bpc))) & ((1 << bpc) - 1)
}

/// Writes `value`'s low `bpc` bits into `row` at bit `at`, most significant first.
fn put_sample(row: &mut [u8], at: usize, bpc: u8, value: u32) {
    for i in 0..usize::from(bpc) {
        if (value >> (usize::from(bpc) - 1 - i)) & 1 == 1 {
            let bit = at + i;
            if let Some(byte) = row.get_mut(bit / 8) {
                *byte |= 0x80 >> (bit % 8);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_bit_blocks_average_their_set_bits() {
        // 10 × 2 pixels, rows padded to 2 bytes: row 0 is 1111000011, row 1 all ones.
        let data = [0b1111_0000, 0b1100_0000, 0xff, 0xff];
        let r = reduce(&data, 10, 2, 1, 1, (4, 2), Sampling::Average).unwrap();
        assert_eq!((r.width, r.height, r.bits_per_component), (3, 1, 8));
        // Blocks of 4, 4 and 2 columns: 8/8, 4/8 and 4/4 set.
        assert_eq!(r.data, [255, 128, 255]);
    }

    #[test]
    fn eight_and_sixteen_bit_colour_average_per_component() {
        let data = [0, 100, 200, 10, 110, 210];
        let r = reduce(&data, 2, 1, 3, 8, (2, 1), Sampling::Average).unwrap();
        assert_eq!(r.data, [5, 105, 205]);
        let data = [0xff, 0xff, 0x00, 0x00];
        let r = reduce(&data, 2, 1, 1, 16, (2, 1), Sampling::Average).unwrap();
        assert_eq!(r.data, [128]);
    }

    #[test]
    fn nearest_keeps_the_bit_depth_and_takes_the_middle_pixel() {
        // 4-bit indices 0..8 in one row; blocks of 3 take indices 1, 4 and 7.
        let data = [0x01, 0x23, 0x45, 0x67, 0x80];
        let r = reduce(&data, 9, 1, 1, 4, (3, 1), Sampling::Nearest).unwrap();
        assert_eq!((r.width, r.bits_per_component), (3, 4));
        assert_eq!(r.data, [0x14, 0x70]);
    }

    #[test]
    fn short_data_gives_the_rows_present() {
        // 8 × 4 pixels at 1 bit with only the first row and a half present.
        let r = reduce(&[0xff], 8, 4, 1, 1, (2, 2), Sampling::Average).unwrap();
        assert_eq!((r.width, r.height), (4, 2));
        assert_eq!(r.data, [255; 4], "one output row, from the one source row present");
        assert!(reduce(&[], 8, 4, 1, 1, (2, 2), Sampling::Average).unwrap().data.is_empty());
    }

    #[test]
    fn factors_follow_the_target_within_the_budget() {
        // 21600 × 14400 drawn at 1350 × 900: reduced 16 times, to 1350 × 900.
        assert_eq!(factors(21_600, 14_400, Some((1350, 900)), 1), (16, 16));
        // Drawn at full size: as large as the budget allows.
        let (fx, fy) = factors(21_600, 14_400, Some((21_600, 14_400)), 1);
        let pixels = u64::from(21_600u32.div_ceil(fx)) * u64::from(14_400u32.div_ceil(fy));
        assert!(pixels <= MAX_REDUCED_BYTES, "{fx} × {fy}");
        assert_eq!((fx, fy), (4, 4));
        // A one-row strip a gigapixel long stays within the longest side.
        let (fx, fy) = factors(1 << 30, 1, None, 3);
        assert_eq!(fy, 1);
        assert!(u64::from((1u32 << 30).div_ceil(fx)) <= MAX_REDUCED_SIDE);
        // Never past the image: a 1 × 1 target of a 3 × 3 image.
        assert_eq!(factors(3, 3, Some((0, 0)), 1), (3, 3));
    }

    #[test]
    fn unsupported_depths_and_sizes_are_refused() {
        assert!(reduce(&[0], 1, 1, 1, 32, (1, 1), Sampling::Average).is_none());
        assert!(reduce(&[0], 1, 1, 0, 8, (1, 1), Sampling::Average).is_none());
        assert!(reduce(&[0], 0, 1, 1, 8, (1, 1), Sampling::Average).is_none());
    }
}
