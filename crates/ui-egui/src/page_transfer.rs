//! Copy selected Organize pages into another open document, using the same insertion edit as
//! Paste pages. The source is a bounded snapshot, not a file path or a system drag payload.

use egui::{Context, Id, Key};
use pdfcraft_engine::{DocId, Edit, FieldConflicts};

use crate::{PageClip, PdfCraftApp};

#[derive(Clone, Debug)]
pub(crate) struct PageDrag {
    clip: PageClip,
    generation: u64,
}

fn incoming_id() -> Id {
    Id::unique("organize-incoming-page-drag")
}

pub(crate) fn incoming(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(incoming_id())).unwrap_or(false)
}

impl PdfCraftApp {
    pub(crate) fn dragging_pages_to(&self, target: DocId) -> bool {
        self.page_drag.as_ref().is_some_and(|d| d.clip.from != target)
    }

    /// Run after the tab strip (which may activate the destination) and before drawing its grid.
    pub(crate) fn prepare_page_drag(&mut self, ctx: &Context) {
        let cancel = self.modal_open()
            || self.palette_open
            || egui::Popup::is_any_open(ctx)
            || !ctx.input(|i| i.focused)
            || ctx.input(|i| i.key_pressed(Key::Escape));
        if cancel {
            self.cancel_page_drag();
        }
        let incoming = self.active_ids().is_some_and(|(_, id)| self.dragging_pages_to(id)) && !cancel;
        ctx.data_mut(|d| d.insert_temp(incoming_id(), incoming));
    }

    fn cancel_page_drag(&mut self) {
        if let Some(drag) = self.page_drag.take() {
            for view in &mut self.views {
                if view.id == drag.clip.from {
                    view.org_drag = None;
                }
            }
        }
    }

    /// Called after pending edits, so snapshots contain current unsaved content. A source edit
    /// during the gesture invalidates its page indices; it must never insert different pages.
    pub(crate) fn finish_page_drag(&mut self, ctx: &Context) {
        if self.modal_open()
            || self.palette_open
            || egui::Popup::is_any_open(ctx)
            || !ctx.input(|i| i.focused)
            || ctx.input(|i| i.key_pressed(Key::Escape))
        {
            self.cancel_page_drag();
            return;
        }
        let released = ctx.input(|i| i.pointer.primary_released());
        if self.page_drag.is_none() && !released && ctx.input(|i| i.pointer.primary_down()) {
            let Some((index, id)) = self.active_ids() else { return };
            let Some(view) = self.views.get(index) else { return };
            let Some(pages) = view.org_drag.as_ref().filter(|p| !p.is_empty()) else { return };
            let Some(doc) = self.session.get(id) else { return };
            if !doc.allows_assembly() || !doc.allows_copying() || pages.iter().any(|p| *p >= doc.info.pages.len()) {
                return;
            }
            self.page_drag = Some(PageDrag {
                clip: PageClip { password: doc.password.clone(), from: id, name: doc.name.clone(), bytes: doc.bytes.clone(), pages: pages.clone() },
                generation: doc.edit_generation(),
            });
        }
        if !released {
            // A window losing its pointer without a release must not retain a stale transfer.
            if !ctx.input(|i| i.pointer.primary_down()) {
                self.cancel_page_drag();
            }
            return;
        }
        let Some(drag) = self.page_drag.clone() else { return };
        self.cancel_page_drag();
        let Some((index, target)) = self.active_ids() else { return };
        if target == drag.clip.from {
            return; // The existing grid handles same-document reorder and modifier-copy.
        }
        let Some(view) = self.views.get(index).filter(|v| v.organize) else { return };
        let Some(at) = view.grid_gap else { return }; // A release outside the grid cancels.
        let Some(source) = self.session.get(drag.clip.from) else { return };
        if source.edit_generation() != drag.generation
            || !std::sync::Arc::ptr_eq(&source.bytes, &drag.clip.bytes)
            || !source.allows_assembly()
            || !source.allows_copying()
        {
            self.notify_tr("The document changed while you were choosing a file, so nothing was changed.");
            return;
        }
        if !self.session.get(target).is_some_and(|d| d.allows_assembly()) {
            return;
        }
        let count = drag.clip.pages.len();
        if self.apply_edit(Edit::InsertPagesFrom {
            name: drag.clip.name,
            bytes: drag.clip.bytes,
            password: drag.clip.password,
            pages: Some(drag.clip.pages),
            at,
            field_conflicts: FieldConflicts::Rename,
        }) && let Some(view) = self.views.get_mut(index)
        {
            view.select_pages(&(at..at.saturating_add(count)).collect::<Vec<_>>());
        }
    }
}
