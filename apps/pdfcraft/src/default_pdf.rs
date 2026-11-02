//! Read-only Windows association inspection and the supported Settings URI. Never set UserChoice.
//! https://learn.microsoft.com/windows/apps/develop/launch/launch-default-apps-settings

const PROG_ID: &str = "PdfCraft.Document";
const SETTINGS: &str = "ms-settings:defaultapps";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Registration {
    None,
    User,
    Machine,
}

fn already_default(user_choice: Option<&str>, classes_default: Option<&str>) -> bool {
    // A per-user choice overrides the Classes fallback, including another app's ProgID.
    user_choice.or(classes_default).is_some_and(|id| id.eq_ignore_ascii_case(PROG_ID))
}

fn registration(user: Option<&str>, machine: Option<&str>) -> Registration {
    if user.is_some_and(|value| !value.trim().is_empty()) {
        Registration::User
    } else if machine.is_some_and(|value| !value.trim().is_empty()) {
        Registration::Machine
    } else {
        Registration::None
    }
}

fn settings_uri(registration: Registration, build: u32, revision: u32) -> &'static str {
    // The app-specific URI was added by the April 2023 Windows 11 cumulative updates.
    let supports_app = build >= 22631 || (build == 22621 && revision >= 1555) || (build == 22000 && revision >= 1817);
    match (supports_app, registration) {
        (true, Registration::User) => "ms-settings:defaultapps?registeredAppUser=PdfCraft",
        (true, Registration::Machine) => "ms-settings:defaultapps?registeredAppMachine=PdfCraft",
        _ => SETTINGS,
    }
}

#[cfg(windows)]
mod native {
    use super::{SETTINGS, already_default, registration, settings_uri};
    use winsafe::{HKEY, RegistryValue, co};

    fn read(key: &HKEY, path: &str, name: Option<&str>) -> Result<Option<RegistryValue>, String> {
        match key.RegGetValue(Some(path), name, co::RRF::RT_ANY | co::RRF::SUBKEY_WOW6464KEY) {
            Ok(value) => Ok(Some(value)),
            Err(e) if e == co::ERROR::FILE_NOT_FOUND || e == co::ERROR::PATH_NOT_FOUND => Ok(None),
            Err(e) => Err(format!("reading the Windows PDF association: {e}")),
        }
    }

    fn text(key: &HKEY, path: &str, name: Option<&str>) -> Result<Option<String>, String> {
        match read(key, path, name)? {
            Some(RegistryValue::Sz(value)) => Ok(Some(value)),
            None => Ok(None),
            _ => Err("the Windows PDF association has an unexpected registry value type".into()),
        }
    }

