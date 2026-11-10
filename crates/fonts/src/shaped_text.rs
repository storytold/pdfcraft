//! Shaped text written into a PDF: each distinct shaped cluster (a letter with its marks, and the
//! text it stands for) becomes one glyph of a Type 3 font drawn from the craft-fonts face's
//! outlines, with a `/ToUnicode` map so copy and search get each character once. No font file is
//! embedded. Add text writes Arabic, Hebrew and Thai with it; text boxes, Fill & Sign text, form
//! fields, headers, footers and watermarks write Thai with it, beside the standard font that
//! draws the rest ([`thai_pieces`]).

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, Object, PdfString, Stream};

use crate::{GlyphError, GlyphOutline, ShapedCluster, ShapedScript, literal, shape_thai, shaped_glyph};

/// Most glyphs in one Type 3 font: its codes are single bytes, 1–240. Text gets one such font per
/// 240 distinct clusters it needs.
pub const GLYPHS_PER_FONT: usize = 240;

/// The Thai block (U+0E00–U+0E7F): letters, vowels, tone marks, digits and signs.
pub fn is_thai(c: char) -> bool {
    matches!(u32::from(c), 0x0E00..=0x0E7F)
}

/// Thai vowel signs and tone marks drawn on the consonant before them, and Sara Am, which the
/// face splits over it: a line never breaks before one, so a mark stays on its letter.
pub fn joins_previous(c: char) -> bool {
    matches!(u32::from(c), 0x0E31 | 0x0E33..=0x0E3A | 0x0E47..=0x0E4E)
}

/// Why Thai text can't be drawn, for a message an agent or user can act on.
pub fn thai_message(e: GlyphError, text: &str) -> String {
    match e {
        GlyphError::NoFont => "Thai text needs a Thai font, and this build has none; rebuild with CRAFT_FONTS_DIR pointing to a craft-fonts \
                               checkout with a Thai face"
            .into(),
        GlyphError::Missing => format!("the Thai font can't show \"{text}\""),
        GlyphError::TooComplex => "the Thai glyphs are too complex; use shorter text".into(),
    }
}

/// A piece of a line of shaped text, in drawing order.
#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    /// Shown in the text's own standard font.
    Latin(String),
    Shaped(Vec<ShapedCluster>),
}

/// `line` split into its Thai runs, shaped with the Thai face, and the text between them, left to
/// the standard font. Thai runs left to right, so the pieces are in typing order.
/// [`GlyphError::NoFont`] when there's Thai and this build has no Thai face.
pub fn thai_pieces(line: &str) -> Result<Vec<Piece>, GlyphError> {
    let mut runs: Vec<(bool, String)> = Vec::new();
    for c in line.chars() {
        match runs.last_mut() {
            Some((thai, s)) if *thai == is_thai(c) => s.push(c),
            _ => runs.push((is_thai(c), c.to_string())),
        }
    }
    runs.into_iter().map(|(thai, s)| if thai { shape_thai(&s).map(Piece::Shaped) } else { Ok(Piece::Latin(s)) }).collect()
}

/// The width of `text` at `size`: its Thai shaped, the rest measured by `latin`. Text the Thai
/// face can't shape is measured by `latin` as a whole (drawing then reports why).
pub fn thai_width(text: &str, size: f64, latin: impl Fn(&str, f64) -> f64) -> f64 {
    if !text.chars().any(is_thai) {
        return latin(text, size);
    }
    match thai_pieces(text) {
        Ok(pieces) => pieces_width(&pieces, size, 0.0, latin),
        Err(_) => latin(text, size),
    }
}

/// The width of `pieces` at `size`, with `spacing` after each shaped cluster (as `Tc` moves the
/// pen; the caller measures it into `latin`'s widths).
pub fn pieces_width(pieces: &[Piece], size: f64, spacing: f64, latin: impl Fn(&str, f64) -> f64) -> f64 {
    pieces
        .iter()
        .map(|p| match p {
            Piece::Latin(s) => latin(s, size),
            // Each cluster is shown on its own, so the spacing follows each once.
            Piece::Shaped(clusters) => clusters.iter().map(|c| c.advance * size + spacing).sum::<f64>(),
        })
        .sum()
}

