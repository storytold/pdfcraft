//! The pointer as the pages see it, read once per frame from the page view's content widget.
//!
//! egui knows what is drawn over the pages (a dialog, a menu, the palette, the quick bar, the
//! chrome over a page scrolled under it) and which widget holds a drag (a panel's tab carried
//! over them), and keeps a press with the widget it began on until release. egui's raw pointer
//! state knows none of that, and its hover position is gone on the frame
//! a touch or a Windows pen lifts: egui-winit sends `PointerGone` with every touch release, and
//! winit reports Windows pen input as touch. So page modules read [`PagePointer`], never egui's
//! pointer positions or button states; a test below keeps it that way.
//!
//! Dialogs are drawn after the pages, so on the frame one opens egui has not met its layer yet:
//! the reading also takes whether one is open.

use egui::{Event, PointerButton, Pos2, Rect};

/// A primary-button press or release, and where it happened.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Edge {
    Press(Pos2),
    Release(Pos2),
}

/// How many of a frame's presses and releases are kept. A frame holds one or two (a click, a
/// double-click in a slow frame), but one after a stall can hold any number.
const MAX_EDGES: usize = 8;

/// One frame's pointer, as the pages see it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PagePointer {
    /// Where the pointer is, while the pages are what egui has under it and no other widget holds
    /// a drag (a panel's tab carried over them), or while they hold a press (until the frame of
    /// its release, past a page's edge included). With a dialog open only a drag the pages
    /// already have keeps it. It is egui's `interact_pos`, which still has the position on the
    /// frame a touch lifts.
    pub pos: Option<Pos2>,
    /// Where the press the pages hold began, also on the frame of its release when the press
    /// came in that same frame (egui has forgotten it by then). `None` for a press that began on
    /// something else.
    pub origin: Option<Pos2>,
    /// This frame's primary presses and releases of a press the pages hold, in order.
    pub edges: Vec<Edge>,
    /// The primary button is down, wherever it was pressed: a gesture ends once it is up.
    pub primary_down: bool,
    /// Some button is down, wherever it was pressed.
    pub any_down: bool,
    /// The current interaction began in excluded viewport chrome.
    pub blocked_press: bool,
    /// A pointer-button edge in excluded viewport chrome in this frame. Unlike `blocked_press`,
    /// this is not retained into later frames, so hover or a past ruler click cannot suppress an
    /// editor's ordinary focus-loss handling.
    pub blocked_interaction: Option<Pos2>,
}

impl PagePointer {
    /// The current press origin when it did not begin in excluded viewport chrome.
    ///
    /// The Hand tool reads this before the page widget is laid out for the current frame, while
    /// egui still identifies the previous frame's page widget as the drag owner. Keeping the raw
    /// pointer read here preserves the single page-pointer boundary.
    pub(crate) fn unblocked_press_origin(ui: &egui::Ui, blocked: &[Rect]) -> Option<Pos2> {
        ui.input(|i| i.pointer.press_origin()).filter(|origin| !blocked.iter().any(|rect| rect.contains(*origin)))
    }

