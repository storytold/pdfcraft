//! W28: the tool-metadata channel of the pointer-emulated tablet input (see
//! `platform_impl/linux/wayland/seat/tablet`).
//!
//! The phase 1 translation reproduces a mouse with every tool kind: its presses are plain
//! `MouseInput` left clicks, and nothing about the event names the tool that made it. The
//! application stack's web (eframe → egui-winit) translates a closed set of winit events, so
//! a tool kind cannot ride those events, and an event of this crate's own would not reach it
//! through them either. Until the native tablet event set (W29) replaces the emulation, the
//! Wayland backend publishes here what pointer-routed applications cannot read out of the
//! pointer events: which tool kind is behind them, and — phase 2 — the pen's pressure,
//! sampled per contact into a bounded buffer. The application asks for both beside the
//! presses it sees; what to draw or erase with stays its decision.
//!
//! The channel is process-wide (a consumer could name no seat on any platform), and counts
//! across seats: contacts of different kinds on different seats at once are reported as
//! [`ContactTool::Mixed`], which makes no eraser claim the shared pointer channel cannot
//! support, and [`sole_contact`] likewise reports nothing while more than one contact is
//! live, because a pointer-routed consumer then cannot tell which contact its frame's
//! presses belong to. The kind attribution stays atomic and lock-free; the per-contact
//! samples sit behind one mutex, written only during Wayland dispatch and read once per
//! frame — the same thread in this crate's eframe users. The backend pairs every contact's
//! start with exactly one end, anchored on the translation machine's own record, so an
//! unpaired end (a compositor forgetting an `up`) can never take a count below zero: ends
//! give back the record they were handed, and a record that does not exist is left alone.
//!
//! Only the Wayland backend writes it; on every other backend [`contact_tool`] is
//! [`ContactTool::None`], [`sole_contact`] is `None`, and every press keeps the selected
//! tool.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use crate::event::Force;

/// How many pressure samples one contact's buffer holds at most, its start included.
///
/// The reports a compositor sends between two application frames (pressure can arrive at a
/// few hundred hertz, stylus reports at up to report-rate) far exceed a frame's worth of
/// samples, so the buffer is a faithful small window of the contact, not a log of it: the
/// overflow policy below keeps what a drawing application reads — the stroke's start and the
/// newest samples — and [`ContactSnapshot::dropped`] says what it did not keep.
pub const SAMPLE_CAPACITY: usize = 64;

/// The tool kind behind the pointer events the current tablet contacts are producing.
///
/// What the application does with `Eraser` is its own decision; this crate only carries what
/// the compositor reported about the tool, unmodified and honest about what it does not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContactTool {
    /// Nothing is in contact; the pointer events (if any) are not a tablet's.
    #[default]
    None,
    /// Every contacting tool was reported eraser-typed.
    Eraser,
    /// Every contacting tool was reported of another kind. Phase 1 draws with all of them
    /// alike, like a pen.
    Pen,
    /// Contacting tools of different kinds share the pointer channel; which of its presses
    /// belongs to which cannot be said. No eraser claim is made.
    Mixed,
}

/// One contact's identity in the channel: unique across the process's session, handed out by
/// the start pairing and used by the backend to address pressure reports and the contact's
/// end to the contact they belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContactId(u64);

/// One pressure report on one contact, in the order the tool reported it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// The report's place in its contact's stream, from `0`: the buffer may drop old samples
    /// and the consumer may read late, but the order and the gaps stay sayable.
    pub seq: u64,
    /// The force the tool reported, normalized by the backend (the protocol's `pressure` is
    /// a `0`–`65535` value).
    pub force: Force,
}

/// What [`sole_contact`] saw in the one live contact, at the instant it answered.
#[derive(Debug, Clone, PartialEq)]
pub struct ContactSnapshot {
    /// The contact the samples belong to.
    pub id: ContactId,
    /// Whether the contacting tool was reported of the eraser type (its own tool object, as
    /// compositors report a pen's eraser end). What to erase with is the application's call.
    pub eraser: bool,
    /// The pressure reports kept for the contact, in report order: `samples.first()` is the
    /// contact's first report and never the buffer's eviction victim, `samples.last()` is its
    /// newest. A tool (or driver) without pressure reports reaches here empty — the
    /// application's means to draw such a contact with its fixed width.
    pub samples: Vec<Sample>,
    /// How many reports the contact fed the channel in total,
    /// `samples.len() + dropped == received` while the contact is live and buffered.
    pub received: u64,
    /// How many of those the fixed capacity evicted, oldest kept after the first.
    pub dropped: u64,
}

