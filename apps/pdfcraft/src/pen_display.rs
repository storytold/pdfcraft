//! Linux: on a Wayland session with a pen attached, open the window through Xwayland (#3650).
//!
//! winit 0.30 binds no Wayland tablet protocol (`zwp_tablet_v2`), so the compositor decides what
//! a pen does in the window: on KDE Plasma a hovering pen moves the cursor but its tip never
//! clicks, and on Hyprland the pen does nothing at all. Xwayland binds the protocol itself and
//! hands X clients the pen as a pointer, so through Xwayland the pen moves, clicks and draws. It
//! has no pressure there either, since winit's X11 backend doesn't read it. PhotoCraft makes the
//! same choice.
//!
//! The window goes through Xwayland only when all of these hold; otherwise it stays native:
//! - winit would use Wayland (`WAYLAND_DISPLAY` or `WAYLAND_SOCKET` is set);
//! - `PDFCRAFT_NATIVE_WAYLAND` is unset, empty or `0`;
//! - a pen is attached ([`has_pen`], from `/proc/bus/input/devices`);
//! - `DISPLAY` names a local X display whose socket exists ([`x11_socket`]);
//! - the libraries winit loads for X11 are installed ([`X11_LIBRARIES`]);
//! - the X server there accepts a connection ([`x11_server_accepts`]).
//!
//! The last three are checked because winit allows one event loop per process: an X11 start
//! that failed couldn't fall back to Wayland, and the app wouldn't open at all. When they can't
//! be confirmed, the window stays on Wayland, as it was before this check. Only start-up
//! decides: a pen plugged in later takes effect at the next launch.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Set to anything but empty or `0`, keeps native Wayland with a pen attached.
pub const NATIVE_WAYLAND_VAR: &str = "PDFCRAFT_NATIVE_WAYLAND";

/// The kernel's list of input devices and their capability bitmaps.
const INPUT_DEVICES: &str = "/proc/bus/input/devices";

/// Where a local X server listens: display `:N` is `/tmp/.X11-unix/XN`.
const X11_SOCKET_DIR: &str = "/tmp/.X11-unix";

/// How long the X server gets to answer the connection probe. A running server answers in
/// milliseconds; past this the window stays on Wayland rather than wait for one that may never.
const X11_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The shared libraries winit 0.30 opens at run time for X11. x11-dl opens Xlib, Xcursor,
/// Xlib-xcb and XInput2 (`XConnection::new`), x11rb's `dl-libxcb` opens libxcb, and
/// xkbcommon-dl opens xkbcommon and xkbcommon-x11, panicking if one is missing.
pub const X11_LIBRARIES: &[&str] =
    &["libX11.so.6", "libXcursor.so.1", "libX11-xcb.so.1", "libXi.so.6", "libxcb.so.1", "libxkbcommon.so.0", "libxkbcommon-x11.so.0"];

/// What start-up should do with the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Open on the session's own display server.
    Native,
    /// Open through Xwayland: a pen is attached and Xwayland can run the window.
    Xwayland,
    /// A pen is attached, but Xwayland couldn't be confirmed usable, so the window stays native.
    XwaylandUnavailable,
}

/// Decides for this launch from the environment and the system, and logs a switch.
pub fn choose_for_launch() -> Choice {
    let var = |name: &str| std::env::var(name).ok();
    let pen = || std::fs::read_to_string(INPUT_DEVICES).is_ok_and(|devices| has_pen(&devices));
    let xwayland = || {
        let display = var("DISPLAY");
        xwayland_usable(
            display.as_deref(),
            |socket| socket.exists(),
            || x11_libraries_installed(var("LD_LIBRARY_PATH").as_deref()),
            |display| x11_server_accepts(display, X11_PROBE_TIMEOUT),
        )
    };
    let choice = choose(var, pen, xwayland);
    match choice {
        Choice::Native => {}
        Choice::Xwayland => {
            log::info!("a pen is attached: opening the window through Xwayland, where the pen works ({NATIVE_WAYLAND_VAR}=1 keeps Wayland)")
        }
        Choice::XwaylandUnavailable => {
            log::warn!(
                "a pen is attached but Xwayland isn't usable (no local DISPLAY socket, X11 libraries or answering X server): staying on Wayland, where the pen may not click"
            )
        }
    }
    choice
}

