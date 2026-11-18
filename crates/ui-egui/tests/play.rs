//! Human sessions for the editor.
//!
//! Each file in `tests/play/*.json` is one session a person can do: open a page, click a
//! paragraph, type, confirm, drag a box edge, then click the paragraph that must still be
//! separate and read it back. Every session is repeated on several desks (window size and
//! zoom), because that is what changes where a click lands.
//!
//! A miss writes a screenshot and the paragraph list to `/tmp/pdfcraft-play/` and fails this
//! test with that report. `cargo test --workspace` runs it on the Linux and Windows CI
//! machines. The paragraph rules themselves live in `pdfcraft-edit` and run on every OS that
//! job tests. A pass covers these sessions and these desks, not every PDF a person might open.
//!
//! Page coordinates are PDF points. The default page is US Letter (612 by 792).
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
    /// PDF page size in points. Omitted sessions use US Letter.
    #[serde(default = "letter")]
    page: [f32; 2],
    /// Window sizes and zooms. Omitted sessions use the shared desks.
    #[serde(default)]
    desks: Vec<Desk>,
    steps: Vec<Step>,
}

#[derive(Clone, Debug, Deserialize)]
struct Desk {
    width: f32,
    height: f32,
    /// Zoom percent, as `ui.set zoom` accepts it (`100` is 100%).
    zoom: String,
}

fn desks() -> Vec<Desk> {
    vec![
        Desk { width: 1100.0, height: 1400.0, zoom: "100".into() },
        Desk { width: 1280.0, height: 800.0, zoom: "60".into() },
        Desk { width: 900.0, height: 700.0, zoom: "50".into() },
    ]
}

fn letter() -> [f32; 2] {
    [612.0, 792.0]
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
    /// Select everything in the open paragraph and type, including line breaks.
    Type {
        text: String,
    },
    /// Replace the open paragraph's text without keystrokes. Prefer `type`.
    Editor {
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
    ExpectTogether {
        parts: Vec<String>,
    },
    ExpectApart {
        text: String,
        not: String,
    },
    Shot {
        name: String,
    },
}

#[test]
fn play_sessions_fetch_paragraph_bugs() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/play");
    let mut files: Vec<_> = fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())).map(|e| e.unwrap().path()).collect();
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
    let desks = if session.desks.is_empty() { desks() } else { session.desks.clone() };
    let mut bugs = Vec::new();
    for desk in &desks {
        if let Err(report) = play_desk(&session, desk) {
            bugs.push(report);
        }
    }
    if bugs.is_empty() { Ok(()) } else { Err(bugs.join("\n")) }
}

