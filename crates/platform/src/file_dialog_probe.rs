//! Why a native file dialog came back empty on Linux (#3988).
//!
//! `rfd` asks the XDG desktop portal for a file chooser and falls back to `zenity` only when it
//! can't reach the portal at all. When the portal runs but the backend that draws its file chooser
//! doesn't (`xdg-desktop-portal-gtk` started without a display, #3988), the portal still lists
//! `org.freedesktop.portal.FileChooser` and accepts the request, then answers it with response 2
//! ("ended in some other way"). rfd 0.17 returns the same empty answer for that as for Cancel and
//! doesn't try zenity, so the app can't tell the two apart from the answer alone.
//!
//! [`why_empty`] runs after an empty answer, off the UI thread, and names what is missing. It only
//! reads bus state: it never starts a backend, since asking one directly would D-Bus-activate
//! every installed backend after every Cancel. A backend that just answered a request is still
//! running, so "none of them is running" means the request could not have been shown. Anything it
//! can't tell counts as working, so a cancelled dialog on a working system never shows a message.

/// What kept a file dialog from showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// No desktop portal offers a file chooser and `zenity` (rfd's fallback) isn't installed.
    NoChooser,
    /// The portal offers a file chooser, but none of the backends that implement it is running.
    /// Holds their package names (`xdg-desktop-portal-gtk`), from their `.portal` files.
    BackendDown(Vec<String>),
}

/// What the probe found. `None` means it couldn't tell.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Findings {
    /// The portal lists `org.freedesktop.portal.FileChooser`.
    pub portal: Option<bool>,
    /// Each installed file-chooser backend's package name and whether its bus name has an owner.
    pub backends: Vec<(String, Option<bool>)>,
    /// `zenity` is on `PATH`.
    pub zenity: bool,
}

/// The problem `findings` prove, if any.
pub fn problem(findings: &Findings) -> Option<Problem> {
    match findings.portal {
        // rfd falls back to zenity when it can't reach a file-chooser portal.
        Some(false) => (!findings.zenity).then_some(Problem::NoChooser),
        // rfd 0.17 never falls back to zenity once the portal took the request.
        Some(true) if !findings.backends.is_empty() && findings.backends.iter().all(|(_, running)| *running == Some(false)) => {
            Some(Problem::BackendDown(findings.backends.iter().map(|(name, _)| name.clone()).collect()))
        }
        _ => None,
    }
}

/// Why the file dialog that just came back empty didn't show, or `None` when it may simply have
/// been cancelled. Blocks for at most a few seconds of D-Bus calls: call it off the UI thread.
#[cfg(target_os = "linux")]
pub fn why_empty() -> Option<Problem> {
    // Flatpak's bus proxy hides the backends' names and Snap's AppArmor rules refuse the
    // question, so inside either one a running backend would look missing.
    if std::path::Path::new("/.flatpak-info").exists() || std::env::var_os("SNAP").is_some() {
        return None;
    }
    let zenity = std::env::var_os("PATH").is_some_and(|path| on_path("zenity", &path));
    problem(&linux::findings(zenity))
}

/// Other platforms show their own panels, which report no such failure.
#[cfg(not(target_os = "linux"))]
pub fn why_empty() -> Option<Problem> {
    None
}

#[cfg(any(target_os = "linux", test))]
fn on_path(program: &str, path: &std::ffi::OsStr) -> bool {
    std::env::split_paths(path).any(|dir| dir.join(program).is_file())
}

/// The `.portal` files the portal reads, in the order it reads them: `XDG_DESKTOP_PORTAL_DIR`
/// alone when set, else `portals` under the user's and the system's data folders.
#[cfg(any(target_os = "linux", test))]
fn portal_dirs(
    portal_dir: Option<std::ffi::OsString>,
    data_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    data_dirs: Option<std::ffi::OsString>,
) -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    if let Some(dir) = portal_dir.filter(|d| !d.is_empty()) {
        return vec![PathBuf::from(dir)];
    }
    let user = match data_home.filter(|d| !d.is_empty()) {
        Some(d) => Some(PathBuf::from(d)),
        None => home.filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join(".local/share")),
    };
    let system = data_dirs.filter(|d| !d.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    user.into_iter().chain(std::env::split_paths(&system)).map(|d| d.join("xdg-desktop-portal/portals")).collect()
}

/// The bus name of a `.portal` file's backend when it implements the file chooser.
#[cfg(any(target_os = "linux", test))]
fn file_chooser_backend(text: &str) -> Option<&str> {
    let mut name = None;
    let mut chooser = false;
    let mut in_portal = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_portal = line == "[portal]";
        } else if in_portal && let Some((key, value)) = line.split_once('=') {
            match key.trim() {
                "DBusName" => name = Some(value.trim()),
                "Interfaces" => chooser = value.split(';').any(|i| i.trim() == "org.freedesktop.impl.portal.FileChooser"),
                _ => {}
            }
        }
    }
    name.filter(|n| chooser && !n.is_empty())
}

