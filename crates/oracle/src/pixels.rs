//! Comparing a rendered page against an independent renderer's pixels.
//!
//! A raw pixel-for-pixel difference is useless for this. Two correct renderers disagree on every
//! glyph edge — antialiasing coverage, hinting, and where a subpixel-positioned stem lands — so a
//! strict comparison reports a page as badly wrong when it is perfect, and the number it produces
//! tells you nothing about whether a shading is missing or a glyph is the wrong one.
//!
//! [`compare`] uses the usual remedy: a pixel counts as differing only when it differs from
//! **every** pixel in the other image's 3×3 neighbourhood by more than `tolerance`. A one-pixel
//! shift in a stem, or a differently-weighted edge, finds a near match next door and is forgiven.
//! A glyph that is not there, a shading that was not drawn, or a region painted the wrong colour
//! has no near match anywhere nearby and is counted.
//!
//! The test is run in both directions and the worse result reported, because the one-directional
//! form is asymmetric in a way that flatters whichever image is blurrier: thin content missing
//! from A is forgiven when scanning A against B's neighbourhood if B's stroke happens to be wide.

/// A single-channel (grayscale) image borrowed for comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gray<'a> {
    width: usize,
    height: usize,
    pixels: &'a [u8],
}

impl<'a> Gray<'a> {
    /// Borrow `pixels` as a `width` × `height` image, or `None` when the buffer is not exactly
    /// that size — a truncated decode must not be compared as if it were a short page.
    pub fn new(width: usize, height: usize, pixels: &'a [u8]) -> Option<Self> {
        (width.checked_mul(height)? == pixels.len()).then_some(Self { width, height, pixels })
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// The pixel at `(x, y)`, or `None` outside the image.
    pub fn get(&self, x: usize, y: usize) -> Option<u8> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.pixels.get(y.checked_mul(self.width)?.checked_add(x)?).copied()
    }
}

/// Why two images could not be compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeMismatch {
    pub a: (usize, usize),
    pub b: (usize, usize),
}

impl std::fmt::Display for SizeMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cannot compare a {}x{} image with a {}x{} one; render both at the same scale", self.a.0, self.a.1, self.b.0, self.b.1)
    }
}

/// How far apart two renderings are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Diff {
    /// Pixels with no near match in the other image's neighbourhood, in the worse direction.
    pub differing: usize,
    /// Pixels in the image.
    pub total: usize,
    /// The largest single-pixel difference found, before the neighbourhood is considered. A high
    /// value with a low [`Diff::fraction`] is the signature of an edge-only disagreement.
    pub max_delta: u8,
}

impl Diff {
    /// The share of pixels with no near match, in `0.0..=1.0`. Zero for an empty image: nothing
    /// disagreed because there was nothing to disagree about.
    pub fn fraction(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        // total is non-zero, and differing <= total, so this is finite and in 0..=1.
        self.differing as f64 / self.total as f64
    }

    /// `fraction` as a percentage, for reports.
    pub fn percent(&self) -> f64 {
        self.fraction() * 100.0
    }
}

/// Compare two grayscale renderings of the same page at the same size.
///
/// `tolerance` is the per-channel difference tolerated outright; 8 or so absorbs ordinary
/// antialiasing weight differences without hiding a wrong colour.
pub fn compare(a: Gray<'_>, b: Gray<'_>, tolerance: u8) -> Result<Diff, SizeMismatch> {
    if a.width != b.width || a.height != b.height {
        return Err(SizeMismatch { a: (a.width, a.height), b: (b.width, b.height) });
    }
    let max_delta = a.pixels.iter().zip(b.pixels).map(|(&p, &q)| p.abs_diff(q)).max().unwrap_or(0);
    let forward = unmatched(a, b, tolerance);
    let backward = unmatched(b, a, tolerance);
    Ok(Diff { differing: forward.max(backward), total: a.pixels.len(), max_delta })
}

