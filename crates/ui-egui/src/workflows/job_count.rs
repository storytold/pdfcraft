//! The progress of a background job and the user's request to stop it, shared between the job's
//! thread and the app's frame.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use craft::host::queue::Cancel;

/// Items done and total of a running job, and its cancel request. The job calls [`set`](Self::set)
/// as it goes (and wakes the app, since the queue's own wake fires only when a job finishes); the
/// frame takes each change once with [`take_change`](Self::take_change). The cancel is the count's
/// and not the queue's: a queue job that is cancelled delivers nothing, while a cancelled run keeps
/// what it has read.
#[derive(Debug, Default)]
pub struct JobCount {
    done: AtomicUsize,
    total: AtomicUsize,
    changed: AtomicBool,
    cancel: Cancel,
}

impl JobCount {
    /// Records that `done` of `total` items are finished.
    pub fn set(&self, done: usize, total: usize) {
        self.total.store(total, Ordering::Relaxed);
        self.done.store(done, Ordering::Relaxed);
        self.changed.store(true, Ordering::Release);
    }

    /// The items done and the total, as last set.
    pub fn read(&self) -> (usize, usize) {
        (self.done.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed))
    }

    /// The count if it was set since the last call, so progress is posted when it moves and not
    /// every frame.
    pub fn take_change(&self) -> Option<(usize, usize)> {
        self.changed.swap(false, Ordering::Acquire).then(|| self.read())
    }

    /// Asks the job to stop at its next step.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Whether the job was asked to stop.
    pub fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}
