//! PDF/A 6.2.11.4: embedding a program for a standard-14 text font that a document names without
//! embedding (#7658). PDF/A wants every font used for rendering embedded, and most files name
//! Helvetica or Times this way, so without it a conversion could never finish.
//!
//! The program is the substitute PdfCraft already draws that font with
//! ([`pdfcraft_render::standard_font_program`]), so the page looks as it did. Its advances differ
//! from the standard-14 metrics for a few glyphs (the euro sign, `@` in some faces, guillemets),
//! and PDF/A wants a font dictionary's widths consistent with its embedded program (ISO 19005-2,
//! 6.2.11.5), so `/Widths` is written from the program for every code.
//!
//! Only a simple `/Type1` font of the twelve Latin text faces (or a name that is an alias of one,
//! such as `Arial,Bold`) is embedded, under an encoding that maps its codes to glyphs the program
//! has: WinAnsi, MacRoman or the built-in StandardEncoding, with `/Differences` naming glyphs the
//! program holds. Symbol and ZapfDingbats are left alone, since their substitutes' encodings and
//! advances don't match the standard ones, and so are TrueType and composite fonts.
//!
//! Writing the substitute's widths must not move any text, so a font is embedded only if every
//! code it draws ([`Drawn`], read from the content) is laid out now with the substitute's advance,
//! and every other width it gives is the substitute's or the standard metric (see [`plan`]). That
//! leaves alone a font that draws one of the few glyphs the substitute differs on, and a font
//! whose name reads as an alias of a standard face but isn't it (`Helvetica-Narrow`).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};
use pdfcraft_fonts::{Std14, Std14Match};
use skrifa::GlyphId;
use skrifa::raw::model::pen::{NullPen, OutlinePen};
use skrifa::raw::ps::cff::CffFontRef;

/// The units per em of the substitutes, whose advances are then in glyph space as written.
const UNITS_PER_EM: i32 = 1000;

/// The `/Encoding` of a simple font, read to the extent an embedded program needs.
struct FontEncoding {
    /// The base encoding: the font's own, or StandardEncoding when it names none (a Latin
    /// substitute's built-in encoding is StandardEncoding).
    base: Vec<u8>,
    /// The `/Differences` glyph names, by code.
    differences: HashMap<u8, Vec<u8>>,
}

impl FontEncoding {
    /// `None` for an encoding whose codes a substitute can't be shown to cover: MacExpert, a
    /// misspelt name, or something that is neither a name nor a dictionary.
    fn read(doc: &Document, font: &Dict) -> Option<Self> {
        let known = |n: &[u8]| matches!(n, b"WinAnsiEncoding" | b"MacRomanEncoding" | b"StandardEncoding");
        let standard = || b"StandardEncoding".to_vec();
        let Some(enc) = font.get(b"Encoding").map(|e| doc.resolve(e)) else {
            return Some(Self { base: standard(), differences: HashMap::new() });
        };
        match &*enc {
            Object::Name(n) if known(n) => Some(Self { base: n.clone(), differences: HashMap::new() }),
            Object::Dict(d) => {
                let base = match d.get(b"BaseEncoding").map(|b| doc.resolve(b)).as_deref() {
                    None => standard(),
                    Some(Object::Name(n)) if known(n) => n.clone(),
                    Some(_) => return None,
                };
                let mut differences = HashMap::new();
                if let Some(diffs) = d.get(b"Differences").map(|x| doc.resolve(x)) {
                    let mut code: Option<u8> = None;
                    for item in diffs.as_array()? {
                        match item {
                            // Codes past 255 name nothing a simple font shows.
                            Object::Int(n) => code = u8::try_from(*n).ok(),
                            Object::Name(n) => {
                                if let Some(c) = code {
                                    differences.insert(c, n.clone());
                                }
                                code = code.and_then(|c| c.checked_add(1));
                            }
                            _ => return None,
                        }
                    }
                }
                Some(Self { base, differences })
            }
            _ => None,
        }
    }
}

