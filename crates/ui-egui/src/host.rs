//! Web host mode (`?host=parent`): saves into the page that embeds PdfCraft.
//!
//! The web app hands each save to the page ([`crate::HostSaveFn`]). By default a save counts as
//! done once it is handed over, as it always has. A page that sends `pdfcraft:init` with
//! `acknowledgeSaves: true` promises to answer every save, and then a document stays unsaved,
//! and closing it after Save waits, until the page says it stored the file (#1792): an answer
//! that never comes, or a failure, can't lose the user's work. `HostSaves` is that protocol's
//! state, kept apart from the browser so it can be tested anywhere. The messages are described
//! for embedders in `packaging/web/README.md`.

use std::sync::{Arc, Mutex, PoisonError};

use pdfcraft_engine::DocId;

use crate::PdfCraftApp;

/// How long a save waits for the page's answer when `pdfcraft:init` sets no `saveTimeoutMs`.
pub const DEFAULT_SAVE_TIMEOUT_SECS: f64 = 120.0;
/// The shortest and longest wait a page may set.
const SAVE_TIMEOUT_RANGE_SECS: (f64, f64) = (1.0, 3600.0);
/// Saves that may wait for an answer at once. Each keeps its file in memory until then.
pub const MAX_WAITING_SAVES: usize = 8;
/// The most of a page's error text shown in the notice.
const MAX_ERROR_CHARS: usize = 300;

/// A save handed to the embedding page.
pub struct HostSave<'a> {
    /// The document's file name.
    pub name: &'a str,
    /// The PDF to store.
    pub bytes: &'a [u8],
    /// Save As rather than Save: the user wants to choose where it goes.
    pub save_as: bool,
    /// Set when the page answers saves (`pdfcraft:init` with `acknowledgeSaves`). It goes out
    /// as `saveId`, and the page's `pdfcraft:saved` or `pdfcraft:save-failed` names it.
    pub save_id: Option<&'a str>,
}

/// A message from the embedding page, as the web app read it.
#[derive(Clone, Debug, PartialEq)]
pub enum HostMessage {
    /// `pdfcraft:init`. `acknowledge_saves`: the page answers every save. `save_timeout_ms`:
    /// how long to wait for that answer.
    Init { acknowledge_saves: bool, save_timeout_ms: Option<f64> },
    /// `pdfcraft:saved`: the page stored the file of save `save_id`.
    Saved { save_id: String },
    /// `pdfcraft:save-failed`: the page could not store it; `error` says why.
    SaveFailed { save_id: String, error: String },
}

/// Messages from the embedding page, queued by the web app and read every frame.
pub type HostInbox = Arc<Mutex<Vec<HostMessage>>>;

/// No room for another save that waits for the page ([`MAX_WAITING_SAVES`]).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Busy;

/// The saves that wait for the embedding page's answer. `T` is what the app needs once it comes.
pub(crate) struct HostSaves<T> {
    /// The page answers saves (`pdfcraft:init`).
    acknowledged: bool,
    timeout_secs: f64,
    /// Starts every id, so an answer meant for an earlier copy of the page (before a reload)
    /// never matches a save of this one.
    prefix: String,
    last_id: u64,
    waiting: Vec<Waiting<T>>,
}

struct Waiting<T> {
    id: String,
    /// Set when the save is first seen by [`HostSaves::expire`].
    deadline: Option<f64>,
    payload: T,
}

impl<T> Default for HostSaves<T> {
    fn default() -> Self {
        Self { acknowledged: false, timeout_secs: DEFAULT_SAVE_TIMEOUT_SECS, prefix: String::new(), last_id: 0, waiting: Vec::new() }
    }
}

impl<T> HostSaves<T> {
    /// `pdfcraft:init`: whether the page answers saves, and how long to wait (milliseconds;
    /// missing or unusable means the default, and it is kept within a second and an hour).
    /// Saves already waiting keep waiting for their answer.
    pub(crate) fn configure(&mut self, acknowledge_saves: bool, save_timeout_ms: Option<f64>) {
        self.acknowledged = acknowledge_saves;
        let (shortest, longest) = SAVE_TIMEOUT_RANGE_SECS;
        self.timeout_secs = save_timeout_ms
            .map(|ms| ms / 1000.0)
            .filter(|secs| secs.is_finite() && *secs > 0.0)
            .map_or(DEFAULT_SAVE_TIMEOUT_SECS, |secs| secs.clamp(shortest, longest));
    }

