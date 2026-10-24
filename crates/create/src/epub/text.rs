//! Fonts and shaping for the layout. Each character is set in the first face that has it: the
//! book's own fonts for the families its CSS names, the standard 14 (Times, Helvetica, Courier)
//! for the generic families and for text WinAnsi can encode, then Inter, then the craft-fonts faces
//! when PdfCraft was built with them. Text in a font program is shaped with harfrust (ligatures,
//! marks, contextual forms, right-to-left runs), so it is drawn as reading systems draw it.

use std::collections::HashMap;
use std::sync::Arc;

use harfrust::{BufferClusterLevel, Direction, ShapeOptions, ShaperData, UnicodeBuffer};
use pdfcraft_fonts::Std14;
use skrifa::raw::TableProvider;
use skrifa::{FontRef, MetadataProvider};

use super::dom::RunStyle;

/// A face text is set in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FaceId {
    Std(Std14),
    /// Index into [`Fonts::programs`].
    Program(usize),
}

/// A font program's bytes: PdfCraft's own (static) or the book's.
#[derive(Clone)]
pub enum Bytes {
    Static(&'static [u8]),
    Owned(Arc<[u8]>),
}

impl std::ops::Deref for Bytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Bytes::Static(b) => b,
            Bytes::Owned(b) => b,
        }
    }
}

/// A font program the book or PdfCraft supplies.
pub struct Program {
    pub bytes: Bytes,
    /// The family name the PDF font carries.
    pub family: String,
    data: ShaperData,
    upem: f64,
}

impl Program {
    fn new(bytes: Bytes, family: String) -> Option<Self> {
        pdfcraft_fonts::embedding::validate(&bytes).ok()?;
        let font = FontRef::new(&bytes).ok()?;
        let data = ShaperData::new(&font);
        let upem = f64::from(font.head().ok()?.units_per_em().max(1));
        Some(Program { bytes, family, data, upem })
    }

    fn has(&self, c: char) -> bool {
        FontRef::new(&self.bytes).is_ok_and(|f| f.charmap().map(c).is_some())
    }
}

/// A book face: the family its CSS names, and its style.
struct BookFace {
    family: String,
    bold: bool,
    italic: bool,
    program: usize,
}

/// One glyph of shaped text. Advances and offsets are in ems.
#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    pub face: FaceId,
    /// The glyph id (a program) or the WinAnsi code (a standard face).
    pub id: u32,
    /// The text this glyph shows (empty for the second glyph of a cluster).
    pub text: String,
    pub advance: f64,
    pub offset: [f64; 2],
}

/// The faces a book is set in.
pub struct Fonts {
    pub programs: Vec<Program>,
    book: Vec<BookFace>,
    /// Programs by what they were loaded from, so a face is loaded once.
    loaded: HashMap<String, usize>,
    /// Characters no face has, and how often.
    pub missing: HashMap<char, usize>,
    /// Whether the craft-fonts faces are loaded (on the first character Inter lacks).
    craft: bool,
    /// Faces already chosen, by character and style.
    chosen: HashMap<(char, Vec<String>, bool, bool), Option<FaceId>>,
}

fn std_for(generic: &str, bold: bool, italic: bool) -> Option<Std14> {
    let base = match generic {
        "serif" | "times" | "times new roman" | "georgia" | "cambria" | "palatino" | "book antiqua" | "garamond" => 0,
        "sans-serif" | "helvetica" | "arial" | "verdana" | "tahoma" | "calibri" | "segoe ui" | "system-ui" | "trebuchet ms" => 1,
        "monospace" | "courier" | "courier new" | "consolas" | "menlo" | "monaco" => 2,
        _ => return None,
    };
    Some(match (base, bold, italic) {
        (0, false, false) => Std14::TimesRoman,
        (0, true, false) => Std14::TimesBold,
        (0, false, true) => Std14::TimesItalic,
        (0, true, true) => Std14::TimesBoldItalic,
        (1, false, false) => Std14::Helvetica,
        (1, true, false) => Std14::HelveticaBold,
        (1, false, true) => Std14::HelveticaOblique,
        (1, true, true) => Std14::HelveticaBoldOblique,
        (_, false, false) => Std14::Courier,
        (_, true, false) => Std14::CourierBold,
        (_, false, true) => Std14::CourierOblique,
        (_, true, true) => Std14::CourierBoldOblique,
    })
}

