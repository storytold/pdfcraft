//! Preferences dialog: card-based settings interface organized into clean, modular cards
//! with segmented pill controls and real-time search filtering.

use egui::{Color32, CornerRadius, Margin, Rect, RichText, Sense, Stroke, pos2, vec2};

use crate::PdfCraftApp;
use crate::canvas::{Fit, PageLayout};
use crate::motion::AnimationEffects;
use crate::scroll_speed::ScrollSpeed;
use crate::side_panels::ToolsSide;
use crate::theme::{self, ThemeKind, Tokens};
use crate::units::Unit;
use crate::widgets::{self, icons};

/// The height the Preferences dialog keeps below its scrolling settings for the buttons row.
pub(crate) const PREFERENCES_BUTTON_ROW: f32 = 52.0;

/// The least height the scrolling settings are given in very small windows.
pub(crate) const PREFERENCES_MIN_SCROLL: f32 = 60.0;

/// A segmented pill control for mutually exclusive choices.
///
/// Draws a rounded container with options inside. The active option is highlighted
/// with the accent colour. Emits AccessKit RadioButton widget info with the option's label.
pub fn segmented_pill_choice<T: PartialEq + Copy>(ui: &mut egui::Ui, t: &Tokens, current: &mut T, options: &[(T, &str)]) -> bool {
    let with_a11y: Vec<(T, &str, &str)> = options.iter().map(|&(v, l)| (v, l, l)).collect();
    segmented_pill_choice_a11y(ui, t, current, &with_a11y)
}

/// A segmented pill selection bar with separate accessibility labels.
pub fn segmented_pill_choice_a11y<T: PartialEq + Copy>(ui: &mut egui::Ui, t: &Tokens, current: &mut T, options: &[(T, &str, &str)]) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(t.field)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::symmetric(2, 2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for &(value, label, a11y_label) in options {
                    let is_active = *current == value;
                    let font = theme::medium(12.0);
                    let text_color = if is_active { Color32::WHITE } else { t.text };
                    let label_galley = crate::bidi::layout_ui(ui, label.to_owned(), font, text_color);
                    let w = label_galley.size().x;
                    let (rect, resp) = ui.allocate_exact_size(vec2(w + 18.0, 24.0), Sense::click());
                    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, is_active, a11y_label));

                    if is_active {
                        ui.painter().rect(rect, CornerRadius::same(12), t.accent, Stroke::NONE, egui::StrokeKind::Inside);
                    } else {
                        let press = theme::Press::track(ui, &resp);
                        let fill = press.fill(Color32::TRANSPARENT, t.hover, t.pressed);
                        ui.painter().rect(rect, CornerRadius::same(12), fill, Stroke::NONE, egui::StrokeKind::Inside);
                    }
                    ui.painter().galley(rect.center() - label_galley.size() / 2.0, label_galley, text_color);

                    let resp = theme::hand(resp);
                    if resp.clicked() && *current != value {
                        *current = value;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// An independent toggle pill button (e.g. for standalone toggles).
#[allow(dead_code)]
pub fn toggle_pill(ui: &mut egui::Ui, t: &Tokens, active: &mut bool, label: &str) -> bool {
    let mut changed = false;
    let is_active = *active;
    let font = theme::medium(12.0);
    let text_color = if is_active { Color32::WHITE } else { t.text };
    let label_galley = crate::bidi::layout_ui(ui, label.to_owned(), font, text_color);
    let w = label_galley.size().x;
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 20.0, 26.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, is_active, label));

    let (fill, stroke) = if is_active {
        (t.accent, Stroke::NONE)
    } else {
        let press = theme::Press::track(ui, &resp);
        (press.fill(t.field, t.hover, t.pressed), Stroke::new(1.0, t.border))
    };
    ui.painter().rect(rect, CornerRadius::same(13), fill, stroke, egui::StrokeKind::Inside);
    ui.painter().galley(rect.center() - label_galley.size() / 2.0, label_galley, text_color);

    let resp = theme::hand(resp);
    if resp.clicked() {
        *active = !*active;
        changed = true;
    }
    changed
}

