//! Arabic shaping and subset Type3 fonts. Outlines use the same variation coordinates as the
//! shaper; ActualText preserves the logical source independently of visual glyph order.
use std::{borrow::Cow, collections::BTreeMap, fmt::Write as _, sync::OnceLock};

use harfrust::{Direction, ShaperData, ShaperInstance, UnicodeBuffer};
use skrifa::{
    FontRef, MetadataProvider,
    instance::Size,
    outline::{DrawSettings, OutlinePen},
};
use unicode_bidi::BidiInfo;

use crate::{CID_FONT, Error, Font, TextDirection, n, needs_cid, pdf_string, pdf_utf16_string};

include!(concat!(env!("OUT_DIR"), "/arabic_fonts.rs"));

const MAX_TEXT_BYTES: usize = 128 * 1024;
const MAX_OUTLINE_BYTES: usize = 256 * 1024;
const MAX_SUBSET_BYTES: usize = 64 * 1024 * 1024;
const MAX_SUBSET_GLYPHS: usize = 16_384;
const SYNTHETIC_BOLD_STROKE: f32 = 30.0;

/// How `text` set in `font` is drawn. Symbol keeps its own one-run encoding, and Arabic without
/// an installed or bundled face falls back to the standard fonts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shaping {
    Plain,
    Shaped,
    NoFont,
}

pub(crate) fn shaping(font: Font, text: &str) -> Shaping {
    if font == Font::Symbol || !needed(text) {
        return Shaping::Plain;
    }
    match face(bold(font)) {
        Ok(None) => Shaping::NoFont,
        Ok(Some(_)) | Err(_) => Shaping::Shaped,
    }
}

pub(crate) fn shaped(font: Font, text: &str) -> bool {
    shaping(font, text) == Shaping::Shaped
}

pub(crate) fn needed(text: &str) -> bool {
    text.chars().any(|ch| matches!(ch as u32, 0x0600..=0x06ff | 0x0750..=0x077f | 0x0870..=0x089f | 0x08a0..=0x08ff | 0xfb50..=0xfdff | 0xfe70..=0xfefc | 0x10e60..=0x10e7f | 0x1ee00..=0x1eeff))
}

fn bold(font: Font) -> bool {
    matches!(
        font,
        Font::HelveticaBold | Font::HelveticaBoldOblique | Font::TimesBold | Font::TimesBoldItalic | Font::CourierBold | Font::CourierBoldOblique
    )
}

fn italic(font: Font) -> bool {
    matches!(
        font,
        Font::HelveticaOblique
            | Font::HelveticaBoldOblique
            | Font::TimesItalic
            | Font::TimesBoldItalic
            | Font::CourierOblique
            | Font::CourierBoldOblique
    )
}

struct Face {
    bytes: Cow<'static, [u8]>,
    data: ShaperData,
    instance: ShaperInstance,
    synthetic_bold: bool,
}

impl Face {
    fn new(bytes: Cow<'static, [u8]>, bold: bool) -> Result<Self, Error> {
        let font = FontRef::from_index(&bytes, 0).map_err(|e| Error(format!("Invalid Arabic PDF font: {e}")))?;
        if !"العربية١٢٣".chars().all(|ch| font.charmap().map(ch).is_some()) {
            return Err(Error("The PDF font lacks Arabic glyphs".into()));
        }
        let data = ShaperData::new(&font);
        let instance = ShaperInstance::from_variations(&font, [("wght", if bold { 700.0 } else { 400.0 })]);
        Ok(Self { bytes, data, instance, synthetic_bold: false })
    }

    fn font(&self) -> Result<FontRef<'_>, Error> {
        FontRef::from_index(&self.bytes, 0).map_err(|e| Error(format!("Invalid Arabic PDF font: {e}")))
    }
}

#[cfg(test)]
pub(crate) fn bold_face_is_synthetic() -> bool {
    face(true).is_ok_and(|face| face.is_some_and(|face| face.synthetic_bold))
}

