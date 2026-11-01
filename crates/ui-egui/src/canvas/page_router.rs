//! Who a press on the pages belongs to.
//!
//! Each page module answers "what of mine is under this point?" in one place, its hit test, and
//! the answer is a [`Hit`]: what is there ([`Owner`]), on which [`Layer`], with which cursor and
//! tip. The pointer's hits set the cursor and the tip; on the frame of a press, the hits at the
//! press point decide who captures it ([`PageRouter::press`]), by the same table: the highest layer
//! the active tool reaches. Until the button is up only the owner acts on the press, its drag and
//! its release, so what the cursor promised is what the press does, and nothing takes a gesture
//! back halfway.

use egui::CursorIcon;

use crate::QuickTool;
use crate::page_cursor::Layer;

/// What a hit is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Owner {
    /// A comment (or a Fill & Sign mark), by its `/Annots` index, and the part of it.
    Comment { index: usize, part: CommentPart },
    /// A link, by its index in the document's links.
    Link { index: usize },
    /// A form field's widget, by the field's index in the form and the widget's in the field.
    Field { field: usize, widget: usize },
    /// Prepare Form: a field's widget, to select and move.
    PrepareField { field: usize, widget: usize },
    /// Prepare Form: a corner handle of the selected field, to resize it.
    PrepareCorner { left: bool, top: bool },
    /// Edit text: a paragraph, by its index among the page's, and what of it a press takes.
    Paragraph { block: usize, part: ParagraphPart },
    /// Edit text: a picture, by its index among the page's, and what of it a press takes.
    Picture { index: usize, part: PicturePart },
    /// Edit text: the objects selected together, or the frame of the paragraph open for typing.
    Objects { part: ObjectsPart },
    /// Added text or a picture placed on the page, by its index among the page's, and a corner of
    /// the selected one (left, top) or the item itself.
    Added { index: usize, corner: Option<(bool, bool)> },
}

/// The part of a picture a hit is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PicturePart {
    /// The picture: a click selects it, a drag moves the selected one.
    Body,
    /// A corner of the selected picture (left top, right top, left bottom, right bottom): a drag
    /// resizes it.
    Corner(usize),
    /// The selected picture's rotate knob: a drag turns it.
    Knob,
}

/// What of the objects a hit is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObjectsPart {
    /// The objects selected together: a drag moves them all.
    Selection,
    /// The frame drawn around the paragraph open for typing: a click selects the box, a drag moves it.
    Frame,
}

/// The part of a paragraph a hit is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ParagraphPart {
    /// The paragraph: a drag moves it, a click opens it for typing.
    Body,
    /// Its left or right edge (on screen): a drag rewraps it.
    Left,
    Right,
}

/// The part of a comment a hit is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommentPart {
    /// The comment itself: a drag moves it, a click selects it.
    Body,
    /// A resize handle of the selected comment: (-1, 0 or 1) across, then down.
    Handle(i8, i8),
    /// A point of the selected line, polygon or cloud.
    Point(usize),
}

/// What lies under a point, for one page module.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Hit {
    pub owner: Owner,
    pub layer: Layer,
    pub icon: CursorIcon,
    /// Beside the pointer where this hit wins (a link's target).
    pub tip: Option<String>,
    /// A drag from here is not this owner's: a click on a highlight selects it, a drag across it
    /// selects the text underneath.
    pub declines_drag: bool,
}

impl Hit {
    pub(crate) fn new(owner: Owner, layer: Layer, icon: CursorIcon) -> Self {
        Hit { owner, layer, icon, tip: None, declines_drag: false }
    }
}

/// The hit that wins among `hits` with `tool`: the highest layer the tool reaches, the last of
/// equals (modules are asked bottom up, in paint order).
pub(crate) fn top(hits: impl IntoIterator<Item = Hit>, tool: QuickTool) -> Option<Hit> {
    hits.into_iter().filter(|h| h.layer.reached_by(tool)).fold(None, |best: Option<Hit>, h| match best {
        Some(b) if b.layer > h.layer => Some(b),
        _ => Some(h),
    })
}

/// A press the pages hold, and who it belongs to.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Capture {
    pub page: usize,
    pub owner: Owner,
    pub declines_drag: bool,
}

/// The press the pages hold, from its frame until the button is up.
#[derive(Debug, Default)]
pub(crate) struct PageRouter {
    capture: Option<Capture>,
}

impl PageRouter {
    /// A press on `page`: `hits` at the press point decide who it belongs to. A press nobody's
    /// hit claims is held by nobody (the page's text, or a tool that acts anywhere on the page).
    pub(crate) fn press(&mut self, page: usize, hits: impl IntoIterator<Item = Hit>, tool: QuickTool) {
        self.capture = top(hits, tool).map(|h| Capture { page, owner: h.owner, declines_drag: h.declines_drag });
    }

    /// Who holds the press on `page`.
    pub(crate) fn owner(&self, page: usize) -> Option<Owner> {
        self.capture.as_ref().filter(|c| c.page == page).map(|c| c.owner)
    }

    /// Whether the press is held by someone who keeps its drag: then the page's text is not
    /// selected by it.
    pub(crate) fn keeps_drag(&self) -> bool {
        self.capture.as_ref().is_some_and(|c| !c.declines_drag)
    }

    /// The press is over once the button is up and its release has been seen.
    pub(crate) fn settle(&mut self, any_down: bool, edges: bool) {
        if !any_down && !edges {
            self.capture = None;
        }
    }

    /// Drops the press (Escape, a dialog, automation's cancel).
    pub(crate) fn cancel(&mut self) {
        self.capture = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(layer: Layer, index: usize) -> Hit {
        Hit::new(Owner::Comment { index, part: CommentPart::Body }, layer, CursorIcon::Move)
    }

    #[test]
    fn the_highest_reached_layer_wins_and_the_last_of_equals() {
        let hits = [hit(Layer::Comment, 0), hit(Layer::Link, 1), hit(Layer::Comment, 2)];
        assert_eq!(top(hits.clone(), QuickTool::Select).map(|h| h.owner), Some(hits[2].owner));
        // A stamp reaches neither links nor comments.
        let stamp = QuickTool::Stamp(pdfcraft_engine::StampKind::DynApproved);
        assert_eq!(top(hits, stamp), None);
        assert_eq!(top([hit(Layer::Placed, 3), hit(Layer::Comment, 4)], stamp).map(|h| h.owner), Some(hit(Layer::Placed, 3).owner));
    }

    #[test]
    fn a_press_is_held_until_the_button_is_up_and_its_release_seen() {
        let mut r = PageRouter::default();
        r.press(2, [hit(Layer::Comment, 7)], QuickTool::Select);
        assert_eq!(r.owner(2), Some(hit(Layer::Comment, 7).owner));
        assert_eq!(r.owner(1), None, "only on its page");
        assert!(r.keeps_drag());
        r.settle(false, true);
        assert!(r.owner(2).is_some(), "the release frame still belongs to the press");
        r.settle(false, false);
        assert_eq!(r.owner(2), None);
        r.press(0, [], QuickTool::Select);
        assert_eq!(r.owner(0), None, "a press nobody claims is held by nobody");
        assert!(!r.keeps_drag());
    }

    #[test]
    fn a_highlight_lets_its_drag_go_to_the_text() {
        let mut r = PageRouter::default();
        r.press(0, [Hit { declines_drag: true, ..hit(Layer::Comment, 0) }], QuickTool::Select);
        assert!(r.owner(0).is_some() && !r.keeps_drag());
        r.cancel();
        assert_eq!(r.owner(0), None);
    }
}