/// [`choose_for_launch`]'s decision. The system probes run only when the earlier conditions
/// hold, so a session without Wayland or without a pen never touches the X socket or libraries.
pub fn choose(var: impl Fn(&str) -> Option<String>, pen: impl FnOnce() -> bool, xwayland_usable: impl FnOnce() -> bool) -> Choice {
    let non_empty = |name: &str| var(name).is_some_and(|v| !v.trim().is_empty());
    let wayland = non_empty("WAYLAND_DISPLAY") || non_empty("WAYLAND_SOCKET");
    let keep_native = var(NATIVE_WAYLAND_VAR).is_some_and(|v| {
        let v = v.trim();
        !v.is_empty() && v != "0"
    });
    if !wayland || keep_native || !pen() {
        Choice::Native
    } else if xwayland_usable() {
        Choice::Xwayland
    } else {
        Choice::XwaylandUnavailable
    }
}

/// `BTN_TOOL_PEN` (linux/input-event-codes.h): the key a pen reports while it is in range.
const BTN_TOOL_PEN: u32 = 0x140;
/// `ABS_X` and `ABS_Y`: the absolute position axes every pen reports.
const ABS_X: u32 = 0x00;
const ABS_Y: u32 = 0x01;

/// Whether `/proc/bus/input/devices` lists a pen: a device with the pen tool key and absolute
/// X and Y axes, which is what libinput treats as a tablet tool. The key alone isn't enough: a
/// virtual keyboard made through uinput (RustDesk's, for one) can claim every key code,
/// `BTN_TOOL_PEN` included, without having any axes.
pub fn has_pen(devices: &str) -> bool {
    devices.split("\n\n").any(|device| {
        let bitmap = |prefix: &str| device.lines().find_map(|line| line.trim().strip_prefix(prefix));
        bitmap("B: KEY=").is_some_and(|keys| bit_set(keys, BTN_TOOL_PEN))
            && bitmap("B: ABS=").is_some_and(|axes| bit_set(axes, ABS_X) && bit_set(axes, ABS_Y))
    })
}

/// Bit `bit` of a `/proc/bus/input/devices` bitmap: hexadecimal words the size of the kernel's
/// `long` (a `usize` on Linux), most significant first. A missing or malformed word has no bits.
fn bit_set(bitmap: &str, bit: u32) -> bool {
    let word_bits = usize::BITS;
    let Ok(word_index) = usize::try_from(bit / word_bits) else { return false };
    bitmap
        .split_whitespace()
        .rev()
        .nth(word_index)
        .and_then(|word| u64::from_str_radix(word, 16).ok())
        .is_some_and(|word| word.checked_shr(bit % word_bits).is_some_and(|w| w & 1 == 1))
}

/// The socket of a local X display: `:1` and `:1.0` are `/tmp/.X11-unix/X1`. Anything else (a
/// host name, a path, a malformed value) is `None`: whether it can be reached isn't known, and
/// guessing wrong would leave the app without a window.
pub fn x11_socket(display: &str) -> Option<PathBuf> {
    let display = display.trim();
    let number = display.strip_prefix(':')?;
    let number = number.split_once('.').map_or(number, |(number, _screen)| number);
    // Display numbers are small; the length cap only bounds a hostile value.
    let digits = !number.is_empty() && number.len() <= 9 && number.bytes().all(|b| b.is_ascii_digit());
    digits.then(|| Path::new(X11_SOCKET_DIR).join(format!("X{number}")))
}

/// Whether Xwayland can run the window: `display` is a local X display whose socket exists, the
/// X11 libraries are installed and the server accepts a connection. Cheapest first: each probe
/// runs only when the ones before it pass.
fn xwayland_usable(
    display: Option<&str>,
    socket_exists: impl FnOnce(&Path) -> bool,
    libraries_installed: impl FnOnce() -> bool,
    server_accepts: impl FnOnce(&str) -> bool,
) -> bool {
    let Some(display) = display else { return false };
    x11_socket(display).is_some_and(|socket| socket_exists(&socket)) && libraries_installed() && server_accepts(display.trim())
}

