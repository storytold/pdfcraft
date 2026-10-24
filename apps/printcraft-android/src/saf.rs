#![cfg(target_os = "android")]

//! The Storage Access Framework as PrintCraft's file dialogs.
//!
//! PrintCraft's engine reads and writes real file paths, while Android hands out `content://`
//! URIs, so the Java side ([`MainActivity`] + `SafBridge`, under `android/`) copies a picked
//! document into the app's own storage and reports the copy's path back through a manifest file —
//! no bulk data crosses JNI. Saving works the same way in reverse: the engine writes a scratch
//! file in the app's export directory and a worker thread copies the finished file to wherever the
//! user pointed `ACTION_CREATE_DOCUMENT`.
//!
//! This module implements [`Host`], the dialog seam in `pdfcraft-ui-egui` (`src/fd.rs`), which
//! PrintCraft's own dialogs call. Two properties matter:
//!
//! - **The UI thread never blocks on a picker.** An open runs on the worker thread that polls the
//!   future `Pickers` created (`PdfCraftApp::pick`); a save answers at once with the scratch path
//!   and the destination is settled in the background.
//! - **A picker result arrives on a later frame.** The picked paths go back to PrintCraft's own
//!   asynchronous picker (`pickers::Pickers`), which opens them with `PdfCraftApp::open_path` on a
//!   later frame — the desktop path, with the paths coming from the SAF instead of `rfd`.
//!
//! Everything here is safe Rust except the two JNI boundary helpers, which carry `// SAFETY:`
//! notes; the workspace forbids `unsafe` everywhere else.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use jni::objects::{JObject, JValue};
use jni::{JavaVM, jni_sig, jni_str};
use pdfcraft_ui_egui::fd::{DialogRequest, Host};

use crate::{Notices, wake};

/// The manifest the Java side writes; one copied path per line.
const MANIFEST: &str = "import-manifest.txt";
/// How long a picker may stay open before the shell gives up on it.
const PICK_TIMEOUT: Duration = Duration::from_secs(300);
/// How long the "where should this be saved?" picker may stay open.
const SAVE_TIMEOUT: Duration = Duration::from_secs(300);
/// How long to wait for the engine to write the scratch file.
const WRITE_TIMEOUT: Duration = Duration::from_secs(900);

/// The Android file dialogs: the Storage Access Framework through the Java bridge.
pub struct SafHost {
    /// `AndroidApp::activity_as_ptr()` — the Java activity object.
    activity: usize,
    /// The app's private directory (`filesDir`), where picked documents are copied.
    data_dir: PathBuf,
    /// Where files being saved are written before they are published.
    exports: PathBuf,
    /// Notices for the UI thread.
    notices: Notices,
}

impl SafHost {
    pub fn new(activity: usize, data_dir: PathBuf, exports: Option<PathBuf>, notices: Notices) -> Self {
        Self { activity, data_dir: data_dir.clone(), exports: exports.unwrap_or(data_dir), notices }
    }
}

impl Host for SafHost {
    fn open(&self, request: &DialogRequest, multiple: bool) -> Vec<PathBuf> {
        let mimes = mimes_for(request);
        let manifest = self.data_dir.join(MANIFEST);
        let _ = std::fs::remove_file(&manifest);
        log::info!("SAF: opening the picker (types {mimes}, multiple {multiple}, title {:?})", request.title.as_str());
        let picked = call(self.activity, |env, activity| {
            let mimes = env.new_string(&mimes)?;
            env.call_method(
                activity,
                jni_str!("safPickDocuments"),
                jni_sig!("(Ljava/lang/String;Z)V"),
                &[JValue::Object(&mimes), JValue::Bool(multiple)],
            )?;
            let deadline = Instant::now() + PICK_TIMEOUT;
            loop {
                if env.call_method(activity, jni_str!("safIsDone"), jni_sig!("()Z"), &[])?.z().unwrap_or(false) {
                    return Ok(true);
                }
                if Instant::now() > deadline {
                    log::warn!("SAF: picker timed out after {PICK_TIMEOUT:?}");
                    return Ok(false);
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        });
        match picked {
            Some(true) => read_manifest(&manifest),
            _ => Vec::new(),
        }
    }

    fn save(&self, request: &DialogRequest) -> Option<PathBuf> {
        let name = leaf_name(&request.file_name);
        let scratch = self.exports.join(&name);
        let title = if request.title.is_empty() { "Save as".to_owned() } else { request.title.clone() };
        let mime = mime_for_name(&name);
        log::info!("SAF: save dialog → scratch {} (suggested {name}, type {mime})", scratch.display());
        let (activity, scratch_for_thread, notices) = (self.activity, scratch.clone(), self.notices.clone());
        std::thread::spawn(move || {
            if !pick_save(activity, &title, &name, &mime) {
                log::info!("SAF: no destination chosen; the file stays at {}", scratch_for_thread.display());
                notify(&notices, "Save cancelled: the file stayed in the app's folder".to_owned());
                return;
            }
            match wait_for_file(&scratch_for_thread) {
                Some(size) => {
                    if publish(activity, &scratch_for_thread) {
                        log::info!("SAF: published {} ({size} bytes) to the chosen destination", scratch_for_thread.display());
                        notify(&notices, format!("Saved {name} ({size} bytes)"));
                    } else {
                        log::warn!("SAF: publishing {} failed; the file stays in the export directory", scratch_for_thread.display());
                        notify(&notices, format!("Could not copy {name} to the chosen destination"));
                    }
                }
                None => {
                    log::warn!("SAF: {} was never written; nothing to publish", scratch_for_thread.display());
                    notify(&notices, format!("{name} was not written"));
                }
            }
        });
        Some(scratch)
    }

    fn folder(&self) -> Option<PathBuf> {
        // The SAF tree picker hands back a document tree the engine cannot write to by path, so
        // folder questions (exports, split results, OCR output) get the app's own directory.
        Some(self.exports.clone())
    }
}

/// Queue a notice for the UI thread and wake it, so the toast appears without waiting for a touch.
fn notify(notices: &Notices, text: String) {
    lock(notices).push(text);
    wake();
}

/// A `Mutex` that survives a panicking neighbour.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Attach the current thread, look up the activity object and run `body`.
#[allow(unsafe_code)]
fn call<T>(activity: usize, body: impl FnOnce(&mut jni::Env<'_>, &JObject<'_>) -> jni::errors::Result<T>) -> Option<T> {
    let ctx = ndk_context::android_context();
    // SAFETY: `ndk_context` is set up by android-activity before `android_main` runs and stays valid
    // for the life of the process; `from_raw` only wraps the pointer in the jni crate's type (it
    // caches the JVM, which the JVM itself keeps alive for the process).
    let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) };
    vm.attach_current_thread(|env| -> jni::errors::Result<T> {
        // SAFETY: the pointer is `AndroidApp::activity_as_ptr()`, a reference to the running
        // activity that is valid for the whole process; `JObject::from_raw` borrows it for this JNI
        // frame.
        let activity = unsafe { JObject::from_raw(env, activity as *mut _) };
        body(env, &activity)
    })
    .ok()
}