/// What embedding a substitute into one font dictionary writes.
pub(crate) struct Embedding {
    face: Std14,
    /// The advance of each code 0–255 under the font's encoding, from the program.
    widths: Vec<f64>,
    metrics: &'static Substitute,
}

/// What a substitute program holds that embedding it needs, read once per face: the font
/// dictionaries of a document are many, and may be hostile in number.
struct Substitute {
    /// The advance of each glyph, by name.
    by_name: HashMap<Vec<u8>, f64>,
    /// The advance of the first glyph whose name reads as each character.
    by_char: HashMap<char, f64>,
    notdef: f64,
    bbox: [f64; 4],
    italic_angle: f64,
    /// The tops of `d` and `H` and the bottom of `p`, for `/Ascent`, `/CapHeight` and `/Descent`.
    ascent: f64,
    cap_height: f64,
    descent: f64,
}

/// The substitute for `face`, read on first use; `None` if its program can't be read.
fn substitute(face: Std14) -> Option<&'static Substitute> {
    static READ: [OnceLock<Option<Substitute>>; 14] = [const { OnceLock::new() }; 14];
    READ.get(face as usize)?.get_or_init(|| read_substitute(&pdfcraft_render::standard_font_program(face))).as_ref()
}

/// The highest and lowest point an outline reaches, control points included (enough for the
/// straight stems of `d`, `H` and `p`).
#[derive(Default)]
struct Extent {
    top: Option<f32>,
    bottom: Option<f32>,
}

impl Extent {
    fn point(&mut self, y: f32) {
        self.top = Some(self.top.map_or(y, |t| t.max(y)));
        self.bottom = Some(self.bottom.map_or(y, |b| b.min(y)));
    }
}

impl OutlinePen for Extent {
    fn move_to(&mut self, _: f32, y: f32) {
        self.point(y);
    }
    fn line_to(&mut self, _: f32, y: f32) {
        self.point(y);
    }
    fn quad_to(&mut self, _: f32, y1: f32, _: f32, y: f32) {
        self.point(y1);
        self.point(y);
    }
    fn curve_to(&mut self, _: f32, y1: f32, _: f32, y2: f32, _: f32, y: f32) {
        self.point(y1);
        self.point(y2);
        self.point(y);
    }
    fn close(&mut self) {}
}

/// How far, in thousandths of an em, a width the document gives may be from the one embedding
/// writes and still count as the same: generators round widths, or write them as reals.
const WIDTH_TOLERANCE: f64 = 1.0;

/// The standard face of the font dictionary `font`, if it is a simple standard-14 Latin text font
/// with no program of its own: one [`plan`] may find a substitute for.
fn candidate(doc: &Document, font: &Dict) -> Option<Std14> {
    if font.name(b"Subtype") != Some(b"Type1") || crate::font_embedded(doc, font) {
        return None;
    }
    let base_font = String::from_utf8_lossy(font.name(b"BaseFont")?).into_owned();
    match Std14::recognise(&base_font)? {
        (Std14::Symbol | Std14::ZapfDingbats, _) | (_, Std14Match::FamilyHint) => None,
        (face, Std14Match::Alias) => Some(face),
    }
}

