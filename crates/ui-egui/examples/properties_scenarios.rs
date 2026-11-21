//! JSONL adapter for interaction-check, driving the real Document Properties dialog.
//!
//! Each process owns two synthetic PDFs. Widget edits/clicks use egui_kittest; external
//! document edits, reloads and save/reopen use the real engine. No network or shipped API.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_engine::{DocId, Edit, Session};
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};
use serde::Deserialize;
use serde_json::{Value, json};

const LIMIT: u64 = 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    protocol: u64,
    id: u64,
    event: Event,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Event {
    Open,
    Type { field: String, value: String },
    Click { label: String },
    Escape,
    ExternalEdit { key: String, value: String },
    ExternalEditUndo { key: String, value: String },
    Switch { document: usize },
    Reload { title: String },
    CloseDocument,
    Undo,
    Redo,
    SaveReopen,
    Inspect,
}

/// Build and save a synthetic two-page PDF with known standard and custom metadata.
fn fixture(title: &str) -> Result<Arc<Vec<u8>>, String> {
    let mut session = Session::new();
    let bytes = session.create_blank(200.0, 300.0, 2).map_err(|e| e.to_string())?;
    let id = session.open("fixture.pdf", None, bytes, None).map_err(|e| e.to_string())?;
    session
        .apply(
            id,
            Edit::Batch {
                label: "Fixture metadata".into(),
                edits: vec![
                    Edit::SetInfo { key: "Title".into(), value: title.into() },
                    Edit::SetInfo { key: "Author".into(), value: "Original author".into() },
                    Edit::SetInfo { key: "Untouched".into(), value: "Keep me".into() },
                ],
            },
        )
        .map_err(|e| e.to_string())?;
    session.save_full_bytes(id).map_err(|e| e.to_string())
}

/// Expose actual engine metadata, edit generation and history labels for subset assertions.
fn document_state(doc: &pdfcraft_engine::Document) -> Value {
    let custom: serde_json::Map<String, Value> = doc.custom_info().into_iter().map(|(k, v)| (k, json!(v))).collect();
    let custom_count = custom.len();
    json!({
        "title": doc.info_value("Title"), "author": doc.info_value("Author"),
        "subject": doc.info_value("Subject"), "keywords": doc.info_value("Keywords"),
        "custom": custom, "custom_count": custom_count, "display_title": doc.initial_view().display_title,
        "dirty": doc.dirty, "undo": doc.can_undo(), "redo": doc.can_redo(),
        "generation": doc.edit_generation(), "pages": doc.info.pages.len(),
    })
}

struct Adapter {
    h: Harness<'static, PdfCraftApp>,
    originals: Vec<(DocId, Arc<Vec<u8>>)>,
    reopened: Option<Value>,
    artifacts: Option<PathBuf>,
}