/// The eraser-typed contacts and the contacts of every other kind.
static ERASERS: AtomicU32 = AtomicU32::new(0);
static OTHERS: AtomicU32 = AtomicU32::new(0);
/// The identity the next contact start takes hold of.
static NEXT_CONTACT: AtomicU64 = AtomicU64::new(0);
/// The live contacts, oldest first. Their buffers are the pressure side of the channel.
static CONTACTS: Mutex<Vec<ContactRecord>> = Mutex::new(Vec::new());

/// One live tool contact and its pressure samples.
struct ContactRecord {
    id: ContactId,
    eraser: bool,
    pressure: SampleBuffer,
}

/// The contact's fixed-capacity window of its pressure reports.
///
/// The overflow policy, applied when a report arrives in a full window: the report after the
/// first evicts the *oldest kept-after-first* sample, so the start of the stroke and the
/// newest samples survive, and `dropped` counts what did not.
struct SampleBuffer {
    first: Option<Sample>,
    after_first: VecDeque<Sample>,
    received: u64,
    dropped: u64,
}

impl Default for SampleBuffer {
    fn default() -> Self {
        SampleBuffer {
            first: None,
            after_first: VecDeque::with_capacity(SAMPLE_CAPACITY.saturating_sub(1)),
            received: 0,
            dropped: 0,
        }
    }
}