/// The substitute to embed into the font dictionary `font` (object `r`), if it is a simple
/// standard-14 Latin text font with no program of its own, under an encoding the substitute
/// covers, and embedding it moves no glyph the font draws.
///
/// Embedding writes `/Widths` from the substitute (6.2.11.5), so a glyph would move wherever the
/// width it is laid out with now (the font's own `/Widths`, or with none the standard metric)
/// differs from the substitute's advance.
/// - Each code the font draws (`drawn`) must have that width be the substitute's advance. The
///   substitutes differ from the standard metrics on a few glyphs (the euro sign, `@` in some
///   faces, guillemets), so a page that draws one of those with the standard width keeps its
///   font unembedded rather than have the rest of the line move.
/// - Each other width the font gives must be the substitute's advance or the standard metric.
///   The name alone doesn't say the font is the standard face: `Helvetica-Narrow` and
///   `Helvetica-Condensed` read as aliases of Helvetica, but their widths are 15–22% narrower.
pub(crate) fn plan(doc: &Document, r: ObjRef, font: &Dict, drawn: &Drawn) -> Option<Embedding> {
    let face = candidate(doc, font)?;
    let encoding = FontEncoding::read(doc, font)?;
    let given = given_widths(doc, font)?;
    let metrics = substitute(face)?;
    let close = |a: f64, b: f64| (a - b).abs() <= WIDTH_TOLERANCE;
    let mut widths = Vec::with_capacity(256);
    for code in 0..=u8::MAX {
        let (advance, ch) = match encoding.differences.get(&code) {
            // A glyph the program lacks would be drawn as .notdef: not this font's to fix.
            Some(name) => {
                let ch = std::str::from_utf8(name).ok().and_then(pdfcraft_fonts::pdf::glyph_unicode);
                (Some(*metrics.by_name.get(name)?), ch)
            }
            None => {
                let ch = pdfcraft_fonts::base_encoding_char(&encoding.base, code);
                (ch.and_then(|c| metrics.by_char.get(&c).copied()), ch)
            }
        };
        let standard = ch.and_then(pdfcraft_fonts::win_ansi_byte).map(|b| face.width(b)).filter(|&s| s != 0.0);
        let own = given.as_ref().map(|g| g.get(usize::from(code)).copied().unwrap_or(0.0));
        if drawn.draws(r, code) {
            let now = if given.is_some() { own } else { standard };
            if !matches!((now, advance), (Some(n), Some(a)) if close(n, a)) {
                return None;
            }
        } else if let (Some(w), Some(a)) = (own.filter(|&w| w != 0.0), advance)
            && !close(w, a)
            && !standard.is_some_and(|s| close(w, s))
        {
            return None;
        }
        widths.push(advance.unwrap_or(metrics.notdef));
    }
    Some(Embedding { face, widths, metrics })
}

/// The width `font`'s `/Widths` gives each code 0–255, zero for a code it gives none for, or
/// `Some(None)` if it has no `/Widths`. `None` if `/Widths` is there but can't be read, so a font
/// whose widths can't be compared isn't embedded.
fn given_widths(doc: &Document, font: &Dict) -> Option<Option<Vec<f64>>> {
    let Some(list) = font.get(b"Widths").map(|w| doc.resolve(w)) else { return Some(None) };
    let Object::Int(first) = *doc.resolve(font.get(b"FirstChar")?) else { return None };
    let mut given = vec![0.0; 256];
    for (i, w) in list.as_array()?.iter().take(256).enumerate() {
        let w = doc.resolve(w).as_f64()?;
        let code = i64::try_from(i).ok().and_then(|i| first.checked_add(i)).and_then(|c| usize::try_from(c).ok());
        if let Some(slot) = code.and_then(|c| given.get_mut(c)) {
            *slot = w;
        }
    }
    Some(Some(given))
}

/// How many bytes of content [`Drawn::read`] decodes before it stops and counts every code drawn.
const MAX_DRAWN_DECODED: usize = 1 << 30;

/// How deeply `q` nests before further saved states are counted rather than kept (and the codes
/// drawn then count as unknown, since a `Q` can no longer restore the font).
const MAX_SAVED_STATES: usize = 1024;

/// Fonts by the name a resource dictionary gives them: the font dictionary a reference ends at,
/// or none for a font written in place (never a candidate, since only referenced ones are).
type ResourceFonts = HashMap<Vec<u8>, Vec<ObjRef>>;

/// The codes each font draws, from the document's content: its pages, and its form XObjects
/// (appearance streams among them), tiling patterns and Type 3 glyphs.
pub(crate) struct Drawn {
    codes: HashMap<ObjRef, [bool; 256]>,
    /// Whether `codes` is the whole story. Not if a content stream couldn't be read, text was
    /// shown with a font its content doesn't name (a form XObject using its caller's font), a
    /// graphics state sets a font (`/ExtGState /Font`, which `Tf` doesn't show), or `q` nests past
    /// [`MAX_SAVED_STATES`]: then any font may draw any code.
    known: bool,
}

