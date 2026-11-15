//! A single-document, serialized mobile boundary. PDF semantics stay in the existing engine.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
// swift-bridge 0.1.59 emits a redundant pointer cast outside its annotated module.
#![cfg_attr(feature = "swift", allow(clippy::unnecessary_cast))]

use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::{self, SyncSender};
use std::thread::JoinHandle;

use pdfcraft_automation::{Automation, Content};
use pdfcraft_engine::DocId;
use serde_json::{Value, json};

const MAX_INPUT: u64 = 32 * 1024 * 1024;
const MAX_PAGES: usize = 2_000;
const MAX_EDGE: u32 = 2_048;
const MAX_PIXELS: f64 = 4_000_000.0;

#[cfg(feature = "swift")]
#[swift_bridge::bridge]
mod ffi {
    extern "Rust" {
        type MobileSession;
        fn create_mobile_session() -> Result<MobileSession, String>;
        #[swift_bridge(rust_name = "open_response")]
        fn open(&self, path: &str) -> String;
        fn render(&self, page: u32, edge: u32) -> Result<String, String>;
        fn rotate(&self, page: u32) -> Result<String, String>;
        fn save_copy(&self) -> Result<String, String>;
        fn close(&self, discard: bool) -> Result<String, String>;
    }
}

enum Operation {
    Open(String),
    Render(u32, u32),
    Rotate(u32),
    Save,
    Close(bool),
}

struct Job {
    operation: Operation,
    reply: SyncSender<Result<String, String>>,
}

/// One owned worker; non-Send engine state never crosses the worker boundary.
pub struct MobileSession {
    sender: Option<SyncSender<Job>>,
    worker: Option<JoinHandle<()>>,
}

pub fn create_mobile_session() -> Result<MobileSession, String> {
    let directory = tempfile::Builder::new().prefix("pdfcraft-mobile-").tempdir().map_err(|_| error("storage", "Cannot create private workspace"))?;
    let (sender, receiver) = mpsc::sync_channel::<Job>(1);
    let worker = std::thread::Builder::new()
        .name("pdfcraft-mobile".into())
        .spawn(move || {
            let mut state = State { automation: Automation::new().with_javascript(false), document: None, directory, copies: 0 };
            for job in receiver {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| state.run(job.operation)));
                let failed = result.is_err();
                let result = result.unwrap_or_else(|_| Err(error("engine", "The engine stopped unexpectedly. Close and reopen the document.")));
                // A disconnected caller has cancelled delivery; engine state remains on this worker.
                let _ = job.reply.send(result);
                if failed {
                    break;
                }
            }
        })
        .map_err(|_| error("worker", "Cannot start the document worker"))?;
    Ok(MobileSession { sender: Some(sender), worker: Some(worker) })
}

impl Drop for MobileSession {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            // Panics are contained at the operation boundary; no document data is logged.
            if worker.join().is_err() {
                eprintln!("PdfCraft mobile worker stopped unexpectedly");
            }
        }
    }
}

impl MobileSession {
    #[cfg(feature = "swift")]
    fn open_response(&self, path: &str) -> String {
        // swift-bridge 0.1.59 cannot generate a throwing Swift closure for a borrowed str.
        match self.open(path) {
            Ok(value) => json!({ "ok": true, "value": value }).to_string(),
            Err(value) => json!({ "ok": false, "value": value }).to_string(),
        }
    }

    fn call(&self, operation: Operation) -> Result<String, String> {
        let sender = self.sender.as_ref().ok_or_else(|| error("closed", "The document worker is closed"))?;
        let (reply, result) = mpsc::sync_channel(1);
        sender.try_send(Job { operation, reply }).map_err(|e| match e {
            mpsc::TrySendError::Full(_) => error("busy", "The document worker is busy"),
            mpsc::TrySendError::Disconnected(_) => error("closed", "The document worker is closed"),
        })?;
        result.recv().map_err(|_| error("worker", "The document worker stopped"))?
    }

    pub fn open(&self, path: &str) -> Result<String, String> {
        self.call(Operation::Open(path.into()))
    }

    pub fn render(&self, page: u32, edge: u32) -> Result<String, String> {
        self.call(Operation::Render(page, edge))
    }

    pub fn rotate(&self, page: u32) -> Result<String, String> {
        self.call(Operation::Rotate(page))
    }

    pub fn save_copy(&self) -> Result<String, String> {
        self.call(Operation::Save)
    }

    pub fn close(&self, discard: bool) -> Result<String, String> {
        self.call(Operation::Close(discard))
    }
}

fn error(code: &str, message: &str) -> String {
    json!({ "code": code, "message": message }).to_string()
}

struct State {
    automation: Automation,
    document: Option<u64>,
    directory: tempfile::TempDir,
    copies: u32,
}

impl State {
    fn tool(&mut self, name: &str, args: Value) -> Result<Vec<Content>, String> {
        self.automation.call(name, &args).map_err(|e| error("document", &e.to_string()))
    }

    fn id(&self) -> Result<u64, String> {
        self.document.ok_or_else(|| error("closed", "Open a PDF first"))
    }

    fn metadata(&self) -> Result<String, String> {
        let doc = self.automation.session().get(DocId(self.id()?)).ok_or_else(|| error("closed", "The document is closed"))?;
        Ok(json!({
            "page_count": doc.info.pages.len(),
            "dirty": doc.dirty,
            "editable": doc.read_only_reason.is_none(),
            "pages": doc.info.pages.iter().enumerate().map(|(index, page)| json!({
                "number": index + 1, "width": page.width, "height": page.height, "rotation": page.rotation
            })).collect::<Vec<_>>(),
            "warnings": doc.info.warnings,
        })
        .to_string())
    }

