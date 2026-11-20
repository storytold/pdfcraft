//! File pickers through the host's file service ([`Files`]): a pick never blocks the frame, and
//! what is done with the choice is a continuation that runs on the pass that takes the answer.
//!
//! Work that depends on the choice (writing the chosen file, then closing the tab; deleting
//! extracted pages once their folder is chosen) happens in that continuation and nowhere else.
//! One picker shows at a time and later requests wait in order: that is the file service's rule,
//! not this module's.

use std::collections::BTreeMap;
use std::path::PathBuf;

use craft::host::{PickId, PickRequest, Picked};

use crate::PdfCraftApp;
use crate::files::PickTarget;

/// What runs with a pick's answer, given the context of the pass that takes it.
type Then = Box<dyn FnOnce(&mut PdfCraftApp, &egui::Context, Picked)>;

/// The picks asked for and not answered yet, each with the document it acts on and its
/// continuation. Keyed by ticket, in the order they were asked (tickets grow).
#[derive(Default)]
pub struct Picks {
    waiting: BTreeMap<PickId, (Option<PickTarget>, Then)>,
}

impl std::fmt::Debug for Picks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Picks").field("waiting", &self.waiting.keys().collect::<Vec<_>>()).finish()
    }
}

impl PdfCraftApp {
    /// Ask the host's file service for `request`, and run `then` with the answer on the pass that
    /// takes it ([`Self::process_picks`]), cancelled or not.
    ///
    /// With `target` ([`Self::pick_target`]), a choice that arrives once that document is no
    /// longer the active one, or has been edited, is dropped and the user is told
    /// ([`Self::still_pick_target`]): use it whenever `then` edits the active document or refers
    /// to its pages or objects by position. A cancelled answer reaches `then` whatever the target.
    pub(crate) fn pick_file(
        &mut self,
        request: PickRequest,
        target: Option<PickTarget>,
        then: impl FnOnce(&mut PdfCraftApp, &egui::Context, Picked) + 'static,
    ) -> PickId {
        let id = self.host.files().pick(request);
        self.picks.waiting.insert(id, (target, Box::new(then)));
        id
    }

    /// [`Self::pick_file`] for one path: `then` gets the first chosen path, and nothing runs when
    /// the picker is cancelled. On the web the path is the browser's file name.
    pub(crate) fn pick_first(
        &mut self,
        request: PickRequest,
        target: Option<PickTarget>,
        then: impl FnOnce(&mut PdfCraftApp, &egui::Context, PathBuf) + 'static,
    ) -> PickId {
        self.pick_file(request, target, move |app, ctx, picked| {
            if let Some(path) = picked.first_path() {
                then(app, ctx, path);
            }
        })
    }

    /// Run the continuations of the picks answered since the last call, oldest first. Called each
    /// pass. It reads the file service's answers, not the frame's input, so a second pass in one
    /// frame finds only what was answered since the first and nothing runs twice. A continuation
    /// that asks again gets a new ticket, taken on a later pass.
    pub(crate) fn process_picks(&mut self, ctx: &egui::Context) {
        let asked: Vec<PickId> = self.picks.waiting.keys().copied().collect();
        for id in asked {
            let Some(picked) = self.host.files().take_answer(id) else { continue };
            let Some((target, then)) = self.picks.waiting.remove(&id) else { continue };
            if let Picked::Unavailable(cause) = &picked {
                self.notify_fmt("Couldn't show the file picker: {e}", &[("e", cause.as_str())]);
            }
            let chosen = matches!(picked, Picked::Paths(_) | Picked::Contents(_));
            if chosen && !self.still_pick_target(target) {
                continue;
            }
            // Last-resort guard (AGENTS.md §4), as for commands: this work runs on behalf of the
            // command that asked, and a panic in it must not take the app down.
            if let Err(m) = pdfcraft_engine::guard(|| then(self, ctx, picked)) {
                self.notify_fmt("That didn't work: an internal error stopped it ({m}).", &[("m", m.as_str())]);
            }
        }
    }

    /// A picker is on screen or waiting: opens and full-screen prompts hold back meanwhile.
    pub(crate) fn picks_pending(&self) -> bool {
        !self.picks.waiting.is_empty() || self.host.files().pick_pending()
    }
}
