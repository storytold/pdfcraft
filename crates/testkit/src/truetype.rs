//! Synthetic TrueType programs, built in memory: no font file is involved.
//!
//! [`subset`] writes a program with a chosen number of glyphs of which only some draw (a small
//! square), the rest empty, as a subsetter that keeps the original glyph ids writes it, or a
//! dense one, as a subsetter that renumbers them writes it; with or without a `cmap` and a
//! `post` table.

/// The `cmap` table of a [`subset`].
#[derive(Clone, Copy, Debug)]
pub enum Cmap<'a> {
    /// None at all.
    None,
    /// A symbolic (3, 0) subtable mapping `0xF000 + code` to a glyph id, as Windows printer
    /// drivers write for their TrueType subsets.
    Symbolic(&'a [(u8, u16)]),
    /// A Unicode (3, 1) subtable mapping characters to glyph ids.
    Unicode(&'a [(char, u16)]),
    /// A Macintosh (1, 0) subtable mapping codes to glyph ids.
    Mac(&'a [(u8, u16)]),
    /// A Macintosh (1, 0) subtable, then a Unicode (3, 1) one (the last in the table).
    MacThenUnicode(&'a [(u8, u16)], &'a [(char, u16)]),
}

fn be16(out: &mut Vec<u8>, n: u16) {
    out.extend_from_slice(&n.to_be_bytes());
}

fn be32(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_be_bytes());
}

/// A format 4 subtable mapping each `(code, glyph)` with one segment per pair.
fn format4(pairs: &[(u16, u16)]) -> Vec<u8> {
    let mut pairs = pairs.to_vec();
    pairs.sort_unstable();
    pairs.dedup_by_key(|p| p.0);
    let segments = pairs.len().saturating_add(1);
    let seg_x2 = u16::try_from(segments.saturating_mul(2)).unwrap_or(u16::MAX);
    let mut t = Vec::new();
    be16(&mut t, 4);
    let length = 16usize.saturating_add(segments.saturating_mul(8));
    be16(&mut t, u16::try_from(length).unwrap_or(u16::MAX));
    be16(&mut t, 0);
    be16(&mut t, seg_x2);
    be16(&mut t, 2);
    be16(&mut t, 0);
    be16(&mut t, 0);
    for &(c, _) in &pairs {
        be16(&mut t, c); // endCode
    }
    be16(&mut t, 0xFFFF);
    be16(&mut t, 0); // reservedPad
    for &(c, _) in &pairs {
        be16(&mut t, c); // startCode
    }
    be16(&mut t, 0xFFFF);
    for &(c, g) in &pairs {
        be16(&mut t, g.wrapping_sub(c)); // idDelta
    }
    be16(&mut t, 1);
    for _ in 0..segments {
        be16(&mut t, 0); // idRangeOffset
    }
    t
}

/// A TrueType program with `num_glyphs` glyphs (at least 1) where the glyph ids in `drawn` are a
/// small square and every other glyph is empty, with the given `cmap` and, when `post` is
/// `Some(version)`, a `post` table of that version: `0x0002_0000` names every glyph `glyph`;
/// the others carry no names.
pub fn subset(num_glyphs: u16, drawn: &[u16], cmap: Cmap<'_>, post: Option<u32>) -> Vec<u8> {
    subset_with_widths(num_glyphs, drawn, cmap, post, None)
}

/// [`subset`] whose glyphs have their own advance widths, `widths(gid)` in units of 1000 per em
/// (`None`: every glyph 600 wide, one metric for all, as [`subset`]).
pub fn subset_with_widths(num_glyphs: u16, drawn: &[u16], cmap: Cmap<'_>, post: Option<u32>, widths: Option<&dyn Fn(u16) -> u16>) -> Vec<u8> {
    let square: crate::Outline<'_> = &[&[(0, 0), (500, 0), (500, 500), (0, 500)]];
    let outlines: Vec<(u16, crate::Outline<'_>)> = drawn.iter().map(|&gid| (gid, square)).collect();
    program(num_glyphs, &outlines, cmap, post, widths)
}

/// A TrueType program with `num_glyphs` glyphs (at least 1) where each glyph of `outlines` draws
/// its polygons (one contour each, of on-curve points in units of 1000 per em) and every other
/// glyph is empty; `cmap` and `post` as for [`subset`], every glyph 600 wide.
pub fn with_outlines(num_glyphs: u16, outlines: &[(u16, crate::Outline<'_>)], cmap: Cmap<'_>, post: Option<u32>) -> Vec<u8> {
    program(num_glyphs, outlines, cmap, post, None)
}

/// A simple glyph drawing `contours`, each a polygon of on-curve points, two-byte coordinates.
fn simple_glyph(contours: crate::Outline<'_>) -> Vec<u8> {
    let points: Vec<(i16, i16)> = contours.iter().flat_map(|c| c.iter().copied()).collect();
    let mut g = Vec::new();
    be16(&mut g, u16::try_from(contours.len()).unwrap_or(0));
    let (x0, y0, x1, y1) =
        points.iter().fold((i16::MAX, i16::MAX, i16::MIN, i16::MIN), |(a, b, c, d), &(x, y)| (a.min(x), b.min(y), c.max(x), d.max(y)));
    for n in [x0, y0, x1, y1] {
        g.extend_from_slice(&n.to_be_bytes());
    }
    let mut end = 0usize;
    for c in contours {
        end = end.saturating_add(c.len());
        be16(&mut g, u16::try_from(end.saturating_sub(1)).unwrap_or(0));
    }
    be16(&mut g, 0);
    g.extend(std::iter::repeat_n(1u8, points.len()));
    let mut at = (0i16, 0i16);
    let mut ys = Vec::new();
    for &(x, y) in &points {
        g.extend_from_slice(&x.wrapping_sub(at.0).to_be_bytes());
        ys.extend_from_slice(&y.wrapping_sub(at.1).to_be_bytes());
        at = (x, y);
    }
    g.extend(ys);
    if g.len() % 2 == 1 {
        g.push(0);
    }
    g
}

fn program(
    num_glyphs: u16,
    outlines: &[(u16, crate::Outline<'_>)],
    cmap: Cmap<'_>,
    post: Option<u32>,
    widths: Option<&dyn Fn(u16) -> u16>,
) -> Vec<u8> {
    let num_glyphs = num_glyphs.max(1);
    let mut glyf = Vec::new();
    let mut loca = Vec::new();
    // One lookup per glyph, so a program of tens of thousands of glyphs builds quickly.
    let mut draws: Vec<Option<crate::Outline<'_>>> = vec![None; usize::from(num_glyphs)];
    for &(gid, contours) in outlines {
        if let Some(d) = draws.get_mut(usize::from(gid)) {
            *d = Some(contours);
        }
    }
    let mut cache: Option<(crate::Outline<'_>, Vec<u8>)> = None;
    for gid in 0..num_glyphs {
        be32(&mut loca, u32::try_from(glyf.len()).unwrap_or(u32::MAX));
        if let Some(Some(contours)) = draws.get(usize::from(gid)).copied() {
            match &cache {
                Some((c, bytes)) if std::ptr::eq(*c, contours) => glyf.extend_from_slice(bytes),
                _ => {
                    let bytes = simple_glyph(contours);
                    glyf.extend_from_slice(&bytes);
                    cache = Some((contours, bytes));
                }
            }
        }
    }
    be32(&mut loca, u32::try_from(glyf.len()).unwrap_or(u32::MAX));

    let mut head = vec![0; 54];
    head[0..4].copy_from_slice(&[0, 1, 0, 0]);
    head[12..16].copy_from_slice(&0x5f0f_3cf5u32.to_be_bytes());
    head[18..20].copy_from_slice(&1000u16.to_be_bytes());
    head[40..42].copy_from_slice(&500i16.to_be_bytes());
    head[42..44].copy_from_slice(&500i16.to_be_bytes());
    head[50..52].copy_from_slice(&1i16.to_be_bytes()); // long loca offsets
    let mut hhea = vec![0; 36];
    hhea[0..4].copy_from_slice(&[0, 1, 0, 0]);
    hhea[4..6].copy_from_slice(&800i16.to_be_bytes());
    let mut hmtx = Vec::new();
    match widths {
        Some(width) => {
            hhea[34..36].copy_from_slice(&num_glyphs.to_be_bytes());
            for gid in 0..num_glyphs {
                be16(&mut hmtx, width(gid));
                be16(&mut hmtx, 0);
            }
        }
        None => {
            hhea[34..36].copy_from_slice(&1u16.to_be_bytes());
            be16(&mut hmtx, 600);
            be16(&mut hmtx, 0);
            for _ in 1..num_glyphs {
                be16(&mut hmtx, 0);
            }
        }
    }
    let mut maxp = vec![0, 1, 0, 0];
    be16(&mut maxp, num_glyphs);
    maxp.resize(32, 0);

    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    let mac = |pairs: &[(u8, u16)]| ((1u16, 0u16), format4(&pairs.iter().map(|&(c, g)| (u16::from(c), g)).collect::<Vec<_>>()));
    let unicode = |pairs: &[(char, u16)]| {
        ((3u16, 1u16), format4(&pairs.iter().map(|&(c, g)| (u16::try_from(u32::from(c)).unwrap_or(0xFFFD), g)).collect::<Vec<_>>()))
    };
    let subtables = match cmap {
        Cmap::None => vec![],
        Cmap::Mac(pairs) => vec![mac(pairs)],
        Cmap::Symbolic(pairs) => vec![((3, 0), format4(&pairs.iter().map(|&(c, g)| (0xF000 | u16::from(c), g)).collect::<Vec<_>>()))],
        Cmap::Unicode(pairs) => vec![unicode(pairs)],
        Cmap::MacThenUnicode(m, u) => vec![mac(m), unicode(u)],
    };
    if !subtables.is_empty() {
        let mut t = Vec::new();
        be16(&mut t, 0);
        be16(&mut t, u16::try_from(subtables.len()).unwrap_or(0));
        let mut offset = 4usize.saturating_add(8usize.saturating_mul(subtables.len()));
        for ((platform, encoding), sub) in &subtables {
            be16(&mut t, *platform);
            be16(&mut t, *encoding);
            be32(&mut t, u32::try_from(offset).unwrap_or(u32::MAX));
            offset = offset.saturating_add(sub.len());
        }
        for (_, sub) in subtables {
            t.extend(sub);
        }
        tables.push((*b"cmap", t));
    }
    tables.push((*b"glyf", glyf));
    tables.push((*b"head", head));
    tables.push((*b"hhea", hhea));
    tables.push((*b"hmtx", hmtx));
    tables.push((*b"loca", loca));
    tables.push((*b"maxp", maxp));
    if let Some(version) = post {
        let mut t = Vec::new();
        be32(&mut t, version);
        t.resize(32, 0);
        if version == 0x0002_0000 {
            // Every glyph named `glyph`, a name of the font's own (index 258).
            be16(&mut t, num_glyphs);
            for _ in 0..num_glyphs {
                be16(&mut t, 258);
            }
            t.push(5);
            t.extend_from_slice(b"glyph");
        }
        tables.push((*b"post", t));
    }

    let count = u16::try_from(tables.len()).unwrap_or(u16::MAX);
    let mut out = vec![0, 1, 0, 0];
    be16(&mut out, count);
    for n in [128u16, 3, count.saturating_mul(16).saturating_sub(128)] {
        be16(&mut out, n);
    }
    let mut payload = Vec::new();
    let mut offset = 12usize.saturating_add(16usize.saturating_mul(tables.len()));
    for (tag, bytes) in tables {
        out.extend(tag);
        let mut padded = bytes.clone();
        padded.resize(bytes.len().div_ceil(4).saturating_mul(4), 0);
        let sum = padded.as_chunks::<4>().0.iter().fold(0u32, |s, b| s.wrapping_add(u32::from_be_bytes(*b)));
        be32(&mut out, sum);
        be32(&mut out, u32::try_from(offset).unwrap_or(u32::MAX));
        be32(&mut out, u32::try_from(bytes.len()).unwrap_or(u32::MAX));
        offset = offset.saturating_add(padded.len());
        payload.extend(padded);
    }
    out.extend(payload);
    out
}

/// The glyph id the standard Macintosh glyph order gives a printable ASCII character (`' '` is
/// 3, `'A'` 36, `'a'` 68): the character's code minus 29.
pub fn mac_gid(c: char) -> u16 {
    u16::try_from(u32::from(c)).unwrap_or(0).saturating_sub(29)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_program_has_its_tables_and_glyphs() {
        let font = subset(300, &[36, 68], Cmap::Symbolic(&[(1, 36)]), Some(0x0003_0000));
        assert_eq!(&font[0..4], &[0, 1, 0, 0]);
        assert!(font.windows(4).any(|w| w == b"post") && font.windows(4).any(|w| w == b"cmap"));
        assert_eq!(mac_gid('A'), 36);
        assert_eq!(mac_gid('a'), 68);
        assert_eq!(mac_gid(' '), 3);
    }
}
