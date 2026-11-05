//! Reproducible mobile project preparation. Product code and this harness are Rust.
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};

#[path = "mobile_android_notices.rs"]
mod android_notices;

fn run_in(dir: &Path, tool: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(tool).args(args).current_dir(dir).status().with_context(|| format!("run {tool}"))?;
    ensure!(status.success(), "{tool} {} failed: {status}", args.join(" "));
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        } else if entry.file_type()?.is_file() {
            std::fs::copy(entry.path(), to.join(entry.file_name()))?;
        } else {
            bail!("mobile assets must be regular files: {}", entry.path().display());
        }
    }
    Ok(())
}

fn append_notice(text: &mut String, label: &str, path: &Path) -> Result<()> {
    text.push_str(&format!("\n\n===== {label} =====\n\n"));
    text.push_str(&std::fs::read_to_string(path).with_context(|| format!("required mobile notice {}", path.display()))?);
    Ok(())
}

fn cargo_packages(manifest: &Path) -> Result<Vec<serde_json::Value>> {
    let metadata = Command::new("cargo").args(["metadata", "--format-version", "1", "--locked", "--manifest-path"]).arg(manifest).output()?;
    ensure!(metadata.status.success(), "Cargo metadata failed while collecting mobile notices: {}", String::from_utf8_lossy(&metadata.stderr));
    let metadata: serde_json::Value = serde_json::from_slice(&metadata.stdout)?;
    metadata.get("packages").and_then(serde_json::Value::as_array).cloned().context("Missing Cargo packages")
}

fn cargo_notices(text: &mut String, packages: &[serde_json::Value]) -> Result<()> {
    let mut seen = std::collections::BTreeSet::new();
    for package in packages {
        let name = package.get("name").and_then(serde_json::Value::as_str).context("Missing Cargo package name")?;
        let version = package.get("version").and_then(serde_json::Value::as_str).context("Missing Cargo package version")?;
        let manifest = package.get("manifest_path").and_then(serde_json::Value::as_str).context("Missing crate manifest path")?;
        if !seen.insert(manifest) {
            continue;
        }
        let source = Path::new(manifest).parent().context("Invalid crate manifest path")?;
        text.push_str(&format!("\n\n===== Cargo package {name} {version} =====\n"));
        for field in ["license", "authors", "source", "repository"] {
            if let Some(value) = package.get(field).filter(|v| !v.is_null()) {
                text.push_str(&format!("{field}: {value}\n"));
            }
        }
        let mut files = std::collections::BTreeSet::new();
        if let Some(path) = package.get("license_file").and_then(serde_json::Value::as_str) {
            files.insert(source.join(path));
        }
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_ascii_uppercase();
            if entry.file_type()?.is_file()
                && ["LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "NOTICE", "UNLICENSE"].iter().any(|prefix| name.starts_with(prefix))
            {
                files.insert(entry.path());
            }
        }
        for path in files {
            let label = path.file_name().and_then(|n| n.to_str()).context("Invalid licence filename")?;
            append_notice(text, label, &path)?;
        }
    }
    Ok(())
}