impl Fonts {
    pub fn new() -> Self {
        let mut f =
            Fonts { programs: Vec::new(), book: Vec::new(), loaded: HashMap::new(), missing: HashMap::new(), craft: false, chosen: HashMap::new() };
        f.load("pdfcraft:inter", Bytes::Static(pdfcraft_fonts::INTER_REGULAR), "Inter");
        f
    }

    fn load(&mut self, key: &str, bytes: Bytes, family: &str) -> Option<usize> {
        if let Some(i) = self.loaded.get(key) {
            return Some(*i);
        }
        let p = Program::new(bytes, family.to_string())?;
        self.programs.push(p);
        let i = self.programs.len() - 1;
        self.loaded.insert(key.to_string(), i);
        Some(i)
    }

    /// Add a book font (`@font-face`) for `family`. False when the file can't be embedded (WOFF,
    /// a collection, a variable font, embedding forbidden by its licence bits).
    pub fn add_book(&mut self, path: &str, bytes: Vec<u8>, family: &str, bold: bool, italic: bool) -> bool {
        let Some(program) = self.load(&format!("book:{path}"), Bytes::Owned(Arc::from(bytes)), family) else { return false };
        self.book.push(BookFace { family: family.to_ascii_lowercase(), bold, italic, program });
        self.chosen.clear();
        true
    }

    /// The face `c` is set in for `style`; `None` when no face has it.
    pub fn face(&mut self, c: char, style: &RunStyle) -> Option<FaceId> {
        let key = (c, style.families.clone(), style.bold, style.italic);
        if let Some(f) = self.chosen.get(&key) {
            return *f;
        }
        let mut face = self.pick(c, style);
        if face.is_none() && !self.craft {
            self.craft = true;
            for f in pdfcraft_fonts::CRAFT_FONTS {
                self.load(&format!("craft:{}", f.name()), Bytes::Static(f.bytes), &f.name());
            }
            face = self.pick(c, style);
        }
        self.chosen.insert(key, face);
        face
    }

    fn pick(&self, c: char, style: &RunStyle) -> Option<FaceId> {
        let encodable = pdfcraft_fonts::win_ansi_byte(c).is_some();
        for family in &style.families {
            let mut faces: Vec<&BookFace> = self.book.iter().filter(|f| f.family == *family).collect();
            // The exact style first, then the family's other faces.
            faces.sort_by_key(|f| (f.bold != style.bold) as u8 + (f.italic != style.italic) as u8);
            if let Some(f) = faces.iter().find(|f| self.programs[f.program].has(c)) {
                return Some(FaceId::Program(f.program));
            }
            if encodable && let Some(s) = std_for(family, style.bold, style.italic) {
                return Some(FaceId::Std(s));
            }
        }
        if encodable {
            return std_for("serif", style.bold, style.italic).map(FaceId::Std);
        }
        // Inter, then craft-fonts, in load order.
        self.programs
            .iter()
            .enumerate()
            .filter(|(i, _)| !self.book.iter().any(|b| b.program == *i))
            .find(|(_, p)| p.has(c))
            .map(|(i, _)| FaceId::Program(i))
    }