/// A standard card container for a Preferences section.
/// Groups related settings with clean margins, subtle border and rounded corners.
pub fn preference_card<R>(ui: &mut egui::Ui, t: &Tokens, title: &str, icon: Option<&str>, content: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let card_fill = if t.kind == ThemeKind::Light { Color32::from_rgb(0xF7, 0xF8, 0xFA) } else { Color32::from_rgb(0x24, 0x24, 0x28) };
    egui::Frame::new()
        .fill(card_fill)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if let Some(ic) = icon {
                    ui.add(icons::image(ic, 16.0, t.accent));
                }
                ui.label(RichText::new(title).font(theme::semibold(13.5)).color(t.text));
            });
            ui.add_space(8.0);
            content(ui)
        })
        .inner
}

fn matches_search(query: &str, title: &str, keywords: &[&str]) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    title.to_lowercase().contains(&q) || keywords.iter().any(|k| k.to_lowercase().contains(&q))
}

/// Whether the interface can draw a language picker's native name.
fn picker_text_is_drawable(ui: &egui::Ui, text: &str) -> bool {
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

/// The date picker's displayed name.
pub(crate) fn date_language_picker_name(l: &pdfcraft_engine::dates::DateLanguage, drawable: impl FnOnce(&str) -> bool) -> &'static str {
    match crate::i18n::Lang::from_code(l.code) {
        Some(language) => language.picker_name(drawable),
        None => l.name,
    }
}

/// Interactive search box widget at the top of Preferences.
fn preference_search_box_at(ui: &mut egui::Ui, t: &Tokens, query: &mut String, rect: Rect) {
    let stroke = Stroke::new(1.0, t.border);
    ui.painter().rect(rect, CornerRadius::same(15), t.field, stroke, egui::StrokeKind::Inside);

    // Magnifying glass icon: 14pt icon centered vertically at (rect.min.x + 16, rect.center().y)
    let icon_rect = Rect::from_center_size(pos2(rect.min.x + 16.0, rect.center().y), vec2(14.0, 14.0));
    icons::paint(ui, icon_rect, "search", 14.0, t.text_muted);

    let clear_w = if query.is_empty() { 12.0 } else { 30.0 };
    let edit_rect = Rect::from_min_max(pos2(rect.min.x + 28.0, rect.min.y), pos2(rect.max.x - clear_w, rect.max.y));
    let mut child_ui = ui.new_child(egui::UiBuilder::new().max_rect(edit_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let edit = child_ui.add(
        egui::TextEdit::singleline(query)
            .hint_text(tl!("Search settings…"))
            .frame(egui::Frame::NONE)
            .align(egui::Align2::LEFT_CENTER)
            .desired_width(edit_rect.width()),
    );
    edit.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, tl!("Search settings…")));

    if !query.is_empty() {
        let clear_rect = Rect::from_center_size(pos2(rect.max.x - 16.0, rect.center().y), vec2(16.0, 16.0));
        let clear_resp = ui.interact(clear_rect, ui.auto_id_with("clear_search"), Sense::click());
        clear_resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!("Clear search")));
        icons::paint(ui, clear_rect, "x", 12.0, if clear_resp.hovered() { t.text } else { t.text_muted });
        if clear_resp.clicked() {
            query.clear();
        }
    }

    if edit.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        query.clear();
    }
}

