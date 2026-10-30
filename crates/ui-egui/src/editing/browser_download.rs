//! Browser download handoff and bounded, delayed object-URL cleanup.
//!
//! Clicking an anchor never confirms persistence. The browser can refuse or cancel the
//! download without reporting it, so neither dispatch nor cleanup acknowledges a save.

use std::{cell::Cell, rc::Rc};

const MAX_DOWNLOADS: usize = 8;
const MAX_DOWNLOAD_BYTES: usize = 128 * 1024 * 1024;
#[cfg(target_arch = "wasm32")]
const URL_LIFETIME_MS: i32 = 60_000;

#[derive(Default)]
struct Budget {
    count: Cell<usize>,
    bytes: Cell<usize>,
}

impl Budget {
    /// Reserve before allocating JavaScript buffers. A full budget refuses new work rather
    /// than revoking a URL the browser might still need. Timers delayed in a background tab
    /// retain their reservations, so suspension cannot bypass the bound.
    fn reserve(self: &Rc<Self>, bytes: usize) -> Result<Reservation, String> {
        if self.count.get() >= MAX_DOWNLOADS {
            return Err("Too many browser downloads are pending. Wait a minute and try again.".into());
        }
        let total = self
            .bytes
            .get()
            .checked_add(bytes)
            .filter(|total| *total <= MAX_DOWNLOAD_BYTES)
            .ok_or("Browser downloads can retain at most 128 MiB. Wait a minute and retry, or use the desktop app for larger files.")?;
        self.count.set(self.count.get() + 1); // Bounded by MAX_DOWNLOADS above.
        self.bytes.set(total);
        Ok(Reservation { budget: self.clone(), bytes })
    }
}

struct Reservation {
    budget: Rc<Budget>,
    bytes: usize,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.count.set(self.budget.count.get().saturating_sub(1));
        self.budget.bytes.set(self.budget.bytes.get().saturating_sub(self.bytes));
    }
}

/// Browser exceptions belong in a short notice, not an implementation stack trace.
fn error_text(message: &str) -> String {
    let line = message.lines().next().map(str::trim).filter(|line| !line.is_empty()).unwrap_or("The browser couldn't start the download.");
    let mut chars = line.chars();
    let mut text: String = chars.by_ref().take(240).collect();
    if chars.next().is_some() {
        text.push('…');
    }
    text
}

#[cfg(target_arch = "wasm32")]
fn js_error(e: wasm_bindgen::JsValue) -> String {
    // Read message instead of Debug: browser Error's debug form includes its whole WASM stack.
    // Reflect also handles DOMException and cross-realm errors without instanceof assumptions.
    let message = e
        .as_string()
        .or_else(|| js_sys::Reflect::get(&e, &wasm_bindgen::JsValue::from_str("message")).ok().and_then(|message| message.as_string()))
        .unwrap_or_default();
    error_text(&message)
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    static BUDGET: Rc<Budget> = Rc::default();
}

#[cfg(target_arch = "wasm32")]
struct DownloadUrl {
    url: String,
    _reservation: Reservation,
}

#[cfg(target_arch = "wasm32")]
impl Drop for DownloadUrl {
    fn drop(&mut self) {
        let _ = web_sys::Url::revoke_object_url(&self.url);
    }
}

