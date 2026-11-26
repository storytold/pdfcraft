//! The recording buffer: the commands a person (or an agent) ran while recording, each with the
//! document, sheet and selection it ran in.
//!
//! Only top-level calls are kept. A command that runs other commands (Custom Views › Show, a
//! script) is one step, so playing the recording back does not run the inner commands twice. The
//! buffer is separate from the journal, so trimming the journal never loses recorded steps, and
//! secret parameters are left out as they are in the journal ([`crate::cmd::journal_params`]).

use serde::Serialize;
use serde_json::Value as Json;
use std::collections::BTreeMap;

use crate::Selection;

/// The most steps one recording keeps; later ones are counted in [`Recorder::dropped`].
pub const RECORD_LIMIT: usize = 100_000;
/// The most bytes (steps serialized as JSON) one recording keeps. A step keeps its full
/// parameters so it can be played back, and a `file.open` holds the whole file as base64, so
/// without a size cap a recording of a few opens of a large workbook keeps tens of MB alive.
pub const RECORD_MAX_BYTES: usize = 64 * 1024 * 1024;

/// Commands never recorded: recovery actions and the debugger, not steps of the person's work. A
/// recording that replayed Stop Action would stop the run it is a step of, and the debugger runs
/// on its own, never as a step of a script or macro.
pub const NOT_RECORDED: [&str; 2] = ["automate.stopAction", "macro.debug"];

/// One recorded command and where it ran.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedStep {
    /// Uid of the document that was active when the command ran (0 when none was open).
    pub doc: u64,
    /// That document's active sheet (index and name) just before the command ran.
    pub sheet: usize,
    pub sheet_name: String,
    /// That document's selection just before the command ran.
    pub selection: Selection,
    /// The command id and its parameters, without secrets.
    pub id: String,
    pub params: Json,
}

/// Numbers each recording, so an undo group's mark from an earlier one isn't read as this one's.
static NEXT_RECORDING: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The steps recorded since recording started.
#[derive(Clone, Debug)]
pub struct Recorder {
    pub steps: Vec<RecordedStep>,
    /// Steps not kept because the recording reached [`RECORD_LIMIT`] or [`RECORD_MAX_BYTES`].
    /// Once a step is dropped, every later one is too, so `steps` is always the beginning of
    /// what ran, never a sequence with gaps.
    pub dropped: usize,
    /// The size of `steps` and `ran` serialized as JSON.
    bytes: usize,
    /// What a step ran, by its index in `steps`, for a command that runs steps stored in the
    /// workbook (`automate.runScript`, `macro.run`): the commands as they ran, so a recording
    /// keeps them even when the script or macro is edited or deleted before the recording stops.
    /// A run without an entry is left out when the recording is used.
    pub ran: BTreeMap<usize, Vec<(String, Json)>>,
    /// Which recording this is, and the number the next kept step gets: each step's number is in
    /// `seqs`, beside it, so a mark stays right however many steps rollbacks take out.
    generation: u64,
    next_seq: u64,
    seqs: Vec<u64>,
}

impl Default for Recorder {
    fn default() -> Self {
        let generation = NEXT_RECORDING.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Recorder { steps: Vec::new(), dropped: 0, bytes: 0, ran: BTreeMap::new(), generation, next_seq: 0, seqs: Vec::new() }
    }
}

/// `params` with the state a toggle set (Bold's `on`, from the command's result), as Excel
/// records `Bold = True`: playing the step back sets that state rather than flipping it again.
/// `python.output` without `mode` is a toggle too: it keeps the return type it set (the result's
/// `output`) as its `mode`.
pub(crate) fn with_toggle_state(id: &str, mut params: Json, result: &Json) -> Json {
    let state = match id {
        "python.output" => result.get("output").filter(|o| o.is_string()).map(|o| ("mode", o)),
        _ => result.get("on").filter(|o| o.is_boolean()).map(|o| ("on", o)),
    };
    if let Some((key, state)) = state {
        if params.is_null() {
            params = Json::Object(serde_json::Map::new());
        }
        if let Json::Object(p) = &mut params
            && p.get(key).is_none_or(Json::is_null)
        {
            p.insert(key.into(), state.clone());
        }
    }
    params
}

impl Recorder {
    /// Adds a step, or counts it as dropped once the buffer is full; `true` when it was kept.
    pub fn push(&mut self, step: RecordedStep) -> bool {
        // Once a step has been dropped, or the step count is full, the rest are counted without
        // being serialized (a `file.open` step holds a whole file).
        if self.dropped > 0 || self.steps.len() >= RECORD_LIMIT {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        let size = serde_json::to_vec(&step).map_or(0, |v| v.len());
        if self.bytes.saturating_add(size) <= RECORD_MAX_BYTES {
            self.bytes = self.bytes.saturating_add(size);
            self.steps.push(step);
            self.seqs.push(self.next_seq);
            self.next_seq = self.next_seq.saturating_add(1);
            true
        } else {
            self.dropped = self.dropped.saturating_add(1);
            false
        }
    }

    /// Attaches what the last kept step ran ([`Recorder::ran`]), within the byte budget: past it,
    /// the step stays without it, and the recording leaves the run out and lists it as skipped.
    pub fn attach_ran(&mut self, steps: Vec<(String, Json)>) {
        let Some(index) = self.steps.len().checked_sub(1) else { return };
        let size = serde_json::to_vec(&steps).map_or(0, |v| v.len());
        if self.bytes.saturating_add(size) <= RECORD_MAX_BYTES {
            self.bytes = self.bytes.saturating_add(size);
            self.ran.insert(index, steps);
        }
    }

    /// Where the recording is now, for [`Recorder::cut_back`]: this recording and the number its
    /// next step gets.
    pub(crate) fn mark(&self) -> (u64, u64) {
        (self.generation, self.next_seq)
    }

    /// Takes back the steps recorded in the document with uid `doc` since `mark` (an undo group
    /// on it was rolled back, so playing them back would make edits it no longer has). Steps in
    /// other documents stay: their edits do too. Steps the buffer dropped stay counted.
    /// What a step ran ([`Recorder::ran`]) goes with its step, and stays with a kept one under its
    /// new index. A mark from another recording (the recording started again inside the group)
    /// means every step of this one came after it.
    pub(crate) fn cut_back(&mut self, (generation, seq): (u64, u64), doc: u64) {
        let from = if generation == self.generation { seq } else { 0 };
        let gone = |i: usize, step: &RecordedStep| step.doc == doc && self.seqs.get(i).is_some_and(|&n| n >= from);
        if !self.steps.iter().enumerate().any(|(i, s)| gone(i, s)) {
            return;
        }
        let mut ran = std::mem::take(&mut self.ran);
        let (mut steps, mut seqs) = (Vec::with_capacity(self.steps.len()), Vec::with_capacity(self.seqs.len()));
        for (i, step) in self.steps.iter().enumerate() {
            if gone(i, step) {
                continue;
            }
            if let Some(r) = ran.remove(&i) {
                self.ran.insert(steps.len(), r);
            }
            steps.push(step.clone());
            seqs.push(self.seqs.get(i).copied().unwrap_or(0));
        }
        (self.steps, self.seqs) = (steps, seqs);
        let size = |v: Result<Vec<u8>, serde_json::Error>| v.map_or(0, |v| v.len());
        self.bytes = self.steps.iter().map(|s| size(serde_json::to_vec(s))).sum::<usize>()
            + self.ran.values().map(|r| size(serde_json::to_vec(r))).sum::<usize>();
    }
}
