//! Which windowing backend winit starts on Linux and the BSDs.
//!
//! winit 0.30 has no file drag-and-drop on Wayland (winit#1881), so on a Wayland desktop that also
//! runs XWayland, PdfCraft starts on X11 and file drops from the file manager work. It does that
//! only after connecting to the X server named by `DISPLAY`: winit can create its event loop once
//! per process, so a stale or forwarded `DISPLAY` must not get as far as winit, or the app would
//! have no window at all instead of a native Wayland one.
//!
//! Overrides, first match wins:
//! - `PDFCRAFT_WAYLAND` (any non-empty value): never force X11; winit's own choice (Wayland first).
//! - `WINIT_UNIX_BACKEND=x11` or `=wayland`: that backend (winit 0.29 dropped the variable; it is
//!   read here so the familiar override still works).
//!
//! Nothing here sets or clears an environment variable: child processes see the user's own.

/// The backend to ask winit for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Backend {
    /// winit's own choice: Wayland when `WAYLAND_DISPLAY` or `WAYLAND_SOCKET` is set, then X11.
    Auto,
    /// X11, through XWayland on a Wayland desktop.
    X11,
    /// Native Wayland.
    Wayland,
}

/// The backend for this process, from its environment.
pub(crate) fn from_env() -> Backend {
    if cfg!(any(target_os = "macos", target_os = "windows")) {
        return Backend::Auto;
    }
    let backend = choose(|name| std::env::var(name).ok(), x11_reachable);
    log::info!("windowing backend: {backend:?}");
    backend
}

/// The backend for the environment `var` describes. `x11_reachable` is asked, with `DISPLAY`'s
/// value, only when the answer depends on it.
pub(crate) fn choose(var: impl Fn(&str) -> Option<String>, x11_reachable: impl FnOnce(&str) -> bool) -> Backend {
    // Empty variables count as unset, as winit counts them.
    let set = |name: &str| var(name).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    if set("PDFCRAFT_WAYLAND").is_some() {
        return Backend::Auto;
    }
    match set("WINIT_UNIX_BACKEND").map(|v| v.to_ascii_lowercase()).as_deref() {
        Some("x11") => return Backend::X11,
        Some("wayland") => return Backend::Wayland,
        _ => {}
    }
    let wayland = set("WAYLAND_DISPLAY").is_some() || set("WAYLAND_SOCKET").is_some();
    // A session that says it is X11 (or a console) keeps winit's choice, even with a nested
    // Wayland compositor's WAYLAND_DISPLAY around.
    let session_allows_wayland = !matches!(set("XDG_SESSION_TYPE").map(|v| v.to_ascii_lowercase()).as_deref(), Some("x11" | "tty"));
    match set("DISPLAY") {
        // Without Wayland winit already picks X11; with it, XWayland only when it answers.
        Some(display) if wayland && session_allows_wayland && x11_reachable(&display) => Backend::X11,
        _ => Backend::Auto,
    }
}

/// Applies the choice to eframe's event loop; `Auto` leaves winit's default alone.
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn apply(backend: Backend, native: &mut eframe::NativeOptions) {
    match backend {
        Backend::Auto => {}
        Backend::X11 => {
            native.event_loop_builder = Some(Box::new(|b| {
                use winit::platform::x11::EventLoopBuilderExtX11;
                b.with_x11();
            }));
        }
        Backend::Wayland => {
            native.event_loop_builder = Some(Box::new(|b| {
                use winit::platform::wayland::EventLoopBuilderExtWayland;
                b.with_wayland();
            }));
        }
    }
}

/// Where an X display listens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Endpoint {
    /// A local display number: `/tmp/.X11-unix/X<n>` (and its Linux abstract twin).
    Local(u16),
    /// A remote display: host and TCP port (6000 + display number).
    Tcp(String, u16),
}

/// The endpoint `DISPLAY` names (`[protocol/][host]:display[.screen]`), or `None` for a form
/// PdfCraft doesn't probe (which then counts as unreachable, so winit's own choice stands).
pub(crate) fn endpoint(display: &str) -> Option<Endpoint> {
    let (host, number) = display.rsplit_once(':')?;
    let number = number.split('.').next()?;
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let number: u16 = number.parse().ok()?;
    let (protocol, host) = match host.split_once('/') {
        Some((protocol, host)) => (Some(protocol), host),
        None => (None, host),
    };
    match (protocol, host) {
        (Some("unix") | None, "" | "unix") => Some(Endpoint::Local(number)),
        (Some("unix"), _) => None,
        (Some("tcp" | "inet" | "inet6") | None, host) => {
            let host = host.trim_start_matches('[').trim_end_matches(']');
            if host.is_empty() || host.starts_with('/') {
                return None;
            }
            Some(Endpoint::Tcp(host.to_owned(), 6000u16.checked_add(number)?))
        }
        _ => None,
    }
}