/// Request a download, retaining its URL for a minute so the browser can read the Blob.
/// This is a bounded handoff grace period, not evidence that a download finished.
#[cfg(target_arch = "wasm32")]
pub(crate) fn download(name: &str, bytes: &[u8]) -> Result<(), String> {
    use wasm_bindgen::{JsCast, closure::Closure};
    let err = js_error;
    let window = web_sys::window().ok_or("no browser window")?;
    let document = window.document().ok_or("no document")?;
    let body = document.body().ok_or("no document body")?;
    let a: web_sys::HtmlAnchorElement = document.create_element("a").map_err(err)?.dyn_into().map_err(|_| "no download anchor")?;
    a.set_attribute("hidden", "").map_err(err)?;
    a.set_download(name);

    let reservation = BUDGET.with(|budget| budget.reserve(bytes.len()))?;
    // The budget bounds retained Blobs plus this call's transient Uint8Array/Blob copies.
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::of1(&array);
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("application/pdf");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(err)?;
    let download = DownloadUrl { url: web_sys::Url::create_object_url_with_blob(&blob).map_err(err)?, _reservation: reservation };
    a.set_href(&download.url);
    body.append_child(&a).map_err(err)?;

    // The callback owns its Closure through a small self-reference, broken when it fires.
    // Unlike forgetting a Closure, this also frees the callback wrapper on every cleanup.
    let owner: Rc<Cell<Option<Closure<dyn FnMut()>>>> = Rc::new(Cell::new(None));
    let callback_owner = owner.clone();
    // Only Cell::take, dropping a callback/guard and the caught URL-revocation API run here.
    // These cannot panic, so the Cell captures are unwind safe despite lacking that auto trait.
    let cleanup = Closure::once_assert_unwind_safe(move || {
        drop(callback_owner.take());
        drop(download);
    });
    // Failure drops cleanup before a self-reference exists, revoking the URL and releasing
    // its reservation immediately. Register cleanup before dispatching the download.
    if let Err(e) = window.set_timeout_with_callback_and_timeout_and_arguments_0(cleanup.as_ref().unchecked_ref(), URL_LIFETIME_MS) {
        a.remove();
        return Err(err(e));
    }
    owner.set(Some(cleanup));
    // A suspended timer retains its URL, callback and reservation; count/bytes stay bounded.
    a.click();
    a.remove();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_download_error_omits_stack_lines() {
        assert_eq!(error_text("cleanup timer unavailable\n    at window.setTimeout (wasm:123)"), "cleanup timer unavailable");
    }

    #[test]
    fn browser_download_error_bounds_unicode_without_cutting_characters() {
        assert_eq!(error_text(&"界".repeat(241)), format!("{}…", "界".repeat(240)));
    }

    #[test]
    fn browser_download_error_preserves_the_actual_reason() {
        assert_eq!(error_text("The download was refused."), "The download was refused.");
    }

    #[test]
    fn browser_download_error_has_a_reason_for_empty_or_whitespace_exceptions() {
        for message in ["", "   ", "\nstack only"] {
            assert_eq!(error_text(message), "The browser couldn't start the download.");
        }
    }

    #[test]
    fn download_count_bound_retains_existing_reservations_until_cleanup() {
        let budget = Rc::new(Budget::default());
        let mut held = (0..MAX_DOWNLOADS).map(|_| budget.reserve(1).unwrap()).collect::<Vec<_>>();
        assert!(budget.reserve(1).is_err());
        assert_eq!((budget.count.get(), budget.bytes.get()), (MAX_DOWNLOADS, MAX_DOWNLOADS));
        held.pop();
        let replacement = budget.reserve(2).unwrap();
        assert_eq!(budget.bytes.get(), MAX_DOWNLOADS + 1);
        drop((replacement, held));
        assert_eq!((budget.count.get(), budget.bytes.get()), (0, 0));
    }

    #[test]
    fn download_byte_bound_includes_all_pending_files_and_rejects_overflow() {
        let budget = Rc::new(Budget::default());
        assert!(budget.reserve(usize::MAX).is_err());
        assert!(budget.reserve(MAX_DOWNLOAD_BYTES + 1).is_err());
        assert_eq!((budget.count.get(), budget.bytes.get()), (0, 0));
        let first = budget.reserve(MAX_DOWNLOAD_BYTES - 1).unwrap();
        let last_byte = budget.reserve(1).unwrap();
        assert!(budget.reserve(1).is_err());
        assert!(budget.reserve(usize::MAX).is_err());
        assert_eq!(budget.bytes.get(), MAX_DOWNLOAD_BYTES);
        drop((first, last_byte));
        assert_eq!((budget.count.get(), budget.bytes.get()), (0, 0));
    }

    #[test]
    fn download_setup_failure_releases_its_reservation_without_touching_other_downloads() {
        let budget = Rc::new(Budget::default());
        let retained = budget.reserve(10).unwrap();
        let prepare = || -> Result<(), &'static str> {
            let _setup = budget.reserve(20).unwrap();
            Err("browser setup failed")
        };
        assert!(prepare().is_err());
        assert_eq!((budget.count.get(), budget.bytes.get()), (1, 10));
        drop(retained);
        assert_eq!((budget.count.get(), budget.bytes.get()), (0, 0));
    }
}