impl Adapter {
    /// Open two independent fixtures and create a deterministic 60 Hz headless UI harness.
    fn new(artifacts: Option<PathBuf>) -> Result<Self, String> {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en")?;
        let mut originals = Vec::new();
        for (name, title) in [("A.pdf", "Original"), ("B.pdf", "Second document")] {
            let bytes = fixture(title)?;
            app.open_bytes(name, None, bytes.as_ref().clone()).map_err(|e| e.to_string())?;
            let id = app.active_ids().map(|(_, id)| id).ok_or("fixture did not open")?;
            originals.push((id, bytes));
        }
        app.active = Some(0);
        let mut h = Harness::builder().with_size(egui::vec2(1200.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_| app);
        h.run_steps(4);
        if let Some(path) = &artifacts {
            std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
        }
        Ok(Self { h, originals, reopened: None, artifacts })
    }

    /// Resolve the current document, returning a protocol error when no tab is active.
    fn active(&self) -> Result<DocId, String> {
        self.h.state().active_ids().map(|(_, id)| id).ok_or_else(|| "no active document".into())
    }

    /// Report whether the active modal is a Document Properties tab.
    fn properties_open(&self) -> bool {
        matches!(self.h.state().dialog, Some(Dialog::Properties(_)))
    }

    /// Replace an enabled property field through focus and keyboard events; never set its buffer directly.
    fn type_text(&mut self, field: &str, value: &str) -> Result<(), String> {
        if !self.properties_open() || !matches!(field, "Title" | "Author" | "Subject" | "Keywords" | "Name:" | "Value:") {
            return Err("no editable property field".into());
        }
        if value.len() > 65536 {
            return Err("property input exceeds 64 KiB".into());
        }
        {
            let mut nodes = self.h.query_all_by_role_and_label(Role::TextInput, field);
            let node = nodes.next().ok_or_else(|| format!("missing field: {field}"))?;
            if nodes.next().is_some() || node.accesskit_node().is_disabled() {
                return Err(format!("ambiguous or disabled field: {field}"));
            }
            node.focus();
        }
        self.h.run_steps(1);
        self.h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        self.h.run_steps(1);
        self.h.key_press(egui::Key::Backspace);
        self.h.event(egui::Event::Text(value.to_owned()));
        Ok(())
    }

    /// Click one enabled button or radio control; reject unsupported or ambiguous targets.
    fn click(&mut self, label: &str) -> Result<(), String> {
        if !self.properties_open() {
            return Err("properties dialog is closed".into());
        }
        if !matches!(
            label,
            "OK" | "Cancel"
                | "Close"
                | "Description"
                | "Initial View"
                | "Custom"
                | "Advanced"
                | "Space Audit"
                | "Add"
                | "Update"
                | "Delete"
                | "Document Title"
                | "File Name"
        ) {
            return Err("unsupported property action".into());
        }
        let role = if matches!(label, "Document Title" | "File Name") { Role::RadioButton } else { Role::Button };
        let mut nodes = self.h.query_all_by_role_and_label(role, label);
        let node = nodes.next().ok_or_else(|| format!("missing action: {label}"))?;
        if nodes.next().is_some() || node.accesskit_node().is_disabled() {
            return Err(format!("ambiguous or disabled action: {label}"));
        }
        node.click();
        Ok(())
    }

    /// Dispatch a semantic event through UI controls or the documented external-change engine path.
    fn event(&mut self, event: Event) -> Result<(), String> {
        match event {
            Event::Open => {
                self.active()?;
                if self.properties_open() {
                    return Err("properties dialog is already open".into());
                }
                self.h.state_mut().run_command("file.properties");
            }
            Event::Type { field, value } => self.type_text(&field, &value)?,
            Event::Click { label } => self.click(&label)?,
            Event::Escape => self.h.key_press(egui::Key::Escape),
            Event::ExternalEdit { key, value } => {
                let id = self.active()?;
                self.h.state_mut().session.apply(id, Edit::SetInfo { key, value }).map_err(|e| e.to_string())?;
            }
            Event::ExternalEditUndo { key, value } => {
                let id = self.active()?;
                self.h.state_mut().session.apply(id, Edit::SetInfo { key, value }).map_err(|e| e.to_string())?;
                self.h.state_mut().session.undo(id).map_err(|e| e.to_string())?;
            }
            Event::Switch { document } => {
                // Scenario document numbers are 1-based, like PdfCraft's automation pages.
                let index = document.checked_sub(1).ok_or("document numbers start at 1")?;
                if self.h.state().views.get(index).is_none() {
                    return Err("document is not open".into());
                }
                self.h.state_mut().active = Some(index);
            }
            Event::Reload { title } => {
                let id = self.active()?;
                self.h.state_mut().session.reload(id, fixture(&title)?).map_err(|e| e.to_string())?;
            }
            Event::CloseDocument => {
                let id = self.active()?;
                let app = self.h.state_mut();
                app.session.close(id);
                app.views.retain(|view| view.id != id);
                app.active = (!app.views.is_empty()).then_some(0);
            }
            Event::Undo | Event::Redo => {
                let id = self.active()?;
                let doc = self.h.state().session.get(id).ok_or("document is not open")?;
                let undo = matches!(event, Event::Undo);
                if (if undo { doc.can_undo() } else { doc.can_redo() }).is_none() {
                    return Err("history action is unavailable".into());
                }
                self.h.state_mut().run_command(if undo { "edit.undo" } else { "edit.redo" });
            }
            Event::SaveReopen => {
                let id = self.active()?;
                let bytes = self.h.state().session.save_bytes(id).map_err(|e| e.to_string())?;
                if let Some(path) = &self.artifacts {
                    std::fs::write(path.join("saved.pdf"), bytes.as_slice()).map_err(|e| e.to_string())?;
                }
                let mut fresh = Session::new();
                let reopened = fresh.open("reopened.pdf", None, bytes, None).map_err(|e| e.to_string())?;
                self.reopened = fresh.get(reopened).map(document_state);
            }
            Event::Inspect => {}
        }
        Ok(())
    }

    /// Report engine and draft state after an event, retaining any action error for the runner to assert.
    fn state(&self, error: Option<String>) -> Value {
        let app = self.h.state();
        let doc = app.active_ids().and_then(|(_, id)| app.session.get(id));
        let original_bytes =
            doc.is_some_and(|doc| self.originals.iter().any(|(id, bytes)| *id == doc.id && bytes.as_slice() == doc.bytes.as_slice()));
        let draft = app.props_draft.as_ref().map(|(_, fields)| {
            let custom: serde_json::Map<String, Value> =
                app.custom_draft.as_ref().into_iter().flat_map(|(_, d)| d.props.iter().map(|(k, v)| (k.clone(), json!(v)))).collect();
            json!({
                "title": fields.first(), "author": fields.get(1), "subject": fields.get(2), "keywords": fields.get(3),
                "custom": custom, "display_title": app.view_draft.as_ref().map(|(_, v)| v.display_title),
            })
        });
        json!({
            "error": error, "properties_open": self.properties_open(), "active": doc.map(|d| &d.name),
            "notice": app.toast.as_ref().map(|(message, _)| message),
            "document": doc.map(document_state), "draft": draft, "original_bytes": original_bytes,
            "reopened": self.reopened,
        })
    }

    /// Optionally render a local PNG, reporting renderer setup or file errors as failures.
    fn snapshot(&mut self, step: u64) -> Result<(), String> {
        let Some(path) = &self.artifacts else { return Ok(()) };
        // kittest's optional renderer panics when no GPU/software adapter is available.
        // Report that setup failure without turning it into a successful scenario.
        let image = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.h.render()))
            .map_err(|_| "headless renderer failed; check GPU/software-renderer availability".to_owned())??;
        image.save(path.join(format!("step-{step:03}.png"))).map_err(|e| e.to_string())
    }
}

