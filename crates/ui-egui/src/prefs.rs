//! Preferences (#2264): the categories in a list at the start of the dialog, the chosen one's
//! settings beside it. Each category is a module of its own, so a new setting edits one small
//! file. Settings are grouped in cards, each with its name above it, and a choice between page
//! displays, zooms or panel sides is a row of picture tiles ([`tiles`]). Every name comes from
//! the translation catalogs and wraps rather than widening the dialog, so a long translation
//! adds a line instead of pushing controls off the card.

mod accessibility;
mod commenting;
mod fill_sign;
mod general;
mod javascript;
mod page_display;
mod reading_colours;
pub(crate) mod tiles;
mod units_guides;

use egui::{Align, CornerRadius, Layout, Rect, RichText, Sense, Stroke, pos2, vec2};

use crate::PdfCraftApp;
use crate::theme::{self, Tokens};

/// The height the dialog keeps below its settings for the OK row: the button, and the space
/// above it.
pub(crate) const BUTTON_ROW: f32 = 52.0;

/// The least height the settings are given. The dialog around them takes about 80 more, so this
/// still fits the smallest window (520 pt) at the largest interface size (300%, which leaves
/// 173 pt): a taller floor would push OK off the screen again.
pub(crate) const MIN_SCROLL: f32 = 60.0;

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
    UnitsGuides,
    Accessibility,
    Commenting,
    FillSign,
    JavaScript,
}

impl PrefsPage {
    /// The categories, in the order the list shows them.
    pub const ALL: [Self; 8] = [
        Self::General,
        Self::Appearance,
        Self::PageDisplay,
        Self::UnitsGuides,
        Self::Accessibility,
        Self::Commenting,
        Self::FillSign,
        Self::JavaScript,
    ];

    /// The name the control channel uses: `dialog preferences:page-display`.
    pub fn slug(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Appearance => "appearance",
            Self::PageDisplay => "page-display",
            Self::UnitsGuides => "units-guides",
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
            Self::UnitsGuides => "Units and guides",
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
            Self::UnitsGuides => "ruler",
            Self::Accessibility => "accessibility",
            Self::Commenting => "message-square",
            Self::FillSign => "pen-line",
            Self::JavaScript => "code-xml",
        }
    }
}

/// The Preferences dialog: the title, the categories and the chosen one's settings, and OK.
/// Returns `true` to close.
pub(crate) fn show(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    let screen = ui.ctx().content_rect();
    let width = (screen.width() - 64.0).clamp(240.0, MAX_WIDTH);
    ui.set_width(width);
    // A fixed height, the same for every category: the dialog opens at its size and stays there.
    let height = (screen.height() - 120.0 - BUTTON_ROW).clamp(MIN_SCROLL, MAX_SCROLL);
    let title = row(ui, |ui| ui.label(RichText::new(tl!("Preferences")).font(theme::semibold(18.0))));
    ui.add_space(6.0);
    if width < COMPACT_WIDTH {
        // Too narrow for a list beside the settings: choose the category above them.
        let selected = app.preferences_page;
        // The dialog's title names the category picker for assistive technology.
        craft_ui::ComboBox::from_id_salt("preferences-category")
            .selected_text(selected.title())
            .width(width.min(280.0))
            .show_ui(ui, |ui| {
                for page in PrefsPage::ALL {
                    if ui.selectable_label(selected == page, page.title()).clicked() {
                        app.preferences_page = page;
                        ui.close();
                    }
                }
            })
            .response
            .labelled_by(title.id);
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
            let x = ui.cursor().min.x + if rtl { -4.0 } else { 4.0 };
            let top = ui.min_rect().top();
            ui.painter().vline(x, top..=top + height, Stroke::new(1.0, t.divider));
            ui.add_space(12.0);
            ui.allocate_ui_with_layout(vec2(ui.available_width(), height), column(), |ui| {
                settings(ui, app, t, height);
            });
        });
    }
    ui.add_space(10.0);
    crate::js_ui::buttons(ui, "OK", &[]).is_some()
}

/// A column of settings, its text against the start edge of the reading direction.
fn column() -> Layout {
    Layout::top_down(if crate::i18n::current().rtl() { Align::Max } else { Align::Min })
}

/// A row of controls in reading order: right to left in Arabic.
pub(crate) fn row<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let layout = if crate::i18n::current().rtl() { Layout::right_to_left(Align::Center) } else { Layout::left_to_right(Align::Center) };
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 0.0), layout, add).inner
}

/// [`row`] that wraps onto more lines when its controls don't fit.
pub(crate) fn wrapped<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let layout = if crate::i18n::current().rtl() { Layout::right_to_left(Align::Center) } else { Layout::left_to_right(Align::Center) };
    // No height of its own: the row grows to its lines instead of centring them in the page.
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 0.0), layout.with_main_wrap(true), add).inner
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