impl SampleBuffer {
    fn push(&mut self, force: Force) {
        let seq = self.received;
        self.received = self.received.saturating_add(1);
        let sample = Sample { seq, force };
        let Some(first) = &self.first else {
            self.first = Some(sample);
            return;
        };
        if self.after_first.len() >= SAMPLE_CAPACITY.saturating_sub(1) {
            let _ = self.after_first.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        debug_assert!(first.seq <= sample.seq, "sample sequences grow per contact");
        self.after_first.push_back(sample);
    }

    fn snapshot(&self) -> Vec<Sample> {
        let mut samples =
            Vec::with_capacity(self.after_first.len() + self.first.is_some() as usize);
        if let Some(first) = self.first {
            samples.push(first);
        }
        samples.extend(self.after_first.iter().copied());
        samples
    }
}

/// The tool kind behind the pointer events the current tablet contacts produce: `Eraser` when
/// exactly eraser-typed tools are in contact, `Pen` for exactly others, [`ContactTool::None`]
/// when nothing is in contact, and [`ContactTool::Mixed`] when kinds differ.
pub fn contact_tool() -> ContactTool {
    match (ERASERS.load(Ordering::Relaxed), OTHERS.load(Ordering::Relaxed)) {
        (0, 0) => ContactTool::None,
        (0, _) => ContactTool::Pen,
        (_, 0) => ContactTool::Eraser,
        _ => ContactTool::Mixed,
    }
}

/// The channel's one live contact with its pressure samples, or `None` while no contact (or
/// more than one) is live: a pointer-routed consumer cannot attribute its frame's presses
/// across contacts, so the channel makes no claim there, like [`ContactTool::Mixed`] does.
///
/// The samples are owned by the copy: the buffer belongs to the dispatch side, and a
/// returned snapshot stays valid however the contact moves on after the read.
pub fn sole_contact() -> Option<ContactSnapshot> {
    let contacts = contacts();
    if contacts.len() != 1 {
        return None;
    }
    let record = contacts.first()?;
    Some(ContactSnapshot {
        id: record.id,
        eraser: record.eraser,
        samples: record.pressure.snapshot(),
        received: record.pressure.received,
        dropped: record.pressure.dropped,
    })
}

/// The channel's contacts, with poison recovery: a writer that panicked mid-write left its
/// guard poisoned, and the channel degrades to what the bytes hold rather than panicking.
fn contacts() -> std::sync::MutexGuard<'static, Vec<ContactRecord>> {
    match CONTACTS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// A tool's contact took the channel over, or joined it: forget the identity it answers under.
///
/// Call this exactly once per contact a tool registers; only the Wayland backend does.
#[cfg_attr(not(wayland_platform), allow(dead_code))]
pub(crate) fn contact_started(eraser: bool) -> ContactId {
    let id = ContactId(NEXT_CONTACT.fetch_add(1, Ordering::Relaxed));
    contacts().push(ContactRecord { id, eraser, pressure: SampleBuffer::default() });
    let counter = if eraser { &ERASERS } else { &OTHERS };
    counter.fetch_add(1, Ordering::Relaxed);
    id
}

/// A contacting tool's contact ended, giving back the record [`contact_started`] registered
/// for it. Ends address their contact, so an end without a start (or a repeated end) finds no
/// record and leaves the counts alone: the floor the counters sat on is not reachable here —
/// the backing store is the pairing, not the arithmetic.
#[cfg_attr(not(wayland_platform), allow(dead_code))]
pub(crate) fn contact_ended(id: ContactId) {
    let mut contacts = contacts();
    let Some(index) = contacts.iter().position(|record| record.id == id) else {
        return;
    };
    let record = contacts.remove(index);
    let counter = if record.eraser { &ERASERS } else { &OTHERS };
    let mut current = counter.load(Ordering::Relaxed);
    while current > 0 {
        match counter.compare_exchange_weak(
            current,
            current - 1,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(actual) => current = actual,
        }
    }
}

/// One pressure report lands on the contact it belongs to: the buffers live per contact, so
/// an address that is not a live contact (a report after its end, against protocol) has no
/// record to feed and goes away.
#[cfg_attr(not(wayland_platform), allow(dead_code))]
pub(crate) fn contact_pressure(id: ContactId, force: Force) {
    if let Some(record) = contacts().iter_mut().find(|record| record.id == id) {
        record.pressure.push(force);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        contact_ended, contact_pressure, contact_started, contact_tool, sole_contact, ContactId,
        ContactTool, Force, SampleBuffer, SAMPLE_CAPACITY,
    };

    /// The whole channel's runtime in one serial test: its state is process-global and other
    /// tests never touch it, so running alone keeps every assertion here stable.
    #[test]
    fn contacts_pair_into_attribution() {
        assert_eq!(contact_tool(), ContactTool::None);
        assert_eq!(sole_contact(), None);

        // A pen contact draws as usual.
        let pen = contact_started(false);
        assert_eq!(contact_tool(), ContactTool::Pen);
        // And a device that reports no pressure arrives with an empty sample set — the app's
        // fixed-width drawing case.
        assert_eq!(
            sole_contact().map(|contact| (contact.eraser, contact.samples.len(), contact.received)),
            Some((false, 0, 0))
        );
        contact_ended(pen);
        assert_eq!(contact_tool(), ContactTool::None);
        assert_eq!(sole_contact(), None);

        // The eraser end is its own tool object: while it is in contact, its presses are
        // attributed to the eraser, whatever pen contacts came before.
        let eraser = contact_started(true);
        assert_eq!(contact_tool(), ContactTool::Eraser);

        // A second eraser-typed tool (the driver may present each end anew per proximity).
        let second = contact_started(true);
        assert_eq!(contact_tool(), ContactTool::Eraser);
        contact_ended(second);
        assert_eq!(contact_tool(), ContactTool::Eraser);
        contact_ended(eraser);
        assert_eq!(contact_tool(), ContactTool::None);

        // A contact of another kind while the eraser is down: the shared pointer channel
        // cannot attribute such presses, so it makes no claim.
        let eraser = contact_started(true);
        let pen = contact_started(false);
        assert_eq!(contact_tool(), ContactTool::Mixed);
        assert_eq!(sole_contact(), None);
        contact_ended(pen);
        contact_ended(eraser);
        assert_eq!(contact_tool(), ContactTool::None);

        // Two contacts of the same kind across different tool objects: the kind claim
        // stands, but no sole contact is nameable across them.
        let one = contact_started(false);
        let two = contact_started(false);
        assert_eq!(contact_tool(), ContactTool::Pen, "same-kind contacts keep the kind claim");
        assert_eq!(sole_contact(), None, "but no sole contact across them");
        contact_ended(two);
        contact_ended(one);
        assert_eq!(contact_tool(), ContactTool::None);

        // Ends heavier than the pairings they saw (a compositor bug per protocol) stay
        // where they are instead of underflowing into a false attribution: an end whose
        // contact was never registered finds no record.
        contact_ended(ContactId(u64::MAX));
        assert_eq!(contact_tool(), ContactTool::None);
        assert_eq!(sole_contact(), None);
        let recorded = contact_started(true);
        assert_eq!(contact_tool(), ContactTool::Eraser);
        contact_ended(recorded);

        // Pressure follows its contact: per-contact sequence, per-contact kind.
        let eraser = contact_started(true);
        contact_pressure(eraser, Force::Normalized(0.25));
        contact_pressure(eraser, Force::Normalized(0.5));
        let snapshot = sole_contact().expect("one live contact");
        assert!(snapshot.eraser);
        assert_eq!(snapshot.samples.len(), 2);
        assert_eq!(snapshot.samples[0], super::Sample { seq: 0, force: Force::Normalized(0.25) });
        assert_eq!(snapshot.samples[1].seq, 1);
        assert_eq!(snapshot.received, 2);
        assert_eq!(snapshot.dropped, 0);
        // A second live contact of the same kind hides the sole contact, whose samples can
        // no longer be attributed; it neither steals nor merges the first contact's samples.
        let second = contact_started(true);
        assert_eq!(sole_contact(), None);
        contact_ended(second);
        assert_eq!(sole_contact().map(|s| s.samples.len()), Some(2));
        contact_ended(eraser);
        assert_eq!(sole_contact(), None);

        // The fixed-capacity window keeps the stroke's start and the newest samples: the
        // first report is never the eviction victim, the oldest kept-after-first is.
        let contact = contact_started(false);
        for step in 0..(SAMPLE_CAPACITY as u64 + 8) {
            contact_pressure(contact, Force::Normalized(step as f64 / 100.0));
        }
        let snapshot = sole_contact().expect("one live contact");
        assert_eq!(snapshot.received, SAMPLE_CAPACITY as u64 + 8);
        assert_eq!(snapshot.dropped, 8, "exactly the overflow the window saw");
        assert_eq!(snapshot.samples.len(), SAMPLE_CAPACITY);
        assert_eq!(snapshot.samples.first().map(|s| s.seq), Some(0), "the stroke start stays");
        assert_eq!(
            snapshot.samples.last().map(|s| s.seq),
            Some(SAMPLE_CAPACITY as u64 + 7),
            "the newest sample stays"
        );
        contact_ended(contact);
        assert_eq!(sole_contact(), None);

        // A contact that reports nothing is the fixed-width case, not the eraser case: its
        // provided attribution stays per-kind, and its empty buffers end with the contact.
        let silent = contact_started(false);
        assert_eq!(
            sole_contact().map(|contact| (contact.eraser, contact.samples.is_empty())),
            Some((false, true))
        );
        contact_ended(silent);

        // The buffer policy stands on its own, so the snapshot path cannot hide an overflow
        // the unit didn't sample.
        let mut buffer = SampleBuffer::default();
        buffer.push(Force::Normalized(1.0));
        for _ in 0..SAMPLE_CAPACITY {
            buffer.push(Force::Normalized(0.5));
        }
        assert_eq!(buffer.snapshot().len(), SAMPLE_CAPACITY);
        assert_eq!(buffer.snapshot()[0], super::Sample { seq: 0, force: Force::Normalized(1.0) });
        assert_eq!(buffer.snapshot()[SAMPLE_CAPACITY - 1].seq, SAMPLE_CAPACITY as u64);
        assert_eq!(buffer.received, SAMPLE_CAPACITY as u64 + 1);
        assert_eq!(buffer.dropped, 1);
    }
}