/// The body of the Preferences dialog with card layout and search filtering.
pub(crate) fn preferences_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    let search_id = egui::Id::unique("pdfcraft_preferences_search_query");
    let mut query: String = ui.data(|d| d.get_temp(search_id)).unwrap_or_default();

    // Top Header: Title on the left, Search bar on the right.
    // Allocate the full available width so the search bar aligns flush with the card boxes below.
    let full_w = ui.available_width();
    let header_h = 30.0;
    let (header_rect, _) = ui.allocate_exact_size(vec2(full_w, header_h), Sense::hover());

    // Title on the left, vertically centered in the header row:
    let title_rect = Rect::from_min_max(header_rect.min, pos2(header_rect.min.x + 240.0, header_rect.max.y));
    let mut title_ui = ui.new_child(egui::UiBuilder::new().max_rect(title_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    title_ui.label(RichText::new(tl!("Preferences")).font(theme::semibold(18.0)).color(t.text));

    // Search bar on the right, right edge flush with the card box boundary:
    let search_w = 210.0;
    let search_rect = Rect::from_min_max(pos2(header_rect.max.x - search_w, header_rect.min.y), header_rect.max);
    let before = query.clone();
    preference_search_box_at(ui, t, &mut query, search_rect);
    if query != before {
        ui.data_mut(|d| d.insert_temp(search_id, query.clone()));
    }

    ui.add_space(10.0);

    let mut matches_count = 0;

    // Card 1: General & Interface
    let kw1 = &[
        "general",
        "interface",
        "language",
        "size",
        "tools",
        "panel",
        "side",
        "animation",
        "motion",
        "reopen",
        "session",
        "restore",
        "reload",
        "recent",
        "title",
    ];
    let title1 = tl!("General & Interface");
    if matches_search(&query, title1, kw1) {
        matches_count += 1;
        preference_card(ui, t, title1, Some("settings"), |ui| {
            card_general(ui, app, t);
        });
        ui.add_space(12.0);
    }

    // Card 2: Appearance & Theme
    let kw2 = &["appearance", "theme", "dark", "light", "shadow", "edge", "highlight", "field", "reading", "colors", "colours", "custom", "contrast"];
    let title2 = tl_ctx!("theme", "Appearance");
    if matches_search(&query, title2, kw2) {
        matches_count += 1;
        preference_card(ui, t, title2, Some("sun"), |ui| {
            card_appearance(ui, app, t);
        });
        ui.add_space(12.0);
    }

    // Card 3: Documents and View
    let kw3 =
        &["documents", "view", "workspace", "mode", "page", "display", "continuous", "scrolling", "gaps", "zoom", "fit", "units", "scroll", "speed"];
    let title3 = tl!("Documents and view");
    if matches_search(&query, title3, kw3) {
        matches_count += 1;
        preference_card(ui, t, title3, Some("file-text"), |ui| {
            card_documents_and_view(ui, app, t);
        });
        ui.add_space(12.0);
    }

    // Card 4: Comment
    let kw4 = &["comment", "comments", "identity", "name", "author", "preview", "hover", "highlight"];
    let title4 = tl!("Comment");
    if matches_search(&query, title4, kw4) {
        matches_count += 1;
        preference_card(ui, t, title4, Some("message-square-text"), |ui| {
            card_identity(ui, app, t);
        });
        ui.add_space(12.0);
    }

    // Card 5: Fill & Sign
    let kw5 = &["fill", "sign", "flatten", "date", "format", "pattern", "today", "language", "token"];
    let title5 = tl!("Fill & Sign");
    if matches_search(&query, title5, kw5) {
        matches_count += 1;
        preference_card(ui, t, title5, Some("pen-tool"), |ui| {
            card_fill_and_sign(ui, app, t);
        });
        ui.add_space(12.0);
    }

    // Card 6: JavaScript
    let kw6 = &["javascript", "js", "scripts", "sandbox", "security", "acrobat"];
    let title6 = "JavaScript";
    if matches_search(&query, title6, kw6) {
        matches_count += 1;
        preference_card(ui, t, title6, Some("code-xml"), |ui| {
            card_javascript(ui, app, t);
        });
        ui.add_space(12.0);
    }

    // Empty search state when nothing matches
    if matches_count == 0 {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
            icons::paint(ui, rect, "search", 32.0, t.text_muted);
            ui.add_space(8.0);
            ui.label(RichText::new(format!("No settings match \"{query}\"")).font(theme::medium(14.0)).color(t.text_muted));
            ui.add_space(10.0);
            if widgets::pill_button(ui, tl!("Clear search"), false).clicked() {
                ui.data_mut(|d| d.insert_temp(search_id, String::new()));
                ui.ctx().request_repaint();
            }
        });
    }
}

