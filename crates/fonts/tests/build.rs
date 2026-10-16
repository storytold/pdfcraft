//! Exercise the actual build script in isolated processes; no environment mutation or font assets.
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Build {
    dir: PathBuf,
    executable: PathBuf,
}

impl Build {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("pdfcraft-font-build-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(dir.join("fonts")).unwrap();
        std::fs::create_dir_all(dir.join("out")).unwrap();
        // Only the include path is inspected. These empty placeholders are never parsed as fonts.
        std::fs::write(dir.join("fonts/face"), []).unwrap();
        let executable = dir.join(format!("build-script{}", std::env::consts::EXE_SUFFIX));
        let status = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args(["--edition=2024", concat!(env!("CARGO_MANIFEST_DIR"), "/build.rs"), "-o"])
            .arg(&executable)
            .status()
            .unwrap();
        assert!(status.success());
        Self { dir, executable }
    }

    fn manifest(&self, rows: &str) {
        std::fs::write(self.dir.join("fonts/manifest.txt"), rows).unwrap();
    }

    fn run(&self, arch: &str, policy: Option<&str>, required: bool, with_dir: bool) -> (String, String) {
        let mut command = Command::new(&self.executable);
        command.env("OUT_DIR", self.dir.join("out")).env("CARGO_CFG_TARGET_ARCH", arch);
        for name in ["CRAFT_FONTS_WEB", "CRAFT_FONTS_DIR", "CRAFT_FONTS_REQUIRED"] {
            command.env_remove(name);
        }
        if let Some(policy) = policy {
            command.env("CRAFT_FONTS_WEB", policy);
        }
        if with_dir {
            command.env("CRAFT_FONTS_DIR", &self.dir);
        }
        if required {
            command.env("CRAFT_FONTS_REQUIRED", "1");
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let log = String::from_utf8(output.stdout).unwrap();
        let generated = std::fs::read_to_string(self.dir.join("out/craft_fonts.rs")).unwrap_or_default();
        (log, generated)
    }
}

impl Drop for Build {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const MANIFEST: &str = "# family | style | path | scripts
BIZ UDPGothic | Regular | fonts/face | Jpan,Latn
BIZ UDPGothic | Bold | fonts/face | Jpan,Latn
Shippori Mincho | Regular | fonts/face | Jpan
Inter | Regular | fonts/face | Latn
Noto Sans Arabic | Bold | fonts/face | Latn, Arab
Noto Sans Telugu | Regular | fonts/face | Telu
Noto Sans Hebrew | Regular | fonts/face | Hebr
Noto Sans Thai | Regular | fonts/face | Thai
Custom Hans | Regular | fonts/face | Hans
Lowercase script | Regular | fonts/face | arab
";

#[test]
fn default_wasm_selection_is_unchanged() {
    let build = Build::new();
    build.manifest(MANIFEST);
    let (_, generated) = build.run("wasm32", None, true, true);
    let names: Vec<_> = generated.lines().filter(|line| line.contains("CraftFont {")).collect();
    assert_eq!(names.len(), 5, "{generated}");
    for family in ["BIZ UDPGothic", "Noto Sans Arabic", "Noto Sans Telugu", "Noto Sans Hebrew", "Noto Sans Thai"] {
        assert!(generated.contains(&format!("family: {family:?}")), "{generated}");
    }
    assert!(!generated.contains(r#"style: "Bold", scripts: &["Jpan""#));
}

#[test]
fn all_embeds_permitted_faces_excluded_by_default() {
    let build = Build::new();
    build.manifest(MANIFEST);
    let (_, default) = build.run("wasm32", None, true, true);
    assert!(!default.contains(r#"family: "Inter""#));
    let (log, all) = build.run("wasm32", Some("all"), true, true);
    assert!(all.contains(r#"family: "Inter""#), "{all}");
    assert_eq!(all.lines().filter(|line| line.contains("CraftFont {")).count(), 10);
    assert!(all.contains("include_bytes!"));
    assert!(log.contains("cargo::rerun-if-env-changed=CRAFT_FONTS_WEB"));
    assert!(log.contains("cargo::rerun-if-changed="));
}

#[test]
fn prohibited_families_stay_excluded_with_all() {
    let build = Build::new();
    // No prohibited font files are needed: the policy must skip them before touching a path.
    build.manifest(
        "Source Han Sans | Regular | missing | Jpan
Source Serif 4 | Regular | missing | Latn
Source Sans 3 | Regular | missing | Latn
Noto Sans CJK SC | Regular | missing | Hans
Noto Serif CJK TC | Regular | missing | Hant
Inter | Regular | fonts/face | Latn
",
    );
    for arch in ["wasm32", "x86_64"] {
        let (log, generated) = build.run(arch, Some("all"), false, true);
        assert!(!log.contains("cargo::error="), "{log}");
        assert_eq!(generated.lines().filter(|line| line.contains("CraftFont {")).count(), 1);
        assert!(generated.contains(r#"family: "Inter""#));
        assert_eq!(log.matches("(AGENTS.md §1.1)").count(), 5);
    }
}

#[test]
fn native_selection_is_unaffected() {
    let build = Build::new();
    build.manifest(MANIFEST);
    for arch in ["x86_64", "aarch64"] {
        let (_, default) = build.run(arch, None, false, true);
        let (_, all) = build.run(arch, Some("all"), false, true);
        assert_eq!(default, all);
        assert_eq!(all.lines().filter(|line| line.contains("CraftFont {")).count(), 10);
    }
}

#[test]
fn invalid_policy_is_always_a_clear_build_error() {
    let build = Build::new();
    build.manifest(MANIFEST);
    for arch in ["wasm32", "x86_64"] {
        for policy in ["", "ALL", "default", " all", "all ", "Inter"] {
            for with_dir in [false, true] {
                let (log, _) = build.run(arch, Some(policy), false, with_dir);
                assert!(log.contains("cargo::error="), "{arch} {policy:?}: {log}");
                assert!(log.contains("CRAFT_FONTS_WEB"), "{log}");
                assert!(log.contains("unset") && log.contains("all"), "{log}");
            }
        }
    }
}

#[test]
fn optional_and_required_input_behavior_is_preserved() {
    let build = Build::new();
    for policy in [None, Some("all")] {
        let (log, generated) = build.run("wasm32", policy, false, false);
        assert!(!log.contains("cargo::error="));
        assert!(!generated.contains("CraftFont {"));
        let (log, _) = build.run("wasm32", policy, false, true);
        assert!(log.contains("cargo::warning=building without craft-fonts"));
        let (log, _) = build.run("wasm32", policy, true, true);
        assert!(log.contains("cargo::error=CRAFT_FONTS_DIR="));
    }
}