    /// Start every save id with `prefix` (at most 64 characters).
    pub(crate) fn set_prefix(&mut self, prefix: &str) {
        self.prefix = prefix.chars().take(64).collect();
    }

    /// An id for the next save: `None` when the page doesn't answer saves, so the save counts as
    /// soon as it is handed over; [`Busy`] when [`MAX_WAITING_SAVES`] are waiting already.
    pub(crate) fn start(&mut self) -> Result<Option<String>, Busy> {
        if !self.acknowledged {
            return Ok(None);
        }
        if self.waiting.len() >= MAX_WAITING_SAVES {
            return Err(Busy);
        }
        self.last_id = self.last_id.wrapping_add(1);
        Ok(Some(format!("{}{}", self.prefix, self.last_id)))
    }

    /// Save `id` was handed to the page: `payload` waits for the answer.
    pub(crate) fn wait(&mut self, id: String, payload: T) {
        self.waiting.push(Waiting { id, deadline: None, payload });
    }

    /// The save an answer is about, and the answer: `Err` holds the page's reason. `None` for
    /// [`HostMessage::Init`] and for an id that nothing waits for (another save's, one already
    /// answered or timed out, or made up), which changes nothing.
    pub(crate) fn answer(&mut self, message: &HostMessage) -> Option<(T, Result<(), String>)> {
        let (id, result) = match message {
            HostMessage::Saved { save_id } => (save_id, Ok(())),
            HostMessage::SaveFailed { save_id, error } => (save_id, Err(error.clone())),
            HostMessage::Init { .. } => return None,
        };
        let at = self.waiting.iter().position(|w| &w.id == id)?;
        Some((self.waiting.remove(at).payload, result))
    }

    /// The saves whose answer did not come in time, at `now` (seconds). A save's time starts
    /// at the first call after it was handed over.
    pub(crate) fn expire(&mut self, now: f64) -> Vec<T> {
        let timeout = self.timeout_secs;
        for w in &mut self.waiting {
            w.deadline.get_or_insert(now + timeout);
        }
        let (late, on_time): (Vec<_>, Vec<_>) = std::mem::take(&mut self.waiting).into_iter().partition(|w| w.deadline.is_some_and(|d| now >= d));
        self.waiting = on_time;
        late.into_iter().map(|w| w.payload).collect()
    }

    /// Seconds from `now` to the next deadline, if a save waits.
    pub(crate) fn next_deadline_in(&self, now: f64) -> Option<f64> {
        self.waiting.iter().map(|w| w.deadline.map_or(self.timeout_secs, |d| d - now)).reduce(f64::min).map(|s| s.max(0.0))
    }
}

/// A save the page has yet to answer.
pub(crate) struct WaitingSave {
    pub(crate) id: DocId,
    /// The document's edit generation the file was made from.
    pub(crate) generation: u64,
    pub(crate) bytes: Arc<Vec<u8>>,
    pub(crate) name: String,
    /// What runs once the page has the file (close the tab, quit).
    pub(crate) after: Box<dyn FnOnce(&mut PdfCraftApp) + Send>,
}

impl PdfCraftApp {
    /// Web host mode: start the ids of this page's saves with `prefix`, unique to this copy of
    /// the page, so an answer meant for an earlier copy (before a reload) never matches.
    pub fn set_host_save_prefix(&mut self, prefix: &str) {
        self.host_saves.set_prefix(prefix);
    }

    /// Read what the embedding page sent, give up on saves it did not answer in time, and come
    /// back for the next deadline.
    pub(crate) fn poll_host(&mut self, ctx: &egui::Context, now: f64) {
        self.host_messages(now);
        if let Some(secs) = self.host_saves.next_deadline_in(now) {
            ctx.request_repaint_after(std::time::Duration::try_from_secs_f64(secs).unwrap_or_default());
        }
    }

