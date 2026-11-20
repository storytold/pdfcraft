use super::*;

fn glyph(plan: &TextPlan, word: usize, cluster: usize) -> &GlyphOutline {
    let (font, code) = plan.encoding(word)[cluster];
    &plan.fonts[font].glyphs[usize::from(code) - 1].1
}

#[test]
fn editable_nfd_turkish_clusters_match_composed_inter_ink_without_normalizing_source() {
    let pairs = [
        ("c\u{327}", 'ç'),
        ("C\u{327}", 'Ç'),
        ("g\u{306}", 'ğ'),
        ("G\u{306}", 'Ğ'),
        ("I\u{307}", 'İ'),
        ("o\u{308}", 'ö'),
        ("O\u{308}", 'Ö'),
        ("s\u{327}", 'ş'),
        ("S\u{327}", 'Ş'),
        ("u\u{308}", 'ü'),
        ("U\u{308}", 'Ü'),
        ("a\u{302}", 'â'),
        ("A\u{302}", 'Â'),
        ("i\u{302}", 'î'),
        ("I\u{302}", 'Î'),
        ("u\u{302}", 'û'),
        ("U\u{302}", 'Û'),
    ];
    let plan = TextPlan::for_editable_words(pairs.iter().map(|(text, _)| *text)).unwrap();
    let latin = document_latin_fonts(false)[0];
    for (i, (source, composed)) in pairs.iter().enumerate() {
        assert_eq!(plan.encoding(i).len(), 1, "one code owns the complete cluster");
        let (f, c) = plan.encoding(i)[0];
        assert_eq!(plan.cluster_text(f, c), Some(*source));
        assert_eq!(plan.fonts[f].family, latin.family, "NFD never needs optional craft-fonts");
        assert_eq!(glyph(&plan, i, 0), &craft_glyph(latin, *composed).unwrap());
    }
}

#[test]
fn editable_nfd_cluster_bakes_real_horizontal_and_vertical_offsets() {
    let text = "q\u{315}\u{301}\u{301}";
    let row = crate::latin_text::plan(text, false).unwrap();
    assert!(row.glyphs.iter().any(|g| g.position[0] != 0.0 && g.position[1] != 0.0));
    let face = skrifa::FontRef::new(crate::INTER_REGULAR).unwrap();
    let expected: Vec<Vec<[f64; 2]>> = row
        .glyphs
        .iter()
        .flat_map(|g| {
            crate::script::bounded_outline(&face, g.id, 0.0)
                .unwrap()
                .contours
                .into_iter()
                .map(|c| c.into_iter().map(|p| [p[0] + g.position[0], p[1] + g.position[1]]).collect())
        })
        .collect();
    let plan = TextPlan::for_editable_words([text]).unwrap();
    assert_eq!(glyph(&plan, 0, 0).contours, expected);
    assert_eq!(glyph(&plan, 0, 0).width, row.advance);
    let stacked = format!("q{}", "\u{301}".repeat(12));
    let high = TextPlan::for_editable_words([stacked.as_str()]).unwrap();
    assert!(glyph(&high, 0, 0).bbox[3] > 1.0, "a tall stack needs a dynamic FontBBox");
}

#[test]
fn editable_nfd_keeps_ascii_and_distinct_cluster_spellings_and_reuses_only_exact_sources() {
    let words = ["AbC\u{327}g\u{306}Z", "C\u{327}", "C\u{301}", "C\u{327}"];
    let plan = TextPlan::for_editable_words(words).unwrap();
    for (i, source) in words.iter().enumerate() {
        let decoded: String = plan.encoding(i).iter().map(|&(f, c)| plan.cluster_text(f, c).unwrap()).collect();
        assert_eq!(&decoded, source);
    }
    assert_ne!(plan.encoding(1), plan.encoding(2));
    assert_eq!(plan.encoding(1), plan.encoding(3));
    assert_eq!(plan.encoding(0).len(), 5, "ASCII plus marked clusters share one word plan");
}

#[test]
fn editable_nfd_refuses_orphan_unsupported_and_oversized_clusters() {
    let maximum = format!("q{}", "\u{301}".repeat(crate::latin_text::MAX_CLUSTER_SCALARS - 1));
    assert!(TextPlan::for_editable_words([maximum.as_str()]).is_ok());
    let excess = format!("{maximum}\u{301}");
    let row = format!("{}a\u{301}", "a".repeat(crate::latin_text::MAX_ROW_BYTES));
    for source in ["\u{306}", "\u{1ab0}", "a\u{1ab0}", "\u{306}a", "Ж\u{306}", "a\u{301}\n", excess.as_str(), row.as_str()] {
        assert!(TextPlan::for_editable_words([source]).is_err(), "{source:?}");
    }
    let glyph_overflow = format!("{}a\u{301}", "a".repeat(65_534));
    assert!(TextPlan::for_editable_words([glyph_overflow.as_str()]).is_err());
}