/// Whether something accepts connections at the X display `DISPLAY` names. A connection, not the
/// X handshake: enough to tell a live XWayland from a stale variable, and quick.
pub(crate) fn x11_reachable(display: &str) -> bool {
    match endpoint(display) {
        Some(Endpoint::Local(n)) => local_reachable(&std::path::Path::new("/tmp/.X11-unix").join(format!("X{n}"))),
        Some(Endpoint::Tcp(host, port)) => tcp_reachable(&host, port),
        None => false,
    }
}

/// Whether the X server's Unix socket at `path` accepts a connection.
#[cfg(unix)]
pub(crate) fn local_reachable(path: &std::path::Path) -> bool {
    use std::os::unix::net::UnixStream;
    // XWayland and Xorg also listen on the abstract socket of the same name, which works where
    // /tmp is private (a sandbox) and the file isn't visible.
    #[cfg(target_os = "linux")]
    {
        use std::os::linux::net::SocketAddrExt;
        if let Ok(addr) = std::os::unix::net::SocketAddr::from_abstract_name(path.as_os_str().as_encoded_bytes())
            && UnixStream::connect_addr(&addr).is_ok()
        {
            return true;
        }
    }
    UnixStream::connect(path).is_ok()
}

#[cfg(not(unix))]
pub(crate) fn local_reachable(_: &std::path::Path) -> bool {
    false
}

/// Whether `host:port` accepts a TCP connection within a short time per address.
pub(crate) fn tcp_reachable(host: &str, port: u16) -> bool {
    use std::net::ToSocketAddrs;
    const TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);
    let Ok(addrs) = (host, port).to_socket_addrs() else { return false };
    addrs.take(4).any(|addr| std::net::TcpStream::connect_timeout(&addr, TIMEOUT).is_ok())
}

#[cfg(test)]
mod tests {
    use super::{Backend, Endpoint, choose, endpoint};
    use std::collections::HashMap;

    /// `choose` with exactly `vars` set, nothing from the host, and a probe that answers
    /// `reachable` and records the `DISPLAY` it was asked about.
    fn pick(vars: &[(&str, &str)], reachable: bool) -> (Backend, Option<String>) {
        let env: HashMap<&str, &str> = vars.iter().copied().collect();
        let mut asked = None;
        let backend = choose(
            |name| env.get(name).map(|v| (*v).to_owned()),
            |display| {
                asked = Some(display.to_owned());
                reachable
            },
        );
        (backend, asked)
    }

    const GNOME_WAYLAND: [(&str, &str); 3] = [("XDG_SESSION_TYPE", "wayland"), ("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")];

    #[test]
    fn a_wayland_desktop_with_a_live_xwayland_runs_on_x11_for_file_drops() {
        assert_eq!(pick(&GNOME_WAYLAND, true), (Backend::X11, Some(":0".to_owned())));
        // WAYLAND_SOCKET (a socket handed over by the compositor) counts like WAYLAND_DISPLAY.
        assert_eq!(pick(&[("WAYLAND_SOCKET", "5"), ("DISPLAY", ":1")], true).0, Backend::X11);
        // No XDG_SESSION_TYPE at all (started outside a login session) still counts.
        assert_eq!(pick(&[("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")], true).0, Backend::X11);
    }

    #[test]
    fn a_stale_display_falls_back_to_native_wayland() {
        // Before the probe, a DISPLAY nobody answers forced X11 and the app never opened a window.
        assert_eq!(pick(&GNOME_WAYLAND, false), (Backend::Auto, Some(":0".to_owned())));
    }

    #[test]
    fn no_or_empty_display_keeps_wayland_without_probing() {
        assert_eq!(pick(&[("WAYLAND_DISPLAY", "wayland-0")], true), (Backend::Auto, None));
        assert_eq!(pick(&[("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", "")], true), (Backend::Auto, None));
        assert_eq!(pick(&[("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", "  ")], true), (Backend::Auto, None));
    }

