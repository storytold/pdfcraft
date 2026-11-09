//! PdfCraft patch (71): bounded, display-only fallback for missing simple-font glyphs.
use super::blob::OpenTypeFontBlob;
use super::generated::glyph_names;
use super::{FontData, FontQuery, StandardFont, stretch_glyph, unicode_from_name};
use crate::FontResolverFn;
use hayro_cmap::{BfString, CMap};
use kurbo::{Affine, BezPath};
use skrifa::{GlyphId, MetadataProvider, attribute::Style};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

// Separate from the existing composite-letter IDs and the real font's glyph IDs.
const FIRST_GLYPH: u32 = 0x0200_0000;
const MAX_FACES: usize = 8;
const MAX_OUTLINE_ELEMENTS: usize = 16_384;

#[derive(Clone)]
struct FallbackGlyph {
    path: BezPath,
    advance: f32,
}

pub(super) struct SimpleGlyphFallback {
    resolver: FontResolverFn,
    standard: StandardFont,
    // At most 256 entries: the PDF font is single-byte. Cache failed mappings too.
    paths: RefCell<HashMap<u8, Option<FallbackGlyph>>>,
    // Retain bytes for pointer identity; parse each returned face (including failures) once.
    faces: RefCell<Vec<(FontData, u32, Option<OpenTypeFontBlob>)>>,
}

impl fmt::Debug for SimpleGlyphFallback {
    /// Format the selected standard family without formatting resolver state, font bytes,
    /// or cached outlines.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SimpleGlyphFallback")
            .field("standard", &self.standard)
            .finish_non_exhaustive()
    }
}

impl SimpleGlyphFallback {
    /// Create empty per-code and per-face caches for one non-embedded simple font.
    /// `standard` supplies the requested family/style; `resolver` is called lazily for printable
    /// missing characters, not while constructing the cache.
    pub(super) fn new(resolver: FontResolverFn, standard: StandardFont) -> Self {
        Self {
            resolver,
            standard,
            paths: RefCell::new(HashMap::new()),
            faces: RefCell::new(Vec::new()),
        }
    }

    /// Resolve and cache a display-only virtual glyph for this byte, including failed mappings.
    /// Encoding `name` takes precedence over `unicode`; multi-scalar text gets a box rather than
    /// a guessed first character. Invisible or undecodable input returns `None`.
    ///
    /// `width` is the advance already selected by the caller, including an explicit zero.
    /// Narrow a usable alternate outline to a smaller width, but never widen its design;
    /// the caller still uses the full PDF advance for text placement. Without a selected
    /// width, use the alternate's natural advance, or 600 units for a missing box.
    /// Cache at most 256 codes and eight parsed faces.
    /// Reject empty, oversized or non-finite alternate outlines; keep a finite missing box instead.
    /// The cached natural advance is separate from the caller's explicit PDF width.
    pub(super) fn map_code(
        &self,
        code: u8,
        name: Option<&str>,
        unicode: Option<&CMap>,
        width: Option<f32>,
    ) -> Option<GlyphId> {
        let id = GlyphId::new(FIRST_GLYPH + u32::from(code));
        if let Some(path) = self.paths.borrow().get(&code) {
            return path.as_ref().map(|_| id);
        }
        // Encoding names describe the drawn glyph. ToUnicode is a last resort only:
        // multi-character semantic/ligature replacements are never guessed into one glyph.
        let decoded = decode(code, name, unicode);
        let character = match decoded {
            Some(Decoded::Character(c)) => Some(c),
            _ => None,
        };
        let printable_sequence = matches!(decoded, Some(Decoded::Sequence(true)));
        let path = character.filter(|c| !invisible(*c)).map(|character| {
            let query = FontQuery::MissingGlyph {
                character,
                standard_font: self.standard,
            };
            let resolved = (self.resolver)(&query).and_then(|(data, index)| self.face(data, index));
            let mut advance = 600.0;
            let mut path = resolved
                .and_then(|face| {
                    let glyph = face.font_ref().charmap().map(character)?;
                    if glyph == GlyphId::NOTDEF {
                        return None;
                    }
                    let path = face.outline_glyph(glyph);
                    if path.elements().is_empty() || path.elements().len() > MAX_OUTLINE_ELEMENTS {
                        return None;
                    }
                    let actual = face
                        .glyph_metrics()
                        .advance_width(glyph)
                        .filter(|w| w.is_finite() && *w >= 0.0);
                    advance = actual.unwrap_or(600.0);
                    let mut path = fit_width(path, width, actual);
                    if self.standard.is_italic()
                        && face.font_ref().attributes().style == Style::Normal
                    {
                        // About 12 degrees; keep the baseline fixed. No synthesized bold.
                        path = Affine::new([1.0, 0.0, 0.2126, 1.0, 0.0, 0.0]) * path;
                    }
                    Some(path)
                })
                .unwrap_or_else(|| missing_box(width));
            // Keep only finite paths, including when PDF widths are NaN/infinite.
            if !path.elements().iter().all(|e| match e {
                kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) => p.is_finite(),
                kurbo::PathEl::QuadTo(a, b) => a.is_finite() && b.is_finite(),
                kurbo::PathEl::CurveTo(a, b, c) => a.is_finite() && b.is_finite() && c.is_finite(),
                kurbo::PathEl::ClosePath => true,
            }) {
                path = missing_box(None);
                advance = 600.0;
            }
            FallbackGlyph { path, advance }
        });
        let path = path.or_else(|| {
            printable_sequence.then(|| FallbackGlyph {
                path: missing_box(width),
                advance: 600.0,
            })
        });
        let present = path.is_some();
        self.paths.borrow_mut().insert(code, path);
        present.then_some(id)
    }

    /// Parse a resolver-supplied face at `index`, caching success or failure by Arc identity
    /// and face index. Return `None` for invalid data or after eight distinct face entries;
    /// retained byte Arcs keep identity stable for subsequent lookups.
    fn face(&self, data: FontData, index: u32) -> Option<OpenTypeFontBlob> {
        let mut faces = self.faces.borrow_mut();
        if let Some((_, _, face)) = faces
            .iter()
            .find(|(bytes, i, _)| *i == index && Arc::ptr_eq(bytes, &data))
        {
            return face.clone();
        }
        if faces.len() >= MAX_FACES {
            return None;
        }
        let face = OpenTypeFontBlob::new(data.clone(), index);
        faces.push((data, index, face.clone()));
        face
    }

    /// Clone the cached path for a virtual fallback ID. Return `None` for a non-fallback ID,
    /// an unmapped byte or a cached failure; callers may then try their primary/composite path.
    pub(super) fn outline(&self, glyph: GlyphId) -> Option<BezPath> {
        let code = u8::try_from(glyph.to_u32().checked_sub(FIRST_GLYPH)?).ok()?;
        self.paths
            .borrow()
            .get(&code)?
            .as_ref()
            .map(|g| g.path.clone())
    }

    /// Return the cached natural advance in 1000-unit glyph space, including a real zero advance
    /// for combining marks or the missing box's 600-unit default. Return `None` before a successful
    /// `map_code` call or for a cached failure. Callers use this only after PDF and standard metrics.
    pub(super) fn advance(&self, code: u8) -> Option<f32> {
        self.paths.borrow().get(&code)?.as_ref().map(|g| g.advance)
    }
}