    /// Shape `text` (one face, one direction) into glyphs.
    pub fn shape(&self, face: FaceId, text: &str, rtl: bool) -> Vec<Glyph> {
        match face {
            FaceId::Std(s) => text
                .chars()
                .filter_map(|c| {
                    let code = pdfcraft_fonts::win_ansi_byte(if c == '\u{a0}' { ' ' } else { c })?;
                    Some(Glyph { face, id: u32::from(code), text: c.to_string(), advance: s.width(code) / 1000.0, offset: [0.0; 2] })
                })
                .collect(),
            FaceId::Program(i) => {
                let p = &self.programs[i];
                let Ok(font) = FontRef::new(&p.bytes) else { return Vec::new() };
                let shaper = p.data.shaper(&font).build();
                let mut buffer = UnicodeBuffer::new();
                buffer.push_str(text);
                buffer.set_cluster_level(BufferClusterLevel::MonotoneGraphemes);
                buffer.set_direction(if rtl { Direction::RightToLeft } else { Direction::LeftToRight });
                buffer.guess_segment_properties();
                let out = shaper.shape(buffer, ShapeOptions::new());
                let mut starts: Vec<usize> = out.glyph_infos().iter().map(|g| g.cluster as usize).collect();
                starts.sort_unstable();
                starts.dedup();
                let mut seen = std::collections::HashSet::new();
                out.glyph_infos()
                    .iter()
                    .zip(out.glyph_positions())
                    .filter(|(g, _)| g.glyph_id != 0)
                    .map(|(g, pos)| {
                        let start = g.cluster as usize;
                        let end = starts.get(starts.partition_point(|s| *s <= start)).copied().unwrap_or(text.len());
                        let t = if seen.insert(start) { text.get(start..end).unwrap_or_default().to_string() } else { String::new() };
                        Glyph {
                            face,
                            id: g.glyph_id,
                            text: t,
                            advance: f64::from(pos.x_advance) / p.upem,
                            offset: [f64::from(pos.x_offset) / p.upem, f64::from(pos.y_offset) / p.upem],
                        }
                    })
                    .collect()
            }
        }
    }

    /// The advance the PDF font itself gives glyph `id` of `face`, in ems (the content stream
    /// corrects for the difference from the shaped advance).
    pub fn font_advance(&self, face: FaceId, id: u32) -> f64 {
        match face {
            FaceId::Std(s) => u8::try_from(id).map_or(0.0, |code| s.width(code) / 1000.0),
            FaceId::Program(i) => {
                let p = &self.programs[i];
                FontRef::new(&p.bytes)
                    .ok()
                    .and_then(|f| {
                        let m = f.glyph_metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default());
                        m.advance_width(skrifa::GlyphId::new(id)).map(|w| f64::from(w) / p.upem)
                    })
                    .unwrap_or(0.0)
            }
        }
    }

    /// Note a character no face has.
    pub fn miss(&mut self, c: char) {
        *self.missing.entry(c).or_default() += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(families: &[&str], bold: bool) -> RunStyle {
        RunStyle {
            size: 12.0,
            bold,
            italic: false,
            families: families.iter().map(|f| f.to_string()).collect(),
            color: [0.0; 3],
            underline: false,
            strike: false,
            valign: super::super::dom::VAlign::Baseline,
            small_caps: false,
        }
    }

    #[test]
    fn faces_follow_the_families_then_fall_back() {
        let mut fonts = Fonts::new();
        assert_eq!(fonts.face('a', &style(&["sans-serif"], true)), Some(FaceId::Std(Std14::HelveticaBold)));
        assert_eq!(fonts.face('a', &style(&["unknown", "monospace"], false)), Some(FaceId::Std(Std14::Courier)));
        assert_eq!(fonts.face('é', &style(&["unknown"], false)), Some(FaceId::Std(Std14::TimesRoman)));
        // Cyrillic is not in WinAnsi: Inter has it.
        assert_eq!(fonts.face('Ж', &style(&["serif"], false)), Some(FaceId::Program(0)));
        assert_eq!(fonts.programs[0].family, "Inter");
    }

    #[test]
    fn shaping_measures_both_kinds_of_face() {
        let fonts = Fonts::new();
        let std = fonts.shape(FaceId::Std(Std14::Helvetica), "Hi", false);
        assert_eq!(std.iter().map(|g| g.id).collect::<Vec<_>>(), [72, 105]);
        assert!((std.iter().map(|g| g.advance).sum::<f64>() - (0.722 + 0.222)).abs() < 1e-9);
        let inter = fonts.shape(FaceId::Program(0), "Жук", false);
        assert_eq!(inter.len(), 3);
        assert_eq!(inter.iter().map(|g| g.text.as_str()).collect::<String>(), "Жук");
        assert!(inter.iter().all(|g| g.advance > 0.2 && g.advance < 1.2));
        assert!((fonts.font_advance(FaceId::Program(0), inter[0].id) - inter[0].advance).abs() < 0.05);
    }
}