    fn run(&mut self, operation: Operation) -> Result<String, String> {
        match operation {
            Operation::Open(path) => {
                if self.document.is_some() {
                    return Err(error("open", "Close the current document before opening another PDF"));
                }
                // A bounded private snapshot also makes Files-provider input stable during parsing.
                let file = std::fs::File::open(Path::new(&path)).map_err(|_| error("access", "Cannot read the selected PDF"))?;
                let metadata = file.metadata().map_err(|_| error("access", "Cannot inspect the selected PDF"))?;
                if !metadata.is_file() || metadata.len() > MAX_INPUT {
                    return Err(error("limit", "Select a regular PDF file no larger than 32 MiB"));
                }
                let mut input =
                    tempfile::NamedTempFile::new_in(self.directory.path()).map_err(|_| error("storage", "Cannot stage the selected PDF"))?;
                let copied = std::io::copy(&mut file.take(MAX_INPUT + 1), &mut input)
                    .map_err(|_| error("storage", "Cannot read the selected PDF into the workspace"))?;
                if copied > MAX_INPUT {
                    return Err(error("limit", "The PDF exceeds 32 MiB"));
                }
                input.flush().map_err(|_| error("storage", "Cannot stage the selected PDF"))?;
                let path = input.path().to_str().ok_or_else(|| error("storage", "The workspace path is not UTF-8"))?;
                let result = self.tool("doc_open", json!({ "path": path }))?;
                let id = result
                    .iter()
                    .find_map(|part| match part {
                        Content::Json(value) => value.get("doc").and_then(Value::as_u64),
                        _ => None,
                    })
                    .ok_or_else(|| error("engine", "The engine returned no document"))?;
                self.document = Some(id);
                let count = self.automation.session().get(DocId(id)).map_or(0, |doc| doc.info.pages.len());
                if count == 0 || count > MAX_PAGES {
                    self.tool("doc_close", json!({ "doc": id, "discard_changes": true }))?;
                    self.document = None;
                    return Err(error("limit", "This proof of concept supports PDFs with 1 to 2,000 pages"));
                }
                let invalid_geometry = self.automation.session().get(DocId(id)).is_none_or(|doc| {
                    doc.info.pages.iter().any(|page| {
                        let ratio = page.width / page.height;
                        !page.width.is_finite()
                            || !page.height.is_finite()
                            || page.width <= 0.0
                            || page.height <= 0.0
                            || !(0.01..=100.0).contains(&ratio)
                    })
                });
                if invalid_geometry {
                    self.tool("doc_close", json!({ "doc": id, "discard_changes": true }))?;
                    self.document = None;
                    return Err(error("limit", "Page dimensions must be positive and finite, with an aspect ratio between 1:100 and 100:1"));
                }
                self.metadata()
            }
            Operation::Render(page, edge) => {
                let id = self.id()?;
                if edge == 0 || edge > MAX_EDGE {
                    return Err(error("limit", "Render edge must be between 1 and 2,048 pixels"));
                }
                let index = page.checked_sub(1).ok_or_else(|| error("page", "Pages start at 1"))? as usize;
                let doc = self.automation.session().get(DocId(id)).ok_or_else(|| error("closed", "The document is closed"))?;
                let info = doc.info.pages.get(index).ok_or_else(|| error("page", "The page is out of range"))?;
                let (width, height) = (f64::from(info.width), f64::from(info.height));
                if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
                    return Err(error("page", "The page dimensions are invalid"));
                }
                let scale = (f64::from(edge) / width.max(height)).min((MAX_PIXELS / (width * height)).sqrt()).min(2.0);
                let dpi = scale * 72.0;
                if !dpi.is_finite() || dpi < 1.0 {
                    return Err(error("limit", "The page is too large to render within the mobile budget"));
                }
                let content = self.tool("page_render", json!({ "doc": id, "page": page, "dpi": dpi }))?;
                let image = content
                    .iter()
                    .find_map(|part| match part {
                        Content::Png { data, width, height } => Some((data, width, height)),
                        _ => None,
                    })
                    .ok_or_else(|| error("render", "The engine returned no page image"))?;
                let output = self.directory.path().join("preview.png");
                let path = output.to_str().ok_or_else(|| error("storage", "The workspace path is not UTF-8"))?;
                self.automation.write_output(path, image.0).map_err(|_| error("storage", "Cannot store the page preview"))?;
                let warnings: Vec<&Value> = content
                    .iter()
                    .filter_map(|part| match part {
                        Content::Json(value) => Some(value),
                        _ => None,
                    })
                    .collect();
                Ok(json!({ "path": path, "width": image.1, "height": image.2, "warnings": warnings }).to_string())
            }
            Operation::Rotate(page) => {
                self.tool("page_rotate", json!({ "doc": self.id()?, "pages": [page], "degrees": 90 }))?;
                self.metadata()
            }
            Operation::Save => {
                if self.copies >= 8 {
                    return Err(error("limit", "Close and reopen the document before creating more than eight export copies"));
                }
                let next_copy = self.copies + 1;
                let output = self.directory.path().join(format!("copy-{next_copy}.pdf"));
                let path = output.to_str().ok_or_else(|| error("storage", "The workspace path is not UTF-8"))?;
                self.tool("doc_save", json!({ "doc": self.id()?, "path": path, "full": false }))?;
                self.copies = next_copy;
                Ok(json!({ "path": path }).to_string())
            }
            Operation::Close(discard) => {
                if let Some(id) = self.document {
                    self.tool("doc_close", json!({ "doc": id, "discard_changes": discard }))?;
                    self.document = None;
                }
                Ok("{}".into())
            }
        }
    }
}
