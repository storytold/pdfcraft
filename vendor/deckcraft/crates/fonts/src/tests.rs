use super::*;

fn any_face() -> std::sync::Arc<FontFace> {
    FontDb::global().face(DEFAULT_FAMILY, "Regular")
}

#[test]
fn emoji_clusters_are_recognised() {
    let c = |s: &str| s.chars().collect::<Vec<char>>();
    // As PowerPoint draws them: emoji-presentation characters (⭐ ✅ ☔ ⌚ ⏩) in colour even
    // without a selector, text-presentation ones (♥ ✔ ☎ ⬆ ▶) as text unless U+FE0F asks, and
    // keycaps always as text.
    for yes in ["😀", "👍🏽", "❤\u{FE0F}", "👨\u{200D}👩\u{200D}👧", "🇯🇵", "🏳\u{FE0F}\u{200D}🌈", "⭐", "✅", "☔", "⌚", "⏩", "🀄"]
    {
        assert!(is_emoji_cluster(&c(yes)), "{yes:?}");
    }
    for no in ["a", "1", "❤", "☺\u{FE0E}", "😀\u{FE0E}", "字", "→", "♥", "✔", "☎", "⬆", "▶", "1\u{FE0F}\u{20E3}", "#\u{20E3}"] {
        assert!(!is_emoji_cluster(&c(no)), "{no:?}");
    }
}

#[test]
fn colour_fonts_are_recognised() {
    let db = FontDb::with_font_dirs(Vec::new());
    assert!(db.add_font(include_bytes!("../../render/tests/fixtures/colr-squares.ttf").to_vec()) > 0);
    assert!(db.face("DeckCraft Colr Squares", "Regular").is_color());
    assert!(!fontdb::last_resort_face().is_color());
}

#[test]
fn last_resort_face_parses() {
    let f = fontdb::last_resort_face();
    assert_eq!(f.family, fontdb::LAST_RESORT_FAMILY);
    assert!(f.upem > 0.0);
}

#[test]
fn line_split_follows_the_windows_ascent_and_descent() {
    use skrifa::raw::TableProvider;
    let f = fontdb::last_resort_face();
    let font = skrifa::FontRef::new(f.data()).unwrap();
    let os2 = font.os2().unwrap();
    let (a, d) = (os2.us_win_ascent() as f64, os2.us_win_descent() as f64);
    assert!((f.line_split - a / (a + d)).abs() < 1e-9, "{} vs {}", f.line_split, a / (a + d));
    assert!(f.line_split > 0.5 && f.line_split < 1.0);
}

#[test]
fn some_face_always_resolves() {
    let db = FontDb::with_font_dirs(Vec::new());
    db.set_system_fallback(false);
    let f = db.face("No Such Font Anywhere", "Regular");
    assert!(!f.family.is_empty());
    assert!(!shape(&f, "Hello", &[], |c| c).is_empty());
}

#[test]
fn filtered_fallback_continues_after_a_rejected_loaded_face() {
    let db = FontDb::with_font_dirs(Vec::new());
    db.set_system_fallback(false);
    assert_eq!(db.add_font(include_bytes!("../tests/fixtures/legacy-regular.ttf").to_vec()), 1);
    let first = db.fallback_for('A', u32::MAX).unwrap();
    let accepted_family = if first.family == LAST_RESORT_FAMILY { "DeckCraft Legacy" } else { LAST_RESORT_FAMILY };
    let fallback = db.fallback_for_filtered('A', u32::MAX, |f| f.family == accepted_family).unwrap();
    assert_eq!(fallback.family, accepted_family);
    assert!(fallback.covers('A'));
    assert_ne!(fallback.id(), first.id());
    assert_eq!(db.fallback_for('A', u32::MAX).unwrap().id(), first.id(), "document fallback order is unchanged");
    assert!(db.fallback_for_filtered('A', fallback.id(), |f| f.family == accepted_family).is_none(), "the excluded face stays excluded");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn filtered_fallback_loads_an_eligible_system_face_and_does_not_cache_policy_misses() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let db = FontDb::with_font_dirs(vec![dir]);
    let first = db.fallback_for('A', u32::MAX).unwrap();
    assert_ne!(first.family, "DeckCraft Legacy");
    assert!(db.fallback_for_filtered('A', u32::MAX, |_| false).is_none());
    assert!(db.fallback_for('A', u32::MAX).is_some(), "an interface policy miss does not hide document glyphs");
    let accepted = db.fallback_for_filtered('A', u32::MAX, |f| f.family == "DeckCraft Legacy").unwrap();
    assert_eq!(accepted.family, "DeckCraft Legacy");
    assert!(accepted.covers('A'));
}