/// Whether the X server at `display` completes the connection handshake, authentication
/// included, within `timeout`: what winit's X11 start (`XOpenDisplay`) does next. A socket left
/// behind by a server that crashed, or a server that refuses this client, fails here, and the
/// window stays on Wayland instead of the forced X11 start failing with no window.
pub fn x11_server_accepts(display: &str, timeout: Duration) -> bool {
    let target = display.to_owned();
    match handshake_within(move || x11rb::connect(Some(&target)).map(drop).map_err(|e| e.to_string()), timeout) {
        Ok(()) => true,
        Err(why) => {
            log::info!("the X server at {display} isn't usable: {why}");
            false
        }
    }
}

/// Runs `handshake` on its own thread and waits for it up to `timeout`, so a server that never
/// answers can't hold up start-up. After a timeout the thread is left to finish or wait on its own.
fn handshake_within(handshake: impl FnOnce() -> Result<(), String> + Send + 'static, timeout: Duration) -> Result<(), String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("x11-probe".into())
        .spawn(move || {
            // After a timeout nobody is waiting for the answer, and that's fine.
            let _ = sender.send(handshake());
        })
        .map_err(|e| format!("no thread for the probe: {e}"))?;
    receiver.recv_timeout(timeout).map_err(|_| format!("no answer within {timeout:?}"))?
}

/// Whether every library in [`X11_LIBRARIES`] is installed: in a directory of `LD_LIBRARY_PATH`
/// or a usual library directory, or else listed in the dynamic linker's cache (`ldconfig -p`).
fn x11_libraries_installed(ld_library_path: Option<&str>) -> bool {
    let dirs = library_dirs(ld_library_path);
    let missing: Vec<&str> = X11_LIBRARIES.iter().copied().filter(|soname| !dirs.iter().any(|dir| dir.join(soname).exists())).collect();
    if missing.is_empty() {
        return true;
    }
    // Directories set up in /etc/ld.so.conf.d are only known to the cache.
    ldconfig_listing().is_some_and(|cache| missing.iter().all(|soname| ldconfig_lists(&cache, soname)))
}

/// Where to look for a library: `LD_LIBRARY_PATH` first, then the usual directories, with this
/// architecture's Debian-style multiarch directory (`/usr/lib/x86_64-linux-gnu`).
fn library_dirs(ld_library_path: Option<&str>) -> Vec<PathBuf> {
    // A real LD_LIBRARY_PATH has a few entries; the cap only bounds a hostile value.
    let mut dirs: Vec<PathBuf> = ld_library_path.unwrap_or("").split(':').filter(|d| !d.is_empty()).take(64).map(PathBuf::from).collect();
    let multiarch = format!("{}-linux-gnu", std::env::consts::ARCH);
    for dir in [format!("/usr/lib/{multiarch}"), format!("/lib/{multiarch}")] {
        dirs.push(PathBuf::from(dir));
    }
    for dir in ["/usr/lib64", "/lib64", "/usr/lib", "/lib", "/usr/local/lib"] {
        dirs.push(PathBuf::from(dir));
    }
    dirs
}

/// The output of `ldconfig -p`, when one of the usual `ldconfig`s runs and succeeds.
fn ldconfig_listing() -> Option<String> {
    ["ldconfig", "/sbin/ldconfig", "/usr/sbin/ldconfig"].iter().find_map(|program| {
        let output = std::process::Command::new(program).arg("-p").env("LC_ALL", "C").output().ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    })
}