/// `gtk.portal` → `xdg-desktop-portal-gtk`, the package that installs it.
#[cfg(any(target_os = "linux", test))]
fn package_name(portal_file: &std::path::Path) -> Option<String> {
    portal_file.file_stem().and_then(|s| s.to_str()).filter(|s| !s.is_empty()).map(|s| format!("xdg-desktop-portal-{s}"))
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{Findings, file_chooser_backend, package_name, portal_dirs};
    use std::time::Duration;

    const PORTAL: &str = "org.freedesktop.portal.Desktop";
    const PATH: &str = "/org/freedesktop/portal/desktop";
    /// Bounds the directory walk; a desktop installs a handful of backends.
    const MAX_FILES: usize = 64;
    /// A `.portal` file is a few lines; anything bigger isn't one.
    const MAX_FILE_BYTES: u64 = 64 * 1024;

    pub(super) fn findings(zenity: bool) -> Findings {
        let backends = installed_backends();
        futures_lite::future::block_on(async {
            let connection = match zbus::connection::Builder::session() {
                Ok(b) => b.method_timeout(Duration::from_secs(3)).build().await,
                Err(e) => Err(e),
            };
            let Ok(connection) = connection else {
                // No session bus: rfd can't reach a portal either and goes straight to zenity.
                return Findings { portal: Some(false), backends: Vec::new(), zenity };
            };
            let portal = file_chooser_listed(&connection).await;
            let mut running = Vec::with_capacity(backends.len());
            for (package, bus_name) in backends {
                running.push((package, has_owner(&connection, &bus_name).await));
            }
            Findings { portal, backends: running, zenity }
        })
    }

    /// Whether the portal lists the file chooser. A portal that isn't installed is absent; one
    /// that can't be asked is unknown.
    async fn file_chooser_listed(connection: &zbus::Connection) -> Option<bool> {
        match connection.call_method(Some(PORTAL), PATH, Some("org.freedesktop.DBus.Introspectable"), "Introspect", &()).await {
            Ok(reply) => reply.body().deserialize::<String>().ok().map(|xml| xml.contains("\"org.freedesktop.portal.FileChooser\"")),
            Err(e) => absent(&e).then_some(false),
        }
    }

    /// Whether `bus_name` has an owner, asked of the bus itself so nothing gets activated.
    async fn has_owner(connection: &zbus::Connection, bus_name: &str) -> Option<bool> {
        let reply = connection
            .call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "NameHasOwner", &bus_name)
            .await;
        reply.ok().and_then(|r| r.body().deserialize::<bool>().ok())
    }

    /// The error a call gets when nothing provides the name (as `MethodError` or as `FDO`).
    fn absent(e: &zbus::Error) -> bool {
        let name = match e {
            zbus::Error::MethodError(name, _, _) => name.to_string(),
            zbus::Error::FDO(e) => zbus::DBusError::name(&**e).to_string(),
            _ => return false,
        };
        matches!(name.as_str(), "org.freedesktop.DBus.Error.ServiceUnknown" | "org.freedesktop.DBus.Error.NameHasNoOwner")
            || name.starts_with("org.freedesktop.DBus.Error.Spawn.")
    }

    /// Each installed backend that implements the file chooser, as (package, bus name). A name
    /// in several folders counts once, from the first, as the portal reads them.
    fn installed_backends() -> Vec<(String, String)> {
        let dirs = portal_dirs(
            std::env::var_os("XDG_DESKTOP_PORTAL_DIR"),
            std::env::var_os("XDG_DATA_HOME"),
            std::env::var_os("HOME"),
            std::env::var_os("XDG_DATA_DIRS"),
        );
        let mut found: Vec<(String, String)> = Vec::new();
        let mut seen = 0;
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            let mut files: Vec<_> =
                entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "portal")).collect();
            files.sort();
            for file in files {
                seen += 1;
                if seen > MAX_FILES {
                    return found;
                }
                if !std::fs::metadata(&file).is_ok_and(|m| m.len() <= MAX_FILE_BYTES) {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&file) else { continue };
                if let (Some(bus_name), Some(package)) = (file_chooser_backend(&text), package_name(&file))
                    && !found.iter().any(|(_, n)| n == bus_name)
                {
                    found.push((package, bus_name.to_string()));
                }
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(portal: Option<bool>, backends: &[(&str, Option<bool>)], zenity: bool) -> Findings {
        Findings { portal, backends: backends.iter().map(|(n, r)| (n.to_string(), *r)).collect(), zenity }
    }

    #[test]
    fn a_running_backend_means_the_empty_answer_was_a_cancel() {
        let gtk = "xdg-desktop-portal-gtk";
        assert_eq!(problem(&found(Some(true), &[(gtk, Some(true))], false)), None);
        // An unused backend that isn't running doesn't matter while another one is.
        assert_eq!(problem(&found(Some(true), &[("xdg-desktop-portal-kde", Some(false)), (gtk, Some(true))], false)), None);
    }

    #[test]
    fn no_running_backend_is_reported_with_the_packages_to_check() {
        let f = found(Some(true), &[("xdg-desktop-portal-gtk", Some(false)), ("xdg-desktop-portal-kde", Some(false))], true);
        // zenity doesn't help: rfd doesn't fall back to it once the portal took the request.
        assert_eq!(problem(&f), Some(Problem::BackendDown(vec!["xdg-desktop-portal-gtk".into(), "xdg-desktop-portal-kde".into()])));
    }

    #[test]
    fn anything_the_probe_cannot_tell_counts_as_working() {
        assert_eq!(problem(&found(None, &[], false)), None, "portal unknown");
        assert_eq!(problem(&found(Some(true), &[], false)), None, "no .portal files found");
        assert_eq!(problem(&found(Some(true), &[("xdg-desktop-portal-gtk", None)], false)), None, "owner unknown");
        assert_eq!(problem(&found(Some(true), &[("a", Some(false)), ("b", None)], false)), None, "one unknown");
    }

    #[test]
    fn without_a_portal_zenity_decides() {
        assert_eq!(problem(&found(Some(false), &[], true)), None);
        assert_eq!(problem(&found(Some(false), &[], false)), Some(Problem::NoChooser));
    }

    #[test]
    fn portal_files_name_their_file_chooser_backend() {
        let gtk = "[portal]\nDBusName=org.freedesktop.impl.portal.desktop.gtk\nInterfaces=org.freedesktop.impl.portal.FileChooser;org.freedesktop.impl.portal.AppChooser;\nUseIn=gnome\n";
        assert_eq!(file_chooser_backend(gtk), Some("org.freedesktop.impl.portal.desktop.gtk"));
        let keyring = "[portal]\nDBusName=org.freedesktop.impl.portal.desktop.gnome-keyring\nInterfaces=org.freedesktop.impl.portal.Secret\n";
        assert_eq!(file_chooser_backend(keyring), None, "no file chooser");
        // Keys outside [portal], a prefix of the interface name, empty or missing names.
        assert_eq!(file_chooser_backend("[other]\nDBusName=x\nInterfaces=org.freedesktop.impl.portal.FileChooser\n"), None);
        assert_eq!(file_chooser_backend("[portal]\nDBusName=x\nInterfaces=org.freedesktop.impl.portal.FileChooserX\n"), None);
        assert_eq!(file_chooser_backend("[portal]\nDBusName=\nInterfaces=org.freedesktop.impl.portal.FileChooser\n"), None);
        assert_eq!(file_chooser_backend("[portal]\nInterfaces=org.freedesktop.impl.portal.FileChooser\n"), None);
        assert_eq!(file_chooser_backend(""), None);
        assert_eq!(file_chooser_backend(" [portal] \r\n DBusName = y \r\nInterfaces = org.freedesktop.impl.portal.FileChooser ; \r\n"), Some("y"));
    }

    #[test]
    fn package_names_come_from_portal_file_names() {
        assert_eq!(package_name(std::path::Path::new("/usr/share/xdg-desktop-portal/portals/gtk.portal")).as_deref(), Some("xdg-desktop-portal-gtk"));
        assert_eq!(package_name(std::path::Path::new("/")), None);
    }

    #[test]
    fn portal_dirs_follow_the_portals_own_order() {
        let s = |v: &str| Some(std::ffi::OsString::from(v));
        let p = |v: &str| std::path::PathBuf::from(v);
        assert_eq!(portal_dirs(s("/x"), s("/h"), s("/home/u"), s("/a:/b")), vec![p("/x")], "XDG_DESKTOP_PORTAL_DIR replaces the rest");
        assert_eq!(
            portal_dirs(None, None, s("/home/u"), None),
            vec![
                p("/home/u/.local/share/xdg-desktop-portal/portals"),
                p("/usr/local/share/xdg-desktop-portal/portals"),
                p("/usr/share/xdg-desktop-portal/portals")
            ]
        );
        assert_eq!(
            portal_dirs(s(""), s("/h"), None, s("/a")),
            vec![p("/h/xdg-desktop-portal/portals"), p("/a/xdg-desktop-portal/portals")],
            "empty variables count as unset"
        );
        assert_eq!(portal_dirs(None, None, None, None).len(), 2);
    }

    #[test]
    fn zenity_is_found_on_path() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = std::env::join_paths([dir.join("missing"), dir.clone()]).unwrap();
        assert!(!on_path("zenity", &path));
        std::fs::write(dir.join("zenity"), b"").unwrap();
        assert!(on_path("zenity", &path));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
