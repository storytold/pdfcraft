//! Font programs for the font conditions' tests, built in memory: contributor-original data, no
//! binary fixture. Each is about the smallest program of its kind the parsers read, with the
//! glyphs, widths and cmaps a test asks for.

fn be16(out: &mut Vec<u8>, n: u16) {
    out.extend_from_slice(&n.to_be_bytes());
}

fn be32(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_be_bytes());
}

/// A cmap subtable: its (platform, encoding) and its code → glyph id mappings.
pub(super) type Cmap = ((u16, u16), Vec<(u32, u16)>);

/// A TrueType program.
#[derive(Clone, Debug)]
pub(super) struct TrueType {
    /// Each glyph's advance in font units, glyph 0 (.notdef) first, and whether it has an outline.
    pub glyphs: Vec<(u16, bool)>,
    /// Its cmap subtables, in order.
    pub cmaps: Vec<Cmap>,
}

impl TrueType {
    /// `.notdef`, then one outlined glyph `advance` wide for each of `chars`, mapped by a (3,1) cmap.
    pub fn latin(chars: impl IntoIterator<Item = char>, advance: u16) -> Self {
        let chars: Vec<char> = chars.into_iter().collect();
        let mut glyphs = vec![(advance, true)];
        glyphs.extend(chars.iter().map(|_| (advance, true)));
        let map = chars.iter().zip(1u16..).map(|(c, g)| (u32::from(*c), g)).collect();
        TrueType { glyphs, cmaps: vec![((3, 1), map)] }
    }

    /// The program, at 1000 units per em.
    pub fn bytes(&self) -> Vec<u8> {
        let n = u16::try_from(self.glyphs.len()).unwrap();
        // One triangle each for the outlined glyphs, nothing for the others; long `loca` offsets.
        let triangle: [u8; 29] = [0, 1, 0, 0, 0, 0, 0, 100, 0, 100, 0, 2, 0, 0, 1, 1, 1, 0, 0, 0, 100, 255, 156, 0, 0, 0, 0, 0, 100];
        let (mut glyf, mut loca) = (Vec::new(), Vec::new());
        for (_, outlined) in &self.glyphs {
            be32(&mut loca, u32::try_from(glyf.len()).unwrap());
            if *outlined {
                glyf.extend_from_slice(&triangle);
                glyf.extend_from_slice(&[0, 0, 0]);
            }
        }
        be32(&mut loca, u32::try_from(glyf.len()).unwrap());
        let mut head = vec![0; 54];
        head[0..4].copy_from_slice(&[0, 1, 0, 0]);
        head[12..16].copy_from_slice(&0x5f0f_3cf5u32.to_be_bytes());
        head[18..20].copy_from_slice(&1000u16.to_be_bytes());
        head[50..52].copy_from_slice(&1i16.to_be_bytes());
        let mut hhea = vec![0; 36];
        hhea[0..4].copy_from_slice(&[0, 1, 0, 0]);
        hhea[34..36].copy_from_slice(&n.to_be_bytes());
        let mut hmtx = Vec::new();
        for (advance, _) in &self.glyphs {
            be16(&mut hmtx, *advance);
            be16(&mut hmtx, 0);
        }
        let mut maxp = vec![0, 0, 0x50, 0];
        be16(&mut maxp, n);
        // Every subtable is format 12, one group a mapping.
        let mut subtables = Vec::new();
        for (_, map) in &self.cmaps {
            let mut t = Vec::new();
            be16(&mut t, 12);
            be16(&mut t, 0);
            be32(&mut t, u32::try_from(16 + 12 * map.len()).unwrap());
            be32(&mut t, 0);
            be32(&mut t, u32::try_from(map.len()).unwrap());
            let mut sorted = map.clone();
            sorted.sort_unstable();
            for (code, gid) in sorted {
                be32(&mut t, code);
                be32(&mut t, code);
                be32(&mut t, u32::from(gid));
            }
            subtables.push(t);
        }
        let mut cmap = Vec::new();
        be16(&mut cmap, 0);
        be16(&mut cmap, u16::try_from(self.cmaps.len()).unwrap());
        let mut offset = 4 + 8 * self.cmaps.len();
        for (((platform, encoding), _), t) in self.cmaps.iter().zip(&subtables) {
            be16(&mut cmap, *platform);
            be16(&mut cmap, *encoding);
            be32(&mut cmap, u32::try_from(offset).unwrap());
            offset += t.len();
        }
        for t in subtables {
            cmap.extend(t);
        }
        let mut post = vec![0, 3, 0, 0];
        post.resize(32, 0);
        let mut tables: Vec<([u8; 4], Vec<u8>)> =
            vec![(*b"glyf", glyf), (*b"head", head), (*b"hhea", hhea), (*b"hmtx", hmtx), (*b"loca", loca), (*b"maxp", maxp), (*b"post", post)];
        if !self.cmaps.is_empty() {
            tables.push((*b"cmap", cmap));
        }
        tables.sort_by_key(|(tag, _)| *tag);
        sfnt([0, 1, 0, 0], tables)
    }
}

