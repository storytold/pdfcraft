//! Bounded Latin text plans which preserve the caller's Unicode spelling.
//!
//! Only base-plus-mark clusters need OpenType shaping. Unmarked characters keep their
//! original cmap advances, so enabling NFD does not change existing NFC text layout.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use pdfcraft_cos::{Dict, Document};
use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, GlyphId, MetadataProvider as _};

/// The existing source-shaping and CID mapping budgets, applied before font resources change.
pub const MAX_ROW_BYTES: usize = 262_144;
/// A base plus at most 32 combining marks prevents adversarial shaping work per cluster.
pub const MAX_CLUSTER_SCALARS: usize = 33;
const MAX_GLYPHS: usize = 65_534;

/// A positioned glyph in em units. The first glyph alone owns its original logical cluster.
#[derive(Clone, Debug, PartialEq)]
pub struct LatinGlyph {
    pub id: GlyphId,
    pub position: [f64; 2],
    pub text: String,
    /// Number of logical clusters preceding this glyph, including for zero-advance marks.
    pub cluster_index: usize,
    pub spaces_before: usize,
}

/// One row in the licensed bundled Inter face; no normalization is applied to `text`.
#[derive(Clone, Debug, PartialEq)]
pub struct LatinTextPlan {
    pub text: String,
    pub glyphs: Vec<LatinGlyph>,
    pub advance: f64,
    /// Ink bounds `[left, bottom, right, top]` in em, relative to the baseline.
    pub bounds: [f64; 4],
    pub cluster_count: usize,
    bold: bool,
}

impl LatinTextPlan {
    fn validate(&self) -> Result<(), String> {
        if self.text.len() > MAX_ROW_BYTES
            || self.text.contains(['\r', '\n'])
            || self.glyphs.len() > MAX_GLYPHS
            || self.cluster_count != clusters(&self.text).count()
            || !self.advance.is_finite()
            || self.bounds.iter().any(|n| !n.is_finite())
        {
            return Err("the Latin text plan exceeds its budget or geometry".into());
        }
        let mut offset = 0usize;
        for glyph in &self.glyphs {
            let end = offset.checked_add(glyph.text.len()).ok_or("the Latin cluster exceeds its text budget")?;
            if glyph.id.to_u32() == 0
                || glyph.position.iter().any(|n| !n.is_finite())
                || glyph.cluster_index >= self.cluster_count
                || glyph.spaces_before > self.cluster_count
                || self.text.get(offset..end) != Some(glyph.text.as_str())
            {
                return Err("the Latin glyph does not match its logical text or position".into());
            }
            offset = end;
        }
        if offset != self.text.len() {
            return Err("the Latin plan does not retain its complete logical text".into());
        }
        Ok(())
    }
}

/// A base character and every following combining mark form one indivisible cluster.
/// This deliberately groups all combining-mark blocks even when coverage rejects a mark.
pub fn clusters(text: &str) -> impl Iterator<Item = &str> {
    let mut remaining = text;
    std::iter::from_fn(move || {
        if remaining.is_empty() {
            return None;
        }
        let end = remaining.char_indices().skip(1).find(|(_, c)| !is_mark(*c)).map_or(remaining.len(), |(i, _)| i);
        let (cluster, rest) = remaining.split_at(end);
        remaining = rest;
        Some(cluster)
    })
}

fn is_mark(c: char) -> bool {
    matches!(u32::from(c), 0x0300..=0x036f | 0x1ab0..=0x1aff | 0x1dc0..=0x1dff | 0x20d0..=0x20ff | 0xfe20..=0xfe2f)
}

/// Whether this row needs the cluster-aware writer. The legacy scalar OCR writer stays guarded.
pub fn required(text: &str) -> bool {
    text.chars().any(is_mark)
}

fn latin_base(c: char) -> bool {
    c.is_alphabetic() && matches!(u32::from(c), 0x0041..=0x007a | 0x00c0..=0x024f | 0x1e00..=0x1eff)
}

fn face_bytes(bold: bool) -> Result<&'static [u8], String> {
    crate::document_latin_fonts(bold).first().map(|f| f.bytes).ok_or_else(|| "the bundled Latin document font is unavailable".into())
}