fn play_desk(session: &Session, desk: &Desk) -> Result<(), String> {
    let mut h = open_letter(&session.name, &session.content, desk);
    if let Err(e) = h.state_mut().set_option("zoom", &desk.zoom) {
        return Err(format!("{name} at {desk:?}: zoom: {e}", name = session.name));
    }
    h.run_steps(4);
    eprintln!("{name}: {width}×{height} at {zoom}%", name = session.name, width = desk.width, height = desk.height, zoom = desk.zoom);
    for (n, step) in session.steps.iter().enumerate() {
        if let Err(why) = step.run(&mut h, session) {
            let shot = save_shot(&mut h, &format!("{}-{}x{}", session.name, desk.width, desk.height));
            let paras = paragraphs(&h);
            return Err(format!(
                "{name} on a {width}×{height} window at {zoom}% zoom: step {step_no} {step:?}\n{why}\nparagraphs:\n{paras}\nscreenshot: {shot}",
                name = session.name,
                width = desk.width,
                height = desk.height,
                zoom = desk.zoom,
                step_no = n + 1,
                paras = paras.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n"),
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
            Step::Editor { text } => {
                let Some(editor) = h.state_mut().views.first_mut().and_then(|v| v.line_editor.as_mut()) else {
                    return Err("no paragraph is open for typing".into());
                };
                editor.text.clone_from(text);
                h.run_steps(1);
            }
            Step::Key { key, modifiers } => {
                let Some(key) = Key::from_name(key) else { return Err(format!("unknown key {key:?}")) };
                h.key_press_modifiers(modifiers_of(modifiers)?, key);
                h.run_steps(8);
            }
            Step::DragEdge { contains, dx, dy } => drag_edge(h, session, contains, *dx, *dy)?,
            Step::ExpectEditor { contains, text } => {
                // The open editor is what a click just showed. Otherwise take the shortest
                // paragraph that contains the words, so a heading that merely mentions them
                // does not win.
                let actual = if let Some(open) = h.state().views.first().and_then(|v| v.line_editor.as_ref()) {
                    open.text.clone()
                } else {
                    let mut matches: Vec<_> =
                        paragraphs_full(h).into_iter().map(|p| p.editor_text).filter(|p| p.contains(contains.as_str())).collect();
                    matches.sort_by_key(|p| p.len());
                    let Some(found) = matches.into_iter().next() else { return Err(format!("no paragraph contains {contains:?}")) };
                    found
                };
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
            Step::ExpectTogether { parts } => {
                if parts.is_empty() {
                    return Err("expect_together needs at least one part".into());
                }
                if !paragraphs(h).iter().any(|p| parts.iter().all(|part| p.contains(part))) {
                    return Err(format!("these are not in the same paragraph: {parts:?}"));
                }
            }
            Step::ExpectApart { text, not } => {
                let paras = paragraphs(h);
                if !paras.iter().any(|p| p.contains(text)) {
                    return Err(format!("no paragraph contains {text:?}"));
                }
                // A heading can mention both strings. The paragraph itself is one that has
                // `text` and does not also have `not`.
                if !paras.iter().any(|p| p.contains(text) && !p.contains(not)) {
                    return Err(format!("{text:?} was absorbed into a paragraph that also has {not:?}"));
                }
            }
            Step::Shot { name } => {
                let _ = save_named(&mut *h, name);
            }
        }
        Ok(())
    }
}

fn type_into_paragraph(h: &mut Harness<'static, PdfCraftApp>, text: &str) -> Result<(), String> {
    if h.state().views.first().and_then(|v| v.line_editor.as_ref()).is_none() {
        return Err("no paragraph is open for typing".into());
    }
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    // One text event keeps the line breaks. The multiline editor inserts them.
    h.event(egui::Event::Text(text.into()));
    h.run_steps(2);
    Ok(())
}

fn drag_edge(h: &mut Harness<'static, PdfCraftApp>, session: &Session, contains: &str, dx: f32, dy: f32) -> Result<(), String> {
    let edge = {
        let s = h.state();
        let Some(view) = s.views.first() else { return Err("no document is open".into()) };
        let Some(page) = view.page_screen_rect(0) else { return Err("the page is not on screen".into()) };
        let Some(doc) = s.session.get(view.id) else { return Err("the document is gone".into()) };
        let blocks = doc.text_blocks(0);
        let Some(block) = blocks.iter().find(|b| b.editor_text.contains(contains) || b.text.contains(contains)) else {
            return Err(format!("no paragraph contains {contains:?}"));
        };
        let rect = block.rect;
        let [pw, ph] = session.page;
        egui::pos2(page.left() + rect[2] as f32 / pw * page.width(), page.top() + (ph - (rect[1] + rect[3]) as f32 / 2.0) / ph * page.height())
    };
    let to = edge + egui::vec2(dx, dy);
    h.hover_at(edge);
    h.run_steps(1);
    h.drag_at(edge);
    h.run_steps(1);
    for k in 1..=6 {
        h.hover_at(edge + (to - edge) * (k as f32 / 6.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(8);
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
    paragraphs_full(h).into_iter().map(|b| b.text).collect()
}

fn paragraphs_full(h: &Harness<'static, PdfCraftApp>) -> Vec<pdfcraft_engine::TextBlock> {
    let s = h.state();
    let Some(view) = s.views.first() else { return Vec::new() };
    s.session.get(view.id).map(|d| d.text_blocks(0)).unwrap_or_default()
}

fn save_shot(h: &mut Harness<'static, PdfCraftApp>, name: &str) -> String {
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    save_named(h, &safe)
}

fn save_named(h: &mut Harness<'static, PdfCraftApp>, name: &str) -> String {
    for _ in 0..20 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
    }
    let dir = PathBuf::from("/tmp/pdfcraft-play");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}.png"));
    if let Ok(image) = h.render() {
        let _ = image.save(&path);
    }
    path.display().to_string()
}

fn open_letter(name: &str, content: &str, desk: &Desk) -> Harness<'static, PdfCraftApp> {
    let bytes = letter_pdf(content);
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

fn letter_pdf(content: &str) -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
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