    /// [`Self::poll_host`] without the repaint: at `now` (seconds). The settings come first, then
    /// the saves whose time is up, then the answers: an answer read on the frame its save times
    /// out comes too late, like any later one.
    pub(crate) fn host_messages(&mut self, now: f64) {
        let messages = std::mem::take(&mut *self.host_inbox.lock().unwrap_or_else(PoisonError::into_inner));
        let (settings, answers): (Vec<_>, Vec<_>) = messages.into_iter().partition(|m| matches!(m, HostMessage::Init { .. }));
        for message in settings {
            if let HostMessage::Init { acknowledge_saves, save_timeout_ms } = message {
                self.host_saves.configure(acknowledge_saves, save_timeout_ms);
            }
        }
        for save in self.host_saves.expire(now) {
            self.host_not_stored(&save, tl!("the website did not confirm that it stored the file"));
        }
        for message in answers {
            if let Some((save, answer)) = self.host_saves.answer(&message) {
                match answer {
                    Ok(()) => self.host_stored(save),
                    Err(error) if error.trim().is_empty() => self.host_not_stored(&save, tl!("the website did not confirm that it stored the file")),
                    Err(error) => self.host_not_stored(&save, &error),
                }
            }
        }
    }

    /// The page has the file of `save`. The document counts as saved only if it is still the
    /// version the file was made from: edits made meanwhile stay unsaved, and so does typing
    /// not committed yet, and then `after` (closing the tab) does not run.
    fn host_stored(&mut self, save: WaitingSave) {
        let WaitingSave { id, generation, bytes, after, .. } = save;
        if self.session.get(id).map(|d| d.edit_generation()) != Some(generation) {
            return;
        }
        self.host_has(id, bytes);
        if let Some(i) = self.views.iter().position(|v| v.id == id)
            && !self.has_unsaved_work(i)
        {
            after(self);
        }
    }

    /// The page did not store the file of `save`: the work stays unsaved, `after` does not run,
    /// and the notice says why. Nothing to say about a document that was closed meanwhile: the
    /// user discarded it, or an earlier save of it was stored.
    fn host_not_stored(&mut self, save: &WaitingSave, error: &str) {
        if self.session.get(save.id).is_none() {
            return;
        }
        let error: String = error.chars().take(MAX_ERROR_CHARS).collect();
        self.notify_fmt("Couldn't save {name}: {e}", &[("name", &save.name), ("e", &error)]);
    }

    /// Document `id` is saved as `bytes` in the embedding page.
    pub(crate) fn host_has(&mut self, id: DocId, bytes: Arc<Vec<u8>>) {
        let _ = self.session.mark_saved(id, bytes, None);
        if let Some(doc) = self.session.get(id)
            && let Some(view) = self.views.iter_mut().find(|v| v.id == id)
        {
            view.document_changed(&doc.info);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CloseRequest, SaveTarget};

    fn answering() -> HostSaves<&'static str> {
        let mut saves = HostSaves::default();
        saves.configure(true, None);
        saves
    }

    fn saved(id: &str) -> HostMessage {
        HostMessage::Saved { save_id: id.to_string() }
    }

    #[test]
    fn a_page_that_does_not_answer_saves_gets_no_save_ids() {
        let mut saves = HostSaves::<()>::default();
        assert_eq!(saves.start(), Ok(None), "the original protocol: no init, no ids");
        saves.configure(false, Some(5000.0));
        assert_eq!(saves.start(), Ok(None), "an init without acknowledgeSaves changes nothing");
    }

    #[test]
    fn each_save_waits_for_its_own_answer() {
        let mut saves = answering();
        saves.set_prefix("p7-");
        let first = saves.start().unwrap().unwrap();
        let second = saves.start().unwrap().unwrap();
        assert_ne!(first, second);
        assert!(first.starts_with("p7-") && second.starts_with("p7-"), "{first} {second}");
        saves.wait(first.clone(), "first");
        saves.wait(second.clone(), "second");
        assert_eq!(saves.answer(&saved(&second)), Some(("second", Ok(()))));
        assert_eq!(saves.answer(&saved(&second)), None, "an answer counts once");
        assert_eq!(saves.answer(&saved(&first)), Some(("first", Ok(()))));
    }