impl Drawn {
    /// Read the codes the fonts draw. The content is read only when one of `fonts` (those the
    /// document draws with) could be embedded, since most documents embed theirs.
    pub(crate) fn read(doc: &Document, objs: &[(ObjRef, Arc<Object>)], fonts: &HashSet<ObjRef>) -> Self {
        let mut drawn = Self { codes: HashMap::new(), known: true };
        if !fonts.iter().any(|&r| doc.get(r).as_dict().is_some_and(|f| candidate(doc, f).is_some())) {
            return drawn;
        }
        let mut budget = pdfcraft_cos::DecodeBudget::new(MAX_DRAWN_DECODED, usize::MAX);
        // Every font any resource dictionary names, for content with no resources of its own (a
        // Type 3 glyph, or a form XObject that uses its page's).
        let mut every: HashMap<Vec<u8>, HashSet<ObjRef>> = HashMap::new();
        for (_, o) in objs {
            if let Some(res) = crate::dict_of(o).and_then(|d| d.get(b"Resources")) {
                for (name, refs) in drawn.fonts(doc, res) {
                    every.entry(name).or_default().extend(refs);
                }
            }
        }
        // One of each font per name, however many resource dictionaries repeat it, so text is
        // marked once per font.
        let every: ResourceFonts = every.into_iter().map(|(name, refs)| (name, refs.into_iter().collect())).collect();
        let mut pages_content = HashSet::new();
        for page in pdfcraft_model::pages(doc) {
            let fonts = page.dict.get(b"Resources").map(|res| drawn.fonts(doc, res)).unwrap_or_default();
            // A page's content may be split between its streams anywhere, so they're read as one.
            let mut data = Vec::new();
            for r in crate::page_contents(doc, &page.dict) {
                pages_content.insert(r);
                if let Object::Stream(s) = &*doc.get(r) {
                    match budget.decode(s) {
                        Ok(d) => {
                            data.extend(d);
                            data.push(b'\n');
                        }
                        Err(_) => drawn.known = false,
                    }
                }
            }
            drawn.scan(&data, &fonts);
        }
        let referenced = crate::referenced_content(doc, objs);
        for (r, o) in objs {
            let Object::Stream(s) = &**o else { continue };
            if pages_content.contains(r) || !(referenced.contains(r) || crate::is_content(&s.dict)) {
                continue;
            }
            let own = s.dict.get(b"Resources").map(|res| drawn.fonts(doc, res));
            match budget.decode(s) {
                Ok(data) => drawn.scan(&data, own.as_ref().unwrap_or(&every)),
                Err(_) => drawn.known = false,
            }
        }
        drawn
    }

    /// Whether the font dictionary `font` may draw `code`.
    fn draws(&self, font: ObjRef, code: u8) -> bool {
        !self.known || self.codes.get(&font).and_then(|c| c.get(usize::from(code))).copied().unwrap_or(false)
    }

    /// The fonts the resource dictionary `res` names.
    fn fonts(&mut self, doc: &Document, res: &Object) -> ResourceFonts {
        let mut out = ResourceFonts::new();
        let res = doc.resolve(res);
        let Some(res) = res.as_dict() else { return out };
        if let Some(fonts) = res.get(b"Font").map(|f| doc.resolve(f))
            && let Some(fonts) = fonts.as_dict()
        {
            for (name, f) in fonts.iter() {
                out.insert(name.clone(), f.as_ref().map(|r| vec![crate::font_object(doc, r)]).unwrap_or_default());
            }
        }
        if let Some(states) = res.get(b"ExtGState").map(|g| doc.resolve(g))
            && let Some(states) = states.as_dict()
            && states.iter().any(|(_, s)| doc.resolve(s).as_dict().is_some_and(|s| s.get(b"Font").is_some()))
        {
            self.known = false;
        }
        out
    }

