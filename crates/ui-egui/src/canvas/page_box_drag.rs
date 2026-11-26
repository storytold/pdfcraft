//! A rectangle dragged on one placed page: the one gesture of Snapshot, Crop, Prepare's field draw,
//! Redact and SignArea.

use craft::pages::ui::PlacedPage;
use craft::ui::canvas::ToolContext;
use egui::{Pos2, Rect, pos2};

/// The placed page under `at`, if any.
pub(crate) fn page_under(placed: &[PlacedPage], at: Pos2) -> Option<&PlacedPage> {
    placed.iter().find(|p| p.rect.contains(at))
}

/// A drag on a page: where it began, and where it is, kept inside the page it began on. Rectangles
/// are in screen points of the pass, the unit of [`PlacedPage::rect`].
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PageBoxDrag {
    active: Option<Active>,
}

#[derive(Clone, Debug, PartialEq)]
struct Active {
    page: usize,
    bounds: Rect,
    start: Pos2,
    end: Pos2,
}

impl PageBoxDrag {
    /// Starts a drag at `at` on `placed`; a press outside the page starts nothing.
    pub(crate) fn press(&mut self, placed: &PlacedPage, at: Pos2) -> bool {
        if !placed.rect.contains(at) {
            return false;
        }
        self.active = Some(Active { page: placed.page, bounds: placed.rect, start: at, end: at });
        true
    }

    /// Moves the free corner to `at`, clamped to the page.
    pub(crate) fn drag(&mut self, at: Pos2) {
        if let Some(a) = &mut self.active {
            a.end = pos2(at.x.clamp(a.bounds.left(), a.bounds.right()), at.y.clamp(a.bounds.top(), a.bounds.bottom()));
        }
    }

    /// Ends the drag: the page and the rectangle when both sides are at least `min` (a click with no
    /// drag gives nothing), in the unit of the press.
    pub(crate) fn release(&mut self, min: f32) -> Option<(usize, Rect)> {
        let a = self.active.take()?;
        let rect = Rect::from_two_pos(a.start, a.end);
        (rect.width() >= min && rect.height() >= min).then_some((a.page, rect))
    }

    /// Drops the drag without a result.
    pub(crate) fn cancel(&mut self) {
        self.active = None;
    }

    /// Whether a drag is running.
    pub(crate) fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// The page's rectangle and the dragged rectangle, while dragging.
    pub(crate) fn current(&self) -> Option<(Rect, Rect)> {
        self.active.as_ref().map(|a| (a.bounds, Rect::from_two_pos(a.start, a.end)))
    }

    /// Outlines the dragged rectangle with marching ants, in a paint call.
    pub(crate) fn paint(&self, cx: &ToolContext<'_>) {
        if let Some((_, r)) = self.current() {
            cx.marching_ants(&[r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()], true);
        }
    }
}