/// The Arabic face for a weight, or `None` when none is bundled or installed. The search runs
/// once per weight, so measuring text never rescans the font directories.
fn face(is_bold: bool) -> Result<Option<&'static Face>, Error> {
    static REGULAR: OnceLock<Result<Option<Face>, Error>> = OnceLock::new();
    static BOLD: OnceLock<Result<Option<Face>, Error>> = OnceLock::new();
    let cache = if is_bold { &BOLD } else { &REGULAR };
    cache.get_or_init(|| find_face(ARABIC_FONTS, &system_font_dirs(), is_bold)).as_ref().map(Option::as_ref).map_err(Error::clone)
}

fn loaded_face(font: Font) -> Result<&'static Face, Error> {
    face(bold(font))?.ok_or_else(|| Error("Arabic PDF export needs Noto Sans Arabic".into()))
}

fn bundled_match(fonts: &[(&str, &'static [u8])], is_bold: bool) -> Option<&'static [u8]> {
    fonts
        .iter()
        .find(|(name, bytes)| if is_bold { name.ends_with(":Bold") || has_weight_axis(bytes) } else { !name.ends_with(":Bold") })
        .map(|&(_, bytes)| bytes)
}

#[cfg(not(target_arch = "wasm32"))]
fn system_font_dirs() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    let mut dirs: Vec<PathBuf> = [
        "/usr/share/fonts/noto",
        "/usr/share/fonts/google-noto-vf",
        "/usr/share/fonts/truetype/noto",
        "/usr/share/fonts/TTF",
        "/usr/local/share/fonts",
        "/usr/local/share/fonts/noto",
        "/usr/local/share/fonts/TTF",
        "/Library/Fonts",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = PathBuf::from(home);
        dirs.extend([home.join(".local/share/fonts"), home.join(".fonts"), home.join("Library/Fonts")]);
    }
    if let Some(root) = std::env::var_os("WINDIR") {
        dirs.push(PathBuf::from(root).join("Fonts"));
    }
    if let Some(root) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(root).join("Microsoft/Windows/Fonts"));
    }
    dirs
}

#[cfg(target_arch = "wasm32")]
fn system_font_dirs() -> Vec<std::path::PathBuf> {
    Vec::new()
}