/// Pixels of `a` with no pixel within `tolerance` anywhere in `b`'s surrounding 3x3.
fn unmatched(a: Gray<'_>, b: Gray<'_>, tolerance: u8) -> usize {
    let mut count = 0usize;
    for y in 0..a.height {
        for x in 0..a.width {
            let Some(p) = a.get(x, y) else { continue };
            let mut matched = false;
            // Saturating bounds keep the window inside the image at the edges; `get` rejects the
            // rest, so no index is ever derived from an unchecked subtraction.
            for ny in y.saturating_sub(1)..=y.saturating_add(1) {
                for nx in x.saturating_sub(1)..=x.saturating_add(1) {
                    if b.get(nx, ny).is_some_and(|q| p.abs_diff(q) <= tolerance) {
                        matched = true;
                        break;
                    }
                }
                if matched {
                    break;
                }
            }
            if !matched {
                count = count.saturating_add(1);
            }
        }
    }
    count
}

/// Decode a binary netpbm grayscale image (`P5`), which is what `pdftoppm -gray` writes.
///
/// Returns the dimensions and the pixel bytes. Only the maxval 255 form is accepted: a 16-bit
/// image would otherwise be read as twice as many 8-bit pixels and compared against nonsense.
pub fn decode_pgm(bytes: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let mut fields = Vec::new();
    let mut rest = bytes.strip_prefix(b"P5")?;
    // Three whitespace-separated fields follow the magic: width, height, maxval. Comments run
    // from `#` to end of line and may appear between any of them.
    while fields.len() < 3 {
        let skip = rest.iter().position(|b| !b.is_ascii_whitespace())?;
        rest = rest.get(skip..)?;
        if rest.first() == Some(&b'#') {
            let end = rest.iter().position(|&b| b == b'\n')?;
            rest = rest.get(end.checked_add(1)?..)?;
            continue;
        }
        let end = rest.iter().position(u8::is_ascii_whitespace)?;
        let field = rest.get(..end)?;
        fields.push(std::str::from_utf8(field).ok()?.parse::<usize>().ok()?);
        rest = rest.get(end..)?;
    }
    let (&width, &height, &maxval) = (fields.first()?, fields.get(1)?, fields.get(2)?);
    if maxval != 255 {
        return None;
    }
    // Exactly one whitespace byte separates the header from the data.
    let pixels = rest.get(1..)?;
    let wanted = width.checked_mul(height)?;
    (pixels.len() >= wanted).then(|| (width, height, pixels.get(..wanted).unwrap_or_default().to_vec()))
}