fn distinct_clusters(count: usize) -> Vec<String> {
    (0..count)
        .map(|i| {
            let mut cluster = String::from("q");
            for bit in 0..12 {
                cluster.push(if i & (1 << bit) == 0 { '\u{300}' } else { '\u{301}' });
            }
            cluster
        })
        .collect()
}

#[test]
fn editable_nfd_font_chunks_and_outline_budget_remain_bounded() {
    let sources = distinct_clusters(250);
    let plan = TextPlan::for_editable_words(sources.iter().map(String::as_str)).unwrap();
    assert!(plan.fonts.len() >= 2, "cluster codes split before overflowing a byte");
    assert!(plan.fonts.iter().all(|f| f.glyphs.len() <= MAX_TYPE3_GLYPHS));
    for (i, source) in sources.iter().enumerate() {
        let (f, c) = plan.encoding(i)[0];
        assert_eq!(plan.cluster_text(f, c), Some(source.as_str()));
    }
    let excessive = distinct_clusters(MAX_PLAN_GLYPHS + 1);
    assert!(TextPlan::for_editable_words(excessive.iter().map(String::as_str)).is_err());
}

#[test]
fn editable_nfd_does_not_change_legacy_scalar_words_or_public_for_words() {
    let words = ["ASCII", "123", "€", "Привет", "Ελλάδα"];
    let old = TextPlan::for_words(words);
    let new = TextPlan::for_editable_words(words).unwrap();
    for i in 0..words.len() {
        assert_eq!(old.encoding(i), new.encoding(i));
        assert_eq!(old.advance_of(old.encoding(i)), new.advance_of(new.encoding(i)));
        assert!(new.shaped_bounds(i).is_none());
    }
    for (a, b) in old.fonts.iter().zip(&new.fonts) {
        assert_eq!(a.glyphs, b.glyphs);
    }
    let public_scalar = TextPlan::for_words(["ASCII", "café", "Ü", "Ç", "Å"]);
    assert!(public_scalar.fonts.is_empty(), "the public scalar API must retain WinAnsi output");
    assert!(TextPlan::for_editable_words(["\u{1f600}"]).is_err(), "editable output cannot silently substitute");
    assert!(TextPlan::for_words(["a\u{301}"]).missing() > 0, "legacy scalar planner remains guarded");
    if !crate::CRAFT_FONTS.is_empty() {
        let old = TextPlan::for_words(["日本"]);
        let new = TextPlan::for_editable_words(["日本"]).unwrap();
        assert_eq!(old.encoding(0), new.encoding(0));
        assert_eq!(old.fonts[0].glyphs, new.fonts[0].glyphs);
    }
}

#[test]
fn editable_nfd_checked_latin_face_is_canonical_with_optional_fonts() {
    for (nfc, nfd) in [("ÇağrıİŞÜ", "C\u{327}ag\u{306}rıI\u{307}S\u{327}U\u{308}"), ("ÜÇCaféÅ", "U\u{308}C\u{327}Cafe\u{301}A\u{30a}")] {
        let nfc = TextPlan::for_editable_words([nfc]).unwrap();
        let nfd = TextPlan::for_editable_words([nfd]).unwrap();
        for plan in [&nfc, &nfd] {
            assert!(!plan.fonts.is_empty() && plan.fonts.iter().all(|f| f.family == document_latin_fonts(false)[0].family));
        }
        let outlines =
            |plan: &TextPlan| plan.encoding(0).iter().map(|&(f, c)| plan.fonts[f].glyphs[usize::from(c) - 1].1.clone()).collect::<Vec<_>>();
        assert_eq!(outlines(&nfc), outlines(&nfd), "optional CJK coverage must not choose a different canonical Latin appearance");
        assert_eq!(nfc.advance_of(nfc.encoding(0)), nfd.advance_of(nfd.encoding(0)));
    }
}

#[test]
fn editable_nfd_works_with_no_optional_document_faces() {
    let faces = document_latin_fonts(false);
    let words = ["C\u{327}ag\u{306}rı", "I\u{307}stanbul", "S\u{327}is\u{327}li", "Çağrı", "Привет", "Ελλάδα", "ASCII"];
    let plan = TextPlan::editable_with_faces(words, &faces).unwrap();
    assert_eq!(plan.missing(), 0);
    assert!(plan.fonts.iter().all(|f| f.family == faces[0].family));
    assert!(TextPlan::editable_with_faces(["日本"], &faces).is_err(), "without optional fonts unsupported Japanese stays an explicit error");
}
