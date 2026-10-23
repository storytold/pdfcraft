//! Human sessions for the editor. See `docs/developing.md`.
//!
//! Each file in `tests/play/*.json` opens a synthetic page and drives the real editor widgets:
//! click, type, confirm, drag, then read the text that opened. Every session runs on each
//! desk (window size and zoom). A miss writes a screenshot under `/tmp/pdfcraft-play/` and
//! fails this test with the paragraph list.
//!
//! ```text
//! cargo test -p pdfcraft-ui-egui --test play -- --nocapture
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Session {
    name: String,
    content: String,
    #[serde(default = "letter")]
    page: [f32; 2],
    #[serde(default)]
    desks: Vec<Desk>,
    steps: Vec<Step>,
}

#[derive(Clone, Debug, Deserialize)]
struct Desk {
    width: f32,
    height: f32,
    zoom: String,
}

fn letter() -> [f32; 2] {
    [612.0, 792.0]
}

fn default_desks() -> Vec<Desk> {
    vec![Desk { width: 1100.0, height: 1400.0, zoom: "100".into() }, Desk { width: 1280.0, height: 800.0, zoom: "60".into() }]
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Step {
    Command {
        id: String,
    },
    ClickPage {
        x: f32,
        y: f32,
    },
    Type {
        text: String,
    },
    Key {
        key: String,
        #[serde(default)]
        modifiers: Vec<String>,
    },
    DragEdge {
        contains: String,
        dx: f32,
        #[serde(default)]
        dy: f32,
    },
    ExpectEditor {
        contains: String,
        text: String,
    },
    ExpectParagraph {
        text: String,
    },
    Shot {
        name: String,
    },
}

#[test]
fn play_sessions() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/play");
    let mut files: Vec<_> =
        fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())).map(|e| e.expect("a session file").path()).collect();
    files.retain(|p| p.extension().is_some_and(|x| x == "json"));
    files.sort();
    assert!(!files.is_empty(), "no sessions in {}", dir.display());
    let mut bugs = Vec::new();
    for path in &files {
        if let Err(report) = play_file(path) {
            bugs.push(report);
        }
    }
    assert!(bugs.is_empty(), "\n\n{}", bugs.join("\n\n"));
}

fn play_file(path: &Path) -> Result<(), String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let session: Session = serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
    let desks = if session.desks.is_empty() { default_desks() } else { session.desks.clone() };
    let mut bugs = Vec::new();
    for desk in &desks {
        if let Err(report) = play_desk(&session, desk) {
            bugs.push(report);
        }
    }
    if bugs.is_empty() { Ok(()) } else { Err(bugs.join("\n")) }
}

fn play_desk(session: &Session, desk: &Desk) -> Result<(), String> {
    let mut h = open_letter(&session.name, &session.content, desk, session.page);
    h.state_mut().set_option("zoom", &desk.zoom).map_err(|e| format!("{}: zoom: {e}", session.name))?;
    h.run_steps(4);
    eprintln!("{name}: {w}×{h} at {z}%", name = session.name, w = desk.width, h = desk.height, z = desk.zoom);
    for (n, step) in session.steps.iter().enumerate() {
        if let Err(why) = step.run(&mut h, session) {
            let shot = match save_shot(&mut h, &format!("{}-{}x{}", session.name, desk.width, desk.height)) {
                Ok(path) => path,
                Err(error) => format!("not written ({error})"),
            };
            let paras = paragraphs(&h).join("\n");
            return Err(format!(
                "{name} on {w}×{h} at {z}%: step {step_no} {step:?}\n{why}\nparagraphs:\n{paras}\nscreenshot: {shot}",
                name = session.name,
                w = desk.width,
                h = desk.height,
                z = desk.zoom,
                step_no = n + 1,
            ));
        }
    }
    Ok(())
}

impl Step {
    fn run(&self, h: &mut Harness<'static, PdfCraftApp>, session: &Session) -> Result<(), String> {
        match self {
            Step::Command { id } => {
                if !h.state_mut().execute(id) {
                    return Err(format!("command {id:?} did not run"));
                }
                h.run_steps(2);
            }
            Step::ClickPage { x, y } => click_page(h, session, *x, *y)?,
            Step::Type { text } => type_into_paragraph(h, text)?,
            Step::Key { key, modifiers } => {
                let Some(key) = Key::from_name(key) else { return Err(format!("unknown key {key:?}")) };
                h.key_press_modifiers(modifiers_of(modifiers)?, key);
                h.run_steps(6);
            }
            Step::DragEdge { contains, dx, dy } => drag_edge(h, session, contains, *dx, *dy)?,
            Step::ExpectEditor { contains, text } => {
                let actual = open_editor_text(h).ok_or_else(|| format!("no paragraph is open to read {contains:?}"))?;
                if !actual.contains(contains.as_str()) {
                    return Err(format!("the open paragraph is {actual:?}, which does not contain {contains:?}"));
                }
                if actual != *text {
                    return Err(format!("editor text\n  actual: {actual:?}\n  expect: {text:?}"));
                }
            }
            Step::ExpectParagraph { text } => {
                if !paragraphs(h).iter().any(|p| p.contains(text)) {
                    return Err(format!("no paragraph contains {text:?}"));
                }
            }
            Step::Shot { name } => {
                save_named(h, name)?;
            }
        }
        Ok(())
    }
}