/// The shaped clusters of `lines`' pieces, for [`ShapedFonts::new`].
pub fn clusters<'a>(lines: impl IntoIterator<Item = &'a [Piece]>) -> impl Iterator<Item = &'a ShapedCluster> {
    lines.into_iter().flatten().flat_map(|p| match p {
        Piece::Shaped(clusters) => clusters.as_slice(),
        Piece::Latin(_) => &[],
    })
}

/// What makes two clusters the same Type 3 glyph: text, glyphs, offsets and advance (bit for bit;
/// equal clusters come from the same font units).
type ClusterKey = (ShapedScript, String, Vec<(u32, u64, u64)>, u64);

fn cluster_key(c: &ShapedCluster) -> ClusterKey {
    (c.script, c.text.clone(), c.glyphs.iter().map(|(id, [x, y])| (*id, x.to_bits(), y.to_bits())).collect(), c.advance.to_bits())
}

/// The Type 3 fonts that draw a set of shaped clusters, one glyph per distinct cluster. The
/// default has none (for text with nothing shaped).
#[derive(Default)]
pub struct ShapedFonts {
    index: HashMap<ClusterKey, usize>,
    /// Each font's resource name.
    names: Vec<String>,
}

impl ShapedFonts {
    /// Build the fonts for `clusters` into `fonts`, under names starting `prefix` that neither
    /// `taken` (the page's fonts) nor `fonts` already has. `outline` gives a glyph's outline in its
    /// script's face (the craft-fonts faces, or a stand-in in tests). `store` keeps each new
    /// object and returns what refers to it: `doc.add` as a reference, or the object itself for
    /// an appearance that isn't in a document yet, whose streams [`store_streams`] then moves in.
    ///
    /// Every outline is read before anything is stored, so a failure (and the cluster it came
    /// from) leaves the document as it was.
    pub fn new<'a>(
        clusters: impl IntoIterator<Item = &'a ShapedCluster>,
        outline: impl Fn(ShapedScript, u32) -> Result<GlyphOutline, GlyphError>,
        taken: &Dict,
        fonts: &mut Dict,
        prefix: &str,
        store: &mut dyn FnMut(Object) -> Object,
    ) -> Result<Self, (GlyphError, &'a ShapedCluster)> {
        let mut keys: Vec<&ShapedCluster> = Vec::new();
        let mut index: HashMap<ClusterKey, usize> = HashMap::new();
        for c in clusters {
            index.entry(cluster_key(c)).or_insert_with(|| {
                keys.push(c);
                keys.len() - 1
            });
        }
        let mut outlines = HashMap::new();
        for c in &keys {
            for (id, _) in &c.glyphs {
                if let std::collections::hash_map::Entry::Vacant(entry) = outlines.entry((c.script, *id)) {
                    entry.insert(outline(c.script, *id).map_err(|e| (e, *c))?);
                }
            }
        }
        // A name no font on the page has, so items never replace each other's fonts (object
        // numbers alone don't do: a full save renumbers objects but keeps resource names).
        let mut taken_names: HashSet<Vec<u8>> = taken.iter().chain(fonts.iter()).map(|(name, _)| name.clone()).collect();
        let mut names = Vec::new();
        for (i, chunk) in keys.chunks(GLYPHS_PER_FONT).enumerate() {
            let font = type3_font(chunk, &outlines, store);
            let font = store(Object::Dict(font));
            let number = font.as_ref().map_or(i + 1, |r| r.num as usize);
            let mut name = format!("{prefix}{number}");
            let mut suffix = 0usize;
            while taken_names.contains(name.as_bytes()) {
                suffix = suffix.saturating_add(1);
                name = format!("{prefix}{number}_{suffix}");
            }
            taken_names.insert(name.clone().into_bytes());
            fonts.set(name.clone().into_bytes(), font);
            names.push(name);
        }
        Ok(ShapedFonts { index, names })
    }

    /// The font resource name and code that show `c`, if it was one of the clusters.
    pub fn code(&self, c: &ShapedCluster) -> Option<(&str, u8)> {
        let &k = self.index.get(&cluster_key(c))?;
        let name = self.names.get(k / GLYPHS_PER_FONT)?;
        // Codes start at 1; k % 240 + 1 is at most 240.
        Some((name.as_str(), u8::try_from(k % GLYPHS_PER_FONT + 1).ok()?))
    }

    /// Show `clusters` inside `BT … ET` from `pen` on baseline `y` at `size`, each placed with its
    /// own `Tm`, `spacing` after each and `word_spacing` more after a space. `current` is the
    /// shaped font the text state has selected (`None` after another font); returns the pen
    /// after the clusters.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &self,
        out: &mut Vec<u8>,
        clusters: &[ShapedCluster],
        mut pen: f64,
        y: f64,
        size: f64,
        spacing: f64,
        word_spacing: f64,
        current: &mut Option<usize>,
    ) -> f64 {
        for c in clusters {
            let Some(&k) = self.index.get(&cluster_key(c)) else { continue };
            let Some(name) = self.names.get(k / GLYPHS_PER_FONT) else { continue };
            let code = u8::try_from(k % GLYPHS_PER_FONT + 1).unwrap_or(1);
            if *current != Some(k / GLYPHS_PER_FONT) {
                out.extend(format!("/{name} {} Tf ", n(size)).bytes());
                *current = Some(k / GLYPHS_PER_FONT);
            }
            out.extend(format!("1 0 0 1 {} {} Tm ", n(pen), n(y)).bytes());
            out.extend(literal(&[code]));
            out.extend_from_slice(b" Tj\n");
            pen += c.advance * size + spacing + if c.text == " " { word_spacing } else { 0.0 };
        }
        pen
    }

    /// Show a line's `pieces` inside `BT … ET` from `x` on baseline `y` at `size`: shaped clusters
    /// in their Type 3 fonts, the rest in the font `latin` (which the text state has selected when
    /// `current` is `None`), written by `encode` and `measure`d. `current` is as for [`Self::show`].
    #[allow(clippy::too_many_arguments)]
    pub fn show_line(
        &self,
        out: &mut Vec<u8>,
        pieces: &[Piece],
        x: f64,
        y: f64,
        size: f64,
        latin: &str,
        encode: impl Fn(&str) -> Vec<u8>,
        measure: impl Fn(&str) -> f64,
        current: &mut Option<usize>,
    ) {
        let mut pen = x;
        for p in pieces {
            match p {
                Piece::Latin(s) => {
                    if current.take().is_some() {
                        out.extend(format!("/{latin} {} Tf ", n(size)).bytes());
                    }
                    out.extend(format!("1 0 0 1 {} {} Tm ", n(pen), n(y)).bytes());
                    out.extend(literal(&encode(s)));
                    out.extend_from_slice(b" Tj\n");
                    pen += measure(s);
                }
                Piece::Shaped(clusters) => pen = self.show(out, clusters, pen, y, size, 0.0, 0.0, current),
            }
        }
    }
}

