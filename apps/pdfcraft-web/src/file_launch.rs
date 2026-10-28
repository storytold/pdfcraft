//! Read browser-granted PWA file launches into the existing background startup inbox.
//!
//! These are intake limits, not a promise about the renderer's or browser's total heap.
//! Launch handles are transient read inputs; they never become paths or write-back targets.

#![cfg(any(target_arch = "wasm32", test))]

use std::{collections::VecDeque, sync::Mutex};

const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
const MAX_QUEUED_BYTES: usize = 128 * 1024 * 1024;
const MAX_PENDING_HANDLES: usize = 16;
const MAX_QUEUED_FILES: usize = 8;
const MAX_NAME_CHARS: usize = 160;
const MAX_FAILURE_NOTICES: usize = 8;
const LAUNCH_NAME: &str = "PDF file launch";
const OVERFLOW_NOTICE: &str = "PWA file launch: additional files could not be opened. Open fewer or smaller files and try again.";
const PENDING_LIMIT: &str = "Too many files are waiting to open. Wait for the current files and try again.";
const STARTUP_LIMIT: &str = "The browser startup queue is full (8 files or 128 MiB). Finish the current opening and try again.";
const FILE_LIMIT: &str = "This file exceeds the browser file-launch limit of 64 MiB.";

/// Validate the browser-reported size before materializing or copying its file bytes.
/// The accepted value must be finite, integral, nonnegative, and within the per-file intake limit.
fn checked_file_size(size: f64) -> Result<usize, &'static str> {
    if !size.is_finite() || size < 0.0 || size.fract() != 0.0 {
        return Err("The browser returned an invalid file size.");
    }
    if size > MAX_FILE_BYTES as f64 {
        return Err(FILE_LIMIT);
    }
    // The finite integral value is now within a 64 MiB bound, including on wasm32.
    Ok(size as usize)
}

/// Check that the browser buffer length matches the previously checked file size.
/// This runs before constructing a Rust byte buffer and does not bound total browser or renderer memory.
fn checked_buffer_size(expected: usize, actual: u32) -> Result<usize, &'static str> {
    let actual = usize::try_from(actual).map_err(|_| FILE_LIMIT)?;
    if expected > MAX_FILE_BYTES || actual > MAX_FILE_BYTES {
        return Err(FILE_LIMIT);
    }
    if actual != expected {
        return Err("The browser returned incomplete or inconsistent file bytes. Try opening the file again.");
    }
    Ok(actual)
}

/// Check the existing startup inbox together with one additional incoming file.
/// The incoming item consumes a queue slot even when its size is zero.
fn check_startup_budget(sizes: impl IntoIterator<Item = usize>, incoming: usize) -> Result<(), &'static str> {
    if incoming > MAX_FILE_BYTES {
        return Err(FILE_LIMIT);
    }
    let mut count = 1_usize;
    let mut bytes = incoming;
    for size in sizes {
        count = count.checked_add(1).ok_or(STARTUP_LIMIT)?;
        bytes = bytes.checked_add(size).ok_or(STARTUP_LIMIT)?;
        if count > MAX_QUEUED_FILES || bytes > MAX_QUEUED_BYTES {
            return Err(STARTUP_LIMIT);
        }
    }
    Ok(())
}

/// Check the queued handles, any handle already being read, and the full incoming batch.
/// Checked arithmetic makes overflow a refusal rather than allowing the pending-handle cap to wrap.
fn check_pending_budget(queued: usize, in_flight: bool, incoming: usize) -> Result<(), &'static str> {
    let total = queued.checked_add(usize::from(in_flight)).and_then(|n| n.checked_add(incoming)).ok_or(PENDING_LIMIT)?;
    if total > MAX_PENDING_HANDLES { Err(PENDING_LIMIT) } else { Ok(()) }
}

/// Exactly one reader owns the returned item until `finish` returns its successor.
struct Pending<T> {
    queued: VecDeque<T>,
    in_flight: bool,
}

impl<T> Pending<T> {
    /// Create an empty queue with no reader holding an item.
    fn new() -> Self {
        Self { queued: VecDeque::new(), in_flight: false }
    }

