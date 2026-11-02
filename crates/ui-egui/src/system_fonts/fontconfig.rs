//! Discover font files in configured directories, including Nix store paths.
//!
//! Only the existing fallback filenames are selected by the caller. Font bytes are still
//! checked for size and glyph coverage there; no font is copied or embedded.

use std::collections::HashSet;
use std::io::Read as _;
use std::path::{Path, PathBuf};

// A configured directory may contain unrelated files or symlink cycles. Bound the walk.
const MAX_ENTRIES: usize = 32_768;
const MAX_DEPTH: usize = 16;
const MAX_CONFIGS: usize = 256;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

#[derive(Default)]
struct Directories {
    dirs: Vec<PathBuf>,
    seen: HashSet<PathBuf>,
    entries: usize,
}

impl Directories {
    fn include(&mut self, path: &Path, depth: usize) {
        if depth > MAX_DEPTH || self.seen.len() >= MAX_CONFIGS {
            return;
        }
        let Ok(canonical) = path.canonicalize() else { return };
        if self.seen.contains(&canonical) {
            return;
        }
        if canonical.is_dir() {
            self.seen.insert(canonical);
            let Ok(entries) = std::fs::read_dir(path) else { return };
            let mut configs = Vec::new();
            for entry in entries {
                if self.entries >= MAX_ENTRIES {
                    break;
                }
                self.entries += 1;
                let Ok(entry) = entry else { continue };
                // Fontconfig directory includes accept digit-prefixed *.conf only.
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if name.as_bytes().first().is_some_and(u8::is_ascii_digit) && name.ends_with(".conf") && entry.path().is_file() {
                    configs.push(entry.path());
                }
            }
            configs.sort();
            for config in configs {
                self.include(&config, depth + 1);
            }
            return;
        }
        if !canonical.is_file() {
            return;
        }
        self.seen.insert(canonical);
        let Ok(file) = std::fs::File::open(path) else { return };
        let mut bytes = Vec::new();
        if file.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 > MAX_CONFIG_BYTES {
            return;
        }
        let Ok(xml) = String::from_utf8(bytes) else { return };
        let parts = match fontconfig_parser::parse_config_parts(&xml) {
            Ok(parts) => parts,
            Err(error) => {
                log::debug!("interface font directories: {}: {error}", path.display());
                return;
            }
        };
        for part in parts {
            use fontconfig_parser::{ConfigPart, DirPrefix};
            match part {
                ConfigPart::Dir(dir) if self.dirs.len() < MAX_ENTRIES => self.dirs.push(dir.calculate_path(path)),
                ConfigPart::ResetDirs => self.dirs.clear(),
                ConfigPart::Include(mut include) => {
                    // The dependency's default include prefix hardcodes /etc/fonts.
                    // Resolve relative to this file, including nested and FONTCONFIG_PATH configs.
                    if include.prefix == DirPrefix::Default {
                        include.prefix = DirPrefix::Relative;
                    }
                    self.include(&include.calculate_path(path), depth + 1);
                }
                _ => {}
            }
        }
    }
}

pub(super) fn config_path() -> PathBuf {
    let file = std::env::var_os("FONTCONFIG_FILE").map_or_else(|| PathBuf::from("fonts.conf"), PathBuf::from);
    if file.is_absolute() {
        return file;
    }
    let dir =
        std::env::var_os("FONTCONFIG_PATH").and_then(|paths| std::env::split_paths(&paths).next()).unwrap_or_else(|| PathBuf::from("/etc/fonts"));
    dir.join(file)
}

