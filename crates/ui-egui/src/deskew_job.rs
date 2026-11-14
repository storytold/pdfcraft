//! Scan & OCR ▸ Straighten all pages: the run's state. Measuring and rotating happen on a worker
//! thread (they are the whole cost), and the pages are put back one per frame so the dialog can
//! show progress while it runs.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, TryRecvError};

use pdfcraft_engine::deskew::{DeskewInputs, Deskewed, Prepared};
use pdfcraft_engine::{DocId, Session};

enum Phase {
    /// A worker thread is measuring the pages.
    Measuring(Receiver<Vec<Prepared>>),
    /// The pages are measured; each frame puts one back.
    Applying(VecDeque<Prepared>),
}

/// One batch deskew in flight.
pub struct Job {
    id: DocId,
    total: usize,
    phase: Phase,
    applied: usize,
    fixed: Vec<(usize, f64)>,
    straight: usize,
    unmeasured: Vec<usize>,
    skipped: Vec<(usize, String)>,
    failed: Vec<(usize, String)>,
}

impl Job {
    /// Start measuring `inputs` on a worker thread, so the window keeps painting while it runs.
    pub fn new(id: DocId, inputs: DeskewInputs, total: usize) -> Self {
        // `std::thread::spawn` is not available on the web: measure here instead, and let the
        // dialog apply the pages one per frame as usual.
        #[cfg(target_arch = "wasm32")]
        let rx = {
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = tx.send(inputs.prepare(1));
            rx
        };
        #[cfg(not(target_arch = "wasm32"))]
        let rx = {
            let (tx, rx) = std::sync::mpsc::channel();
            let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
            std::thread::spawn(move || {
                let _ = tx.send(inputs.prepare(threads));
            });
            rx
        };
        Self {
            id,
            total,
            phase: Phase::Measuring(rx),
            applied: 0,
            fixed: Vec::new(),
            straight: 0,
            unmeasured: Vec::new(),
            skipped: Vec::new(),
            failed: Vec::new(),
        }
    }

    /// Poll the worker, then put one measured page back.
    pub fn step(&mut self, session: &mut Session) {
        if let Phase::Measuring(rx) = &self.phase {
            match rx.try_recv() {
                Ok(prepared) => self.phase = Phase::Applying(prepared.into()),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    // The worker died: finish the run rather than wait for a result that will
                    // never come, so the dialog can close.
                    for page in self.applied..self.total {
                        self.failed.push((page + 1, "the measuring thread stopped".into()));
                    }
                    self.applied = self.total;
                    self.phase = Phase::Applying(VecDeque::new());
                    return;
                }
            }
        }
        let Phase::Applying(ready) = &mut self.phase else { return };
        let Some(prepared) = ready.pop_front() else { return };
        let page = prepared.page;
        let error = prepared.error.clone();
        match session.apply_prepared(self.id, prepared) {
            Ok(Deskewed::Straightened(angle)) => self.fixed.push((page + 1, angle)),
            Ok(Deskewed::AlreadyStraight) => self.straight += 1,
            Ok(Deskewed::Unmeasured) => match error {
                Some(why) => self.skipped.push((page + 1, why)),
                None => self.unmeasured.push(page + 1),
            },
            Err(e) => self.failed.push((page + 1, e.to_string())),
        }
        self.applied += 1;
    }

    pub fn done(&self) -> bool {
        matches!(&self.phase, Phase::Applying(ready) if ready.is_empty() && self.applied >= self.total)
    }

    /// What the dialog shows: progress while it runs, then what was done.
    pub fn text(&self) -> String {
        let mut text = if matches!(self.phase, Phase::Measuring(_)) {
            format!("Measuring {total} page(s)…\n", total = self.total)
        } else if !self.done() {
            format!("Straightening page {} of {total}…\n", (self.applied + 1).min(self.total), total = self.total)
        } else {
            format!(
                "Straightened {fixed} of {total} page(s); {straight} already straight.\n",
                fixed = self.fixed.len(),
                total = self.total,
                straight = self.straight
            )
        };
        if !self.fixed.is_empty() {
            text.push_str("\nStraightened:\n");
            for (page, angle) in &self.fixed {
                text.push_str(&format!("  page {page}: {angle:.2}°\n"));
            }
        }
        if !self.unmeasured.is_empty() {
            text.push_str(&format!(
                "\nCheck by hand ({} page(s) where the skew could not be judged: full-page photographs, drawings or blank pages):\n",
                self.unmeasured.len()
            ));
            text.push_str(&format!("  {}\n", self.unmeasured.iter().map(usize::to_string).collect::<Vec<_>>().join(", ")));
        }
        if !self.skipped.is_empty() {
            text.push_str(&format!("\nLeft alone ({} page(s)):\n", self.skipped.len()));
            for (page, why) in &self.skipped {
                text.push_str(&format!("  page {page}: {why}\n"));
            }
        }
        if !self.failed.is_empty() {
            text.push_str(&format!("\nCould not be read ({} page(s)):\n", self.failed.len()));
            for (page, why) in &self.failed {
                text.push_str(&format!("  page {page}: {why}\n"));
            }
        }
        text
    }
}