/// Content for Card 1: General & Interface
fn card_general(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    ui.horizontal(|ui| {
        let label = ui.label(tl!("Interface language"));
        let (selected, picker) = {
            let drawable = |text: &str| picker_text_is_drawable(ui, text);
            let selected = crate::i18n::Lang::from_code(&app.language)
                .map_or_else(|| tl!("Auto").to_owned(), |language| language.picker_name(drawable).to_owned());
            let picker = crate::i18n::Lang::for_picker_by(|language| language.picker_name(drawable))
                .into_iter()
                .map(|language| (language, language.picker_name(drawable)))
                .collect::<Vec<_>>();
            (selected, picker)
        };
        let before = app.language.clone();
        craft_ui::ComboBox::from_id_salt("interface-language")
            .selected_text(selected)
            .width(160.0)
            .show_ui(ui, |ui| {
                if ui.selectable_value(&mut app.language, crate::i18n::AUTO.to_string(), tl!("Auto")).clicked() {
                    ui.close();
                }
                for (language, name) in &picker {
                    if ui.selectable_value(&mut app.language, language.code().to_string(), *name).clicked() {
                        ui.close();
                    }
                }
            })
            .response
            .labelled_by(label.id);
        if app.language != before {
            crate::i18n::set_current(crate::i18n::Lang::from_pref(&app.language));
        }

        ui.add_space(16.0);
        use crate::ui_scale;
        let label_size = ui.label(tl!("Interface size"));
        let native = ui.ctx().native_pixels_per_point().unwrap_or(1.0);
        let auto_percent = ui_scale::percent(ui_scale::drawn(None, app.system_text_scale, native));
        let auto = crate::i18n::fmt(tl!("Auto ({percent}%)"), &[("percent", &auto_percent.to_string())]);
        let selected = app.ui_scale.map_or_else(|| auto.clone(), |f| format!("{}%", ui_scale::percent(f)));
        craft_ui::ComboBox::from_id_salt("interface-size")
            .selected_text(selected)
            .width(130.0)
            .show_ui(ui, |ui| {
                if ui.selectable_value(&mut app.ui_scale, None, auto).clicked() {
                    ui.close();
                }
                for p in ui_scale::PRESETS {
                    let factor = f32::from(p) / 100.0;
                    if ui.selectable_label(app.ui_scale.is_some_and(|f| ui_scale::percent(f) == u32::from(p)), format!("{p}%")).clicked() {
                        app.ui_scale = Some(factor);
                        ui.close();
                    }
                }
            })
            .response
            .labelled_by(label_size.id);
    });
    ui.label(
        RichText::new(tl!("Auto follows the system's text size. Pages always render at the display's full resolution.")).small().color(t.text_muted),
    );
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.label(tl!("Tools panel side"));
        let right = app.side_panels.tools_on_right();
        let mut side = if right { ToolsSide::Right } else { ToolsSide::Left };
        if segmented_pill_choice(ui, t, &mut side, &[(ToolsSide::Left, tl!("Left")), (ToolsSide::Right, tl!("Right"))]) {
            app.side_panels.tools_side = side;
        }

        ui.add_space(16.0);
        ui.label(tl!("Navigation rail side"));
        let rail_right = app.side_panels.rail_on_right();
        let mut rside = if rail_right { ToolsSide::Right } else { ToolsSide::Left };
        if segmented_pill_choice_a11y(
            ui,
            t,
            &mut rside,
            &[(ToolsSide::Left, tl!("Left"), tl!("Navigation rail on left")), (ToolsSide::Right, tl!("Right"), tl!("Navigation rail on right"))],
        ) {
            app.side_panels.rail_side = rside;
        }
    });
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.label(tl!("Animation effects"));
        segmented_pill_choice(
            ui,
            t,
            &mut app.animation_effects,
            &[
                (AnimationEffects::System, tl_ctx!("animation effects", AnimationEffects::System.label())),
                (AnimationEffects::On, tl_ctx!("animation effects", AnimationEffects::On.label())),
                (AnimationEffects::Off, tl_ctx!("animation effects", AnimationEffects::Off.label())),
            ],
        );
    });
    ui.add_space(6.0);

    #[cfg(not(target_arch = "wasm32"))]
    ui.add(widgets::checkbox(&mut app.reopen_last_session, tl!("Reopen the files that were open when PdfCraft last closed")));
    #[cfg(not(target_arch = "wasm32"))]
    ui.add(widgets::checkbox(&mut app.restore_last_view, tl!("Restore the last view (page, zoom and page display) when a document is opened again")));
    #[cfg(not(target_arch = "wasm32"))]
    ui.add(widgets::checkbox(&mut app.reload_changed_files, tl!("Reload documents when their files change on disk")));
    ui.add(widgets::checkbox(&mut app.show_secured_notice, tl!("Show a notice when a document is secured")));

    #[cfg(not(target_arch = "wasm32"))]
    {
        ui.horizontal(|ui| {
            let label = ui.label(tl!("Recent files to keep"));
            ui.add(egui::DragValue::new(&mut app.recent_limit).range(0..=crate::RECENT_FILES_MAX)).labelled_by(label.id);
        });
        ui.label(RichText::new(tl!("Lowering this forgets the oldest files. At 0 there is no list.")).small().color(t.text_muted));
    }

    if app.window_controls.is_some() {
        ui.add(widgets::checkbox(&mut app.system_title_bar, tl!("Use system title bar"))).on_hover_text(tl!(
            "Show a basic title bar for the window buttons instead of the tab strip. GNOME doesn't draw title bars for apps, so this one doesn't follow its theme."
        ));
    }
}