fn frontend_notices(root: &Path) -> Result<()> {
    let mut text = String::from("PdfCraft mobile: project, icon and embedded-font notices\n");
    for file in [
        "LICENSE-MIT",
        "LICENSE-APACHE",
        "NOTICE",
        "assets/app-icon/LICENSE.txt",
        "assets/icons/LICENSE-lucide.txt",
        "assets/fonts/OFL-Inter.txt",
        "assets/fonts/OFL-JetBrainsMono.txt",
        "assets/fonts/OFL-DancingScript.txt",
        "vendor/hayro/LICENSE-MIT",
        "vendor/hayro/LICENSE-APACHE",
        "vendor/hayro-interpret/LICENSE-MIT",
        "vendor/hayro-interpret/LICENSE-APACHE",
        "vendor/hayro-interpret/assets/LICENSE_FOXIT",
        "vendor/hayro-interpret/assets/CGATS_LICENSE.txt",
        "packaging/mobile/NOTICE.txt",
        "packaging/mobile/LICENSE-MPL-2.0.txt",
    ] {
        append_notice(&mut text, file, &root.join(file))?;
    }
    // The same permitted web faces selected by crates/fonts/build.rs. This reads
    // licence text only; no additional font or trademark artwork is copied.
    if let Some(fonts) = std::env::var_os("CRAFT_FONTS_DIR") {
        let fonts = PathBuf::from(fonts);
        let manifest = std::fs::read_to_string(fonts.join("fonts/manifest.txt"))?;
        let mut seen = std::collections::BTreeSet::new();
        for line in manifest.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')) {
            let fields = line.split(" | ").map(str::trim).collect::<Vec<_>>();
            let [family, style, _, scripts, _, licence, ..] = fields.as_slice() else { bail!("Invalid craft-fonts manifest line") };
            let web = (*family == "BIZ UDPGothic" && *style == "Regular") || scripts.split(',').any(|s| matches!(s.trim(), "Arab" | "Telu"));
            if web && seen.insert(*licence) {
                ensure!(Path::new(licence).components().all(|c| matches!(c, std::path::Component::Normal(_))), "Invalid font licence path");
                append_notice(&mut text, family, &fonts.join(licence))?;
            }
        }
        ensure!(!seen.is_empty(), "No web font licences in craft-fonts");
    } else {
        ensure!(std::env::var("CRAFT_FONTS_REQUIRED").as_deref() != Ok("1"), "CRAFT_FONTS_DIR is required for mobile packages");
    }
    // Resolve the exact locked dependency source, without guessing Cargo's
    // cache path or copying fonts out of their source packages.
    let mut packages = cargo_packages(&root.join("Cargo.toml"))?;
    for (name, files) in [
        ("epaint_default_fonts", &["fonts/emoji-icon-font-mit-license.txt", "fonts/Hack-Regular.txt", "fonts/OFL.txt", "fonts/UFL.txt"][..]),
        ("hayro-cmap", &["assets/LICENSE.txt"][..]),
    ] {
        let package =
            packages.iter().find(|p| p.get("name").and_then(serde_json::Value::as_str) == Some(name)).context("Missing bundled font/data crate")?;
        let manifest = package.get("manifest_path").and_then(serde_json::Value::as_str).context("Missing crate manifest path")?;
        let source = Path::new(manifest).parent().context("Invalid crate manifest path")?;
        for file in files {
            append_notice(&mut text, &format!("{name}/{file}"), &source.join(file))?;
        }
    }
    // Include the native host's permissive-code notices as well as the five MPL
    // exceptions. Metadata covers both pinned graphs, including build tooling;
    // its package labels retain authors, SPDX declarations and source revisions.
    packages.extend(cargo_packages(&root.join("packaging/mobile/app/Cargo.toml"))?);
    cargo_notices(&mut text, &packages)?;
    std::fs::create_dir_all(root.join("dist/mobile"))?;
    std::fs::write(root.join("dist/mobile/THIRD-PARTY-NOTICES.txt"), text)?;
    Ok(())
}

/// The interface languages `crates/ui-egui/src/i18n.rs` offers, English included. The catalogs
/// themselves come from craft-i18n at build time, so the source list is what the app advertises.
fn ui_languages(root: &Path) -> Result<Vec<String>> {
    let source = std::fs::read_to_string(root.join("crates/ui-egui/src/i18n.rs"))?;
    let languages =
        source.split("LangInfo { code: \"").skip(1).filter_map(|rest| rest.split_once('"').map(|(code, _)| code.to_owned())).collect::<Vec<_>>();
    ensure!(languages.iter().any(|code| code == "en") && languages.len() > 1, "No interface languages in ui-egui's i18n.rs");
    Ok(languages)
}

