//! Font fallback per grapheme cluster, and ligature caret positions.
//!
//! The cascade for a cluster (a base character with its marks, never split across faces) is:
//! 1. the faces the document asks for, in order (the run's slot typeface, then the theme's script
//!    font, `a:font[@script="Arab"]`);
//! 2. for scripts with a platform cascade (Arabic, Hebrew, Thai, Devanagari): the craft-fonts face
//!    bundled for the script (Noto Naskh Arabic for Arabic), so every machine draws the same; then
//!    openly licensed or OS-native families found by name among the installed fonts (no platform
//!    API is called); then any installed face covering the cluster;
//! 3. any loaded face (bundled first: Japanese keeps its craft-fonts faces), then the first
//!    requested face (drawn as .notdef).

use std::sync::Arc;

use skrifa::raw::TableProvider;

use crate::script::{ScriptTag, script_of};
use crate::{CRAFT_FONTS, FontDb, FontFace};

/// Variation selector 15: text presentation.
const TEXT_SELECTOR: char = '\u{FE0E}';
/// Combining enclosing keycap.
const KEYCAP: char = '\u{20E3}';

/// Characters that never decide a face: they're drawn by whatever face the cluster gets.
fn ignorable(c: char) -> bool {
    c.is_whitespace()
        || c.is_control()
        || matches!(c as u32, 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2069 | 0xFE00..=0xFE0F | 0xFEFF | 0xE0100..=0xE01EF)
}

/// Is the cluster an emoji drawn in colour: a pictograph that defaults to emoji presentation
/// (Unicode's Emoji_Presentation, including BMP ones such as ⭐ ✅ ☔ ⌚ ⏩), or a sequence with an
/// emoji presentation selector (U+FE0F), a zero-width joiner between pictographs, a skin tone
/// modifier or regional indicators (flags)? Keycaps ("1️⃣") are not: PowerPoint draws them as text
/// (an outlined key), as it draws ♥ ✔ ☎ ⬆ ▶ without a selector.
pub fn is_emoji_cluster(cluster: &[char]) -> bool {
    if cluster.contains(&'\u{FE0E}') || cluster.contains(&'\u{20E3}') {
        return false;
    }
    let pictographic = |c: char| matches!(c as u32, 0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF | 0x2190..=0x21FF | 0x2300..=0x23FF);
    let default_emoji = |c: char| {
        matches!(
            c as u32,
            0x1F300..=0x1F64F
                | 0x1F680..=0x1F6FF
                | 0x1F900..=0x1F9FF
                | 0x1FA70..=0x1FAFF
                | 0x1F1E6..=0x1F1FF
                | 0x1F004
                | 0x1F0CF
                | 0x1F18E
                | 0x1F191..=0x1F19A
                | 0x1F201
                | 0x1F21A
                | 0x1F22F
                | 0x1F232..=0x1F236
                | 0x1F238..=0x1F23A
                | 0x1F250..=0x1F251
                | 0x231A..=0x231B
                | 0x23E9..=0x23EC
                | 0x23F0
                | 0x23F3
                | 0x25FD..=0x25FE
                | 0x2614..=0x2615
                | 0x2648..=0x2653
                | 0x267F
                | 0x2693
                | 0x26A1
                | 0x26AA..=0x26AB
                | 0x26BD..=0x26BE
                | 0x26C4..=0x26C5
                | 0x26CE
                | 0x26D4
                | 0x26EA
                | 0x26F2..=0x26F3
                | 0x26F5
                | 0x26FA
                | 0x26FD
                | 0x2705
                | 0x270A..=0x270B
                | 0x2728
                | 0x274C
                | 0x274E
                | 0x2753..=0x2755
                | 0x2757
                | 0x2795..=0x2797
                | 0x27B0
                | 0x27BF
                | 0x2B1B..=0x2B1C
                | 0x2B50
                | 0x2B55
        )
    };
    cluster.iter().any(|&c| default_emoji(c))
        || (cluster.contains(&'\u{FE0F}') && cluster.iter().any(|&c| pictographic(c)))
        || (cluster.contains(&'\u{200D}') && cluster.iter().any(|&c| pictographic(c)))
}

/// Does `face` draw every character of the cluster?
pub fn covers_cluster(face: &FontFace, cluster: &[char]) -> bool {
    cluster.iter().all(|c| ignorable(*c) || face.covers(*c))
}

/// Installed families tried by name for a script after the bundled face, before scanning every
/// installed font. Microsoft font names are not listed (AGENTS.md §1.1); an installed one is
/// still used by the scan when nothing earlier covers the cluster.
fn os_families(script: ScriptTag) -> &'static [&'static str] {
    match &script {
        b"Arab" => &["Noto Naskh Arabic", "Noto Sans Arabic", "Geeza Pro", "SF Arabic", "DejaVu Sans"],
        b"Hebr" => &["Noto Sans Hebrew", "Arial Hebrew", "SF Hebrew", "DejaVu Sans"],
        b"Thai" => &["Noto Sans Thai", "Thonburi"],
        b"Deva" => &["Noto Sans Devanagari", "Kohinoor Devanagari", "Devanagari Sangam MN"],
        _ => &[],
    }
}