    #[test]
    fn an_answer_for_a_save_nobody_waits_for_is_ignored() {
        let mut saves = answering();
        let id = saves.start().unwrap().unwrap();
        saves.wait(id.clone(), "doc");
        assert_eq!(saves.answer(&saved(&format!("{id}0"))), None);
        assert_eq!(saves.answer(&HostMessage::SaveFailed { save_id: "elsewhere".into(), error: "no".into() }), None);
        assert_eq!(saves.answer(&HostMessage::Init { acknowledge_saves: true, save_timeout_ms: None }), None);
        assert_eq!(saves.answer(&saved(&id)), Some(("doc", Ok(()))), "the save still waits for its own answer");
    }

    #[test]
    fn a_failure_answer_brings_the_page_reason() {
        let mut saves = answering();
        let id = saves.start().unwrap().unwrap();
        saves.wait(id.clone(), "doc");
        let failed = HostMessage::SaveFailed { save_id: id, error: "Quota exceeded".into() };
        assert_eq!(saves.answer(&failed), Some(("doc", Err("Quota exceeded".to_string()))));
        assert_eq!(saves.expire(1e9), Vec::<&str>::new(), "nothing waits any more");
    }

    #[test]
    fn a_save_the_page_never_answers_times_out() {
        let mut saves = answering();
        saves.configure(true, Some(5000.0));
        let id = saves.start().unwrap().unwrap();
        saves.wait(id.clone(), "doc");
        assert!(saves.expire(10.0).is_empty(), "the time starts now");
        assert_eq!(saves.next_deadline_in(12.0), Some(3.0));
        assert!(saves.expire(14.9).is_empty());
        assert_eq!(saves.expire(15.0), vec!["doc"]);
        assert_eq!(saves.answer(&saved(&id)), None, "a late answer finds nothing to confirm");
        assert_eq!(saves.next_deadline_in(16.0), None);
    }

    #[test]
    fn the_page_wait_is_kept_within_bounds() {
        let mut saves = HostSaves::<&str>::default();
        let timeout = |saves: &mut HostSaves<&str>, ms: Option<f64>| {
            saves.configure(true, ms);
            saves.timeout_secs
        };
        assert_eq!(timeout(&mut saves, None), DEFAULT_SAVE_TIMEOUT_SECS);
        assert_eq!(timeout(&mut saves, Some(f64::NAN)), DEFAULT_SAVE_TIMEOUT_SECS);
        assert_eq!(timeout(&mut saves, Some(-5.0)), DEFAULT_SAVE_TIMEOUT_SECS);
        assert_eq!(timeout(&mut saves, Some(0.0)), DEFAULT_SAVE_TIMEOUT_SECS);
        assert_eq!(timeout(&mut saves, Some(10.0)), 1.0);
        assert_eq!(timeout(&mut saves, Some(f64::INFINITY)), DEFAULT_SAVE_TIMEOUT_SECS);
        assert_eq!(timeout(&mut saves, Some(1e15)), 3600.0);
        assert_eq!(timeout(&mut saves, Some(30_000.0)), 30.0);
    }

    #[test]
    fn only_so_many_saves_wait_at_once() {
        let mut saves = answering();
        for _ in 0..MAX_WAITING_SAVES {
            let id = saves.start().unwrap().unwrap();
            saves.wait(id, "doc");
        }
        assert_eq!(saves.start(), Err(Busy));
        assert_eq!(saves.expire(0.0).len(), 0);
        assert_eq!(saves.expire(DEFAULT_SAVE_TIMEOUT_SECS).len(), MAX_WAITING_SAVES);
        assert!(matches!(saves.start(), Ok(Some(_))), "room again once they are answered or late");
    }

    // The app with a host page: what a save does to the document, as the web build runs it.

    type Sent = Arc<Mutex<Vec<(String, Option<String>)>>>;

