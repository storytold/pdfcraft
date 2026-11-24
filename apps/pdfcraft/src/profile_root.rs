//! App-owned nightly profile fences; no stable fallback, migration or process environment changes.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use craft::settings::Settings as _;
use pdfcraft_ui_egui::settings::PdfCraftSettings;

/// The folder PdfCraft keeps beside the executable in portable mode (`PdfCraftData`).
const PORTABLE_DATA_DIR: &str = PdfCraftSettings::PORTABLE_DIR;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Platform {
    Windows,
    Linux,
    Mac,
}

impl Platform {
    fn current() -> Result<Self, String> {
        match std::env::consts::OS {
            "windows" => Ok(Self::Windows),
            "linux" => Ok(Self::Linux),
            "macos" => Ok(Self::Mac),
            _ => Err("nightly-v1 is not defined for this operating system".into()),
        }
    }

    fn ignores_case(self) -> bool {
        self != Self::Linux
    }
}

fn absolute(path: PathBuf, label: &str) -> Result<PathBuf, String> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("{label} must be absolute and contain no parent traversal"));
    }
    Ok(path)
}

fn required_base(env: &impl Fn(&str) -> Option<OsString>, key: &str) -> Result<PathBuf, String> {
    let value = env(key).filter(|v| !v.is_empty()).ok_or_else(|| format!("cannot establish the stable {key} boundary"))?;
    absolute(PathBuf::from(value), key)
}

fn xdg_base(env: &impl Fn(&str) -> Option<OsString>, key: &str, fallback: &str) -> Result<PathBuf, String> {
    match env(key).filter(|v| !v.is_empty()) {
        Some(value) => absolute(PathBuf::from(value), key),
        None => Ok(required_base(env, "HOME")?.join(fallback)),
    }
}

/// Resolve existing ancestors before creating anything. A dangling link or inaccessible ancestor
/// is not a missing directory that may be replaced or followed later.
fn canonical_future(path: &Path) -> Result<PathBuf, String> {
    let mut parent = path;
    let mut missing = Vec::new();
    loop {
        match std::fs::symlink_metadata(parent) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(parent.file_name().ok_or("the profile root has no existing ancestor")?.to_os_string());
                parent = parent.parent().ok_or("the profile root has no existing ancestor")?;
            }
            Err(error) => return Err(format!("cannot inspect the profile boundary: {error}")),
        }
    }
    let mut resolved = std::fs::canonicalize(parent).map_err(|e| format!("cannot resolve the profile boundary: {e}"))?;
    for part in missing.into_iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

fn overlaps(a: &Path, b: &Path, platform: Platform) -> bool {
    let parts = |p: &Path| {
        p.components()
            .map(|c| {
                let value = c.as_os_str().to_string_lossy().into_owned();
                if platform.ignores_case() { value.to_lowercase() } else { value }
            })
            .collect::<Vec<_>>()
    };
    let (a, b) = (parts(a), parts(b));
    a.starts_with(&b) || b.starts_with(&a)
}

fn installation_boundary(platform: Platform, exe: &Path) -> Result<PathBuf, String> {
    let parent = exe.parent().ok_or("cannot establish the installation boundary")?;
    if platform == Platform::Mac {
        // A profile in Resources is still inside the versioned install, not beside MacOS.
        if let Some(bundle) = exe.ancestors().find(|p| p.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("app"))) {
            return Ok(bundle.to_path_buf());
        }
    }
    Ok(parent.to_path_buf())
}