/// Coverage for consumers which actually use this module's shaped writer.
/// Line breaks are layout separators; tabs retain their legacy space appearance. Other controls, orphan marks, absent glyphs and
/// marks outside U+0300..U+036F remain unsupported. No scalar-only caller should use this.
pub fn first_unsupported(text: &str, bold: bool) -> Option<char> {
    let bytes = match face_bytes(bold) {
        Ok(bytes) => bytes,
        Err(_) => return text.chars().find(|c| !matches!(c, '\r' | '\n')),
    };
    let face = match FontRef::new(bytes) {
        Ok(face) => face,
        Err(_) => return text.chars().find(|c| !matches!(c, '\r' | '\n')),
    };
    for cluster in clusters(text) {
        let Some(base) = cluster.chars().next() else { continue };
        if matches!(base, '\r' | '\n') && cluster.len() == 1 {
            continue;
        }
        if !required(cluster) && crate::win_ansi_encodable(base) {
            continue;
        }
        if base.is_control() || is_mark(base) {
            return Some(base);
        }
        if required(cluster) {
            if !latin_base(base) || cluster.chars().count() > MAX_CLUSTER_SCALARS {
                return cluster.chars().nth(1).or(Some(base));
            }
            if let Some(mark) = cluster.chars().skip(1).find(|c| !matches!(u32::from(*c), 0x0300..=0x036f)) {
                return Some(mark);
            }
            if crate::source_shaping::shape(bytes, cluster).is_err() {
                return cluster.chars().nth(1).or(Some(base));
            }
        } else if face.charmap().map(base).is_none_or(|g| g.to_u32() == 0) {
            return Some(base);
        }
    }
    None
}

/// Preflight a complete row before embedding anything. Mark clusters are shaped as a unit;
/// surrounding NFC characters retain their old cmap glyphs and advances (no new kerning).
pub fn plan(text: &str, bold: bool) -> Result<LatinTextPlan, String> {
    if text.len() > MAX_ROW_BYTES || text.contains(['\r', '\n']) {
        return Err("plan one bounded Latin text row at a time".into());
    }
    if let Some(c) = first_unsupported(text, bold) {
        return Err(format!("the bundled Latin face cannot shape U+{:04X} in this cluster", u32::from(c)));
    }
    let bytes = face_bytes(bold)?;
    let font = FontRef::new(bytes).map_err(|e| e.to_string())?;
    let metrics = font.glyph_metrics(Size::unscaled(), LocationRef::default());
    let scale = 1.0 / f64::from(font.metrics(Size::unscaled(), LocationRef::default()).units_per_em.max(1));
    let mut result = LatinTextPlan { text: text.into(), glyphs: Vec::new(), advance: 0.0, bounds: [0.0; 4], cluster_count: 0, bold };
    let mut spaces = 0usize;
    for cluster in clusters(text) {
        let mut shaped = if required(cluster) {
            crate::source_shaping::shape(bytes, cluster)?
        } else {
            let c = cluster.chars().next().ok_or("empty Latin cluster")?;
            let shown = if c == '\t' { ' ' } else { c };
            let id = font.charmap().map(shown).filter(|g| g.to_u32() != 0).ok_or("the Latin face lacks a glyph")?;
            let advance = f64::from(metrics.advance_width(id).ok_or("the Latin glyph has no advance")?) * scale;
            crate::source_shaping::ShapedRun {
                glyphs: vec![crate::source_shaping::ShapedGlyph { id, position: [0.0, 0.0], text: cluster.into() }],
                advance,
            }
        };
        if result.glyphs.len().saturating_add(shaped.glyphs.len()) > MAX_GLYPHS {
            return Err("the Latin row exceeds its glyph budget".into());
        }
        for glyph in &mut shaped.glyphs {
            glyph.position[0] += result.advance;
            if let Some(b) = metrics.bounds(glyph.id) {
                let [x, y] = glyph.position;
                result.bounds[0] = result.bounds[0].min(x + f64::from(b.x_min) * scale);
                result.bounds[1] = result.bounds[1].min(y + f64::from(b.y_min) * scale);
                result.bounds[2] = result.bounds[2].max(x + f64::from(b.x_max) * scale);
                result.bounds[3] = result.bounds[3].max(y + f64::from(b.y_max) * scale);
            }
            result.glyphs.push(LatinGlyph {
                id: glyph.id,
                position: glyph.position,
                text: std::mem::take(&mut glyph.text),
                cluster_index: result.cluster_count,
                spaces_before: spaces,
            });
        }
        result.advance += shaped.advance;
        result.cluster_count += 1;
        spaces += cluster.matches(' ').count();
    }
    if !result.advance.is_finite() || result.bounds.iter().any(|v| !v.is_finite()) {
        return Err("the Latin row exceeds its geometry budget".into());
    }
    Ok(result)
}

/// A deduplicated logical-cluster/glyph map shared by every prepared row in an appearance.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddedLatinText {
    pub name: String,
    codes: BTreeMap<(String, u32), u16>,
    bold: bool,
}