/// Decode the netpbm `P7` RGB_ALPHA image `pdfcraft-cli render --out x.pam` writes, flattening it
/// to grayscale the way a viewer would show it: composited over white, then luminance.
///
/// The alpha is premultiplied, which is why the composite is `channel + white * (1 - alpha)`
/// rather than the usual `mix`. A page is normally opaque, but a transparent one must not come out
/// darker here than it looks on screen, or the comparison blames the renderer for the harness.
pub fn decode_pam_gray(bytes: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let header_end = find(bytes, b"ENDHDR\n")?.checked_add(b"ENDHDR\n".len())?;
    let header = std::str::from_utf8(bytes.get(..header_end)?).ok()?;
    if !header.starts_with("P7") {
        return None;
    }
    let field = |name: &str| -> Option<usize> { header.lines().find_map(|l| l.strip_prefix(name)?.trim().parse::<usize>().ok()) };
    let (width, height) = (field("WIDTH")?, field("HEIGHT")?);
    if field("DEPTH")? != 4 || field("MAXVAL")? != 255 || !header.contains("TUPLTYPE RGB_ALPHA") {
        return None;
    }
    let data = bytes.get(header_end..)?;
    let wanted = width.checked_mul(height)?.checked_mul(4)?;
    if data.len() < wanted {
        return None;
    }
    let mut out = Vec::with_capacity(width.checked_mul(height)?);
    for px in data.get(..wanted)?.as_chunks::<4>().0 {
        let (&r, &g, &b, &a) = (px.first()?, px.get(1)?, px.get(2)?, px.get(3)?);
        // Composite premultiplied RGB over white, in integer arithmetic that cannot overflow:
        // each term is at most 255, and the sum is at most 255.
        let over_white = |c: u8| u32::from(c).saturating_add(255u32.saturating_sub(u32::from(a))).min(255);
        let (r, g, b) = (over_white(r), over_white(g), over_white(b));
        // Rec. 601 luma, the usual grayscale for this, rounded. Each term is at most 255*587, so
        // the sum cannot overflow a u32.
        let luma = (r * 299 + g * 587 + b * 114 + 500) / 1000;
        out.push(u8::try_from(luma.min(255)).unwrap_or(255));
    }
    Some((width, height, out))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    // `windows(0)` panics.
    if needle.is_empty() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray(w: usize, h: usize, fill: u8) -> Vec<u8> {
        vec![fill; w * h]
    }

    fn pam(w: usize, h: usize, px: &[u8]) -> Vec<u8> {
        let mut out = format!("P7\nWIDTH {w}\nHEIGHT {h}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n").into_bytes();
        out.extend_from_slice(px);
        out
    }

    #[test]
    fn pam_flattens_to_the_grey_a_viewer_would_show() {
        // Opaque white, opaque black, opaque mid-grey.
        let px = [255, 255, 255, 255, 0, 0, 0, 255, 128, 128, 128, 255];
        let (w, h, g) = decode_pam_gray(&pam(3, 1, &px)).expect("a P7 image");
        assert_eq!((w, h), (3, 1));
        assert_eq!(g, vec![255, 0, 128]);
    }

    /// A fully transparent pixel is white on screen, not black. Getting this wrong would make the
    /// harness report a blank margin as a difference on every page that has one.
    #[test]
    fn transparent_pixels_composite_over_white() {
        let px = [0, 0, 0, 0, 0, 0, 0, 128];
        let (_, _, g) = decode_pam_gray(&pam(2, 1, &px)).expect("a P7 image");
        assert_eq!(g.first().copied(), Some(255), "fully transparent is white");
        assert_eq!(g.get(1).copied(), Some(127), "half-transparent black over white is mid grey");
    }

    #[test]
    fn hostile_pam_headers_are_rejected() {
        let px = [0u8; 8];
        assert!(decode_pam_gray(&pam(2, 1, &px)).is_some());
        let cases: Vec<Vec<u8>> = vec![
            b"".to_vec(),
            b"P7\n".to_vec(),
            b"P7\nWIDTH 2\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\n".to_vec(), // no ENDHDR
            pam(2, 1, &[0u8; 4]),                                                         // truncated data
            b"P6\nWIDTH 2\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n".to_vec(),
            b"P7\nWIDTH 2\nHEIGHT 1\nDEPTH 3\nMAXVAL 255\nTUPLTYPE RGB\nENDHDR\n".to_vec(), // no alpha
            b"P7\nWIDTH 2\nHEIGHT 1\nDEPTH 4\nMAXVAL 65535\nTUPLTYPE RGB_ALPHA\nENDHDR\n".to_vec(),
            b"P7\nWIDTH 99999999999999999999\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n".to_vec(),
        ];
        for case in &cases {
            assert!(decode_pam_gray(case).is_none(), "{:?}", String::from_utf8_lossy(case.get(..40).unwrap_or(case)));
        }
        // A declared area far larger than the data must fail on length, not allocate.
        let huge = b"P7\nWIDTH 4000000000\nHEIGHT 4000000000\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n".to_vec();
        assert!(decode_pam_gray(&huge).is_none());
    }

    #[test]
    fn identical_images_do_not_differ() {
        let buf = gray(16, 16, 128);
        let a = Gray::new(16, 16, &buf).expect("sized");
        let d = compare(a, a, 0).expect("same size");
        assert_eq!(d.differing, 0);
        assert_eq!(d.max_delta, 0);
        assert_eq!(d.fraction(), 0.0);
        assert_eq!(d.total, 256);
    }

    /// The whole point: a one-pixel shift of a stem is forgiven, because every moved pixel finds
    /// its match next door. This is what makes the number mean something across renderers.
    #[test]
    fn a_one_pixel_shift_is_forgiven() {
        let (w, h) = (9usize, 9usize);
        let mut left = gray(w, h, 255);
        let mut right = gray(w, h, 255);
        for y in 0..h {
            left[y * w + 4] = 0; // a vertical stem
            right[y * w + 5] = 0; // the same stem, one pixel over
        }
        let a = Gray::new(w, h, &left).expect("sized");
        let b = Gray::new(w, h, &right).expect("sized");
        let d = compare(a, b, 8).expect("same size");
        assert_eq!(d.differing, 0, "a one-pixel shift must not count as a difference");
        // ...but it is still visible as a large single-pixel delta, which is the honest signal.
        assert_eq!(d.max_delta, 255);
    }

    /// And the point of the point: content that is simply absent is *not* forgiven, however
    /// tolerant the neighbourhood test is.
    #[test]
    fn missing_content_is_not_forgiven() {
        let (w, h) = (16usize, 16usize);
        let blank = gray(w, h, 255);
        let mut drawn = blank.clone();
        // A filled 6x6 block: its interior is further than one pixel from any white pixel.
        for y in 5..11 {
            for x in 5..11 {
                drawn[y * w + x] = 0;
            }
        }
        let a = Gray::new(w, h, &blank).expect("sized");
        let b = Gray::new(w, h, &drawn).expect("sized");
        let d = compare(a, b, 8).expect("same size");
        // Scanning the blank against the drawn image forgives the block's border, whose
        // neighbourhood still reaches white, and counts only its 4x4 interior. Scanning the other
        // way counts all 36 black pixels, since a blank image offers no dark match anywhere. The
        // worse of the two is reported, so the whole block is -- which is the honest answer.
        assert_eq!(d.differing, 36, "every pixel of a missing block must be counted");
        assert!(d.fraction() > 0.0);
    }

    /// A three-pixel shift is beyond the 3x3 window and is reported.
    #[test]
    fn a_shift_wider_than_the_window_is_reported() {
        let (w, h) = (16usize, 16usize);
        let mut left = gray(w, h, 255);
        let mut right = gray(w, h, 255);
        for y in 0..h {
            for x in 4..8 {
                left[y * w + x] = 0;
            }
            for x in 11..15 {
                right[y * w + x] = 0;
            }
        }
        let a = Gray::new(w, h, &left).expect("sized");
        let b = Gray::new(w, h, &right).expect("sized");
        assert!(compare(a, b, 8).expect("same size").differing > 0);
    }

    #[test]
    fn tolerance_absorbs_small_differences_only() {
        let flat = gray(8, 8, 100);
        let near = gray(8, 8, 105);
        let far = gray(8, 8, 200);
        let a = Gray::new(8, 8, &flat).expect("sized");
        assert_eq!(compare(a, Gray::new(8, 8, &near).expect("sized"), 8).expect("ok").differing, 0);
        assert_eq!(compare(a, Gray::new(8, 8, &far).expect("sized"), 8).expect("ok").differing, 64);
        // Tolerance is inclusive at the boundary.
        assert_eq!(compare(a, Gray::new(8, 8, &near).expect("sized"), 5).expect("ok").differing, 0);
        assert_eq!(compare(a, Gray::new(8, 8, &near).expect("sized"), 4).expect("ok").differing, 64);
    }

    /// Comparison is symmetric: neither argument order flatters one renderer.
    #[test]
    fn comparison_is_symmetric() {
        let (w, h) = (12usize, 12usize);
        let mut thin = gray(w, h, 255);
        let mut thick = gray(w, h, 255);
        for y in 0..h {
            thin[y * w + 6] = 0;
            for x in 4..9 {
                thick[y * w + x] = 0;
            }
        }
        let a = Gray::new(w, h, &thin).expect("sized");
        let b = Gray::new(w, h, &thick).expect("sized");
        let forward = compare(a, b, 8).expect("ok");
        let backward = compare(b, a, 8).expect("ok");
        assert_eq!(forward, backward, "argument order must not change the verdict");
        assert!(forward.differing > 0, "a 1px stem against a 5px bar is a real difference");
    }

    #[test]
    fn different_sizes_are_an_error_not_a_score() {
        let small = gray(4, 4, 0);
        let large = gray(8, 8, 0);
        let err = compare(Gray::new(4, 4, &small).expect("sized"), Gray::new(8, 8, &large).expect("sized"), 0).expect_err("mismatch");
        assert_eq!(err, SizeMismatch { a: (4, 4), b: (8, 8) });
        assert!(err.to_string().contains("4x4"), "{err}");
    }

    #[test]
    fn a_buffer_that_is_not_the_stated_size_is_rejected() {
        let buf = gray(4, 4, 0);
        assert!(Gray::new(4, 4, &buf).is_some());
        assert!(Gray::new(5, 4, &buf).is_none());
        assert!(Gray::new(4, 5, &buf).is_none());
        assert!(Gray::new(usize::MAX, usize::MAX, &buf).is_none(), "the area must not overflow into a match");
        // A zero-sized image is representable and compares as no difference.
        let empty: [u8; 0] = [];
        let z = Gray::new(0, 0, &empty).expect("zero is a valid size");
        assert_eq!(compare(z, z, 0).expect("ok").fraction(), 0.0);
    }

    #[test]
    fn pgm_decodes_the_shape_pdftoppm_writes() {
        let mut bytes = b"P5\n4 2\n255\n".to_vec();
        bytes.extend_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7]);
        let (w, h, px) = decode_pgm(&bytes).expect("a P5 image");
        assert_eq!((w, h), (4, 2));
        assert_eq!(px, vec![0, 1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn pgm_handles_comments_and_odd_whitespace() {
        let mut bytes = b"P5\n# written by pdftoppm\n  4\t2\n255 ".to_vec();
        bytes.extend_from_slice(&[9; 8]);
        let (w, h, px) = decode_pgm(&bytes).expect("a commented P5 image");
        assert_eq!((w, h, px.len()), (4, 2, 8));
    }

    /// An empty needle is not found, rather than panicking in `windows(0)`.
    #[test]
    fn find_with_an_empty_needle_is_none() {
        assert_eq!(find(b"abc", b""), None);
        assert_eq!(find(b"", b""), None);
        assert_eq!(find(b"abcabc", b"ca"), Some(2));
    }

    /// Malformed or hostile headers return `None` rather than panicking or inventing a size.
    #[test]
    fn hostile_pgm_headers_are_rejected() {
        let cases: &[&[u8]] = &[
            b"",
            b"P5",
            b"P6\n4 2\n255\n",     // colour, not grayscale
            b"P5\n4 2\n65535\n",   // 16-bit: would be read as twice the pixels
            b"P5\n4 2\n255\n\x00", // truncated data
            b"P5\n-4 2\n255\n",    // negative
            b"P5\n99999999999999999999 2\n255\n",
            b"P5\n4\n", // header cut short
            b"P5\n#unterminated comment",
        ];
        for case in cases {
            assert!(decode_pgm(case).is_none(), "{:?}", String::from_utf8_lossy(case));
        }
        // A huge declared area must not allocate: it fails on the data length check.
        let huge = b"P5\n4000000000 4000000000\n255\n".to_vec();
        assert!(decode_pgm(&huge).is_none());
    }
}
