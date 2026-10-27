//! Shaping an explicitly selected source/document face, with OpenType features and UAX #9.
use harfrust::{BufferClusterLevel, Direction, ShapeOptions, ShaperData, UnicodeBuffer};
use skrifa::raw::TableProvider;
use skrifa::{FontRef, GlyphId, MetadataProvider};
use unicode_bidi::{Level, ParagraphBidiInfo};

#[derive(Clone, Debug)]
pub struct ShapedGlyph {
    pub id: GlyphId,
    pub position: [f64; 2],
    /// The characters this glyph stands for in `/ToUnicode`: its own in a left-to-right cluster
    /// whose glyphs each take one, else the whole cluster on its first glyph and none on the rest.
    pub text: String,
    /// On the first glyph of a left-to-right cluster of several glyphs (a letter and its marks,
    /// Sara Am drawn as two glyphs): the cluster's text and glyph count, for an `/ActualText`
    /// span around just that cluster, as Chrome writes them. Readers that honour it read the
    /// marks in typing order wherever shaping put them; the rest read `text`.
    pub span: Option<(String, usize)>,
}
#[derive(Clone, Debug)]
pub struct ShapedRun {
    pub glyphs: Vec<ShapedGlyph>,
    pub advance: f64,
    /// Whether any of it is right-to-left: drawn in visual order, it then needs one `/ActualText`
    /// in typing order around the whole run.
    pub rtl: bool,
}

/// Complex scripts/marks require shaping instead of independently encoding characters.
pub fn required(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(u32::from(c), 0x0300..=0x036f | 0x0590..=0x0fff | 0x1780..=0x17ff | 0x200c..=0x200f | 0x202a..=0x202e | 0x2066..=0x2069))
}

/// Shape one row of `text` with the face in `bytes`, run by run in visual (bidi) order: each glyph
/// with its position in ems and the characters it stands for in `/ToUnicode`, its left-to-right
/// clusters marked for their own `/ActualText` spans ([`ShapedGlyph::span`]). Errors on a row
/// over 256 KiB or with a line break, a face that can't be read, Arabic or Khmer in a face with no
/// `GSUB`, and a glyph the face lacks.
pub fn shape(bytes: &[u8], text: &str) -> Result<ShapedRun, String> {
    if text.len() > 262_144 || text.contains(['\n', '\r']) {
        return Err("shape one bounded text row at a time".into());
    }
    if text.is_empty() {
        return Ok(ShapedRun { glyphs: Vec::new(), advance: 0.0, rtl: false });
    }
    let face = FontRef::new(bytes).map_err(|e| e.to_string())?;
    if text.chars().any(|c| matches!(u32::from(c), 0x0600..=0x06ff | 0x0750..=0x077f | 0x08a0..=0x0fff | 0x1780..=0x17ff)) && face.gsub().is_err() {
        return Err("the source subset has no contextual substitution tables; select its full font".into());
    }
    let data = ShaperData::new(&face);
    let shaper = data.shaper(&face).build();
    let em = f64::from(shaper.units_per_em().max(1));
    let base = if unicode_bidi::get_base_direction(text) == unicode_bidi::Direction::Rtl { Level::rtl() } else { Level::ltr() };
    let bidi = ParagraphBidiInfo::new(text, Some(base));
    let (levels, runs) = bidi.visual_runs(0..text.len());
    let charmap = face.charmap();
    let mut glyphs: Vec<ShapedGlyph> = Vec::new();
    let mut x = 0.0;
    let mut any_rtl = false;
    for run in runs {
        let Some(input) = text.get(run.clone()) else { continue };
        let rtl = levels.get(run.start).is_some_and(Level::is_rtl);
        any_rtl |= rtl;
        let run_start = glyphs.len();
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(input);
        buffer.set_cluster_level(BufferClusterLevel::MonotoneGraphemes);
        buffer.set_direction(if rtl { Direction::RightToLeft } else { Direction::LeftToRight });
        buffer.guess_segment_properties();
        let output = shaper.shape(buffer, ShapeOptions::new());
        let mut starts: Vec<_> = output.glyph_infos().iter().map(|g| g.cluster as usize).collect();
        starts.sort_unstable();
        starts.dedup();
        let mut seen = std::collections::HashSet::new();
        for (g, p) in output.glyph_infos().iter().zip(output.glyph_positions()) {
            if g.glyph_id == 0 {
                return Err("the full font lacks a required contextual glyph; choose the complete source face".into());
            }
            let start = g.cluster as usize;
            let end = starts.get(starts.partition_point(|i| *i <= start)).copied().unwrap_or(input.len());
            let logical = if seen.insert(start) { input.get(start..end).unwrap_or_default().to_string() } else { String::new() };
            glyphs.push(ShapedGlyph {
                id: GlyphId::new(g.glyph_id),
                position: [x + f64::from(p.x_offset) / em, f64::from(p.y_offset) / em],
                text: logical,
                span: None,
            });
            x += f64::from(p.x_advance) / em;
        }
        if !rtl {
            let clusters: Vec<u32> = output.glyph_infos().iter().map(|g| g.cluster).collect();
            split_clusters(glyphs.get_mut(run_start..).unwrap_or_default(), &clusters, |c| charmap.map(c));
        }
    }
    if !x.is_finite() || glyphs.len() > 65_534 {
        return Err("shaped row exceeds its geometry budget".into());
    }
    Ok(ShapedRun { glyphs, advance: x, rtl: any_rtl })
}

