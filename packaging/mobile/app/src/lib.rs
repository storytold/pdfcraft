//! The webview never receives a filesystem capability or an arbitrary path command.
//! Only native picker results and OS-delivered document URLs can be read.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

mod documents;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
};
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, FilePath};

#[derive(Default)]
struct State {
    incoming: Mutex<VecDeque<FilePath>>,
    diagnostics: Mutex<VecDeque<String>>,
    recoveries: Mutex<BTreeMap<String, std::path::PathBuf>>,
    recoverable: Mutex<BTreeSet<std::path::PathBuf>>,
    dialog: AtomicBool,
    tickets: Mutex<BTreeMap<u32, FilePath>>,
    next_ticket: AtomicU32,
}

struct DialogClaim<'a>(&'a AtomicBool);
impl Drop for DialogClaim<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
fn claim(state: &State) -> Result<DialogClaim<'_>, String> {
    if state.dialog.swap(true, Ordering::SeqCst) {
        return Err("Another file dialog is still open. Finish with it first.".into());
    }
    Ok(DialogClaim(&state.dialog))
}

#[derive(serde::Serialize)]
struct Imported {
    name: String,
    ticket: u32,
    origin: Option<String>,
}

impl State {
    fn issue(&self, file: FilePath) -> Result<Imported, String> {
        let mut tickets = self.tickets.lock().map_err(|_| "The document transfer is unavailable")?;
        if tickets.len() >= 8 {
            return Err("Finish the pending document imports first".into());
        }
        // The ticket-map lock serializes registration and counter updates.
        let id = self.next_ticket.load(Ordering::Relaxed);
        let next = id.checked_add(1).ok_or("Restart PdfCraft to import more documents")?;
        self.next_ticket.store(next, Ordering::Relaxed);
        let name = documents::name(&file);
        let origin = if let Some(path) = file.as_path() {
            if self.recoverable.lock().map_err(|_| "Recovery storage is unavailable")?.contains(path) {
                let token = id.to_string();
                self.recoveries.lock().map_err(|_| "Recovery storage is unavailable")?.insert(token.clone(), path.to_path_buf());
                Some(token)
            } else {
                None
            }
        } else {
            None
        };
        tickets.insert(id, file);
        Ok(Imported { name, ticket: id, origin })
    }
    fn consume(&self, id: u32) -> Result<FilePath, String> {
        self.tickets.lock().map_err(|_| "The document transfer is unavailable")?.remove(&id).ok_or_else(|| "The import ticket has expired".into())
    }
}

fn ticket(app: &tauri::AppHandle, file: FilePath) -> Result<Imported, String> {
    app.state::<State>().issue(file)
}

#[tauri::command]
async fn mobile_open(app: tauri::AppHandle) -> Result<Option<Imported>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<State>();
        let _claim = claim(&state)?;
        let Some(file) = app.dialog().file().add_filter("PDF", &["pdf"]).blocking_pick_file() else { return Ok(None) };
        ticket(&app, file).map(Some)
    })
    .await
    .map_err(|e| format!("The document picker failed: {e}"))?
}

#[tauri::command]
async fn mobile_incoming(app: tauri::AppHandle) -> Result<Option<Imported>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(error) = app.state::<State>().diagnostics.lock().map_err(|_| "Recovery diagnostics are unavailable")?.pop_front() {
            return Err(error);
        }
        let file = app.state::<State>().incoming.lock().map_err(|_| "The incoming-document queue is unavailable")?.pop_front();
        file.map(|file| ticket(&app, file)).transpose()
    })
    .await
    .map_err(|e| format!("The document import failed: {e}"))?
}

#[tauri::command]
async fn mobile_read(app: tauri::AppHandle, ticket: u32) -> Result<tauri::ipc::Response, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = app.state::<State>().consume(ticket)?;
        documents::read(&app, path).map(tauri::ipc::Response::new)
    })
    .await
    .map_err(|e| format!("The document read failed: {e}"))?
}