/// Ask where the file should go (`ACTION_CREATE_DOCUMENT`); true when a destination was chosen.
#[allow(unsafe_code)]
fn pick_save(activity: usize, title: &str, name: &str, mime: &str) -> bool {
    let result = call(activity, |env, activity| {
        let title = env.new_string(title)?;
        let name = env.new_string(name)?;
        let mime = env.new_string(mime)?;
        env.call_method(
            activity,
            jni_str!("safPickSave"),
            jni_sig!("(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V"),
            &[JValue::Object(&title), JValue::Object(&name), JValue::Object(&mime)],
        )?;
        let deadline = Instant::now() + SAVE_TIMEOUT;
        loop {
            if env.call_method(activity, jni_str!("safSaveDone"), jni_sig!("()Z"), &[])?.z().unwrap_or(false) {
                return env.call_method(activity, jni_str!("safHasSaveUri"), jni_sig!("()Z"), &[])?.z();
            }
            if Instant::now() > deadline {
                log::warn!("SAF: save picker timed out after {SAVE_TIMEOUT:?}");
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    });
    result.unwrap_or(false)
}

/// Copy a finished file to the destination the user chose.
#[allow(unsafe_code)]
fn publish(activity: usize, path: &Path) -> bool {
    let result = call(activity, |env, activity| {
        let path = env.new_string(path.to_string_lossy())?;
        env.call_method(activity, jni_str!("safPublish"), jni_sig!("(Ljava/lang/String;)Z"), &[JValue::Object(&path)])?.z()
    });
    result.unwrap_or(false)
}

/// Wait until the engine has written `path` and its size has stopped changing. Returns the size.
fn wait_for_file(path: &Path) -> Option<u64> {
    let deadline = Instant::now() + WRITE_TIMEOUT;
    let (mut size, mut stable_since) = (0u64, Instant::now());
    loop {
        let now = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if now != size {
            size = now;
            stable_since = Instant::now();
        } else if size > 0 && stable_since.elapsed() > Duration::from_millis(1500) {
            return Some(size);
        }
        if Instant::now() > deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}

fn read_manifest(path: &Path) -> Vec<PathBuf> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let paths: Vec<PathBuf> = text.lines().map(str::trim).filter(|l| !l.is_empty()).map(PathBuf::from).collect();
            log::info!("SAF: {} document(s) picked", paths.len());
            paths
        }
        Err(e) => {
            log::info!("SAF: nothing picked ({e})");
            Vec::new()
        }
    }
}

/// A safe file name for the scratch file: no directory parts, always something to write.
fn leaf_name(suggested: &str) -> String {
    let leaf = suggested.rsplit(['/', '\\']).next().unwrap_or(suggested).trim();
    if leaf.is_empty() { "document.pdf".to_owned() } else { leaf.to_owned() }
}

/// The `|`-separated MIME types to offer the picker for a dialog's filters.
fn mimes_for(request: &DialogRequest) -> String {
    let mut types: Vec<String> = Vec::new();
    for (_, extensions) in &request.filters {
        for extension in extensions {
            let mime = mime_for_name(extension);
            if !types.contains(&mime) {
                types.push(mime);
            }
        }
    }
    if types.is_empty() {
        // PrintCraft is a PDF application: without a filter, offer PDFs plus the images its own
        // Create ▸ PDF from file accepts.
        types.push("application/pdf".to_owned());
        types.push("image/*".to_owned());
    }
    types.join("|")
}

/// The MIME type for a file name or bare extension.
fn mime_for_name(name: &str) -> String {
    let extension = name.rsplit('.').next().unwrap_or(name).to_ascii_lowercase();
    match extension.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "tif" | "tiff" => "image/tiff",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "jp2" | "j2k" | "jpx" => "image/jp2",
        "txt" | "text" => "text/plain",
        "csv" => "text/csv",
        "xml" => "application/xml",
        "xfdf" => "application/vnd.adobe.xfdf",
        "fdf" => "application/vnd.fdf",
        "p12" | "pfx" => "application/x-pkcs12",
        _ => "*/*",
    }
    .to_owned()
}