/// The chosen category's settings, under its name, scrolling inside `height`.
fn settings(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens, height: f32) {
    let page = app.preferences_page;
    // A scrollbar of its own: the floating one lies over the right edge of the cards (#3557).
    ui.scope(|ui| {
        ui.spacing_mut().scroll.floating = false;
        egui::ScrollArea::vertical()
            .id_salt(("preferences-page", page.slug()))
            .max_height(height)
            .min_scrolled_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width() - 4.0);
                ui.label(RichText::new(page.title()).font(theme::semibold(16.0)).color(t.text));
                ui.add_space(8.0);
                match page {
                    PrefsPage::General => general::show(ui, app, t),
                    PrefsPage::Appearance => {
                        crate::appearance::preferences_section(ui, app, t);
                        reading_colours::show(ui, app, t);
                    }
                    PrefsPage::PageDisplay => page_display::show(ui, app, t),
                    PrefsPage::UnitsGuides => units_guides::show(ui, app, t),
                    PrefsPage::Accessibility => accessibility::show(ui, app, t),
                    PrefsPage::Commenting => commenting::show(ui, app, t),
                    PrefsPage::FillSign => fill_sign::show(ui, app, t),
                    PrefsPage::JavaScript => javascript::show(ui, app, t),
                }
            });
    });
}

/// Whether the interface can draw a language picker's native name. On the web the browser's glyph
/// rasterizer draws scripts no embedded face has, so the native names stay; on native, ask the
/// fonts this build installed (including an installed fallback face, unless system fonts are off).
pub(super) fn picker_text_is_drawable(ui: &egui::Ui, text: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (ui, text);
        true
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        ui.fonts_mut(|fonts| fonts.has_glyphs(&egui::FontId::proportional(13.0), text))
    }
}

/// The date picker's displayed name. Its languages are the interface languages, so it uses the
/// same drawable-name fallback: a native name whose script this build has no face for would
/// otherwise abort under egui 0.37's `MissingGlyphPolicy::Panic` (for example Hindi without a
/// Devanagari face and installed system fonts turned off).
pub(super) fn date_language_picker_name(l: &pdfcraft_engine::dates::DateLanguage, drawable: impl FnOnce(&str) -> bool) -> &'static str {
    match crate::i18n::Lang::from_code(l.code) {
        Some(language) => language.picker_name(drawable),
        None => l.name,
    }
}

/// A group of settings in a card, with its name above it when it has one.
pub(crate) fn card<R>(ui: &mut egui::Ui, t: &Tokens, title: Option<&str>, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    if let Some(title) = title {
        ui.label(RichText::new(title).font(theme::semibold(13.0)).color(t.text));
        ui.add_space(2.0);
    }
    let inner = egui::Frame::new()
        .fill(t.card)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 8.0;
            add(ui)
        })
        .inner;
    ui.add_space(14.0);
    inner
}

/// An explanation under a setting: small, muted, wrapping.
pub(crate) fn hint(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).small().color(t.text_muted)).wrap());
}

/// A setting's name on a line of its own, above its control, so a long translation wraps rather
/// than pushes the control off the card. Returns the label for `labelled_by`.
pub(crate) fn field_label(ui: &mut egui::Ui, t: &Tokens, text: &str) -> egui::Response {
    ui.add(egui::Label::new(RichText::new(text).color(t.text_muted)).wrap())
}

/// Radio choices that wrap onto more lines in a narrow card or a long language.
pub(crate) fn radios<V: PartialEq + Copy>(ui: &mut egui::Ui, current: &mut V, choices: impl IntoIterator<Item = (V, &'static str)>) {
    wrapped(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        for (value, label) in choices {
            crate::widgets::radio_value(ui, current, value, label);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_category_round_trips_through_its_slug() {
        for page in PrefsPage::ALL {
            assert_eq!(PrefsPage::parse(page.slug()), Some(page));
            assert_eq!(PrefsPage::parse(&format!(" {} ", page.slug().to_uppercase())), Some(page));
        }
        assert_eq!(PrefsPage::parse("page display"), None);
        assert_eq!(PrefsPage::parse(""), None);
    }

    #[test]
    fn date_language_names_fall_back_when_the_native_name_cannot_be_drawn() {
        let date_language = |code: &str| pdfcraft_engine::dates::DATE_LANGUAGES.iter().find(|l| l.code == code).expect("date language");
        let hindi = date_language("hi");
        assert_eq!(date_language_picker_name(hindi, |_| true), hindi.name, "the native name is kept when it can be drawn");
        assert_eq!(date_language_picker_name(hindi, |_| false), "Hindi", "no Devanagari face: show the English name");
        let chinese = date_language("zh-hans");
        assert_eq!(date_language_picker_name(chinese, |_| false), "Simplified Chinese");
        for language in &pdfcraft_engine::dates::DATE_LANGUAGES {
            assert!(!date_language_picker_name(language, |_| false).is_empty(), "{}", language.code);
        }
    }
}