pub(super) fn font_files(config_path: &Path) -> Vec<PathBuf> {
    // Use the dependency's XML parser, but own include resolution and directory filtering.
    // Missing or malformed configuration must not prevent the ordinary fallback paths.
    let mut config = Directories::default();
    config.include(config_path, 0);
    let mut pending: Vec<_> = config.dirs.into_iter().map(|dir| (dir, 0)).collect();
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    let mut remaining = MAX_ENTRIES;
    while let Some((dir, depth)) = pending.pop() {
        let Ok(canonical) = dir.canonicalize() else { continue };
        if !seen.insert(canonical) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries {
            if remaining == 0 {
                files.sort();
                return files;
            }
            remaining -= 1;
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            // Follow links to fonts and directories, but visit each directory only once.
            if path.is_dir() {
                if depth < MAX_DEPTH {
                    pending.push((path, depth + 1));
                }
            } else if path.extension().is_some_and(|ext| ext == "ttf" || ext == "ttc") && path.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures contain only configuration and dummy files, never copied system fonts.
    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("pdfcraft-fontconfig-{}-{n}", std::process::id()));
            std::fs::create_dir(&dir).unwrap();
            Self(dir)
        }

        fn config(&self, name: &str, body: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, format!("<fontconfig>{body}</fontconfig>")).unwrap();
            path
        }

        fn isolated(&self, test: &str) {
            // Environment selection is process-global: never mutate it in a parallel harness.
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", &format!("system_fonts::fontconfig::tests::{test}"), "--nocapture"])
                .env("FONTCONFIG_PATH", &self.0)
                .env("FONTCONFIG_FILE", "fonts.conf")
                .env("PDFCRAFT_FONTCONFIG_TEST_ROOT", &self.0)
                .output()
                .unwrap();
            assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        }

        fn font(&self) -> PathBuf {
            let dir = self.0.join("fonts");
            std::fs::create_dir(&dir).unwrap();
            let font = dir.join("DejaVuSans.ttf");
            std::fs::write(&font, b"dummy font for directory discovery").unwrap();
            font
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn relative_includes_follow_each_including_config_under_fontconfig_path() {
        if let Some(root) = std::env::var_os("PDFCRAFT_FONTCONFIG_TEST_ROOT") {
            let root = PathBuf::from(root);
            assert_eq!(config_path(), root.join("fonts.conf"));
            assert_eq!(font_files(&config_path()), vec![root.join("fonts/DejaVuSans.ttf")]);
            return;
        }
        let fixture = Fixture::new();
        fixture.font();
        std::fs::create_dir(fixture.0.join("conf.d")).unwrap();
        std::fs::create_dir(fixture.0.join("nested")).unwrap();
        fixture.config("fonts.conf", "<include>conf.d</include>");
        fixture.config("conf.d/10-fonts.conf", "<include>../nested/next.conf</include>");
        fixture.config("nested/next.conf", &format!("<dir>{}</dir>", fixture.0.join("fonts").display()));
        fixture.isolated("relative_includes_follow_each_including_config_under_fontconfig_path");
    }

    #[test]
    fn included_directories_ignore_disabled_backup_and_non_numbered_files() {
        if let Some(root) = std::env::var_os("PDFCRAFT_FONTCONFIG_TEST_ROOT") {
            let root = PathBuf::from(root);
            assert_eq!(font_files(&config_path()), vec![root.join("fonts/DejaVuSans.ttf")]);
            return;
        }
        for name in ["99-reset.conf.disabled", "99-reset.conf.bak", "reset.conf"] {
            let fixture = Fixture::new();
            fixture.font();
            std::fs::create_dir(fixture.0.join("conf.d")).unwrap();
            fixture.config("fonts.conf", &format!("<include>{}</include>", fixture.0.join("conf.d").display()));
            fixture.config("conf.d/10-fonts.conf", &format!("<dir>{}</dir>", fixture.0.join("fonts").display()));
            fixture.config(&format!("conf.d/{name}"), "<reset-dirs/>");
            fixture.isolated("included_directories_ignore_disabled_backup_and_non_numbered_files");
        }
    }

    #[test]
    fn numbered_configs_keep_lexical_reset_order_and_include_cycles_terminate() {
        let fixture = Fixture::new();
        let font = fixture.font();
        std::fs::create_dir(fixture.0.join("conf.d")).unwrap();
        let config = fixture.config("fonts.conf", "<include>conf.d</include>");
        fixture.config("conf.d/10-reset.conf", "<reset-dirs/>");
        let external = fixture.config("external.conf", &format!("<dir>{}</dir><include>../fonts.conf</include>", fixture.0.join("fonts").display()));
        std::os::unix::fs::symlink(external, fixture.0.join("conf.d/20-fonts.conf")).unwrap();
        assert_eq!(font_files(&config), vec![font]);
        fixture.config("conf.d/30-reset.conf", "<reset-dirs/>");
        assert!(font_files(&config).is_empty());
    }

    #[test]
    fn oversized_configs_are_skipped_without_losing_other_includes() {
        let fixture = Fixture::new();
        let font = fixture.font();
        std::fs::create_dir(fixture.0.join("conf.d")).unwrap();
        let config = fixture.config("fonts.conf", "<include>conf.d</include>");
        std::fs::write(fixture.0.join("conf.d/10-large.conf"), vec![b' '; (MAX_CONFIG_BYTES + 1) as usize]).unwrap();
        fixture.config("conf.d/20-fonts.conf", &format!("<dir>{}</dir>", fixture.0.join("fonts").display()));
        assert_eq!(font_files(&config), vec![font]);
    }

    #[test]
    fn deep_include_chains_are_bounded() {
        let fixture = Fixture::new();
        fixture.font();
        for depth in 0..=MAX_DEPTH + 1 {
            let body = if depth > MAX_DEPTH {
                format!("<dir>{}</dir>", fixture.0.join("fonts").display())
            } else {
                format!("<include>depth-{}.conf</include>", depth + 1)
            };
            fixture.config(&format!("depth-{depth}.conf"), &body);
        }
        assert!(font_files(&fixture.0.join("depth-0.conf")).is_empty());
    }

    #[test]
    fn wide_include_directories_are_bounded() {
        let fixture = Fixture::new();
        fixture.font();
        std::fs::create_dir(fixture.0.join("conf.d")).unwrap();
        let config = fixture.config("fonts.conf", "<include>conf.d</include>");
        for n in 0..MAX_CONFIGS {
            fixture.config(&format!("conf.d/{n:04}-empty.conf"), "");
        }
        fixture.config("conf.d/9999-fonts.conf", &format!("<dir>{}</dir>", fixture.0.join("fonts").display()));
        assert!(font_files(&config).is_empty());
    }

    #[test]
    fn included_configuration_finds_fonts_outside_standard_directories() {
        let fixture = Fixture::new();
        let fonts = fixture.0.join("nix-store-font/share/fonts/truetype");
        std::fs::create_dir_all(&fonts).unwrap();
        let font = fonts.join("DejaVuSans.ttf");
        std::fs::write(&font, b"not parsed during discovery").unwrap();
        std::fs::write(fonts.join("README.txt"), b"not a font").unwrap();
        let included = fixture.config("nixos.conf", &format!("<dir>{}</dir>", fixture.0.join("nix-store-font").display()));
        let config = fixture.config("fonts.conf", &format!("<include>{}</include>", included.display()));
        assert_eq!(font_files(&config), vec![font]);
    }

    #[test]
    fn missing_and_malformed_configuration_are_skipped() {
        let fixture = Fixture::new();
        assert!(font_files(&fixture.0.join("missing.conf")).is_empty());
        let path = fixture.0.join("broken.conf");
        std::fs::write(&path, "<fontconfig><dir>").unwrap();
        assert!(font_files(&path).is_empty());
    }

    #[test]
    fn symlinked_fonts_are_found_and_directory_cycles_terminate() {
        let fixture = Fixture::new();
        let fonts = fixture.0.join("fonts");
        std::fs::create_dir(&fonts).unwrap();
        let original = fixture.0.join("original.ttf");
        std::fs::write(&original, b"dummy").unwrap();
        let linked = fonts.join("DejaVuSans.ttf");
        std::os::unix::fs::symlink(&original, &linked).unwrap();
        std::os::unix::fs::symlink(&fonts, fonts.join("loop")).unwrap();
        std::os::unix::fs::symlink(fixture.0.join("missing.ttf"), fonts.join("broken.ttf")).unwrap();
        let config = fixture.config("fonts.conf", &format!("<dir>{}</dir><dir>{}</dir>", fonts.display(), fonts.display()));
        assert_eq!(font_files(&config), vec![linked]);
    }

    #[test]
    fn deeply_nested_directories_are_bounded() {
        let fixture = Fixture::new();
        let mut dir = fixture.0.join("fonts");
        std::fs::create_dir(&dir).unwrap();
        let shallow = dir.join("DejaVuSans.ttf");
        std::fs::write(&shallow, b"dummy").unwrap();
        for _ in 0..MAX_DEPTH + 1 {
            dir.push("nested");
            std::fs::create_dir(&dir).unwrap();
        }
        std::fs::write(dir.join("DejaVuSans.ttf"), b"dummy").unwrap();
        let config = fixture.config("fonts.conf", &format!("<dir>{}</dir>", fixture.0.join("fonts").display()));
        assert_eq!(font_files(&config), vec![shallow]);
    }
}