/// Embed all preflighted plans in one CID resource. Different mappings still share the full
/// font program through `embed_mapping`; existing resource names are never overwritten.
pub fn embed(doc: &mut Document, fonts: &mut Dict, plans: &[LatinTextPlan], bold: bool) -> Result<EmbeddedLatinText, String> {
    let mut codes = BTreeMap::new();
    let mut mappings = Vec::new();
    for row in plans {
        row.validate()?;
        if row.bold != bold {
            return Err("the Latin text plan does not match its font or budget".into());
        }
        for glyph in &row.glyphs {
            let key = (glyph.text.clone(), glyph.id.to_u32());
            if let std::collections::btree_map::Entry::Vacant(entry) = codes.entry(key) {
                let code = u16::try_from(mappings.len().saturating_add(1)).map_err(|_| "the Latin mapping exceeds its glyph budget")?;
                if usize::from(code) > MAX_GLYPHS {
                    return Err("the Latin mapping exceeds its glyph budget".into());
                }
                entry.insert(code);
                mappings.push((glyph.text.clone(), glyph.id));
            }
        }
    }
    let bytes = face_bytes(bold)?;
    let (_, family) = crate::embedding::names(bytes)?;
    let name = crate::embedding::embed_mapping(doc, fonts, bytes, &family, &mappings)?;
    Ok(EmbeddedLatinText { name, codes, bold })
}

impl EmbeddedLatinText {
    /// Write inside the caller's BT/ET. Positions include cluster-based character/word
    /// spacing, so PDF Tc/Tw/Ts are reset. The caller restates them before its next legacy run.
    /// `shear` slants the complete glyph cluster, including the offsets of its marks.
    #[allow(clippy::too_many_arguments)]
    pub fn write(
        &self,
        out: &mut Vec<u8>,
        plan: &LatinTextPlan,
        size: f64,
        at: [f64; 2],
        shear: f64,
        char_spacing: f64,
        word_spacing: f64,
    ) -> Result<f64, String> {
        plan.validate()?;
        let advance = plan.advance * size + plan.cluster_count as f64 * char_spacing + plan.text.matches(' ').count() as f64 * word_spacing;
        if plan.bold != self.bold
            || size <= 0.0
            || !advance.is_finite()
            || [size, at[0], at[1], shear, char_spacing, word_spacing].iter().any(|n| !n.is_finite())
        {
            return Err("the Latin text has invalid placement or spacing".into());
        }
        let mut buffer = String::new();
        buffer.push_str("/Span << /ActualText <FEFF");
        for unit in plan.text.encode_utf16() {
            let _ = write!(buffer, "{unit:04X}");
        }
        let _ = writeln!(buffer, "> >> BDC /{} {} Tf 0 Tc 0 Tw 0 Ts", self.name, number(size));
        for glyph in &plan.glyphs {
            let code = self.codes.get(&(glyph.text.clone(), glyph.id.to_u32())).ok_or("the Latin glyph has no prepared mapping")?;
            let x = at[0]
                + (glyph.position[0] + shear * glyph.position[1]) * size
                + glyph.cluster_index as f64 * char_spacing
                + glyph.spaces_before as f64 * word_spacing;
            let y = at[1] + glyph.position[1] * size;
            if !x.is_finite() || !y.is_finite() {
                return Err("the Latin glyph has invalid placement".into());
            }
            let _ = writeln!(buffer, "1 0 {} 1 {} {} Tm <{code:04X}> Tj", number(shear), number(x), number(y));
        }
        buffer.push_str("0 Tc 0 Tw 0 Ts EMC\n");
        out.extend_from_slice(buffer.as_bytes());
        Ok(advance)
    }
}