/// Mark each left-to-right cluster of several glyphs for a span, and give each of its glyphs its
/// own character when the cluster has as many glyphs as characters and they correspond in order:
/// every glyph is its character's own (the face's cmap) and stands at its character's place, or
/// the cluster is Thai, whose shaping substitutes mark forms one for one (Sara Am, split into two
/// glyphs, has one more glyph than characters), and at most one of its glyphs is such a form, so
/// the others fix where every character is drawn. Otherwise the text stays on the first glyph. `glyphs` and
/// `clusters` (harfrust's cluster of each) are one run, in order; each cluster's text is already
/// on its first glyph.
fn split_clusters(glyphs: &mut [ShapedGlyph], clusters: &[u32], nominal: impl Fn(char) -> Option<GlyphId>) {
    let mut at = 0;
    while at < glyphs.len() {
        let cluster = clusters.get(at).copied();
        let len = clusters.get(at..).unwrap_or_default().iter().take_while(|c| Some(**c) == cluster).count().max(1);
        let Some(group) = glyphs.get_mut(at..at + len) else { break };
        at += len;
        if group.len() < 2 {
            continue;
        }
        let text = group.first().map(|g| g.text.clone()).unwrap_or_default();
        let chars: Vec<char> = text.chars().collect();
        let thai = chars.iter().all(|c| matches!(u32::from(*c), 0x0e01..=0x0e5b));
        // Shaping sorts a cluster's marks by combining class (`กฺุ` is drawn with ุ before ฺ, and
        // a tone typed before a lower vowel after it), so the drawn order has to be shown, not
        // assumed: every glyph that is some character's own must stand at that character's place,
        // and at most one may be a substituted form (a lowered tone mark), which is no character's
        // own. The placed glyphs then leave it only its own place; two substituted forms could
        // have been swapped. The characters' own glyphs are collected once, so a cluster of many
        // marks stays linear.
        let one_for_one = chars.len() == group.len() && {
            let own: Vec<Option<GlyphId>> = chars.iter().map(|c| nominal(*c)).collect();
            let owned: std::collections::HashSet<GlyphId> = own.iter().flatten().copied().collect();
            let placed = group.iter().zip(&own).all(|(g, o)| *o == Some(g.id) || !owned.contains(&g.id));
            let substituted = group.iter().filter(|g| !owned.contains(&g.id)).count();
            placed && substituted <= usize::from(thai)
        };
        if one_for_one {
            for (glyph, c) in group.iter_mut().zip(chars) {
                glyph.text = c.to_string();
            }
        }
        if let Some(first) = group.first_mut() {
            first.span = Some((text, len));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cluster's glyphs as shaping leaves them: its text on the first glyph, none on the rest.
    fn cluster(text: &str, ids: &[u32]) -> Vec<ShapedGlyph> {
        ids.iter()
            .enumerate()
            .map(|(i, id)| ShapedGlyph {
                id: GlyphId::new(*id),
                position: [0.0, 0.0],
                text: if i == 0 { text.to_owned() } else { String::new() },
                span: None,
            })
            .collect()
    }

    /// #4919: each glyph of a cluster takes its own character when they correspond one for one,
    /// and every cluster of several glyphs is marked for its own span. No font is needed: the
    /// face's cmap is a function from characters to glyphs.
    #[test]
    fn clusters_map_each_glyph_to_its_character_where_they_correspond() {
        // The face's own glyphs: ก 10, ท 11, น 12, ป 13, ี 20, ่ 21, ้ 22, า 23, ํ 24, ุ 25, ู 26, ฺ 27,
        // q 30, ́ 31, a 32, ̣ 33, n 34, ̃ 35, ̄ 36, ๛ 37 (the last Thai character), U+0E5C 38.
        let nominal = |c: char| {
            let id = match c {
                'ก' => 10,
                'ท' => 11,
                'น' => 12,
                'ป' => 13,
                '\u{e35}' => 20,
                '\u{e48}' => 21,
                '\u{e49}' => 22,
                '\u{e32}' => 23,
                '\u{e4d}' => 24,
                '\u{e38}' => 25,
                '\u{e39}' => 26,
                '\u{e3a}' => 27,
                'q' => 30,
                '\u{301}' => 31,
                'a' => 32,
                '\u{323}' => 33,
                'n' => 34,
                '\u{303}' => 35,
                '\u{304}' => 36,
                '\u{e5b}' => 37,
                '\u{e5c}' => 38,
                _ => return None,
            };
            Some(GlyphId::new(id))
        };
        let texts = |glyphs: &[ShapedGlyph]| glyphs.iter().map(|g| g.text.clone()).collect::<Vec<_>>();
        let spans = |glyphs: &[ShapedGlyph]| glyphs.iter().map(|g| g.span.clone()).collect::<Vec<_>>();
        let cases: [(&str, &[u32], &[&str]); 17] = [
            // Thai, with the tone mark swapped for a lowered form (99): still one for one.
            ("ที่", &[11, 20, 99], &["ท", "\u{e35}", "\u{e48}"]),
            ("ปู่", &[13, 26, 99], &["ป", "\u{e39}", "\u{e48}"]),
            // Marks shaping reordered by combining class: ุ drawn before ฺ, and a tone typed before
            // its lower vowel drawn after it. The cluster stays on the first glyph.
            ("ก\u{e3a}\u{e38}", &[10, 25, 27], &["ก\u{e3a}\u{e38}", "", ""]),
            ("ป\u{e48}\u{e39}", &[13, 26, 99], &["ป\u{e48}\u{e39}", "", ""]),
            // Sara Am split into nikhahit and sara aa: one more glyph than characters.
            ("น้ำ", &[12, 24, 22, 23], &["น้ำ", "", "", ""]),
            // Latin with the face's own mark glyph.
            ("q\u{301}", &[30, 31], &["q", "\u{301}"]),
            // Latin whose mark was substituted (a stacking form): the glyphs can't be told apart.
            ("q\u{301}", &[30, 98], &["q\u{301}", ""]),
            // Devanagari reordered (the i-matra drawn before its consonant): not split.
            ("कि", &[41, 40], &["कि", ""]),
            // A ligature: one glyph for two characters, no span needed.
            ("fi", &[50], &["fi"]),
            // A letter on its own.
            ("ก", &[10], &["ก"]),
            // More characters than glyphs (ñ composed, then the macron): the text stays on the first.
            ("n\u{303}\u{304}", &[80, 36], &["n\u{303}\u{304}", ""]),
            // Latin marks drawn swapped, both the face's own glyphs: not in place, not split.
            ("a\u{301}\u{323}", &[32, 33, 31], &["a\u{301}\u{323}", "", ""]),
            ("a\u{301}\u{323}", &[32, 31, 33], &["a", "\u{301}", "\u{323}"]),
            // Thai with a non-Thai mark is not Thai: own glyphs in place map, a substitution doesn't.
            ("ก\u{301}", &[10, 31], &["ก", "\u{301}"]),
            ("ก\u{301}", &[10, 98], &["ก\u{301}", ""]),
            // The Thai range ends at U+0E5B: one substitution is allowed there, not past it.
            ("ก\u{e5b}", &[10, 98], &["ก", "\u{e5b}"]),
            ("ก\u{e5c}", &[10, 98], &["ก\u{e5c}", ""]),
        ];
        for (text, ids, expected) in cases {
            let mut glyphs = cluster(text, ids);
            let clusters = vec![7; glyphs.len()];
            split_clusters(&mut glyphs, &clusters, nominal);
            assert_eq!(texts(&glyphs), expected.iter().map(|s| s.to_string()).collect::<Vec<_>>(), "{text}");
            let span = (ids.len() > 1).then(|| (text.to_owned(), ids.len()));
            let mut want = vec![None; ids.len()];
            want[0] = span;
            assert_eq!(spans(&glyphs), want, "{text}");
        }
        // Clusters are found by harfrust's cluster values, one run after another.
        let mut glyphs = cluster("ก", &[10]);
        glyphs.extend(cluster("ที่", &[11, 20, 21]));
        split_clusters(&mut glyphs, &[0, 3, 3, 3], nominal);
        assert_eq!(texts(&glyphs), ["ก", "ท", "\u{e35}", "\u{e48}"]);
        assert_eq!(spans(&glyphs), [None, Some(("ที่".to_owned(), 3)), None, None]);
    }

    /// The same with real shaping, by every Thai face this build embeds: the interface face (Noto
    /// Sans Thai, in every build with craft-fonts, CI's included) and the document face. Shaping
    /// reads the font only, so a variable face does. The marks shaping reorders keep their cluster
    /// on the first glyph; the rest map one for one.
    #[test]
    fn real_thai_shaping_maps_glyphs_only_in_typing_order() {
        let mut faces = crate::ui_thai_fonts();
        faces.extend(crate::document_thai_font().filter(|d| !faces.iter().any(|f| std::ptr::eq(*f, *d))));
        for face in faces {
            let texts = |text: &str| shape(face.bytes, text).unwrap().glyphs.into_iter().map(|g| g.text).collect::<Vec<_>>();
            let name = face.family;
            assert_eq!(texts("ปู่"), ["ป", "\u{e39}", "\u{e48}"], "{name}");
            assert_eq!(texts("กิ่"), ["ก", "\u{e34}", "\u{e48}"], "{name}");
            assert_eq!(texts("ป\u{e48}\u{e39}"), ["ป\u{e48}\u{e39}", "", ""], "{name}");
            assert_eq!(texts("ก\u{e3a}\u{e38}"), ["ก\u{e3a}\u{e38}", "", ""], "{name}");
        }
    }

    /// Property: whatever order shaping draws a Thai cluster's marks in, and whichever of them it
    /// swaps for another form, a cluster mapped glyph by glyph gives every glyph its own character.
    /// Every sequence of one to three marks after a letter, in every drawn order, with every set
    /// of substituted glyphs (ids no character owns), the letter's included. A cluster it can't map stays on its first
    /// glyph, which is never wrong.
    #[test]
    fn a_thai_cluster_is_mapped_glyph_by_glyph_only_when_every_glyph_is_placed() {
        const MARKS: [char; 9] = ['\u{e38}', '\u{e39}', '\u{e3a}', '\u{e34}', '\u{e35}', '\u{e48}', '\u{e49}', '\u{e47}', '\u{e4c}'];
        let nominal = |c: char| {
            Some(GlyphId::new(match c {
                'ก' => 10,
                c => 100 + u32::from(c) - 0x0e00,
            }))
        };
        let mut sequences: Vec<Vec<char>> = vec![Vec::new()];
        for _ in 0..3 {
            let longer: Vec<Vec<char>> =
                sequences.iter().filter(|s| s.len() < 3).flat_map(|s| MARKS.iter().map(move |m| [s.as_slice(), &[*m]].concat())).collect();
            sequences.extend(longer);
        }
        /// Every order of `rest`: the drawn orders shaping could give a cluster's marks.
        fn permutations(rest: Vec<usize>) -> Vec<Vec<usize>> {
            if rest.is_empty() {
                return vec![Vec::new()];
            }
            let mut all = Vec::new();
            for (k, first) in rest.iter().enumerate() {
                let mut others = rest.clone();
                others.remove(k);
                all.extend(permutations(others).into_iter().map(|tail| [vec![*first], tail].concat()));
            }
            all
        }
        let mut checked = 0;
        for marks in sequences.iter().filter(|s| !s.is_empty()) {
            let chars: Vec<char> = std::iter::once('ก').chain(marks.iter().copied()).collect();
            let text: String = chars.iter().collect();
            for order in permutations((0..marks.len()).collect()) {
                // The letter stays first; the marks are drawn in `order`.
                let drawn: Vec<char> = std::iter::once('ก').chain(order.iter().map(|i| marks[*i])).collect();
                for substituted in 0u32..(1 << (marks.len() + 1)) {
                    let ids: Vec<u32> = drawn
                        .iter()
                        .enumerate()
                        .map(|(i, c)| match i {
                            i if substituted & (1 << i) != 0 => 900 + i as u32,
                            0 => 10,
                            _ => nominal(*c).map_or(0, |g| g.to_u32()),
                        })
                        .collect();
                    let mut glyphs = cluster(&text, &ids);
                    split_clusters(&mut glyphs, &vec![0; ids.len()], nominal);
                    if glyphs.iter().skip(1).any(|g| !g.text.is_empty()) {
                        for (glyph, truth) in glyphs.iter().zip(&drawn) {
                            assert_eq!(glyph.text, truth.to_string(), "{text:?} drawn {drawn:?}, substituted {substituted:b}");
                        }
                    } else {
                        assert_eq!(glyphs[0].text, text);
                    }
                    checked += 1;
                }
            }
        }
        assert!(checked > 60_000, "{checked}");
    }

    /// Malformed runs never panic: no glyphs, and fewer cluster values than glyphs (the glyphs past
    /// them are left as shaped).
    #[test]
    fn split_clusters_takes_empty_and_short_cluster_lists() {
        let nominal = |_| None;
        split_clusters(&mut [], &[], nominal);
        let mut glyphs = cluster("ที่", &[11, 20, 21]);
        split_clusters(&mut glyphs, &[], nominal);
        split_clusters(&mut glyphs, &[5], nominal);
        assert_eq!(glyphs.iter().map(|g| g.text.as_str()).collect::<Vec<_>>(), ["ที่", "", ""]);
    }

    /// A cluster of many marks (one letter with 20,000 tone marks, within the input limit) looks
    /// each character's own glyph up once, not once per glyph, so it can't stall shaping.
    #[test]
    fn a_cluster_of_many_marks_looks_each_character_up_once() {
        let text: String = std::iter::once('ก').chain(std::iter::repeat_n('\u{e48}', 20_000)).collect();
        let mut ids = vec![10];
        ids.extend(std::iter::repeat_n(21, 20_000));
        let mut glyphs = cluster(&text, &ids);
        let clusters = vec![0; glyphs.len()];
        let lookups = std::cell::Cell::new(0usize);
        split_clusters(&mut glyphs, &clusters, |c| {
            lookups.set(lookups.get() + 1);
            Some(GlyphId::new(if c == 'ก' { 10 } else { 21 }))
        });
        assert_eq!(glyphs.last().map(|g| g.text.as_str()), Some("\u{e48}"));
        assert!(lookups.get() <= ids.len(), "{} lookups for {} characters", lookups.get(), ids.len());
    }
}