/// `lines` as a generated appearance not yet in a document draws them: each line's pieces, its
/// Thai shaped, and the Type 3 fonts for that Thai built into `fonts` (the form's font resources)
/// as direct objects, whose streams [`store_streams`] moves into the document with the
/// appearance. Thai the face can't draw, and every line when a glyph can't be read, is left to the
/// standard font, whose encoder writes `?` for it; [`first_undrawable`] reports that.
pub fn appearance_lines(lines: &[String], fonts: &mut Dict) -> (Vec<Vec<Piece>>, ShapedFonts) {
    let plain = |l: &String| vec![Piece::Latin(l.clone())];
    let pieces: Vec<Vec<Piece>> =
        lines.iter().map(|l| if l.chars().any(is_thai) { thai_pieces(l).unwrap_or_else(|_| plain(l)) } else { plain(l) }).collect();
    match ShapedFonts::new(clusters(pieces.iter().map(Vec::as_slice)), shaped_glyph, &Dict::new(), fonts, "PCTh", &mut |o| o).map_err(|(e, _)| e) {
        Ok(shaped) => (pieces, shaped),
        Err(_) => (lines.iter().map(plain).collect(), ShapedFonts::default()),
    }
}

/// How far the Thai glyphs of `line` reach below and above its baseline, in em units; `None`
/// without Thai, or with Thai the face can't draw.
pub fn thai_reach(line: &str) -> Option<[f64; 2]> {
    if !line.chars().any(is_thai) {
        return None;
    }
    let pieces = thai_pieces(line).ok()?;
    let mut reach: Option<[f64; 2]> = None;
    for c in clusters([pieces.as_slice()]) {
        for (id, [_, dy]) in &c.glyphs {
            let g = shaped_glyph(c.script, *id).ok()?;
            if !g.contours.is_empty() {
                let b = [g.bbox[1] + dy, g.bbox[3] + dy];
                reach = Some(reach.map_or(b, |r| [r[0].min(b[0]), r[1].max(b[1])]));
            }
        }
    }
    reach
}