    /// Check whether an entire new batch fits alongside queued and in-flight handles.
    fn check_batch(&self, count: usize) -> Result<(), &'static str> {
        check_pending_budget(self.queued.len(), self.in_flight, count)
    }

    /// Admit the whole batch or leave the logical queue and reader state unchanged.
    /// When no reader is active, return its first item and mark it in flight; otherwise enqueue all items.
    fn admit(&mut self, batch: Vec<T>) -> Result<Option<T>, &'static str> {
        self.check_batch(batch.len())?;
        self.queued.try_reserve(batch.len()).map_err(|_| "Not enough browser memory to queue these files.")?;
        let mut items = batch.into_iter();
        let first = if self.in_flight { None } else { items.next() };
        if first.is_some() {
            self.in_flight = true;
        }
        self.queued.extend(items);
        Ok(first)
    }

    /// Finish the current read and transfer ownership of the next queued item to its reader.
    fn finish(&mut self) -> Option<T> {
        let next = self.queued.pop_front();
        self.in_flight = next.is_some();
        next
    }
}

/// Make a bounded, control-character-safe display or download name.
/// This display string is not a path and must not be used as a file access target.
fn display_name(name: &str) -> String {
    let mut chars = name.chars();
    let mut short: String = chars.by_ref().take(MAX_NAME_CHARS).map(|c| if c.is_control() { ' ' } else { c }).collect();
    if chars.next().is_some() {
        // Preserve the PDF suffix for the document/download name without treating it as a path.
        let pdf = name.rsplit_once('.').is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("pdf"));
        let suffix = if pdf { "....pdf" } else { "..." };
        short = short.chars().take(MAX_NAME_CHARS - suffix.len()).collect();
        short.push_str(suffix);
    }
    if short.trim().is_empty() { "document.pdf".to_string() } else { short }
}

/// Append a bounded launch notice while preserving errors from other producers.
/// Once the pending notice list has eight entries, add at most one launch overflow summary.
fn record_failure(failures: &mut Vec<(String, String)>, name: &str, error: &'static str) {
    if failures.len() < MAX_FAILURE_NOTICES {
        failures.push((display_name(name), error.to_string()));
    } else if !failures.iter().any(|(name, error)| name == LAUNCH_NAME && error == OVERFLOW_NOTICE) {
        // At most one extra summary can be added while this failure batch remains queued.
        failures.push((LAUNCH_NAME.to_string(), OVERFLOW_NOTICE.to_string()));
    }
}

/// Record a failure and recover the complete notice list if a prior holder poisoned its mutex.
/// Clear poison while holding the recovered guard so the regular UI drain can read the list.
fn queue_failure(failed: &Mutex<Vec<(String, String)>>, name: &str, error: &'static str) {
    let mut notices = match failed.lock() {
        Ok(notices) => notices,
        Err(poisoned) => {
            // This queue has no invariant beyond owning complete Rust strings. Recover all
            // earlier notices, then clear poison while holding its guard so the UI's normal
            // lock-and-drain path can actually display this failure and the earlier errors.
            let notices = poisoned.into_inner();
            failed.clear_poison();
            notices
        }
    };
    record_failure(&mut notices, name, error);
}

#[cfg(target_arch = "wasm32")]
pub use browser::register;

#[cfg(target_arch = "wasm32")]
mod browser {
    use super::{LAUNCH_NAME, Pending, check_startup_budget, checked_buffer_size, checked_file_size, display_name, queue_failure};
    use eframe::wasm_bindgen::{JsCast, JsValue, closure::Closure};
    use pdfcraft_ui_egui::{FailedInbox, Inbox};
    use std::{cell::RefCell, rc::Rc};
    use wasm_bindgen_futures::JsFuture;
    use web_sys::FileSystemFileHandle;

    struct Intake {
        pending: RefCell<Pending<FileSystemFileHandle>>,
        inbox: Inbox,
        failed: FailedInbox,
        ctx: egui::Context,
    }

    impl Intake {
        /// Report a failed launch to the console and bounded notice queue, then wake the UI.
        fn report(&self, name: &str, error: &'static str) {
            eframe::web_sys::console::error_1(&format!("PdfCraft: could not open {}: {error}", display_name(name)).into());
            queue_failure(&self.failed, name, error);
            self.ctx.request_repaint();
        }

