//! Keeps the Windows File Explorer PDF preview handler and thumbnail provider (both in
//! `LinkcoPdfPreviewHandler.dll`, built from `packaging/windows/PreviewHandler.cs`) registered for
//! the current user.
//!
//! The installers register the handler machine-wide, but Explorer resolves a preview handler
//! through the user's default PDF app (`UserChoice`) first, and that choice is per user and can
//! change at any time. So on every start the app checks, cheaply and in the background, whether
//! the per-user registration still matches
//!
//! * the DLL beside the running executable (path, size and modification time — an update or a
//!   moved install registers again; the path is compared in an ASCII form, [`dll_path_key`],
//!   because `reg.exe` can't print most non-ASCII paths faithfully),
//! * the registration schema the DLL writes ([`SCHEMA`]), and
//! * the user's current default PDF app,
//!
//! and only when it does not, runs the DLL's own `RegisterPreviewHandler(path)` in a hidden
//! PowerShell (the registration logic lives in one place: the C# handler). That needs no
//! administrator rights: everything goes to `HKEY_CURRENT_USER`.
//!
//! Skipped in portable mode (no registry writes from a USB stick), when the DLL or
//! `pdfcraft-cli.exe` is not beside the executable (`cargo run`), and when
//! `LINKCO_NO_PREVIEW_REGISTRATION=1` is set.

/// `RegistrationSchema` in PreviewHandler.cs; bump both together.
#[cfg(any(windows, test))]
const SCHEMA: u32 = 4;

/// One value line of `reg query` output, with the key it was listed under.
#[cfg(any(windows, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct RegValue {
    key: String,
    name: String,
    kind: String,
    data: String,
}

/// Parses the text `reg query <key> [/s]` prints:
///
/// ```text
/// HKEY_CURRENT_USER\Software\Linkco\Linkco PDF Editor
///     PreviewHandlerDll    REG_SZ    C:\Program Files\Linkco\LinkcoPdfPreviewHandler.dll
///     PreviewHandlerSchema    REG_DWORD    0x4
/// ```
#[cfg(any(windows, test))]
fn parse_reg_query(output: &str) -> Vec<RegValue> {
    let mut key = String::new();
    let mut values = Vec::new();
    for line in output.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with("HKEY_") {
            key = line.trim_end().to_string();
            continue;
        }
        let Some(rest) = line.strip_prefix("    ") else { continue };
        let Some((name, typed)) = rest.split_once("    REG_") else { continue };
        let (kind, data) = match typed.split_once("    ") {
            Some((kind, data)) => (kind, data),
            None => (typed.trim_end(), ""),
        };
        values.push(RegValue { key: key.clone(), name: name.to_string(), kind: format!("REG_{kind}"), data: data.to_string() });
    }
    values
}

/// The value `name` listed directly under a key whose path ends with `key_suffix` (ASCII
/// case-insensitive, as registry paths are).
#[cfg(any(windows, test))]
fn entry<'a>(values: &'a [RegValue], key_suffix: &str, name: &str) -> Option<&'a RegValue> {
    let suffix = key_suffix.to_ascii_lowercase();
    values.iter().find(|v| v.key.to_ascii_lowercase().ends_with(&suffix) && v.name.eq_ignore_ascii_case(name))
}

/// The data of [`entry`].
#[cfg(any(windows, test))]
fn value<'a>(values: &'a [RegValue], key_suffix: &str, name: &str) -> Option<&'a str> {
    entry(values, key_suffix, name).map(|v| v.data.as_str())
}

/// The user's default app for `.pdf` from `reg query …\FileExts\.pdf /s`, resolved the way
/// PreviewHandler.cs's `ReadUserChoiceProgId` does: `UserChoiceLatest` (Windows 11 24H2+, value or
/// `ProgId` subkey), then `UserChoice`; empty when there is none.
#[cfg(any(windows, test))]
fn user_choice_prog_id(values: &[RegValue]) -> String {
    [r"\.pdf\UserChoiceLatest", r"\.pdf\UserChoiceLatest\ProgId", r"\.pdf\UserChoice"]
        .iter()
        .find_map(|key| value(values, key, "ProgId").filter(|p| !p.is_empty()))
        .unwrap_or_default()
        .to_string()
}

/// `PreviewHandlerDllStamp` as PreviewHandler.cs writes it: `"<length>:<UTC FILETIME>"`.
#[cfg(any(windows, test))]
fn dll_stamp(len: u64, modified: std::time::SystemTime) -> Option<String> {
    /// 100 ns intervals between 1601-01-01 (FILETIME) and 1970-01-01 (Unix).
    const FILETIME_UNIX_EPOCH: u128 = 116_444_736_000_000_000;
    let since_unix = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(format!("{len}:{}", since_unix.as_nanos() / 100 + FILETIME_UNIX_EPOCH))
}