/// The first character of `s` a generated appearance would draw as `?`: one the standard fonts
/// (WinAnsiEncoding) can't draw and that isn't Thai, or Thai on a line the Thai face can't draw
/// (this build has none, or it lacks a character). Line breaks don't count.
pub fn first_undrawable(s: &str) -> Option<char> {
    s.split(['\n', '\r']).find_map(first_undrawable_in_line)
}

/// [`first_undrawable`] for one line drawn as it is: a line break in it counts.
pub fn first_undrawable_in_line(line: &str) -> Option<char> {
    let thai_fails = line.chars().any(is_thai) && thai_pieces(line).is_err();
    line.chars().find(|c| if is_thai(*c) { thai_fails } else { !crate::win_ansi_encodable(*c) })
}

/// Why the Thai in `text` can't be drawn ([`thai_message`]): this build has no Thai face, or the
/// face lacks a character. For tools that refuse such text rather than write `?`; `None` when all
/// of its Thai draws, or it has none.
pub fn thai_refusal(text: &str) -> Option<String> {
    text.split(['\n', '\r']).filter(|l| l.chars().any(is_thai)).find_map(|l| thai_pieces(l).err().map(|e| thai_message(e, l)))
}

/// [`type3_font`] written into `doc`, with its glyphs, descriptor and ToUnicode as objects of their
/// own: for content that draws `clusters` (at most [`GLYPHS_PER_FONT`]) as codes 1… directly.
pub fn add_type3_font(
    doc: &mut Document,
    clusters: &[&ShapedCluster],
    outlines: &HashMap<(ShapedScript, u32), GlyphOutline>,
) -> pdfcraft_cos::ObjRef {
    let font = type3_font(clusters, outlines, &mut |o| Object::Ref(doc.add(o)));
    doc.add(Object::Dict(font))
}