/// The craft-fonts families for a script (manifest order), e.g. Noto Naskh Arabic for `Arab`.
pub fn bundled_families(script: ScriptTag) -> Vec<&'static str> {
    let tag = std::str::from_utf8(&script).unwrap_or("");
    let mut v: Vec<&'static str> = CRAFT_FONTS.iter().filter(|f| f.scripts.contains(&tag)).map(|f| f.family).collect();
    v.dedup();
    v
}

impl FontDb {
    /// The face for one grapheme cluster: the first of `chain` that draws all of it, else the
    /// bundled face for its script, else an installed face for the script, else any loaded
    /// face that draws it, else `chain[0]` (or the default face if `chain` is empty).
    pub fn cascade(&self, cluster: &[char], chain: &[Arc<FontFace>], style: &str) -> Arc<FontFace> {
        // Emoji take a colour face (with its ZWJ sequences, skin tones and flags) before the
        // document's fonts, as PowerPoint draws them, even when a text font has a plain glyph.
        if is_emoji_cluster(cluster) {
            if let Some(f) = chain.iter().find(|f| f.is_color() && covers_cluster(f, cluster)) {
                return f.clone();
            }
            if let Some(key) = cluster.iter().copied().find(|c| !ignorable(*c))
                && let Some(f) = self.color_face_for(key, cluster)
            {
                return f;
            }
        }
        if let Some(f) = chain.iter().find(|f| covers_cluster(f, cluster)) {
            return f.clone();
        }
        // Text presentation (U+FE0E) and keycaps: a plain face, not a colour emoji one.
        let text_style = cluster.contains(&TEXT_SELECTOR) || cluster.contains(&KEYCAP);
        if text_style
            && let Some(key) = cluster.iter().copied().find(|c| !ignorable(*c))
            && let Some(f) = self.plain_face_for(key, cluster)
        {
            return f;
        }
        let first = || chain.first().cloned().unwrap_or_else(|| self.face(crate::DEFAULT_FAMILY, style));
        let Some(key) = cluster.iter().copied().find(|c| !ignorable(*c)) else { return first() };
        let script = cluster.iter().find_map(|c| script_of(*c));
        // Scripts with a platform cascade (Arabic, Hebrew, Thai, Devanagari): the bundled face,
        // then installed ones; the others keep the loaded-faces-first order (Japanese documents
        // prefer the bundled craft-fonts faces).
        if let Some(s) = script.filter(|s| !os_families(*s).is_empty()) {
            let bundled = bundled_families(s);
            for fam in &bundled {
                let f = self.face(fam, style);
                if f.family.eq_ignore_ascii_case(fam) && covers_cluster(&f, cluster) {
                    return f;
                }
            }
            for fam in os_families(s) {
                if bundled.contains(fam) {
                    continue;
                }
                if self.is_loaded(fam) || (self.system_fallback_enabled() && self.has_family(fam)) {
                    // With system fallback off, only a face that is already loaded: `face` could load more catalog files.
                    let f = if self.system_fallback_enabled() {
                        self.face(fam, style)
                    } else {
                        let Some(f) = self.find(fam, style) else { continue };
                        f
                    };
                    if f.family.eq_ignore_ascii_case(fam) && covers_cluster(&f, cluster) {
                        return f;
                    }
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(f) = self.system_face_for(key, cluster, &bundled) {
                return f;
            }
        }
        let exclude = chain.first().map(|f| f.id()).unwrap_or(u32::MAX);
        let f = match self.fallback_for(key, exclude) {
            Some(f) if covers_cluster(&f, cluster) => f,
            Some(f) if chain.iter().all(|c| !c.covers(key)) => f,
            // A letter with marks the run's font lacks (stacked combining marks): a loaded plain
            // face that draws the whole cluster, as PowerPoint draws the marks, before the run's
            // font with missing-glyph boxes.
            _ => {
                // Searched before `first` runs: it may load the default family, which needs the
                // faces' write lock, so the read lock must not be held across it.
                let found = self.read_faces().iter().find(|f| !f.is_color() && covers_cluster(f, cluster)).cloned();
                found.unwrap_or_else(first)
            }
        };
        // A pictograph that defaults to text presentation (⚠ ♥ ⬆ ▶) whose first fallback is the
        // colour emoji font: PowerPoint draws it from a plain symbol font.
        if f.is_color()
            && !is_emoji_cluster(cluster)
            && let Some(plain) = self.plain_face_for(key, cluster)
        {
            return plain;
        }
        f
    }

    /// An installed (non-bundled) face covering the cluster, loading one from the system catalog
    /// if no loaded face does.
    #[cfg(not(target_arch = "wasm32"))]
    fn system_face_for(&self, key: char, cluster: &[char], bundled: &[&str]) -> Option<Arc<FontFace>> {
        let pick = |db: &FontDb| {
            let faces = db.read_faces();
            let mut v: Vec<&Arc<FontFace>> = faces
                .iter()
                .filter(|f| !bundled.iter().any(|b| f.family.eq_ignore_ascii_case(b)) && !CRAFT_FONTS.iter().any(|c| c.family == f.family))
                .filter(|f| covers_cluster(f, cluster))
                .collect();
            v.sort_by_key(|f| (f.italic, (f.weight - 400.0).abs() as i32));
            v.first().map(|f| (*f).clone())
        };
        if let Some(f) = pick(self) {
            return Some(f);
        }
        if self.system_fallback(key) { pick(self) } else { None }
    }
}

/// Most caret records read for one ligature glyph.
const MAX_LIGATURE_CARETS: usize = 64;

/// Caret positions inside a ligature glyph from GDEF `LigCaretList`, in font units from the
/// glyph's origin (sorted ascending). Empty when the font has none for `gid`.
pub fn ligature_carets(face: &FontFace, gid: u32) -> Vec<i32> {
    let Some(font) = face.skrifa() else { return Vec::new() };
    let Ok(gdef) = font.gdef() else { return Vec::new() };
    let Some(Ok(list)) = gdef.lig_caret_list() else { return Vec::new() };
    let Ok(cov) = list.coverage() else { return Vec::new() };
    let Some(idx) = cov.get(skrifa::GlyphId::new(gid)) else { return Vec::new() };
    let Ok(lig) = list.lig_glyphs().get(idx as usize) else { return Vec::new() };
    // The limit applies to the records read, not to the carets kept: a font can list thousands of
    // records it then has to be read through for every ligature laid out.
    let mut out: Vec<i32> = lig
        .caret_values()
        .iter()
        .take(MAX_LIGATURE_CARETS)
        .filter_map(|cv| cv.ok())
        .filter_map(|cv| match cv {
            skrifa::raw::tables::gdef::CaretValue::Format1(f) => Some(f.coordinate() as i32),
            skrifa::raw::tables::gdef::CaretValue::Format3(f) => Some(f.coordinate() as i32),
            // Format 2 is a contour point index; outlines aren't consulted here.
            skrifa::raw::tables::gdef::CaretValue::Format2(_) => None,
        })
        .collect();
    out.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_face_wins_when_it_covers() {
        let db = FontDb::global();
        let inter = db.face(crate::DEFAULT_FAMILY, "Regular");
        let f = db.cascade(&['a'], std::slice::from_ref(&inter), "Regular");
        assert!(Arc::ptr_eq(&f, &inter));
        // Spaces and joiners never force a fallback.
        let f = db.cascade(&[' ', '\u{200D}'], std::slice::from_ref(&inter), "Regular");
        assert!(Arc::ptr_eq(&f, &inter));
        // Nothing to go on: the default face.
        assert!(!db.cascade(&[], &[], "Regular").family.is_empty());
    }

    #[test]
    fn arabic_cluster_falls_back_to_an_arabic_face() {
        let db = FontDb::global();
        let inter = db.face(crate::DEFAULT_FAMILY, "Regular");
        let cluster = ['\u{0628}', '\u{064E}']; // beh + fatha: one cluster, one face.
        let f = db.cascade(&cluster, std::slice::from_ref(&inter), "Regular");
        if covers_cluster(&f, &cluster) {
            assert!(f.covers('\u{0628}'));
        } else {
            eprintln!("skipped: no installed or bundled face covers Arabic");
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn falling_back_to_the_default_face_does_not_deadlock() {
        // An empty chain and a character no loaded face draws: the loaded faces are searched,
        // then the default family is looked up, which loads it (taking the faces' write lock).
        // Holding the search's read lock across that lookup hung the thread.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let db = FontDb::with_font_dirs(Vec::new());
            db.set_system_fallback(false);
            let _ = tx.send(db.cascade(&['\u{10FFFD}'], &[], "Regular").family.clone());
        });
        let family = rx.recv_timeout(std::time::Duration::from_secs(30)).expect("cascade did not return");
        assert!(!family.is_empty());
    }

    #[test]
    fn ligature_carets_never_fail() {
        let face = FontDb::global().face(crate::DEFAULT_FAMILY, "Regular");
        for gid in [0, 1, 100, u32::MAX] {
            let c = ligature_carets(&face, gid);
            assert!(c.windows(2).all(|w| w[0] <= w[1]));
        }
    }

    /// With system fallback off, the automatic cascades (script families and symbol faces) load no
    /// installed family: the result comes from what is already loaded.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn cascades_load_no_installed_family_when_system_fallback_is_off() {
        let db = FontDb::with_font_dirs(Vec::new());
        db.set_system_fallback(false);
        let base = db.face(crate::DEFAULT_FAMILY, "Regular");
        let loaded = |db: &FontDb| [b"Arab", b"Hebr", b"Thai", b"Deva"].into_iter().flat_map(|s| os_families(*s)).filter(|f| db.is_loaded(f)).count();
        let before = loaded(&db);
        for c in ['\u{0628}', '\u{05D0}', '\u{0E01}', '\u{0905}', '\u{2211}', '\u{2603}'] {
            let f = db.cascade(&[c], std::slice::from_ref(&base), "Regular");
            assert!(!f.family.is_empty());
        }
        assert_eq!(loaded(&db), before, "no installed family was loaded");
    }
}
