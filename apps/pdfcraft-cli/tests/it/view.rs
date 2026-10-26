//! `view` draws pages through the kitty graphics protocol. Test output is a pipe, not a terminal,
//! so these tests pass `--protocol kitty` and read the escape codes back from stdout.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use base64::Engine as _;

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

fn tmp(test: &str, name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pdfcraft-cli-view-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

/// A US Letter document (612 × 792 pt) with `pages` pages.
fn fixture(test: &str, pages: usize) -> PathBuf {
    let one = tmp(test, "one.pdf");
    let bytes = pdfcraft_engine::Session::new().create_from_text("View test", "Hello from PdfCraft").unwrap();
    std::fs::write(&one, bytes.as_slice()).unwrap();
    if pages == 1 {
        return one;
    }
    let many = tmp(test, "many.pdf");
    let mut args = vec!["combine".to_string()];
    args.extend(std::iter::repeat_n(one.to_str().unwrap().to_string(), pages));
    args.extend(["--out".to_string(), many.to_str().unwrap().to_string()]);
    assert!(Command::new(BIN).args(&args).status().unwrap().success());
    many
}

/// Run `view`, kept away from the terminal, the tmux and the SSH login the tests may run in.
fn view(pdf: &Path, args: &[&str]) -> Output {
    view_with(pdf, args, &[])
}

/// [`view`], with `env` set.
fn view_with(pdf: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(BIN);
    command.arg("view").arg(pdf).args(args).env("TERM", "dumb");
    for name in ["TMUX", "TMUX_PANE", "KITTY_WINDOW_ID", "TERM_PROGRAM", "SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"] {
        command.env_remove(name);
    }
    command.envs(env.iter().copied()).output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// One image drawn by `view`: its first control keys and its decoded payload.
struct Drawn {
    keys: String,
    png: Vec<u8>,
}

/// Split stdout into images: escape codes `ESC _G <control> ; <base64> ESC \`, with `m=1` on
/// every chunk but an image's last. Anything between images must be a newline, a carriage return
/// or a cursor move (`ESC [ n A`, `B` or `C`).
fn images(stdout: &[u8]) -> Vec<Drawn> {
    let mut rest = std::str::from_utf8(stdout).unwrap();
    let (mut out, mut keys, mut payload) = (Vec::new(), None::<String>, String::new());
    while !rest.is_empty() {
        if keys.is_none() {
            if let Some(after) = rest.strip_prefix('\n').or_else(|| rest.strip_prefix('\r')) {
                rest = after;
                continue;
            }
            if let Some(after) = rest.strip_prefix("\x1b[") {
                let digits = after.find(|c: char| !c.is_ascii_digit()).unwrap();
                if digits > 0 && after[digits..].starts_with(['A', 'B', 'C']) {
                    rest = &after[digits + 1..];
                    continue;
                }
            }
        }
        let body = rest.strip_prefix("\x1b_G").unwrap_or_else(|| panic!("expected an escape code at {:?}", &rest[..rest.len().min(20)]));
        let end = body.find("\x1b\\").unwrap();
        let (control, data) = body[..end].split_once(';').unwrap();
        keys.get_or_insert_with(|| control.to_string());
        payload.push_str(data);
        if control.ends_with("m=0") {
            let png = base64::engine::general_purpose::STANDARD.decode(&payload).unwrap();
            out.push(Drawn { keys: keys.take().unwrap(), png });
            payload.clear();
        } else {
            assert!(control.ends_with("m=1"), "{control}");
        }
        rest = &body[end + 2..];
    }
    assert!(keys.is_none(), "an image was left unfinished");
    out
}

fn png_size(png: &[u8]) -> (u32, u32) {
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(&png[12..16], b"IHDR");
    let be32 = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    (be32(&png[16..20]), be32(&png[20..24]))
}

/// The value of `key` in a control string such as `a=T,c=40,r=20`.
fn key(control: &str, key: &str) -> u32 {
    control.split(',').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('=')).unwrap().parse().unwrap()
}

#[test]
fn each_page_is_a_png_shown_at_the_cursor() {
    let pdf = fixture("pages", 2);
    let out = view(&pdf, &["--pages", "2,1", "--protocol", "kitty"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let drawn = images(&out.stdout);
    assert_eq!(drawn.len(), 2);
    for image in &drawn {
        assert!(image.keys.starts_with("a=T,f=100,t=d,q=2,"), "{}", image.keys);
        let (w, h) = png_size(&image.png);
        // The page's shape, whatever the size of the terminal running the tests.
        let ratio = w as f32 / h as f32;
        assert!((ratio - 612.0 / 792.0).abs() < 0.01, "{w}×{h}");
        assert!(key(&image.keys, "c") >= 1 && key(&image.keys, "r") >= 1);
    }
}

/// `view` takes the owner password as `render` does, also of revision 2–4 files, whose owner
/// password the renderer doesn't take: what reaches it is the user password owner authentication
/// recovers (#2670). A wrong password still shows nothing.
#[test]
fn either_password_opens_a_protected_file_of_any_revision() {
    for algorithm in crate::owner_password::ALGORITHMS {
        let pdf = tmp("passwords", &format!("{algorithm:?}.pdf"));
        std::fs::write(&pdf, crate::owner_password::protected(algorithm)).unwrap();
        for password in ["user", "owner"] {
            let out = view(&pdf, &["--password", password, "--protocol", "kitty"]);
            assert!(out.status.success(), "{algorithm:?} with {password:?}: {}", text(&out.stderr));
            let drawn = images(&out.stdout);
            assert_eq!(drawn.len(), 1, "{algorithm:?} with {password:?}");
            let (w, h) = png_size(&drawn[0].png);
            assert!((w as f32 / h as f32 - 200.0 / 300.0).abs() < 0.01, "{algorithm:?} with {password:?}: {w}×{h}");
        }
        let out = view(&pdf, &["--password", "wrong", "--protocol", "kitty"]);
        assert!(!out.status.success() && out.stdout.is_empty(), "{algorithm:?} with a wrong password");
    }
}

/// A dynamic XFA form shows the pages its template lays out into, as `render`, `text` and the
/// app do (#1645), not the placeholder page the file carries; the file itself is left alone.
#[test]
fn a_dynamic_xfa_form_shows_its_laid_out_pages() {
    let pdf = crate::xfa::fixture("view");
    let original = std::fs::read(&pdf).unwrap();
    let out = view(&pdf, &["--protocol", "kitty"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let drawn = images(&out.stdout);
    assert_eq!(drawn.len(), 2, "the form lays out into two pages; the file has one placeholder");
    for image in &drawn {
        let (w, h) = png_size(&image.png);
        // 200 × 300 pt, the form's medium: the placeholder is US Letter.
        assert!((w as f32 / h as f32 - 200.0 / 300.0).abs() < 0.01, "{w}×{h}");
    }
    assert_eq!(std::fs::read(&pdf).unwrap(), original, "viewing must not write the layout to the file");
}

/// A colour the renderer can't compute is painted in a fallback colour (#4260): `view` says so,
/// in the words `render` uses.
#[test]
fn a_colour_painted_in_a_fallback_colour_is_said() {
    // A Separation colour whose tint transform is of no known function type.
    let content = "/CS1 cs 1 scn 0 0 200 200 re f";
    let bytes = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] \
         /Resources << /ColorSpace << /CS1 [/Separation /Spot /DeviceRGB << /FunctionType 7 /Domain [0 1] >>] >> >> /Contents 4 0 R >> endobj\n\
         4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n\
         trailer << /Root 1 0 R >>\n%%EOF",
        content.len()
    );
    let pdf = tmp("fallback", "fallback.pdf");
    std::fs::write(&pdf, bytes).unwrap();
    let said = "warning: page 1: a colour that could not be computed was painted in a fallback colour";
    let png = tmp("fallback", "page.png");
    let render = Command::new(BIN).arg("render").arg(&pdf).arg("--out").arg(&png).output().unwrap();
    assert!(render.status.success() && text(&render.stderr).contains(said), "render: {}", text(&render.stderr));
    let out = view(&pdf, &["--protocol", "kitty"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(images(&out.stdout).len(), 1, "the page is still drawn");
    assert!(text(&out.stderr).contains(said), "{}", text(&out.stderr));
}

#[test]
fn zoom_width_is_wider_than_a_page_to_a_row() {
    let pdf = fixture("fit", 1);
    let page = images(&view(&pdf, &["--protocol", "kitty", "--zoom", "1"]).stdout);
    let width = images(&view(&pdf, &["--protocol", "kitty", "--zoom", "width"]).stdout);
    let (cols_page, cols_width) = (key(&page[0].keys, "c"), key(&width[0].keys, "c"));
    // A portrait page fits the height of any window wider than it is tall in cells.
    assert!(cols_width >= cols_page, "{cols_width} < {cols_page}");
    assert!(png_size(&width[0].png).0 >= png_size(&page[0].png).0);
    // As the width, the page moves the cursor down and the screen with it, as text does: a page
    // taller than the window scrolls by.
    assert!(width[0].keys.starts_with("a=T,f=100,t=d,q=2,c="), "{}", width[0].keys);
    // One page has no others to share its row: three to a row prints it as one does, and so does
    // auto.
    for zoom in ["3", "auto"] {
        let printed = images(&view(&pdf, &["--protocol", "kitty", "--zoom", zoom]).stdout);
        assert_eq!((key(&printed[0].keys, "c"), png_size(&printed[0].png)), (cols_page, png_size(&page[0].png)), "--zoom {zoom}");
    }
}

#[test]
fn without_a_terminal_auto_points_to_render() {
    let pdf = fixture("auto", 1);
    let out = view(&pdf, &[]);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    let stderr = text(&out.stderr);
    assert!(stderr.contains("stdout is not a terminal") && stderr.contains("render --out"), "{stderr}");
}

#[test]
fn page_numbers_are_checked_before_anything_is_drawn() {
    let pdf = fixture("past", 1);
    let out = view(&pdf, &["--pages", "1,3", "--protocol", "kitty"]);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert!(text(&out.stderr).contains("no page 3"), "{}", text(&out.stderr));
}

#[test]
fn bad_arguments_say_what_is_allowed() {
    let pdf = fixture("args", 1);
    for (args, expected) in [
        (&["--zoom", "sideways"][..], "use auto, width or a number of pages per row (1–8)"),
        (&["--zoom", "9"], "use auto, width or a number of pages per row (1–8)"),
        (&["--protocol", "sixel"], "use auto or kitty"),
        (&["--fit", "width"], "unknown option --fit"),
        (&["--pages", "0"], "bad page number"),
    ] {
        let out = view(&pdf, args);
        assert!(!out.status.success(), "{args:?}");
        assert!(text(&out.stderr).contains(expected), "{args:?}: {}", text(&out.stderr));
    }
    let out = Command::new(BIN).arg("view").output().unwrap();
    assert!(text(&out.stderr).contains("missing file"));
}

/// Run `view` as if in a tmux pane, with no `tmux` command on the PATH to ask.
fn view_in_tmux(test: &str, pdf: &Path, args: &[&str]) -> Output {
    let empty = tmp(test, "empty-bin");
    std::fs::create_dir_all(&empty).unwrap();
    Command::new(BIN)
        .arg("view")
        .arg(pdf)
        .args(args)
        .env("TMUX", "/nonexistent/tmux-socket,1,0")
        .env_remove("TMUX_PANE")
        .env_remove("KITTY_WINDOW_ID")
        .env_remove("TERM_PROGRAM")
        .env("TERM", "tmux-256color")
        .env("PATH", &empty)
        .output()
        .unwrap()
}

/// The bytes inside the tmux passthrough sequences at the start of `bytes` (`ESC P tmux; …
/// ESC \`, with every inner ESC doubled), and what follows them.
fn passthrough(bytes: &[u8]) -> (Vec<u8>, &[u8]) {
    let mut inner = Vec::new();
    let mut rest = bytes;
    while let Some(body) = rest.strip_prefix(b"\x1bPtmux;") {
        let mut i = 0;
        loop {
            match (body[i], body[i + 1]) {
                (0x1b, 0x1b) => {
                    inner.push(0x1b);
                    i += 2;
                }
                (0x1b, b'\\') => {
                    i += 2;
                    break;
                }
                (b, _) => {
                    assert_ne!(b, 0x1b, "a lone ESC inside a passthrough sequence");
                    inner.push(b);
                    i += 1;
                }
            }
        }
        rest = &body[i..];
    }
    (inner, rest)
}

#[test]
fn inside_tmux_pages_go_through_passthrough_and_placeholders_draw_them() {
    let pdf = fixture("tmux", 1);
    let out = view_in_tmux("tmux", &pdf, &["--protocol", "kitty"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    // With no tmux to ask, `view` says it couldn't check passthrough, and goes on.
    assert!(text(&out.stderr).contains("allow-passthrough"), "{}", text(&out.stderr));

    // The image is transmitted, then placed virtually, all through passthrough.
    let (codes, after) = passthrough(&out.stdout);
    let mut transmitted: Vec<(&str, &str)> = std::str::from_utf8(&codes)
        .unwrap()
        .strip_suffix("\x1b\\")
        .unwrap()
        .split("\x1b\\")
        .map(|code| {
            let body = code.strip_prefix("\x1b_G").unwrap();
            body.split_once(';').unwrap_or((body, ""))
        })
        .collect();
    let (place, _) = transmitted.pop().unwrap();
    let id = key(transmitted[0].0, "i");
    assert!(transmitted[0].0.starts_with("a=t,f=100,t=d,q=2,i="), "{}", transmitted[0].0);
    // tmux couldn't say whether it passes 24-bit colour, so a palette colour carries the low 8
    // bits and a third diacritic the high byte; nothing is left for the bits in between.
    assert_eq!(id & 0x00FF_FF00, 0, "{id:#x}");
    assert_ne!(id & 0xff, 0, "{id:#x}");
    let png = base64::engine::general_purpose::STANDARD.decode(transmitted.iter().map(|(_, p)| *p).collect::<String>()).unwrap();
    let (w, h) = png_size(&png);
    assert!((w as f32 / h as f32 - 612.0 / 792.0).abs() < 0.01, "{w}×{h}");
    let (cols, rows) = (key(place, "c"), key(place, "r"));
    assert_eq!(place, format!("a=p,U=1,q=2,i={id},c={cols},r={rows}"));

    // Then a line of placeholder cells per row, in the colour that names the image.
    let lines: Vec<&str> = std::str::from_utf8(after).unwrap().split_terminator('\n').collect();
    assert_eq!(lines.len(), rows as usize);
    for line in lines {
        assert!(line.starts_with(&format!("\x1b[38;5;{}m", id & 0xff)) && line.ends_with("\x1b[39m"), "{line:?}");
        assert_eq!(line.chars().filter(|c| *c == '\u{10EEEE}').count(), cols as usize);
    }
}

/// Printed inside tmux, pages side by side share their lines of placeholder cells: the first
/// page's, the gap, then the second's, each in the colour that names its image.
#[test]
fn inside_tmux_pages_side_by_side_share_their_lines() {
    let pdf = fixture("tmux-zoom", 2);
    let out = view_in_tmux("tmux-zoom", &pdf, &["--protocol", "kitty", "--zoom", "2"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let (codes, after) = passthrough(&out.stdout);
    let codes = std::str::from_utf8(&codes).unwrap();
    let placed: Vec<&str> = codes.split("\x1b\\").filter_map(|code| code.strip_prefix("\x1b_Ga=p,U=1,")).collect();
    assert_eq!(placed.len(), 2, "{codes:?}");
    let (first, second) = (key(placed[0], "i"), key(placed[1], "i"));
    assert_ne!(first, second);
    let (cols, rows) = (key(placed[0], "c"), key(placed[0], "r"));
    let lines: Vec<&str> = std::str::from_utf8(after).unwrap().split_terminator('\n').collect();
    assert_eq!(lines.len(), rows as usize);
    for line in lines {
        let (left, right) = line.split_once("\x1b[39m  ").unwrap_or_else(|| panic!("no gap between the pages: {line:?}"));
        assert!(left.starts_with(&format!("\x1b[38;5;{}m", first & 0xff)), "{line:?}");
        assert!(right.starts_with(&format!("\x1b[38;5;{}m", second & 0xff)) && right.ends_with("\x1b[39m"), "{line:?}");
        assert_eq!(line.chars().filter(|c| *c == '\u{10EEEE}').count(), 2 * cols as usize);
    }
}

#[test]
fn inside_tmux_auto_still_needs_a_terminal() {
    let pdf = fixture("tmux-auto", 1);
    let out = view_in_tmux("tmux-auto", &pdf, &[]);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert!(text(&out.stderr).contains("stdout is not a terminal"), "{}", text(&out.stderr));
}

#[test]
fn a_hostile_file_name_cant_send_escape_sequences_to_the_terminal() {
    // A name that would write to the clipboard (OSC 52) and retitle the window if printed raw.
    let dir = tmp("escape", "");
    let name = dir.join("x\x1b]52;c;cHduZWQ=\x07y\x1b]0;owned\x07.pdf");
    for args in [&[][..], &["--protocol", "kitty"], &["--pages", "x\x1b[2J"]] {
        let out = Command::new(BIN).arg("view").arg(&name).args(args).env_remove("TMUX").output().unwrap();
        assert!(!out.status.success());
        let stderr = text(&out.stderr);
        assert!(stderr.contains("x\u{FFFD}]52;c;cHduZWQ=\u{FFFD}y"), "{stderr:?}");
        assert!(!out.stderr.iter().any(|b| *b == 0x1b || *b == 0x07), "{stderr:?}");
    }
}

#[test]
fn compression_trades_bytes_for_speed_and_auto_follows_ssh() {
    let pdf = fixture("compression", 1);
    let png = |args: &[&str], env: &[(&str, &str)]| {
        let out = view_with(&pdf, &[&["--protocol", "kitty"], args].concat(), env);
        assert!(out.status.success(), "{}", text(&out.stderr));
        images(&out.stdout).remove(0).png
    };
    let (fast, small) = (png(&["--compression", "fast"], &[]), png(&["--compression", "small"], &[]));
    // The same page, in more bytes for less work.
    assert_eq!(png_size(&fast), png_size(&small));
    assert!(fast.len() > small.len(), "fast {} bytes, small {}", fast.len(), small.len());
    // auto: fast on this machine, small when the login came over SSH.
    assert_eq!(png(&[], &[]), fast);
    assert_eq!(png(&[], &[("SSH_CONNECTION", "192.0.2.7 50122 192.0.2.1 22")]), small);
    assert_eq!(png(&["--compression", "fast"], &[("SSH_TTY", "/dev/pts/3")]), fast);
    let bad = view(&pdf, &["--protocol", "kitty", "--compression", "best"]);
    assert!(text(&bad.stderr).contains("use auto, fast or small"));
}

#[test]
fn render_without_out_points_to_view() {
    let pdf = fixture("render", 1);
    let out = Command::new(BIN).args(["render", pdf.to_str().unwrap()]).output().unwrap();
    assert!(!out.status.success());
    assert!(text(&out.stderr).contains("pdfcraft-cli view"), "{}", text(&out.stderr));
}

#[test]
fn help_lists_view() {
    let out = Command::new(BIN).arg("--help").output().unwrap();
    assert!(text(&out.stdout).contains("pdfcraft-cli view   <file.pdf>"));
}
