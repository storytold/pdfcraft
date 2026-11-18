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
        eprintln!("checking installed-font discovery ({mode})");
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        // Remove bundled/script-specific faces to test only the installed providers.
        ctx.set_fonts(FontDefinitions::empty());
        // Check backend availability independently: a real lookup regression must not skip.
        let enabled = mode == "enabled";
        if enabled && fontconfig_sys::statics::LIB_RESULT.is_err() {
            eprintln!("skipping installed-font discovery: optional libfontconfig is unavailable");
            return;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let mut covered = false;
            ctx.run_ui(Default::default(), |ui| {
                covered = ui.ctx().fonts_mut(|fonts| fonts.has_glyph(&FontId::proportional(13.0), 'A'));
            })
            .drop_without_applying_deltas();
            if !enabled {
                assert!(!covered, "opt-out must remove every provider");
                break;
            }
            if covered {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "installed font was not discovered");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
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
        // Successful children can skip optional discovery: keep their diagnostics
        // available in the parent test's output (with --nocapture or --show-output).
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.status.success(), "{mode}: {}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn successful_installed_font_checks_forward_child_diagnostics() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "platform::system_fonts::providers::tests::installed_fonts_work_without_known_filenames_and_honour_opt_out", "--nocapture"])
        .env_remove("PDFCRAFT_PROVIDER_TEST_CHILD")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{}\n{stderr}", String::from_utf8_lossy(&output.stdout));
    for mode in ["enabled", "disabled"] {
        assert!(
            stderr.contains(&format!("checking installed-font discovery ({mode})")),
            "child diagnostics must survive a successful check: {stderr}"
        );
    }
}

#[test]
fn cached_unifont_does_not_hide_later_system_fonts() {
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = calls.clone();
    let system: Arc<dyn FontProvider> = Arc::new(move |r: &FallbackRequest<'_>| {
        seen.lock().unwrap().push(r.cluster.to_owned());
        (r.cluster == "B").then(|| FontInsert::new("test-system", FontData::from_static(INTER), vec![]))
    });
    let mut fonts = Fonts::new(TextOptions::default(), FontDefinitions::empty()).with_font_providers(vec![system, fallback()]);
    for family in [FontFamily::Proportional, FontFamily::Monospace, FontFamily::Name("semibold".into())] {
        for c in ['A', 'C', 'B', 'A', 'B'] {
            assert!(fonts.has_glyph(&FontId::new(13.0, family.clone()), c));
        }
    }
    assert_eq!(*calls.lock().unwrap(), vec!["A", "C", "B", "A", "C", "B", "A", "C", "B"]);
    assert!(fonts.discovered_fonts().iter().any(|font| font.name == "test-system"));
}

#[cfg(unix)]
#[test]
fn last_resort_skips_fifos_and_accepts_regular_symlinks() {
    use rustix::fs::{CWD, Mode, mkfifoat};
    let fixture = Fixture::new();
    let fifo = fixture.0.join("fonts/unifont-pipe.otf");
    let font = fixture.0.join("fonts/unifont.otf");
    let link = fixture.0.join("fonts/unifont-link.otf");
    mkfifoat(CWD, &fifo, Mode::RUSR | Mode::WUSR).unwrap();
    std::fs::write(&font, INTER).unwrap();
    std::os::unix::fs::symlink(&font, &link).unwrap();
    let (send, recv) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // Exercise the nonblocking open itself, as if the path changed after metadata().
        assert!(!open_font(&fifo).unwrap().metadata().unwrap().is_file());
        let loaded = load_files(vec![fifo, link, font]);
        send.send(loaded.len()).unwrap();
    });
    assert_eq!(recv.recv_timeout(std::time::Duration::from_secs(3)).unwrap(), 1);
}