fn find_face(bundled: &[(&str, &'static [u8])], dirs: &[std::path::PathBuf], is_bold: bool) -> Result<Option<Face>, Error> {
    use std::io::Read as _;
    if let Some(bytes) = bundled_match(bundled, is_bold) {
        return Face::new(Cow::Borrowed(bytes), is_bold).map(Some);
    }
    let filename = if is_bold { "NotoSansArabic-Bold.ttf" } else { "NotoSansArabic-Regular.ttf" };
    for dir in dirs {
        for name in ["NotoSansArabic.ttf", "NotoSansArabic[wght].ttf", filename] {
            let path = dir.join(name);
            let file = match std::fs::File::open(&path) {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(Error(format!("Cannot read Arabic PDF font {}: {e}", path.display()))),
            };
            let mut bytes = Vec::new();
            file.take((64 << 20) + 1).read_to_end(&mut bytes).map_err(|e| Error(format!("Cannot read Arabic PDF font: {e}")))?;
            if bytes.len() > 64 << 20 {
                return Err(Error("Arabic PDF font exceeds 64 MiB".into()));
            }
            if is_bold && name != filename && !has_weight_axis(&bytes) {
                continue;
            }
            return Face::new(Cow::Owned(bytes), is_bold).map(Some);
        }
    }
    if let Some((name, bytes)) = bundled_fallback(bundled, is_bold) {
        let mut face = Face::new(Cow::Borrowed(bytes), is_bold)?;
        face.synthetic_bold = needs_synthetic_bold(name, bytes, is_bold);
        return Ok(Some(face));
    }
    Ok(None)
}

/// The bundled face to use when no face matches the requested weight: regular text never borrows a bold face.
fn bundled_fallback<'a>(fonts: &[(&'a str, &'static [u8])], is_bold: bool) -> Option<(&'a str, &'static [u8])> {
    fonts.iter().find(|(name, _)| is_bold || !name.ends_with(":Bold")).copied()
}

/// A bold request served by a static regular face is emboldened by stroking its outlines.
fn needs_synthetic_bold(name: &str, bytes: &[u8], is_bold: bool) -> bool {
    is_bold && !name.ends_with(":Bold") && !has_weight_axis(bytes)
}

fn has_weight_axis(bytes: &[u8]) -> bool {
    FontRef::from_index(bytes, 0).is_ok_and(|font| font.axes().iter().any(|axis| axis.tag() == skrifa::raw::types::Tag::new(b"wght")))
}

#[derive(Debug)]
pub(crate) enum Item {
    Glyph { id: u32, x: f32, y: f32, source: String },
    Latin { text: String, x: f32 },
    Cid { text: String, x: f32 },
}

pub(crate) struct Layout {
    pub items: Vec<Item>,
    pub width: f32,
}

/// Positions are in 1/1000 em, matching the Type3 font matrix.
pub(crate) fn layout(font: Font, text: &str, direction: TextDirection) -> Result<Layout, Error> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(Error("Arabic PDF text exceeds the 128 KiB per-line limit".into()));
    }
    let direction = if direction == TextDirection::Context { direction.resolve(text) } else { direction };
    let face = loaded_face(font)?;
    let raw = face.font()?;
    let shaper = face.data.shaper(&raw).instance(Some(&face.instance)).build();
    let units = shaper.units_per_em();
    if units <= 0 {
        return Err(Error("Arabic PDF font has invalid units per em".into()));
    }
    let scale = 1000.0 / units as f32;
    let single_line: String = text.chars().map(|ch| if is_paragraph_break(ch) { ' ' } else { ch }).collect();
    let text = single_line.as_str();
    let bidi = BidiInfo::new(text, direction.level());
    let mut layout = Layout { items: Vec::new(), width: 0.0 };
    for para in &bidi.paragraphs {
        let (levels, runs) = bidi.visual_runs(para, para.range.clone());
        for run in runs {
            let rtl = levels.get(run.start).is_some_and(|level| level.is_rtl());
            let run_text = text.get(run.clone()).ok_or_else(|| Error("Invalid PDF text boundary".into()))?;
            let mut spans = Vec::new();
            let mut start = 0;
            let mut embedded = None;
            for (i, ch) in run_text.char_indices() {
                let supported = embeddable(raw.charmap().map(ch).is_some(), ch);
                if embedded.is_some_and(|v| v != supported) {
                    spans.push((start..i, embedded.unwrap_or(false)));
                    start = i;
                }
                embedded = Some(supported);
            }
            spans.push((start..run_text.len(), embedded.unwrap_or(false)));
            if rtl {
                spans.reverse();
            }
            for (span, embedded) in spans {
                let part = run_text.get(span.clone()).ok_or_else(|| Error("Invalid PDF text boundary".into()))?;
                if !embedded {
                    let part = part.chars().map(|ch| if is_line_break(ch) { ' ' } else { ch });
                    let part: Vec<char> = if rtl { part.rev().map(mirror_ansi).collect() } else { part.collect() };
                    for run in part.chunk_by(|a, b| needs_cid(*a) == needs_cid(*b)) {
                        let (text, x) = (run.iter().collect(), layout.width);
                        layout.width += run.iter().map(|&ch| f32::from(font.char_width(ch))).sum::<f32>();
                        layout.items.push(if run.first().is_some_and(|&ch| needs_cid(ch)) { Item::Cid { text, x } } else { Item::Latin { text, x } });
                    }
                    continue;
                }
                let mut buffer = UnicodeBuffer::new();
                buffer.push_str(part);
                buffer.set_pre_context(run_text.get(..span.start).unwrap_or_default());
                buffer.set_post_context(run_text.get(span.end..).unwrap_or_default());
                buffer.set_direction(if rtl { Direction::RightToLeft } else { Direction::LeftToRight });
                buffer.set_flags(harfrust::BufferFlags::REMOVE_DEFAULT_IGNORABLES);
                if needed(part) {
                    buffer.set_script(harfrust::script::ARABIC);
                }
                buffer.guess_segment_properties();
                let shaped = shaper.shape(buffer, harfrust::ShapeOptions::new());
                let mut clusters: Vec<_> = shaped.glyph_infos().iter().map(|g| g.cluster as usize).collect();
                clusters.push(part.len());
                clusters.sort_unstable();
                clusters.dedup();
                let mut previous_cluster = None;
                for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
                    if info.glyph_id == 0 {
                        return Err(Error("The Arabic PDF font produced a missing glyph".into()));
                    }
                    let cluster = info.cluster as usize;
                    let source = if previous_cluster == Some(cluster) {
                        String::new()
                    } else {
                        let index = clusters.binary_search(&cluster).map_err(|_| Error("Invalid PDF glyph cluster".into()))?;
                        let end = clusters.get(index + 1).copied().ok_or_else(|| Error("Invalid PDF glyph cluster".into()))?;
                        part.get(cluster..end).ok_or_else(|| Error("Invalid PDF glyph cluster".into()))?.to_string()
                    };
                    previous_cluster = Some(cluster);
                    layout.items.push(Item::Glyph {
                        id: info.glyph_id,
                        x: layout.width + pos.x_offset as f32 * scale,
                        y: pos.y_offset as f32 * scale,
                        source,
                    });
                    layout.width += pos.x_advance as f32 * scale;
                }
            }
        }
    }
    Ok(layout)
}