    /// Record the codes the content `data` shows, with the fonts `fonts` names.
    fn scan(&mut self, data: &[u8], fonts: &ResourceFonts) {
        // The fonts `Tf` last chose, or `None` before any (or for a name `fonts` doesn't hold).
        let mut font: Option<&[ObjRef]> = None;
        let mut saved = Vec::new();
        let mut deeper = 0usize;
        for op in pdfcraft_content::ops(data) {
            let shown: Vec<&Object> = match op.op.as_slice() {
                b"q" => {
                    if saved.len() < MAX_SAVED_STATES {
                        saved.push(font);
                    } else {
                        // The matching `Q` can't restore the font this one would have saved.
                        self.known = false;
                        deeper = deeper.saturating_add(1);
                    }
                    continue;
                }
                b"Q" => {
                    if deeper > 0 {
                        deeper -= 1;
                    } else if let Some(f) = saved.pop() {
                        font = f;
                    }
                    continue;
                }
                b"Tf" => {
                    font = op.name(0).and_then(|n| fonts.get(n)).map(Vec::as_slice);
                    continue;
                }
                b"Tj" | b"'" => op.operands.first().into_iter().collect(),
                b"\"" => op.operands.get(2).into_iter().collect(),
                b"TJ" => op.operands.first().and_then(Object::as_array).map(|a| a.iter().collect()).unwrap_or_default(),
                _ => continue,
            };
            let Some(refs) = font else {
                self.known = false;
                continue;
            };
            for s in shown.iter().filter_map(|o| o.as_string()) {
                for &r in refs {
                    let codes = self.codes.entry(r).or_insert([false; 256]);
                    for &b in &s.bytes {
                        if let Some(c) = codes.get_mut(usize::from(b)) {
                            *c = true;
                        }
                    }
                }
            }
        }
    }
}

/// Read the substitute `program`; `None` if it can't be read, or a glyph has no advance.
fn read_substitute(program: &[u8]) -> Option<Substitute> {
    let cff = CffFontRef::new(program, 0, None).ok()?;
    if cff.upem() != UNITS_PER_EM {
        return None;
    }
    let subfont = cff.subfont(0, &[]).ok()?;
    let advance = |gid: GlyphId| cff.draw(&subfont, gid, &[], None, &mut NullPen).ok().flatten().map(f64::from);
    let mut glyphs: HashMap<Vec<u8>, GlyphId> = HashMap::new();
    let mut by_name = HashMap::new();
    let mut by_char = HashMap::new();
    for (gid, sid) in cff.charset()?.iter() {
        let Some(name) = cff.string(sid) else { continue };
        let width = advance(gid)?;
        glyphs.entry(name.to_vec()).or_insert(gid);
        by_name.entry(name.to_vec()).or_insert(width);
        if let Some(c) = std::str::from_utf8(name).ok().and_then(pdfcraft_fonts::pdf::glyph_unicode) {
            by_char.entry(c).or_insert(width);
        }
    }
    let extent = |name: &[u8]| {
        let mut e = Extent::default();
        let g = *glyphs.get(name)?;
        cff.draw(&subfont, g, &[], None, &mut e).ok()?;
        Some(e)
    };
    let md = cff.metadata()?;
    let b = md.bbox();
    let bbox = [b.x_min, b.y_min, b.x_max, b.y_max].map(|v| f64::from(v.to_f32()));
    let ascent = extent(b"d").and_then(|e| e.top).map_or(bbox[3], f64::from);
    let cap_height = extent(b"H").and_then(|e| e.top).map_or(ascent, f64::from);
    let descent = extent(b"p").and_then(|e| e.bottom).map_or(bbox[1], f64::from);
    Some(Substitute {
        by_name,
        by_char,
        notdef: advance(GlyphId::NOTDEF)?,
        bbox,
        italic_angle: f64::from(md.italic_angle().to_f32()),
        ascent,
        cap_height,
        descent,
    })
}

/// A number as written into a font dictionary: an integer where it is one.
fn number(v: f64) -> Object {
    if v.fract() == 0.0 && v.abs() < 1e9 { Object::Int(v as i64) } else { Object::Real(v) }
}