/// A Type 3 font with `clusters` as codes 1…, each drawn from its script face's glyph `outlines`.
fn type3_font(clusters: &[&ShapedCluster], outlines: &HashMap<(ShapedScript, u32), GlyphOutline>, store: &mut dyn FnMut(Object) -> Object) -> Dict {
    let mut charprocs = Dict::new();
    let mut differences = vec![Object::Int(1)];
    let mut widths = Vec::with_capacity(clusters.len());
    let mut bbox = [0.0f64; 4];
    let mut to_unicode = Vec::new();
    for (i, c) in clusters.iter().enumerate() {
        let code = i + 1;
        // Glyph space is 1000 units per em.
        let mut paths = Vec::new();
        let mut ink: Option<[f64; 4]> = None;
        for (id, [dx, dy]) in &c.glyphs {
            let Some(g) = outlines.get(&(c.script, *id)) else { continue };
            for contour in &g.contours {
                for (j, p) in contour.iter().enumerate() {
                    paths.extend(format!("{} {} {}\n", n((p[0] + dx) * 1000.0), n((p[1] + dy) * 1000.0), if j == 0 { "m" } else { "l" }).bytes());
                }
                paths.extend_from_slice(b"h\n");
                let b = [g.bbox[0] + dx, g.bbox[1] + dy, g.bbox[2] + dx, g.bbox[3] + dy];
                ink = Some(ink.map_or(b, |a| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]));
            }
        }
        // Rounded outwards so no ink is clipped.
        let [x0, y0, x1, y1] =
            ink.map_or([0.0; 4], |b| [(b[0] * 1000.0).floor(), (b[1] * 1000.0).floor(), (b[2] * 1000.0).ceil(), (b[3] * 1000.0).ceil()]);
        bbox = [bbox[0].min(x0), bbox[1].min(y0), bbox[2].max(x1), bbox[3].max(y1)];
        let mut proc = format!("{} 0 {} {} {} {} d1\n", n(c.advance * 1000.0), n(x0), n(y0), n(x1), n(y1)).into_bytes();
        if !paths.is_empty() {
            proc.extend(paths);
            proc.extend_from_slice(b"f\n");
        }
        let glyph = format!("g{code:02X}");
        charprocs.set(glyph.clone().into_bytes(), store(Object::Stream(Stream::flate(Dict::new(), &proc))));
        differences.push(Object::name(&glyph));
        widths.push(Object::Real((c.advance * 1000.0).round()));
        if !c.text.is_empty() {
            let hex: String = c.text.encode_utf16().map(|u| format!("{u:04X}")).collect();
            to_unicode.push(format!("<{code:02X}> <{hex}>\n"));
        }
    }
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapType 2 def\n1 begincodespacerange\n<01> <FF>\nendcodespacerange\n",
    );
    // A CMap block holds at most 100 entries.
    for block in to_unicode.chunks(100) {
        cmap.push_str(&format!("{} beginbfchar\n{}endbfchar\n", block.len(), block.concat()));
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let cmap = store(Object::Stream(Stream::flate(Dict::new(), cmap.as_bytes())));
    // The face's family when every glyph comes from one script; without the face, the script's
    // name (as Arabic items were always named).
    let family = match clusters.first().map(|c| c.script).filter(|s| clusters.iter().all(|c| c.script == *s)) {
        Some(script) => script.font().map_or(script.name(), |f| f.family),
        None => "PdfCraft Shaped Text",
    };
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name(&family.replace(' ', "")));
    descriptor.set(b"FontFamily".to_vec(), Object::String(PdfString::literal(family.as_bytes().to_vec())));
    descriptor.set(b"Flags".to_vec(), Object::Int(4));
    descriptor.set(b"ItalicAngle".to_vec(), Object::Int(0));
    let mut encoding = Dict::new();
    encoding.set(b"Type".to_vec(), Object::name("Encoding"));
    encoding.set(b"Differences".to_vec(), Object::Array(differences));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type3"));
    font.set(b"FontDescriptor".to_vec(), store(Object::Dict(descriptor)));
    font.set(b"FontBBox".to_vec(), Object::Array(bbox.iter().map(|v| Object::Real(*v)).collect()));
    font.set(
        b"FontMatrix".to_vec(),
        Object::Array(vec![Object::Real(0.001), Object::Int(0), Object::Int(0), Object::Real(0.001), Object::Int(0), Object::Int(0)]),
    );
    font.set(b"FirstChar".to_vec(), Object::Int(1));
    font.set(b"LastChar".to_vec(), Object::Int(clusters.len() as i64));
    font.set(b"Widths".to_vec(), Object::Array(widths));
    font.set(b"Encoding".to_vec(), Object::Dict(encoding));
    font.set(b"CharProcs".to_vec(), Object::Dict(charprocs));
    font.set(b"ToUnicode".to_vec(), cmap);
    font
}

