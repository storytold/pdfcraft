//! Typed signatures and bounded glyph outlines from approved fonts (bundled, or craft-fonts).

use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};

static FONT: &[u8] = include_bytes!("../../../assets/fonts/DancingScript.ttf");

/// Inter Regular, the interface font. Typed signatures fall back to it, slanted, for characters
/// the script font lacks (Dancing Script is Latin and Vietnamese only: no Cyrillic or Greek).
pub static INTER_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");

/// Forward slant of fallback glyphs in a typed signature (about 12°), so they lean like the script.
const FALLBACK_SLANT: f64 = 0.21;

/// Bound signature work while allowing long personal names.
pub const MAX_SIGNATURE_CHARS: usize = 256;

/// Outlines of a line of existing text at a font size of 1 (em units).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptOutline {
    pub contours: Vec<Vec<[f64; 2]>>,
    pub width: f64,
    pub ascent: f64,
    pub descent: f64,
}

impl ScriptOutline {
    /// Layout bounds including glyph overhangs, so previews and PDF appearances never clip ink.
    pub fn bounds(&self) -> [f64; 4] {
        self.contours.iter().flatten().fold([0.0, self.descent, self.width, self.ascent], |mut b, p| {
            b[0] = b[0].min(p[0]);
            b[1] = b[1].min(p[1]);
            b[2] = b[2].max(p[0]);
            b[3] = b[3].max(p[1]);
            b
        })
    }
}