    /// A one-page document with a proper xref table.
    fn one_page_pdf() -> Vec<u8> {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
            "<< /Type /Page /Parent 2 0 R >>",
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    /// An edited document in an app whose host page records each save as `(name, saveId)`, and
    /// has sent `pdfcraft:init` with `acknowledgeSaves` when `answers` is set.
    fn hosted(answers: bool) -> (PdfCraftApp, Sent) {
        let sent = Sent::default();
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let sink = sent.clone();
        app.host_save = Some(Box::new(move |save: HostSave<'_>| {
            assert!(save.bytes.starts_with(b"%PDF"));
            sink.lock().unwrap().push((save.name.to_string(), save.save_id.map(str::to_string)));
            Ok(())
        }));
        if answers {
            tell(&mut app, HostMessage::Init { acknowledge_saves: true, save_timeout_ms: Some(30_000.0) }, 0.0);
        }
        app.open_bytes("Quote.pdf", None, one_page_pdf()).unwrap();
        assert!(app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
        assert!(app.has_unsaved_work(0));
        (app, sent)
    }

    /// The page sends `message`; the app reads it on the frame at `now`.
    fn tell(app: &mut PdfCraftApp, message: HostMessage, now: f64) {
        app.host_inbox.lock().unwrap().push(message);
        app.host_messages(now);
    }

    fn last_id(sent: &Sent) -> String {
        sent.lock().unwrap().last().and_then(|(_, id)| id.clone()).expect("the save carries an id")
    }

    fn notice(app: &PdfCraftApp) -> String {
        app.toast.as_ref().map(|(text, _)| text.clone()).unwrap_or_default()
    }

    #[test]
    fn without_the_opt_in_a_save_counts_once_it_is_handed_over() {
        let (mut app, sent) = hosted(false);
        assert!(app.save_active(SaveTarget::InPlace), "saved at once, as before");
        assert_eq!(*sent.lock().unwrap(), vec![("Quote.pdf".to_string(), None)], "no saveId for a page that didn't ask");
        assert!(!app.has_unsaved_work(0));
        app.host_messages(1e6);
        assert!(!app.has_unsaved_work(0), "nothing waits, nothing times out");
    }

    #[test]
    fn with_the_opt_in_the_work_stays_unsaved_until_the_page_has_it() {
        let (mut app, sent) = hosted(true);
        assert!(!app.save_active(SaveTarget::InPlace), "not saved yet: the page has to answer");
        let id = last_id(&sent);
        app.host_messages(1.0);
        assert!(app.has_unsaved_work(0), "handed over is not stored");
        tell(&mut app, saved(&id), 2.0);
        assert!(!app.has_unsaved_work(0), "stored");
    }

    #[test]
    fn a_failed_save_keeps_the_work_unsaved_and_says_why() {
        let (mut app, sent) = hosted(true);
        app.save_active(SaveTarget::InPlace);
        let id = last_id(&sent);
        tell(&mut app, HostMessage::SaveFailed { save_id: id.clone(), error: "Quota exceeded".into() }, 1.0);
        assert!(app.has_unsaved_work(0));
        assert!(notice(&app).contains("Couldn't save Quote.pdf: Quota exceeded"), "{}", notice(&app));
        tell(&mut app, saved(&id), 2.0);
        assert!(app.has_unsaved_work(0), "that save is over: a later answer for it changes nothing");
    }

    #[test]
    fn a_failure_without_a_reason_still_says_the_file_was_not_stored() {
        let (mut app, sent) = hosted(true);
        app.save_active(SaveTarget::InPlace);
        tell(&mut app, HostMessage::SaveFailed { save_id: last_id(&sent), error: " ".into() }, 1.0);
        assert_eq!(notice(&app), "Couldn't save Quote.pdf: the website did not confirm that it stored the file");
        assert!(app.has_unsaved_work(0));
    }

    #[test]
    fn an_answer_for_another_save_changes_nothing() {
        let (mut app, sent) = hosted(true);
        app.save_active(SaveTarget::InPlace);
        let id = last_id(&sent);
        tell(&mut app, saved(&format!("{id}-other")), 1.0);
        tell(&mut app, saved(""), 1.0);
        assert!(app.has_unsaved_work(0));
        tell(&mut app, saved(&id), 2.0);
        assert!(!app.has_unsaved_work(0));
    }

    #[test]
    fn a_save_the_page_never_answers_fails_after_the_wait() {
        let (mut app, sent) = hosted(true);
        app.save_active(SaveTarget::InPlace);
        let id = last_id(&sent);
        app.host_messages(10.0);
        app.host_messages(39.9);
        assert!(app.has_unsaved_work(0));
        assert_eq!(notice(&app), "");
        app.host_messages(40.0);
        assert!(app.has_unsaved_work(0));
        assert!(notice(&app).starts_with("Couldn't save Quote.pdf"), "{}", notice(&app));
        tell(&mut app, saved(&id), 41.0);
        assert!(app.has_unsaved_work(0), "too late to count");
    }

    #[test]
    fn an_answer_read_on_the_frame_its_save_times_out_comes_too_late() {
        let (mut app, sent) = hosted(true);
        app.save_active(SaveTarget::InPlace);
        let id = last_id(&sent);
        app.host_messages(10.0);
        tell(&mut app, saved(&id), 40.0);
        assert!(app.has_unsaved_work(0));
        assert!(notice(&app).starts_with("Couldn't save Quote.pdf"), "{}", notice(&app));
    }

    #[test]
    fn a_save_that_starts_waiting_asks_for_the_frame_that_times_it() {
        let (mut app, _sent) = hosted(true);
        let ctx = egui::Context::default();
        // Use up the context's first repaint demand, so only the save can ask for another.
        for _ in 0..2 {
            let _ = ctx.run_logic(&egui::RawInput::default(), |_| {});
        }
        assert!(!ctx.has_requested_repaint());
        app.ctx = Some(ctx.clone());
        assert!(!app.save_active(SaveTarget::InPlace));
        assert!(ctx.has_requested_repaint(), "the next frame starts the save's timeout");
    }

    #[test]
    fn edits_made_while_the_page_saves_stay_unsaved() {
        let (mut app, sent) = hosted(true);
        app.save_active(SaveTarget::InPlace);
        let id = last_id(&sent);
        assert!(app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
        tell(&mut app, saved(&id), 1.0);
        assert!(app.has_unsaved_work(0), "the page has the version before the last edit");
    }

    /// Answer the "save changes?" prompt for the first tab with Save.
    fn save_and_close(app: &mut PdfCraftApp) {
        app.request_close_tab(0);
        assert!(matches!(app.close_request, Some(CloseRequest::Tab(_))), "asks first");
        app.resolve_close(&egui::Context::default(), Some(true));
    }

    #[test]
    fn closing_after_save_waits_for_the_page() {
        let (mut app, sent) = hosted(true);
        save_and_close(&mut app);
        assert_eq!(app.views.len(), 1, "still open while the page stores the file");
        let id = last_id(&sent);
        app.host_messages(1.0);
        assert_eq!(app.views.len(), 1);
        tell(&mut app, saved(&id), 2.0);
        assert!(app.views.is_empty(), "closed once stored");
    }

    #[test]
    fn closing_after_save_does_not_happen_when_the_page_fails() {
        let (mut app, sent) = hosted(true);
        save_and_close(&mut app);
        tell(&mut app, HostMessage::SaveFailed { save_id: last_id(&sent), error: "Disk full".into() }, 1.0);
        assert_eq!(app.views.len(), 1, "the document stays open");
        assert!(app.has_unsaved_work(0));
    }

    #[test]
    fn closing_after_save_does_not_happen_after_newer_edits() {
        let (mut app, sent) = hosted(true);
        save_and_close(&mut app);
        assert!(app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
        tell(&mut app, saved(&last_id(&sent)), 1.0);
        assert_eq!(app.views.len(), 1, "closing would lose the newer edit");
        assert!(app.has_unsaved_work(0));
    }

    #[test]
    fn a_document_discarded_while_its_save_waits_gets_no_notice() {
        let (mut app, sent) = hosted(true);
        app.save_active(SaveTarget::InPlace);
        app.request_close_tab(0);
        app.resolve_close(&egui::Context::default(), Some(false));
        assert!(app.views.is_empty(), "discarded");
        let before = notice(&app);
        tell(&mut app, HostMessage::SaveFailed { save_id: last_id(&sent), error: "Disk full".into() }, 1.0);
        assert_eq!(notice(&app), before, "nothing left to save");
    }

    #[test]
    fn without_the_opt_in_closing_after_save_closes_at_once() {
        let (mut app, _) = hosted(false);
        save_and_close(&mut app);
        assert!(app.views.is_empty(), "as before");
    }
}