/// Every stable PdfCraft data folder, from the app's own resolvers (`settings::resolution`,
/// `persist::recovery`, the single-instance socket folder) and the installers' default folders.
fn forbidden_roots(platform: Platform, env: &impl Fn(&str) -> Option<OsString>, exe: &Path) -> Result<Vec<PathBuf>, String> {
    let mut roots = match platform {
        Platform::Windows => {
            let mut roots = ["APPDATA", "LOCALAPPDATA"]
                .into_iter()
                .map(|key| required_base(env, key).map(|p| p.join("PdfCraft")))
                .collect::<Result<Vec<_>, _>>()?;
            // The MSI's default per-user and per-machine folders (install-policy.wxi). A custom
            // install folder is not known here; the executable's own folder is fenced below.
            roots.push(required_base(env, "LOCALAPPDATA")?.join("Programs").join("ArtCraft").join("PdfCraft"));
            for key in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"] {
                if let Some(base) = env(key).filter(|v| !v.is_empty()) {
                    roots.push(absolute(PathBuf::from(base), key)?.join("ArtCraft").join("PdfCraft"));
                }
            }
            roots
        }
        Platform::Linux => {
            let mut roots =
                [("XDG_CONFIG_HOME", ".config"), ("XDG_DATA_HOME", ".local/share"), ("XDG_CACHE_HOME", ".cache"), ("XDG_STATE_HOME", ".local/state")]
                    .into_iter()
                    .map(|(key, fallback)| xdg_base(env, key, fallback).map(|p| p.join("pdfcraft")))
                    .collect::<Result<Vec<_>, _>>()?;
            roots.extend(["/usr/bin/pdfcraft", "/usr/local/bin/pdfcraft", "/opt/pdfcraft"].map(PathBuf::from));
            // The stable Flatpak keeps its data in its own per-app folder.
            roots.push(required_base(env, "HOME")?.join(".var/app/ai.storyteller.pdfcraft"));
            roots
        }
        Platform::Mac => {
            let home = required_base(env, "HOME")?;
            let library = home.join("Library");
            vec![
                library.join("Application Support/PdfCraft"),
                library.join("Caches/PdfCraft"),
                library.join("Caches/ai.storyteller.pdfcraft"),
                library.join("Preferences/PdfCraft"),
                library.join("Preferences/ai.storyteller.pdfcraft.plist"),
                library.join("Saved Application State/ai.storyteller.pdfcraft.savedState"),
                home.join("Applications/PdfCraft.app"),
                PathBuf::from("/Applications/PdfCraft.app"),
            ]
        }
    };
    roots.push(installation_boundary(platform, exe)?);
    roots.into_iter().map(|p| canonical_future(&p)).collect()
}

/// Subfolders of the profile that a write can be redirected out of. `Recovery` holds whole
/// documents, `Dictionary` the OCR dictionaries, `Instance` the single-instance socket: each must
/// resolve inside the profile, because a link there would put Nightly's writes in another app's
/// folder.
pub const PRIVATE_SUBDIRECTORIES: [&str; 3] = ["Recovery", "Dictionary", "Instance"];

fn private_subdirectory(root: &Path, name: impl AsRef<Path>, label: &str) -> Result<PathBuf, String> {
    let chosen = canonical_future(&absolute(root.to_path_buf(), "the nightly profile root")?)?;
    let child = canonical_future(&chosen.join(name))?;
    if child == chosen || !child.starts_with(&chosen) {
        return Err(format!("the nightly {label} must stay inside its selected profile"));
    }
    Ok(child)
}

/// Every existing entry directly in the profile (the log folder, the merged settings file and so
/// on) must resolve inside it, so a link there cannot point Nightly's writes at another app's data.
fn entries_stay_inside(root: &Path) -> Result<(), String> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot inspect the nightly profile: {error}")),
    };
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot inspect the nightly profile: {e}"))?;
        private_subdirectory(root, entry.file_name(), "profile entry")?;
    }
    Ok(())
}

/// The profile's own `Instance` folder, where a nightly build keeps its single-instance socket so
/// two nightly launches hand files to each other and never to the stable window.
pub fn instance_dir(root: &Path) -> Result<PathBuf, String> {
    private_subdirectory(root, "Instance", "instance folder")
}