/// Content for Card 2: Appearance & Theme
fn card_appearance(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    if ui.add(widgets::checkbox(&mut app.show_page_edge_shadows, tl!("Show Page Edge Shadows"))).changed() {
        ui.ctx().request_repaint();
    }
    ui.add_space(4.0);
    crate::appearance::preferences_section(ui, app, t);
    ui.add_space(8.0);
    reading_colours_section(ui, app, t);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DisplayModeChoice {
    Single,
    TwoUp,
    Continuous,
}

/// Content for Card 3: Documents and View
fn card_documents_and_view(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    ui.horizontal(|ui| {
        ui.label(tl!("Default workspace mode"));
        segmented_pill_choice(
            ui,
            t,
            &mut app.default_mode,
            &[
                (crate::Mode::AllTools, tl!("All tools")),
                (crate::Mode::Read, tl!("Read")),
                (crate::Mode::Edit, tl!("Edit")),
                (crate::Mode::Convert, tl!("Convert")),
                (crate::Mode::Sign, tl!("E-Sign")),
            ],
        );
    });
    ui.label(
        RichText::new(tl!("Used when opening PDFs. An explicit launch or control mode takes precedence for the session."))
            .small()
            .color(t.text_muted),
    );
    ui.add_space(6.0);

    let defaults = &mut app.view_defaults;
    ui.horizontal(|ui| {
        ui.label(tl!("Default page display"));
        let mut display_choice = if defaults.continuous {
            DisplayModeChoice::Continuous
        } else if defaults.layout == PageLayout::TwoUp {
            DisplayModeChoice::TwoUp
        } else {
            DisplayModeChoice::Single
        };
        if segmented_pill_choice(
            ui,
            t,
            &mut display_choice,
            &[
                (DisplayModeChoice::Single, tl!("Single page")),
                (DisplayModeChoice::TwoUp, tl!("Two-page view")),
                (DisplayModeChoice::Continuous, tl!("Continuous scrolling")),
            ],
        ) {
            match display_choice {
                DisplayModeChoice::Single => {
                    defaults.layout = PageLayout::Single;
                    defaults.continuous = false;
                }
                DisplayModeChoice::TwoUp => {
                    defaults.layout = PageLayout::TwoUp;
                    defaults.continuous = false;
                }
                DisplayModeChoice::Continuous => {
                    defaults.layout = PageLayout::Single;
                    defaults.continuous = true;
                }
            }
        }
    });
    ui.add(widgets::checkbox(&mut defaults.show_page_gaps, tl!("Show gaps between pages")));
    ui.label(RichText::new(tl!("Hide gaps to join page edges. Margins inside the PDF remain unchanged.")).small().color(t.text_muted));
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.label(tl!("Default zoom"));
        segmented_pill_choice(
            ui,
            t,
            &mut defaults.fit,
            &[(Fit::Width, tl!("Fit to width")), (Fit::Page, tl!("Zoom to page level")), (Fit::None, tl!("Custom"))],
        );
        let mut percent = defaults.zoom * 100.0;
        if ui.add(egui::DragValue::new(&mut percent).range(8.0..=6400.0).max_decimals(0).suffix("%")).changed() {
            (defaults.fit, defaults.zoom) = (Fit::None, percent / 100.0);
        }
    });
    ui.label(
        RichText::new(tl!("Used when a PDF doesn't ask for a layout or zoom. Continuous scrolling never snaps between pages."))
            .small()
            .color(t.text_muted),
    );
    ui.add(widgets::checkbox(&mut app.always_default_view, tl!("Always use my default zoom and page layout")));
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.label(tl!("Units"));
        segmented_pill_choice(
            ui,
            t,
            &mut app.units,
            &[
                (Unit::Inches, Unit::Inches.label()),
                (Unit::Millimetres, Unit::Millimetres.label()),
                (Unit::Centimetres, Unit::Centimetres.label()),
                (Unit::Points, Unit::Points.label()),
            ],
        );
    });
    ui.label(RichText::new(tl!("For page sizes and margins.")).small().color(t.text_muted));
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.label(tl!("Scroll speed (mouse wheel and arrow keys)"));
        segmented_pill_choice(
            ui,
            t,
            &mut app.scroll_speed,
            &[
                (ScrollSpeed::Slow, tl_ctx!("scroll speed", ScrollSpeed::Slow.label())),
                (ScrollSpeed::Normal, tl_ctx!("scroll speed", ScrollSpeed::Normal.label())),
                (ScrollSpeed::Fast, tl_ctx!("scroll speed", ScrollSpeed::Fast.label())),
            ],
        );
    });
}

