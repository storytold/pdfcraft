//! Preferences (#2264): the categories in a list at the start of the dialog, the chosen one's
//! settings beside it. The settings themselves are the cards of [`crate::dialogs::preferences`],
//! which every category shows a slice of; a choice between page displays, zooms or panel sides is
//! a row of picture tiles ([`tiles`]). Every name comes from the translation catalogs and wraps
//! rather than widening the dialog, so a long translation adds a line instead of pushing controls
//! off the card.

pub(crate) mod tiles;

use egui::{Align, CornerRadius, Layout, Rect, RichText, Sense, Stroke, pos2, vec2};

use crate::PdfCraftApp;
use crate::theme::{self, Tokens};

/// The most height the settings take: a tall window doesn't stretch the dialog past its
/// content, and every category opens at the same size, so the dialog doesn't jump between them.
const MAX_SCROLL: f32 = 600.0;

/// The widest the dialog gets, and below what width the category list becomes a drop-down
/// above the settings.
const MAX_WIDTH: f32 = 860.0;
const COMPACT_WIDTH: f32 = 600.0;

/// A category of Preferences.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PrefsPage {
    #[default]
    General,
    Appearance,
    PageDisplay,
    Accessibility,
    Commenting,
    FillSign,
    JavaScript,
}

impl PrefsPage {
    /// The categories, in the order the list shows them.
    pub const ALL: [Self; 7] =
        [Self::General, Self::Appearance, Self::PageDisplay, Self::Accessibility, Self::Commenting, Self::FillSign, Self::JavaScript];

    /// The name the control channel uses: `dialog preferences:page-display`.
    pub fn slug(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Appearance => "appearance",
            Self::PageDisplay => "page-display",
            Self::Accessibility => "accessibility",
            Self::Commenting => "commenting",
            Self::FillSign => "fill-sign",
            Self::JavaScript => "javascript",
        }
    }

    /// The category a [`slug`](Self::slug) names, in any case. `None` for anything else.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.slug().eq_ignore_ascii_case(value.trim()))
    }

    /// The English name, the catalog key.
    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::PageDisplay => "Page display",
            Self::Accessibility => "Accessibility",
            Self::Commenting => "Commenting",
            Self::FillSign => "Fill & Sign",
            Self::JavaScript => "JavaScript",
        }
    }

    /// The translated name. JavaScript is a name, the same in every language.
    fn title(self) -> &'static str {
        match self {
            Self::JavaScript => "JavaScript",
            other => tl!(other.label()),
        }
    }

    /// The Lucide icon beside the name.
    fn icon(self) -> &'static str {
        match self {
            Self::General => "settings",
            Self::Appearance => "palette",
            Self::PageDisplay => "book-open",
            Self::Accessibility => "accessibility",
            Self::Commenting => "message-square",
            Self::FillSign => "pen-line",
            Self::JavaScript => "code-xml",
        }
    }
}

/// The Preferences dialog: the title and search, the categories and the chosen one's settings,
/// and OK. Returns `true` to close.
pub(crate) fn show(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    let screen = ui.ctx().content_rect();
    let width = (screen.width() - 64.0).clamp(240.0, MAX_WIDTH);
    ui.set_width(width);
    // A fixed height, the same for every category: the dialog opens at its size and stays there.
    let height = (screen.height() - 120.0 - crate::dialogs::preferences::PREFERENCES_BUTTON_ROW)
        .clamp(crate::dialogs::preferences::PREFERENCES_MIN_SCROLL, MAX_SCROLL);
    crate::dialogs::preferences::preferences_header(ui, t);
    ui.add_space(6.0);
    if width < COMPACT_WIDTH {
        // Too narrow for a list beside the settings: choose the category above them.
        let selected = app.preferences_page;
        craft_ui::ComboBox::from_id_salt("preferences-category").selected_text(selected.title()).width(width.min(280.0)).show_ui(ui, |ui| {
            for page in PrefsPage::ALL {
                if ui.selectable_label(selected == page, page.title()).clicked() {
                    app.preferences_page = page;
                    ui.close();
                }
            }
        });
        ui.add_space(6.0);
        ui.with_layout(column(), |ui| settings(ui, app, t, height - 36.0));
    } else {
        let nav_w = (width * 0.27).clamp(170.0, 220.0);
        let rtl = crate::i18n::current().rtl();
        // The list sits at the start of the reading direction: on the right in Arabic.
        let row = if rtl { Layout::right_to_left(Align::TOP) } else { Layout::left_to_right(Align::TOP) };
        ui.allocate_ui_with_layout(vec2(width, height), row, |ui| {
            ui.allocate_ui_with_layout(vec2(nav_w, height), Layout::top_down(Align::Min), |ui| {
                ui.set_width(nav_w);
                egui::ScrollArea::vertical().id_salt("preferences-nav").max_height(height).show(ui, |ui| nav(ui, app, t));
            });
            // A divider between the list and the settings.
            let x = divider_x(ui.cursor(), rtl);
            let top = ui.min_rect().top();
            ui.painter().vline(x, top..=top + height, Stroke::new(1.0, t.divider));
            ui.add_space(12.0);
            ui.allocate_ui_with_layout(vec2(ui.available_width(), height), column(), |ui| {
                settings(ui, app, t, height);
            });
        });
    }
    crate::dialogs::preferences::preferences_buttons(ui)
}