fn number(value: f64) -> String {
    let formatted = format!("{value:.8}");
    formatted.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_cos::Object;

    #[test]
    fn nfd_plans_keep_original_clusters_and_canonical_inter_geometry() {
        for bold in [false, true] {
            let nfd = plan("g\u{306} I\u{307} s\u{327} u\u{308} a\u{302}", bold).unwrap();
            let nfc = plan("ğ İ ş ü â", bold).unwrap();
            assert_eq!(nfd.text, "g\u{306} I\u{307} s\u{327} u\u{308} a\u{302}");
            assert_eq!(nfd.glyphs.iter().map(|g| g.text.as_str()).collect::<String>(), nfd.text);
            assert_eq!(nfd.cluster_count, nfc.cluster_count);
            assert!((nfd.advance - nfc.advance).abs() < 1e-8);
            assert_eq!(nfd.bounds, nfc.bounds);
            for (a, b) in nfd.glyphs.iter().zip(&nfc.glyphs) {
                assert_eq!((a.id, a.position), (b.id, b.position));
            }
            assert!(crate::first_non_win_ansi_or_latin(&nfd.text, bold).is_some(), "scalar OCR guard is intentionally unchanged");
            assert_eq!(first_unsupported(&nfd.text, bold), None);
        }
    }

    #[test]
    fn incomposable_marks_have_shaper_positions_but_one_logical_spacing_unit() {
        let text = "q\u{301}x\u{323}n\u{303}\u{304}";
        let row = plan(text, false).unwrap();
        assert_eq!(row.cluster_count, 3);
        assert!(row.glyphs.len() > row.cluster_count, "test really exercises multiple glyphs in a cluster");
        let bytes = face_bytes(false).unwrap();
        let mut pen = 0.0;
        let mut actual = row.glyphs.iter();
        for (index, cluster) in clusters(text).enumerate() {
            let shaped = crate::source_shaping::shape(bytes, cluster).unwrap();
            for expected in shaped.glyphs {
                let got = actual.next().unwrap();
                assert_eq!(got.id, expected.id);
                assert_eq!(got.position, [pen + expected.position[0], expected.position[1]]);
                assert_eq!(got.cluster_index, index);
            }
            pen += shaped.advance;
        }
        assert_eq!(row.advance, pen);
        assert_eq!(row.glyphs.iter().map(|g| g.text.as_str()).collect::<String>(), text);
    }

    #[test]
    fn mapping_and_actual_text_retain_nfd_while_program_is_shared() {
        let row = plan("g\u{306} q\u{301}", false).unwrap();
        let mut doc = Document::new_empty();
        let mut fonts = Dict::new();
        fonts.set(b"PCFull0".to_vec(), Object::name("KeepForeignFont"));
        let embedded = embed(&mut doc, &mut fonts, std::slice::from_ref(&row), false).unwrap();
        assert_ne!(embedded.name, "PCFull0");
        assert_eq!(fonts.get(b"PCFull0").and_then(Object::as_name), Some(b"KeepForeignFont".as_slice()));
        let font = doc.dict(fonts.get(embedded.name.as_bytes()).unwrap()).unwrap();
        let cmap = match doc.resolve(font.get(b"ToUnicode").unwrap()).as_ref() {
            Object::Stream(stream) => String::from_utf8(stream.decoded().unwrap()).unwrap(),
            _ => panic!("ToUnicode is not a stream"),
        };
        assert!(cmap.contains("00670306"), "the Unicode map must not contain normalized g-breve instead");
        let mut out = Vec::new();
        let width = embedded.write(&mut out, &row, 12.0, [4.0, 5.0], 0.21, 2.0, 3.0).unwrap();
        assert_eq!(width, row.advance * 12.0 + 3.0 * 2.0 + 3.0);
        let content = String::from_utf8(out).unwrap();
        assert!(content.contains("/ActualText <FEFF00670306002000710301>"));
        let count = doc.object_numbers().len();
        assert_eq!(embed(&mut doc, &mut fonts, &[row], false).unwrap(), embedded);
        assert_eq!(doc.object_numbers().len(), count);
        assert_eq!(doc.scan_objects().filter(|(_, o)| matches!(o.as_ref(), Object::Stream(s) if s.dict.get(b"PCFullFont").is_some())).count(), 1);
    }

    #[test]
    fn orphan_marks_unsupported_blocks_missing_glyphs_and_budgets_are_errors() {
        for text in ["\u{306}g", "g\u{1ab0}", "a\u{fe20}", "🦀", "g\u{0000}", "g\n"] {
            assert!(plan(text, false).is_err(), "{text:?}");
        }
        let chain = format!("a{}", "\u{306}".repeat(MAX_CLUSTER_SCALARS));
        assert!(plan(&chain, false).is_err());
        assert!(plan(&"a".repeat(MAX_ROW_BYTES + 1), false).is_err());
        assert!(plan(&"a".repeat(MAX_GLYPHS + 1), false).is_err());
        assert_eq!(plan("", false).unwrap().advance, 0.0);
        let mut doc = Document::new_empty();
        let mut fonts = Dict::new();
        let mut broken = plan("g\u{306}", false).unwrap();
        broken.glyphs[0].id = GlyphId::new(0);
        let before = doc.object_numbers().len();
        assert!(embed(&mut doc, &mut fonts, &[broken], false).is_err());
        assert_eq!(doc.object_numbers().len(), before, "bad second-row/map must be refused before any PDF object changes");
        assert_eq!(fonts.iter().count(), 0);
    }

    #[test]
    fn writing_an_unprepared_plan_is_atomic() {
        let row = plan("g\u{306}", false).unwrap();
        let other = plan("q\u{301}", false).unwrap();
        let mut doc = Document::new_empty();
        let mut fonts = Dict::new();
        let embedded = embed(&mut doc, &mut fonts, &[row], false).unwrap();
        let mut out = b"previous work".to_vec();
        assert!(embedded.write(&mut out, &other, 12.0, [0.0, 0.0], 0.0, 0.0, 0.0).is_err());
        assert_eq!(out, b"previous work");
    }
}
