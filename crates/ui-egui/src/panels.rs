//! Left tool panel (All tools + tool sub-panels, generated from the engine catalogue) and the
//! right-hand panels (Comments, Bookmarks, Pages, Fields, Layers, Attachments).

use egui::{Align, Align2, Color32, CornerRadius, Layout, Rect, Sense, Stroke, pos2, vec2};
use pdfcraft_engine::catalog::{self, Availability, TOOL_GROUPS, ToolGroup, ToolItem};
use pdfcraft_render::{DocInfo, FieldKind, OutlineItem};

use crate::theme::{self, Tokens};
use crate::{LeftPanel, PdfCraftApp, RightPanel, icons, widgets};

pub(super) mod all_tools_ui;
mod bookmarks;
pub(super) mod comments_panel;
mod edit;
pub(super) mod format_text_ui;
pub(super) mod organize_panel;
pub(super) mod page_selection_ui;
mod pages;
mod right;
pub(super) mod search_ui;
pub mod side_panels;
mod tools;

pub use bookmarks::BmAction;
pub(crate) use bookmarks::ellipsized_prefix;
pub use right::right_panel;
pub use tools::left_panel;
pub(crate) use tools::panel_header;

// ───────────────────────────────────────────────────────────────────────────── right panels

pub(crate) enum Nav {
    Page(usize),
    /// A bookmark's destination: its page and where on it.
    Dest(usize, pdfcraft_render::DestView),
    Flash(usize, [f32; 4]),
}

pub fn human_size(n: usize) -> String {
    match n {
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.1} KB", n as f64 / (1u64 << 10) as f64),
        n => byte_count(n),
    }
}

/// An exact size in bytes, with the count's own plural form.
pub fn byte_count(n: usize) -> String {
    crate::i18n::trn(crate::i18n::current(), n as u64, "{n} byte", "{n} bytes")
}

#[cfg(test)]
mod tool_items {
    use super::edit::{EDIT_CARET_INSET, edit_label_width};
    use super::tools::{CARET_GAP, LABEL_INSET, item_label_width};
    use egui::{Rect, pos2};

    /// A right-to-left label ends where its width does: for an item that opens a menu (Header
    /// and footer, Watermark) that edge must clear the caret before the chip, which covered the
    /// first letter of the Arabic label. Items without a menu keep the width they had.
    #[test]
    fn a_label_at_its_full_width_clears_the_menu_caret() {
        let body = Rect::from_min_max(pos2(15.0, 0.0), pos2(257.0, 34.0));
        let chip = Rect::from_min_max(pos2(215.0, 9.0), pos2(247.0, 25.0));
        let label_end = |menu| body.left() + LABEL_INSET + item_label_width(body, chip, menu);
        let caret_left = chip.left() - CARET_GAP - 3.5;
        assert!(label_end(true) < caret_left, "the label reaches {} and the caret starts at {caret_left}", label_end(true));
        assert_eq!(item_label_width(body, chip, false), chip.left() - body.left() - 42.0);
    }

    /// The Edit panel's rows (Header and footer, Watermark, Background) put the caret near the
    /// row's end instead of before a chip; their label must clear it too.
    #[test]
    fn an_edit_panel_label_at_its_full_width_clears_the_menu_caret() {
        let row = egui::Rect::from_min_max(pos2(15.0, 0.0), pos2(257.0, 34.0));
        let label_end = |menu| row.left() + LABEL_INSET + edit_label_width(row.width(), menu);
        let caret_left = row.right() - EDIT_CARET_INSET - 3.5;
        assert!(label_end(true) < caret_left, "the label reaches {} and the caret starts at {caret_left}", label_end(true));
        assert_eq!(edit_label_width(row.width(), false), row.width() - 42.0);
    }
}