pub(crate) fn vertical_extents(font: Font, text: &str, direction: TextDirection) -> Result<(f32, f32), Error> {
    let layout = layout(font, text, direction)?;
    let face = loaded_face(font)?;
    let raw = face.font()?;
    let metrics = raw.glyph_metrics(Size::new(1000.0), skrifa::instance::LocationRef::new(face.instance.coords()));
    let (mut ascent, mut descent) = (0.0_f32, 0.0_f32);
    for item in layout.items {
        match item {
            Item::Glyph { id, y, .. } => {
                let (bottom, top) = if let Some(bounds) = metrics.bounds(id.into()) {
                    (bounds.y_min, bounds.y_max)
                } else {
                    let glyph = raw.outline_glyphs().get(id.into()).ok_or_else(|| Error("Missing Arabic PDF glyph outline".into()))?;
                    let mut pen = Pen::default();
                    glyph
                        .draw(DrawSettings::unhinted(Size::new(1000.0), skrifa::instance::LocationRef::new(face.instance.coords())), &mut pen)
                        .map_err(|e| Error(format!("Cannot measure Arabic PDF glyph: {e}")))?;
                    if pen.full {
                        return Err(Error("Arabic PDF glyph exceeds the outline size limit".into()));
                    }
                    (pen.bounds[1], pen.bounds[3])
                };
                ascent = ascent.max(y + top);
                descent = descent.max(-y - bottom);
            }
            Item::Latin { .. } | Item::Cid { .. } => {
                ascent = ascent.max(800.0);
                descent = descent.max(220.0);
            }
        }
    }
    Ok((ascent, descent))
}

/// Whether standard-font `text` is drawn in its logical order, unchanged.
pub(crate) fn is_logical_order(text: &str, direction: TextDirection) -> bool {
    direction != TextDirection::RightToLeft && !text.chars().any(is_control)
}

/// Bidi reordering for standard-font text requires no optional Arabic font.
pub(crate) fn latin_visual(text: &str, direction: TextDirection) -> Result<Cow<'_, str>, Error> {
    if is_logical_order(text, direction) {
        return Ok(Cow::Borrowed(text));
    }
    if text.len() > MAX_TEXT_BYTES {
        return Err(Error("Bidirectional PDF text exceeds the 128 KiB per-line limit".into()));
    }
    let single_line: String = text.chars().map(|ch| if is_paragraph_break(ch) { ' ' } else { ch }).collect();
    let text = single_line.as_str();
    let bidi = BidiInfo::new(text, direction.level());
    let mut out = String::with_capacity(text.len());
    for para in &bidi.paragraphs {
        let (levels, runs) = bidi.visual_runs(para, para.range.clone());
        for run in runs {
            let part = text.get(run.clone()).unwrap_or_default();
            if levels.get(run.start).is_some_and(|level| level.is_rtl()) {
                out.extend(part.chars().rev().filter(|ch| !is_control(*ch)).map(mirror_ansi));
            } else {
                out.extend(part.chars().filter(|ch| !is_control(*ch)));
            }
        }
    }
    Ok(Cow::Owned(out))
}

fn mirror_ansi(ch: char) -> char {
    match ch {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '«' => '»',
        '»' => '«',
        '‹' => '›',
        '›' => '‹',
        other => other,
    }
}