fn sfnt(version: [u8; 4], tables: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {
    let mut out = version.to_vec();
    be16(&mut out, u16::try_from(tables.len()).unwrap());
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    let mut payload = Vec::new();
    let mut offset = 12 + 16 * tables.len();
    for (tag, bytes) in tables {
        out.extend(tag);
        let mut padded = bytes.clone();
        while !padded.len().is_multiple_of(4) {
            padded.push(0);
        }
        be32(&mut out, 0);
        be32(&mut out, u32::try_from(offset).unwrap());
        be32(&mut out, u32::try_from(bytes.len()).unwrap());
        offset += padded.len();
        payload.extend(padded);
    }
    out.extend(payload);
    out
}

/// `eexec`-encrypts `plain` with seed `r` (Adobe Type 1 Font Format §7).
fn encrypt(plain: &[u8], mut r: u16) -> Vec<u8> {
    plain
        .iter()
        .map(|&p| {
            let c = p ^ (r >> 8) as u8;
            r = u16::from(c).wrapping_add(r).wrapping_mul(52845).wrapping_add(22719);
            c
        })
        .collect()
}

/// A Type 1 charstring number (Type 1 Font Format §6.2).
fn t1_number(n: i32) -> Vec<u8> {
    match n {
        -107..=107 => vec![u8::try_from(n + 139).unwrap()],
        108..=1131 => vec![u8::try_from((n - 108) / 256 + 247).unwrap(), u8::try_from((n - 108) % 256).unwrap()],
        _ => {
            let mut v = vec![255];
            v.extend_from_slice(&n.to_be_bytes());
            v
        }
    }
}

/// A Type 1 program: `.notdef` 250 units wide and the named glyphs, empty, with `encoding` as
/// its built-in encoding (`StandardEncoding`, or a custom one as PostScript).
pub(super) fn type1(glyphs: &[(&str, i32)], encoding: &str) -> Vec<u8> {
    let mut all = vec![(".notdef", 250)];
    all.extend_from_slice(glyphs);
    let mut private = b"dup /Private 8 dict dup begin /lenIV 4 def /Subrs 0 array\n".to_vec();
    private.extend_from_slice(format!("2 index /CharStrings {} dict dup begin\n", all.len()).as_bytes());
    for (name, width) in all {
        let mut cs = vec![0, 0, 0, 0];
        cs.extend(t1_number(0));
        cs.extend(t1_number(width));
        cs.extend([13, 14]);
        let cs = encrypt(&cs, 4330);
        private.extend_from_slice(format!("/{name} {} RD ", cs.len()).as_bytes());
        private.extend(cs);
        private.extend_from_slice(b" ND\n");
    }
    private.extend_from_slice(b"end end\nmark currentfile closefile\n");
    let mut plain = vec![0, 0, 0, 0];
    plain.extend(private);
    let mut out =
        format!("%!PS-AdobeFont-1.0: Test 001\n/FontName /Test def\n/FontMatrix [0.001 0 0 0.001 0 0] readonly def\n/Encoding {encoding} def\ncurrentfile eexec\n")
            .into_bytes();
    out.extend(encrypt(&plain, 55665));
    out
}

/// A CFF DICT integer in its five-byte form, so a DICT's size doesn't depend on its values.
fn dict_int(n: i32) -> Vec<u8> {
    let mut v = vec![29];
    v.extend_from_slice(&n.to_be_bytes());
    v
}

/// A Type 2 charstring number.
fn t2_number(n: i32) -> Vec<u8> {
    match n {
        -107..=107 => vec![u8::try_from(n + 139).unwrap()],
        108..=1131 => vec![u8::try_from((n - 108) / 256 + 247).unwrap(), u8::try_from((n - 108) % 256).unwrap()],
        _ => {
            let mut v = vec![28];
            v.extend_from_slice(&i16::try_from(n).unwrap().to_be_bytes());
            v
        }
    }
}

/// A CFF INDEX.
fn index(items: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    be16(&mut out, u16::try_from(items.len()).unwrap());
    if items.is_empty() {
        return out;
    }
    let total: usize = items.iter().map(Vec::len).sum::<usize>() + 1;
    let size = if total < 0x100 {
        1
    } else if total < 0x1_0000 {
        2
    } else {
        4
    };
    out.push(u8::try_from(size).unwrap());
    let mut at = 1usize;
    let mut offsets = vec![at];
    for i in items {
        at += i.len();
        offsets.push(at);
    }
    for o in offsets {
        out.extend_from_slice(&u32::try_from(o).unwrap().to_be_bytes()[4 - size..]);
    }
    for i in items {
        out.extend(i);
    }
    out
}

/// A CID-keyed CFF program (Adobe-Identity-0): CID 0 (.notdef) and the given CIDs, each the given
/// width and empty, in that glyph order.
pub(super) fn cid_cff(cids: &[(u16, i32)]) -> Vec<u8> {
    let mut glyphs = vec![(0u16, 500)];
    glyphs.extend_from_slice(cids);
    let n = glyphs.len();
    let charstrings = index(
        &glyphs
            .iter()
            .map(|(_, w)| {
                let mut cs = t2_number(*w);
                cs.push(14);
                cs
            })
            .collect::<Vec<_>>(),
    );
    let mut charset = vec![0];
    for (cid, _) in glyphs.iter().skip(1) {
        be16(&mut charset, *cid);
    }
    let mut fd_select = vec![0];
    fd_select.extend(std::iter::repeat_n(0, n));
    // nominalWidthX 0: each charstring's own number is its width.
    let private = vec![139, 21];
    let name = index(&[b"Test".to_vec()]);
    let strings = index(&[b"Adobe".to_vec(), b"Identity".to_vec()]);
    let globals = index(&[]);
    // ROS (Adobe, Identity, 0), then charset, CharStrings, FDArray and FDSelect offsets.
    let top_len = 3 + 3 + 1 + 2 + 6 + 6 + 7 + 7;
    let top_index_len = index(&[vec![0; top_len]]).len();
    let charset_at = 4 + name.len() + top_index_len + strings.len() + globals.len();
    let fd_select_at = charset_at + charset.len();
    let charstrings_at = fd_select_at + fd_select.len();
    let fd_array_at = charstrings_at + charstrings.len();
    let fd_len = 5 + 5 + 1;
    let fd_array_len = index(&[vec![0; fd_len]]).len();
    let private_at = fd_array_at + fd_array_len;
    let at = |x: usize| dict_int(i32::try_from(x).unwrap());
    let mut top = vec![28, 1, 135, 28, 1, 136, 139, 12, 30];
    top.extend(at(charset_at));
    top.push(15);
    top.extend(at(charstrings_at));
    top.push(17);
    top.extend(at(fd_array_at));
    top.extend([12, 36]);
    top.extend(at(fd_select_at));
    top.extend([12, 37]);
    assert_eq!(top.len(), top_len);
    let mut fd = at(private.len());
    fd.extend(at(private_at));
    fd.push(18);
    let mut out = vec![1, 0, 4, 4];
    out.extend(name);
    out.extend(index(&[top]));
    out.extend(strings);
    out.extend(globals);
    out.extend(charset);
    out.extend(fd_select);
    out.extend(charstrings);
    out.extend(index(&[fd]));
    out.extend(private);
    out
}

/// `data` as the hex digits an `/ASCIIHexDecode` stream holds.
pub(super) fn hex(data: &[u8]) -> String {
    let mut out: String = data.iter().map(|b| format!("{b:02X}")).collect();
    out.push('>');
    out
}

/// A stream holding `data`, hex-encoded, with `entries` in its dictionary.
pub(super) fn hex_stream(entries: &str, data: &[u8]) -> String {
    let body = hex(data);
    format!("<< /Length {} /Filter /ASCIIHexDecode {entries} >>\nstream\n{body}\nendstream", body.len())
}

#[test]
fn the_programs_parse() {
    use pdfcraft_fonts::program::{Format, Program};
    let tt = Program::from_bytes(Format::TrueType, TrueType::latin(['A', 'B'], 600).bytes()).expect("TrueType");
    let face = tt.face().unwrap();
    assert_eq!(face.num_glyphs(), 3);
    assert_eq!(face.cmap_glyph(3, 1, 0x42), Some(2));
    assert_eq!(face.advance(2), Some(600.0));
    assert!(face.has_outline(2));
    let t1 = Program::from_bytes(Format::Type1, type1(&[("A", 600)], "StandardEncoding")).expect("Type 1");
    assert_eq!(t1.face().unwrap().glyph_of_name("A").and_then(|g| t1.face().unwrap().advance(g)), Some(600.0));
    let cff = Program::from_bytes(Format::Cff, cid_cff(&[(5, 700), (9, 300)])).expect("CID-keyed CFF");
    let face = cff.face().unwrap();
    assert!(face.cid_keyed());
    assert_eq!(face.glyph_of_cid(9), Some(2));
    assert_eq!(face.glyph_of_cid(6), None);
    assert_eq!(face.advance(1), Some(700.0));
}