    #[test]
    fn an_x11_session_keeps_winits_choice_without_probing() {
        // winit picks X11 by itself here; nothing to force.
        assert_eq!(pick(&[("XDG_SESSION_TYPE", "x11"), ("DISPLAY", ":0")], true), (Backend::Auto, None));
        // Empty WAYLAND_DISPLAY counts as unset.
        assert_eq!(pick(&[("WAYLAND_DISPLAY", ""), ("DISPLAY", ":0")], true), (Backend::Auto, None));
        // A nested Wayland compositor inside an X11 session.
        assert_eq!(pick(&[("XDG_SESSION_TYPE", "X11"), ("WAYLAND_DISPLAY", "wayland-1"), ("DISPLAY", ":0")], true), (Backend::Auto, None));
        assert_eq!(pick(&[("XDG_SESSION_TYPE", "tty"), ("WAYLAND_DISPLAY", "wayland-1"), ("DISPLAY", ":0")], true), (Backend::Auto, None));
        // A session that says Wayland but has no Wayland display to reach.
        assert_eq!(pick(&[("XDG_SESSION_TYPE", "wayland"), ("DISPLAY", ":0")], true), (Backend::Auto, None));
        assert_eq!(pick(&[], true), (Backend::Auto, None));
    }

    #[test]
    fn pdfcraft_wayland_keeps_native_wayland() {
        let mut vars = GNOME_WAYLAND.to_vec();
        vars.push(("PDFCRAFT_WAYLAND", "1"));
        assert_eq!(pick(&vars, true), (Backend::Auto, None));
        // It wins over WINIT_UNIX_BACKEND too: it's PdfCraft's own switch.
        vars.push(("WINIT_UNIX_BACKEND", "x11"));
        assert_eq!(pick(&vars, true), (Backend::Auto, None));
        // Empty is unset.
        let mut empty = GNOME_WAYLAND.to_vec();
        empty.push(("PDFCRAFT_WAYLAND", ""));
        assert_eq!(pick(&empty, true).0, Backend::X11);
    }

    #[test]
    fn winit_unix_backend_is_honoured() {
        let with = |value| {
            let mut vars = GNOME_WAYLAND.to_vec();
            vars.push(("WINIT_UNIX_BACKEND", value));
            vars
        };
        assert_eq!(pick(&with("wayland"), true), (Backend::Wayland, None));
        assert_eq!(pick(&with(" X11 "), false), (Backend::X11, None), "an explicit choice isn't second-guessed");
        assert_eq!(pick(&[("WINIT_UNIX_BACKEND", "x11"), ("DISPLAY", ":0")], false).0, Backend::X11);
        // Anything else is ignored, and the automatic choice applies.
        assert_eq!(pick(&with("vulkan"), true).0, Backend::X11);
        assert_eq!(pick(&with("vulkan"), false).0, Backend::Auto);
    }

    #[test]
    fn display_values_name_their_endpoint() {
        assert_eq!(endpoint(":0"), Some(Endpoint::Local(0)));
        assert_eq!(endpoint(":1.0"), Some(Endpoint::Local(1)));
        assert_eq!(endpoint("unix:2"), Some(Endpoint::Local(2)));
        assert_eq!(endpoint("unix/:3"), Some(Endpoint::Local(3)));
        assert_eq!(endpoint("localhost:10.0"), Some(Endpoint::Tcp("localhost".to_owned(), 6010)));
        assert_eq!(endpoint("tcp/example.org:1"), Some(Endpoint::Tcp("example.org".to_owned(), 6001)));
        assert_eq!(endpoint(":65535"), Some(Endpoint::Local(65535)));
        assert_eq!(endpoint("[::1]:0"), Some(Endpoint::Tcp("::1".to_owned(), 6000)));
        assert_eq!(endpoint("::1:0"), Some(Endpoint::Tcp("::1".to_owned(), 6000)));
        for unprobed in
            ["", ":", "0", ":x", ":0x", "host:", "host:65535", ":65536", "/private/tmp/launchd/org.xquartz:0", "unix/host:0", "decnet/node:0"]
        {
            assert_eq!(endpoint(unprobed), None, "{unprobed:?}");
        }
    }

    #[test]
    fn unprobed_display_values_count_as_unreachable() {
        assert!(!super::x11_reachable(""));
        assert!(!super::x11_reachable("not a display"));
    }

    #[test]
    fn tcp_probe_tells_a_listener_from_a_closed_port() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind a loopback port");
        let port = listener.local_addr().expect("bound address").port();
        assert!(super::tcp_reachable("127.0.0.1", port));
        drop(listener);
        assert!(!super::tcp_reachable("127.0.0.1", port));
    }

    #[cfg(unix)]
    #[test]
    fn local_probe_tells_a_listening_socket_from_a_stale_one() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("pdfcraft-x11-probe-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).expect("create the test folder");
        let path = dir.join("X99");
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("bind a Unix socket");
        assert!(super::local_reachable(&path));
        // The file stays behind when the server is gone, as /tmp/.X11-unix/X0 does after a crash.
        drop(listener);
        assert!(path.exists());
        assert!(!super::local_reachable(&path));
        assert!(!super::local_reachable(&dir.join("X98")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