/// Recognise the shared portable data namespace in both the supplied and resolved path.
/// This is not an inventory of arbitrary custom Stable overrides or a Launcher ownership proof.
fn is_portable_layout(path: &Path, platform: Platform) -> bool {
    // A folder under a portable copy belongs to that copy too. Above the chosen folder itself, a
    // marker only counts beside a PdfCraft executable, so an unrelated portable.txt higher up
    // (in a user folder or at a drive root) does not refuse the profile.
    let markers = PdfCraftSettings::PORTABLE_MARKERS;
    let marked = |dir: &Path| markers.iter().any(|name| dir.join(name).is_file());
    path.ancestors()
        .enumerate()
        .any(|(depth, ancestor)| marked(ancestor) && (depth == 0 || ["pdfcraft.exe", "pdfcraft"].iter().any(|exe| ancestor.join(exe).is_file())))
        || path.components().any(|component| match component {
            Component::Normal(name) if platform.ignores_case() => name.to_string_lossy().eq_ignore_ascii_case(PORTABLE_DATA_DIR),
            Component::Normal(name) => name == PORTABLE_DATA_DIR,
            _ => false,
        })
}

/// Create `dir` and prove a file can be written in it, without opening, following or removing an
/// entry someone else placed at a probe name (`create_new` only).
pub fn ensure_writable(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let pid = std::process::id();
    for attempt in 0..64u32 {
        let probe = dir.join(format!(".write-test-{pid}-{attempt}"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
            Ok(file) => {
                drop(file);
                let _ = std::fs::remove_file(&probe);
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "every write-test name is already taken"))
}

/// The writable hook is the app's own exclusive probe. Keeping it injected lets filesystem
/// boundary tests exercise the real policy without starting the app or changing global state.
pub fn validate(
    root: &Path,
    env: impl Fn(&str) -> Option<OsString>,
    exe: Option<&Path>,
    writable: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<PathBuf, String> {
    validate_for(Platform::current()?, root, env, exe, writable)
}

fn validate_for(
    platform: Platform,
    root: &Path,
    env: impl Fn(&str) -> Option<OsString>,
    exe: Option<&Path>,
    writable: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<PathBuf, String> {
    absolute(root.to_path_buf(), "the nightly profile root")?;
    if env("PDFCRAFT_CONFIG_DIR").is_some_and(|v| !v.is_empty()) {
        return Err("nightly-v1 cannot be combined with PDFCRAFT_CONFIG_DIR".into());
    }
    if platform != Platform::Windows && env("FLATPAK_ID").is_some_and(|v| !v.is_empty()) {
        // These rows are native tar/app-bundle launches, not a Flatpak package. Inheriting its
        // runtime socket namespace could point another profile at the stable app's socket.
        return Err("nightly-v1 native packages cannot inherit a Flatpak instance; launch through the native Launcher".into());
    }
    let chosen = canonical_future(root)?;
    if is_portable_layout(root, platform) || is_portable_layout(&chosen, platform) {
        return Err("the nightly root must not use a portable PdfCraft installation or its PdfCraftData settings folder".into());
    }
    let exe = absolute(exe.ok_or("cannot establish the installation boundary")?.to_path_buf(), "the executable")?;
    let forbidden = forbidden_roots(platform, &env, &exe)?;
    if forbidden.iter().any(|stable| overlaps(&chosen, stable, platform)) {
        return Err("the nightly root must not overlap stable data or an installation".into());
    }
    entries_stay_inside(&chosen)?;
    for name in PRIVATE_SUBDIRECTORIES {
        private_subdirectory(&chosen, name, "profile subfolder")?;
    }
    let mut create = std::fs::DirBuilder::new();
    create.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        create.mode(0o700);
    }
    create.create(&chosen).map_err(|e| format!("cannot create the nightly profile: {e}"))?;
    let confirmed = std::fs::canonicalize(&chosen).map_err(|e| format!("cannot resolve the created nightly profile: {e}"))?;
    if confirmed != chosen || forbidden.iter().any(|stable| overlaps(&confirmed, stable, platform)) {
        return Err("the nightly profile changed while it was being validated".into());
    }
    for name in PRIVATE_SUBDIRECTORIES {
        private_subdirectory(&confirmed, name, "profile subfolder")?;
    }
    entries_stay_inside(&confirmed)?;
    writable(&confirmed).map_err(|e| format!("the nightly profile is not writable: {e}"))?;
    Ok(confirmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture {
        root: PathBuf,
        env: std::collections::BTreeMap<String, OsString>,
        exe: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = loop {
                let candidate =
                    std::env::temp_dir().join(format!("pdfcraft-profile-policy-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
                match std::fs::create_dir(&candidate) {
                    Ok(()) => break std::fs::canonicalize(candidate).unwrap(),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("cannot create owned fixture: {error}"),
                }
            };
            let home = root.join("home");
            let exe = root.join("install/pdfcraft");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
            std::fs::write(&exe, b"fixture executable, never run").unwrap();
            let env = [("HOME", home), ("APPDATA", root.join("roaming")), ("LOCALAPPDATA", root.join("local"))]
                .into_iter()
                .map(|(key, value)| (key.into(), value.into_os_string()))
                .collect();
            Self { root, env, exe }
        }

        fn get(&self, key: &str) -> Option<OsString> {
            self.env.get(key).cloned()
        }

        fn check(&self, platform: Platform, root: &Path) -> Result<PathBuf, String> {
            self.check_with(platform, root, |dir| {
                let probe = dir.join("exclusive-test-probe");
                let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&probe)?;
                drop(file);
                std::fs::remove_file(probe)
            })
        }

        fn check_with(&self, platform: Platform, root: &Path, writable: impl FnOnce(&Path) -> std::io::Result<()>) -> Result<PathBuf, String> {
            validate_for(platform, root, |key| self.get(key), Some(&self.exe), writable)
        }

        fn home(&self) -> PathBuf {
            PathBuf::from(self.get("HOME").unwrap())
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // Only the directory successfully created by this fixture is owned for removal.
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[test]
    fn portable_settings_from_another_install_are_refused_before_any_probe() {
        let fixture = Fixture::new();
        let stable_install = fixture.root.join("stable portable install");
        let stable_data = stable_install.join(PORTABLE_DATA_DIR);
        std::fs::create_dir_all(&stable_data).unwrap();
        std::fs::write(stable_install.join("portable.txt"), b"owned portable fixture").unwrap();
        std::fs::write(stable_data.join("config.toml"), b"stable settings must not change").unwrap();
        for platform in [Platform::Windows, Platform::Linux, Platform::Mac] {
            for candidate in [&stable_data, &stable_data.join("nested nightly")] {
                let probed = Cell::new(false);
                let result = fixture.check_with(platform, candidate, |_| {
                    probed.set(true);
                    Ok(())
                });
                assert!(result.is_err(), "a different portable install is not a private Nightly profile");
                assert!(!probed.get(), "reject portable data before a writable probe");
                assert_eq!(std::fs::read(stable_data.join("config.toml")).unwrap(), b"stable settings must not change");
                assert_eq!(std::fs::read_dir(&stable_data).unwrap().count(), 1);
            }
        }
        let private = fixture.root.join("PdfCraftData-backup/nightly");
        assert!(fixture.check(Platform::Linux, &private).is_ok(), "a merely similar name is not a portable settings component");
    }

    #[cfg(unix)]
    #[test]
    fn recovery_dictionary_and_instance_aliases_cannot_redirect_to_stable_data() {
        let fixture = Fixture::new();
        let stable = fixture.home().join(".local/state/pdfcraft/Recovery");
        std::fs::create_dir_all(&stable).unwrap();
        std::fs::write(stable.join("stable-autosave"), b"stable recovery bytes").unwrap();
        for name in PRIVATE_SUBDIRECTORIES {
            for (index, target) in [stable.clone(), fixture.root.join("missing target")].into_iter().enumerate() {
                let private = fixture.root.join(format!("nightly-{name}-{index}"));
                std::fs::create_dir(&private).unwrap();
                std::os::unix::fs::symlink(&target, private.join(name)).unwrap();
                let probed = Cell::new(false);
                let result = fixture.check_with(Platform::Linux, &private, |_| {
                    probed.set(true);
                    Ok(())
                });
                assert!(result.is_err(), "{name} must not escape the private profile or follow a dangling alias");
                assert!(!probed.get());
                std::fs::remove_file(private.join(name)).unwrap();
            }
        }
        assert_eq!(std::fs::read(stable.join("stable-autosave")).unwrap(), b"stable recovery bytes");
        assert_eq!(std::fs::read_dir(stable).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_private_subfolder_link_keeps_the_profile_usable() {
        let fixture = Fixture::new();
        let private = fixture.root.join("nightly-private");
        let recovery = private.join("private-recovery");
        std::fs::create_dir_all(&recovery).unwrap();
        std::os::unix::fs::symlink(&recovery, private.join("Recovery")).unwrap();
        assert!(fixture.check(Platform::Linux, &private).is_ok());
        assert_eq!(std::fs::canonicalize(private.join("Recovery")).unwrap(), recovery);
    }

    #[test]
    fn overlap_check_handles_ancestor_descendant_and_prefix_lookalikes() {
        let fixture = Fixture::new();
        let root = fixture.home().join("nightly/profiles/pdfcraft");
        for platform in [Platform::Windows, Platform::Linux, Platform::Mac] {
            assert!(overlaps(&root, &root.join("Recovery"), platform));
            assert!(overlaps(&root.join("Recovery"), &root, platform));
            assert!(!overlaps(&root, &root.with_file_name("pdfcraft-other"), platform));
        }
    }

    #[test]
    fn linux_fences_app_xdg_subtrees_but_allows_launcher_sibling() {
        let mut fixture = Fixture::new();
        for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME"] {
            fixture.env.insert(key.into(), fixture.root.join(key).into_os_string());
        }
        for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME"] {
            let stable = PathBuf::from(fixture.get(key).unwrap()).join("pdfcraft");
            std::fs::create_dir_all(&stable).unwrap();
            std::fs::write(stable.join("stable-sentinel"), b"stable bytes").unwrap();
            for candidate in [stable.clone(), stable.join("nightly"), stable.parent().unwrap().to_path_buf()] {
                assert!(fixture.check(Platform::Linux, &candidate).unwrap_err().contains("overlap"));
            }
            assert_eq!(std::fs::read(stable.join("stable-sentinel")).unwrap(), b"stable bytes");
        }
        let data = PathBuf::from(fixture.get("XDG_DATA_HOME").unwrap());
        let nightly = data.join("ArtCraftLauncher/nightly/profiles/pdfcraft");
        assert_eq!(fixture.check(Platform::Linux, &nightly).unwrap(), nightly);
        assert_eq!(std::fs::read_dir(&nightly).unwrap().count(), 0);
    }

    #[test]
    fn mac_fences_only_app_library_subtrees_and_the_entire_bundle() {
        let mut fixture = Fixture::new();
        fixture.exe = fixture.root.join("install/PdfCraft Nightly.app/Contents/MacOS/PdfCraft");
        std::fs::create_dir_all(fixture.exe.parent().unwrap()).unwrap();
        std::fs::write(&fixture.exe, b"fixture executable, never run").unwrap();
        let library = fixture.home().join("Library");
        for suffix in [
            "Application Support/PdfCraft",
            "Caches/PdfCraft",
            "Caches/ai.storyteller.pdfcraft",
            "Preferences/PdfCraft",
            "Preferences/ai.storyteller.pdfcraft.plist",
            "Saved Application State/ai.storyteller.pdfcraft.savedState",
        ] {
            let stable = library.join(suffix);
            assert!(fixture.check(Platform::Mac, &stable.join("nightly")).unwrap_err().contains("overlap"));
            assert!(!stable.exists());
        }
        let inside_resources = fixture.exe.parent().unwrap().parent().unwrap().join("Resources/profile");
        assert!(fixture.check(Platform::Mac, &inside_resources).unwrap_err().contains("overlap"));
        assert!(!inside_resources.exists());
        let valid = library.join("Application Support/ArtCraftLauncher/nightly/profiles/pdfcraft");
        assert_eq!(fixture.check(Platform::Mac, &valid).unwrap(), valid);
    }

    #[test]
    fn windows_roaming_local_and_install_fences_remain_active() {
        let fixture = Fixture::new();
        for key in ["APPDATA", "LOCALAPPDATA"] {
            let stable = PathBuf::from(fixture.get(key).unwrap()).join("PdfCraft");
            assert!(fixture.check(Platform::Windows, &stable.join("nightly")).unwrap_err().contains("overlap"));
            assert!(!stable.exists());
        }
        assert!(fixture.check(Platform::Windows, &fixture.exe.parent().unwrap().join("profile")).is_err());
        assert!(fixture.check(Platform::Windows, &fixture.root.join("Launcher/profiles/pdfcraft")).is_ok());
    }

    #[test]
    fn windows_default_msi_install_folders_are_fenced() {
        let mut fixture = Fixture::new();
        let local = PathBuf::from(fixture.get("LOCALAPPDATA").unwrap());
        let per_user = local.join("Programs").join("ArtCraft").join("PdfCraft");
        assert!(fixture.check(Platform::Windows, &per_user.join("nightly")).unwrap_err().contains("overlap"));
        for key in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"] {
            let base = fixture.root.join(format!("machine {key}"));
            fixture.env.insert(key.into(), base.clone().into_os_string());
            let machine = base.join("ArtCraft").join("PdfCraft");
            assert!(fixture.check(Platform::Windows, &machine.join("nightly")).unwrap_err().contains("overlap"));
            assert!(!machine.exists());
        }
        let sibling = local.join("Programs").join("ArtCraft").join("ArtCraftLauncher").join("nightly").join("pdfcraft");
        assert!(fixture.check(Platform::Windows, &sibling).is_ok(), "the Launcher's own Programs sibling is not PdfCraft's install");
    }

    #[test]
    fn folders_under_a_stable_portable_copy_are_refused_before_any_probe() {
        let fixture = Fixture::new();
        let portable = fixture.root.join("D/PC");
        std::fs::create_dir_all(&portable).unwrap();
        std::fs::write(portable.join("PdfCraft.portable"), b"stable portable copy").unwrap();
        std::fs::write(portable.join("pdfcraft.exe"), b"stable portable executable, never run").unwrap();
        for platform in [Platform::Windows, Platform::Linux, Platform::Mac] {
            for candidate in [portable.join("nightly"), portable.join("deeper/nightly")] {
                let probed = Cell::new(false);
                let result = fixture.check_with(platform, &candidate, |_| {
                    probed.set(true);
                    Ok(())
                });
                assert!(result.unwrap_err().contains("portable"));
                assert!(!probed.get());
                assert!(!candidate.exists());
            }
        }
        // A portable marker that no PdfCraft executable sits beside is someone else's file.
        let unrelated = fixture.root.join("user folder");
        std::fs::create_dir_all(&unrelated).unwrap();
        std::fs::write(unrelated.join("portable.txt"), b"another app's marker").unwrap();
        assert!(fixture.check(Platform::Windows, &unrelated.join("Launcher/nightly/pdfcraft")).is_ok());
    }

    #[test]
    fn linux_fences_the_stable_flatpak_data_folder() {
        let fixture = Fixture::new();
        let flatpak = fixture.home().join(".var/app/ai.storyteller.pdfcraft");
        for candidate in [flatpak.clone(), flatpak.join("data/nightly")] {
            assert!(fixture.check(Platform::Linux, &candidate).unwrap_err().contains("overlap"));
        }
        assert!(!flatpak.exists());
        assert!(fixture.check(Platform::Linux, &fixture.home().join(".var/app/ai.storyteller.launcher/nightly")).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn any_profile_entry_alias_must_stay_inside_the_profile() {
        let fixture = Fixture::new();
        let stable = fixture.home().join(".config/pdfcraft/presets");
        std::fs::create_dir_all(&stable).unwrap();
        std::fs::write(stable.join("stable-preset"), b"stable preset bytes").unwrap();
        for name in ["presets", "logs", "config.toml"] {
            let private = fixture.root.join(format!("nightly-{name}"));
            std::fs::create_dir(&private).unwrap();
            std::os::unix::fs::symlink(&stable, private.join(name)).unwrap();
            let probed = Cell::new(false);
            let result = fixture.check_with(Platform::Linux, &private, |_| {
                probed.set(true);
                Ok(())
            });
            assert!(result.is_err(), "{name} must not lead outside the profile");
            assert!(!probed.get());
        }
        let private = fixture.root.join("nightly-inside");
        std::fs::create_dir_all(private.join("real-presets")).unwrap();
        std::fs::write(private.join("config.toml"), b"{}").unwrap();
        std::os::unix::fs::symlink(private.join("real-presets"), private.join("presets")).unwrap();
        assert!(fixture.check(Platform::Linux, &private).is_ok(), "regular files and links that stay inside are fine");
        assert_eq!(std::fs::read(stable.join("stable-preset")).unwrap(), b"stable preset bytes");
    }

    #[test]
    fn ambiguous_environment_and_non_native_sandbox_are_refused_before_creation() {
        let mut fixture = Fixture::new();
        let candidate = fixture.home().join("nightly/profiles/pdfcraft");
        fixture.env.insert("XDG_CACHE_HOME".into(), "relative/cache".into());
        assert!(fixture.check(Platform::Linux, &candidate).unwrap_err().contains("absolute"));
        fixture.env.remove("XDG_CACHE_HOME");
        fixture.env.insert("PDFCRAFT_CONFIG_DIR".into(), fixture.home().join("custom-stable").into_os_string());
        assert!(fixture.check(Platform::Linux, &candidate).unwrap_err().contains("PDFCRAFT_CONFIG_DIR"));
        fixture.env.remove("PDFCRAFT_CONFIG_DIR");
        fixture.env.insert("FLATPAK_ID".into(), "ai.storyteller.pdfcraft".into());
        assert!(fixture.check(Platform::Linux, &candidate).unwrap_err().contains("Flatpak"));
        fixture.env.remove("FLATPAK_ID");
        fixture.env.remove("HOME");
        assert!(fixture.check(Platform::Linux, &candidate).unwrap_err().contains("HOME"));
        assert!(!candidate.exists());
    }

    #[test]
    fn non_directory_and_failed_write_probe_never_fall_back_to_stable() {
        let fixture = Fixture::new();
        let file = fixture.home().join("not-a-directory");
        std::fs::write(&file, b"do not replace me").unwrap();
        assert!(fixture.check(Platform::Linux, &file).is_err());
        assert_eq!(std::fs::read(file).unwrap(), b"do not replace me");
        let candidate = fixture.home().join("nightly");
        let calls = Cell::new(0);
        let result = fixture.check_with(Platform::Linux, &candidate, |_| {
            calls.set(calls.get() + 1);
            Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "injected write refusal"))
        });
        assert!(result.unwrap_err().contains("not writable"));
        assert_eq!(calls.get(), 1);
        assert!(!fixture.home().join(".config/pdfcraft").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_and_dangling_aliases_cannot_create_or_modify_stable_data() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let stable = fixture.home().join(".config/pdfcraft");
        std::fs::create_dir_all(&stable).unwrap();
        std::fs::write(stable.join("sentinel"), b"untouched").unwrap();
        let alias = fixture.home().join("alias");
        symlink(&stable, &alias).unwrap();
        assert!(fixture.check(Platform::Linux, &alias.join("nightly")).unwrap_err().contains("overlap"));
        let missing = fixture.home().join("missing-target");
        let dangling = fixture.home().join("dangling");
        symlink(&missing, &dangling).unwrap();
        assert!(fixture.check(Platform::Linux, &dangling.join("nightly")).is_err());
        assert!(!missing.exists());
        assert!(!stable.join("nightly").exists());
        assert_eq!(std::fs::read(stable.join("sentinel")).unwrap(), b"untouched");
    }

    #[test]
    fn the_write_probe_leaves_no_trace_and_never_reuses_a_taken_name() {
        let dir = Fixture::new().root.join("probe-target");
        ensure_writable(&dir).unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "the probe removes only what it created");
        // Every probe name taken: refuse instead of opening one.
        for attempt in 0..64u32 {
            std::fs::write(dir.join(format!(".write-test-{}-{attempt}", std::process::id())), b"taken").unwrap();
        }
        assert!(ensure_writable(&dir).is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 64);
    }
}
