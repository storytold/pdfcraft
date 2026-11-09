//! Which run a page read by a Web Worker belongs to (the web build's OCR, `ocr_web`).
//!
//! Recognize Text and Recognize Text in Multiple Files can run at once and share the workers, so
//! each answer goes back to the run that sent its page: [`Router::send`] notes the run's
//! [`Inbox`] under the page's id and [`Router::deliver`] puts the answer there. A run that has
//! been dropped (cancelled or finished) gets nothing, and its answers are let go.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

/// Answers by page id.
type Answers<T> = RefCell<Vec<(u64, T)>>;

/// Where one run's answers arrive.
pub struct Inbox<T>(Rc<Answers<T>>);

impl<T> Default for Inbox<T> {
    fn default() -> Self {
        Inbox(Rc::new(RefCell::new(Vec::new())))
    }
}

impl<T> Inbox<T> {
    /// The answers that have arrived since the last call.
    pub fn take(&self) -> Vec<(u64, T)> {
        self.0.try_borrow_mut().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
    }
}

/// Page ids and the run each one belongs to.
pub struct Router<T> {
    next_id: u64,
    owners: HashMap<u64, Weak<Answers<T>>>,
}

impl<T> Default for Router<T> {
    fn default() -> Self {
        Router { next_id: 0, owners: HashMap::new() }
    }
}

impl<T> Router<T> {
    /// Send a page for `inbox`'s run: `post(id)` hands it to a worker under a fresh id (ids are
    /// never reused, so a late answer can't be mistaken for a newer page). The id, or `None` when
    /// `post` couldn't send it.
    pub fn send(&mut self, inbox: &Inbox<T>, post: impl FnOnce(u64) -> bool) -> Option<u64> {
        let id = self.next_id;
        if !post(id) {
            return None;
        }
        self.next_id = self.next_id.wrapping_add(1);
        self.owners.insert(id, Rc::downgrade(&inbox.0));
        Some(id)
    }

    /// The answer for page `id`: to its run's inbox, or dropped when that run is gone or the id
    /// was never sent.
    pub fn deliver(&mut self, id: u64, answer: T) {
        let Some(inbox) = self.owners.remove(&id).and_then(|w| w.upgrade()) else { return };
        if let Ok(mut v) = inbox.try_borrow_mut() {
            v.push((id, answer));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Single-file and multiple-file OCR share the workers: whichever run started first, and
    /// whichever run collects first, each gets exactly its own pages back.
    #[test]
    fn answers_go_to_the_run_that_sent_the_page_in_either_start_order() {
        for single_first in [true, false] {
            let mut r: Router<&str> = Router::default();
            let (single, batch) = (Inbox::default(), Inbox::default());
            let (s, b) = if single_first {
                let s = r.send(&single, |_| true).expect("sent");
                (s, r.send(&batch, |_| true).expect("sent"))
            } else {
                let b = r.send(&batch, |_| true).expect("sent");
                (r.send(&single, |_| true).expect("sent"), b)
            };
            // Both answers arrive in one drain of the workers' events, the batch's first.
            r.deliver(b, "batch page");
            r.deliver(s, "single page");
            // The single run collects first, as pump_web_ocr does; it must not swallow the
            // batch's answer.
            assert_eq!(single.take(), vec![(s, "single page")], "single_first: {single_first}");
            assert_eq!(batch.take(), vec![(b, "batch page")], "single_first: {single_first}");
        }
    }

    /// A cancelled (dropped) run's late answers are let go, the other run still gets its own,
    /// and a run started afterwards gets its pages.
    #[test]
    fn a_cancelled_run_lets_the_other_and_the_next_run_finish() {
        let mut r: Router<&str> = Router::default();
        let (single, batch) = (Inbox::default(), Inbox::default());
        let s = r.send(&single, |_| true).expect("sent");
        let b = r.send(&batch, |_| true).expect("sent");
        drop(single);
        r.deliver(s, "late");
        r.deliver(b, "batch page");
        assert_eq!(batch.take(), vec![(b, "batch page")]);
        assert!(r.owners.is_empty(), "nothing is kept for a run that's gone");

        let next = Inbox::default();
        let n = r.send(&next, |_| true).expect("sent");
        assert_ne!(n, s, "ids are not reused");
        r.deliver(n, "next page");
        assert_eq!(next.take(), vec![(n, "next page")]);
    }

    /// A page that couldn't be posted takes no id and has no owner.
    #[test]
    fn a_page_that_could_not_be_sent_is_not_routed() {
        let mut r: Router<&str> = Router::default();
        let inbox = Inbox::default();
        assert_eq!(r.send(&inbox, |_| false), None);
        assert!(r.owners.is_empty());
        let id = r.send(&inbox, |_| true);
        assert_eq!(id, Some(0), "the id was not used up");
    }

    /// Answers for ids nobody sent, or sent twice, change nothing.
    #[test]
    fn unknown_and_repeated_answers_are_ignored() {
        let mut r: Router<&str> = Router::default();
        let inbox = Inbox::default();
        let id = r.send(&inbox, |_| true).expect("sent");
        r.deliver(id + 100, "stray");
        r.deliver(id, "page");
        r.deliver(id, "again");
        assert_eq!(inbox.take(), vec![(id, "page")]);
        assert!(inbox.take().is_empty());
    }
}