    /// The page pointer with gestures that begin in `blocked` retained as blocked through every
    /// egui pass of their release frame. A new frame clears the retained block.
    pub(crate) fn read_excluding(ui: &egui::Ui, resp: &egui::Response, modal: bool, blocked: &[Rect]) -> Self {
        let (at, origin, edges, mut primary_down, mut any_down, raw_press_blocked, raw_primary_press_blocked, blocked_interaction) = ui.input(|i| {
            (
                i.pointer.interact_pos(),
                i.pointer.press_origin(),
                edges(&i.events),
                i.pointer.primary_down(),
                i.pointer.any_down(),
                i.events.iter().any(|event| match event {
                    Event::PointerButton { pos, pressed: true, .. } => blocked.iter().any(|rect| rect.contains(*pos)),
                    _ => false,
                }),
                i.events.iter().any(|event| match event {
                    Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, .. } => blocked.iter().any(|rect| rect.contains(*pos)),
                    _ => false,
                }),
                i.events.iter().find_map(|event| match event {
                    Event::PointerButton { pos, .. } if pos.is_finite() && blocked.iter().any(|rect| rect.contains(*pos)) => Some(*pos),
                    _ => None,
                }),
            )
        });
        let blocked_press_key = resp.id.with("blocked-press");
        let primary_press_blocked_key = resp.id.with("primary-press-blocked");
        let clear_blocked_press_key = resp.id.with("clear-blocked-press");
        if ui.ctx().current_pass_index() == 0 && ui.data(|data| data.get_temp::<bool>(clear_blocked_press_key)).unwrap_or(false) {
            ui.data_mut(|data| {
                data.remove_temp::<bool>(blocked_press_key);
                data.remove_temp::<bool>(primary_press_blocked_key);
                data.remove_temp::<bool>(clear_blocked_press_key);
            });
        }
        let blocked_at = |point: Pos2| blocked.iter().any(|rect| rect.contains(point));
        if raw_primary_press_blocked {
            ui.data_mut(|data| data.insert_temp(primary_press_blocked_key, true));
        }
        let primary_press_blocked = raw_primary_press_blocked || ui.data(|data| data.get_temp::<bool>(primary_press_blocked_key)).unwrap_or(false);
        let started_blocked = raw_press_blocked || origin.or_else(|| first_press(&edges)).is_some_and(blocked_at);
        if started_blocked {
            ui.data_mut(|data| data.insert_temp(blocked_press_key, true));
        }
        let blocked_press = started_blocked || ui.data(|data| data.get_temp::<bool>(blocked_press_key)).unwrap_or(false);
        let released = edges.iter().any(|edge| matches!(edge, Edge::Release(_)));
        // A later button press over viewport chrome must not revoke a primary drag that the pages
        // already own. Keep the chrome press blocked for context-menu handling while delivering
        // the held primary pointer and its release to the active page operation.
        let preserve_primary = blocked_press
            && !primary_press_blocked
            && ((primary_down && resp.dragged_by(PointerButton::Primary)) || (released && resp.drag_stopped()));
        let raw_any_down = any_down;
        if blocked_press && !preserve_primary {
            primary_down = false;
            any_down = false;
        }
        // egui's own answer to "do the pages hold a press?": from the press on until the frame
        // of its click, release or drag stop. A drag can outlive egui's press while the
        // pointer is outside the window. With a dialog open only a drag counts, up to the frame
        // it stops: on the frame one opens egui still gives the pages a press that lands under
        // it, and its click.
        let held =
            preserve_primary || (!blocked_press && (resp.dragged() || resp.drag_stopped() || (!modal && resp.interact_pointer_pos().is_some())));
        // `contains_pointer` alone also holds a few points under the edge of an Area over the
        // pages (egui's interact radius), so the top layer there must be the pages' own. A
        // dialog covers them from the frame it opens, before egui knows its layer.
        let layer = ui.layer_id();
        let on_top =
            |p: &Pos2| !modal && !dragged_elsewhere(ui, resp) && resp.contains_pointer() && ui.ctx().layer_id_at(*p).is_none_or(|l| l == layer);
        let pos = at.filter(|p| preserve_primary || (!blocked_press && (held || (on_top(p) && !blocked_at(*p)))));
        if blocked_press && !raw_any_down {
            ui.data_mut(|data| data.insert_temp(clear_blocked_press_key, true));
        }
        if !held {
            return Self { pos, origin: None, edges: Vec::new(), primary_down, any_down, blocked_press, blocked_interaction };
        }
        Self { pos, origin: origin.or_else(|| first_press(&edges)), edges, primary_down, any_down, blocked_press, blocked_interaction }
    }

    /// Where the pointer is, when it is inside `r`.
    pub(crate) fn within(&self, r: Rect) -> Option<Pos2> {
        self.pos.filter(|p| r.contains(*p))
    }

    /// Where this frame's first primary press was.
    pub(crate) fn pressed(&self) -> Option<Pos2> {
        first_press(&self.edges)
    }
}

/// Whether egui gives the drag to a widget that is not the pages' (a panel's tab, a floating panel's
/// grip): egui hovers nothing else until it stops, though it still reports the pages under the
/// pointer. A widget on the pages (a form field's editor) keeps them.
fn dragged_elsewhere(ui: &egui::Ui, resp: &egui::Response) -> bool {
    let ctx = ui.ctx();
    ctx.dragged_id().is_some_and(|id| {
        id != resp.id && !ctx.read_response(id).is_some_and(|r| r.layer_id == resp.layer_id && resp.interact_rect.contains_rect(r.interact_rect))
    })
}

fn first_press(edges: &[Edge]) -> Option<Pos2> {
    edges.iter().find_map(|e| match e {
        Edge::Press(p) => Some(*p),
        Edge::Release(_) => None,
    })
}

/// The first [`MAX_EDGES`] primary presses and releases among `events`, in order.
fn edges(events: &[Event]) -> Vec<Edge> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::PointerButton { pos, button: PointerButton::Primary, pressed, .. } if pos.is_finite() => {
                Some(if *pressed { Edge::Press(*pos) } else { Edge::Release(*pos) })
            }
            _ => None,
        })
        .take(MAX_EDGES)
        .collect()
}

#[cfg(test)]
mod tests;