/// Where the divider between the list and the settings goes: a little way into the free space
/// the cursor has left. Left to right that space starts at the cursor's `min.x` and runs on
/// without end; right to left it ends at `max.x` and runs back without end (egui sets the
/// cursor's `min.x` to minus infinity there), so the divider is measured from the edge that is
/// not infinite.
fn divider_x(cursor: egui::Rect, rtl: bool) -> f32 {
    if rtl { cursor.max.x - 4.0 } else { cursor.min.x + 4.0 }
}

/// A column of settings, its text against the start edge of the reading direction.
fn column() -> Layout {
    Layout::top_down(if crate::i18n::current().rtl() { Align::Max } else { Align::Min })
}

/// The list of categories: an icon and the name, the chosen one highlighted.
fn nav(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    let rtl = crate::i18n::current().rtl();
    let width = ui.available_width();
    for page in PrefsPage::ALL {
        let selected = app.preferences_page == page;
        let title = page.title();
        let text_w = width - 48.0;
        let mut job = egui::text::LayoutJob::simple(
            title.to_owned(),
            if selected { theme::semibold(13.0) } else { theme::regular(13.0) },
            if selected { t.accent_text } else { t.text },
            text_w.max(1.0),
        );
        job.direction = crate::bidi::ui_direction();
        let galley = ui.painter().layout_job(job);
        let h = (galley.size().y + 14.0).max(34.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(width, h), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, ui.is_enabled(), selected, title));
        if resp.clicked() {
            app.preferences_page = page;
        }
        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            let fill = if selected {
                t.accent_soft
            } else if resp.hovered() {
                t.hover
            } else {
                egui::Color32::TRANSPARENT
            };
            painter.rect_filled(rect, CornerRadius::same(6), fill);
            if selected {
                // A bar on the start edge marks the chosen category.
                let bar_x = if rtl { rect.right() - 3.0 } else { rect.left() };
                painter.rect_filled(Rect::from_min_size(pos2(bar_x, rect.top() + 7.0), vec2(3.0, h - 14.0)), 1.5, t.accent);
            }
            if resp.has_focus() {
                painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
            }
            let icon_x = if rtl { rect.right() - 22.0 } else { rect.left() + 22.0 };
            let icon_rect = Rect::from_center_size(pos2(icon_x, rect.center().y), vec2(18.0, 18.0));
            crate::icons::paint(ui, icon_rect, page.icon(), 17.0, if selected { t.accent_text } else { t.icon });
            let text_x = if rtl { rect.right() - 38.0 - galley.size().x } else { rect.left() + 38.0 };
            painter.galley(pos2(text_x, rect.center().y - galley.size().y / 2.0), galley, t.text);
        }
        theme::hand(resp);
        ui.add_space(2.0);
    }
}

/// The chosen category's settings, under its name, scrolling inside `height`. While the search
/// box holds a query every card is drawn instead, as the single-column dialog did: a match can
/// sit in any category, and the highlight has to reach it.
fn settings(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens, height: f32) {
    let page = app.preferences_page;
    let query: String = ui.data(|d| d.get_temp(egui::Id::unique("pdfcraft_preferences_search_query"))).unwrap_or_default();
    crate::dialogs::preferences_scroll(ui, height, |ui| {
        ui.set_width(ui.available_width() - 4.0);
        if query.trim().is_empty() {
            ui.label(RichText::new(page.title()).font(theme::semibold(16.0)).color(t.text));
            ui.add_space(8.0);
            crate::dialogs::preferences::preferences_category(ui, app, t, page);
        } else {
            crate::dialogs::preferences::preferences_body(ui, app, t);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The divider stays in view in both reading directions: the free side of the cursor is
    /// infinite, and the divider is measured from the other.
    #[test]
    fn the_divider_is_measured_from_the_cursor_edge_that_is_not_infinite() {
        let ltr = egui::Rect::from_min_max(egui::pos2(220.0, 0.0), egui::pos2(f32::INFINITY, 400.0));
        assert_eq!(divider_x(ltr, false), 224.0);
        let rtl = egui::Rect::from_min_max(egui::pos2(f32::NEG_INFINITY, 0.0), egui::pos2(640.0, 400.0));
        assert_eq!(divider_x(rtl, true), 636.0);
        assert!(divider_x(rtl, true).is_finite());
    }

    #[test]
    fn every_category_round_trips_through_its_slug() {
        for page in PrefsPage::ALL {
            assert_eq!(PrefsPage::parse(page.slug()), Some(page));
            assert_eq!(PrefsPage::parse(&format!(" {} ", page.slug().to_uppercase())), Some(page));
        }
        assert_eq!(PrefsPage::parse("page display"), None);
        assert_eq!(PrefsPage::parse(""), None);
    }
}