/// Whether a character is drawn from the embedded Arabic face. Line breaks never are, so the
/// fallback path draws them as spaces even when the face maps them.
fn embeddable(mapped: bool, ch: char) -> bool {
    (mapped || is_control(ch)) && !is_line_break(ch)
}

fn is_line_break(ch: char) -> bool {
    matches!(ch, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// Bidi paragraph separators (class B). A drawn line is one paragraph: left in, they would split it
/// into paragraphs laid out left to right, swapping the parts of a right-to-left line.
fn is_paragraph_break(ch: char) -> bool {
    matches!(ch, '\n' | '\r' | '\u{1C}'..='\u{1E}' | '\u{85}' | '\u{2029}')
}

fn is_control(ch: char) -> bool {
    matches!(ch, '\u{061c}' | '\u{200c}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

#[derive(Default)]
pub(crate) struct Subset {
    keys: BTreeMap<(u32, bool, bool, String), usize>,
    glyphs: Vec<Glyph>,
    bytes: usize,
    /// Some text on the page set a run in the CJK CID font, so the page must declare it.
    pub(crate) uses_cid: bool,
}

struct Glyph {
    path: String,
    width: f32,
    source: String,
    bounds: [f32; 4],
    emboldened: bool,
}

/// The Type3 glyph procedure: a filled outline, also stroked in the text colour when emboldened.
fn glyph_procedure(glyph: &Glyph) -> String {
    if glyph.emboldened {
        format!("{} 0 d0\n{} w 1 j\n{}B\n", n(glyph.width), n(SYNTHETIC_BOLD_STROKE), glyph.path)
    } else {
        format!("{} 0 d0\n{}f\n", n(glyph.width), glyph.path)
    }
}

impl Subset {
    fn insert(&mut self, font: Font, id: u32, source: String) -> Result<usize, Error> {
        let key = (id, bold(font), italic(font), source.clone());
        if let Some(index) = self.keys.get(&key) {
            return Ok(*index);
        }
        if self.glyphs.len() >= MAX_SUBSET_GLYPHS {
            return Err(Error("PDF page has too many distinct Arabic glyphs".into()));
        }
        let face = loaded_face(font)?;
        let raw = face.font()?;
        let location = skrifa::instance::LocationRef::new(face.instance.coords());
        let glyph = raw.outline_glyphs().get(id.into()).ok_or_else(|| Error("Missing Arabic PDF glyph outline".into()))?;
        let width =
            raw.glyph_metrics(Size::new(1000.0), location).advance_width(id.into()).ok_or_else(|| Error("Missing Arabic PDF glyph width".into()))?;
        let mut pen = Pen { shear: if italic(font) { 0.212_556_56 } else { 0.0 }, ..Default::default() };
        glyph.draw(DrawSettings::unhinted(Size::new(1000.0), location), &mut pen).map_err(|e| Error(format!("Cannot draw Arabic PDF glyph: {e}")))?;
        if pen.full || self.bytes.saturating_add(pen.path.len()).saturating_add(source.len().saturating_mul(2)) > MAX_SUBSET_BYTES {
            return Err(Error("Arabic PDF outlines exceed the page size limit".into()));
        }
        let index = self.glyphs.len();
        self.bytes += pen.path.len() + source.len() * 2;
        let emboldened = face.synthetic_bold;
        let grow = if emboldened { SYNTHETIC_BOLD_STROKE / 2.0 } else { 0.0 };
        let [x0, y0, x1, y1] = pen.bounds;
        self.glyphs.push(Glyph { path: pen.path, width, source, bounds: [x0 - grow, y0 - grow, x1 + grow, y1 + grow], emboldened });
        self.keys.insert(key, index);
        Ok(index)
    }

    pub(crate) fn text(&mut self, font: Font, text: &str, size: f32, direction: TextDirection) -> Result<String, Error> {
        let layout = layout(font, text, direction)?;
        let mut out = format!("/Span << /ActualText {} >> BDC\nBT\n", unicode_string(text, true));
        for item in layout.items {
            match item {
                Item::Glyph { id, x, y, source } => {
                    let index = self.insert(font, id, source)?;
                    let _ = writeln!(
                        out,
                        "/A{} {} Tf 1 0 0 1 {} {} Tm <{:02X}> Tj",
                        index / 255 + 1,
                        n(size),
                        n(x * size / 1000.0),
                        n(y * size / 1000.0),
                        index % 255 + 1
                    );
                }
                Item::Latin { text, x } => {
                    let _ = writeln!(out, "/F{} {} Tf 1 0 0 1 {} 0 Tm {} Tj", font.index() + 1, n(size), n(x * size / 1000.0), pdf_string(&text));
                }
                Item::Cid { text, x } => {
                    self.uses_cid = true;
                    let _ = writeln!(out, "/F{} {} Tf 1 0 0 1 {} 0 Tm {} Tj", CID_FONT + 1, n(size), n(x * size / 1000.0), pdf_utf16_string(&text));
                }
            }
        }
        out.push_str("ET\nEMC");
        Ok(out)
    }

    /// Writes the subset fonts into `out` and returns their page resource entries.
    pub(crate) fn write_objects(&self, next: &mut u32, out: &mut craft_pdf::classic::Writer) -> Result<String, Error> {
        let mut resources = String::new();
        for (batch, glyphs) in self.glyphs.chunks(255).enumerate() {
            let id = *next;
            let cmap_id = id + 1;
            *next += 2;
            let mut procs = String::new();
            let mut names = String::new();
            let mut widths = String::new();
            let mut mappings = Vec::new();
            let mut bounds = [0.0_f32; 4];
            for (i, glyph) in glyphs.iter().enumerate() {
                let code = i + 1;
                let _ = write!(procs, "/g{code} {} 0 R ", *next);
                let _ = write!(names, "/g{code} ");
                let _ = write!(widths, "{} ", n(glyph.width));
                let body = glyph_procedure(glyph);
                out.stream(*next, "", body.as_bytes())?;
                *next += 1;
                bounds[0] = bounds[0].min(glyph.bounds[0]);
                bounds[1] = bounds[1].min(glyph.bounds[1]);
                bounds[2] = bounds[2].max(glyph.bounds[2]);
                bounds[3] = bounds[3].max(glyph.bounds[3]);
                if !glyph.source.is_empty() {
                    mappings.push(format!("<{code:02X}> {}\n", unicode_string(&glyph.source, false)));
                }
            }
            let mut cmap = "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /GridCraftArabicUnicode def\n/CMapType 2 def\n1 begincodespacerange\n<00> <FF>\nendcodespacerange\n".to_string();
            for group in mappings.chunks(100) {
                let _ = writeln!(cmap, "{} beginbfchar", group.len());
                for mapping in group {
                    cmap.push_str(mapping);
                }
                cmap.push_str("endbfchar\n");
            }
            cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
            out.stream(cmap_id, "", cmap.as_bytes())?;
            out.object(id,
                &format!(
                    "<< /Type /Font /Subtype /Type3 /Name /GridCraftArabic /FontBBox [{} {} {} {}] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << {procs}>> /Encoding << /Type /Encoding /Differences [1 {names}] >> /FirstChar 1 /LastChar {} /Widths [{widths}] /Resources << >> /ToUnicode {cmap_id} 0 R >>",
                    n(bounds[0]),
                    n(bounds[1]),
                    n(bounds[2]),
                    n(bounds[3]),
                    glyphs.len()
                ),
            )?;
            let _ = write!(resources, "/A{} {id} 0 R ", batch + 1);
        }
        Ok(resources)
    }
}

pub(crate) fn unicode_string(text: &str, bom: bool) -> String {
    let mut out = if bom { "<FEFF".to_string() } else { "<".to_string() };
    for unit in text.encode_utf16() {
        let _ = write!(out, "{unit:04X}");
    }
    out.push('>');
    out
}

#[derive(Default)]
struct Pen {
    path: String,
    point: [f32; 2],
    bounds: [f32; 4],
    shear: f32,
    full: bool,
}

impl Pen {
    fn point(&mut self, x: f32, y: f32) -> [f32; 2] {
        let x = x + self.shear * y;
        self.bounds[0] = self.bounds[0].min(x);
        self.bounds[1] = self.bounds[1].min(y);
        self.bounds[2] = self.bounds[2].max(x);
        self.bounds[3] = self.bounds[3].max(y);
        [x, y]
    }
    fn op(&mut self, value: &str) {
        if self.path.len().saturating_add(value.len()) > MAX_OUTLINE_BYTES {
            self.full = true;
        }
        if !self.full {
            self.path.push_str(value);
        }
    }
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        let [px, py] = self.point(x, y);
        self.op(&format!("{} {} m\n", n(px), n(py)));
        self.point = [x, y];
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let [px, py] = self.point(x, y);
        self.op(&format!("{} {} l\n", n(px), n(py)));
        self.point = [x, y];
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.curve_to(
            self.point[0] + (cx - self.point[0]) * 2.0 / 3.0,
            self.point[1] + (cy - self.point[1]) * 2.0 / 3.0,
            x + (cx - x) * 2.0 / 3.0,
            y + (cy - y) * 2.0 / 3.0,
            x,
            y,
        );
    }
    fn curve_to(&mut self, cx: f32, cy: f32, dx: f32, dy: f32, x: f32, y: f32) {
        let [cx, cy] = self.point(cx, cy);
        let [dx, dy] = self.point(dx, dy);
        let [px, py] = self.point(x, y);
        self.op(&format!("{} {} {} {} {} {} c\n", n(cx), n(cy), n(dx), n(dy), n(px), n(py)));
        self.point = [x, y];
    }
    fn close(&mut self) {
        self.op("h\n");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fallback_mirrors_only_paired_ansi_punctuation() {
        let visual: String = "(مرحبا)".chars().rev().map(super::mirror_ansi).collect();
        assert_eq!(visual, "(ابحرم)");
        assert_eq!("[(<«‹abc›»>)]".chars().rev().map(super::mirror_ansi).collect::<String>(), "[(<«‹cba›»>)]");
    }

    #[test]
    fn a_bold_request_served_by_a_static_regular_face_is_emboldened() {
        assert!(super::needs_synthetic_bold("craft:Noto Sans Arabic:Regular", b"static", true));
        assert!(!super::needs_synthetic_bold("craft:Noto Sans Arabic:Regular", b"static", false));
        assert!(!super::needs_synthetic_bold("craft:Noto Sans Arabic:Bold", b"static", true));
        let glyph =
            |emboldened| super::Glyph { path: "0 0 m 10 0 l 10 10 l h\n".into(), width: 500.0, source: String::new(), bounds: [0.0; 4], emboldened };
        assert_eq!(super::glyph_procedure(&glyph(false)), "500 0 d0\n0 0 m 10 0 l 10 10 l h\nf\n");
        assert_eq!(super::glyph_procedure(&glyph(true)), "500 0 d0\n30 w 1 j\n0 0 m 10 0 l 10 10 l h\nB\n");
    }

    #[test]
    fn line_breaks_never_use_the_embedded_face() {
        for ch in ['\n', '\r', '\u{2028}', '\u{2029}'] {
            assert!(!super::embeddable(true, ch), "{ch:?}");
        }
        assert!(super::embeddable(true, 'م'));
        assert!(super::embeddable(false, '\u{200f}'));
        assert!(!super::embeddable(false, 'A'));
    }

    #[test]
    fn no_bundled_or_installed_face_is_not_an_error() {
        for bold in [false, true] {
            assert!(matches!(super::find_face(&[], &[], bold), Ok(None)));
            assert!(matches!(super::find_face(&[], &[std::env::temp_dir().join("gridcraft-no-fonts-here")], bold), Ok(None)));
        }
    }

    #[test]
    fn bundled_faces_match_the_requested_weight() {
        const REGULAR: &[u8] = b"regular";
        const BOLD: &[u8] = b"bold";
        let bold_first = [("Noto Sans Arabic:Bold", BOLD), ("Noto Sans Arabic", REGULAR)];
        assert_eq!(super::bundled_match(&bold_first, false), Some(REGULAR));
        assert_eq!(super::bundled_match(&bold_first, true), Some(BOLD));
        assert_eq!(super::bundled_match(&[("Noto Sans Arabic", REGULAR)], true), None);
        assert_eq!(super::bundled_match(&[], false), None);
        assert_eq!(super::bundled_fallback(&[("Noto Sans Arabic:Bold", BOLD)], false), None);
        assert_eq!(super::bundled_fallback(&[("Noto Sans Arabic:Regular", REGULAR)], true), Some(("Noto Sans Arabic:Regular", REGULAR)));
    }
}