/// `PreviewHandlerDllKey` as PreviewHandler.cs's `DllPathKey` writes it, from the path's UTF-16
/// code units: ASCII letters upper-cased, `%`, control characters and every non-ASCII unit as
/// `%XXXX`. Pure ASCII, so it survives `reg.exe`, which prints in the console's OEM code page and
/// turns e.g. an Arabic user name in `C:\Users\…` into `?`s; comparing the path itself would then
/// fail, and the app would register again (PowerShell, an Explorer refresh) on every start.
#[cfg(any(windows, test))]
fn dll_path_key(utf16: impl IntoIterator<Item = u16>) -> String {
    let mut key = String::new();
    for unit in utf16 {
        match u8::try_from(unit).ok().filter(|b| (0x20..=0x7E).contains(b) && *b != b'%') {
            Some(b) => key.push(char::from(b.to_ascii_uppercase())),
            None => key.push_str(&format!("%{unit:04X}")),
        }
    }
    key
}

/// Whether the per-user registration recorded under `HKCU\Software\Linkco\Linkco PDF Editor`
/// (`config`) is current for the DLL whose [`dll_path_key`] is `dll_key`, with `stamp` and the
/// user's default app `user_choice`, and the COM server it points at (`codebase_registered`)
/// still exists.
#[cfg(any(windows, test))]
fn registry_values_match(config: &[RegValue], dll_key: &str, stamp: &str, user_choice: &str, codebase_registered: bool) -> bool {
    let key = r"\Software\Linkco\Linkco PDF Editor";
    let schema = entry(config, key, "PreviewHandlerSchema")
        .filter(|v| v.kind == "REG_DWORD")
        .and_then(|v| v.data.strip_prefix("0x"))
        .and_then(|hex| u32::from_str_radix(hex, 16).ok());
    codebase_registered
        && schema == Some(SCHEMA)
        && value(config, key, "PreviewHandlerDllKey") == Some(dll_key)
        && value(config, key, "PreviewHandlerDllStamp") == Some(stamp)
        && value(config, key, "PreviewHandlerUserChoice").unwrap_or_default().eq_ignore_ascii_case(user_choice)
}