fn open_editor_text(h: &Harness<'static, PdfCraftApp>) -> Option<String> {
    h.state().views.first().and_then(|v| v.line_editor.as_ref()).map(|e| e.text.clone())
}

fn type_into_paragraph(h: &mut Harness<'static, PdfCraftApp>, text: &str) -> Result<(), String> {
    if open_editor_text(h).is_none() {
        return Err("no paragraph is open for typing".into());
    }
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    h.event(egui::Event::Text(text.into()));
    h.run_steps(2);
    Ok(())
}

fn click_page(h: &mut Harness<'static, PdfCraftApp>, session: &Session, x: f32, y: f32) -> Result<(), String> {
    let page = h.state().views.first().and_then(|v| v.page_screen_rect(0)).ok_or("the page is not on screen")?;
    let [pw, ph] = session.page;
    let at = egui::pos2(page.left() + x / pw * page.width(), page.top() + (ph - y) / ph * page.height());
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(4);
    Ok(())
}

fn drag_edge(h: &mut Harness<'static, PdfCraftApp>, session: &Session, contains: &str, dx: f32, dy: f32) -> Result<(), String> {
    let edge = {
        let s = h.state();
        let view = s.views.first().ok_or("no document is open")?;
        let page = view.page_screen_rect(0).ok_or("the page is not on screen")?;
        let doc = s.session.get(view.id).ok_or("the document is gone")?;
        let blocks = doc.text_blocks(0);
        let block = blocks
            .iter()
            .find(|b| b.editor_text.contains(contains) || b.text.contains(contains))
            .ok_or_else(|| format!("no paragraph contains {contains:?}"))?;
        let rect = block.rect;
        let [pw, ph] = session.page;
        egui::pos2(page.left() + rect[2] as f32 / pw * page.width(), page.top() + (ph - (rect[1] + rect[3]) as f32 / 2.0) / ph * page.height())
    };
    let to = edge + egui::vec2(dx, dy);
    h.hover_at(edge);
    h.run_steps(1);
    h.drag_at(edge);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(edge + (to - edge) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(6);
    Ok(())
}

fn modifiers_of(names: &[String]) -> Result<Modifiers, String> {
    let mut m = Modifiers::NONE;
    for name in names {
        match name.as_str() {
            "command" | "cmd" => m.command = true,
            "shift" => m.shift = true,
            "alt" | "option" => m.alt = true,
            "ctrl" | "control" => m.ctrl = true,
            other => return Err(format!("unknown modifier {other:?}")),
        }
    }
    if m.command {
        m.mac_cmd = cfg!(target_os = "macos");
        m.ctrl |= !cfg!(target_os = "macos");
    }
    Ok(m)
}

fn paragraphs(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    let s = h.state();
    let Some(view) = s.views.first() else { return Vec::new() };
    s.session.get(view.id).map(|d| d.text_blocks(0).into_iter().map(|b| b.text).collect()).unwrap_or_default()
}

fn save_shot(h: &mut Harness<'static, PdfCraftApp>, name: &str) -> Result<String, String> {
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    save_named(h, &safe)
}

fn save_named(h: &mut Harness<'static, PdfCraftApp>, name: &str) -> Result<String, String> {
    for _ in 0..20 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
    }
    let dir = PathBuf::from("/tmp/pdfcraft-play");
    fs::create_dir_all(&dir).map_err(|error| format!("screenshot directory: {error}"))?;
    let path = dir.join(format!("{name}.png"));
    let image = h.render().map_err(|error| format!("screenshot render: {error}"))?;
    image.save(&path).map_err(|error| format!("screenshot {}: {error}", path.display()))?;
    Ok(path.display().to_string())
}

fn open_letter(name: &str, content: &str, desk: &Desk, page: [f32; 2]) -> Harness<'static, PdfCraftApp> {
    let bytes = letter_pdf(content, page);
    let title = name.to_string();
    let mut h = Harness::builder().with_size(egui::vec2(desk.width, desk.height)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        let _ = app.set_option("language", "en");
        let _ = app.open_bytes(&title, None, bytes);
        app
    });
    h.run_steps(4);
    h
}

fn letter_pdf(content: &str, page: [f32; 2]) -> Vec<u8> {
    let [width, height] = page;
    let stream = format!("{content}\n");
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"),
        format!("<< /Length {} >>\nstream\n{stream}endstream", stream.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}