/// One glyph outline in em units, with the baseline at y = 0.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphOutline {
    pub contours: Vec<Vec<[f64; 2]>>,
    pub width: f64,
    pub bbox: [f64; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlyphError {
    /// This build has no Japanese face: it was built without craft-fonts (`CRAFT_FONTS_DIR`).
    NoFont,
    Missing,
    TooComplex,
}

struct Flatten {
    contours: Vec<Vec<[f64; 2]>>,
    cur: Vec<[f64; 2]>,
    scale: f64,
    /// Horizontal shear applied to every point (x += slant × y), for slanted fallback glyphs.
    slant: f64,
    dx: f64,
    dy: f64,
    points: usize,
    too_complex: bool,
    max_points: usize,
}

impl Flatten {
    fn new(scale: f64) -> Self {
        Self { contours: Vec::new(), cur: Vec::new(), scale, slant: 0.0, dx: 0.0, dy: 0.0, points: 0, too_complex: false, max_points: 4096 }
    }

    fn pt(&self, x: f32, y: f32) -> [f64; 2] {
        let (x, y) = (x as f64 * self.scale, y as f64 * self.scale);
        [self.dx + x + self.slant * y, self.dy + y]
    }

    fn last(&self) -> [f64; 2] {
        self.cur.last().copied().unwrap_or([self.dx, self.dy])
    }

    fn push(&mut self, p: [f64; 2]) {
        if self.points >= self.max_points {
            self.too_complex = true;
            return;
        }
        self.points += 1;
        self.cur.push(p);
    }

    fn close(&mut self) {
        if self.cur.len() > 2 {
            self.contours.push(std::mem::take(&mut self.cur));
        } else {
            self.cur.clear();
        }
    }
}

const STEPS: usize = 8;

impl OutlinePen for Flatten {
    fn move_to(&mut self, x: f32, y: f32) {
        self.close();
        self.push(self.pt(x, y));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.push(self.pt(x, y));
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let (p0, c, p) = (self.last(), self.pt(cx, cy), self.pt(x, y));
        for i in 1..=STEPS {
            let t = i as f64 / STEPS as f64;
            let u = 1.0 - t;
            self.push([u * u * p0[0] + 2.0 * u * t * c[0] + t * t * p[0], u * u * p0[1] + 2.0 * u * t * c[1] + t * t * p[1]]);
        }
    }

    fn curve_to(&mut self, c0x: f32, c0y: f32, c1x: f32, c1y: f32, x: f32, y: f32) {
        let (p0, c0, c1, p) = (self.last(), self.pt(c0x, c0y), self.pt(c1x, c1y), self.pt(x, y));
        for i in 1..=STEPS {
            let t = i as f64 / STEPS as f64;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            self.push([a * p0[0] + b * c0[0] + c * c1[0] + d * p[0], a * p0[1] + b * c0[1] + c * c1[1] + d * p[1]]);
        }
    }

    fn close(&mut self) {
        Flatten::close(self);
    }
}

/// Whether the face has a glyph for `ch` (a cheap character-map probe, no outline work).
pub fn face_has(bytes: &[u8], ch: char) -> bool {
    FontRef::new(bytes).is_ok_and(|font| font.charmap().map(ch).is_some())
}

/// Return one glyph of `face` (a craft-fonts face), bounded so hostile replacement text cannot
/// allocate unbounded outline data.
pub fn craft_glyph(face: &crate::CraftFont, ch: char) -> Result<GlyphOutline, GlyphError> {
    glyph_in_face(face.bytes, ch)
}

/// Return one glyph of the Japanese document face ([`crate::document_japanese_font`], Shippori
/// Mincho from craft-fonts), bounded so hostile replacement text cannot allocate unbounded
/// outline data. [`GlyphError::NoFont`] when the build has no Japanese face.
pub fn japanese_glyph(ch: char) -> Result<GlyphOutline, GlyphError> {
    let Some(face) = crate::document_japanese_font() else { return Err(GlyphError::NoFont) };
    japanese_glyph_from(face, ch)
}

/// The bounded outline and advance from the explicitly selected craft-fonts face.
/// Uses the same complexity and missing-glyph checks as [`japanese_glyph`].
pub fn japanese_glyph_from(face: &crate::CraftFont, ch: char) -> Result<GlyphOutline, GlyphError> {
    glyph_in_face(face.bytes, ch)
}

fn glyph_in_face(bytes: &[u8], ch: char) -> Result<GlyphOutline, GlyphError> {
    let Ok(font) = FontRef::new(bytes) else { return Err(GlyphError::Missing) };
    let loc = LocationRef::default();
    let metrics = font.metrics(Size::unscaled(), loc);
    let scale = 1.0 / metrics.units_per_em.max(1) as f64;
    let Some(gid) = font.charmap().map(ch) else { return Err(GlyphError::Missing) };
    let advances = font.glyph_metrics(Size::unscaled(), loc);
    let width = advances.advance_width(gid).unwrap_or(0.0) as f64 * scale;
    if !width.is_finite() || width <= 0.0 || width > 2.0 {
        return Err(GlyphError::Missing);
    }
    bounded_outline(&font, gid, width)
}

/// Glyph `gid` of `font` in em units with advance `width`, bounded like [`japanese_glyph`].
pub(crate) fn bounded_outline(font: &FontRef, gid: GlyphId, width: f64) -> Result<GlyphOutline, GlyphError> {
    let loc = LocationRef::default();
    let scale = 1.0 / font.metrics(Size::unscaled(), loc).units_per_em.max(1) as f64;
    let Some(glyph) = font.outline_glyphs().get(gid) else { return Err(GlyphError::Missing) };
    let mut pen = Flatten::new(scale);
    let _ = glyph.draw(DrawSettings::unhinted(Size::unscaled(), loc), &mut pen);
    pen.close();
    if pen.too_complex || pen.contours.len() > 256 {
        return Err(GlyphError::TooComplex);
    }
    let mut bbox = [0.0, 0.0, width, 0.0];
    let mut any = false;
    for p in pen.contours.iter().flatten() {
        if !p[0].is_finite() || !p[1].is_finite() {
            return Err(GlyphError::Missing);
        }
        if !any {
            bbox = [p[0], p[1], p[0], p[1]];
            any = true;
        } else {
            bbox[0] = bbox[0].min(p[0]);
            bbox[1] = bbox[1].min(p[1]);
            bbox[2] = bbox[2].max(p[0]);
            bbox[3] = bbox[3].max(p[1]);
        }
    }
    Ok(GlyphOutline { contours: pen.contours, width, bbox })
}

struct SignatureCluster {
    run: crate::source_shaping::ShapedRun,
    fallback: bool,
}

/// Marked Latin clusters must fit the common cluster budget and be shaped in one face.
/// Prefer the script face; a missing mark sends the complete cluster to the existing fallback.
fn signature_cluster(text: &str) -> Option<SignatureCluster> {
    if crate::latin_text::first_unsupported(text, false).is_some() {
        return None;
    }
    if let Ok(run) = crate::source_shaping::shape(FONT, text) {
        return Some(SignatureCluster { run, fallback: false });
    }
    crate::source_shaping::shape(INTER_REGULAR, text).ok().map(|run| SignatureCluster { run, fallback: true })
}

/// The first unsupported glyph/cluster, independently of the signature's total length.
/// Marked Latin clusters use the same face selection as [`script_outline`]; other characters
/// retain the script face and slanted Inter fallback. `None` means every cluster is drawable;
/// callers enforce [`MAX_SIGNATURE_CHARS`] separately before accepting a complete signature.
pub fn script_missing(text: &str) -> Option<char> {
    let font = FontRef::new(FONT).ok()?;
    let charmap = font.charmap();
    let fallback = FontRef::new(INTER_REGULAR).ok();
    let fallback_map = fallback.as_ref().map(|f| f.charmap());
    for cluster in crate::latin_text::clusters(text) {
        if crate::latin_text::required(cluster) {
            if signature_cluster(cluster).is_none() {
                return cluster.chars().nth(1).or_else(|| cluster.chars().next());
            }
        } else if let Some(ch) = cluster.chars().next()
            && !ch.is_whitespace()
            && charmap.map(ch).is_none()
            && fallback_map.as_ref().and_then(|m| m.map(ch)).is_none()
        {
            return Some(ch);
        }
    }
    None
}

fn draw_signature_cluster(pen: &mut Flatten, font: &FontRef, run: &crate::source_shaping::ShapedRun, em: f64, ratio: f64, slant: f64) -> bool {
    let start = pen.dx;
    let loc = LocationRef::default();
    for glyph in &run.glyphs {
        let Some(outline) = font.outline_glyphs().get(glyph.id) else { return false };
        (pen.scale, pen.slant) = (em, slant);
        pen.dx = start + (glyph.position[0] + slant * glyph.position[1]) * ratio;
        pen.dy = glyph.position[1] * ratio;
        if !pen.dx.is_finite() || !pen.dy.is_finite() || outline.draw(DrawSettings::unhinted(Size::unscaled(), loc), pen).is_err() {
            return false;
        }
        pen.close();
        if pen.too_complex {
            return false;
        }
    }
    pen.dx = start + run.advance * ratio;
    pen.dy = 0.0;
    pen.dx.is_finite()
}

/// The outlines of `text` in the script font. Characters it lacks (Cyrillic, Greek, …) are drawn
/// from Inter, slanted and scaled to the script's capital height; characters neither font has are
/// skipped (check [`script_missing`] first where nobody sees a preview). Marked Latin clusters
/// are shaped as a unit; invalid marked input or over-limit input returns an empty outline
/// instead of a partial signature. Unmarked NFC/ASCII retains its scalar drawing and advances.
pub fn script_outline(text: &str) -> ScriptOutline {
    if text.chars().take(MAX_SIGNATURE_CHARS + 1).count() > MAX_SIGNATURE_CHARS {
        return ScriptOutline::default();
    }
    if crate::latin_text::required(text) && script_missing(text).is_some() {
        return ScriptOutline::default();
    }
    let Ok(font) = FontRef::new(FONT) else { return ScriptOutline::default() };
    let loc = LocationRef::default();
    let metrics = font.metrics(Size::unscaled(), loc);
    let scale = 1.0 / metrics.units_per_em.max(1) as f64;
    let charmap = font.charmap();
    let glyphs = font.outline_glyphs();
    let advances = font.glyph_metrics(Size::unscaled(), loc);
    // The fallback face, scaled so its capitals are as tall as the script's.
    let fallback = FontRef::new(INTER_REGULAR).ok().map(|f| {
        let m = f.metrics(Size::unscaled(), loc);
        let em = 1.0 / m.units_per_em.max(1) as f64;
        let ratio = match (metrics.cap_height, m.cap_height) {
            (Some(ours), Some(theirs)) if ours > 0.0 && theirs > 0.0 => (ours as f64 * scale) / (theirs as f64 * em),
            _ => 1.0,
        };
        let ratio = if ratio.is_finite() { ratio.clamp(0.5, 2.0) } else { 1.0 };
        (f, em * ratio, ratio)
    });
    let mut pen = Flatten::new(scale);
    // The 4096-point budget is for one untrusted document glyph (japanese_glyph), not a
    // whole signature in our bundled font. Keep a separate bounded budget for the line.
    pen.max_points = 262_144;
    for cluster in crate::latin_text::clusters(text) {
        if crate::latin_text::required(cluster) {
            let Some(shaped) = signature_cluster(cluster) else { return ScriptOutline::default() };
            let drawn = if shaped.fallback {
                fallback.as_ref().is_some_and(|(font, em, ratio)| draw_signature_cluster(&mut pen, font, &shaped.run, *em, *ratio, FALLBACK_SLANT))
            } else {
                draw_signature_cluster(&mut pen, &font, &shaped.run, scale, 1.0, 0.0)
            };
            if !drawn {
                return ScriptOutline::default();
            }
            continue;
        }
        let Some(ch) = cluster.chars().next() else { continue };
        if let Some(gid) = charmap.map(ch) {
            (pen.scale, pen.slant) = (scale, 0.0);
            if let Some(g) = glyphs.get(gid) {
                let _ = g.draw(DrawSettings::unhinted(Size::unscaled(), loc), &mut pen);
                pen.close();
            }
            pen.dx += advances.advance_width(gid).unwrap_or(0.0) as f64 * scale;
        } else if let Some((face, em, _)) = &fallback
            && let Some(gid) = face.charmap().map(ch)
        {
            (pen.scale, pen.slant) = (*em, FALLBACK_SLANT);
            if let Some(g) = face.outline_glyphs().get(gid) {
                let _ = g.draw(DrawSettings::unhinted(Size::unscaled(), loc), &mut pen);
                pen.close();
            }
            pen.dx += face.glyph_metrics(Size::unscaled(), loc).advance_width(gid).unwrap_or(0.0) as f64 * em;
        }
    }
    pen.close();
    if pen.too_complex {
        return ScriptOutline::default();
    }
    ScriptOutline { contours: pen.contours, width: pen.dx, ascent: metrics.ascent as f64 * scale, descent: metrics.descent as f64 * scale }
}

#[cfg(test)]
mod tests {
    #[test]
    fn nfd_signature_clusters_match_their_composed_script_ink_and_advance() {
        for (nfc, nfd) in [
            (
                "ç Ç ğ Ğ İ ö Ö ş Ş ü Ü â Â î Î û Û",
                "c\u{327} C\u{327} g\u{306} G\u{306} I\u{307} o\u{308} O\u{308} s\u{327} S\u{327} u\u{308} U\u{308} a\u{302} A\u{302} i\u{302} I\u{302} u\u{302} U\u{302}",
            ),
            ("Çağrı Şişli", "C\u{327}ag\u{306}rı S\u{327}is\u{327}li"),
        ] {
            assert_eq!(super::script_missing(nfd), None);
            let expected = super::script_outline(nfc);
            let actual = super::script_outline(nfd);
            assert!(!actual.contours.is_empty());
            assert!((actual.width - expected.width).abs() < 1e-9, "{nfd:?}: {} vs {}", actual.width, expected.width);
            assert_eq!(actual.contours, expected.contours, "the same canonical glyphs must occupy the same positions");
            assert_eq!(actual.bounds(), expected.bounds());
        }
    }

    #[test]
    fn nfd_signature_incomposable_script_cluster_matches_the_shapers_ink() {
        let cluster = "q\u{301}";
        let (bytes, slant) = if crate::source_shaping::shape(super::FONT, cluster).is_ok() {
            (super::FONT, 0.0)
        } else {
            (super::INTER_REGULAR, super::FALLBACK_SLANT)
        };
        assert_shaped_signature_ink(cluster, bytes, slant, false);
    }

    #[test]
    fn nfd_signature_missing_script_mark_uses_one_scaled_slanted_inter_cluster() {
        // U+0315 forces the complete cluster into Inter. The stacked acute marks
        // have real horizontal/vertical placement, confirmed by the source shaper.
        let cluster = "q\u{315}\u{301}\u{301}";
        assert!(crate::source_shaping::shape(super::FONT, cluster).is_err(), "the fixture must force the complete fallback cluster");
        assert!(crate::source_shaping::shape(super::INTER_REGULAR, cluster).is_ok());
        assert_eq!(super::script_missing(cluster), None);
        assert_shaped_signature_ink(cluster, super::INTER_REGULAR, super::FALLBACK_SLANT, true);
    }

    fn assert_shaped_signature_ink(cluster: &str, bytes: &[u8], slant: f64, require_positioned_mark: bool) {
        use skrifa::MetadataProvider as _;
        let shaped = crate::source_shaping::shape(bytes, cluster).unwrap();
        assert!(shaped.glyphs.len() > 1, "the test must exercise a separate accent glyph");
        let face = skrifa::FontRef::new(bytes).unwrap();
        let script = skrifa::FontRef::new(super::FONT).unwrap();
        let loc = skrifa::instance::LocationRef::default();
        let metrics = face.metrics(skrifa::instance::Size::unscaled(), loc);
        let script_metrics = script.metrics(skrifa::instance::Size::unscaled(), loc);
        let em = 1.0 / f64::from(metrics.units_per_em);
        let advances = face.glyph_metrics(skrifa::instance::Size::unscaled(), loc);
        let mut scalar_pen = 0.0;
        let mut positioned = false;
        for glyph in &shaped.glyphs {
            positioned |= (glyph.position[0] - scalar_pen).abs() > 1e-9 || glyph.position[1] != 0.0;
            scalar_pen += f64::from(advances.advance_width(glyph.id).unwrap()) * em;
        }
        if require_positioned_mark {
            assert!(positioned, "the forced fallback fixture must move a mark away from independently painted scalar origins: {shaped:?}");
        }
        let ratio = if slant == 0.0 {
            1.0
        } else {
            let script_cap = f64::from(script_metrics.cap_height.unwrap()) / f64::from(script_metrics.units_per_em);
            (script_cap / (f64::from(metrics.cap_height.unwrap()) * em)).clamp(0.5, 2.0)
        };
        let mut expected = super::Flatten::new(em * ratio);
        expected.max_points = 262_144;
        expected.slant = slant;
        for glyph in &shaped.glyphs {
            let mut glyph_pen = super::Flatten::new(em * ratio);
            glyph_pen.max_points = 262_144;
            glyph_pen.slant = slant;
            let outline = face.outline_glyphs().get(glyph.id).unwrap();
            outline.draw(skrifa::outline::DrawSettings::unhinted(skrifa::instance::Size::unscaled(), loc), &mut glyph_pen).unwrap();
            glyph_pen.close();
            let dx = (glyph.position[0] + slant * glyph.position[1]) * ratio;
            let dy = glyph.position[1] * ratio;
            expected.contours.extend(glyph_pen.contours.into_iter().map(|contour| contour.into_iter().map(|p| [p[0] + dx, p[1] + dy]).collect()));
        }
        let actual = super::script_outline(cluster);
        assert!((actual.width - shaped.advance * ratio).abs() < 1e-9);
        assert_eq!(actual.contours.len(), expected.contours.len());
        assert_eq!(actual.contours.iter().map(Vec::len).collect::<Vec<_>>(), expected.contours.iter().map(Vec::len).collect::<Vec<_>>());
        for (actual, expected) in actual.contours.iter().flatten().zip(expected.contours.iter().flatten()) {
            assert!((actual[0] - expected[0]).abs() < 1e-9 && (actual[1] - expected[1]).abs() < 1e-9, "{actual:?} vs {expected:?}");
        }
    }

    #[test]
    fn nfd_signature_rejects_orphan_unsupported_and_excessive_mark_clusters() {
        for text in ["\u{306}g".to_string(), "g\u{306} a\u{1ab0}".into(), format!("g{}", "\u{306}".repeat(33))] {
            assert!(super::script_missing(&text).is_some(), "invalid cluster {text:?} must have an explicit missing-mark result");
            assert!(super::script_outline(&text).contours.is_empty(), "invalid NFD must not leave partial signature ink");
        }
    }

    #[test]
    fn nfd_signature_preserves_the_existing_scalar_limit_and_outline_budget() {
        let at_limit = format!("{}g\u{306}", "a".repeat(super::MAX_SIGNATURE_CHARS - 2));
        assert_eq!(at_limit.chars().count(), super::MAX_SIGNATURE_CHARS);
        assert!(!super::script_outline(&at_limit).contours.is_empty());
        let over_limit = at_limit + "a";
        assert_eq!(super::script_missing(&over_limit), None, "the length limit must not masquerade as an unsupported glyph");
        assert!(super::script_outline(&over_limit).contours.is_empty());
        let stacked = format!("a{}", "\u{301}".repeat(8));
        let ink = super::script_outline(&stacked);
        assert!(!ink.contours.is_empty());
        assert!(ink.bounds().iter().all(|n| n.is_finite()));
        assert!(ink.contours.iter().map(Vec::len).sum::<usize>() <= 262_144);
    }

    #[test]
    fn long_names_keep_every_glyph_with_bounded_outline_work() {
        let text = "Alexandria Catherine Elizabeth Montgomery-Wellington";
        let o = super::script_outline(text);
        let expected: usize = text.chars().map(|c| super::script_outline(&c.to_string()).contours.len()).sum();
        assert_eq!(o.contours.len(), expected, "the entire name must be drawn");
        let max_x = o.contours.iter().flatten().map(|p| p[0]).fold(0.0, f64::max);
        assert!(max_x > o.width - 0.5, "ink reaches the last letter: {max_x} / {}", o.width);
        assert!(!super::script_outline(&"W".repeat(super::MAX_SIGNATURE_CHARS)).contours.is_empty());
        assert!(super::script_outline(&"W".repeat(super::MAX_SIGNATURE_CHARS + 1)).contours.is_empty());
    }

    /// Issue #846: Cyrillic (and Greek) names come out of the fallback face instead of vanishing.
    #[test]
    fn cyrillic_and_greek_names_are_drawn_from_the_fallback_face() {
        for name in ["Саша Дермановић", "Љубица Ђорђевић", "Ђурђа Џаковић", "Νίκος"] {
            let o = super::script_outline(name);
            let letters = name.chars().filter(|c| !c.is_whitespace()).count();
            assert!(o.contours.len() >= letters, "{name}: {} contours for {letters} letters", o.contours.len());
            for ch in name.chars().filter(|c| !c.is_whitespace()) {
                let one = super::script_outline(&ch.to_string());
                assert!(!one.contours.is_empty() && one.width > 0.0, "{name}: {ch} has no outline");
            }
            let [_, bottom, right, top] = o.bounds();
            assert!(right <= o.width + 0.5 && top <= o.ascent + 0.2 && bottom >= o.descent - 0.2, "{name}: {:?}", o.bounds());
        }
        // Capitals from the fallback are about as tall as the script's.
        let cap = |c: &str| super::script_outline(c).contours.iter().flatten().map(|p| p[1]).fold(0.0, f64::max);
        let (latin, cyrillic) = (cap("H"), cap("Н"));
        assert!((cyrillic / latin - 1.0).abs() < 0.2, "Н {cyrillic} vs H {latin}");
        // Mixed names keep every letter, in order: the Cyrillic follows the Latin.
        let mixed = super::script_outline("Ana Ана");
        let latin_only = super::script_outline("Ana ");
        assert!(mixed.width > latin_only.width + 0.5, "{} vs {}", mixed.width, latin_only.width);
    }

    #[test]
    fn missing_script_glyphs_are_named() {
        assert_eq!(super::script_missing("Ada Lovelace-King"), None);
        assert_eq!(super::script_missing("Zoë Ñúñez"), None);
        assert_eq!(super::script_missing("Ada 张三"), Some('张'));
        // Cyrillic and Greek come from the Inter fallback (#963), so they aren't missing.
        assert_eq!(super::script_missing("Саша Дермановић Νίκος"), None);
        assert_eq!(super::script_missing("Саша 张"), Some('张'));
        assert_eq!(super::script_missing(" \t"), None);
    }

    #[test]
    fn a_name_becomes_outlines() {
        let o = super::script_outline("Ada L.");
        assert!(o.contours.len() >= 5, "{}", o.contours.len());
        assert!(o.width > 1.0 && o.width < 6.0, "{}", o.width);
        assert!(o.ascent > 0.5 && o.descent < 0.0);
        let max_x = o.contours.iter().flatten().map(|p| p[0]).fold(0.0, f64::max);
        assert!(max_x <= o.width + 0.2);
        assert!(super::script_outline("").contours.is_empty());
    }

    #[test]
    fn japanese_glyphs_come_from_craft_fonts() {
        if crate::document_japanese_font().is_none() {
            eprintln!("skipping the glyph checks: built without craft-fonts (set CRAFT_FONTS_DIR)");
            assert_eq!(super::japanese_glyph('こ'), Err(super::GlyphError::NoFont));
            return;
        }
        for ch in "日本語の文字こ".chars() {
            let g = super::japanese_glyph(ch).expect("craft-fonts face has the Japanese glyph");
            assert!(g.width > 0.2 && g.width < 2.0);
            assert!(!g.contours.is_empty());
        }
        assert_eq!(super::japanese_glyph('\u{1f4a9}'), Err(super::GlyphError::Missing));
    }
}