/// Most levels of dictionaries and arrays [`store_streams`] looks into: a Type 3 font in a form's
/// resources is four deep (`/Font` → font → `/CharProcs` → glyph).
const STREAM_DEPTH: usize = 8;

/// Move the streams written into `d` (the fonts [`ShapedFonts::new`] built for an appearance not
/// yet in a document) into `doc` as objects of their own, leaving references: a stream is never a
/// direct object (ISO 32000-2 §7.3.8).
pub fn store_streams(doc: &mut Document, d: &mut Dict) {
    fn walk(doc: &mut Document, o: &mut Object, depth: usize) {
        if depth > STREAM_DEPTH {
            return;
        }
        match o {
            Object::Stream(_) => {
                let Object::Stream(mut s) = std::mem::replace(o, Object::Null) else { return };
                walk_dict(doc, &mut s.dict, depth + 1);
                *o = Object::Ref(doc.add(Object::Stream(s)));
            }
            Object::Dict(d) => walk_dict(doc, d, depth + 1),
            Object::Array(a) => a.iter_mut().for_each(|o| walk(doc, o, depth + 1)),
            _ => {}
        }
    }
    fn walk_dict(doc: &mut Document, d: &mut Dict, depth: usize) {
        let keys: Vec<Vec<u8>> =
            d.iter().filter(|(_, v)| matches!(v, Object::Stream(_) | Object::Dict(_) | Object::Array(_))).map(|(k, _)| k.clone()).collect();
        for k in keys {
            if let Some(v) = d.get_mut(&k) {
                walk(doc, v, depth);
            }
        }
    }
    walk_dict(doc, d, 0);
}