/// Content for Card 4: Identity
fn card_identity(ui: &mut egui::Ui, app: &mut PdfCraftApp, _t: &Tokens) {
    ui.horizontal(|ui| {
        let label = ui.label(tl!("Name on new comments"));
        ui.add(crate::widgets::line(&mut app.comment_prefs.author).desired_width(240.0).char_limit(crate::MAX_AUTHOR_CHARS)).labelled_by(label.id);
    });
    ui.add(widgets::checkbox(&mut app.comment_prefs.hide_empty_highlight_hover, tl!("Hide hover previews for highlights without comment text")));
}

/// Content for Card 5: Fill & Sign
fn card_fill_and_sign(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    ui.add(widgets::checkbox(&mut app.flatten_fill_sign_on_save, tl!("Flatten Fill & Sign when saving")));
    ui.label(RichText::new(tl!("Text, marks and signatures become part of the page. Other comments stay editable.")).small().color(t.text_muted));
    ui.add_space(8.0);
    date_format(ui, app, t);
}

/// Content for Card 6: JavaScript
fn card_javascript(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    let mut on = app.session.javascript();
    if ui.add(widgets::checkbox(&mut on, tl!("Enable Acrobat JavaScript"))).changed() {
        app.session.set_javascript(on);
    }
    ui.label(
        RichText::new(tl!(
            "Scripts run in a sandbox without file or network access. A script that asks to open a web page needs your permission first. With JavaScript off, Acrobat's standard format, validate and calculate functions still work."
        ))
        .small()
        .color(t.text_muted),
    );
}

/// Preferences dialog buttons: Cancel and OK on the right. Returns true to close.
pub(crate) fn preferences_buttons(ui: &mut egui::Ui) -> bool {
    ui.add_space(10.0);
    let mut close = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("OK"), true).clicked() {
                close = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                close = true;
            }
        });
    });
    close
}

