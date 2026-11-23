//! Left tool panel (All tools + tool sub-panels, generated from the engine catalogue) and the
//! right-hand panels (Comments, Bookmarks, Pages, Fields, Layers, Attachments).

use pdfcraft_engine::catalog::{self, Availability, TOOL_GROUPS, ToolGroup, ToolItem};

use crate::{LeftPanel, PdfCraftApp, RightPanel};

pub(super) mod all_tools_ui;
mod attachments;
pub(crate) mod bookmarks;
pub(super) mod comments_panel;
mod edit;
mod fields;
pub(super) mod format_text_ui;
mod layers;
pub(super) mod organize_panel;
mod pages;
mod right;
pub(super) mod search_ui;
pub mod side_panels;
mod tools;

pub use bookmarks::BmAction;
pub use tools::left_panel;

// ───────────────────────────────────────────────────────────────────────────── right panels

pub(crate) enum Nav {
    Page(usize),
    /// A bookmark's destination: its page and where on it.
    Dest(usize, pdfcraft_render::DestView),
    /// A bookmark's action that goes nowhere in this document, run as a link's is.
    Action(pdfcraft_render::LinkTarget),
    Flash(usize, [f32; 4]),
}

/// A panel's empty state: a centred notice with the panel's icon and `text`. Every empty-panel message
/// goes through here.
pub(crate) fn empty_state(ui: &mut egui::Ui, icon: &'static str, text: &str) {
    use craft::ui::contract::Show as _;
    craft::ui::notices::Notice::new(craft::theme::variant::Tone::Neutral, text.to_string())
        .layout(craft::ui::notices::NoticeLayout::Centred)
        .icon(craft::ui::notices::NoticeIcon::Custom(craft::ui::primitives::IconSpec::svg(craft::behavior::IconId::new(icon))))
        .show(ui);
}

pub fn human_size(n: usize) -> String {
    match n {
        // A size that rounds to "1024.0 KB" reads as the next unit.
        n if n as f64 / 1024.0 >= 1023.95 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.1} KB", n as f64 / (1u64 << 10) as f64),
        n => byte_count(n),
    }
}

/// An exact size in bytes, with the count's own plural form.
pub fn byte_count(n: usize) -> String {
    crate::i18n::trn(crate::i18n::current(), n as u64, "{n} byte", "{n} bytes")
}

#[cfg(test)]
mod human_size_tests {
    use super::human_size;

    #[test]
    fn rounds_up_into_next_unit() {
        assert_eq!(human_size(1_048_524), "1023.9 KB");
        assert_eq!(human_size(1_048_525), "1.0 MB");
        assert_eq!(human_size(1_048_575), "1.0 MB");
        assert_eq!(human_size(1_048_576), "1.0 MB");
    }
}