/// Checks the registration and, when needed, registers the handler — on a background thread, so
/// start-up never waits for the registry or PowerShell.
#[cfg(windows)]
pub fn ensure_registered() {
    if pdfcraft_ui_egui::portable::data_dir().is_some() || std::env::var_os("LINKCO_NO_PREVIEW_REGISTRATION").is_some_and(|v| v == "1") {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(dir) = exe.parent() else { return };
    let dll = dir.join("LinkcoPdfPreviewHandler.dll");
    if !dll.is_file() || !dir.join("pdfcraft-cli.exe").is_file() {
        return;
    }
    let spawned = std::thread::Builder::new().name("linkco-preview-reg".into()).spawn(move || {
        if is_current(&dll) {
            log::debug!("File Explorer preview handler registration is current ({})", dll.display());
            return;
        }
        match register(&dll) {
            Ok(()) => log::info!("registered the File Explorer PDF preview handler {}", dll.display()),
            Err(e) => log::warn!("registering the File Explorer PDF preview handler {}: {e}", dll.display()),
        }
    });
    if let Err(e) = spawned {
        log::warn!("starting the preview handler registration thread: {e}");
    }
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `%SystemRoot%\System32\<rest>`, so a same-named program earlier in `PATH` is never run.
#[cfg(windows)]
fn system32(rest: &str, fallback: &str) -> std::path::PathBuf {
    std::env::var_os("SystemRoot")
        .map(|root| std::path::PathBuf::from(root).join("System32").join(rest))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| std::path::PathBuf::from(fallback))
}

/// `reg query <args>`: its parsed values, or `None` when the key does not exist.
#[cfg(windows)]
fn reg_query(args: &[&str]) -> Option<Vec<RegValue>> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new(system32("reg.exe", "reg.exe"))
        .arg("query")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| parse_reg_query(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(windows)]
fn is_current(dll: &std::path::Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    let Some(stamp) = std::fs::metadata(dll).ok().and_then(|m| dll_stamp(m.len(), m.modified().ok()?)) else { return false };
    let Some(config) = reg_query(&[r"HKCU\Software\Linkco\Linkco PDF Editor"]) else { return false };
    let codebase = reg_query(&[r"HKCU\Software\Classes\CLSID\{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}\InprocServer32", "/v", "CodeBase"]).is_some();
    let choice = reg_query(&[r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.pdf", "/s"])
        .map(|values| user_choice_prog_id(&values))
        .unwrap_or_default();
    registry_values_match(&config, &dll_path_key(dll.as_os_str().encode_wide()), &stamp, &choice, codebase)
}

/// Runs the DLL's `RegisterPreviewHandler(path)` (per user) in a hidden PowerShell. The DLL is
/// unblocked first (a downloaded ZIP marks it as from the internet, and .NET won't load such a
/// file) and loaded from its bytes, so this process never locks it.
#[cfg(windows)]
fn register(dll: &std::path::Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let dll = dll.to_string_lossy().replace('\'', "''");
    let script = format!(
        "$ErrorActionPreference = 'Stop'; $dll = '{dll}'; \
         Unblock-File -LiteralPath $dll -ErrorAction SilentlyContinue; \
         $asm = [System.Reflection.Assembly]::Load([System.IO.File]::ReadAllBytes($dll)); \
         $t = $asm.GetType('LinkcoPdfPreview.LinkcoPdfPreviewHandler', $true); \
         $t.GetMethod('RegisterPreviewHandler', [type[]]@([string])).Invoke($null, @($dll)) | Out-Null"
    );
    let out = std::process::Command::new(system32(r"WindowsPowerShell\v1.0\powershell.exe", "powershell.exe"))
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("starting PowerShell: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        Err(format!("PowerShell exited with {}: {}", out.status, stderr.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "\r\nHKEY_CURRENT_USER\\Software\\Linkco\\Linkco PDF Editor\r\n    InstallDir    REG_SZ    C:\\Program Files\\Linkco\\Linkco PDF Editor\r\n    PreviewHandlerDll    REG_SZ    C:\\Program Files\\Linkco\\Linkco PDF Editor\\LinkcoPdfPreviewHandler.dll\r\n    PreviewHandlerDllKey    REG_SZ    C:\\PROGRAM FILES\\LINKCO\\LINKCO PDF EDITOR\\LINKCOPDFPREVIEWHANDLER.DLL\r\n    PreviewHandlerSchema    REG_DWORD    0x4\r\n    PreviewHandlerDllStamp    REG_SZ    123:456\r\n    PreviewHandlerUserChoice    REG_SZ    LinkcoPDFEditor.Document\r\n\r\n";
    const DLL: &str = "C:\\Program Files\\Linkco\\Linkco PDF Editor\\LinkcoPdfPreviewHandler.dll";

    fn key(path: &str) -> String {
        dll_path_key(path.encode_utf16())
    }

    #[test]
    fn parses_reg_query_output() {
        let v = parse_reg_query(CONFIG);
        assert_eq!(v.len(), 6);
        assert_eq!(v[0].key, "HKEY_CURRENT_USER\\Software\\Linkco\\Linkco PDF Editor");
        assert_eq!(v[1].name, "PreviewHandlerDll");
        assert_eq!(v[1].kind, "REG_SZ");
        assert_eq!(v[1].data, DLL);
        assert_eq!(v[3].kind, "REG_DWORD");
        assert_eq!(v[3].data, "0x4");
    }

    #[test]
    fn empty_string_values_parse_as_empty() {
        let v = parse_reg_query("HKEY_CURRENT_USER\\X\r\n    PreviewHandlerUserChoice    REG_SZ    \r\n    Other    REG_SZ\r\n");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].data, "");
        assert_eq!(v[1].data, "");
    }

    #[test]
    fn current_registration_matches() {
        let v = parse_reg_query(CONFIG);
        assert!(registry_values_match(&v, &key(DLL), "123:456", "LinkcoPDFEditor.Document", true));
        // Paths and ProgIds compare case-insensitively, like Windows does.
        assert!(registry_values_match(&v, &key(&DLL.to_ascii_lowercase()), "123:456", "linkcopdfeditor.document", true));
    }

    #[test]
    fn any_change_requires_registering_again() {
        let v = parse_reg_query(CONFIG);
        let dll = key(DLL);
        assert!(!registry_values_match(&v, &key("D:\\Other\\LinkcoPdfPreviewHandler.dll"), "123:456", "LinkcoPDFEditor.Document", true), "moved install");
        assert!(!registry_values_match(&v, &dll, "124:456", "LinkcoPDFEditor.Document", true), "updated DLL");
        assert!(!registry_values_match(&v, &dll, "123:456", "AcroExch.Document.DC", true), "new default PDF app");
        assert!(!registry_values_match(&v, &dll, "123:456", "LinkcoPDFEditor.Document", false), "COM server removed");
        let old_schema = parse_reg_query(&CONFIG.replace("0x4", "0x3"));
        assert!(!registry_values_match(&old_schema, &dll, "123:456", "LinkcoPDFEditor.Document", true), "older schema");
        let no_key: String = CONFIG.lines().filter(|l| !l.contains("PreviewHandlerDllKey")).collect::<Vec<_>>().join("\r\n");
        assert!(!registry_values_match(&parse_reg_query(&no_key), &dll, "123:456", "LinkcoPDFEditor.Document", true), "registered by an older build");
        assert!(!registry_values_match(&[], &dll, "123:456", "", true), "never registered");
    }

    #[test]
    fn no_default_app_matches_an_empty_recorded_choice() {
        let v = parse_reg_query(&CONFIG.replace("REG_SZ    LinkcoPDFEditor.Document", "REG_SZ    "));
        assert!(registry_values_match(&v, &key(DLL), "123:456", "", true));
    }

    /// The same vectors as csharp-check's test of PreviewHandler.cs's `DllPathKey`.
    #[test]
    fn dll_path_key_is_ascii_and_matches_the_handler() {
        assert_eq!(key(DLL), "C:\\PROGRAM FILES\\LINKCO\\LINKCO PDF EDITOR\\LINKCOPDFPREVIEWHANDLER.DLL");
        assert_eq!(
            key("C:\\Users\\\u{645}\u{62d}\u{645}\u{62f}\\AppData\\Local\\Programs\\Linkco\\Linkco PDF Editor\\LinkcoPdfPreviewHandler.dll"),
            "C:\\USERS\\%0645%062D%0645%062F\\APPDATA\\LOCAL\\PROGRAMS\\LINKCO\\LINKCO PDF EDITOR\\LINKCOPDFPREVIEWHANDLER.DLL"
        );
        assert_eq!(key("D:\\100% Tools\\\u{dc}n\u{ef}code \u{1f600}\\x.dll"), "D:\\100%0025 TOOLS\\%00DCN%00EFCODE %D83D%DE00\\X.DLL");
        // An Arabic install path, as reg.exe prints it (OEM code page), still matches through the key.
        let arabic = "C:\\Users\\\u{645}\u{62d}\u{645}\u{62f}\\LinkcoPdfPreviewHandler.dll";
        let reg_output = format!(
            "HKEY_CURRENT_USER\\Software\\Linkco\\Linkco PDF Editor\r\n    PreviewHandlerDll    REG_SZ    C:\\Users\\????\\LinkcoPdfPreviewHandler.dll\r\n    PreviewHandlerDllKey    REG_SZ    {}\r\n    PreviewHandlerSchema    REG_DWORD    0x4\r\n    PreviewHandlerDllStamp    REG_SZ    1:2\r\n    PreviewHandlerUserChoice    REG_SZ    \r\n",
            key(arabic)
        );
        assert!(registry_values_match(&parse_reg_query(&reg_output), &key(arabic), "1:2", "", true));
        assert!(key(arabic).is_ascii());
    }

    #[test]
    fn user_choice_latest_wins_over_user_choice() {
        let out = "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\.pdf\r\n\r\n\
                   HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\.pdf\\OpenWithProgids\r\n    MSEdgePDF    REG_NONE    \r\n\r\n\
                   HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\.pdf\\UserChoice\r\n    Hash    REG_SZ    abc=\r\n    ProgId    REG_SZ    MSEdgePDF\r\n\r\n\
                   HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\.pdf\\UserChoiceLatest\r\n    Hash    REG_SZ    def=\r\n\r\n\
                   HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\.pdf\\UserChoiceLatest\\ProgId\r\n    ProgId    REG_SZ    LinkcoPDFEditor.Document\r\n";
        assert_eq!(user_choice_prog_id(&parse_reg_query(out)), "LinkcoPDFEditor.Document");
        let without_latest: String = out.split("\r\n\r\n").take(3).collect::<Vec<_>>().join("\r\n\r\n");
        assert_eq!(user_choice_prog_id(&parse_reg_query(&without_latest)), "MSEdgePDF");
        assert_eq!(user_choice_prog_id(&[]), "");
    }

    #[test]
    fn stamp_is_length_and_utc_filetime() {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_nanos(1_700_000_000_123_456_700);
        assert_eq!(dll_stamp(42, t).as_deref(), Some("42:133444736001234567"));
        assert_eq!(dll_stamp(1, std::time::UNIX_EPOCH).as_deref(), Some("1:116444736000000000"));
    }
}