// Tauri 2.12 emits a never-evaluated `const _: () = if false { unreachable!() }`
// to check borrowed async argument types. Restrict the allowance to that generated
// item; our function body retains the crate's no-panic lint.
#[allow(clippy::unreachable)]
mod save_command {
    use super::*;
    #[deny(clippy::unreachable)]
    #[tauri::command]
    pub(super) async fn mobile_save(app: tauri::AppHandle, request: tauri::ipc::Request<'_>) -> Result<bool, String> {
        let name = request.headers().get("x-pdfcraft-name").and_then(|value| value.to_str().ok()).ok_or("The exported document needs a name")?;
        let name = percent_encoding::percent_decode_str(name).decode_utf8().map_err(|_| "The document name is not UTF-8")?.into_owned();
        let origin = request
            .headers()
            .get("x-pdfcraft-origin")
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| "Invalid document identity")?;
        let bytes = match request.body() {
            tauri::ipc::InvokeBody::Raw(bytes) if bytes.len() <= documents::MAX_DOCUMENT => bytes.clone(),
            _ => return Err("Mobile export needs binary document data of at most 64 MiB".into()),
        };
        tauri::async_runtime::spawn_blocking(move || {
            let state = app.state::<State>();
            let _claim = claim(&state)?;
            documents::export(&app, &name, &bytes, origin.as_deref())
        })
        .await
        .map_err(|e| format!("The document export failed: {e}"))?
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(State::default())
        .setup(|app| {
            if let Err(error) = documents::recover(app.handle())
                && let Ok(mut messages) = app.state::<State>().diagnostics.lock()
            {
                messages.push_back(format!("Could not reopen pending exports: {error}"));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![mobile_open, mobile_incoming, mobile_read, save_command::mobile_save])
        .build(tauri::generate_context!());
    match result {
        Ok(app) => app.run(|_app, _event| {
            #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
            if let tauri::RunEvent::Opened { urls } = _event {
                let state = _app.state::<State>();
                if let Ok(mut queue) = state.incoming.lock() {
                    // Bounded even if another app floods the document-open intent.
                    for url in urls.into_iter().take(8_usize.saturating_sub(queue.len())) {
                        if matches!(url.scheme(), "file" | "content") {
                            queue.push_back(FilePath::Url(url));
                        }
                    }
                }
            }
        }),
        Err(error) => eprintln!("PdfCraft could not start its document host: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identical_names_do_not_grant_recovery_identity_to_an_unrelated_import() {
        let state = State::default();
        let private = std::path::PathBuf::from("private/export-same.pdf");
        state.recoverable.lock().unwrap().insert(private.clone());
        let recovered = state.issue(FilePath::Path(private.clone())).unwrap();
        let external = state.issue(FilePath::Path("selected/export-same.pdf".into())).unwrap();
        assert_eq!(recovered.name, external.name);
        let token = recovered.origin.unwrap();
        assert!(external.origin.is_none());
        assert_eq!(state.recoveries.lock().unwrap().get(&token), Some(&private));
    }
    #[test]
    fn import_tickets_are_one_use_bounded_and_do_not_accept_unknown_paths() {
        let state = State::default();
        assert!(state.consume(123).is_err());
        let first = state.issue(FilePath::Path("selected.pdf".into())).unwrap();
        assert_eq!(first.name, "selected.pdf");
        for _ in 1..8 {
            state.issue(FilePath::Path("selected.pdf".into())).unwrap();
        }
        assert!(state.issue(FilePath::Path("ninth.pdf".into())).is_err());
        assert!(state.consume(first.ticket).is_ok());
        assert!(state.consume(first.ticket).is_err());
        let next = state.issue(FilePath::Path("new.pdf".into())).unwrap();
        assert_ne!(first.ticket, next.ticket);
        state.next_ticket.store(u32::MAX, Ordering::Relaxed);
        state.consume(next.ticket).unwrap();
        assert!(state.issue(FilePath::Path("overflow.pdf".into())).is_err());
    }
    #[test]
    fn native_dialog_claim_is_exclusive_and_released_after_failure() {
        let state = State::default();
        let first = claim(&state).unwrap();
        assert!(claim(&state).is_err());
        drop(first);
        assert!(claim(&state).is_ok());
    }
}