/// Fill & Sign date pattern and date language picker.
fn date_format(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    use pdfcraft_engine::dates::{DATE_FORMATS, DATE_LANGUAGES, MAX_DATE_FORMAT_CHARS};
    ui.label(RichText::new(tl!("Date format")).font(theme::semibold(12.5)));
    let sample = |app: &PdfCraftApp, f: &str| format!("{}   {f}", app.date_text(Some(f)).unwrap_or_default());
    ui.horizontal(|ui| {
        let label = ui.label(tl!("Fill & Sign dates"));
        let current = app.session.date_format().to_string();
        let selected = if DATE_FORMATS.contains(&current.as_str()) { sample(app, &current) } else { tl!("Custom").to_string() };
        craft_ui::ComboBox::from_id_salt("date-format")
            .selected_text(selected)
            .width(240.0)
            .show_ui(ui, |ui| {
                for f in DATE_FORMATS {
                    if ui.selectable_label(current == f, sample(app, f)).clicked() && app.session.set_date_format(f).is_ok() {
                        app.date_format_draft = None;
                        ui.close();
                    }
                }
            })
            .response
            .labelled_by(label.id);
    });
    ui.horizontal(|ui| {
        let label = ui.label(tl!("Pattern"));
        let mut text = app.date_format_draft.clone().unwrap_or_else(|| app.session.date_format().to_string());
        let edit = ui.add(egui::TextEdit::singleline(&mut text).desired_width(160.0).char_limit(MAX_DATE_FORMAT_CHARS)).labelled_by(label.id);
        if edit.changed() {
            let _ = app.session.set_date_format(&text);
            app.date_format_draft = Some(text.clone());
        }
        if !edit.has_focus() && pdfcraft_engine::dates::check_date_format(&text).is_ok() {
            app.date_format_draft = None;
        }
        match app.date_text(Some(&text)) {
            Ok(today) => ui.label(RichText::new(crate::i18n::fmt(tl!("Today: {date}"), &[("date", &today)])).color(t.text_muted)),
            Err(e) => ui.label(RichText::new(e).color(Color32::from_rgb(0xD1, 0x3B, 0x3B))),
        };
    });
    let bad = app.date_text(None).map(|today| pdfcraft_engine::dates::unwritable(&today)).unwrap_or_default();
    if !bad.is_empty() {
        ui.label(
            RichText::new(crate::i18n::fmt(
                tl!(
                    "{chars} can't be written into the PDF yet: Fill & Sign text is Western European only. Pick a numeric format such as dd.mm.yyyy."
                ),
                &[("chars", &bad)],
            ))
            .color(Color32::from_rgb(0xD1, 0x3B, 0x3B)),
        );
    }
    ui.horizontal(|ui| {
        let label = ui.label(tl!("Language"));
        let (follow, selected, current, names) = {
            let drawable = |text: &str| picker_text_is_drawable(ui, text);
            let interface = crate::i18n::current().picker_name(drawable);
            let follow = crate::i18n::fmt(tl!("Same as interface ({language})"), &[("language", interface)]);
            let current = app.session.date_language().map(str::to_string);
            let selected = DATE_LANGUAGES
                .iter()
                .find(|l| current.as_deref() == Some(l.code))
                .map_or_else(|| follow.clone(), |l| date_language_picker_name(l, drawable).to_string());
            let names = DATE_LANGUAGES.iter().map(|l| (l, date_language_picker_name(l, drawable))).collect::<Vec<_>>();
            (follow, selected, current, names)
        };
        craft_ui::ComboBox::from_id_salt("date-language")
            .selected_text(selected)
            .width(240.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(current.is_none(), follow).clicked() && app.session.set_date_language(None).is_ok() {
                    ui.close();
                }
                for (l, name) in &names {
                    if ui.selectable_label(current.as_deref() == Some(l.code), *name).clicked() && app.session.set_date_language(Some(l.code)).is_ok()
                    {
                        ui.close();
                    }
                }
            })
            .response
            .labelled_by(label.id);
    });
    ui.label(
        RichText::new(crate::i18n::fmt(
            tl!("yyyy or yy year · m or mm month · mmm or mmmm month name · d or dd day · ddd or dddd weekday. H, h, M, s and t are time letters and can't be used on their own: put {escape} before a letter to show it as is."),
            &[("escape", "\\")],
        ))
        .small()
        .color(t.text_muted),
    );
}