/// Format a number for content streams (up to three decimals).
fn n(v: f64) -> String {
    let s = format!("{:.3}", if v.abs() < 5e-4 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(width: f64) -> GlyphOutline {
        GlyphOutline { contours: vec![vec![[0.0, -0.4], [width, -0.4], [width, 1.1], [0.0, 1.1]]], width, bbox: [0.0, -0.4, width, 1.1] }
    }

    fn cluster(text: &str, id: u32) -> ShapedCluster {
        ShapedCluster { script: ShapedScript::Thai, glyphs: vec![(id, [0.0, 0.0])], advance: 0.5, text: text.into() }
    }

    #[test]
    fn appearance_fonts_keep_their_streams_until_stored_then_refer_to_them() {
        let clusters = [cluster("น้ำ", 1), cluster("ก", 2), cluster("น้ำ", 1)];
        let mut fonts = Dict::new();
        let made = ShapedFonts::new(&clusters, |_, _| Ok(boxed(0.5)), &Dict::new(), &mut fonts, "PCTh", &mut |o| o).unwrap();
        assert_eq!(made.code(&clusters[0]), Some(("PCTh1", 1)));
        assert_eq!(made.code(&clusters[1]), Some(("PCTh1", 2)), "a repeated cluster is one glyph");
        let mut line = Vec::new();
        let pieces = [Piece::Latin("a".into()), Piece::Shaped(clusters[..1].to_vec()), Piece::Latin("b".into())];
        made.show_line(&mut line, &pieces, 0.0, 0.0, 10.0, "Helv", |s| s.as_bytes().to_vec(), |_| 1.0, &mut None);
        let shown = String::from_utf8_lossy(&line).into_owned();
        assert_eq!(shown, "1 0 0 1 0 0 Tm (a) Tj\n/PCTh1 10 Tf 1 0 0 1 1 0 Tm (\u{1}) Tj\n/Helv 10 Tf 1 0 0 1 6 0 Tm (b) Tj\n");
        let mut out = Vec::new();
        let mut current = None;
        let pen = made.show(&mut out, &clusters, 10.0, 20.0, 10.0, 0.0, 0.0, &mut current);
        assert_eq!(pen, 25.0);
        assert_eq!(String::from_utf8_lossy(&out).matches(" Tf").count(), 1, "one font selection for one font");
        let mut doc = Document::new_empty();
        let mut res = Dict::new();
        res.set(b"Font".to_vec(), Object::Dict(fonts));
        store_streams(&mut doc, &mut res);
        let font = res.get(b"Font").and_then(Object::as_dict).and_then(|f| f.get(b"PCTh1")).and_then(Object::as_dict).unwrap();
        assert!(matches!(font.get(b"ToUnicode"), Some(Object::Ref(_))));
        let procs = font.get(b"CharProcs").and_then(Object::as_dict).unwrap();
        assert!(procs.iter().count() == 2 && procs.iter().all(|(_, v)| matches!(v, Object::Ref(_))));
    }

    #[test]
    fn a_missing_glyph_names_its_cluster_and_writes_nothing() {
        let clusters = [cluster("ก", 2)];
        let mut fonts = Dict::new();
        let mut stored = 0;
        let made = ShapedFonts::new(&clusters, |_, _| Err(GlyphError::Missing), &Dict::new(), &mut fonts, "PCTh", &mut |o| {
            stored += 1;
            o
        });
        assert!(matches!(made, Err((GlyphError::Missing, c)) if c.text == "ก"));
        assert_eq!((stored, fonts.iter().count()), (0, 0));
    }

    #[test]
    fn thai_runs_are_shaped_and_the_rest_left_to_the_standard_font() {
        assert_eq!(thai_pieces("Hello").unwrap(), [Piece::Latin("Hello".into())]);
        assert!(joins_previous('\u{0E49}') && joins_previous('\u{0E33}') && !joins_previous('ก') && !joins_previous('a'));
        assert_eq!((first_undrawable("Café\nпривет"), thai_refusal("Café"), thai_reach("Hg")), (Some('п'), None, None));
        if crate::document_thai_font().is_none() {
            assert_eq!(thai_pieces("น้ำ 1"), Err(GlyphError::NoFont));
            assert!(thai_message(GlyphError::NoFont, "").contains("this build has none"));
            assert_eq!(thai_width("น้ำ", 10.0, |s, size| s.chars().count() as f64 * size), 30.0, "measured by the standard font");
            assert_eq!(first_undrawable("ok\nชื่อ น้ำ"), Some('ช'));
            assert!(thai_refusal("ok\nน้ำ").is_some_and(|m| m.contains("CRAFT_FONTS_DIR")));
            return;
        }
        assert_eq!((first_undrawable("ชื่อ: น้ำ\nOK"), thai_refusal("ชื่อ: น้ำ")), (None, None));
        assert_eq!(first_undrawable("น้ำ ☃"), Some('☃'));
        // Stacked marks reach above the em box's 0.9, a descender below its 0.3.
        let [_, top] = thai_reach("น้ำ").unwrap();
        let [bottom, _] = thai_reach("ฎู").unwrap();
        assert!(top > 0.95 && bottom < -0.35, "{top} {bottom}");
        let pieces = thai_pieces("ชื่อ: บุญฤทธิ์ 12").unwrap();
        assert!(matches!(&pieces[..], [Piece::Shaped(_), Piece::Latin(a), Piece::Shaped(_), Piece::Latin(b)] if a == ": " && b == " 12"));
        let text: String = clusters([pieces.as_slice()]).map(|c| c.text.as_str()).collect();
        assert_eq!(text, "ชื่อบุญฤทธิ์");
        assert!(thai_width("น้ำ", 10.0, |_, _| 0.0) > 0.0);
    }
}