/// Encoding names describe appearance; ToUnicode is used only for an unknown name.
#[derive(Clone, Copy)]
pub(super) enum Decoded {
    Character(char),
    Sequence(bool),
}

/// Decode the appearance named by the Encoding/Differences entry, dropping its glyph suffix.
/// Use the AGL or a Unicode glyph name first; only an unknown name consults the byte's ToUnicode
/// entry. Return `None` for an absent/empty mapping, preserving semantic sequences as sequences.
pub(super) fn decode(code: u8, name: Option<&str>, unicode: Option<&CMap>) -> Option<Decoded> {
    if let Some(name) = name.and_then(|n| n.split('.').next()) {
        if let Some(s) = glyph_names::get(name) {
            return decode_string(s);
        }
        if let Some(c) = unicode_from_name(name) {
            return Some(Decoded::Character(c));
        }
    }
    match unicode?.lookup_bf_string(u32::from(code))? {
        BfString::Char(c) => Some(Decoded::Character(c)),
        BfString::String(s) => decode_string(&s),
    }
}

/// Classify non-empty mapped text as one scalar or a sequence. The sequence flag records
/// whether any scalar needs visible ink; an empty string has no mapping.
fn decode_string(s: &str) -> Option<Decoded> {
    let mut chars = s.chars();
    let first = chars.next()?;
    Some(if chars.next().is_none() {
        Decoded::Character(first)
    } else {
        Decoded::Sequence(s.chars().any(|c| !invisible(c)))
    })
}

/// Narrow an outline to a smaller target advance, never widen it to an oversized PDF width.
/// Both advances must be finite and the source positive; otherwise leave the path unchanged.
/// A finite target zero remains a valid fit. Text placement keeps the caller's full advance.
fn fit_width(path: BezPath, width: Option<f32>, actual: Option<f32>) -> BezPath {
    match width.zip(actual) {
        Some((width, actual)) if width.is_finite() && actual.is_finite() && actual > 0.0 => {
            stretch_glyph(path, width.min(actual), actual)
        }
        _ => path,
    }
}

/// Whitespace, controls and default-ignorable format/variation characters need no visible box.
fn invisible(c: char) -> bool {
    c.is_whitespace()
        || c.is_control()
        || matches!(c,
        '\u{00ad}' | '\u{034f}' | '\u{061c}' | '\u{115f}'..='\u{1160}' |
        '\u{17b4}'..='\u{17b5}' | '\u{180b}'..='\u{180f}' | '\u{200b}'..='\u{200f}' |
        '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{3164}' |
        '\u{fe00}'..='\u{fe0f}' | '\u{feff}' | '\u{ffa0}' | '\u{fff0}'..='\u{fff8}' |
        '\u{1bca0}'..='\u{1bca3}' | '\u{1d173}'..='\u{1d17a}' | '\u{e0000}'..='\u{e0fff}')
}

/// Build the contributor-original missing-glyph box, narrowing only to a positive supplied
/// width below its 600-unit design advance. Otherwise keep its original shape; this path
/// does not select the text advance.
fn missing_box(width: Option<f32>) -> BezPath {
    let mut path = BezPath::new();
    path.move_to((70.0, 0.0));
    path.line_to((530.0, 0.0));
    path.line_to((530.0, 700.0));
    path.line_to((70.0, 700.0));
    path.close_path();
    path.move_to((120.0, 50.0));
    path.line_to((120.0, 650.0));
    path.line_to((480.0, 650.0));
    path.line_to((480.0, 50.0));
    path.close_path();
    fit_width(path, width.filter(|w| *w > 0.0), Some(600.0))
}