    pub(super) fn is_default() -> Result<bool, String> {
        let user = text(&HKEY::CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.pdf\UserChoice", Some("ProgId"))?;
        let fallback = if user.is_none() { text(&HKEY::CLASSES_ROOT, ".pdf", None)? } else { None };
        if already_default(user.as_deref(), fallback.as_deref()) {
            return Ok(true);
        }
        // Choosing an executable through Windows' Open With browser can use this ProgID
        // instead of the MSI's named registration. Require the exact executable path.
        if user.as_deref().or(fallback.as_deref()).is_some_and(|id| id.eq_ignore_ascii_case(r"Applications\pdfcraft.exe")) {
            let command = text(&HKEY::CLASSES_ROOT, r"Applications\pdfcraft.exe\shell\open\command", None)?;
            let executable = std::env::current_exe().map_err(|e| e.to_string())?;
            return Ok(command.as_deref().is_some_and(|command| command_opens_executable(command, &executable)));
        }
        Ok(false)
    }

    pub(super) fn command_opens_executable(command: &str, executable: &std::path::Path) -> bool {
        // Parse only; never execute a registry command or probe its potentially remote path.
        // CommandLineToArgv treats an empty string as the current process command line.
        if command.trim().is_empty() {
            return false;
        }
        let Ok(args) = winsafe::CommandLineToArgv(command) else { return false };
        let Some(first) = args.first().filter(|s| !s.is_empty()) else { return false };
        let Some(current) = executable.to_str() else { return false };
        first.eq_ignore_ascii_case(current.strip_prefix(r"\\?\").unwrap_or(current))
    }

    pub(super) fn open_settings() -> Result<(), String> {
        let registered = r"Software\RegisteredApplications";
        let user = text(&HKEY::CURRENT_USER, registered, Some("PdfCraft"))?;
        let machine =
            if user.as_ref().is_none_or(|s| s.trim().is_empty()) { text(&HKEY::LOCAL_MACHINE, registered, Some("PdfCraft"))? } else { None };
        let version = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
        // If version inspection is unavailable, the generic Settings page remains supported.
        let build = text(&HKEY::LOCAL_MACHINE, version, Some("CurrentBuildNumber")).ok().flatten().and_then(|n| n.parse().ok()).unwrap_or(0);
        let revision = match read(&HKEY::LOCAL_MACHINE, version, Some("UBR")) {
            Ok(Some(RegistryValue::Dword(value))) => value,
            _ => 0,
        };
        let uri = settings_uri(registration(user.as_deref(), machine.as_deref()), build, revision);
        // Called on the window thread after an explicit UI action. main enables drag-and-drop,
        // so winit initializes STA COM with OleInitialize before constructing the window.
        let launch = |uri| {
            winsafe::ShellExecuteEx(&winsafe::SHELLEXECUTEINFO { file: uri, verb: Some("open"), show: co::SW::SHOWNORMAL, ..Default::default() })
                .map_err(|e| e.to_string())
        };
        launch(uri).or_else(|e| if uri != SETTINGS { launch(SETTINGS) } else { Err(e) })
    }
}

#[cfg(windows)]
pub fn integration(automatic_prompt: bool) -> pdfcraft_ui_egui::DefaultPdfIntegration {
    pdfcraft_ui_egui::DefaultPdfIntegration {
        is_default: std::sync::Arc::new(native::is_default),
        open_settings: std::sync::Arc::new(native::open_settings),
        automatic_prompt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_pdf_user_choice_precedes_classes_without_writing_associations() {
        assert!(already_default(Some("pdfcraft.document"), None));
        assert!(already_default(None, Some(PROG_ID)));
        assert!(!already_default(Some("Other.PDF"), Some(PROG_ID)));
        assert!(!already_default(None, None));
        assert!(!already_default(Some(""), Some(PROG_ID)));
    }

    #[cfg(windows)]
    #[test]
    fn default_pdf_open_with_requires_the_actual_executable_path() {
        let exe = std::path::Path::new(r"C:\Program Files\PdfCraft\pdfcraft.exe");
        assert!(native::command_opens_executable(r#""C:\Program Files\PdfCraft\pdfcraft.exe" "%1""#, exe));
        assert!(!native::command_opens_executable(r#""C:\Other\pdfcraft.exe" "%1""#, exe));
        assert!(!native::command_opens_executable(r#"pdfcraft.exe "%1""#, exe));
        assert!(!native::command_opens_executable("", exe));
    }

    #[test]
    fn default_pdf_registered_user_machine_and_unregistered_choose_supported_settings() {
        assert_eq!(registration(Some("Software\\PdfCraft\\Capabilities"), Some("machine")), Registration::User);
        assert_eq!(registration(Some(""), Some("machine")), Registration::Machine);
        assert_eq!(registration(None, None), Registration::None);
        for (build, revision) in [(22000, 1817), (22621, 1555), (22631, 0), (26100, 0)] {
            assert_eq!(settings_uri(Registration::Machine, build, revision), "ms-settings:defaultapps?registeredAppMachine=PdfCraft");
            assert_eq!(settings_uri(Registration::User, build, revision), "ms-settings:defaultapps?registeredAppUser=PdfCraft");
            assert_eq!(settings_uri(Registration::None, build, revision), SETTINGS);
        }
        for (build, revision) in [(0, 0), (19045, 9999), (22000, 1816), (22621, 1554)] {
            assert_eq!(settings_uri(Registration::Machine, build, revision), SETTINGS);
        }
    }
}