/// Distribution from a fork cannot carry the upstream trademarks. Keep this
/// packaging transformation out of the shared UI contribution, fail closed if
/// its source changes, and restore the checkout even when Trunk fails.
fn preview_source(file: &str, source: &str) -> Result<String> {
    fn replace_once(source: &mut String, from: &str, to: &str) -> Result<()> {
        ensure!(source.matches(from).count() == 1, "Preview branding source changed: {from}");
        *source = source.replacen(from, to, 1);
        Ok(())
    }
    let mut text = source.to_owned();
    match file {
        "home.rs" => {
            replace_once(&mut text, "widgets::artcraft_mark(ui, 28.0);", "")?;
            replace_once(&mut text, "tl!(\"Join the ArtCraft community\")", "\"PdfCraft Preview\"")?;
        }
        "dialogs.rs" => {
            replace_once(&mut text, "widgets::artcraft_mark(ui, 40.0);", "")?;
            replace_once(&mut text, "ui.label(egui::RichText::new(tl!(\"Part of\")).color(t.text_muted));", "")?;
            replace_once(
                &mut text,
                "widgets::artcraft_logo(ui, 16.0);",
                "ui.label(egui::RichText::new(\"Independent preview, based on PdfCraft by the ArtCraft team.\").color(t.text_muted));",
            )?;
        }
        "widgets.rs" => {
            let start = text.find("/// The ArtCraft wordmark").context("Missing brand widget start")?;
            let end = text.find("/// Buttons for every community link").context("Missing brand widget end")?;
            ensure!(end > start, "Brand widget order changed");
            text.replace_range(start..end, "");
        }
        _ => bail!("Unexpected preview source: {file}"),
    }
    ensure!(!text.contains("artcraft_mark") && !text.contains("artcraft_logo") && !text.contains("docs/brand/"), "Unremoved brand in {file}");
    Ok(text)
}

#[derive(Default)]
struct PreviewSources(Vec<(PathBuf, String)>);

impl PreviewSources {
    fn apply(root: &Path) -> Result<Self> {
        // Validate every transformation before touching any source file.
        let edits = ["home.rs", "dialogs.rs", "widgets.rs"]
            .into_iter()
            .map(|file| {
                let path = root.join("crates/ui-egui/src").join(file);
                let original = std::fs::read_to_string(&path)?;
                let preview = preview_source(file, &original)?;
                Ok((path, original, preview))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut restore = Self::default();
        for (path, original, preview) in edits {
            restore.0.push((path.clone(), original));
            std::fs::write(path, preview)?;
        }
        Ok(restore)
    }

    fn restore(&mut self) -> Result<()> {
        while let Some((path, original)) = self.0.last() {
            std::fs::write(path, original).with_context(|| format!("restore preview source {}", path.display()))?;
            self.0.pop();
        }
        Ok(())
    }
}

impl Drop for PreviewSources {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            eprintln!("Failed to restore mobile preview checkout: {error:#}");
        }
    }
}