/// Whether an `ldconfig -p` listing has `soname` for this architecture. Lines look like
/// `\tlibX11.so.6 (libc6,x86-64) => /usr/lib/libX11.so.6`; where the architecture has a tag
/// (x86-64, AArch64), a 32-bit library of the same name doesn't count.
fn ldconfig_lists(listing: &str, soname: &str) -> bool {
    let tag = match std::env::consts::ARCH {
        "x86_64" => Some("x86-64"),
        "aarch64" => Some("AArch64"),
        _ => None,
    };
    listing.lines().any(|line| {
        let Some((name, rest)) = line.trim_start().split_once(' ') else { return false };
        if name != soname {
            return false;
        }
        let flags = rest.trim_start().strip_prefix('(').and_then(|r| r.split_once(')')).map_or("", |(flags, _)| flags);
        tag.is_none_or(|tag| flags.split(',').any(|flag| flag.trim() == tag))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic `/proc/bus/input/devices` with a pen tablet's pen and pad, a mouse, and a
    /// uinput keyboard that claims every key (pen tool included) but has no axes.
    const DEVICES: &str = "\
I: Bus=0003 Vendor=1234 Product=5678 Version=0110
N: Name=\"Example Tablet Pen\"
H: Handlers=event10 mouse1
B: EV=1b
B: KEY=1c01 0 0 0 0 0
B: ABS=d000003

I: Bus=0003 Vendor=1234 Product=5678 Version=0110
N: Name=\"Example Tablet Pad\"
H: Handlers=event11 js0
B: EV=1b
B: KEY=800 201ff 0 0 0 0
B: ABS=3

I: Bus=0003 Vendor=0001 Product=0002 Version=0111
N: Name=\"Example Mouse\"
B: EV=17
B: KEY=1f0000 0 0 0 0
B: REL=903
";

    const UINPUT_KEYBOARD: &str = "\
I: Bus=0006 Vendor=0000 Product=0000 Version=0000
N: Name=\"Remote desktop virtual keyboard\"
B: EV=3
B: KEY=ffffffffffffffff ffffffffffffffff ffffffffffffffff ffffffffffffffff ffffffffffffffff ffffffffffffffff
";

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| (*v).to_owned())
    }

    #[test]
    fn a_pen_tablet_is_a_pen() {
        assert!(has_pen(DEVICES));
    }

    #[test]
    fn devices_without_a_pen_are_not_one() {
        let without_pen = DEVICES.split("\n\n").skip(1).collect::<Vec<_>>().join("\n\n");
        assert!(!has_pen(&without_pen), "a tablet's pad and a mouse aren't pens");
        assert!(!has_pen(UINPUT_KEYBOARD), "every key but no axes isn't a pen");
        assert!(!has_pen(""));
    }

    #[test]
    fn bitmaps_are_read_from_the_least_significant_word() {
        assert!(bit_set("1 0", usize::BITS));
        assert!(!bit_set("1 0", 0));
        assert!(bit_set("3", 1));
        assert!(!bit_set("3", 2));
        assert!(!bit_set("", 0));
        assert!(!bit_set("zz", 0), "a malformed word has no bits");
        assert!(!bit_set("ffffffffffffffffff", 0), "a word wider than 64 bits is malformed");
        assert!(!bit_set("1", u32::MAX));
    }

    #[test]
    fn a_wayland_session_with_a_pen_opens_through_xwayland() {
        let wayland = [("WAYLAND_DISPLAY", "wayland-1"), ("DISPLAY", ":1")];
        assert_eq!(choose(env(&wayland), || true, || true), Choice::Xwayland);
        assert_eq!(choose(env(&[("WAYLAND_SOCKET", "3")]), || true, || true), Choice::Xwayland);
    }

    #[test]
    fn without_a_pen_or_wayland_the_window_stays_native() {
        let wayland = [("WAYLAND_DISPLAY", "wayland-1")];
        assert_eq!(choose(env(&wayland), || false, || true), Choice::Native);
        assert_eq!(choose(env(&[("DISPLAY", ":0")]), || true, || true), Choice::Native, "already X11");
        assert_eq!(choose(env(&[("WAYLAND_DISPLAY", " ")]), || true, || true), Choice::Native);
    }

    #[test]
    fn the_environment_keeps_native_wayland() {
        for value in ["1", "yes", " true "] {
            let pairs = [("WAYLAND_DISPLAY", "wayland-1"), (NATIVE_WAYLAND_VAR, value)];
            assert_eq!(choose(env(&pairs), || true, || true), Choice::Native, "{value:?}");
        }
        for value in ["", "0", " 0 "] {
            let pairs = [("WAYLAND_DISPLAY", "wayland-1"), (NATIVE_WAYLAND_VAR, value)];
            assert_eq!(choose(env(&pairs), || true, || true), Choice::Xwayland, "{value:?}");
        }
    }

    #[test]
    fn an_unusable_xwayland_keeps_wayland() {
        let wayland = [("WAYLAND_DISPLAY", "wayland-1")];
        assert_eq!(choose(env(&wayland), || true, || false), Choice::XwaylandUnavailable);
    }

    #[test]
    fn the_system_is_probed_only_when_needed() {
        let probed = |result: bool| move || -> bool { panic!("probed, would return {result}") };
        assert_eq!(choose(env(&[("DISPLAY", ":0")]), probed(true), probed(true)), Choice::Native);
        let wayland = [("WAYLAND_DISPLAY", "wayland-1")];
        assert_eq!(choose(env(&wayland), || false, probed(true)), Choice::Native);
    }

    #[test]
    fn xwayland_needs_a_socket_libraries_and_an_answering_server() {
        let yes = |_: &Path| true;
        assert!(xwayland_usable(Some(":1"), yes, || true, |_| true));
        assert!(!xwayland_usable(Some(":1"), yes, || true, |_| false), "a stale socket or a refusing server");
        assert!(!xwayland_usable(Some(":1"), yes, || false, |_| panic!("no libraries, no connection")));
        assert!(!xwayland_usable(Some(":1"), |_| false, || panic!("no socket"), |_| panic!("no socket")));
        assert!(!xwayland_usable(Some("host:0"), yes, || true, |_| true), "only local displays");
        assert!(!xwayland_usable(None, yes, || true, |_| true));
    }

    /// A fresh directory for one test's fake X server socket.
    fn socket_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pdfcraft-x11-probe-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temporary directory");
        dir
    }

    /// The handshake `x11_server_accepts` makes, over a socket of the test's own. (x11rb's
    /// display parsing reads a path, but then connects to `/tmp/.X11-unix/X0` regardless.)
    fn handshake_at(socket: &Path) -> impl FnOnce() -> Result<(), String> + Send + 'static {
        use x11rb::rust_connection::{DefaultStream, RustConnection};
        let socket = socket.to_path_buf();
        move || {
            let stream = std::os::unix::net::UnixStream::connect(&socket).map_err(|e| e.to_string())?;
            let (stream, _peer) = DefaultStream::from_unix_stream(stream).map_err(|e| e.to_string())?;
            RustConnection::connect_to_stream(stream, 0).map(drop).map_err(|e| e.to_string())
        }
    }

    /// A minimal successful connection setup: no vendor or pixmap formats and one screen, in the
    /// client's byte order (x11rb sends its native one).
    fn setup_success() -> Vec<u8> {
        let mut body = Vec::new();
        for value in [0u32, 0x0020_0000, 0x001f_ffff, 0] {
            // release number, resource id base, resource id mask, motion buffer size
            body.extend(value.to_ne_bytes());
        }
        body.extend(0u16.to_ne_bytes()); // vendor length
        body.extend(0xffffu16.to_ne_bytes()); // maximum request length
        body.extend([1, 0, 0, 0, 32, 32, 8, 255]); // screens, formats, byte and bit order, scanline unit and pad, keycodes
        body.extend([0; 4]);
        for value in [0x100u32, 0x20, 0x00ff_ffff, 0, 0] {
            // root, colormap, white, black, input masks
            body.extend(value.to_ne_bytes());
        }
        for value in [1024u16, 768, 270, 203, 1, 1] {
            // size in pixels and millimetres, installed colormaps
            body.extend(value.to_ne_bytes());
        }
        body.extend(0x21u32.to_ne_bytes()); // root visual
        body.extend([0, 0, 24, 0]); // backing stores, save unders, depth, allowed depths
        let mut reply = vec![1, 0];
        reply.extend(11u16.to_ne_bytes());
        reply.extend(0u16.to_ne_bytes());
        reply.extend(u16::try_from(body.len() / 4).expect("short setup").to_ne_bytes());
        reply.extend(body);
        reply
    }

    #[test]
    fn a_server_that_completes_the_handshake_accepts() {
        use std::io::{Read, Write};
        let dir = socket_dir("answers");
        let socket = dir.join("X0");
        let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream.write_all(&setup_success()).expect("reply");
            // Hold the connection until the client closes it.
            let _ = stream.read_to_end(&mut Vec::new());
        });
        assert_eq!(handshake_within(handshake_at(&socket), Duration::from_secs(10)), Ok(()));
        server.join().expect("server thread");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_stale_socket_or_a_refusing_server_does_not_accept() {
        let dir = socket_dir("refuses");
        // A socket file nobody listens on any more, as a crashed server leaves it.
        let stale = dir.join("X0");
        drop(std::os::unix::net::UnixListener::bind(&stale).expect("bind"));
        assert!(stale.exists());
        assert!(handshake_within(handshake_at(&stale), Duration::from_secs(10)).is_err());
        // A server that hangs up without completing the handshake.
        let refusing = dir.join("X1");
        let listener = std::os::unix::net::UnixListener::bind(&refusing).expect("bind");
        let server = std::thread::spawn(move || drop(listener.accept()));
        assert!(handshake_within(handshake_at(&refusing), Duration::from_secs(10)).is_err());
        server.join().expect("server thread");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_server_that_never_answers_times_out() {
        let dir = socket_dir("silent");
        let socket = dir.join("X0");
        let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
        let (done, wait) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let connection = listener.accept();
            // Keep the connection open, unanswered, until the test is done.
            let _ = wait.recv();
            drop(connection);
        });
        let started = std::time::Instant::now();
        assert!(handshake_within(handshake_at(&socket), Duration::from_millis(200)).is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(done);
        server.join().expect("server thread");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_local_displays_have_a_known_socket() {
        assert_eq!(x11_socket(":1"), Some(PathBuf::from("/tmp/.X11-unix/X1")));
        assert_eq!(x11_socket(" :12.0 "), Some(PathBuf::from("/tmp/.X11-unix/X12")));
        for display in ["", ":", ":.0", ":x", ":1x", "host:0", "unix:0", "/tmp/launch/org.xquartz:0", ":1234567890", ":../1"] {
            assert_eq!(x11_socket(display), None, "{display:?}");
        }
    }

    #[test]
    fn library_dirs_start_with_ld_library_path() {
        let dirs = library_dirs(Some("/opt/a::/opt/b"));
        assert_eq!(dirs.first(), Some(&PathBuf::from("/opt/a")));
        assert_eq!(dirs.get(1), Some(&PathBuf::from("/opt/b")));
        assert!(dirs.contains(&PathBuf::from("/usr/lib")));
        let hostile = vec!["/x"; 10_000].join(":");
        assert!(library_dirs(Some(&hostile)).len() < 100);
    }

    #[test]
    fn the_linker_cache_lists_libraries_for_this_architecture() {
        let tag = match std::env::consts::ARCH {
            "x86_64" => "x86-64",
            "aarch64" => "AArch64",
            _ => return,
        };
        let listing = format!(
            "3 libs found in cache `/etc/ld.so.cache'\n\
             \tlibXi.so.6 (libc6,{tag}) => /usr/lib/libXi.so.6\n\
             \tlibXcursor.so.1 (libc6) => /usr/lib32/libXcursor.so.1\n\
             \tlibX11.so.6.extra (libc6,{tag}) => /usr/lib/libX11.so.6.extra\n"
        );
        assert!(ldconfig_lists(&listing, "libXi.so.6"));
        assert!(!ldconfig_lists(&listing, "libXcursor.so.1"), "only a 32-bit copy");
        assert!(!ldconfig_lists(&listing, "libX11.so.6"), "names match whole");
        assert!(!ldconfig_lists("", "libXi.so.6"));
    }
}
