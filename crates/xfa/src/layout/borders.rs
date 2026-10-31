//! Keep a border's break policy until pagination has chosen its fragments (XFA 3.3,
//! chapter 7 “Borders and Splitting”, chapter 17 “border”). Decoration never chooses a break.

use super::common::{divide, moved, solid_extent};
use super::{Arc, Border, Item, Layouter, Rect, XfaError};

pub(super) struct BlockBorder {
    pub start: usize,
    pub end: usize,
    pub part: BorderPart,
}

pub(super) struct BorderPart {
    border: Arc<Border>,
    rect: Rect,
    open_top: bool,
    open_bottom: bool,
}

pub(super) enum BlockItem {
    Item(Item),
    Border(BorderPart),
}

impl BlockItem {
    pub fn solid_extent(&self) -> Option<(f64, f64)> {
        match self {
            Self::Item(item) => solid_extent(item),
            Self::Border(_) => None,
        }
    }

    pub fn divide(self, cut: f64) -> (Option<Self>, Option<Self>) {
        let part = match self {
            Self::Border(part) => part,
            Self::Item(item) => {
                let (a, b) = divide(item, cut);
                return (a.map(Self::Item), b.map(Self::Item));
            }
        };
        if part.rect.bottom() <= cut + 0.01 {
            (Some(Self::Border(part)), None)
        } else if part.rect.y >= cut - 0.01 {
            (None, Some(Self::Border(part)))
        } else {
            let above = BorderPart {
                border: Arc::clone(&part.border),
                rect: Rect::new(part.rect.x, part.rect.y, part.rect.w, cut - part.rect.y),
                open_top: part.open_top,
                open_bottom: part.border.break_open,
            };
            let below =
                BorderPart { rect: Rect::new(part.rect.x, cut, part.rect.w, part.rect.bottom() - cut), open_top: part.border.break_open, ..part };
            (Some(Self::Border(above)), Some(Self::Border(below)))
        }
    }

    pub fn place(self, l: &mut Layouter<'_>, dx: f64, dy: f64) -> Result<(), XfaError> {
        match self {
            Self::Item(item) => {
                // Already counted when first drawn; moving existing items is not new work.
                if let Some(page) = l.pages.last_mut() {
                    page.items.push(moved(item, dx, dy));
                }
                Ok(())
            }
            Self::Border(part) => l.draw_border_fragment(
                &part.border,
                Rect::new(part.rect.x + dx, part.rect.y + dy, part.rect.w, part.rect.h),
                part.open_top,
                part.open_bottom,
            ),
        }
    }
}

impl Layouter<'_> {
    pub(super) fn draw_border(&mut self, b: &Border, rect: Rect) -> Result<(), XfaError> {
        let start = self.item_count();
        self.draw_border_fragment(b, rect, false, false)?;
        let end = self.item_count();
        if end > start
            && let Some(borders) = &mut self.block_borders
        {
            borders.push(BlockBorder { start, end, part: BorderPart { border: Arc::new(b.clone()), rect, open_top: false, open_bottom: false } });
        }
        Ok(())
    }
}
