//! Windows per-user (`HKCU`) PDF file association, default-app registration, and `-2` document
//! icon repair for portable and manually associated builds.
//!
//! Why this module exists:
//! `apps/pdfcraft/build.rs` embeds two icons into `pdfcraft.exe`:
//! - Resource ID `1` (index `0`): `assets/app-icon/pdfcraft.ico` (the Lion application icon).
//! - Resource ID `2` (`-2`): `assets/app-icon/pdfcraft-document.ico` (the PDF document sheet icon).
//!
//! When a user runs the portable zip or manually picks `pdfcraft.exe` in Windows Explorer
//! ("Open with ▸ Choose another app ▸ Browse"), Windows creates
//! `HKCU\Software\Classes\Applications\pdfcraft.exe\shell\open\command` (or `pdf_auto_file`)
//! without a `DefaultIcon` subkey and without `TypeOverlay = ""`. Explorer then falls back to
//! icon index `0` (the Lion app icon) for `.pdf` files and stamps the Lion icon on PDF thumbnails.
//!
//! This module registers and repairs both `HKCU\Software\Classes\PdfCraft.Document` and
//! `HKCU\Software\Classes\Applications\pdfcraft.exe` with `DefaultIcon = "\"<exe>\",-2"` and
//! `TypeOverlay = ""`, while intentionally avoiding `HKCU\Software\PdfCraft\InstallDir` and
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\App Paths\pdfcraft.exe` so the WiX MSI
//! dual-scope guard (`packaging/windows/install-policy.wxi`) is never tripped.

use std::path::{Path, PathBuf};

/// Resource ID for `pdfcraft-document.ico` embedded in `pdfcraft.exe` by `apps/pdfcraft/build.rs`.
pub const DOCUMENT_ICON_RESOURCE: &str = "-2";

/// ProgID used for PdfCraft's PDF document association.
pub const PROG_ID: &str = "PdfCraft.Document";

/// Production registry root under `HKCU`.
#[cfg(target_os = "windows")]
const DEFAULT_SOFTWARE_ROOT: &str = "Software";

/// Current status of PdfCraft's per-user (`HKCU`) Windows file association.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssociationStatus {
    /// Both `PdfCraft.Document` and `Applications\pdfcraft.exe` are registered for this executable
    /// with the `-2` PDF document icon and empty `TypeOverlay`.
    RegisteredCurrentExe,
    /// An association entry for `pdfcraft.exe` exists in `HKCU` (e.g. from manual Explorer "Open
    /// with" selection or a moved portable folder), but its `DefaultIcon`, `TypeOverlay`, or
    /// command path needs repair.
    NeedsIconOrPathRepair,
    /// A per-user MSI installation is registered at another existing directory on disk.
    MsiInstalledOtherPath(PathBuf),
    /// No per-user registration for PdfCraft exists in `HKCU`.
    Unregistered,
}

/// Strip the `\\?\` verbatim prefix that `std::fs::canonicalize` / `current_exe` may produce,
/// because Windows Explorer and `ShellExecute` expect standard Win32 paths in registry values.
pub fn clean_exe_path(exe: &Path) -> String {
    let raw = exe.to_string_lossy();
    if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        raw.into_owned()
    }
}

/// Expected `DefaultIcon` registry value for PDF documents: `"\"<exe>\",-2"`.
pub fn expected_default_icon(exe: &Path) -> String {
    format!("\"{}\",{}", clean_exe_path(exe), DOCUMENT_ICON_RESOURCE)
}

/// Expected `ApplicationIcon` registry value for Capabilities: `"\"<exe>\",0"`.
pub fn expected_app_icon(exe: &Path) -> String {
    format!("\"{}\",0", clean_exe_path(exe))
}

/// Expected `shell\open\command` registry value: `"\"<exe>\" \"%1\""`.
pub fn expected_open_command(exe: &Path) -> String {
    format!("\"{}\" \"%1\"", clean_exe_path(exe))
}

/// Extract the executable path from a `shell\open\command` or `DefaultIcon` value.
pub fn extract_exe_from_registry_value(val: &str) -> Option<String> {
    let trimmed = val.trim();
    if let Some(rest) = trimmed.strip_prefix('"') {
        let end = rest.find('"')?;
        Some(rest[..end].to_owned())
    } else {
        let end = trimmed.find([',', ' ']).unwrap_or(trimmed.len());
        let slice = trimmed[..end].trim();
        (!slice.is_empty()).then(|| slice.to_owned())
    }
}

/// Case-insensitive comparison of two Windows paths after normalizing slashes and verbatim prefixes.
pub fn same_windows_path(a: &Path, b: &Path) -> bool {
    let na = clean_exe_path(a).replace('/', r"\").to_ascii_lowercase();
    let nb = clean_exe_path(b).replace('/', r"\").to_ascii_lowercase();
    na.trim_end_matches('\\') == nb.trim_end_matches('\\')
}

/// Whether a `shell\open\command` string points to `exe`.
pub fn command_points_to_exe(cmd: &str, exe: &Path) -> bool {
    extract_exe_from_registry_value(cmd).map(|p| same_windows_path(Path::new(&p), exe)).unwrap_or(false)
}

/// Whether a `shell\open\command` string references any `pdfcraft.exe` binary.
pub fn command_mentions_pdfcraft(cmd: &str) -> bool {
    extract_exe_from_registry_value(cmd)
        .and_then(|p| Path::new(&p).file_name().map(|n| n.to_string_lossy().eq_ignore_ascii_case("pdfcraft.exe")))
        .unwrap_or(false)
}

/// Whether a `DefaultIcon` value matches `exe` with resource `-2`.
pub fn icon_matches_exe_document_icon(icon_val: &str, exe: &Path) -> bool {
    let trimmed = icon_val.trim();
    let Some((path_part, res_part)) = trimmed.rsplit_once(',') else {
        return false;
    };
    if res_part.trim() != DOCUMENT_ICON_RESOURCE {
        return false;
    }
    let unquoted = path_part.trim().trim_matches('"');
    same_windows_path(Path::new(unquoted), exe)
}

#[cfg(target_os = "windows")]
mod imp {
    use super::{
        AssociationStatus, PROG_ID, Path, PathBuf, command_mentions_pdfcraft, command_points_to_exe, expected_app_icon, expected_default_icon,
        expected_open_command, icon_matches_exe_document_icon, same_windows_path,
    };
    use std::os::windows::process::CommandExt;
    use winsafe::{
        HKEY, HWND, RegistryValue,
        co::{KEY, REG_OPTION, RRF, SW},
        prelude::Handle,
    };

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn read_sz(subkey: &str, value_name: Option<&str>) -> Option<String> {
        match HKEY::CURRENT_USER.RegGetValue(Some(subkey), value_name, RRF::RT_REG_SZ) {
            Ok(RegistryValue::Sz(s)) => Some(s),
            _ => None,
        }
    }

    fn key_exists(subkey: &str) -> bool {
        HKEY::CURRENT_USER.RegOpenKeyEx(Some(subkey), REG_OPTION::NoValue, KEY::READ).is_ok()
    }

    fn write_sz(subkey: &str, value_name: Option<&str>, data: &str) -> Result<(), String> {
        // `winsafe` 0.0.29's `WString::from_str("")` leaves its buffer unallocated (null pointer)
        // while `RegistryValue::Sz` still reports a 2-byte null terminator length, causing
        // `RegSetKeyValueW` to return `ERROR_NOACCESS` (998). Passing `"\0"` allocates `[0, 0]`,
        // `lstrlenW` returns 0 (`cbData = 2`), and Windows stores a standard empty `REG_SZ` (`""`).
        let payload = if data.is_empty() { "\0".to_owned() } else { data.to_owned() };
        HKEY::CURRENT_USER.RegSetKeyValue(Some(subkey), value_name, RegistryValue::Sz(payload)).map_err(|e| format!("RegSetKeyValue({subkey}): {e}"))
    }

    fn delete_tree_if_exists(subkey: &str) {
        let _ = HKEY::CURRENT_USER.RegDeleteTree(Some(subkey));
    }

    fn delete_value_if_exists(parent_key: &str, value_name: &str) {
        if let Ok(hkey) = HKEY::CURRENT_USER.RegOpenKeyEx(Some(parent_key), REG_OPTION::NoValue, KEY::SET_VALUE) {
            let _ = hkey.RegDeleteValue(Some(value_name));
        }
    }

    /// Check if a per-user MSI installation exists at a different valid path on disk.
    pub fn msi_other_install_dir(software_root: &str, current_exe: &Path) -> Option<PathBuf> {
        let pdfcraft_key = format!(r"{software_root}\PdfCraft");
        let install_dir = read_sz(&pdfcraft_key, Some("InstallDir"))?;
        let trimmed = install_dir.trim();
        if trimmed.is_empty() {
            return None;
        }
        let dir_path = PathBuf::from(trimmed);
        let msi_exe = dir_path.join("pdfcraft.exe");
        if !msi_exe.is_file() {
            return None;
        }
        if same_windows_path(&msi_exe, current_exe) {
            return None;
        }
        Some(dir_path)
    }

    /// Inspect `HKCU\{software_root}` and determine the association status for `current_exe`.
    pub fn status_at(software_root: &str, current_exe: &Path) -> AssociationStatus {
        if let Some(other_dir) = msi_other_install_dir(software_root, current_exe) {
            return AssociationStatus::MsiInstalledOtherPath(other_dir);
        }

        let progid_key = format!(r"{software_root}\Classes\{PROG_ID}");
        let progid_icon_key = format!(r"{progid_key}\DefaultIcon");
        let progid_cmd_key = format!(r"{progid_key}\shell\open\command");

        let app_key = format!(r"{software_root}\Classes\Applications\pdfcraft.exe");
        let app_icon_key = format!(r"{app_key}\DefaultIcon");
        let app_cmd_key = format!(r"{app_key}\shell\open\command");

        let auto_key = format!(r"{software_root}\Classes\pdf_auto_file");
        let auto_icon_key = format!(r"{auto_key}\DefaultIcon");
        let auto_cmd_key = format!(r"{auto_key}\shell\open\command");

        let progid_exists = key_exists(&progid_key);
        let app_exists = key_exists(&app_key);
        let auto_cmd = read_sz(&auto_cmd_key, None);
        let auto_mentions_pdfcraft = auto_cmd.as_deref().map(command_mentions_pdfcraft).unwrap_or(false);

        if !progid_exists && !app_exists && !auto_mentions_pdfcraft {
            return AssociationStatus::Unregistered;
        }

        let progid_ok = progid_exists
            && read_sz(&progid_key, Some("TypeOverlay")).as_deref() == Some("")
            && read_sz(&progid_icon_key, None).as_deref().map(|v| icon_matches_exe_document_icon(v, current_exe)).unwrap_or(false)
            && read_sz(&progid_cmd_key, None).as_deref().map(|v| command_points_to_exe(v, current_exe)).unwrap_or(false);

        let app_ok = app_exists
            && read_sz(&app_key, Some("TypeOverlay")).as_deref() == Some("")
            && read_sz(&app_icon_key, None).as_deref().map(|v| icon_matches_exe_document_icon(v, current_exe)).unwrap_or(false)
            && read_sz(&app_cmd_key, None).as_deref().map(|v| command_points_to_exe(v, current_exe)).unwrap_or(false);

        let auto_ok = if auto_mentions_pdfcraft {
            read_sz(&auto_key, Some("TypeOverlay")).as_deref() == Some("")
                && read_sz(&auto_icon_key, None).as_deref().map(|v| icon_matches_exe_document_icon(v, current_exe)).unwrap_or(false)
                && auto_cmd.as_deref().map(|v| command_points_to_exe(v, current_exe)).unwrap_or(false)
        } else {
            true
        };

        if progid_ok && app_ok && auto_ok { AssociationStatus::RegisteredCurrentExe } else { AssociationStatus::NeedsIconOrPathRepair }
    }

    /// Write per-user file association and default-app capability keys under `HKCU\{software_root}`.
    ///
    /// Never writes `Software\PdfCraft\InstallDir` or `App Paths\pdfcraft.exe` so the WiX MSI
    /// dual-scope check remains unaffected.
    pub fn register_at(software_root: &str, current_exe: &Path) -> Result<(), String> {
        let doc_icon = expected_default_icon(current_exe);
        let app_icon = expected_app_icon(current_exe);
        let open_cmd = expected_open_command(current_exe);

        // 1. ProgID: HKCU\{software_root}\Classes\PdfCraft.Document
        let progid_key = format!(r"{software_root}\Classes\{PROG_ID}");
        write_sz(&progid_key, None, "PDF Document")?;
        write_sz(&progid_key, Some("FriendlyTypeName"), "PDF Document")?;
        write_sz(&progid_key, Some("TypeOverlay"), "")?;
        write_sz(&format!(r"{progid_key}\DefaultIcon"), None, &doc_icon)?;
        write_sz(&format!(r"{progid_key}\shell\open\command"), None, &open_cmd)?;

        // 2. Applications\pdfcraft.exe: covers manual "Open with ▸ Choose another app" selection!
        let app_key = format!(r"{software_root}\Classes\Applications\pdfcraft.exe");
        write_sz(&app_key, Some("FriendlyAppName"), "PdfCraft")?;
        write_sz(&app_key, Some("TypeOverlay"), "")?;
        write_sz(&format!(r"{app_key}\DefaultIcon"), None, &doc_icon)?;
        write_sz(&format!(r"{app_key}\SupportedTypes"), Some(".pdf"), "")?;
        write_sz(&format!(r"{app_key}\shell\open\command"), None, &open_cmd)?;

        // 3. .pdf\OpenWithProgids
        let open_with_key = format!(r"{software_root}\Classes\.pdf\OpenWithProgids");
        write_sz(&open_with_key, Some(PROG_ID), "")?;

        // 4. Capabilities & RegisteredApplications (for Windows Settings ▸ Default Apps)
        let cap_key = format!(r"{software_root}\PdfCraft\Capabilities");
        write_sz(&cap_key, Some("ApplicationName"), "PdfCraft")?;
        write_sz(&cap_key, Some("ApplicationDescription"), "Read, annotate, sign, and organize PDF documents.")?;
        write_sz(&cap_key, Some("ApplicationIcon"), &app_icon)?;
        write_sz(&format!(r"{cap_key}\FileAssociations"), Some(".pdf"), PROG_ID)?;

        let reg_apps_key = format!(r"{software_root}\RegisteredApplications");
        write_sz(&reg_apps_key, Some("PdfCraft"), &cap_key)?;

        // 5. If Windows Explorer created `pdf_auto_file` pointing to `pdfcraft.exe`, repair its
        // DefaultIcon (-2), TypeOverlay (""), and command path too.
        let auto_key = format!(r"{software_root}\Classes\pdf_auto_file");
        let auto_cmd_key = format!(r"{auto_key}\shell\open\command");
        if read_sz(&auto_cmd_key, None).as_deref().map(command_mentions_pdfcraft).unwrap_or(false) {
            write_sz(&auto_key, Some("TypeOverlay"), "")?;
            write_sz(&format!(r"{auto_key}\DefaultIcon"), None, &doc_icon)?;
            write_sz(&auto_cmd_key, None, &open_cmd)?;
        }

        Ok(())
    }

    /// Remove per-user file association keys under `HKCU\{software_root}` (unless owned by an
    /// active MSI per-user installation).
    pub fn unregister_at(software_root: &str, current_exe: &Path) -> Result<(), String> {
        if let Some(other_dir) = msi_other_install_dir(software_root, current_exe) {
            return Err(format!("Windows association belongs to an MSI installation in {}", other_dir.display()));
        }

        delete_tree_if_exists(&format!(r"{software_root}\Classes\{PROG_ID}"));
        delete_tree_if_exists(&format!(r"{software_root}\Classes\Applications\pdfcraft.exe"));
        delete_value_if_exists(&format!(r"{software_root}\Classes\.pdf\OpenWithProgids"), PROG_ID);
        delete_tree_if_exists(&format!(r"{software_root}\PdfCraft\Capabilities"));
        delete_value_if_exists(&format!(r"{software_root}\RegisteredApplications"), "PdfCraft");

        // Clean up empty `Software\PdfCraft` key only if no MSI values (like InstallDir) remain.
        let pdfcraft_key = format!(r"{software_root}\PdfCraft");
        if read_sz(&pdfcraft_key, Some("InstallDir")).is_none() {
            delete_tree_if_exists(&pdfcraft_key);
        }

        // If `pdf_auto_file` pointed to `pdfcraft.exe`, remove its DefaultIcon override so we
        // leave no stale paths behind.
        let auto_key = format!(r"{software_root}\Classes\pdf_auto_file");
        let auto_cmd_key = format!(r"{auto_key}\shell\open\command");
        if read_sz(&auto_cmd_key, None).as_deref().map(command_mentions_pdfcraft).unwrap_or(false) {
            delete_tree_if_exists(&format!(r"{auto_key}\DefaultIcon"));
        }

        Ok(())
    }

    /// Synchronize/repair association on startup against `software_root`. Returns `true` if the
    /// registry was updated.
    pub fn sync_at(software_root: &str, current_exe: &Path, portable_mode: bool, association_disabled: bool) -> Result<bool, String> {
        match status_at(software_root, current_exe) {
            AssociationStatus::MsiInstalledOtherPath(_) | AssociationStatus::RegisteredCurrentExe => Ok(false),
            AssociationStatus::NeedsIconOrPathRepair => {
                // Always repair an existing HKCU registration (such as a manual Explorer "Open
                // with" selection missing its -2 DefaultIcon, or a portable copy moved to a new
                // folder) so PDF files never get stuck with the Lion app icon.
                register_at(software_root, current_exe)?;
                Ok(true)
            }
            AssociationStatus::Unregistered => {
                if portable_mode && !association_disabled {
                    register_at(software_root, current_exe)?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
        }
    }

    /// Ask Windows Explorer to reload its per-user file type and icon cache without restarting
    /// Explorer (`%SystemRoot%\System32\ie4uinit.exe -show`).
    pub fn refresh_shell_icons() {
        let ie4uinit =
            std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("System32").join("ie4uinit.exe");

        if ie4uinit.is_file() {
            let _ = std::process::Command::new(ie4uinit).arg("-show").creation_flags(CREATE_NO_WINDOW).spawn();
        }
    }

    /// Open Windows Settings ▸ Apps ▸ Default apps (`ms-settings:defaultapps`).
    pub fn open_default_apps_settings_window() -> Result<(), String> {
        HWND::NULL
            .ShellExecute("open", "ms-settings:defaultapps", None, None, SW::SHOWNORMAL)
            .map_err(|e| format!("opening Windows Default Apps settings: {e}"))
    }
}

/// Inspect the current user's (`HKCU`) Windows file association status for the running executable.
#[cfg(target_os = "windows")]
pub fn status() -> AssociationStatus {
    let Ok(exe) = std::env::current_exe() else {
        return AssociationStatus::Unregistered;
    };
    imp::status_at(DEFAULT_SOFTWARE_ROOT, &exe)
}

/// Register the running executable in `HKCU` with the `-2` PDF document icon and empty
/// `TypeOverlay`, then refresh Explorer's icon cache.
#[cfg(target_os = "windows")]
pub fn register_current_user() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("locating current executable: {e}"))?;
    if let Some(other_dir) = imp::msi_other_install_dir(DEFAULT_SOFTWARE_ROOT, &exe) {
        return Err(format!("PdfCraft is already installed via MSI in {}. Uninstall the MSI or use that copy instead.", other_dir.display()));
    }
    imp::register_at(DEFAULT_SOFTWARE_ROOT, &exe)?;
    imp::refresh_shell_icons();
    Ok(())
}

/// Repair the `-2` PDF document icon and `TypeOverlay` in `HKCU` for the running executable and
/// refresh Explorer's icon cache.
#[cfg(target_os = "windows")]
pub fn repair_pdf_icon() -> Result<(), String> {
    register_current_user()
}

/// Remove PdfCraft's per-user (`HKCU`) file association keys and refresh Explorer's icon cache.
#[cfg(target_os = "windows")]
pub fn unregister_current_user() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("locating current executable: {e}"))?;
    imp::unregister_at(DEFAULT_SOFTWARE_ROOT, &exe)?;
    imp::refresh_shell_icons();
    Ok(())
}

/// Register the running executable in `HKCU`, refresh Explorer's icon cache, and open Windows
/// Settings ▸ Apps ▸ Default apps (`ms-settings:defaultapps`).
#[cfg(target_os = "windows")]
pub fn register_and_open_default_apps() -> Result<(), String> {
    register_current_user()?;
    imp::open_default_apps_settings_window()
}

/// Run at startup on Windows:
/// - In Portable Mode (unless `association_disabled` is set), registers the current executable in
///   `HKCU` so Windows knows about PdfCraft and its `-2` PDF document icon.
/// - In any mode, if `HKCU` already has an entry for `pdfcraft.exe` (such as a manual Explorer
///   "Open with" selection) whose `DefaultIcon` is missing/wrong or whose path moved, repairs it
///   automatically so `.pdf` files display the PDF document icon instead of the Lion app icon.
#[cfg(target_os = "windows")]
pub fn sync_on_startup(portable_mode: bool, association_disabled: bool) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    match imp::sync_at(DEFAULT_SOFTWARE_ROOT, &exe, portable_mode, association_disabled) {
        Ok(true) => {
            log::info!("windows association: synchronized HKCU PDF association and -2 document icon");
            imp::refresh_shell_icons();
            true
        }
        Ok(false) => false,
        Err(e) => {
            log::warn!("windows association: startup sync failed: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_expected_registry_values_without_verbatim_prefix() {
        let exe = Path::new(r"\\?\D:\PortableApps\PdfCraft\pdfcraft.exe");
        assert_eq!(clean_exe_path(exe), r"D:\PortableApps\PdfCraft\pdfcraft.exe");
        assert_eq!(expected_default_icon(exe), r#""D:\PortableApps\PdfCraft\pdfcraft.exe",-2"#);
        assert_eq!(expected_app_icon(exe), r#""D:\PortableApps\PdfCraft\pdfcraft.exe",0"#);
        assert_eq!(expected_open_command(exe), r#""D:\PortableApps\PdfCraft\pdfcraft.exe" "%1""#);
    }

    #[test]
    fn matches_commands_and_icons_case_insensitively() {
        let exe = Path::new(r"E:\USB\PdfCraft\pdfcraft.exe");
        assert!(command_points_to_exe(r#""e:\usb\pdfcraft\pdfcraft.exe" "%1""#, exe));
        assert!(!command_points_to_exe(r#""C:\Other\pdfcraft.exe" "%1""#, exe));
        assert!(command_mentions_pdfcraft(r#""C:\Other\PDFCRAFT.EXE" "%1""#));
        assert!(!command_mentions_pdfcraft(r#""C:\Other\acrobat.exe" "%1""#));

        assert!(icon_matches_exe_document_icon(r#""e:\usb\pdfcraft\pdfcraft.exe",-2"#, exe));
        assert!(!icon_matches_exe_document_icon(r#""e:\usb\pdfcraft\pdfcraft.exe",0"#, exe));
    }

    #[cfg(target_os = "windows")]
    mod windows_registry_tests {
        use super::{AssociationStatus, Path, imp};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use winsafe::{HKEY, RegistryValue, co::RRF};

        struct TempRegRoot {
            root: String,
        }

        impl TempRegRoot {
            fn new(tag: &str) -> Self {
                static N: AtomicUsize = AtomicUsize::new(0);
                let id = N.fetch_add(1, Ordering::Relaxed);
                let root = format!(r"Software\PdfCraftTest_{}_{}_{}", tag, std::process::id(), id);
                let _ = HKEY::CURRENT_USER.RegDeleteTree(Some(&root));
                Self { root }
            }

            fn read_sz(&self, rel: &str, name: Option<&str>) -> Option<String> {
                let key = format!(r"{}\{rel}", self.root);
                match HKEY::CURRENT_USER.RegGetValue(Some(&key), name, RRF::RT_REG_SZ) {
                    Ok(RegistryValue::Sz(s)) => Some(s),
                    _ => None,
                }
            }

            fn write_sz(&self, rel: &str, name: Option<&str>, val: &str) {
                let key = format!(r"{}\{rel}", self.root);
                HKEY::CURRENT_USER.RegSetKeyValue(Some(&key), name, RegistryValue::Sz(val.to_owned())).unwrap();
            }
        }

        impl Drop for TempRegRoot {
            fn drop(&mut self) {
                let _ = HKEY::CURRENT_USER.RegDeleteTree(Some(&self.root));
            }
        }

        #[test]
        fn manual_explorer_open_with_without_default_icon_is_detected_and_repaired() {
            let reg = TempRegRoot::new("manual_open_with");
            let exe = Path::new(r"E:\Portable\PdfCraft\pdfcraft.exe");

            // Simulate Windows Explorer creating `Applications\pdfcraft.exe\shell\open\command`
            // without a `DefaultIcon` subkey when the user manually picks `pdfcraft.exe`.
            reg.write_sz(r"Classes\Applications\pdfcraft.exe\shell\open\command", None, r#""E:\Portable\PdfCraft\pdfcraft.exe" "%1""#);
            reg.write_sz(r"Classes\pdf_auto_file\shell\open\command", None, r#""E:\Portable\PdfCraft\pdfcraft.exe" "%1""#);

            assert_eq!(imp::status_at(&reg.root, exe), AssociationStatus::NeedsIconOrPathRepair);

            // Even with portable_mode = false, startup sync repairs the broken manual association!
            let updated = imp::sync_at(&reg.root, exe, false, false).unwrap();
            assert!(updated);
            assert_eq!(imp::status_at(&reg.root, exe), AssociationStatus::RegisteredCurrentExe);

            // Verify both `Applications\pdfcraft.exe`, `PdfCraft.Document`, and `pdf_auto_file`
            // now have `DefaultIcon = "...,-2"` and `TypeOverlay = ""`.
            assert_eq!(
                reg.read_sz(r"Classes\Applications\pdfcraft.exe\DefaultIcon", None).as_deref(),
                Some(r#""E:\Portable\PdfCraft\pdfcraft.exe",-2"#)
            );
            assert_eq!(reg.read_sz(r"Classes\Applications\pdfcraft.exe", Some("TypeOverlay")).as_deref(), Some(""));
            assert_eq!(reg.read_sz(r"Classes\PdfCraft.Document\DefaultIcon", None).as_deref(), Some(r#""E:\Portable\PdfCraft\pdfcraft.exe",-2"#));
            assert_eq!(reg.read_sz(r"Classes\pdf_auto_file\DefaultIcon", None).as_deref(), Some(r#""E:\Portable\PdfCraft\pdfcraft.exe",-2"#));
            // Must never write InstallDir (MSI dual-scope guard).
            assert_eq!(reg.read_sz("PdfCraft", Some("InstallDir")), None);
        }

        #[test]
        fn portable_startup_registers_and_updates_when_folder_moves_then_unregisters_cleanly() {
            let reg = TempRegRoot::new("portable_lifecycle");
            let old_exe = Path::new(r"D:\USB_A\PdfCraft\pdfcraft.exe");
            let new_exe = Path::new(r"E:\USB_B\PdfCraft\pdfcraft.exe");

            assert_eq!(imp::status_at(&reg.root, old_exe), AssociationStatus::Unregistered);

            // First launch in portable mode registers old_exe.
            assert!(imp::sync_at(&reg.root, old_exe, true, false).unwrap());
            assert_eq!(imp::status_at(&reg.root, old_exe), AssociationStatus::RegisteredCurrentExe);

            // Moving the portable folder to `new_exe` is detected as NeedsIconOrPathRepair and
            // updated automatically on launch.
            assert_eq!(imp::status_at(&reg.root, new_exe), AssociationStatus::NeedsIconOrPathRepair);
            assert!(imp::sync_at(&reg.root, new_exe, true, false).unwrap());
            assert_eq!(imp::status_at(&reg.root, new_exe), AssociationStatus::RegisteredCurrentExe);
            assert_eq!(reg.read_sz(r"Classes\PdfCraft.Document\DefaultIcon", None).as_deref(), Some(r#""E:\USB_B\PdfCraft\pdfcraft.exe",-2"#));

            // Unregistering removes all keys cleanly.
            imp::unregister_at(&reg.root, new_exe).unwrap();
            assert_eq!(imp::status_at(&reg.root, new_exe), AssociationStatus::Unregistered);
        }
    }
}