/// Serve bounded protocol-1 JSONL requests in order, returning one state per request until EOF.
fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let artifacts = match args.next().as_deref() {
        None => None,
        Some("--artifacts") => Some(PathBuf::from(args.next().ok_or("--artifacts needs a directory")?)),
        _ => return Err("usage: properties_scenarios [--artifacts DIR]".into()),
    };
    if args.next().is_some() {
        return Err("unexpected command-line argument".into());
    }
    let mut adapter = Adapter::new(artifacts)?;
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let mut expected_id = 1_u64;
    loop {
        let mut line = String::new();
        let read = input.by_ref().take(LIMIT + 1).read_line(&mut line).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        if read as u64 > LIMIT {
            return Err("request exceeds 1 MiB".into());
        }
        let request: Request = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        if request.protocol != 1 || request.id != expected_id {
            return Err("expected protocol 1 and consecutive request IDs starting at 1".into());
        }
        let error = adapter.event(request.event).err();
        // Tabs animate the modal's height and position. Advance a bounded 16 frames
        // (about 267 ms of simulated time) before inspecting or targeting the next control.
        adapter.h.run_steps(16);
        adapter.snapshot(request.id)?;
        serde_json::to_writer(&mut output, &json!({"protocol": 1, "id": request.id, "state": adapter.state(error)})).map_err(|e| e.to_string())?;
        writeln!(output).map_err(|e| e.to_string())?;
        output.flush().map_err(|e| e.to_string())?;
        expected_id = expected_id.checked_add(1).ok_or("too many requests")?;
    }
    Ok(())
}