/// The `/FontFile3` program streams written so far in one conversion, by face: the font
/// dictionaries of one face share one (some generators write a `/Helvetica` per page, and
/// a copy each would grow the file by the program every time).
pub(crate) type Programs = HashMap<Std14, ObjRef>;

/// Embed `e` into the font dictionary `r`: a `/FontFile3` program (the face's one in `programs`,
/// written there on first use), `/Widths` for codes 0–255 from it, and a font descriptor of its
/// own. The descriptor starts from the font's old one, so a value the document gave (its flags
/// above all, which say how codes are read) stays; one that describes the program (its name,
/// box and character set) is replaced.
pub(crate) fn embed(doc: &mut Document, r: ObjRef, embedding: Embedding, programs: &mut Programs) -> Result<(), pdfcraft_cos::CosError> {
    let Embedding { face, widths, metrics: e } = embedding;
    let Object::Dict(mut font) = (*doc.get(r)).clone() else { return Ok(()) };
    let mut fd = font.get(b"FontDescriptor").map(|d| doc.resolve(d)).and_then(|d| d.as_dict().cloned()).unwrap_or_default();
    let file = *programs.entry(face).or_insert_with(|| {
        let mut file = Dict::new();
        file.set(b"Subtype".to_vec(), Object::name("Type1C"));
        doc.add(Object::Stream(Stream::flate(file, &pdfcraft_render::standard_font_program(face))))
    });
    fd.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    if let Some(name) = font.get(b"BaseFont").cloned() {
        fd.set(b"FontName".to_vec(), name);
    }
    if fd.get(b"Flags").is_none() {
        // Nonsymbolic (bit 6), plus FixedPitch (1), Serif (2) and Italic (7) as the face is.
        let italic = matches!(
            face,
            Std14::HelveticaOblique
                | Std14::HelveticaBoldOblique
                | Std14::TimesItalic
                | Std14::TimesBoldItalic
                | Std14::CourierOblique
                | Std14::CourierBoldOblique
        );
        let serif = matches!(face, Std14::TimesRoman | Std14::TimesBold | Std14::TimesItalic | Std14::TimesBoldItalic);
        let flags = 32 | i64::from(face.is_fixed_pitch()) | i64::from(serif) << 1 | i64::from(italic) << 6;
        fd.set(b"Flags".to_vec(), Object::Int(flags));
    }
    fd.set(b"FontBBox".to_vec(), Object::Array(e.bbox.iter().map(|&v| number(v)).collect()));
    for (key, value) in [
        (&b"ItalicAngle"[..], e.italic_angle),
        (b"Ascent", e.ascent),
        (b"Descent", e.descent),
        (b"CapHeight", e.cap_height),
        // Readers use the stem width only to fake a font they don't have; this one is embedded,
        // so a typical value for the weight serves.
        (b"StemV", if is_bold(face) { 140.0 } else { 80.0 }),
    ] {
        if fd.get(key).is_none() {
            fd.set(key.to_vec(), number(value));
        }
    }
    // A character set listed for some other program would misdescribe this one.
    fd.remove(b"CharSet");
    fd.set(b"FontFile3".to_vec(), Object::Ref(file));
    let fd = doc.add(Object::Dict(fd));
    font.set(b"FontDescriptor".to_vec(), Object::Ref(fd));
    font.set(b"FirstChar".to_vec(), Object::Int(0));
    font.set(b"LastChar".to_vec(), Object::Int(255));
    font.set(b"Widths".to_vec(), Object::Array(widths.iter().map(|&w| number(w)).collect()));
    doc.set_existing(r, Object::Dict(font))
}

fn is_bold(face: Std14) -> bool {
    matches!(
        face,
        Std14::HelveticaBold
            | Std14::HelveticaBoldOblique
            | Std14::TimesBold
            | Std14::TimesBoldItalic
            | Std14::CourierBold
            | Std14::CourierBoldOblique
    )
}