pub fn run(args: &[String]) -> Result<()> {
    let root = super::gates::root();
    let app = root.join("packaging/mobile/app");
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["android-notices", manifest, output] => android_notices::run(Path::new(manifest), Path::new(output)),
        ["notices"] => frontend_notices(&root),
        [mode @ ("frontend" | "frontend-preview")] => {
            ensure!(
                std::env::var_os("CRAFT_I18N_DIR").is_some_and(|dir| !dir.is_empty()),
                "Set CRAFT_I18N_DIR to the craft-i18n checkout at craft-i18n.rev: the mobile apps advertise every interface language"
            );
            let mut preview = if *mode == "frontend-preview" { PreviewSources::apply(&root)? } else { PreviewSources::default() };
            let result = run_in(
                &root.join("apps/pdfcraft-web"),
                "trunk",
                &["build", "mobile.html", "--release", "--dist", "../../dist/mobile", "--public-url", "./"],
            );
            preview.restore()?;
            result?;
            frontend_notices(&root)
        }
        ["prepare", platform @ ("android" | "ios")] => {
            let version = Command::new("cargo").args(["tauri", "--version"]).output()?;
            ensure!(
                version.status.success() && String::from_utf8_lossy(&version.stdout).contains("2.12.2"),
                "Install the pinned CLI: cargo install tauri-cli --version 2.12.2 --locked"
            );
            run_in(&app, "cargo", &["tauri", platform, "init", "--ci"])?;
            if *platform == "android" {
                copy_tree(&app.join("icons/android"), &app.join("gen/android/app/src/main/res"))?;
                let resources = app.join("gen/android/app/src/main/res/xml");
                std::fs::create_dir_all(&resources)?;
                let mut languages = ui_languages(&root)?;
                languages.sort();
                let mut locales = String::from(
                    "<?xml version=\"1.0\" encoding=\"utf-8\"?><locale-config xmlns:android=\"http://schemas.android.com/apk/res/android\">",
                );
                for code in languages {
                    locales.push_str(&format!("<locale android:name=\"{code}\"/>"));
                }
                locales.push_str("</locale-config>\n");
                std::fs::write(resources.join("pdfcraft_locales.xml"), locales)?;
                let manifest = app.join("gen/android/app/src/main/AndroidManifest.xml");
                let xml = std::fs::read_to_string(&manifest)?;
                ensure!(xml.contains("<application"), "Generated Android manifest has no application");
                if !xml.contains("android:localeConfig=") {
                    std::fs::write(manifest, xml.replacen("<application", "<application android:localeConfig=\"@xml/pdfcraft_locales\"", 1))?;
                }
                let gradle = app.join("gen/android/app/build.gradle.kts");
                let kotlin = std::fs::read_to_string(&gradle)?;
                let hook = "apply(from = file(\"../../../../android-notices.gradle\"))";
                if !kotlin.contains(hook) {
                    std::fs::write(
                        gradle,
                        format!("{kotlin}\n// Package notices from the resolved runtime artifacts before merging APK assets.\n{hook}\n"),
                    )?;
                }
            } else {
                copy_tree(&app.join("icons/ios"), &app.join("gen/apple/Assets.xcassets/AppIcon.appiconset"))?;
                // Sources is already part of the generated Xcode target. A resource
                // file copied there is picked up when XcodeGen regenerates it.
                std::fs::copy(app.join("PrivacyInfo.xcprivacy"), app.join("gen/apple/Sources/PrivacyInfo.xcprivacy"))?;
                let project = app.join("gen/apple/project.yml");
                let yaml = std::fs::read_to_string(&project)?;
                ensure!(yaml.contains("ENABLE_BITCODE: false"), "Generated Xcode project has changed; check tablet device-family settings");
                if !yaml.contains("TARGETED_DEVICE_FAMILY:") {
                    std::fs::write(project, yaml.replace("ENABLE_BITCODE: false", "ENABLE_BITCODE: false\n        TARGETED_DEVICE_FAMILY: \"1,2\""))?;
                }
                run_in(&app.join("gen/apple"), "xcodegen", &["generate"])?;
            }
            Ok(())
        }
        ["android"] => run_in(&app, "cargo", &["tauri", "android", "build", "--ci", "--debug", "--apk", "--target", "aarch64", "x86_64"]),
        ["ios-simulator"] => run_in(&app, "cargo", &["tauri", "ios", "build", "--ci", "--debug", "--target", "aarch64-sim", "--no-sign"]),
        _ => bail!(
            "Usage: cargo xtask mobile frontend | frontend-preview | notices | prepare android | prepare ios | android | ios-simulator | android-notices MANIFEST OUTPUT"
        ),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fork_preview_removes_brand_assets_without_changing_upstream_sources() {
        let root = super::super::gates::root();
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let temp = std::env::temp_dir().join(format!("pdfcraft-mobile-preview-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&temp).unwrap();
        let dir = temp.join("crates/ui-egui/src");
        std::fs::create_dir_all(&dir).unwrap();
        let files = ["home.rs", "dialogs.rs", "widgets.rs"];
        for file in files {
            std::fs::copy(root.join("crates/ui-egui/src").join(file), dir.join(file)).unwrap();
        }
        {
            let _restore_on_failure = super::PreviewSources::apply(&temp).unwrap();
            for file in files {
                let preview = std::fs::read_to_string(dir.join(file)).unwrap();
                assert!(!preview.contains("docs/brand/") && !preview.contains("artcraft_mark") && !preview.contains("artcraft_logo"));
            }
            assert!(std::fs::read_to_string(dir.join("home.rs")).unwrap().contains("PdfCraft Preview"));
            // Reapplying or changed upstream code must fail before further edits.
            assert!(super::PreviewSources::apply(&temp).is_err());
        }
        for file in files {
            assert_eq!(std::fs::read(root.join("crates/ui-egui/src").join(file)).unwrap(), std::fs::read(dir.join(file)).unwrap());
            std::fs::remove_file(dir.join(file)).unwrap();
        }
        for path in [dir, temp.join("crates/ui-egui"), temp.join("crates"), temp] {
            std::fs::remove_dir(path).unwrap();
        }
    }

    #[test]
    fn mobile_packaging_retains_all_ui_languages_and_native_file_associations() {
        let root = super::super::gates::root();
        let plist = std::fs::read_to_string(root.join("packaging/mobile/app/Info.ios.plist")).unwrap();
        let languages = super::ui_languages(&root).unwrap();
        for language in languages {
            assert!(plist.to_lowercase().contains(&format!("<string>{language}</string>")), "missing iOS language {language}");
        }
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("packaging/mobile/app/tauri.conf.json")).unwrap()).unwrap();
        assert_eq!(config["bundle"]["fileAssociations"][0]["mimeType"], "application/pdf");
        assert_eq!(config["build"]["frontendDist"], "../../../dist/mobile");
        let html = std::fs::read_to_string(root.join("apps/pdfcraft-web/mobile.html")).unwrap();
        assert!(html.contains("viewport-fit=cover") && html.contains("safe-area-inset-bottom") && html.contains("mobile-host"));
    }

    #[test]
    fn mobile_license_exception_is_exact_and_notices_match_locked_sources() {
        let root = super::super::gates::root();
        let app = root.join("packaging/mobile/app");
        let exceptions: toml::Value = toml::from_str(&std::fs::read_to_string(app.join("deny.exceptions.toml")).unwrap()).unwrap();
        let entries = exceptions["exceptions"].as_array().unwrap();
        let expected = ["cssparser@0.37.0", "cssparser-macros@0.7.1", "dtoa-short@0.3.5", "selectors@0.38.0", "option-ext@0.2.0"];
        assert_eq!(entries.len(), expected.len());
        let lock: toml::Value = toml::from_str(&std::fs::read_to_string(app.join("Cargo.lock")).unwrap()).unwrap();
        let notice = std::fs::read_to_string(root.join("packaging/mobile/NOTICE.txt")).unwrap();
        for specification in expected {
            let exception = entries.iter().find(|entry| entry["crate"].as_str() == Some(specification)).unwrap();
            assert_eq!(exception["allow"].as_array().unwrap(), &[toml::Value::String("MPL-2.0".into())]);
            let (name, version) = specification.split_once('@').unwrap();
            let package = lock["package"]
                .as_array()
                .unwrap()
                .iter()
                .find(|package| package["name"].as_str() == Some(name) && package["version"].as_str() == Some(version))
                .unwrap();
            assert!(notice.contains(package["checksum"].as_str().unwrap()), "missing source checksum for {specification}");
            assert!(notice.contains(&format!("https://static.crates.io/crates/{name}/{name}-{version}.crate")));
        }
        let policy: toml::Value = toml::from_str(&std::fs::read_to_string(root.join("deny.toml")).unwrap()).unwrap();
        assert!(!policy["licenses"]["allow"].as_array().unwrap().iter().any(|value| value.as_str() == Some("MPL-2.0")));
        let config: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(app.join("tauri.conf.json")).unwrap()).unwrap();
        for resource in ["../NOTICE.txt", "../LICENSE-MPL-2.0.txt"] {
            assert!(config["bundle"]["resources"].as_array().unwrap().iter().any(|value| value.as_str() == Some(resource)));
            assert!(app.join(resource).is_file());
        }
        assert!(
            config["bundle"]["resources"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value.as_str() == Some("../../../dist/mobile/THIRD-PARTY-NOTICES.txt"))
        );
    }
}