#[test]
fn concurrent_lookups_all_find_an_installed_family() {
    // Threads resolving an installed family nobody had loaded yet used to fall back to
    // another face when a different thread finished loading it first, so the same text was
    // laid out in different fonts depending on timing.
    let dir = std::env::temp_dir().join(format!("deckcraft-fonts-concurrent-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Hack-Regular.ttf"), epaint_default_fonts::HACK_REGULAR).unwrap();
    for _ in 0..20 {
        let db = FontDb::with_font_dirs(vec![dir.clone()]);
        db.set_system_fallback(false);
        let start = std::sync::Barrier::new(8);
        let got: Vec<String> = std::thread::scope(|s| {
            let threads: Vec<_> = (0..8)
                .map(|_| {
                    s.spawn(|| {
                        start.wait();
                        db.face("Hack", "Regular").family.clone()
                    })
                })
                .collect();
            threads.into_iter().map(|t| t.join().unwrap()).collect()
        });
        assert!(got.iter().all(|f| f == "Hack"), "{got:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn windows_family_names_find_their_faces() {
    // Documents name fonts by their Windows family ("Segoe UI Semibold"); the font's
    // typographic family is "Segoe UI" with a "Semibold" style. The fixture is named the same way.
    let check = |db: &FontDb| {
        assert!(db.has_family("DeckCraft Legacy Semibold"));
        for style in ["Regular", "Bold"] {
            let f = db.face("DeckCraft Legacy Semibold", style);
            assert_eq!((f.family.as_str(), f.style.as_str()), ("DeckCraft Legacy", "Semibold"), "{style}");
        }
        assert_eq!(db.document_face("DeckCraft Legacy Semibold", "Regular").family, "DeckCraft Legacy", "installed, so not Calibri");
        // The typographic name still works.
        assert_eq!(db.face("DeckCraft Legacy", "Semibold").style, "Semibold");
    };
    // Loaded directly…
    let db = FontDb::with_font_dirs(Vec::new());
    db.set_system_fallback(false);
    assert_eq!(db.add_font(include_bytes!("../tests/fixtures/legacy-semibold.ttf").to_vec()), 1);
    check(&db);
    // …and found by the system font scan under its Windows name, then loaded.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let db = FontDb::with_font_dirs(vec![dir.clone()]);
        db.set_system_fallback(false);
        check(&db);
        // Looking up the Windows name loads the Semibold file only; a later lookup of the
        // family's Regular must still load and pick the Regular file, not settle for Semibold.
        let db = FontDb::with_font_dirs(vec![dir]);
        db.set_system_fallback(false);
        assert_eq!(db.face("DeckCraft Legacy Semibold", "Regular").style, "Semibold");
        let regular = db.face("DeckCraft Legacy", "Regular");
        assert_eq!((regular.style.as_str(), regular.weight), ("Regular", 400.0));
    }
}

#[test]
fn variable_font_instances_answer_to_their_windows_names() {
    // Windows lists a variable font's named instances other than Regular, Italic, Bold and
    // Bold Italic as families of their own, and documents name them that way ("Montserrat
    // ExtraBold"). The bundled Montserrat is variable, so these names found no face and drew
    // in Inter Regular, even with the family's static files installed (their faces are
    // skipped as already present).
    let db = FontDb::with_font_dirs(Vec::new());
    db.set_system_fallback(false);
    if !db.is_loaded("Montserrat") {
        return; // built without craft-fonts
    }
    for (name, style, weight, italic) in [
        ("Montserrat ExtraBold", "Regular", 800.0, false),
        ("Montserrat SemiBold", "Regular", 600.0, false),
        ("Montserrat Medium", "Regular", 500.0, false),
        ("Montserrat Medium", "Italic", 500.0, true),
        ("Montserrat Thin", "Bold", 100.0, false),
    ] {
        assert!(db.has_family(name), "{name}");
        let f = db.face(name, style);
        assert_eq!((f.family.as_str(), f.weight, f.italic), ("Montserrat", weight, italic), "{name} {style}");
        assert_eq!(db.document_face(name, style).family, "Montserrat", "{name}: installed, so not Calibri");
    }
    // Regular and Bold are the family itself, not families of their own.
    assert!(!db.has_family("Montserrat Regular"));
    assert!(!db.has_family("Montserrat Bold"));
    assert_eq!(db.face("Montserrat", "Bold").weight, 700.0);
}

#[test]
fn faces_without_outlines_are_not_cataloged() {
    // Rename the Semibold's `glyf` table, as in fonts whose glyphs are bitmaps or are stored in
    // Apple's `hvgl` table. The face still maps "A", but its outline is empty. The new tag
    // keeps the table directory sorted.
    let mut semibold = include_bytes!("../tests/fixtures/legacy-semibold.ttf").to_vec();
    let tables = u16::from_be_bytes([semibold[4], semibold[5]]) as usize;
    let glyf = (0..tables).map(|i| 12 + 16 * i).find(|&r| &semibold[r..r + 4] == b"glyf").unwrap();
    semibold[glyf..glyf + 4].copy_from_slice(b"gxyz");
    let loaded = FontDb::with_font_dirs(Vec::new());
    assert_eq!(loaded.add_font(semibold.clone()), 1);
    let face = loaded.face("DeckCraft Legacy", "Semibold");
    assert!(face.covers('A'));
    assert!(loaded.outline(&face, face.glyph_for('A')).elements().is_empty());
    // The system font scan lists the Regular, which has outlines, and leaves out the Semibold
    // under both of its names.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let dir = std::env::temp_dir().join(format!("deckcraft-fonts-outlines-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("legacy-semibold.ttf"), &semibold).unwrap();
        std::fs::write(dir.join("legacy-regular.ttf"), include_bytes!("../tests/fixtures/legacy-regular.ttf")).unwrap();
        let db = FontDb::with_font_dirs(vec![dir.clone()]);
        db.set_system_fallback(false);
        assert_eq!(db.styles("DeckCraft Legacy"), ["Regular"]);
        assert!(!db.has_family("DeckCraft Legacy Semibold"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn missing_document_fonts_are_drawn_in_calibri() {
    // PowerPoint draws a missing family in Calibri, whatever its panose or the theme says.
    let db = FontDb::global();
    for missing in ["Qqq Nonexistent", "Avenir Next Qqq", "", "Arimo Qqq"] {
        let f = db.document_face(missing, "Bold");
        assert_eq!(f.family, db.face(MISSING_FONT_FAMILY, "Bold").family, "{missing:?}");
    }
    // PowerPoint's own twins and Windows' substitutes, when the font itself is missing.
    for (missing, twin) in [
        ("Liberation Sans", "Arial"),
        ("Liberation Serif", "Times New Roman"),
        ("Arial CE", "Arial"),
        ("Times New Roman CYR", "Times New Roman"),
        ("Courier New CE", "Courier New"),
    ] {
        if !db.has_family(missing) {
            assert_eq!(db.document_face(missing, "Regular").family, db.face(twin, "Regular").family, "{missing}");
        }
    }
    // Metric-compatible stand-ins and DeckCraft's default keep their own fallbacks.
    // So do Office's cloud fonts, which PowerPoint downloads and draws as themselves.
    for kept in ["Arial", "Times New Roman", "Cambria", "Courier New", FALLBACK_FAMILY, "Aptos", "Aptos Display", "Grandview"] {
        assert_eq!(db.document_face(kept, "Regular").family, db.face(kept, "Regular").family, "{kept}");
    }
}

#[test]
fn office_fonts_have_substitutes() {
    assert!(substitutes("Calibri").contains(&"Carlito"));
    assert!(substitutes("Cambria").contains(&"Caladea"));
    assert!(substitutes("Times New Roman").contains(&"Liberation Serif"));
    assert!(substitutes("Aptos").contains(&"Inter"));
    assert!(substitutes("zzz").is_empty());

    for mono in [
        "Courier New",
        "Cousine",
        "Consolas",
        "Menlo",
        "Monaco",
        "Source Code Pro",
        "JetBrains Mono",
        "Cascadia Code",
        "Cascadia Code PL",
        "Cascadia Mono",
        "Fira Code",
        "Inconsolata",
        "DejaVu Sans Mono",
        "Roboto Mono",
        "Hack",
        "Ubuntu Mono",
        "Custom Monospace",
        "Fantasque Sans Mono",
    ] {
        assert!(substitutes(mono).contains(&"Liberation Mono"), "{mono} should have Liberation Mono substitute");
        assert!(is_monospace_family(mono), "{mono} should be recognized as monospace");
    }

    for prop in ["Calibri", "Arial", "Times New Roman", "Inter", "Monotype Corsiva", "Monoton", "Monotype Baskerville", "Code Pro", "zzz"] {
        assert!(!is_monospace_family(prop), "{prop} should not be recognized as monospace");
    }

    // Explicit checks requested in review: Consolas, Courier New, Fira Code, Monotype Corsiva, Monoton,
    // and ensuring proportional families maintain their unchanged substitutes.
    assert!(is_monospace_family("Consolas"));
    assert!(is_monospace_family("Courier New"));
    assert!(is_monospace_family("Fira Code"));
    assert!(!is_monospace_family("Monotype Corsiva"));
    assert!(!is_monospace_family("Monoton"));

    assert_eq!(substitutes("Calibri"), &["Carlito", "Liberation Sans", "Arial", "Inter"]);
    assert_eq!(substitutes("Arial"), &["Liberation Sans", "Arial", "Helvetica Neue", "Helvetica", "Inter"]);
    assert_eq!(substitutes("Times New Roman"), &["Liberation Serif", "Times New Roman", "Times", "Merriweather"]);
    assert_eq!(substitutes("Georgia"), &["Merriweather", "Liberation Serif", "Georgia"]);
    assert!(substitutes("Monotype Corsiva").is_empty());
    assert!(substitutes("Monoton").is_empty());
}

#[test]
fn missing_monospace_fonts_keep_monospace_fallback() {
    let db = FontDb::global();
    let mono_sub = db.face("Unavailable JetBrains Mono", "Regular").family.clone();
    let calibri_target = db.face(MISSING_FONT_FAMILY, "Regular").family.clone();

    assert!(is_monospace_family(&mono_sub), "substitute {mono_sub} must be monospace");

    // Cousine is metric-compatible with Courier New / Liberation Mono and must not fall back to Calibri.
    if !db.has_family("Cousine") {
        assert_eq!(db.document_face("Cousine", "Regular").family, mono_sub);
    }

    // Monospace fonts missing from the system must resolve to a monospace substitute.
    for mono in ["Unavailable JetBrains Mono", "Custom App Code", "Unknown Space Mono", "Mock Terminal Font"] {
        let f = db.document_face(mono, "Regular");
        assert_eq!(f.family, mono_sub, "{mono}");
        assert!(is_monospace_family(&f.family), "{mono} must resolve to a monospace family");
        if mono_sub != calibri_target {
            assert_ne!(f.family, calibri_target, "{mono} must not fall back to Calibri");
        }
    }
}

#[test]
fn shaping_produces_clusters_and_advances() {
    let face = any_face();
    let g = shape(&face, "Hello", &[], |c| c);
    assert_eq!(g.len(), 5);
    assert_eq!(g.iter().map(|g| g.cluster).collect::<Vec<_>>(), vec![0, 1, 2, 3, 4]);
    assert!(g.iter().all(|g| g.x_advance > 0 && g.gid != 0));
}

#[test]
fn mapping_keeps_clusters() {
    let face = any_face();
    let g = shape(&face, "ab", &[], |c| c.to_ascii_uppercase());
    assert_eq!(g[0].gid, face.glyph_for('A'));
    assert_eq!(g[1].cluster, 1);
}

#[test]
fn outlines_and_metrics() {
    let db = FontDb::global();
    let face = any_face();
    let o = db.outline(&face, face.glyph_for('O'));
    assert!(!o.elements().is_empty());
    assert!(face.ascent > 0.0 && face.descent > 0.0);
    assert!(feature("abc").is_none());
    assert!(feature("liga").is_some());
}

/// When the Latin build input exists, the runtime catalogue carries the expected Latin faces;
/// required builds also assert their presence.
#[test]
fn craft_fonts_latin_when_present() {
    let required = option_env!("CRAFT_FONTS_REQUIRED").is_some();
    if !CRAFT_FONTS.iter().any(|f| f.family == "Inter") {
        assert!(!required, "CRAFT_FONTS_REQUIRED must embed Inter rather than skip the optional Latin manifest");
        eprintln!("skipped: built without craft-fonts' Latin manifest (set CRAFT_FONTS_DIR)");
        return;
    }
    let inter = |style: &str| CRAFT_FONTS.iter().find(|f| f.family == "Inter" && f.style == style);
    let (regular, bold, variable) = (inter("Regular"), inter("Bold"), inter("Variable"));
    if required {
        assert!(
            variable.is_some() || (regular.is_some() && bold.is_some()),
            "CRAFT_FONTS_REQUIRED must embed Inter Regular and Bold (or a Variable face)"
        );
    }
    let db = FontDb::with_font_dirs(Vec::new());
    db.set_system_fallback(false);
    assert_eq!(db.face("Inter", "Regular").family, "Inter");
    assert_eq!(db.face("Aptos", "Regular").family, "Inter");
    if let (Some(regular), Some(bold)) = (regular, bold) {
        // The build input carries one entry per face; real static Bold must not be the Regular
        // file relabelled in the manifest.
        assert_ne!(regular.bytes, bold.bytes, "Inter Bold must be a distinct face file from Regular");
        let (r, b) = (db.face("Inter", "Regular"), db.face("Inter", "Bold"));
        assert_eq!((r.family.as_str(), r.style.as_str()), ("Inter", "Regular"));
        assert_eq!((b.family.as_str(), b.style.as_str()), ("Inter", "Bold"));
        assert_ne!(r.id(), b.id(), "Inter Bold must load as its own face");
        assert!(b.weight > r.weight, "Inter Bold weight {} is not above Regular {}", b.weight, r.weight);
    }
    if CRAFT_FONTS.iter().any(|f| f.family == "Carlito") {
        assert_eq!(db.face("Calibri", "Bold").family, "Carlito");
    }
}

/// An Embedded OpenType header of `version` around `font`, with the font at the end.
fn wrap_eot(font: &[u8], version: u32) -> Vec<u8> {
    let mut h = vec![0u8; 82];
    let total = 82 + font.len();
    h[0..4].copy_from_slice(&(total as u32).to_le_bytes());
    h[4..8].copy_from_slice(&(font.len() as u32).to_le_bytes());
    h[8..12].copy_from_slice(&version.to_le_bytes());
    h[34..36].copy_from_slice(&0x504Cu16.to_le_bytes());
    h.extend_from_slice(font);
    h
}

#[test]
fn embedded_open_type_yields_the_font() {
    let font = epaint_default_fonts::UBUNTU_LIGHT;
    assert_eq!(embedded_sfnt(font), Some(font));
    assert_eq!(embedded_sfnt(&wrap_eot(font, 0x0002_0001)), Some(font));
    assert_eq!(embedded_sfnt(&wrap_eot(font, 0x0001_0000)), Some(font));
    assert!(embedded_sfnt(b"not a font file").is_none());
    assert!(embedded_sfnt(&wrap_eot(b"hello world!!", 0x0002_0002)).is_none());
    let mut truncated = wrap_eot(font, 0x0002_0001);
    truncated.truncate(40);
    assert!(embedded_sfnt(&truncated).is_none());
}

/// A FIFO, a folder and a link loop dressed up as font files: the system scan skips them
/// instead of blocking forever on the FIFO (start-up hung when one sat in `~/.fonts`), and
/// still finds the real font beside them.
#[cfg(unix)]
#[test]
fn the_font_scan_skips_fifos_folders_and_link_loops() {
    let dir = std::env::temp_dir().join(format!("deckcraft-fontscan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::copy(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy-semibold.ttf"), dir.join("sub/real.ttf")).unwrap();
    let fifo = std::process::Command::new("mkfifo").arg(dir.join("trap.ttf")).status().is_ok_and(|s| s.success());
    std::fs::create_dir(dir.join("folder.ttf")).unwrap();
    std::fs::write(dir.join("empty.otf"), b"").unwrap();
    std::os::unix::fs::symlink(&dir, dir.join("sub/loop")).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let scan_dir = dir.clone();
    std::thread::spawn(move || {
        let db = FontDb::with_font_dirs(vec![scan_dir]);
        let _ = tx.send(db.load_system_fonts());
    });
    let n = rx.recv_timeout(std::time::Duration::from_secs(30)).expect("the font scan hung");
    assert!(n >= 1, "the real font was found");
    assert!(fifo || n >= 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn disabling_system_fallback_gates_automatic_family_loads() {
    let db = FontDb::with_font_dirs(Vec::new());
    assert!(cfg!(target_arch = "wasm32") || db.system_fallback_enabled());
    db.set_system_fallback(false);
    assert!(!db.system_fallback_enabled());
}
