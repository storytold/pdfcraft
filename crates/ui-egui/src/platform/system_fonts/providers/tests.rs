use super::*;
use egui::epaint::text::{Fonts, TextOptions};
use egui::{FontDefinitions, FontFamily, FontId};

const INTER: &[u8] = include_bytes!("../../../../../../assets/fonts/Inter-Regular.ttf");

fn fixture_faces() -> Vec<FontData> {
    vec![FontData::from_static(INTER)]
}

fn fallback() -> Arc<Unifont> {
    Arc::new(Unifont { load: fixture_faces, faces: OnceLock::new() })
}

struct System(bool);

impl FontProvider for System {
    fn font_for(&self, request: &FallbackRequest<'_>) -> Option<FontInsert> {
        self.0.then(|| {
            FontInsert::new(
                "test-system",
                FontData::from_static(INTER),
                vec![InsertFontFamily { family: request.family.clone(), priority: FontPriority::Lowest }],
            )
        })
    }
}

#[test]
fn system_font_wins_without_loading_last_resort() {
    let fallback = fallback();
    let mut fonts = Fonts::new(TextOptions::default(), FontDefinitions::empty()).with_font_providers(vec![Arc::new(System(true)), fallback.clone()]);
    assert!(fonts.has_glyph(&FontId::proportional(13.0), 'A'));
    assert!(fallback.faces.get().is_none());
    assert!(fonts.discovered_fonts().iter().any(|font| font.name == "test-system"));
}

#[test]
fn failed_system_lookup_uses_last_resort_in_every_requested_family() {
    let mut fonts = Fonts::new(TextOptions::default(), FontDefinitions::empty()).with_font_providers(vec![Arc::new(System(false)), fallback()]);
    for family in [FontFamily::Proportional, FontFamily::Monospace, FontFamily::Name("semibold".into())] {
        assert!(fonts.has_glyph(&FontId::new(13.0, family), 'A'));
    }
    assert!(fonts.discovered_fonts().iter().any(|font| font.name.starts_with("system-unifont:")));
}

#[test]
fn last_resort_rejects_missing_glyphs_empty_requests_and_invalid_faces() {
    let family = FontFamily::Proportional;
    let fallback = fallback();
    for cluster in ["", "힣"] {
        assert!(fallback.font_for(&FallbackRequest { cluster, family: &family }).is_none());
    }
    let invalid = Unifont { load: || vec![FontData::from_static(b"invalid")], faces: OnceLock::new() };
    assert!(invalid.font_for(&FallbackRequest { cluster: "A", family: &family }).is_none());
}

#[test]
fn unifont_names_include_versioned_opentype_and_upper_plane_files() {
    for name in ["unifont.ttf", "Unifont.OTF", "unifont-16.0.04.otf", "unifont_upper-16.0.04.otf", "unifont.otc"] {
        assert!(is_unifont(Path::new(name)), "{name}");
    }
    for name in ["OtherFont.otf", "unifont.pcf.gz", "unifont.txt", "unifonted.ttf"] {
        assert!(!is_unifont(Path::new(name)), "{name}");
    }
}

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("pdfcraft-system-provider-{}-{n}", std::process::id()));
        std::fs::create_dir_all(path.join("fonts")).unwrap();
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn last_resort_skips_invalid_oversized_and_duplicate_files() {
    let fixture = Fixture::new();
    let valid = fixture.0.join("fonts/unifont.otf");
    let invalid = fixture.0.join("fonts/unifont-broken.otf");
    let huge = fixture.0.join("fonts/unifont-huge.otf");
    std::fs::write(&valid, INTER).unwrap();
    std::fs::write(&invalid, b"invalid").unwrap();
    std::fs::File::create(&huge).unwrap().set_len(super::super::MAX_BYTES + 1).unwrap();
    let loaded = load_files(vec![invalid, huge, valid.clone(), valid]);
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].bytes(), INTER);
}

/// Run in fresh processes: fontconfig and craft-ui cache the process's first font inventory.
/// A renamed, existing bundled face is a synthetic installed font; no host fonts are needed.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn installed_fonts_work_without_known_filenames_and_honour_opt_out() {
    const CHILD: &str = "PDFCRAFT_PROVIDER_TEST_CHILD";
    if let Ok(mode) = std::env::var(CHILD) {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        // Remove bundled/script-specific faces to test only the installed providers.
        ctx.set_fonts(FontDefinitions::empty());
        let output = ctx.run_ui(Default::default(), |ui| {
            let covered = ui.ctx().fonts_mut(|fonts| fonts.has_glyph(&FontId::proportional(13.0), 'A'));
            assert_eq!(covered, mode == "enabled", "installed providers: {mode}");
        });
        output.drop_without_applying_deltas();
        return;
    }
    let fixture = Fixture::new();
    std::fs::write(fixture.0.join("fonts/AnArbitraryInstalledName.otf"), INTER).unwrap();
    let config = fixture.0.join("fonts.conf");
    std::fs::write(
        &config,
        format!("<fontconfig><dir>{}/fonts</dir><cachedir>{}/cache</cachedir></fontconfig>", fixture.0.display(), fixture.0.display()),
    )
    .unwrap();
    for (mode, setting) in [("enabled", "1"), ("disabled", "0")] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "platform::system_fonts::providers::tests::installed_fonts_work_without_known_filenames_and_honour_opt_out",
                "--nocapture",
            ])
            .env(CHILD, mode)
            .env("FONTCONFIG_FILE", &config)
            .env("PDFCRAFT_SYSTEM_FONTS", setting)
            .output()
            .unwrap();
        assert!(output.status.success(), "{mode}: {}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
}
