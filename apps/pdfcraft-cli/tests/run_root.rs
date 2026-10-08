//! `pdfcraft-cli run --root`: a script's image output stays inside the root (#137).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";

/// `root/` next to `outside/` (holding `existing.txt`) in a fresh temporary directory.
fn sandbox(test: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("pdfcraft-cli-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("root")).unwrap();
    std::fs::create_dir_all(base.join("outside")).unwrap();
    std::fs::write(base.join("outside/existing.txt"), "SENTINEL\n").unwrap();
    base
}

/// Run a two-step script (a blank page, then render it to `out`) from the working directory
/// `cwd`, with `--root root` when `root` is given.
fn render_to(base: &Path, cwd: &Path, root: Option<&Path>, out: &str) -> Output {
    let script = base.join("steps.json");
    let steps = serde_json::json!([
        { "tool": "doc_create", "args": { "from": "blank", "width": 100, "height": 100 } },
        { "tool": "page_render", "args": { "doc": 1, "page": 1, "dpi": 10 }, "out": out },
    ]);
    std::fs::write(&script, steps.to_string()).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_pdfcraft-cli"));
    cmd.current_dir(cwd).arg("run").arg("--script").arg(&script);
    if let Some(root) = root {
        cmd.arg("--root").arg(root);
    }
    cmd.output().unwrap()
}

fn is_png(path: &Path) -> bool {
    std::fs::read(path).is_ok_and(|b| b.starts_with(PNG))
}

#[test]
fn script_image_output_is_confined_to_the_root() {
    let base = sandbox("run-root");
    let root = base.join("root");
    let abs = |rel: &str| base.join(rel).to_str().unwrap().to_owned();

    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut refused = vec![abs("outside/escaped.png"), abs("outside/existing.txt"), "../outside/escaped.png".into(), abs("outside/new/dir/x.png")];
    #[cfg(windows)]
    {
        refused.push(r"..\outside\escaped.png".into());
        refused.push(r"\\pdfcraft-test.invalid\share\x.png".into());
        if let Some(drive) = (b'D'..=b'Z').rev().map(|d| format!("{}:\\", d as char)).find(|d| !Path::new(d).exists()) {
            refused.push(format!("{drive}x.png"));
        }
    }
    for out in &refused {
        let o = render_to(&base, &base, Some(&root), out);
        let stderr = String::from_utf8_lossy(&o.stderr);
        assert!(!o.status.success(), "{out}: expected a refusal");
        assert!(stderr.contains("step 2 (page_render)") && stderr.contains("is outside the allowed directory"), "{out}: {stderr}");
    }
    assert!(!base.join("outside/escaped.png").exists() && !base.join("outside/new").exists());
    assert_eq!(std::fs::read_to_string(base.join("outside/existing.txt")).unwrap(), "SENTINEL\n");

    // "." is the root itself: refused, without staging a temporary file beside the root.
    std::fs::write(base.join(".root.pdfcraft-tmp"), "SENTINEL\n").unwrap();
    let o = render_to(&base, &base, Some(&root), ".");
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("is a folder"), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(std::fs::read_to_string(base.join(".root.pdfcraft-tmp")).unwrap(), "SENTINEL\n");

    // A relative path resolves inside the root, not in the working directory.
    let o = render_to(&base, &base, Some(&root), "relative.png");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(is_png(&root.join("relative.png")) && !base.join("relative.png").exists());
    // So does an absolute one inside the root, and a new folder in it.
    for (out, file) in [(abs("root/inside.png"), "inside.png"), ("pages/one.png".into(), "pages/one.png")] {
        let o = render_to(&base, &base, Some(&root), &out);
        assert!(o.status.success(), "{out}: {}", String::from_utf8_lossy(&o.stderr));
        assert!(is_png(&root.join(file)), "{out}");
    }

    // Without --root, the path is used as given.
    let o = render_to(&base, &base, None, "free.png");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(is_png(&base.join("free.png")));
    // Written in place, not through a temporary file beside it, so a device still works.
    #[cfg(unix)]
    {
        let o = render_to(&base, &base, None, "/dev/stdout");
        assert!(o.status.success() && o.stdout.windows(PNG.len()).any(|w| w == PNG), "{}", String::from_utf8_lossy(&o.stderr));
    }
    let _ = std::fs::remove_dir_all(&base);
}