/// Reading colours settings: presets, preview and custom replacement options.
fn reading_colours_section(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    ui.label(RichText::new(tl!("Reading colours")).font(theme::semibold(13.0)));
    let mode = app.reading_colors.mode_name();
    ui.horizontal_wrapped(|ui| {
        for (name, label, colors) in [
            ("off", "Off", pdfcraft_render::ReadingColors::default()),
            ("black-on-white", "Black on white", pdfcraft_render::ReadingColors::BLACK_ON_WHITE),
            ("white-on-black", "White on black", pdfcraft_render::ReadingColors::WHITE_ON_BLACK),
            ("yellow-on-black", "Yellow on black", pdfcraft_render::ReadingColors::YELLOW_ON_BLACK),
            ("green-on-black", "Green on black", pdfcraft_render::ReadingColors::GREEN_ON_BLACK),
        ] {
            let params = colors.params.unwrap_or(pdfcraft_render::ReadingParams {
                text: [0, 0, 0],
                background: [255, 255, 255],
                only_black: false,
                line_art: true,
            });
            ui.vertical(|ui| {
                ui.set_width(100.0);
                let sample = RichText::new("Aa").size(20.0).color(Color32::from_rgb(params.text[0], params.text[1], params.text[2]));
                let button = egui::Button::new(sample)
                    .fill(Color32::from_rgb(params.background[0], params.background[1], params.background[2]))
                    .min_size(vec2(90.0, 40.0))
                    .stroke(Stroke::new(if mode == name { 2.0 } else { 1.0 }, if mode == name { t.accent } else { t.border }));
                let response = ui.add(button);
                response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, mode == name, tl!(label)));
                if response.clicked() {
                    app.set_reading_colors(colors);
                }
                ui.label(tl!(label));
            });
        }
    });
    ui.add_space(4.0);
    if let Some(params) = app.reading_colors.params {
        let text = Color32::from_rgb(params.text[0], params.text[1], params.text[2]);
        let background = Color32::from_rgb(params.background[0], params.background[1], params.background[2]);
        egui::Frame::new().fill(background).inner_margin(12.0).corner_radius(CornerRadius::same(6)).show(ui, |ui| {
            ui.set_min_width((ui.available_width() - 24.0).max(0.0));
            ui.label(RichText::new(tl!("Reading colours")).size(16.0).color(text));
            ui.label(RichText::new(tl!("Colours are remapped while rendering; the document itself never changes.")).color(text));
        });
    } else {
        ui.label(tl!("Original"));
    }
    egui::CollapsingHeader::new(tl!("Custom")).default_open(mode == "custom").show(ui, |ui| {
        let mut colors = if app.reading_colors.params.is_some() { app.reading_colors } else { pdfcraft_render::ReadingColors::BLACK_ON_WHITE };
        let mut changed = false;
        if let Some(params) = colors.params.as_mut() {
            ui.horizontal(|ui| {
                let text = ui.label(tl!("Text"));
                changed |= ui.color_edit_button_srgb(&mut params.text).labelled_by(text.id).changed();
                let background = ui.label(tl!("Page background"));
                changed |= ui.color_edit_button_srgb(&mut params.background).labelled_by(background.id).changed();
            });
            changed |= ui.add(crate::widgets::checkbox(&mut params.only_black, tl!("Only change the colour of black text or line art"))).changed();
            changed |= ui.add(crate::widgets::checkbox(&mut params.line_art, tl!("Change the colour of line art as well as text"))).changed();
        }
        if changed {
            app.set_reading_colors(colors);
        }
    });
    ui.label(RichText::new(tl!("Colours are remapped while rendering; the document itself never changes.")).small().color(t.text_muted));
}