        /// Check whether one incoming file fits the existing startup inbox limits.
        /// This does not reserve a slot; enqueue checks again after browser reads.
        fn check_inbox(&self, incoming: usize) -> Result<(), &'static str> {
            let inbox =
                self.inbox.lock().map_err(|_| "The document-open queue is unavailable. Save your current work, reopen the app, and try again.")?;
            check_startup_budget(inbox.iter().map(|(_, bytes)| bytes.len()), incoming)
        }
    }

    /// Register once during app creation; browsers without the launch API keep ordinary startup behavior.
    /// The registered consumer stays live for the current document.
    pub fn register(inbox: Inbox, failed: FailedInbox, ctx: egui::Context) {
        let Some(window) = web_sys::window() else { return };
        let state = Rc::new(Intake { pending: RefCell::new(Pending::new()), inbox, failed, ctx });
        let queue = match js_sys::Reflect::get(window.as_ref(), &JsValue::from_str("launchQueue")) {
            Ok(queue) if queue.is_null() || queue.is_undefined() => return,
            Ok(queue) => queue,
            Err(_) => {
                state.report(LAUNCH_NAME, "The browser file-launch API could not be inspected.");
                return;
            }
        };
        let setter = match js_sys::Reflect::get(&queue, &JsValue::from_str("setConsumer")).and_then(|value| value.dyn_into::<js_sys::Function>()) {
            Ok(setter) => setter,
            Err(_) => {
                state.report(LAUNCH_NAME, "The browser file-launch API is unavailable or incompatible.");
                return;
            }
        };
        let consumer_state = Rc::clone(&state);
        let consumer = Closure::<dyn FnMut(JsValue)>::new(move |params: JsValue| {
            if let Err(error) = consume(&consumer_state, params) {
                consumer_state.report(LAUNCH_NAME, error);
            }
        });
        if setter.call1(&queue, consumer.as_ref()).is_ok() {
            // The browser owns this consumer for the lifetime of this document, like the
            // existing host bridge and beforeunload listeners. Registration happens once.
            consumer.forget();
        } else {
            state.report(LAUNCH_NAME, "The browser could not register PDF file launches. Try reopening the app.");
        }
    }

    /// Synchronously validate and queue one browser callback batch, then start at most one reader.
    /// No browser read is awaited in the callback, so later callbacks append behind the active reader.
    fn consume(state: &Rc<Intake>, params: JsValue) -> Result<(), &'static str> {
        let files = js_sys::Reflect::get(&params, &JsValue::from_str("files")).map_err(|_| "The browser returned an invalid file launch.")?;
        if files.is_undefined() || files.is_null() {
            return Ok(());
        }
        let files = files.dyn_into::<js_sys::Array>().map_err(|_| "The browser returned an invalid launch file list.")?;
        let length = files.length();
        let count = usize::try_from(length).map_err(|_| super::PENDING_LIMIT)?;
        // FrozenArray is a real Array. Reject an oversized batch before collecting handles.
        state.pending.try_borrow().map_err(|_| "The file-launch queue is busy. Try again.")?.check_batch(count)?;
        let mut handles = Vec::new();
        handles.try_reserve_exact(count).map_err(|_| "Not enough browser memory to queue these files.")?;
        for index in 0..length {
            match files.get(index).dyn_into::<FileSystemFileHandle>() {
                Ok(handle) => handles.push(handle),
                Err(_) => state.report(LAUNCH_NAME, "A launched item was not a readable file. Folders are not supported."),
            }
        }
        let first = {
            let mut pending = state.pending.try_borrow_mut().map_err(|_| "The file-launch queue is busy. Try again.")?;
            pending.admit(handles)?
        };
        if let Some(first) = first {
            wasm_bindgen_futures::spawn_local(drain(Rc::clone(state), first));
        }
        Ok(())
    }

    /// Read handles serially and hand each completed or failed attempt to the next queued handle.
    /// Browser-owned read promises have no cancellation or timeout here, so a stalled read also stalls later handles.
    async fn drain(state: Rc<Intake>, first: FileSystemFileHandle) {
        let mut next = Some(first);
        while let Some(handle) = next {
            let name = display_name(&handle.name());
            if let Err(error) = read_and_queue(&state, &handle, &name).await {
                state.report(&name, error);
            }
            drop(handle);
            // No RefCell or mutex guard survives an await or a browser callback.
            let result = state.pending.try_borrow_mut().map(|mut pending| pending.finish());
            match result {
                Ok(item) => next = item,
                Err(_) => {
                    state.report(LAUNCH_NAME, "The file-launch queue is unavailable. Reopen the app and try again.");
                    return;
                }
            }
        }
    }

    /// Copy one bounded browser file into the startup inbox as owned bytes.
    /// The handle is read-only input, never a path or write-back target; saving uses the existing browser or host flow.
    /// The whole ArrayBuffer is copied, and intake limits do not bound total browser or renderer memory.
    async fn read_and_queue(state: &Intake, handle: &FileSystemFileHandle, name: &str) -> Result<(), &'static str> {
        state.check_inbox(0)?;
        let file = JsFuture::from(handle.get_file())
            .await
            .map_err(|_| "The browser could not read this file. It may have moved or access was denied.")?
            .dyn_into::<web_sys::File>()
            .map_err(|_| "The browser did not return a file.")?;
        let size = checked_file_size(file.size())?;
        state.check_inbox(size)?;
        let buffer = JsFuture::from(file.array_buffer())
            .await
            .map_err(|_| "The browser could not read the file bytes. Try opening the file again.")?
            .dyn_into::<js_sys::ArrayBuffer>()
            .map_err(|_| "The browser did not return file bytes.")?;
        // A real Blob's result has the already-bounded snapshot size. Check again before
        // constructing the view or allocating Rust memory, using stable u32 JS lengths.
        let size = checked_buffer_size(size, buffer.byte_length())?;
        state.check_inbox(size)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(size).map_err(|_| "Not enough browser memory to open this file.")?;
        bytes.resize(size, 0);
        let view = js_sys::Uint8Array::new(buffer.as_ref());
        // The view covers exactly this checked ArrayBuffer; no await occurs before copying.
        view.copy_to(&mut bytes);
        let mut inbox =
            state.inbox.lock().map_err(|_| "The document-open queue is unavailable. Save your current work, reopen the app, and try again.")?;
        check_startup_budget(inbox.iter().map(|(_, bytes)| bytes.len()), bytes.len())?;
        inbox.try_reserve(1).map_err(|_| "Not enough browser memory to queue this file.")?;
        inbox.push((name.to_string(), bytes));
        drop(inbox);
        state.ctx.request_repaint();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FILE_LIMIT, LAUNCH_NAME, MAX_FAILURE_NOTICES, MAX_FILE_BYTES, MAX_NAME_CHARS, MAX_PENDING_HANDLES, MAX_QUEUED_BYTES, MAX_QUEUED_FILES,
        OVERFLOW_NOTICE, Pending, check_pending_budget, check_startup_budget, checked_buffer_size, checked_file_size, display_name, record_failure,
    };

    /// Rejects invalid browser sizes and buffers that exceed or disagree with the checked size.
    #[test]
    fn file_and_buffer_sizes_are_integral_bounded_and_consistent() {
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 0.5, f64::MAX] {
            assert!(checked_file_size(invalid).is_err(), "{invalid}");
        }
        assert_eq!(checked_file_size(0.0), Ok(0));
        assert_eq!(checked_file_size(MAX_FILE_BYTES as f64), Ok(MAX_FILE_BYTES));
        assert_eq!(checked_file_size(MAX_FILE_BYTES as f64 + 1.0), Err(FILE_LIMIT));
        assert_eq!(checked_buffer_size(0, 0), Ok(0));
        assert_eq!(checked_buffer_size(MAX_FILE_BYTES, MAX_FILE_BYTES as u32), Ok(MAX_FILE_BYTES));
        assert!(checked_buffer_size(12, 11).is_err());
        assert!(checked_buffer_size(12, 13).is_err());
        assert!(checked_buffer_size(usize::MAX, u32::MAX).is_err());
        assert!(checked_buffer_size(0, u32::MAX).is_err());
    }

    /// Counts existing inbox entries toward file and byte limits and refuses overflow.
    #[test]
    fn queued_bytes_and_files_include_existing_entries_without_overflow() {
        assert_eq!(check_startup_budget([], MAX_FILE_BYTES), Ok(()));
        assert_eq!(check_startup_budget([MAX_FILE_BYTES], MAX_FILE_BYTES), Ok(()));
        assert!(check_startup_budget([MAX_FILE_BYTES, 1], MAX_FILE_BYTES).is_err());
        assert!(check_startup_budget([usize::MAX], 1).is_err());
        assert!(check_startup_budget([], MAX_FILE_BYTES + 1).is_err());
        assert_eq!(check_startup_budget([0; MAX_QUEUED_FILES - 1], 0), Ok(()));
        assert!(check_startup_budget([0; MAX_QUEUED_FILES], 0).is_err());
        // Existing URL/host inputs may be larger than this producer's per-file limit.
        assert_eq!(check_startup_budget([MAX_QUEUED_BYTES], 0), Ok(()));
        assert!(check_startup_budget([MAX_QUEUED_BYTES], 1).is_err());
    }

    /// Counts queued, in-flight, and incoming handles together without arithmetic wraparound.
    #[test]
    fn pending_accounting_includes_inflight_and_rejects_overflow() {
        assert_eq!(check_pending_budget(0, false, MAX_PENDING_HANDLES), Ok(()));
        assert_eq!(check_pending_budget(MAX_PENDING_HANDLES - 1, true, 0), Ok(()));
        assert!(check_pending_budget(MAX_PENDING_HANDLES - 1, true, 1).is_err());
        assert!(check_pending_budget(usize::MAX, true, 0).is_err());
        assert!(check_pending_budget(1, false, usize::MAX).is_err());
    }

    /// Preserves FIFO order when callbacks arrive during a read, including failed reads.
    #[test]
    fn one_reader_preserves_fifo_across_callbacks_failure_completion_and_restart() {
        let mut pending = Pending::new();
        assert_eq!(pending.admit(vec!['A', 'B']).unwrap(), Some('A'));
        assert_eq!(pending.admit(vec!['C', 'D']).unwrap(), None, "a second callback starts no reader");
        assert_eq!(pending.finish(), Some('B'), "completion is the same for a failed or successful read");
        assert_eq!(pending.finish(), Some('C'));
        assert_eq!(pending.finish(), Some('D'));
        assert_eq!(pending.finish(), None);
        assert_eq!(pending.admit(Vec::new()).unwrap(), None);
        assert_eq!(pending.admit(vec!['E']).unwrap(), Some('E'));
        assert_eq!(pending.finish(), None);
    }

    /// Refuses an over-limit batch without dropping items already admitted to the queue.
    #[test]
    fn a_refused_pending_batch_keeps_all_prior_items() {
        let mut pending = Pending::new();
        assert_eq!(pending.admit((0..MAX_PENDING_HANDLES).collect()).unwrap(), Some(0));
        assert!(pending.admit(vec![MAX_PENDING_HANDLES]).is_err());
        for expected in 1..MAX_PENDING_HANDLES {
            assert_eq!(pending.finish(), Some(expected));
        }
        assert_eq!(pending.finish(), None);
    }

    /// Bounds launch notices while retaining notices produced by other intake paths.
    #[test]
    fn launch_notices_are_bounded_without_replacing_other_producers() {
        let foreign = ("URL.pdf".to_string(), "HTTP 404".to_string());
        let mut failed = vec![foreign.clone()];
        for _ in 0..100 {
            record_failure(&mut failed, "failed.pdf", "Read failed.");
        }
        assert_eq!(failed.first(), Some(&foreign));
        assert_eq!(failed.len(), MAX_FAILURE_NOTICES + 1);
        assert_eq!(failed.last(), Some(&(LAUNCH_NAME.to_string(), OVERFLOW_NOTICE.to_string())));
        let original: Vec<_> = (0..20).map(|i| (format!("URL-{i}.pdf"), "HTTP 404".to_string())).collect();
        let mut failed = original.clone();
        record_failure(&mut failed, "failed.pdf", "Read failed.");
        record_failure(&mut failed, "another.pdf", "Read failed.");
        assert_eq!(failed.len(), original.len() + 1);
        assert!(failed.starts_with(&original));
        failed.clear();
        record_failure(&mut failed, "later.pdf", "Read failed.");
        assert_eq!(failed, vec![("later.pdf".to_string(), "Read failed.".to_string())]);
    }

    #[cfg(not(target_arch = "wasm32"))]
    /// Recovers queued notices after mutex poisoning so the normal UI drain can read them.
    #[test]
    fn recovered_failure_queue_is_readable_and_keeps_existing_errors() {
        let existing = ("URL.pdf".to_string(), "HTTP 404".to_string());
        let failed = std::sync::Mutex::new(vec![existing.clone()]);
        let interrupted = std::panic::catch_unwind(|| {
            let _guard = failed.lock().unwrap();
            panic!("simulate interruption while the notice queue is locked");
        });
        assert!(interrupted.is_err());
        assert!(failed.is_poisoned());
        super::queue_failure(&failed, "launched.pdf", "Read failed.");
        assert!(!failed.is_poisoned(), "the UI's ordinary lock-and-drain path can read the notice");
        assert_eq!(*failed.lock().unwrap(), vec![existing, ("launched.pdf".to_string(), "Read failed.".to_string())]);
    }

    /// Checks Unicode limits, control normalization, PDF suffixes, and the empty-name fallback.
    #[test]
    fn display_names_are_unicode_bounded_and_preserve_the_pdf_suffix() {
        assert_eq!(display_name("ordinary.pdf"), "ordinary.pdf");
        assert_eq!(display_name("\n\r"), "document.pdf");
        assert_eq!(display_name("a\nb.pdf"), "a b.pdf");
        let long = format!("{}.PDF", "界".repeat(MAX_NAME_CHARS + 1));
        let bounded = display_name(&long);
        assert_eq!(bounded.chars().count(), MAX_NAME_CHARS);
        assert!(bounded.ends_with("....pdf"));
        assert!(display_name(&"x".repeat(MAX_NAME_CHARS + 1)).ends_with("..."));
        assert_eq!(display_name("").as_str(), "document.pdf");
    }
}
